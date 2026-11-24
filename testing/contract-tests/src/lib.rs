//! Reusable contract suites for rust-webx ports (2.4-7 starter family).
//!
//! Suites: [`object_store_contract`], [`media_plane_contract`] and the
//! event-transport suite ([`event_transport::EventTransportContract`], 2.5).
//!
//! Every adapter — the in-memory implementations shipped with core, and the
//! external starters planned for 2.5 (S3 object storage, SFU media plane) —
//! must pass the same suite. An adapter crate depends on this crate as a
//! dev-dependency and runs the suite against its own instance:
//!
//! ```ignore
//! #[tokio::test]
//! async fn s3_store_satisfies_the_contract() {
//!     let store = S3ObjectStore::connect(&config).await.expect("s3");
//!     webx_contract_tests::object_store_contract(&store).await;
//!     // The adapter's own behaviour can be asserted after the shared suite.
//! }
//! ```
//!
//! The suites are deliberately behavioural: they do not assert implementation
//! details (ordering, batching, wire formats), only the port's documented
//! contract.

pub mod event_transport;

use webx_core::media::{IMediaPlaneAdapter, MediaStats, MediaTrackInfo, MediaTrackKind};
use webx_core::storage::{IObjectStore, PartEtag};

pub use event_transport::{EventTransportContract, EventTransportReport};

// ---------------------------------------------------------------------------
// IObjectStore
// ---------------------------------------------------------------------------

