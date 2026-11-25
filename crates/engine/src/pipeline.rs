//! Onion middleware pipeline.
//!
//! The pipeline is built once at startup from the ordered stage list the host
//! assembled. Per request it runs the stages forward, calls the terminal
//! handler supplied by the server (the router), then runs the `after` hooks in
//! reverse order.
//!
//! Semantics are exactly those of 1.5:
//!
//! * a stage returning `ControlFlow::Break` stops the remaining stages and the
//!   terminal handler; changes already written to the response are kept;
//! * an error from `invoke` or from the terminal handler stops the forward
//!   pass, and the remaining stages never run;
//! * `after` runs for every stage whose `invoke` completed, in reverse order,
//!   **including** when the request failed, so metrics, tracing, compression
//!   and cleanup observe the failed response;
//! * the first error wins: an `after` hook failure is logged and never masks
//!   the error that already stopped the request.
//!
//! The loop keeps a count of completed stages instead of collecting `Arc`
//! clones into a `Vec`, so it allocates nothing per request.
//!
//! The pipeline is deliberately independent of [`crate::router`]: the terminal
//! handler is a parameter, so a server can route however it likes.

use std::future::Future;
use std::ops::ControlFlow;
use std::pin::Pin;
use std::sync::Arc;

use tracing::Instrument;
use webx_core::error::{Error, Result};
use webx_core::http::IHttpContext;
use webx_core::middleware::{BoxedNextFuture, IMiddleware, NextStage};

use crate::context::EngineContext;

/// The stable span contract lives in `docs/design/observability-contract.md`.
///
/// The root span covers the whole request: the middleware onion, routing, the
/// handler and the reverse `after` pass. The trace id is only known after the
/// tracing middleware ran, so it is declared empty here and recorded once the
/// pipeline returns; `status` is recorded the same way.
///
/// Spans are DEBUG on purpose: the default host installs an INFO-level
/// subscriber, so request spans are opt-in (`RUST_LOG=webx.http=debug` or a
/// per-target filter) and the default hot path stays allocation-free — which is
/// what the alloc-probe framework-tax gate measures.
fn request_span(ctx: &dyn IHttpContext) -> tracing::Span {
    tracing::debug_span!(
        target: "webx.http",
        "http.server.request",
        method = %ctx.request().method(),
        path = %ctx.request().path(),
        trace_id = tracing::field::Empty,
        status = tracing::field::Empty,
    )
}

/// Router-terminal span: route match, endpoint dispatch and response write.
///
/// The matched template is not known when the span opens, so it is recorded
/// from the context after the terminal returns.
fn route_match_span(ctx: &dyn IHttpContext) -> tracing::Span {
    tracing::debug_span!(
        target: "webx.http",
        "http.route_match",
        method = %ctx.request().method(),
        path = %ctx.request().path(),
        pattern = tracing::field::Empty,
        params = tracing::field::Empty,
    )
}

fn finish_route_match_span(span: &tracing::Span, ctx: &dyn IHttpContext) {
    if let Some(pattern) = ctx.request().route_pattern() {
        span.record("pattern", pattern);
    }
    span.record("params", ctx.request().route_params().len() as u64);
}

fn record_request_outcome(span: &tracing::Span, ctx: &dyn IHttpContext) {
    // Always record both fields so the root contract is deterministic: an empty
    // `trace_id` means no trace-id middleware was installed for this request.
    span.record("trace_id", ctx.trace_id().unwrap_or(""));
    span.record("status", u64::from(ctx.response().status()));
}

/// Boxed terminal handler: the server's router entry point.
///
/// It runs after every middleware `invoke` passed and returns the request
/// result; the response itself is written into the context.
pub type FinalHandler = Arc<
    dyn for<'a> Fn(&'a mut EngineContext) -> Pin<Box<dyn Future<Output = Result<()>> + Send + 'a>>
        + Send
        + Sync,
>;

/// Converts a pipeline error into a response before the reverse `after` pass.
pub type ErrorHandler = Arc<
    dyn for<'a> Fn(&'a mut EngineContext, Error) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>>
        + Send
        + Sync,
>;

/// Recover the engine context from the dynamic pipeline context.
fn engine_context(ctx: &mut dyn IHttpContext) -> Result<&mut EngineContext> {
    ctx.as_any_mut()
        .and_then(|any| any.downcast_mut::<EngineContext>())
        .ok_or_else(|| {
            Error::internal("continuation middleware requires the engine HTTP context".to_string())
        })
}

