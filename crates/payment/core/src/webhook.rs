//! Raw-body webhook request type.
//!
//! Signature schemes are defined over the **exact** request body, so binding
//! must not parse JSON first. The type implements [`RequestModel`] by hand and
//! exposes the raw body plus the request headers. Applications declare the route
//! (there is no hidden route in the crate) through `payment_webhook!`:
//!
//! ```ignore
//! webx_pay::payment_webhook!(StripeWebhook, "/webhooks/payments/{provider}");
//! webx_pay::payment_webhook!(AccountWebhook, "/webhooks/payments/{provider}/{account}");
//! ```
//!
//! The optional `{account}` path segment is picked up automatically; there is no
//! separate request type for account-scoped callbacks.

use std::future::Future;

use bytes::Bytes;
use webx_core::error::{Error, Result as WebxResult};
use webx_core::http::{IHttpContext, IHttpRequest};
use webx_core::route::binding::RequestModel;
use webx_core::route::ParamMeta;

use crate::error::PaymentError;
use crate::ids::{AccountId, ProviderId};
use crate::provider::WebhookPayload;

/// Headers signature verification may need when an alternative host does not
/// implement [`IHttpRequest::header_pairs`].
const FALLBACK_HEADERS: &[&str] = &[
    "stripe-signature",
    "wechatpay-signature",
    "wechatpay-timestamp",
    "wechatpay-nonce",
    "wechatpay-serial",
    "paypal-transmission-id",
    "paypal-transmission-time",
    "paypal-transmission-sig",
    "paypal-cert-url",
    "paypal-auth-algo",
    "x-square-hmacsha256-signature",
];

/// Raw webhook request carrying the provider selected by the route.
#[derive(Debug, Clone)]
pub struct PaymentWebhook {
    provider: String,
    account: Option<String>,
    headers: Vec<(String, String)>,
    body: Bytes,
}

impl PaymentWebhook {
    /// Build a request directly (tests, custom transports).
    pub fn new(
        provider: impl Into<String>,
        account: Option<AccountId>,
        payload: WebhookPayload,
    ) -> Self {
        Self {
            provider: provider.into(),
            account: account.map(|id| id.as_str().to_string()),
            headers: payload.headers().to_vec(),
            body: payload.body_bytes().clone(),
        }
    }

    /// Raw path value for the provider segment.
    pub fn provider(&self) -> &str {
        &self.provider
    }

    /// Raw path value for the account segment, when present.
    pub fn account(&self) -> Option<&str> {
        self.account.as_deref()
    }

    /// Parsed provider id.
    pub fn provider_id(&self) -> Result<ProviderId, PaymentError> {
        self.provider.parse().map_err(|_| {
            PaymentError::validation(format!("unknown payment provider `{}`", self.provider))
        })
    }

    /// Parsed account id, when the route carried one.
    pub fn account_id(&self) -> Result<Option<AccountId>, PaymentError> {
        match &self.account {
            Some(value) => AccountId::new(value.clone())
                .map(Some)
                .map_err(PaymentError::validation),
            None => Ok(None),
        }
    }

    /// Signature-verification view.
    pub fn payload(&self) -> WebhookPayload {
        WebhookPayload::new(self.headers.clone(), self.body.clone())
    }

    /// Raw body.
    pub fn body(&self) -> &[u8] {
        &self.body
    }
}

impl RequestModel for PaymentWebhook {
    fn param_meta() -> &'static [ParamMeta] {
        &[]
    }

    #[allow(clippy::manual_async_fn)]
    fn bind_inline(
        ctx: &mut dyn IHttpContext,
    ) -> impl Future<Output = WebxResult<Self>> + Send + '_ {
        async move {
            let (provider, account, headers) = bind_parts(ctx).await?;
            let body = ctx.request_mut().body_bytes_shared().await?;
            Ok(Self {
                provider,
                account,
                headers,
                body,
            })
        }
    }
}

