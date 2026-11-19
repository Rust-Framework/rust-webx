//! Server-sent events (SSE) as a first-class streaming response.
//!
//! `SseResult` is an [`IActionResult`] for `text/event-stream`: the endpoint
//! declares `IRequest<Response>` and the handler returns
//! `Ok(Box::new(SseResult::new(events)))`. The host streams the encoded frames
//! with chunked transfer encoding, so each event reaches the client as soon as
//! the source stream produces it.
//!
//! ```ignore
//! #[post("/events")]
//! impl IRequest<Response> for EventsRequest {}
//!
//! #[handler]
//! #[async_trait::async_trait]
//! impl IRequestHandler<EventsRequest, Response> for EventsHandler {
//!     async fn handle(&mut self, _req: EventsRequest) -> ActionResult<Response> {
//!         let events = futures_util::stream::iter([
//!             Ok(SseEvent::data("hello")),
//!             Ok(SseEvent::event("tick", "world")),
//!         ]);
//!         Ok(Box::new(SseResult::new(events)))
//!     }
//! }
//! ```
//!
//! Lifecycle:
//!
//! * The response is always `200` with `cache-control: no-store` and
//!   `x-accel-buffering: no`; add or override headers with
//!   [`SseResult::header`].
//! * [`SseResult::on_close`] runs **exactly once** when the stream ends. A
//!   normal end reports [`StreamEnd::Completed`]; a client disconnect (the host
//!   drops the response body) reports [`StreamEnd::ClientDisconnected`]. The
//!   hook is synchronous and must not block — spawn settlement work.
//! * [`SseResult::keep_alive`] emits `: ping` comment frames while the source
//!   stream is idle, which keeps proxies from closing a quiet connection.
//! * [`SseResult::on_server_shutdown`] closes the stream with a final
//!   `event: server_shutdown` frame as soon as the host starts draining, so a
//!   deployment finishes long-lived streams instead of cutting them off at the
//!   drain deadline. The hook then reports [`StreamEnd::ServerShutdown`].
//! * [`SseResult::max_lifetime`] bounds a stream that never ends on its own; on
//!   expiry the stream closes cleanly and reports [`StreamEnd::Deadline`].
//!
//! An `Err` from the source stream ends the stream cleanly and is reported as
//! [`StreamEnd::Failed`]. Convert provider errors into an [`SseEvent`] first if
//! the client must see them as data.

use crate::action::IActionResult;
use crate::error::{Error, Result};
use crate::hosted::CancellationToken;
use crate::http::{FileBody, IHttpContext, ResponseBody};
use bytes::{BufMut, Bytes, BytesMut};
use futures_util::stream::{BoxStream, Stream, StreamExt};
use std::future::Future;
use std::io;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;
use tokio::time::Sleep;
use tokio_util::io::StreamReader;

/// One server-sent event.
///
/// `data` is split on `\n` when encoded, so multi-line payloads produce one
/// `data:` line per line (the SSE wire format cannot carry a raw newline inside
/// a single field).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SseEvent {
    /// Optional `event:` name (for example `"message"` or `"error"`).
    pub event: Option<String>,
    /// Optional `id:` used by clients that resume with `Last-Event-ID`.
    pub id: Option<String>,
    /// Payload bytes. Empty data still emits one empty `data:` line.
    pub data: Bytes,
    /// Optional `retry:` hint in milliseconds.
    pub retry: Option<u32>,
}

impl SseEvent {
    /// A data-only event (`data: ...`).
    pub fn data(data: impl Into<Bytes>) -> Self {
        Self {
            data: data.into(),
            ..Self::default()
        }
    }

    /// An event with a name (`event: ...` followed by `data: ...`).
    pub fn event(event: impl Into<String>, data: impl Into<Bytes>) -> Self {
        Self {
            event: Some(event.into()),
            data: data.into(),
            ..Self::default()
        }
    }

    /// Set the `id:` field.
    pub fn with_id(mut self, id: impl Into<String>) -> Self {
        self.id = Some(id.into());
        self
    }

