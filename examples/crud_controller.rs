//! crud_controller — Full CRUD API with in-memory storage + logging middleware.
//!
//! Demonstrates multiple HTTP methods (`#[get]`, `#[post]`, `#[put]`, `#[delete]`),
//! path parameters (`{id}`), JSON request/response bodies, and a custom middleware.
//!
//! Run with: `cargo run --example crud_controller`

use lrwf::*;
use std::collections::HashMap;
use std::sync::Mutex;

// ── Domain model ──────────────────────────────────────────────────────────

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct ItemModel {
    id: String,
    name: String,
    description: String,
}

// ── In-memory repository ──────────────────────────────────────────────────

struct ItemRepository {
    items: Mutex<HashMap<String, ItemModel>>,
}

impl ItemRepository {
    fn new() -> Self {
        Self {
            items: Mutex::new(HashMap::new()),
        }
    }

    fn list(&self) -> Vec<ItemModel> {
        self.items
            .lock()
            .map(|m| m.values().cloned().collect())
            .unwrap_or_default()
    }

    fn get(&self, id: &str) -> Option<ItemModel> {
        self.items.lock().ok()?.get(id).cloned()
    }

    fn create(&self, name: &str, desc: &str) -> ItemModel {
        let id = format!(
            "{:x}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        );
        let item = ItemModel {
            id: id.clone(),
            name: name.into(),
            description: desc.into(),
        };
        self.items
            .lock()
            .map(|mut m| {
                m.insert(id, item.clone());
            })
            .ok();
        item
    }

    fn update(&self, id: &str, name: Option<&str>, desc: Option<&str>) -> Option<ItemModel> {
        let mut items = self.items.lock().ok()?;
        let item = items.get_mut(id)?;
        if let Some(n) = name {
            item.name = n.into();
        }
        if let Some(d) = desc {
            item.description = d.into();
        }
        Some(item.clone())
    }

    fn delete(&self, id: &str) -> bool {
        self.items
            .lock()
            .map(|mut m| m.remove(id).is_some())
            .unwrap_or(false)
    }
}

static REPO: std::sync::LazyLock<ItemRepository> = std::sync::LazyLock::new(ItemRepository::new);

// ── LoggingMiddleware ─────────────────────────────────────────────────────

/// Simple logging middleware that prints each request method + path.
#[derive(Default)]
struct LoggingMiddleware;

#[async_trait]
impl IMiddleware for LoggingMiddleware {
    async fn invoke(&self, ctx: &mut dyn IHttpContext) -> Result<()> {
        println!("[LOG] {} {}", ctx.request().method(), ctx.request().path());
        Ok(())
    }
}

// ── Request / Response contracts ──────────────────────────────────────────

struct ListItemsRequest;

#[get("/api/items")]
impl IRequest<Vec<ItemModel>> for ListItemsRequest {}

struct GetItemRequest {
    id: String,
}

#[get("/api/items/{id}")]
impl IRequest<ItemModel> for GetItemRequest {}

#[derive(serde::Deserialize)]
struct CreateItemRequest {
    name: String,
    description: String,
}

#[post("/api/items")]
impl IRequest<ItemModel> for CreateItemRequest {}

#[derive(serde::Deserialize)]
struct UpdateItemRequest {
    id: String,
    name: Option<String>,
    description: Option<String>,
}

#[put("/api/items/{id}")]
impl IRequest<ItemModel> for UpdateItemRequest {}

struct DeleteItemRequest {
    id: String,
}

#[delete("/api/items/{id}")]
impl IRequest<String> for DeleteItemRequest {}

// ── Handlers ──────────────────────────────────────────────────────────────

#[derive(Default)]
struct ListItemsHandler;
#[derive(Default)]
struct GetItemHandler;
#[derive(Default)]
struct CreateItemHandler;
#[derive(Default)]
struct UpdateItemHandler;
#[derive(Default)]
struct DeleteItemHandler;

#[handler]
#[async_trait]
impl IRequestHandler<ListItemsRequest, Vec<ItemModel>> for ListItemsHandler {
    async fn handle(&self, _req: ListItemsRequest) -> Result<Vec<ItemModel>> {
        Ok(REPO.list())
    }
}

#[handler]
#[async_trait]
impl IRequestHandler<GetItemRequest, ItemModel> for GetItemHandler {
    async fn handle(&self, req: GetItemRequest) -> Result<ItemModel> {
        REPO.get(&req.id)
            .ok_or_else(|| Error::NotFound(format!("Item not found: {}", req.id)))
    }
}

#[handler]
#[async_trait]
impl IRequestHandler<CreateItemRequest, ItemModel> for CreateItemHandler {
    async fn handle(&self, req: CreateItemRequest) -> Result<ItemModel> {
        Ok(REPO.create(&req.name, &req.description))
    }
}

#[handler]
#[async_trait]
impl IRequestHandler<UpdateItemRequest, ItemModel> for UpdateItemHandler {
    async fn handle(&self, req: UpdateItemRequest) -> Result<ItemModel> {
        REPO.update(&req.id, req.name.as_deref(), req.description.as_deref())
            .ok_or_else(|| Error::NotFound(format!("Item not found: {}", req.id)))
    }
}

#[handler]
#[async_trait]
impl IRequestHandler<DeleteItemRequest, String> for DeleteItemHandler {
    async fn handle(&self, req: DeleteItemRequest) -> Result<String> {
        if REPO.delete(&req.id) {
            Ok(format!("Item {} deleted", req.id))
        } else {
            Err(Error::NotFound(format!("Item not found: {}", req.id)))
        }
    }
}

// ── Main ──────────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() {
    Host::builder()
        .mode(AppMode::Development)
        .register(|svc| {
            svc.singleton::<dyn IMiddleware>(|_| std::sync::Arc::new(LoggingMiddleware))
        })
        .configure(|app| {
            app.useOptions(|o| {
                o.app.name = "CRUD API".into();
            });
        })
        .build()
        .run()
        .await
        .expect("Server failed to start");
}
