//! Incremental decoder for an upstream `text/event-stream` body.
//!
//! The decoder never buffers the whole response: it keeps one partial line plus
//! the current event's fields and yields events as blank lines arrive. It is
//! deliberately permissive about unknown fields and comments, and strict about
//! bounded memory (a producer that never sends a newline cannot grow the buffer
//! without limit).

use crate::error::HttpClientError;
use bytes::{BufMut, Bytes, BytesMut};
use futures_util::stream::{BoxStream, Stream, StreamExt};
use std::collections::VecDeque;
use std::pin::Pin;
use std::task::{Context, Poll};
use webx_core::stream::SseEvent;

/// Default cap for one unterminated line/event. A normal SSE event is a few KiB.
pub const DEFAULT_MAX_BUFFER: usize = 1024 * 1024;

/// A stream of decoded [`SseEvent`]s.
pub struct SseStream {
    source: BoxStream<'static, Result<Bytes, HttpClientError>>,
    buffer: BytesMut,
    data: BytesMut,
    event: Option<String>,
    id: Option<String>,
    retry: Option<u32>,
    has_data: bool,
    max_buffer: usize,
    pending: VecDeque<SseEvent>,
    done: bool,
}

impl SseStream {
    /// Decode `source` as SSE with [`DEFAULT_MAX_BUFFER`].
    pub fn new<S>(source: S) -> Self
    where
        S: Stream<Item = Result<Bytes, HttpClientError>> + Send + 'static,
    {
        Self::with_limit(source, DEFAULT_MAX_BUFFER)
    }

    /// Decode `source` as SSE with an explicit unterminated-buffer cap.
    pub fn with_limit<S>(source: S, max_buffer: usize) -> Self
    where
        S: Stream<Item = Result<Bytes, HttpClientError>> + Send + 'static,
    {
        Self {
            source: source.boxed(),
            buffer: BytesMut::new(),
            data: BytesMut::new(),
            event: None,
            id: None,
            retry: None,
            has_data: false,
            max_buffer: max_buffer.max(1),
            pending: VecDeque::new(),
            done: false,
        }
    }
}

impl Stream for SseStream {
    type Item = Result<SseEvent, HttpClientError>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();

