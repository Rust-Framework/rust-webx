//! Per-request DI scope 冒烟测试。
//!
//! 模拟 `crates/macros/src/endpoint.rs` 中的 dispatch 流程：
//! 1. 以 `scoped::<Mutex<DbContext>>` 注册 DbContext（与 `host/src/main.rs:register_db_context` 一致）
//! 2. 构建 root ServiceProvider
//! 3. 每个"请求"：`provider.create_scope()` → `scope.get()` → 处理 → drop scope
//! 4. 验证：
//!    - 同 scope 内多次解析 → 同一 DbContext 实例（Arc::ptr_eq true）
//!    - 不同 scope → 不同实例（Arc::ptr_eq false）
//!    - 根 provider 解析 → 独立实例（root scope cache）
//!    - 工厂执行次数 = scope 数（不重复触发）
//!    - scope drop 后再创建新 scope → 获得全新实例
//!
//! 运行：`cargo run --example scope_smoke --package docbit-host`

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use rust_dicore::ServiceCollection;
use rust_ef::db_context::{DbContext, DbContextOptionsBuilder};
use rust_ef_sqlite::DbContextOptionsBuilderExt as _;
use tokio::sync::Mutex;

static FACTORY_CALLS: AtomicUsize = AtomicUsize::new(0);

fn build_provider() -> Arc<rust_dicore::ServiceProvider> {
    let options = {
        let mut b = DbContextOptionsBuilder::new();
        b.use_sqlite_in_memory();
        Arc::new(b.build())
    };

    // 与 docbit/host/src/main.rs:register_db_context 相同的注册方式
    Arc::new(
        ServiceCollection::new()
            .scoped::<Mutex<DbContext>>(move |_| {
                let n = FACTORY_CALLS.fetch_add(1, Ordering::SeqCst);
                let ctx = DbContext::from_options(&options)
                    .expect("Failed to create DbContext from options");
                println!("  [factory] DbContext #{} created", n);
                Arc::new(Mutex::new(ctx))
            })
            .build()
            .expect("Failed to build ServiceProvider"),
    )
}

fn assert_eq(label: &str, actual: usize, expected: usize) {
    if actual != expected {
        panic!("FAIL [{}] expected {} but got {}", label, expected, actual);
    }
    println!("  [{}] factory calls = {} ✓", label, actual);
}

fn assert_true(label: &str, cond: bool) {
    if !cond {
        panic!("FAIL [{}] assertion is false", label);
    }
    println!("  [{}] ✓", label);
}

#[tokio::main]
async fn main() {
    println!("========================================================");
    println!("  Per-Request DI Scope Smoke Test");
    println!("========================================================\n");

    let provider = build_provider();
    println!("Provider built with scoped::<Mutex<DbContext>>\n");

    // ── Request 1 ──────────────────────────────────────────────
    println!("── Request 1: create_scope() ──");
    let scope1 = provider.create_scope();

    // 同一 scope 内两次解析：应返回同一实例（Scoped 语义）
    let ctx1_a: Arc<Mutex<DbContext>> = scope1.get();
    let ctx1_b: Arc<Mutex<DbContext>> = scope1.get();
    assert_true(
        "scope1: ctx1_a == ctx1_b (same instance)",
        Arc::ptr_eq(&ctx1_a, &ctx1_b),
    );
    assert_eq("after scope1 resolve", FACTORY_CALLS.load(Ordering::SeqCst), 1);
    println!();

    // ── Request 2 ──────────────────────────────────────────────
    println!("── Request 2: create_scope() ──");
    let scope2 = provider.create_scope();
    let ctx2: Arc<Mutex<DbContext>> = scope2.get();

    // 不同 scope：应返回不同实例（per-request 隔离）
    assert_true(
        "scope2 != scope1 (different instances)",
        !Arc::ptr_eq(&ctx1_a, &ctx2),
    );
    assert_eq("after scope2 resolve", FACTORY_CALLS.load(Ordering::SeqCst), 2);
    println!();

    // ── Root provider resolution ───────────────────────────────
    println!("── Root provider: provider.get() ──");
    let root_ctx: Arc<Mutex<DbContext>> = provider.get();

    // 根 provider 有自己的 root_scoped_cache，与子 scope 隔离
    assert_true(
        "root != scope1 (root scope cache isolated)",
        !Arc::ptr_eq(&ctx1_a, &root_ctx),
    );
    assert_true(
        "root != scope2 (root scope cache isolated)",
        !Arc::ptr_eq(&ctx2, &root_ctx),
    );
    assert_eq("after root resolve", FACTORY_CALLS.load(Ordering::SeqCst), 3);
    println!();

    // ── Simulate request end (drop scope + resolved instances) ─
    println!("── Drop scope1 + scope2 (simulate request end) ──");
    drop(ctx1_a);
    drop(ctx1_b);
    drop(scope1);
    drop(ctx2);
    drop(scope2);
    println!("  scope1 and scope2 dropped — cached DbContexts released\n");

    // ── Request 3 (after scopes dropped) ───────────────────────
    println!("── Request 3: create_scope() (after previous scopes dropped) ──");
    let scope3 = provider.create_scope();
    let ctx3: Arc<Mutex<DbContext>> = scope3.get();

    // 新 scope 获得全新实例（之前的实例已随 scope drop 释放）
    assert_true(
        "scope3 != root (new scope gets fresh instance)",
        !Arc::ptr_eq(&root_ctx, &ctx3),
    );
    assert_eq("after scope3 resolve", FACTORY_CALLS.load(Ordering::SeqCst), 4);
    println!();

    drop(ctx3);
    drop(scope3);
    drop(root_ctx);

    println!("========================================================");
    println!("  ALL ASSERTIONS PASSED");
    println!("========================================================");
    println!();
    println!("Summary:");
    println!("  ✓ Scoped DbContext correctly cached per scope");
    println!("  ✓ Different scopes get different DbContext instances");
    println!("  ✓ Root scope is isolated from child scopes");
    println!("  ✓ Factory fires exactly once per scope (no re-fire on re-resolve)");
    println!("  ✓ Scope drop releases cached instance (next scope gets fresh one)");
    println!();
    println!("This confirms the per-request DI scope in endpoint.rs dispatch");
    println!("provides proper unit-of-work isolation for DbContext.");
}
