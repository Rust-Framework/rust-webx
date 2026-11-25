//! `/ws/im` message-as-request protocol.
//!
//! One client frame is one request: the host binds the request/reply pair with
//! `use_websocket_request::<ImRequest, ImReply>("/ws/im")`, the framework
//! decodes the frame, dispatches it through the **same** handler pipeline HTTP
//! uses (per-message DI scope) and encodes the reply — or a structured error —
//! back as a JSON frame. [`ImEvent`] is the out-of-band push frame (room
//! fan-out) that has no request.

pub mod event;
pub mod reply;
pub mod request;

pub use event::ImEvent;
pub use reply::ImReply;
pub use request::ImRequest;