        loop {
            if let Some(event) = this.pending.pop_front() {
                return Poll::Ready(Some(Ok(event)));
            }
            if this.done {
                return Poll::Ready(None);
            }

            // Consume every complete line currently buffered.
            while let Some(newline) = this.buffer.iter().position(|byte| *byte == b'\n') {
                let mut line = this.buffer.split_to(newline + 1);
                line.truncate(newline);
                if line.last() == Some(&b'\r') {
                    line.truncate(line.len() - 1);
                }
                let line = line.freeze();

                if line.is_empty() {
                    // Dispatch the event when the data buffer is non-empty.
                    if this.has_data {
                        let data = std::mem::take(&mut this.data).freeze();
                        this.pending.push_back(SseEvent {
                            event: this.event.take(),
                            id: this.id.take(),
                            data,
                            retry: this.retry.take(),
                        });
                        this.has_data = false;
                        break;
                    }
                    // A blank line with no data resets the event fields.
                    this.event = None;
                    this.id = None;
                    this.retry = None;
                    continue;
                }

                // Comment line: `: keep-alive`.
                if line.first() == Some(&b':') {
                    continue;
                }

                let (field, value) = match line.iter().position(|byte| *byte == b':') {
                    Some(colon) => (&line[..colon], &line[colon + 1..]),
                    None => (&line[..], &b""[..]),
                };
                // A single space after the colon is part of the framing.
                let value = value.strip_prefix(b" ").unwrap_or(value);

                match field {
                    b"event" => this.event = Some(String::from_utf8_lossy(value).into_owned()),
                    b"id" => this.id = Some(String::from_utf8_lossy(value).into_owned()),
                    b"retry" => {
                        this.retry = std::str::from_utf8(value).ok().and_then(|v| v.parse().ok());
                    }
                    b"data" => {
                        // `data` accumulates across lines until a blank line
                        // dispatches the event. Bound it independently of the
                        // partial-line buffer, otherwise an upstream that keeps
                        // sending `data:` lines without a terminator grows the
                        // vector without limit (memory DoS).
                        let extra = value.len().saturating_add(usize::from(this.has_data));
                        if this.data.len().saturating_add(extra) > this.max_buffer {
                            this.done = true;
                            return Poll::Ready(Some(Err(HttpClientError::BodyTooLarge {
                                limit: this.max_buffer,
                            })));
                        }
                        if this.has_data {
                            this.data.put_u8(b'\n');
                        }
                        this.data.put_slice(value);
                        this.has_data = true;
                    }
                    // Unknown fields are ignored, as the SSE spec requires.
                    _ => {}
                }
            }

            if let Some(event) = this.pending.pop_front() {
                return Poll::Ready(Some(Ok(event)));
            }

            match this.source.as_mut().poll_next(cx) {
                Poll::Ready(Some(Ok(chunk))) => {
                    if this.buffer.len().saturating_add(chunk.len()) > this.max_buffer {
                        this.done = true;
                        return Poll::Ready(Some(Err(HttpClientError::BodyTooLarge {
                            limit: this.max_buffer,
                        })));
                    }
                    this.buffer.put_slice(&chunk);
                }
                Poll::Ready(Some(Err(error))) => {
                    this.done = true;
                    return Poll::Ready(Some(Err(error)));
                }
                Poll::Ready(None) => {
                    // An event without its terminating blank line is discarded,
                    // exactly as the SSE spec requires.
                    this.done = true;
                    return Poll::Ready(None);
                }
                Poll::Pending => return Poll::Pending,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::stream;

    fn chunks(parts: &[&'static [u8]]) -> impl Stream<Item = Result<Bytes, HttpClientError>> {
        stream::iter(
            parts
                .iter()
                .map(|part| Ok(Bytes::from_static(part)))
                .collect::<Vec<_>>(),
        )
    }

    async fn decode(parts: &[&'static [u8]]) -> Vec<SseEvent> {
        SseStream::new(chunks(parts))
            .map(|event| event.expect("event decodes"))
            .collect()
            .await
    }

    #[tokio::test]
    async fn event_split_across_chunks_is_decoded_once() {
        let events = decode(&[b"data: hel", b"lo\n\n"]).await;
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].data, Bytes::from_static(b"hello"));
        assert!(events[0].event.is_none());
    }

    #[tokio::test]
    async fn multi_line_data_is_joined_with_lf() {
        let events = decode(&[b"data: first\ndata: second\n\n"]).await;
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].data, Bytes::from_static(b"first\nsecond"));
    }

