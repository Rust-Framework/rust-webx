//! Reserved realtime fan-out events must not be claimed by an application
//! `#[handler]`: the dispatcher prefers inventory registrations over DI, so the
//! framework bridge would be silently replaced. Host construction fails closed.
//!
//! This file is a separate test binary on purpose: `inventory` registrations are
//! global per binary, and this one deliberately submits a reserved-event handler.

use std::any::{Any, TypeId};
use std::sync::Arc;

use webx_core::error::{Error, Result};
use webx_core::mediator::event::{BoxedEventHandlerCall, EventHandlerRegistration};
use webx_core::realtime::RealtimeFanoutEvent;
use webx_host::server::Host;

fn factory(_resolver: &dyn rust_dix::IServiceResolver) -> Result<Arc<dyn Any + Send + Sync>> {
    Err(Error::internal(
        "the reserved-event test handler is never constructed",
    ))
}

fn call(
    _handler: Arc<dyn Any + Send + Sync>,
    _event: Box<dyn Any + Send>,
) -> BoxedEventHandlerCall {
    Box::pin(async { Ok(()) })
}

// Mimics the two inventory items emitted by `#[handler]` on an
// `impl IEventHandler<RealtimeFanoutEvent>` block. The `EventRegistration`
// half is part of the macro output too; submitting only the handler registration
// is enough for the reserved-name check, and keeps this test focused.
inventory::submit! {
    EventHandlerRegistration {
        event_name: || RealtimeFanoutEvent::EVENT_NAME,
        event_type_id: TypeId::of::<RealtimeFanoutEvent>(),
        handler_name: "reserved_event_handler_test::HijackFanout",
        factory,
        call,
    }
}

#[test]
fn a_reserved_fanout_handler_fails_host_build() {
    let error = match Host::builder()
        .mode(webx_core::mode::AppMode::Development)
        .no_spa()
        .try_build()
    {
        Ok(_) => panic!("host build must reject a handler for a reserved event"),
        Err(error) => error,
    };
    let message = error.to_string();
    assert!(
        message.contains("framework-reserved"),
        "the error must explain the reserved event rule, got: {message}"
    );
    assert!(
        message.contains("webx.realtime.fanout"),
        "the error must name the reserved event, got: {message}"
    );
}
