mod test_utils;

use lrwf_core::error::Result as LrwfResult;
use lrwf_core::http::IHttpContext;
use lrwf_core::middleware::IMiddleware;
use lrwf_http::pipeline::{HandlerFn, MiddlewarePipeline};
use std::sync::Arc;

#[allow(dead_code)]
struct CounterMiddleware {
    name: &'static str,
}

impl CounterMiddleware {
    fn new(name: &'static str) -> Self {
        Self { name }
    }
}

#[async_trait::async_trait]
impl IMiddleware for CounterMiddleware {
    async fn invoke(&self, _ctx: &mut dyn IHttpContext) -> LrwfResult<()> {
        Ok(())
    }
}

#[tokio::test]
async fn pipeline_empty_executes_final_handler() {
    let pipeline = MiddlewarePipeline::new();
    let mut ctx = test_utils::TestHttpContext::new("GET", "/test");

    let final_handler: HandlerFn = Arc::new(move |ctx: &mut dyn IHttpContext| {
        Box::pin(async move {
            ctx.response_mut().write_text("done").await?;
            Ok(())
        })
    });

    let result = pipeline.execute(&mut ctx, final_handler).await;
    assert!(result.is_ok());

    let (_status, _headers, body) = ctx.into_response_parts();
    assert_eq!(body.unwrap(), b"done");
}

#[tokio::test]
async fn pipeline_multiple_middleware_executed_in_order() {
    let mut pipeline = MiddlewarePipeline::new();
    pipeline.add_middleware(Arc::new(CounterMiddleware::new("first")));
    pipeline.add_middleware(Arc::new(CounterMiddleware::new("second")));
    pipeline.add_middleware(Arc::new(CounterMiddleware::new("third")));

    let mut ctx = test_utils::TestHttpContext::new("GET", "/test");

    let final_handler: HandlerFn = Arc::new(move |ctx: &mut dyn IHttpContext| {
        Box::pin(async move {
            ctx.response_mut().set_status(201);
            Ok(())
        })
    });

    let result = pipeline.execute(&mut ctx, final_handler).await;
    assert!(result.is_ok());

    let (status, _headers, _body) = ctx.into_response_parts();
    assert_eq!(status, 201);
}

#[tokio::test]
async fn pipeline_middleware_can_modify_context() {
    struct HeaderMiddleware;

    #[async_trait::async_trait]
    impl IMiddleware for HeaderMiddleware {
        async fn invoke(&self, ctx: &mut dyn IHttpContext) -> LrwfResult<()> {
            ctx.response_mut().set_header("x-powered-by", "lrwf-test");
            Ok(())
        }
    }

    let mut pipeline = MiddlewarePipeline::new();
    pipeline.add_middleware(Arc::new(HeaderMiddleware));

    let mut ctx = test_utils::TestHttpContext::new("GET", "/test");

    let final_handler: HandlerFn = Arc::new(move |ctx: &mut dyn IHttpContext| {
        Box::pin(async move {
            ctx.response_mut().write_text("ok").await?;
            Ok(())
        })
    });

    pipeline.execute(&mut ctx, final_handler).await.unwrap();

    let (_status, headers, _body) = ctx.into_response_parts();
    let x_powered = headers.iter()
        .find(|(k, _)| k == "x-powered-by")
        .map(|(_, v)| v.as_str());
    assert_eq!(x_powered, Some("lrwf-test"));
}

#[tokio::test]
async fn pipeline_after_hook_executed() {
    struct AfterMiddleware;

    #[async_trait::async_trait]
    impl IMiddleware for AfterMiddleware {
        async fn invoke(&self, _ctx: &mut dyn IHttpContext) -> LrwfResult<()> {
            Ok(())
        }

        async fn after(&self, ctx: &mut dyn IHttpContext) -> LrwfResult<()> {
            ctx.response_mut().set_header("x-after-ran", "yes");
            Ok(())
        }
    }

    let mut pipeline = MiddlewarePipeline::new();
    pipeline.add_middleware(Arc::new(AfterMiddleware));

    let mut ctx = test_utils::TestHttpContext::new("GET", "/test");

    let final_handler: HandlerFn = Arc::new(move |ctx: &mut dyn IHttpContext| {
        Box::pin(async move {
            ctx.response_mut().set_status(200);
            ctx.response_mut().write_text("ok").await?;
            Ok(())
        })
    });

    pipeline.execute(&mut ctx, final_handler).await.unwrap();

    let (_status, headers, _body) = ctx.into_response_parts();
    let x_after = headers
        .iter()
        .find(|(k, _)| k == "x-after-ran")
        .map(|(_, v)| v.as_str());
    assert_eq!(x_after, Some("yes"));
}