    #[tokio::test]
    async fn crlf_framing_and_comments_are_accepted() {
        let events = decode(&[b": ping\r\nevent: tick\r\ndata: 1\r\n\r\n"]).await;
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event.as_deref(), Some("tick"));
        assert_eq!(events[0].data, Bytes::from_static(b"1"));
    }

    #[tokio::test]
    async fn id_and_retry_are_surfaced() {
        let events = decode(&[b"id: evt-1\nretry: 2500\ndata: x\n\n"]).await;
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].id.as_deref(), Some("evt-1"));
        assert_eq!(events[0].retry, Some(2500));
    }

    #[tokio::test]
    async fn event_without_data_is_not_dispatched() {
        let events = decode(&[b"event: metadata\n\nid: 7\n\n"]).await;
        assert!(events.is_empty());
    }

    #[tokio::test]
    async fn incomplete_trailing_event_is_discarded() {
        let events = decode(&[b"data: complete\n\ndata: incomplete"]).await;
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].data, Bytes::from_static(b"complete"));
    }

    #[tokio::test]
    async fn unterminated_line_is_bounded() {
        let mut stream = SseStream::with_limit(chunks(&[b"data: 0123456789"]), 8);
        let event = stream.next().await.expect("an error is produced");
        assert!(matches!(
            event,
            Err(HttpClientError::BodyTooLarge { limit: 8 })
        ));
    }

    #[tokio::test]
    async fn upstream_error_is_forwarded_and_ends_the_stream() {
        let source = stream::iter(vec![
            Ok(Bytes::from_static(b"data: one\n\n")),
            Err(HttpClientError::Connect("gone".into())),
            Ok(Bytes::from_static(b"data: two\n\n")),
        ]);
        let mut stream = SseStream::new(source);

        let first = stream.next().await.unwrap().unwrap();
        assert_eq!(first.data, Bytes::from_static(b"one"));
        let error = stream.next().await.unwrap().unwrap_err();
        assert!(matches!(error, HttpClientError::Connect(_)));
        assert!(stream.next().await.is_none());
    }

    #[test]
    fn default_limit_is_one_megabyte() {
        assert_eq!(DEFAULT_MAX_BUFFER, 1024 * 1024);
    }

    async fn decode_bytes(chunks: Vec<Bytes>) -> Vec<SseEvent> {
        let source = stream::iter(
            chunks
                .into_iter()
                .map(Ok::<Bytes, HttpClientError>)
                .collect::<Vec<_>>(),
        );
        SseStream::new(source)
            .map(|event| event.expect("event decodes"))
            .collect()
            .await
    }

    #[tokio::test]
    async fn randomized_event_sequences_survive_arbitrary_chunking() {
        // Deterministic pseudo-random payloads and chunk boundaries: the event
        // sequence must be identical no matter how the upstream splits bytes.
        let mut seed = 0x0ddc_0ffe_e0dd_u64;
        for _ in 0..100 {
            seed = seed
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            let event_count = 1 + (seed % 5) as usize;

            let mut wire = String::new();
            let mut expected = Vec::new();
            for _ in 0..event_count {
                seed = seed
                    .wrapping_mul(6364136223846793005)
                    .wrapping_add(1442695040888963407);
                let length = (seed % 40) as usize;
                let mut payload = String::with_capacity(length);
                for offset in 0..length {
                    seed = seed
                        .wrapping_mul(6364136223846793005)
                        .wrapping_add(1442695040888963407);
                    payload.push(char::from(b'a' + ((seed >> offset) % 26) as u8));
                }
                wire.push_str("data:");
                wire.push_str(&payload);
                wire.push_str("\n\n");
                expected.push(payload);
            }

            let bytes = Bytes::from(wire);
            let mut chunks = Vec::new();
            let mut offset = 0;
            while offset < bytes.len() {
                seed = seed
                    .wrapping_mul(6364136223846793005)
                    .wrapping_add(1442695040888963407);
                let size = 1 + (seed % 11) as usize;
                let end = (offset + size).min(bytes.len());
                chunks.push(bytes.slice(offset..end));
                offset = end;
            }

            let decoded: Vec<SseEvent> = SseStream::new(stream::iter(chunks.into_iter().map(Ok)))
                .map(|event| event.expect("event decodes"))
                .collect()
                .await;
            assert_eq!(decoded.len(), expected.len());
            for (event, payload) in decoded.iter().zip(expected.iter()) {
                assert_eq!(event.data, Bytes::from(payload.clone()));
            }
        }
    }

    #[tokio::test]
    async fn arbitrary_chunk_boundaries_preserve_the_events() {
        let payload = Bytes::from_static(
            b": ping\n\
              event: delta\n\
              id: evt-1\n\
              retry: 1500\n\
              data: hel\n\
              data: lo\n\
              \n\
              data: [DONE]\n\
              \n",
        );
        let baseline = decode_bytes(vec![payload.clone()]).await;
        assert_eq!(baseline.len(), 2);
        assert_eq!(baseline[0].event.as_deref(), Some("delta"));
        assert_eq!(baseline[0].data, Bytes::from_static(b"hel\nlo"));
        assert_eq!(baseline[1].data, Bytes::from_static(b"[DONE]"));

        for split in 0..=payload.len() {
            let chunks = vec![payload.slice(..split), payload.slice(split..)];
            assert_eq!(
                decode_bytes(chunks).await,
                baseline,
                "split at byte {split} must decode identically"
            );
        }

        // Deterministic pseudo-random multi-chunk splits (1..=7 bytes each).
        let mut seed = 0x5eed_u64;
        for _ in 0..100 {
            let mut chunks = Vec::new();
            let mut offset = 0;
            while offset < payload.len() {
                seed = seed
                    .wrapping_mul(6364136223846793005)
                    .wrapping_add(1442695040888963407);
                let size = 1 + (seed % 7) as usize;
                let end = (offset + size).min(payload.len());
                chunks.push(payload.slice(offset..end));
                offset = end;
            }
            assert_eq!(decode_bytes(chunks).await, baseline);
        }
    }
}
