//! Per-host direct-dispatch decision (F1 regression test).
//!
//! Before 2.2 the HTTP fast path was gated by a process-global counter, so any
//! host — or test — registering a typed pipeline behavior silently downgraded
//! every host in the process. The decision is now driven by a DI marker in the
//! host's own service collection.

use webx_core::error::ActionResult;
use webx_core::pipeline::{IPipelineBehavior, Next};
use webx_core::route::ext::IServiceCollectionExt;
use webx_host::server::Host;

#[derive(Default)]
struct NoopBehavior;

#[async_trait::async_trait]
impl IPipelineBehavior<u32, u32> for NoopBehavior {
    async fn handle(&self, req: u32, next: Next<u32, u32>) -> ActionResult<u32> {
        next.run(req).await
    }
}

fn build(with_typed_behavior: bool) -> Host {
    let builder = Host::builder()
        .mode(webx_core::mode::AppMode::Development)
        .no_spa();
    let builder = if with_typed_behavior {
        builder.register(|services| services.add_typed_pipeline::<NoopBehavior, u32, u32>())
    } else {
        builder
    };
    builder.try_build().expect("host should build")
}

#[tokio::test]
async fn typed_behavior_only_disables_its_own_host() {
    // A host built before any typed behavior keeps the fast path.
    let plain_first = build(false);
    assert!(
        plain_first.uses_direct_dispatch(),
        "a host without typed behaviors must keep the direct path"
    );

    // The host that registered a behavior must use the mediated path.
    let with_behavior = build(true);
    assert!(
        !with_behavior.uses_direct_dispatch(),
        "a host with a typed behavior must use the mediated path"
    );

    // A later host is not poisoned by the earlier registration.
    let plain_after = build(false);
    assert!(
        plain_after.uses_direct_dispatch(),
        "another host's typed behavior must not downgrade this host"
    );
}