    /// Set the `retry:` hint (milliseconds).
    pub fn with_retry(mut self, retry: u32) -> Self {
        self.retry = Some(retry);
        self
    }

    /// Encode this event into the SSE wire format.
    ///
    /// `event` and `id` must not contain `\r`, `\n` or NUL; a value that does is
    /// rejected instead of being allowed to inject extra protocol lines.
    fn encode(&self, out: &mut BytesMut) -> Result<()> {
        if let Some(event) = &self.event {
            validate_field("event", event)?;
            out.put_slice(b"event:");
            out.put_slice(event.as_bytes());
            out.put_u8(b'\n');
        }

        if let Some(id) = &self.id {
            validate_field("id", id)?;
            out.put_slice(b"id:");
            out.put_slice(id.as_bytes());
            out.put_u8(b'\n');
        }

        if let Some(retry) = self.retry {
            out.put_slice(b"retry:");
            out.put_slice(retry.to_string().as_bytes());
            out.put_u8(b'\n');
        }

        // Even empty data emits one `data:` line: a blank line alone would end
        // the previous event instead of dispatching this one.
        for line in self.data.split(|byte| *byte == b'\n') {
            out.put_slice(b"data:");
            // SSE ignores a `\r` immediately before `\n`; normalize it so the
            // client sees the same payload the caller supplied.
            let line = line.strip_suffix(b"\r").unwrap_or(line);
            out.put_slice(line);
            out.put_u8(b'\n');
        }

        out.put_u8(b'\n');
        Ok(())
    }
}

fn validate_field(name: &str, value: &str) -> Result<()> {
    if value.bytes().any(|byte| matches!(byte, b'\r' | b'\n' | 0)) {
        return Err(Error::Validation(format!(
            "SSE {name} field must not contain CR, LF or NUL"
        )));
    }
    Ok(())
}

/// Why an SSE stream ended.
#[derive(Debug)]
pub enum StreamEnd {
    /// The source stream ended normally.
    Completed,
    /// The client went away (or the response body was dropped) before the
    /// source stream completed.
    ClientDisconnected,
    /// The source stream produced an error.
    Failed(Error),
    /// The configured [`SseResult::max_lifetime`] elapsed.
    Deadline,
    /// The host started graceful shutdown; a final `event: server_shutdown`
    /// frame was written before closing.
    ServerShutdown,
}

/// A `text/event-stream` response.
///
/// Build one from any `Stream<Item = Result<SseEvent>>` and return it from an
/// `IRequest<Response>` handler. See the [module docs](self) for the lifecycle.
pub struct SseResult {
    events: BoxStream<'static, Result<SseEvent>>,
    keep_alive: Option<Duration>,
    max_lifetime: Option<Duration>,
    shutdown: Option<CancellationToken>,
    on_close: Option<Box<dyn FnOnce(StreamEnd) + Send>>,
    headers: Vec<(String, String)>,
}

impl SseResult {
    /// Stream `events` as `text/event-stream`.
    ///
    /// The stream is polled by the HTTP host: a slow client applies backpressure
    /// to the source stream instead of buffering the whole response.
    pub fn new<S>(events: S) -> Self
    where
        S: Stream<Item = Result<SseEvent>> + Send + 'static,
    {
        Self {
            events: events.boxed(),
            keep_alive: None,
            max_lifetime: None,
            shutdown: None,
            on_close: None,
            headers: Vec::new(),
        }
    }

    /// Emit a `: ping` comment frame whenever the source stream stays idle for
    /// `every`. A zero duration disables keep-alive.
    pub fn keep_alive(mut self, every: Duration) -> Self {
        if !every.is_zero() {
            self.keep_alive = Some(every);
        }
        self
    }

    /// Close the stream (and report `StreamEnd::Deadline`) after `lifetime`.
    ///
    /// A zero duration disables the bound. Keep-alive and max-lifetime are
    /// independent: keep-alive works on idle gaps, max-lifetime on total age.
    pub fn max_lifetime(mut self, lifetime: Duration) -> Self {
        if !lifetime.is_zero() {
            self.max_lifetime = Some(lifetime);
        }
        self
    }

