//! End-to-end webhook ingress: route macro + raw-body binding + runtime + ack.
//!
//! This is the compile/runtime proof that an application needs only the route
//! declaration plus a short handler: the request type binds the exact raw body
//! (no JSON parsing before signature verification) and the provider ack is
//! written by the adapter.

use std::net::TcpListener;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use webx::{handler, ActionResult, AppMode, Host, IMediator, IRequestHandler, Inject, Response};
use webx_pay::{
    payment_webhook, text_success_ack, CreatePayment, IPaymentProvider, IPaymentWebhookVerifier,
    PaymentAccount, PaymentError, PaymentEvent, PaymentEventType, PaymentIntent, PaymentScope,
    Payments, PaymentsServiceCollectionExt, ProviderCapabilities, ProviderId, WebhookCapability,
    WebhookPayload,
};

static CAPTURED: Mutex<Vec<PaymentEvent>> = Mutex::new(Vec::new());

struct FakeVerifier;

#[async_trait]
impl IPaymentWebhookVerifier for FakeVerifier {
    fn provider(&self) -> ProviderId {
        ProviderId::Stripe
    }

    async fn verify(
        &self,
        scope: &PaymentScope,
        payload: &WebhookPayload,
    ) -> Result<PaymentEvent, PaymentError> {
        if payload.header("stripe-signature") != Some("verified") {
            return Err(PaymentError::signature(
                ProviderId::Stripe,
                "missing fixture header",
            ));
        }
        let mut event = PaymentEvent::new(
            ProviderId::Stripe,
            PaymentEventType::Succeeded,
            "evt_test_1",
        );
        event.account = scope.account_ref().cloned();
        event.order_id = Some("ORDER-1".to_string());
        event.amount = Some(webx_pay::Money::cny(9900));
        event.raw = payload.json()?;
        Ok(event)
    }

    fn ack(&self) -> Response {
        text_success_ack()
    }
}

struct FakeProvider {
    verifier: FakeVerifier,
}

#[async_trait]
impl IPaymentProvider for FakeProvider {
    fn id(&self) -> ProviderId {
        ProviderId::Stripe
    }

    fn accounts(&self) -> &[PaymentAccount] {
        &[]
    }

    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities::basic(WebhookCapability::HmacSha256)
    }

    fn validate(&self) -> Result<(), PaymentError> {
        Ok(())
    }

    async fn create_payment(
        &self,
        _scope: &PaymentScope,
        _request: CreatePayment,
    ) -> Result<PaymentIntent, PaymentError> {
        Err(PaymentError::internal("not used"))
    }

    async fn get_payment(
        &self,
        _scope: &PaymentScope,
        _order_id: &str,
    ) -> Result<PaymentIntent, PaymentError> {
        Err(PaymentError::internal("not used"))
    }

    fn webhook_verifier(&self) -> &dyn IPaymentWebhookVerifier {
        &self.verifier
    }
}

payment_webhook!(StripeWebhook, "/webhooks/payments/{provider}");

#[derive(Inject)]
struct WebhookHandler {
    #[inject]
    payments: Arc<Payments>,
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<StripeWebhook, Response> for WebhookHandler {
    async fn handle(&mut self, request: StripeWebhook) -> ActionResult<Response> {
        let event = self.payments.verify_webhook(&request).await?;
        CAPTURED.lock().expect("lock").push(event);
        self.payments.ack(&request).await.map_err(webx::Error::from)
    }
}

fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .expect("bind")
        .local_addr()
        .expect("addr")
        .port()
}

#[tokio::test(flavor = "multi_thread")]
async fn route_binds_raw_body_and_writes_the_provider_ack() {
    let port = free_port();
    let base = format!("http://127.0.0.1:{port}");

    let provider: Arc<dyn IPaymentProvider> = Arc::new(FakeProvider {
        verifier: FakeVerifier,
    });
    let host = Host::builder()
        .mode(AppMode::Development)
        .no_spa()
        .register(move |services| services.add_payment_provider(Arc::clone(&provider)))
        .build();

    let host_base = base.clone();
    tokio::spawn(async move {
        let _ = host.run_at(&host_base).await;
    });

    for _ in 0..100 {
        if reqwest::get(format!("{base}/health/live")).await.is_ok() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    let response = reqwest::Client::new()
        .post(format!("{base}/webhooks/payments/stripe"))
        .header("stripe-signature", "verified")
        .body(r#"{"id":"evt_test_1","data":{"object":{}}}"#)
        .send()
        .await
        .expect("webhook request");
    assert_eq!(response.status(), 200);
    let body = response.text().await.expect("body");
    assert_eq!(body, "success");

    {
        let captured = CAPTURED.lock().expect("lock");
        assert_eq!(captured.len(), 1);
        assert_eq!(captured[0].order_id.as_deref(), Some("ORDER-1"));
        assert_eq!(captured[0].dedupe_key, "evt_test_1");
    }

    // A request without the expected signature never reaches the business path.
    let response = reqwest::Client::new()
        .post(format!("{base}/webhooks/payments/stripe"))
        .body("{}")
        .send()
        .await
        .expect("bad webhook request");
    assert_eq!(response.status(), 400);
    assert_eq!(CAPTURED.lock().expect("lock").len(), 1);
}