/// Recursive continuation executor (2.4-4).
///
/// Legacy stages are driven exactly like the fast loop (`invoke` forward, and on
/// unwind `after` for every stage whose `invoke` returned `Continue`);
/// continuation-style stages receive [`Next`] and own their pre/post ordering.
/// Errors are converted through `on_error` where they surface, so outer `after`
/// hooks observe the real error response.
///
/// The chain state is `Arc`-owned so the `Next` closure can be `'static`; this
/// is the documented cost of opting into continuation middleware (one boxed
/// future per stage). Pipelines without continuation stages keep the
/// zero-allocation legacy loop.
fn run_chain(
    ctx: &mut dyn IHttpContext,
    stages: Arc<Vec<Arc<dyn IMiddleware>>>,
    index: usize,
    terminal: FinalHandler,
    on_error: Option<ErrorHandler>,
) -> BoxedNextFuture<'_> {
    Box::pin(async move {
        let Some(stage) = stages.get(index).cloned() else {
            let span = route_match_span(&*ctx);
            let engine = engine_context(&mut *ctx)?;
            let outcome = terminal(engine).instrument(span.clone()).await;
            finish_route_match_span(&span, &*ctx);
            return match outcome {
                Ok(()) => Ok(()),
                Err(error) => convert_error(&mut *ctx, error, on_error.as_ref()).await,
            };
        };
        if stage.continuation() {
            let continuation_stages = Arc::clone(&stages);
            let continuation_terminal = Arc::clone(&terminal);
            let continuation_on_error = on_error.clone();
            let next = NextStage::new(move |ctx: &mut dyn IHttpContext| {
                run_chain(
                    ctx,
                    Arc::clone(&continuation_stages),
                    index + 1,
                    Arc::clone(&continuation_terminal),
                    continuation_on_error.clone(),
                )
            });
            match stage.invoke_next(&mut *ctx, next).await {
                Ok(()) => Ok(()),
                Err(error) => convert_error(&mut *ctx, error, on_error.as_ref()).await,
            }
        } else {
            let outcome = match stage.fast_invoke(&mut *ctx) {
                Some(outcome) => outcome,
                None => stage.invoke(&mut *ctx).await,
            };
            match outcome {
                Ok(ControlFlow::Break(())) => Ok(()),
                Err(error) => convert_error(&mut *ctx, error, on_error.as_ref()).await,
                Ok(ControlFlow::Continue(())) => {
                    let result = run_chain(
                        &mut *ctx,
                        Arc::clone(&stages),
                        index + 1,
                        Arc::clone(&terminal),
                        on_error.clone(),
                    )
                    .await;
                    if !stage.has_after() {
                        return result;
                    }
                    match stage.after(&mut *ctx).await {
                        Ok(()) => result,
                        Err(after_error) => {
                            if result.is_ok() {
                                convert_error(&mut *ctx, after_error, on_error.as_ref()).await
                            } else {
                                tracing::error!(error = %after_error, "middleware after hook failed");
                                result
                            }
                        }
                    }
                }
            }
        }
    })
}

async fn convert_error(
    ctx: &mut dyn IHttpContext,
    error: Error,
    on_error: Option<&ErrorHandler>,
) -> Result<()> {
    match on_error {
        Some(handler) => {
            let engine = engine_context(ctx)?;
            handler(engine, error).await;
            Ok(())
        }
        None => Err(error),
    }
}

/// The ordered middleware stages plus the optimized execution loop.
pub struct Pipeline {
    /// Stages in registration order (the host owns slot placement).
    ///
    /// Private so a pipeline can only be assembled through [`Self::new`]; a
    /// struct literal would bypass the continuation-style detection below.
    stages: Vec<Arc<dyn IMiddleware>>,
    /// Whether any stage uses the continuation style (2.4-4).
    ///
    /// Computed by [`Self::new`]. Default pipelines contain only legacy
    /// `invoke`/`after` stages, so the zero-allocation loop below stays on the
    /// hot path. Declaring a continuation stage selects the continuation
    /// executor for the whole pipeline (it boxes one future per stage by
    /// design).
    pub has_continuation: bool,
}

impl Pipeline {
    /// Build a pipeline from the host's ordered stages.
    pub fn new(stages: Vec<Arc<dyn IMiddleware>>) -> Self {
        let has_continuation = stages.iter().any(|stage| stage.continuation());
        Self {
            stages,
            has_continuation,
        }
    }