    /// Run `hook` exactly once when the stream ends.
    ///
    /// The hook is synchronous; it may `tokio::spawn` asynchronous work (usage
    /// settlement, quota release). It is invoked from the stream's `Drop` when
    /// the client disconnects, so it must not panic.
    pub fn on_close(mut self, hook: impl FnOnce(StreamEnd) + Send + 'static) -> Self {
        self.on_close = Some(Box::new(hook));
        self
    }

    /// Finish the stream when the host starts graceful shutdown.
    ///
    /// The handler obtains the host token by injecting
    /// [`ShutdownToken`](crate::hosted::ShutdownToken) and passes
    /// [`shutdown.token()`](crate::hosted::ShutdownToken::token). When the token
    /// is cancelled the stream writes one final
    /// `event: server_shutdown\ndata: {}\n\n` frame, closes cleanly and reports
    /// [`StreamEnd::ServerShutdown`] — instead of being cut at the drain
    /// deadline. Clients can treat the event as "reconnect after the rollout".
    pub fn on_server_shutdown(mut self, token: CancellationToken) -> Self {
        self.shutdown = Some(token);
        self
    }

    /// Add a response header. Custom headers are applied after the framework's
    /// SSE defaults, so they can override `content-type`/`cache-control`.
    pub fn header(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers.push((key.into(), value.into()));
        self
    }

    /// Resolve the response into the chunked body stream.
    fn into_body(self) -> impl tokio::io::AsyncRead + Send + Unpin + 'static {
        let SseResult {
            events,
            keep_alive,
            max_lifetime,
            shutdown,
            on_close,
            ..
        } = self;

        StreamReader::new(SseBodyStream {
            events,
            pending: BytesMut::new(),
            keep_alive: keep_alive.map(|every| Box::pin(tokio::time::sleep(every))),
            keep_alive_every: keep_alive,
            max_lifetime: max_lifetime.map(|lifetime| Box::pin(tokio::time::sleep(lifetime))),
            shutdown,
            finished: false,
            guard: CompletionGuard {
                hook: on_close,
                fired: false,
            },
        })
    }
}

#[async_trait::async_trait]
impl IActionResult for SseResult {
    async fn execute(mut self: Box<Self>, ctx: &mut dyn IHttpContext) -> Result<()> {
        // `into_body` consumes the stream/close state; headers are plain data.
        let headers = std::mem::take(&mut self.headers);
        let body = (*self).into_body();

        let response = ctx.response_mut();
        response.set_status(200);
        response.set_header("content-type", "text/event-stream; charset=utf-8");
        response.set_header("cache-control", "no-store");
        response.set_header("x-accel-buffering", "no");
        for (key, value) in &headers {
            response.set_header(key, value);
        }
        response
            .write_body(ResponseBody::File(FileBody::stream(body)))
            .await
    }
}

/// Encoded frames plus lifecycle state, polled by [`StreamReader`].
struct SseBodyStream {
    events: BoxStream<'static, Result<SseEvent>>,
    pending: BytesMut,
    keep_alive: Option<Pin<Box<Sleep>>>,
    keep_alive_every: Option<Duration>,
    max_lifetime: Option<Pin<Box<Sleep>>>,
    shutdown: Option<CancellationToken>,
    finished: bool,
    guard: CompletionGuard,
}

impl SseBodyStream {
    fn finish(&mut self, end: StreamEnd) {
        if self.finished {
            return;
        }
        self.finished = true;
        self.guard.finish(end);
    }
}

impl Stream for SseBodyStream {
    type Item = io::Result<Bytes>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();