async fn bind_parts(
    ctx: &mut dyn IHttpContext,
) -> WebxResult<(String, Option<String>, Vec<(String, String)>)> {
    let (provider, account) = {
        let request = ctx.request();
        (
            request
                .route_params()
                .get("provider")
                .cloned()
                .unwrap_or_default(),
            request.route_params().get("account").cloned(),
        )
    };
    if provider.trim().is_empty() {
        return Err(Error::validation(
            "webhook route is missing the `{provider}` path segment".to_string(),
        ));
    }
    let headers = collect_headers(ctx.request());
    Ok((provider, account, headers))
}

fn collect_headers(request: &dyn IHttpRequest) -> Vec<(String, String)> {
    let mut headers: Vec<(String, String)> = request
        .header_pairs()
        .map(|(name, value)| (name.to_string(), value.to_string()))
        .collect();
    for name in FALLBACK_HEADERS {
        if headers
            .iter()
            .any(|(existing, _)| existing.eq_ignore_ascii_case(name))
        {
            continue;
        }
        if let Some(value) = request.header(name) {
            headers.push(((*name).to_string(), value.to_string()));
        }
    }
    headers
}

/// Declare an application-owned webhook route for a payment adapter.
///
/// Rust's orphan rule forbids implementing the framework's request trait for a
/// foreign raw-request type in an application crate, so the macro generates a
/// local newtype that binds the raw request and declares the route. The route
/// may carry an optional `{account}` segment; it is picked up automatically.
///
/// ```ignore
/// webx_pay::payment_webhook!(StripeWebhook, "/webhooks/payments/{provider}");
/// webx_pay::payment_webhook!(AccountWebhook, "/webhooks/payments/{provider}/{account}");
/// ```
///
/// The generated type derefs to [`PaymentWebhook`], so the handler calls
/// `payments.verify_webhook(&req)` and `payments.ack(req.provider())` directly.
#[macro_export]
macro_rules! payment_webhook {
    ($name:ident, $path:literal) => {
        #[derive(Debug, Clone)]
        pub struct $name(pub $crate::PaymentWebhook);

        impl $name {
            /// Unwrap the raw payment webhook request.
            pub fn into_inner(self) -> $crate::PaymentWebhook {
                self.0
            }
        }

        impl ::core::ops::Deref for $name {
            type Target = $crate::PaymentWebhook;

            fn deref(&self) -> &Self::Target {
                &self.0
            }
        }

        impl ::webx::RequestModel for $name {
            fn param_meta() -> &'static [::webx::ParamMeta] {
                <$crate::PaymentWebhook as ::webx::RequestModel>::param_meta()
            }

            #[allow(clippy::manual_async_fn)]
            fn bind_inline(
                ctx: &mut dyn ::webx::IHttpContext,
            ) -> impl ::core::future::Future<Output = ::webx::Result<Self>> + Send + '_ {
                async move {
                    let inner =
                        <$crate::PaymentWebhook as ::webx::RequestModel>::bind_inline(ctx).await?;
                    Ok(Self(inner))
                }
            }
        }

        #[::webx::post($path)]
        impl ::webx::IRequest<::webx::Response> for $name {}
    };
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::WebhookPayload;

    #[test]
    fn payload_view_is_case_insensitive_and_preserves_body() {
        let request = PaymentWebhook::new(
            "stripe",
            None,
            WebhookPayload::new(
                [("Stripe-Signature".to_string(), "t=1,v1=x".to_string())],
                b"{\"id\":\"evt_1\"}".to_vec(),
            ),
        );
        assert_eq!(request.provider_id().unwrap(), ProviderId::Stripe);
        assert_eq!(
            request.payload().header("stripe-signature"),
            Some("t=1,v1=x")
        );
        assert_eq!(request.body(), b"{\"id\":\"evt_1\"}");
    }

    #[test]
    fn account_is_optional_and_validated_when_present() {
        let without = PaymentWebhook::new("alipay", None, WebhookPayload::new([], Vec::new()));
        assert!(without.account_id().unwrap().is_none());

        let with = PaymentWebhook::new(
            "alipay",
            Some(crate::ids::AccountId::from("acct_1")),
            WebhookPayload::new([], Vec::new()),
        );
        assert_eq!(with.account_id().unwrap().unwrap().as_str(), "acct_1");
        assert_eq!(with.account(), Some("acct_1"));
    }
}