    /// Run the onion for one request.
    ///
    /// `final_handler` is the server's terminal (routing) function; it is
    /// called only when no stage short-circuited or failed.
    pub async fn execute(
        &self,
        ctx: &mut EngineContext,
        final_handler: FinalHandler,
    ) -> Result<()> {
        let span = request_span(ctx);
        let result = self
            .execute_inner(ctx, final_handler)
            .instrument(span.clone())
            .await;
        record_request_outcome(&span, ctx);
        result
    }

    async fn execute_inner(
        &self,
        ctx: &mut EngineContext,
        final_handler: FinalHandler,
    ) -> Result<()> {
        if self.has_continuation {
            return self.run_continuation(ctx, final_handler).await;
        }
        self.run(ctx, move |ctx: &mut EngineContext| final_handler(ctx))
            .await
    }

    /// Run the pipeline and convert a forward/terminal failure into a response
    /// **before** the reverse `after` pass.
    ///
    /// This is the production path: metrics, tracing, compression and any other
    /// `after` hook must see the real status/body of a handler or middleware
    /// error, not the default `200` placeholder that used to exist until the
    /// connection layer wrote the error after the whole pipeline had finished.
    ///
    /// The error handler runs inside the same panic boundary as the middleware,
    /// so a broken custom `IErrorResponseWriter` is contained by the caller.
    pub async fn execute_with_error(
        &self,
        ctx: &mut EngineContext,
        final_handler: FinalHandler,
        on_error: ErrorHandler,
    ) -> Result<()> {
        let span = request_span(ctx);
        let result = self
            .execute_with_error_inner(ctx, final_handler, on_error)
            .instrument(span.clone())
            .await;
        record_request_outcome(&span, ctx);
        result
    }

    async fn execute_with_error_inner(
        &self,
        ctx: &mut EngineContext,
        final_handler: FinalHandler,
        on_error: ErrorHandler,
    ) -> Result<()> {
        if self.has_continuation {
            return self
                .run_continuation_with_error(ctx, final_handler, on_error)
                .await;
        }
        self.run_with_error(ctx, final_handler, on_error).await
    }

    /// Continuation executor without an error-to-response hook (`execute`).
    ///
    /// The chain state is Arc-owned so the `NextStage` continuation can be
    /// `'static`; this is part of the documented opt-in cost (the legacy loop
    /// keeps zero allocations).
    async fn run_continuation(
        &self,
        ctx: &mut EngineContext,
        final_handler: FinalHandler,
    ) -> Result<()> {
        let stages = Arc::new(self.stages.clone());
        run_chain(ctx, stages, 0, final_handler, None).await
    }

    /// Continuation executor with error conversion *before* the reverse `after`
    /// pass (production path, same contract as [`Self::run_with_error`]).
    async fn run_continuation_with_error(
        &self,
        ctx: &mut EngineContext,
        final_handler: FinalHandler,
        on_error: ErrorHandler,
    ) -> Result<()> {
        let stages = Arc::new(self.stages.clone());
        run_chain(ctx, stages, 0, final_handler, Some(on_error)).await
    }

    async fn run_with_error(
        &self,
        ctx: &mut EngineContext,
        final_handler: FinalHandler,
        on_error: ErrorHandler,
    ) -> Result<()> {
        let mut executed = 0usize;
        let mut short_circuited = false;
        let mut failure: Option<Error> = None;

        for stage in &self.stages {
            let outcome = match stage.fast_invoke(ctx) {
                Some(outcome) => outcome,
                None => stage.invoke(ctx).await,
            };
            match outcome {
                Ok(ControlFlow::Continue(())) => executed += 1,
                Ok(ControlFlow::Break(())) => {
                    short_circuited = true;
                    break;
                }
                Err(error) => {
                    failure = Some(error);
                    break;
                }
            }
        }

        if !short_circuited && failure.is_none() {
            let span = route_match_span(ctx);
            let outcome = final_handler(ctx).instrument(span.clone()).await;
            finish_route_match_span(&span, ctx);
            if let Err(error) = outcome {
                failure = Some(error);
            }
        }

        // Convert forward/terminal failure while the stage count is still the
        // set of stages whose invoke completed. Those stages' after hooks then
        // observe the real error response.
        let mut written = false;
        if let Some(error) = failure.take() {
            on_error(ctx, error).await;
            written = true;
        }

        for index in (0..executed).rev() {
            if !self.stages[index].has_after() {
                continue;
            }
            if let Err(error) = self.stages[index].after(ctx).await {
                if written {
                    tracing::error!(error = %error, "middleware after hook failed");
                } else {
                    on_error(ctx, error).await;
                    written = true;
                }
            }
        }

        Ok(())
    }