/// Contract for [`IObjectStore`].
///
/// Asserts: whole-object put/get/head/delete, deterministic metadata, resumable
/// uploads (parts, retry of one part, consecutive-part requirement), abort, and
/// key-traversal rejection.
pub async fn object_store_contract(store: &dyn IObjectStore) {
    let prefix = "contract-tests/object-store";
    let meta = store
        .put(
            &format!("{prefix}/cover.json"),
            br#"{"ok":true}"#.to_vec(),
            "application/json",
        )
        .await
        .expect("put object");
    assert_eq!(meta.size, 11);
    assert_eq!(meta.content_type, "application/json");
    assert!(!meta.etag.is_empty(), "content hash must be reported");

    let object = store
        .get(&format!("{prefix}/cover.json"))
        .await
        .expect("get object")
        .expect("object exists");
    assert_eq!(object.bytes, br#"{"ok":true}"#.to_vec());
    assert!(store
        .head(&format!("{prefix}/cover.json"))
        .await
        .expect("head object")
        .is_some());

    // Resumable upload: retrying a part is safe and lists the committed parts.
    let upload = store
        .begin_upload(&format!("{prefix}/blob"), "audio/ogg")
        .await
        .expect("begin upload");
    store
        .put_part(&upload, 1, b"hello-".to_vec())
        .await
        .expect("part 1");
    store
        .put_part(&upload, 2, b"wrong".to_vec())
        .await
        .expect("part 2 first attempt");
    store
        .put_part(&upload, 2, b"world".to_vec())
        .await
        .expect("part 2 retry");
    let parts: Vec<PartEtag> = store.list_parts(&upload).await.expect("list parts");
    assert_eq!(parts.len(), 2, "retried part does not duplicate");
    assert_eq!(parts[1].size, 5);
    let completed = store
        .complete_upload(upload)
        .await
        .expect("complete upload");
    assert_eq!(completed.size, 11);
    assert_eq!(completed.content_type, "audio/ogg");
    let blob = store
        .get(&format!("{prefix}/blob"))
        .await
        .expect("get blob")
        .expect("blob exists");
    assert_eq!(blob.bytes, b"hello-world", "parts concatenate in order");

    // A gap in the part sequence is a validation error.
    let gap = store
        .begin_upload(&format!("{prefix}/gap"), "audio/ogg")
        .await
        .expect("begin gap upload");
    store
        .put_part(&gap, 2, b"x".to_vec())
        .await
        .expect("part 2");
    assert!(
        matches!(store.complete_upload(gap).await, Err(error) if error.code() == "validation_failed"),
        "non-consecutive parts must be rejected"
    );

    // Abort drops the session.
    let aborted = store
        .begin_upload(&format!("{prefix}/aborted"), "audio/ogg")
        .await
        .expect("begin aborted upload");
    store.abort_upload(aborted.clone()).await.expect("abort");
    assert!(
        matches!(store.list_parts(&aborted).await, Err(error) if error.code() == "not_found"),
        "aborted upload is gone"
    );

    // Key traversal is rejected by every adapter.
    assert!(
        matches!(
            store.put("../escape", Vec::new(), "text/plain").await,
            Err(error) if error.code() == "validation_failed"
        ),
        "path traversal keys must be rejected"
    );

    store
        .delete(&format!("{prefix}/cover.json"))
        .await
        .expect("delete");
    assert!(store
        .get(&format!("{prefix}/cover.json"))
        .await
        .expect("get deleted")
        .is_none());
}

// ---------------------------------------------------------------------------
// IMediaPlaneAdapter
// ---------------------------------------------------------------------------

/// Contract for [`IMediaPlaneAdapter`].
///
/// Asserts: session lifecycle, track publication/subscription replacement,
/// 404 semantics for unknown sessions, baseline stats shape, idempotent leave
/// and room teardown.
pub async fn media_plane_contract(adapter: &dyn IMediaPlaneAdapter) {
    let first = adapter.join("room/contract", "alice").await.expect("join");
    let second = adapter.join("room/contract", "bob").await.expect("join");
    assert_ne!(first.session_id, second.session_id);
    assert_eq!(first.room_id, "room/contract");
    assert_eq!(first.user_id, "alice");
    assert!(
        matches!(adapter.join("", "eve").await, Err(error) if error.code() == "validation_failed"),
        "empty room/user must be rejected"
    );

    adapter
        .publish(
            &first.session_id,
            vec![MediaTrackInfo::new("cam", MediaTrackKind::Video)],
        )
        .await
        .expect("publish");
    adapter
        .subscribe(
            &first.session_id,
            vec![MediaTrackInfo::new("remote", MediaTrackKind::Video)],
        )
        .await
        .expect("subscribe");
    assert!(
        matches!(
            adapter
                .publish(
                    "missing",
                    vec![MediaTrackInfo::new("x", MediaTrackKind::Audio)]
                )
                .await,
            Err(error) if error.code() == "not_found"
        ),
        "unknown session is a not-found"
    );

    let stats: MediaStats = adapter.stats(&first.session_id).await.expect("stats");
    assert!(stats.rtt_ms >= 0.0);
    assert!((0.0..=1.0).contains(&stats.packet_loss_ratio));
    assert!(
        matches!(adapter.stats("missing").await, Err(error) if error.code() == "not_found"),
        "unknown session has no stats"
    );

    adapter.leave(&first.session_id).await.expect("leave");
    adapter
        .leave(&first.session_id)
        .await
        .expect("leave is idempotent");
    assert!(
        matches!(
            adapter.stats(&first.session_id).await,
            Err(error) if error.code() == "not_found"
        ),
        "left session is gone"
    );

    adapter
        .close_room("room/contract")
        .await
        .expect("close room");
    assert!(
        matches!(
            adapter.stats(&second.session_id).await,
            Err(error) if error.code() == "not_found"
        ),
        "close_room tears down every session"
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use webx_core::media::InMemoryMediaPlaneAdapter;
    use webx_core::storage::InMemoryObjectStore;

    #[tokio::test]
    async fn in_memory_object_store_satisfies_the_contract() {
        let store = InMemoryObjectStore::new();
        object_store_contract(&store).await;
    }

    #[tokio::test]
    async fn in_memory_media_plane_satisfies_the_contract() {
        let adapter = InMemoryMediaPlaneAdapter::new();
        media_plane_contract(&adapter).await;
    }
}
