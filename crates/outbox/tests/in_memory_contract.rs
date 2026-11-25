//! The built-in in-memory store satisfies the shared `IOutbox` contract.

use std::sync::Arc;

use async_trait::async_trait;
use webx_outbox::{run_contract, IOutbox, InMemoryOutbox, OutboxContractSeeder};

struct MemorySeeder {
    outbox: Arc<InMemoryOutbox>,
}

impl MemorySeeder {
    fn new() -> Self {
        Self {
            outbox: Arc::new(InMemoryOutbox::new(6)),
        }
    }
}

#[async_trait]
impl OutboxContractSeeder for MemorySeeder {
    async fn prepare(&self) {}

    fn store(&self) -> Arc<dyn IOutbox> {
        Arc::clone(&self.outbox) as Arc<dyn IOutbox>
    }

    async fn seed(&self, id: &str, event_name: &str, payload: &str) {
        assert!(
            self.outbox.enqueue(id, event_name, payload),
            "seed ids are unique per case"
        );
    }
}

#[tokio::test]
async fn in_memory_store_satisfies_the_outbox_contract() {
    run_contract(MemorySeeder::new, 6).await;
}
