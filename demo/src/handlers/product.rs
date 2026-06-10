use lrwf::*;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use crate::contracts::product::*;

// ── In-memory ProductRepository ──

struct ProductRepository {
    products: Mutex<HashMap<String, ProductModel>>,
}

impl ProductRepository {
    fn new() -> Self {
        let repo = Self {
            products: Mutex::new(HashMap::new()),
        };
        // Seed data
        repo.create("Widget", 9.99);
        repo.create("Gadget", 24.50);
        repo.create("Thingamajig", 3.75);
        repo
    }

    fn list(&self) -> Vec<ProductModel> {
        self.products
            .lock()
            .map(|g| g.values().cloned().collect())
            .unwrap_or_default()
    }

    fn get(&self, id: &str) -> Option<ProductModel> {
        self.products.lock().ok()?.get(id).cloned()
    }

    fn create(&self, name: &str, price: f64) -> ProductModel {
        let id = format!(
            "{:x}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        );
        let product = ProductModel {
            id: id.clone(),
            name: name.to_string(),
            price,
            created_at: now_string(),
        };
        self.products
            .lock()
            .map(|mut g| {
                g.insert(id, product.clone());
            })
            .ok();
        product
    }

    fn update(&self, id: &str, name: Option<&str>, price: Option<f64>) -> Option<ProductModel> {
        let mut products = self.products.lock().ok()?;
        if let Some(p) = products.get_mut(id) {
            if let Some(n) = name {
                p.name = n.to_string();
            }
            if let Some(pr) = price {
                p.price = pr;
            }
            Some(p.clone())
        } else {
            None
        }
    }

    fn delete(&self, id: &str) -> bool {
        self.products
            .lock()
            .map(|mut g| g.remove(id).is_some())
            .unwrap_or(false)
    }
}

fn now_string() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!("{}", ts)
}

static REPO: OnceLock<Arc<ProductRepository>> = OnceLock::new();

fn repo() -> &'static Arc<ProductRepository> {
    REPO.get().unwrap_or_else(|| {
        let _ = REPO.set(Arc::new(ProductRepository::new()));
        REPO.get().expect("ProductRepository initialization failed")
    })
}

// ── IRequestHandler implementations ──

#[derive(Default)]
pub struct ListProductsHandler;

#[derive(Default)]
pub struct GetProductHandler;

#[derive(Default)]
pub struct CreateProductHandler;

#[derive(Default)]
pub struct UpdateProductHandler;

#[derive(Default)]
pub struct DeleteProductHandler;

#[handler]
#[async_trait]
impl IRequestHandler<ListProductsRequest, Vec<ProductModel>> for ListProductsHandler {
    async fn handle(&self, _: ListProductsRequest) -> Result<Vec<ProductModel>> {
        Ok(repo().list())
    }
}

#[handler]
#[async_trait]
impl IRequestHandler<GetProductRequest, ProductModel> for GetProductHandler {
    async fn handle(&self, req: GetProductRequest) -> Result<ProductModel> {
        repo()
            .get(&req.id)
            .ok_or_else(|| Error::NotFound(format!("Product not found: {}", req.id)))
    }
}

#[handler]
#[async_trait]
impl IRequestHandler<CreateProductRequest, ProductModel> for CreateProductHandler {
    async fn handle(&self, req: CreateProductRequest) -> Result<ProductModel> {
        let product = repo().create(&req.name, req.price);
        tracing::info!("Product created: {} (id: {})", product.name, product.id);
        Ok(product)
    }
}

#[handler]
#[async_trait]
impl IRequestHandler<UpdateProductRequest, ProductModel> for UpdateProductHandler {
    async fn handle(&self, req: UpdateProductRequest) -> Result<ProductModel> {
        repo()
            .update(&req.id, req.name.as_deref(), req.price)
            .ok_or_else(|| Error::NotFound(format!("Product not found: {}", req.id)))
    }
}

#[handler]
#[async_trait]
impl IRequestHandler<DeleteProductRequest, String> for DeleteProductHandler {
    async fn handle(&self, req: DeleteProductRequest) -> Result<String> {
        if repo().delete(&req.id) {
            tracing::info!("Product deleted: {}", req.id);
            Ok(format!("Product {} deleted", req.id))
        } else {
            Err(Error::NotFound(format!("Product not found: {}", req.id)))
        }
    }
}
