//! Application DbContext — wraps the database provider and exposes
//! EF Core-style `Set<T>` + `SaveChanges` semantics.
//!
//! # Usage in handlers (Dependency Injection)
//!
//! ```ignore
//! pub struct CreateProductHandler { ctx: Arc<AppDbContext> }
//!
//! impl IRequestHandler<CreateProductRequest, ProductModel> for CreateProductHandler {
//!     async fn handle(&self, req: CreateProductRequest) -> Result<ProductModel> {
//!         let set = self.ctx.set::<ProductEntity>();
//!         // Query:  set.filter_column(...).to_list().await
//!         // Insert: self.ctx.execute(&sql).await
//!         // No explicit SaveChanges() needed — REF writes are immediate.
//!     }
//! }
//! ```

use lref::prelude::*;
use lref::provider::DatabaseProvider;
use lref::query::QueryBuilder;
use std::sync::Arc;

/// Application-level database context.
///
/// Equivalent to Entity Framework Core's `DbContext`. Provides typed
/// access to database tables via `set::<T>()` and raw SQL execution
/// via `execute()`.
///
/// Registered as a singleton in the DI container.
pub struct AppDbContext {
    provider: Arc<dyn DatabaseProvider>,
}

impl AppDbContext {
    /// Create a new context backed by the given database provider.
    pub fn new(provider: Arc<dyn DatabaseProvider>) -> Self {
        Self { provider }
    }

    /// Return a typed `QueryBuilder<T>` for the given entity.
    ///
    /// Equivalent to EF Core's `context.Set<T>()`.
    ///
    /// ```ignore
    /// let users = ctx.set::<UserEntity>()
    ///     .filter_column("email", "=", DbValue::String(email))
    ///     .first_or_default().await?;
    /// ```
    pub fn set<T: EntityType>(&self) -> QueryBuilder<T> {
        QueryBuilder::<T>::with_provider(
            &T::entity_meta().table_name.as_ref().to_string(),
            Arc::clone(&self.provider) as Arc<dyn DatabaseProvider>,
        )
    }

    /// Execute a raw SQL statement (INSERT / UPDATE / DELETE).
    ///
    /// Equivalent to EF Core's `context.Database.ExecuteSqlRaw()`.
    /// REF is a lightweight query-builder ORM without change tracking —
    /// writes are immediate, no explicit `SaveChanges()` is required.
    pub async fn execute(&self, sql: &str) -> Result<(), String> {
        self.provider
            .execute_migration_command(sql)
            .await
            .map_err(|e| e.to_string())
    }

    /// Access the underlying provider directly (for migration/init use).
    #[allow(dead_code)]
    pub fn provider(&self) -> &Arc<dyn DatabaseProvider> {
        &self.provider
    }
}