        loop {
            if this.finished {
                return Poll::Ready(None);
            }

            if !this.pending.is_empty() {
                return Poll::Ready(Some(Ok(this.pending.split().freeze())));
            }

            // Graceful shutdown wins over new events: stop producing tokens and
            // tell the client why, so it can reconnect against the next instance.
            if this
                .shutdown
                .as_ref()
                .is_some_and(|token| token.is_cancelled())
            {
                this.pending
                    .put_slice(b"event:server_shutdown\ndata:{}\n\n");
                this.finished = true;
                this.guard.finish(StreamEnd::ServerShutdown);
                return Poll::Ready(Some(Ok(this.pending.split().freeze())));
            }

            let lifetime_elapsed = this
                .max_lifetime
                .as_mut()
                .map(|sleep| sleep.as_mut().poll(cx).is_ready())
                .unwrap_or(false);
            if lifetime_elapsed {
                this.finish(StreamEnd::Deadline);
                return Poll::Ready(None);
            }

            match this.events.as_mut().poll_next(cx) {
                Poll::Ready(Some(Ok(event))) => match event.encode(&mut this.pending) {
                    Ok(()) => continue,
                    Err(error) => {
                        this.finish(StreamEnd::Failed(error));
                        return Poll::Ready(None);
                    }
                },
                Poll::Ready(Some(Err(error))) => {
                    this.finish(StreamEnd::Failed(error));
                    return Poll::Ready(None);
                }
                Poll::Ready(None) => {
                    this.finish(StreamEnd::Completed);
                    return Poll::Ready(None);
                }
                Poll::Pending => {}
            }

            let pinged = this
                .keep_alive
                .as_mut()
                .map(|sleep| sleep.as_mut().poll(cx).is_ready())
                .unwrap_or(false);
            if pinged {
                if let Some(every) = this.keep_alive_every {
                    this.pending.put_slice(b": ping\n\n");
                    this.keep_alive = Some(Box::pin(tokio::time::sleep(every)));
                    continue;
                }
            }

            return Poll::Pending;
        }
    }
}

impl Drop for SseBodyStream {
    fn drop(&mut self) {
        if !self.finished {
            self.finished = true;
            self.guard.finish(StreamEnd::ClientDisconnected);
        }
    }
}

/// Runs the close hook exactly once, whichever path ends the stream.
struct CompletionGuard {
    hook: Option<Box<dyn FnOnce(StreamEnd) + Send>>,
    fired: bool,
}

