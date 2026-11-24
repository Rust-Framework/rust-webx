//! hyper ↔ neutral transport adapters (`pub(crate)`).
//!
//! The engine's neutral request representation is fed by this module when the
//! wire protocol is hyper (HTTP/1.1 + HTTP/2). It is deliberately private: a
//! third-party transport calls [`crate::Server::serve_transport_request`]
//! directly and never sees hyper types.
//!
//! Behavior notes:
//! * an empty hyper body becomes [`TransportRequestBody::Empty`] (a buffered
//!   empty body would still yield one empty chunk);
//! * trailer frames become [`TransportBodyFrame::Trailers`] instead of an
//!   error, so buffered readers can keep ignoring them while streaming readers
//!   keep rejecting them with the existing error text;
//! * the raw hyper error is boxed unchanged, preserving the existing
//!   `Display` text used in `problem+json`/logs.

use std::net::SocketAddr;

use futures_util::StreamExt;
use http_body_util::BodyStream;
use hyper::body::{Body, Incoming};

use crate::response::RespBody;
use crate::transport::{
    TransportBodyError, TransportBodyFrame, TransportRequest, TransportRequestBody,
    TransportRequestBodyStream, TransportResponse, TransportResponseBody, TransportUpgrade,
    UpgradedTransport,
};

/// Convert moved hyper request parts and body into a neutral request.
pub(crate) fn request(
    parts: http::request::Parts,
    body: Incoming,
    peer: Option<SocketAddr>,
) -> TransportRequest {
    TransportRequest::with_body(parts, request_body(body), peer)
}

/// Convert a live hyper body into the neutral body representation.
pub(crate) fn request_body(body: Incoming) -> TransportRequestBody {
    if body.is_end_stream() {
        return TransportRequestBody::Empty;
    }
    let exact_len = body.size_hint().exact();
    TransportRequestBody::streaming(request_body_stream(body), exact_len)
}

fn request_body_stream(body: Incoming) -> TransportRequestBodyStream {
    let frames = BodyStream::new(body).map(|frame| match frame {
        Ok(frame) => match frame.into_data() {
            Ok(data) => Ok(TransportBodyFrame::Data(data)),
            Err(frame) => Ok(TransportBodyFrame::Trailers(
                frame.into_trailers().unwrap_or_default(),
            )),
        },
        Err(error) => Err::<TransportBodyFrame, TransportBodyError>(Box::new(error)),
    });
    crate::transport::sync_body_stream(frames)
}

/// Convert the engine's finalized hyper response into a neutral response.
///
/// The existing `finalize()` pipeline (security headers, HEAD/204/304 body
/// suppression, file/range assembly, SSE lifecycle wrapper) is reused as-is;
/// only the body representation changes. A hyper path continues to send the
/// hyper response directly, so its bytes are unaffected (this path keeps that
/// byte-identical and lets non-hyper transports consume the neutral view).
pub(crate) fn response(response: hyper::Response<RespBody>) -> TransportResponse {
    let (parts, body) = response.into_parts();
    TransportResponse {
        status: parts.status.as_u16(),
        headers: parts.headers,
        body: response_body(body),
    }
}

fn response_body(body: RespBody) -> TransportResponseBody {
    if body.is_end_stream() {
        return TransportResponseBody::None;
    }
    let stream = BodyStream::new(body).filter_map(|frame| async move {
        match frame {
            Ok(frame) => frame.into_data().ok().map(Ok),
            Err(error) => Some(Err(error)),
        }
    });
    TransportResponseBody::Stream(Box::pin(stream))
}

/// Wrap hyper's `OnUpgrade` future as a neutral upgrade.
pub(crate) fn upgrade(on_upgrade: hyper::upgrade::OnUpgrade) -> TransportUpgrade {
    let io = Box::pin(async move {
        let upgraded = on_upgrade.await?;
        let transport: UpgradedTransport = Box::new(hyper_util::rt::TokioIo::new(upgraded));
        Ok(transport)
    });
    TransportUpgrade { io }
}
