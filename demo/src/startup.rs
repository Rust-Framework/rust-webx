//! Application startup — database initialization and seeding.
//!
//! Uses `IHostedService` for lifecycle-managed data initialization via DI.
//!
//! Injection chain:
//!   main.rs                              registers Arc<Mutex<lref::DbContext>>
//!     → DbInitService (#[inject_attr])   injects Arc<Mutex<DbContext>>
//!       → start()                        runs migrations + seeding

use std::sync::Arc;

use lref::{db_context::DbContext, prelude::*, provider::DbValue};
use lrwf::*;
use tokio::sync::Mutex;

/// Database initialization service — auto-registered via `#[lrdi::inject_attr]`.
#[lrdi::inject_attr(singleton, as = dyn IHostedService)]
pub struct DbInitService {
    ctx: Arc<Mutex<DbContext>>,
}

#[async_trait]
impl IHostedService for DbInitService {
    async fn start(&self) -> Result<()> {
        tracing::info!("[DbInitService] Starting database initialization...");

        // Run migrations
        {
            let mut ctx = self.ctx.lock().await;
            crate::domain::migrations::m001_initial_20260611::up(&mut ctx)
                .await
                .map_err(|e| Error::Internal(format!("Migration failed: {}", e)))?;
        }
        tracing::info!("[DbInitService] Migrations applied.");

        // Seed admin user
        {
            let mut ctx = self.ctx.lock().await;
            let query = ctx
                .set::<crate::domain::user::UserEntity>()
                .query()
                .filter_column("email", "=", DbValue::String("admin@lrwf.dev".into()));
            drop(ctx);

            let no_admin = query.first_or_default().await.ok().flatten().is_none();

            if no_admin {
                let ctx = self.ctx.lock().await;
                let hashed = bcrypt::hash("admin123", bcrypt::DEFAULT_COST)
                    .map_err(|e| Error::Internal(format!("Hash failed: {}", e)))?;
                let sql = format!(
                    "INSERT INTO users (id, name, email, password_hash, role, created_at) \
                     VALUES ('admin-001', 'Admin', 'admin@lrwf.dev', '{}', 'admin', '{}')",
                    hashed.replace('\'', "''"),
                    now_secs()
                );
                ctx.provider()
                    .execute_migration_command(&sql)
                    .await
                    .map_err(|e| Error::Internal(format!("Failed to insert admin: {}", e)))?;
                tracing::info!("[DbInitService] Default admin created: admin@lrwf.dev / admin123");
            }
        }

        // Seed products
        {
            let mut ctx = self.ctx.lock().await;
            let count = ctx
                .set::<crate::domain::product::ProductEntity>()
                .query()
                .count()
                .await
                .unwrap_or(0);
            drop(ctx);

            if count == 0 {
                for (name, price) in &[("Widget", 9.99), ("Gadget", 24.50), ("Thingamajig", 3.75)] {
                    let ctx = self.ctx.lock().await;
                    let sql = format!(
                        "INSERT INTO products (id, name, price, created_at) \
                         VALUES ('{}', '{}', {}, '{}')",
                        uuid(),
                        name,
                        price,
                        now_secs()
                    );
                    ctx.provider()
                        .execute_migration_command(&sql)
                        .await
                        .map_err(|e| Error::Internal(format!("Failed to seed product: {}", e)))?;
                }
                tracing::info!("[DbInitService] Seeded 3 sample products");
            }
        }

        tracing::info!("[DbInitService] Database initialization complete.");
        Ok(())
    }

    async fn stop(&self) -> Result<()> {
        tracing::info!("[DbInitService] Closing database connections...");
        Ok(())
    }
}

fn uuid() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| format!("{:x}", d.as_nanos()))
        .unwrap_or_else(|_| "0".to_string())
}

fn now_secs() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs().to_string())
        .unwrap_or_else(|_| "0".to_string())
}