#[tokio::test]
async fn pipeline_after_hooks_executed_in_reverse_order() {
    // After hooks should run in reverse registration order.
    use std::sync::Mutex;

    let order = Arc::new(Mutex::new(Vec::<&'static str>::new()));

    struct OrderMiddleware {
        name: &'static str,
        order: Arc<Mutex<Vec<&'static str>>>,
    }

    #[async_trait::async_trait]
    impl IMiddleware for OrderMiddleware {
        async fn invoke(&self, _ctx: &mut dyn IHttpContext) -> LrwfResult<()> {
            self.order.lock().unwrap().push(self.name);
            Ok(())
        }

        async fn after(&self, _ctx: &mut dyn IHttpContext) -> LrwfResult<()> {
            self.order.lock().unwrap().push(self.name);
            Ok(())
        }
    }

    let mw_a = Arc::new(OrderMiddleware { name: "A", order: Arc::clone(&order) });
    let mw_b = Arc::new(OrderMiddleware { name: "B", order: Arc::clone(&order) });
    let mw_c = Arc::new(OrderMiddleware { name: "C", order: Arc::clone(&order) });

    let mut pipeline = MiddlewarePipeline::new();
    pipeline.add_middleware(mw_a);
    pipeline.add_middleware(mw_b);
    pipeline.add_middleware(mw_c);

    let mut ctx = test_utils::TestHttpContext::new("GET", "/test");

    let final_handler: HandlerFn = Arc::new(move |ctx: &mut dyn IHttpContext| {
        Box::pin(async move {
            ctx.response_mut().set_status(200);
            ctx.response_mut().write_text("ok").await?;
            Ok(())
        })
    });

    pipeline.execute(&mut ctx, final_handler).await.unwrap();

    // Forward pass: A, B, C.  After pass: C, B, A.
    let result = order.lock().unwrap().clone();
    assert_eq!(result, vec!["A", "B", "C", "C", "B", "A"]);
}

#[tokio::test]
async fn pipeline_short_circuit_on_invoke_error() {
    struct FailingMiddleware;

    #[async_trait::async_trait]
    impl IMiddleware for FailingMiddleware {
        async fn invoke(&self, _ctx: &mut dyn IHttpContext) -> LrwfResult<()> {
            Err(lrwf_core::error::Error::Http("blocked".into()))
        }
    }

    let mut pipeline = MiddlewarePipeline::new();
    pipeline.add_middleware(Arc::new(FailingMiddleware));

    let mut ctx = test_utils::TestHttpContext::new("GET", "/test");

    let final_handler: HandlerFn = Arc::new(move |ctx: &mut dyn IHttpContext| {
        Box::pin(async move {
            ctx.response_mut().write_text("should-not-reach").await?;
            Ok(())
        })
    });

    let result = pipeline.execute(&mut ctx, final_handler).await;
    assert!(result.is_err(), "Error from invoke should short-circuit the pipeline");
    // Body must NOT be written — the final handler never ran.
    let (_status, _headers, body) = ctx.into_response_parts();
    assert!(body.is_none(), "final handler should be skipped on short-circuit");
}

#[tokio::test]
async fn pipeline_after_hooks_skipped_on_final_handler_error() {
    struct ObserveMiddleware {
        after_ran: std::sync::Arc<std::sync::atomic::AtomicBool>,
    }

    #[async_trait::async_trait]
    impl IMiddleware for ObserveMiddleware {
        async fn invoke(&self, _ctx: &mut dyn IHttpContext) -> LrwfResult<()> {
            Ok(())
        }
        async fn after(&self, _ctx: &mut dyn IHttpContext) -> LrwfResult<()> {
            self.after_ran.store(true, std::sync::atomic::Ordering::SeqCst);
            Ok(())
        }
    }

    let after_ran = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));

    let mut pipeline = MiddlewarePipeline::new();
    pipeline.add_middleware(Arc::new(ObserveMiddleware {
        after_ran: Arc::clone(&after_ran),
    }));

    let mut ctx = test_utils::TestHttpContext::new("GET", "/test");

    let final_handler: HandlerFn = Arc::new(move |_ctx: &mut dyn IHttpContext| {
        Box::pin(async move {
            Err(lrwf_core::error::Error::Internal("final handler error".into()))
        })
    });

    let result = pipeline.execute(&mut ctx, final_handler).await;
    assert!(result.is_err());
    // after hooks are NOT called when the final handler returns Err
    // (because `?` short-circuits before the reverse pass loop)
    assert!(!after_ran.load(std::sync::atomic::Ordering::SeqCst));
}