impl CompletionGuard {
    fn finish(&mut self, end: StreamEnd) {
        if self.fired {
            return;
        }
        self.fired = true;
        if let Some(hook) = self.hook.take() {
            hook(end);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::stream;
    use std::sync::mpsc;
    use tokio::io::AsyncReadExt;

    fn encode(event: &SseEvent) -> Vec<u8> {
        let mut out = BytesMut::new();
        event.encode(&mut out).expect("event encodes");
        out.to_vec()
    }

    #[test]
    fn data_only_event_is_one_data_line() {
        assert_eq!(encode(&SseEvent::data("hello")), b"data:hello\n\n");
    }

    #[test]
    fn multiline_data_is_split_into_data_lines() {
        assert_eq!(
            encode(&SseEvent::data("first\nsecond")),
            b"data:first\ndata:second\n\n"
        );
    }

    #[test]
    fn event_id_and_retry_are_written_in_wire_order() {
        let event = SseEvent::event("tick", "1")
            .with_id("evt-7")
            .with_retry(1500);
        assert_eq!(
            encode(&event),
            b"event:tick\nid:evt-7\nretry:1500\ndata:1\n\n"
        );
    }

    #[test]
    fn empty_data_still_emits_one_data_line() {
        assert_eq!(encode(&SseEvent::data(Bytes::new())), b"data:\n\n");
    }

    #[test]
    fn crlf_payload_is_normalized_to_lf_lines() {
        assert_eq!(encode(&SseEvent::data("a\r\nb")), b"data:a\ndata:b\n\n");
    }

    #[test]
    fn newline_in_event_name_is_rejected_instead_of_injecting_a_line() {
        let mut out = BytesMut::new();
        let error = SseEvent::event("bad\nname", "x")
            .encode(&mut out)
            .expect_err("a newline in the event name must be rejected");
        assert!(matches!(error, Error::Validation(_)));
        assert!(out.is_empty(), "nothing is written before validation fails");
    }

    #[tokio::test]
    async fn dropping_the_body_reports_client_disconnected() {
        let (tx, rx) = mpsc::channel();
        let result = SseResult::new(stream::empty()).on_close(move |end| {
            let _ = tx.send(matches!(end, StreamEnd::ClientDisconnected));
        });

        let body = result.into_body();
        drop(body);

        assert!(rx.recv_timeout(Duration::from_secs(1)).unwrap());
    }

    #[tokio::test]
    async fn completed_stream_fires_the_close_hook_once() {
        let (tx, rx) = mpsc::channel();
        let events = stream::iter([Ok(SseEvent::data("x"))]);
        let result = SseResult::new(events).on_close(move |end| {
            let _ = tx.send(matches!(end, StreamEnd::Completed));
        });

        let mut body = result.into_body();
        let mut bytes = Vec::new();
        body.read_to_end(&mut bytes).await.expect("body reads");

        assert_eq!(bytes, b"data:x\n\n");
        assert!(rx.recv_timeout(Duration::from_secs(1)).unwrap());
    }

    #[tokio::test]
    async fn keep_alive_writes_a_comment_frame_while_idle() {
        let result = SseResult::new(stream::pending())
            .keep_alive(Duration::from_millis(5))
            .max_lifetime(Duration::from_secs(5));

        let mut body = result.into_body();
        let mut buf = [0u8; 32];
        let read = tokio::time::timeout(Duration::from_secs(1), body.read(&mut buf))
            .await
            .expect("keep-alive frame arrives")
            .expect("body reads");

        assert_eq!(&buf[..read], b": ping\n\n");
    }

    #[tokio::test]
    async fn max_lifetime_ends_a_pending_stream() {
        let (tx, rx) = mpsc::channel();
        let result = SseResult::new(stream::pending())
            .max_lifetime(Duration::from_millis(20))
            .on_close(move |end| {
                let _ = tx.send(matches!(end, StreamEnd::Deadline));
            });

        let mut body = result.into_body();
        let mut buf = [0u8; 8];
        let read = tokio::time::timeout(Duration::from_secs(1), body.read(&mut buf))
            .await
            .expect("max lifetime fires")
            .expect("body reads");

        assert_eq!(read, 0);
        assert!(rx.recv_timeout(Duration::from_secs(1)).unwrap());
    }

    #[tokio::test]
    async fn server_shutdown_writes_a_final_event_and_fires_the_hook_once() {
        let (tx, rx) = mpsc::channel();
        let token = CancellationToken::new();
        let result = SseResult::new(stream::pending())
            .on_server_shutdown(token.clone())
            .on_close(move |end| {
                let _ = tx.send(matches!(end, StreamEnd::ServerShutdown));
            });

        let mut body = result.into_body();
        token.cancel();

        let mut bytes = Vec::new();
        tokio::time::timeout(Duration::from_secs(1), body.read_to_end(&mut bytes))
            .await
            .expect("the shutdown frame is delivered")
            .expect("body reads");

        assert_eq!(bytes, b"event:server_shutdown\ndata:{}\n\n");
        assert!(rx.recv_timeout(Duration::from_secs(1)).unwrap());
    }

    #[tokio::test]
    async fn server_shutdown_flushes_pending_frames_before_closing() {
        let token = CancellationToken::new();
        let events = stream::iter([Ok(SseEvent::data("first"))]);
        let result = SseResult::new(events).on_server_shutdown(token.clone());

        let mut body = result.into_body();
        let mut first = [0u8; 32];
        let read = body.read(&mut first).await.expect("body reads");
        assert_eq!(&first[..read], b"data:first\n\n");

        token.cancel();
        let mut rest = Vec::new();
        body.read_to_end(&mut rest).await.expect("body reads");
        assert_eq!(rest, b"event:server_shutdown\ndata:{}\n\n");
    }
}
