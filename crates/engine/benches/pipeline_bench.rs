//! Benchmark: the **production** engine pipeline (`webx_engine::Pipeline`).
//!
//! The host crate's `MiddlewarePipeline::execute` is a 1.5-era helper; the
//! serving path runs the engine pipeline with a typed final handler. The
//! documented contract is that a pipeline made of legacy `invoke`/`after` stages
//! costs **zero allocations per request** (the continuation style boxes per
//! stage, which is a declared cost). This benchmark measures both so a
//! regression in the zero-allocation loop is visible.
//!
//! Each iteration also builds one `EngineContext` (there is no cheap reset), so
//! the **absolute** number is not "pipeline only": read the per-stage cost as
//! the delta between the stage counts (~80-90 ns per legacy stage on the
//! reference machine).

use std::ops::ControlFlow;
use std::pin::Pin;
use std::sync::Arc;

use bytes::Bytes;
use criterion::{black_box, criterion_group, criterion_main, Criterion};
use webx_core::error::Result as WebxResult;
use webx_core::http::IHttpContext;
use webx_core::middleware::IMiddleware;
use webx_engine::body::BodyLimits;
use webx_engine::pipeline::{FinalHandler, Pipeline};
use webx_engine::EngineContext;

/// A legacy-style no-op stage: returns `Continue` and has no `after` hook.
struct NoopStage;

#[async_trait::async_trait]
impl IMiddleware for NoopStage {
    async fn invoke(&self, _ctx: &mut dyn IHttpContext) -> WebxResult<ControlFlow<()>> {
        Ok(ControlFlow::Continue(()))
    }
}

fn context(limits: &Arc<BodyLimits>) -> EngineContext {
    let request = http::Request::builder()
        .method("GET")
        .uri("/bench/pipeline")
        .header("accept", "application/json")
        .body(())
        .expect("build request");
    let (parts, ()) = request.into_parts();
    EngineContext::from_parts(parts, Bytes::new(), Arc::clone(limits), None, None)
}

fn final_handler() -> FinalHandler {
    Arc::new(|ctx: &mut EngineContext| {
        Box::pin(async move {
            ctx.response_mut().set_status(200);
            Ok(())
        }) as Pin<Box<dyn std::future::Future<Output = WebxResult<()>> + Send + '_>>
    })
}

fn pipeline_with(stages: usize) -> Pipeline {
    let stages = (0..stages)
        .map(|_| Arc::new(NoopStage) as Arc<dyn IMiddleware>)
        .collect();
    Pipeline::new(stages)
}

fn bench_pipeline(c: &mut Criterion) {
    let limits = Arc::new(BodyLimits::default());
    let rt = tokio::runtime::Runtime::new().expect("runtime");

    for stages in [0usize, 3, 10] {
        let pipeline = pipeline_with(stages);
        let handler = final_handler();
        c.bench_function(&format!("engine_pipeline/{stages}_stages"), |b| {
            b.iter(|| {
                rt.block_on(async {
                    let mut ctx = context(&limits);
                    pipeline
                        .execute(black_box(&mut ctx), Arc::clone(&handler))
                        .await
                        .expect("pipeline");
                })
            })
        });
    }
}

criterion_group!(benches, bench_pipeline);
criterion_main!(benches);