    /// The execution loop.
    ///
    /// Generic over the context so the unit tests can drive it with a mock
    /// `IHttpContext` (`EngineContext` is built from a hyper request, which a
    /// unit test cannot construct); the public [`Self::execute`] monomorphizes
    /// it for `EngineContext` and is its only production caller.
    async fn run<C, F>(&self, ctx: &mut C, final_handler: F) -> Result<()>
    where
        C: IHttpContext,
        F: for<'a> Fn(&'a mut C) -> Pin<Box<dyn Future<Output = Result<()>> + Send + 'a>>,
    {
        let mut executed = 0usize;
        let mut short_circuited = false;
        let mut failure: Option<Error> = None;

        // Forward pass. `executed` is the number of stages whose `invoke`
        // returned `Continue`; the stage that broke or failed is deliberately
        // not included, matching the 1.5 behavior.
        for stage in &self.stages {
            let outcome = match stage.fast_invoke(ctx) {
                Some(outcome) => outcome,
                None => stage.invoke(ctx).await,
            };
            match outcome {
                Ok(ControlFlow::Continue(())) => executed += 1,
                Ok(ControlFlow::Break(())) => {
                    short_circuited = true;
                    break;
                }
                Err(error) => {
                    failure = Some(error);
                    break;
                }
            }
        }

        // Terminal handler: skipped on short-circuit or forward-pass failure.
        if !short_circuited && failure.is_none() {
            let span = route_match_span(ctx);
            let outcome = final_handler(ctx).instrument(span.clone()).await;
            finish_route_match_span(&span, ctx);
            if let Err(error) = outcome {
                failure = Some(error);
            }
        }

        // Reverse pass over the stages that actually entered. Indexing the
        // slice avoids the per-request `Vec<Arc<dyn IMiddleware>>` clone the
        // 1.5 loop builds.
        for index in (0..executed).rev() {
            if !self.stages[index].has_after() {
                continue;
            }
            if let Err(error) = self.stages[index].after(ctx).await {
                if failure.is_none() {
                    failure = Some(error);
                } else {
                    tracing::error!(error = %error, "middleware after hook failed");
                }
            }
        }

        match failure {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::future::Future;
    use std::pin::Pin;
    use std::sync::Mutex;

    use webx_core::auth::IClaims;
    use webx_core::http::{IHttpRequest, IHttpResponse};

    struct MockRequest {
        method: String,
        path: String,
        headers: HashMap<String, String>,
        query: HashMap<String, String>,
        route_params: HashMap<String, String>,
        route_pattern: Option<String>,
        body: Vec<u8>,
    }

    impl MockRequest {
        fn new(method: &str, path: &str) -> Self {
            Self {
                method: method.to_string(),
                path: path.to_string(),
                headers: HashMap::new(),
                query: HashMap::new(),
                route_params: HashMap::new(),
                route_pattern: None,
                body: Vec::new(),
            }
        }
    }

    #[async_trait::async_trait]
    impl IHttpRequest for MockRequest {
        fn method(&self) -> &str {
            &self.method
        }

        fn path(&self) -> &str {
            &self.path
        }

        fn header(&self, name: &str) -> Option<&str> {
            self.headers.get(name).map(String::as_str)
        }

        fn query(&self) -> &HashMap<String, String> {
            &self.query
        }

        fn route_params(&self) -> &HashMap<String, String> {
            &self.route_params
        }

        fn route_params_mut(&mut self) -> &mut HashMap<String, String> {
            &mut self.route_params
        }

        fn route_pattern(&self) -> Option<&str> {
            self.route_pattern.as_deref()
        }

        fn route_pattern_mut(&mut self) -> &mut Option<String> {
            &mut self.route_pattern
        }

        async fn body_bytes(&mut self) -> Result<Vec<u8>> {
            Ok(std::mem::take(&mut self.body))
        }
    }

    struct MockResponse {
        status: u16,
        headers: HashMap<String, String>,
        body: Vec<u8>,
    }

    #[async_trait::async_trait]
    impl IHttpResponse for MockResponse {
        fn status(&self) -> u16 {
            self.status
        }

        fn set_status(&mut self, code: u16) {
            self.status = code;
        }

        fn set_header(&mut self, key: &str, value: &str) {
            self.headers
                .insert(key.to_ascii_lowercase(), value.to_string());
        }

        async fn write_bytes(&mut self, data: Vec<u8>) -> Result<()> {
            self.body = data;
            Ok(())
        }
    }

    /// Minimal `IHttpContext` for pipeline tests: enough state for the trait
    /// and a shared order log the middlewares and terminal handler append to.
    struct MockContext {
        request: MockRequest,
        response: MockResponse,
        claims: Option<Box<dyn IClaims>>,
        log: Arc<Mutex<Vec<&'static str>>>,
    }

    impl MockContext {
        fn new(log: Arc<Mutex<Vec<&'static str>>>) -> Self {
            Self {
                request: MockRequest::new("GET", "/test"),
                response: MockResponse {
                    status: 200,
                    headers: HashMap::new(),
                    body: Vec::new(),
                },
                claims: None,
                log,
            }
        }
    }

    impl webx_core::http::IClaimsExt for MockContext {
        fn set_claims(&mut self, claims: Box<dyn IClaims>) {
            self.claims = Some(claims);
        }

        fn claims(&self) -> Option<&dyn IClaims> {
            self.claims.as_deref()
        }
    }

    impl IHttpContext for MockContext {
        fn request(&self) -> &dyn IHttpRequest {
            &self.request
        }

        fn request_mut(&mut self) -> &mut dyn IHttpRequest {
            &mut self.request
        }

        fn response(&self) -> &dyn IHttpResponse {
            &self.response
        }

        fn response_mut(&mut self) -> &mut dyn IHttpResponse {
            &mut self.response
        }
    }

    #[derive(Clone, Copy)]
    enum Invoke {
        Continue,
        Break,
        Fail,
    }

    #[derive(Clone, Copy)]
    enum After {
        Ok,
        Fail,
    }

    struct Probe {
        before: &'static str,
        after: &'static str,
        invoke: Invoke,
        after_result: After,
        log: Arc<Mutex<Vec<&'static str>>>,
    }

    #[async_trait::async_trait]
    impl IMiddleware for Probe {
        async fn invoke(&self, _ctx: &mut dyn IHttpContext) -> Result<ControlFlow<()>> {
            self.log.lock().unwrap().push(self.before);
            match self.invoke {
                Invoke::Continue => Ok(ControlFlow::Continue(())),
                Invoke::Break => Ok(ControlFlow::Break(())),
                Invoke::Fail => Err(Error::status(500, format!("{} failed", self.before))),
            }
        }

        async fn after(&self, _ctx: &mut dyn IHttpContext) -> Result<()> {
            self.log.lock().unwrap().push(self.after);
            match self.after_result {
                After::Ok => Ok(()),
                After::Fail => Err(Error::status(500, format!("{} failed", self.after))),
            }
        }
    }

    fn probe(
        before: &'static str,
        after: &'static str,
        invoke: Invoke,
        after_result: After,
        log: &Arc<Mutex<Vec<&'static str>>>,
    ) -> Arc<dyn IMiddleware> {
        Arc::new(Probe {
            before,
            after,
            invoke,
            after_result,
            log: Arc::clone(log),
        })
    }

    fn final_ok<'a>(
        ctx: &'a mut MockContext,
    ) -> Pin<Box<dyn Future<Output = Result<()>> + Send + 'a>> {
        Box::pin(async move {
            ctx.log.lock().unwrap().push("final");
            Ok(())
        })
    }

    fn final_fail<'a>(
        ctx: &'a mut MockContext,
    ) -> Pin<Box<dyn Future<Output = Result<()>> + Send + 'a>> {
        Box::pin(async move {
            ctx.log.lock().unwrap().push("final");
            Err(Error::status(500, "final failed".to_string()))
        })
    }

    fn status_500(error: &Error, message: &str) -> bool {
        error.status_code() == 500 && error.public_detail() == Some(message)
    }

    #[tokio::test]
    async fn empty_pipeline_runs_the_final_handler() {
        let log = Arc::new(Mutex::new(Vec::new()));
        let pipeline = Pipeline::new(Vec::new());
        let mut ctx = MockContext::new(Arc::clone(&log));

        pipeline
            .run(&mut ctx, final_ok)
            .await
            .expect("final handler ran");

        assert_eq!(*log.lock().unwrap(), vec!["final"]);
        assert_eq!(ctx.response.status, 200);
    }

    #[tokio::test]
    async fn stages_run_forward_and_after_hooks_run_in_reverse() {
        let log = Arc::new(Mutex::new(Vec::new()));
        let pipeline = Pipeline::new(vec![
            probe("a", "after a", Invoke::Continue, After::Ok, &log),
            probe("b", "after b", Invoke::Continue, After::Ok, &log),
            probe("c", "after c", Invoke::Continue, After::Ok, &log),
        ]);
        let mut ctx = MockContext::new(Arc::clone(&log));

        pipeline
            .run(&mut ctx, final_ok)
            .await
            .expect("pipeline succeeds");

        assert_eq!(
            *log.lock().unwrap(),
            vec!["a", "b", "c", "final", "after c", "after b", "after a"]
        );
    }

    #[tokio::test]
    async fn break_short_circuits_and_only_entered_stages_get_after() {
        let log = Arc::new(Mutex::new(Vec::new()));
        let pipeline = Pipeline::new(vec![
            probe("a", "after a", Invoke::Continue, After::Ok, &log),
            probe("b", "after b", Invoke::Break, After::Ok, &log),
            probe("c", "after c", Invoke::Continue, After::Ok, &log),
        ]);
        let mut ctx = MockContext::new(Arc::clone(&log));

        pipeline
            .run(&mut ctx, final_ok)
            .await
            .expect("break is not an error");

        // `c` and the final handler never run; `b` broke before its invoke
        // completed, so (as in 1.5) its after hook does not run either.
        assert_eq!(*log.lock().unwrap(), vec!["a", "b", "after a"]);
    }

    #[tokio::test]
    async fn invoke_error_propagates_and_unwinds_entered_stages() {
        let log = Arc::new(Mutex::new(Vec::new()));
        let pipeline = Pipeline::new(vec![
            probe("a", "after a", Invoke::Continue, After::Ok, &log),
            probe("b", "after b", Invoke::Fail, After::Ok, &log),
            probe("c", "after c", Invoke::Continue, After::Ok, &log),
        ]);
        let mut ctx = MockContext::new(Arc::clone(&log));

        let error = pipeline
            .run(&mut ctx, final_ok)
            .await
            .expect_err("invoke error must surface");

        assert!(status_500(&error, "b failed"));
        assert_eq!(*log.lock().unwrap(), vec!["a", "b", "after a"]);
    }

    #[tokio::test]
    async fn final_handler_error_is_returned_after_after_hooks_run() {
        let log = Arc::new(Mutex::new(Vec::new()));
        let pipeline = Pipeline::new(vec![probe(
            "a",
            "after a",
            Invoke::Continue,
            After::Ok,
            &log,
        )]);
        let mut ctx = MockContext::new(Arc::clone(&log));

        let error = pipeline
            .run(&mut ctx, final_fail)
            .await
            .expect_err("final handler error must surface");

        assert!(status_500(&error, "final failed"));
        assert_eq!(*log.lock().unwrap(), vec!["a", "final", "after a"]);
    }

    #[tokio::test]
    async fn first_error_wins_over_an_after_hook_error() {
        let log = Arc::new(Mutex::new(Vec::new()));
        let pipeline = Pipeline::new(vec![probe(
            "a",
            "after a",
            Invoke::Continue,
            After::Fail,
            &log,
        )]);
        let mut ctx = MockContext::new(Arc::clone(&log));

        let error = pipeline
            .run(&mut ctx, final_fail)
            .await
            .expect_err("the first error is returned");

        // The final handler failed first; the `after` hook failure is logged
        // and does not replace it.
        assert!(status_500(&error, "final failed"));
        assert_eq!(*log.lock().unwrap(), vec!["a", "final", "after a"]);
    }

    #[tokio::test]
    async fn after_hook_error_is_returned_when_nothing_else_failed() {
        let log = Arc::new(Mutex::new(Vec::new()));
        let pipeline = Pipeline::new(vec![probe(
            "a",
            "after a",
            Invoke::Continue,
            After::Fail,
            &log,
        )]);
        let mut ctx = MockContext::new(Arc::clone(&log));

        let error = pipeline
            .run(&mut ctx, final_ok)
            .await
            .expect_err("after hook error surfaces when it is the only failure");

        assert!(status_500(&error, "after a failed"));
        assert_eq!(*log.lock().unwrap(), vec!["a", "final", "after a"]);
    }
}
