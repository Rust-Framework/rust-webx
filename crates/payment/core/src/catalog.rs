//! Frontend-facing payment method catalog.
//!
//! The catalog tells a checkout page which provider/account/method/flow
//! combinations are currently enabled, plus the **public** client configuration
//! each frontend SDK needs. It never contains secret keys.
//!
//! Applications expose it through their own endpoint; the framework supplies the
//! typed DTO and the runtime enumeration:
//!
//! ```ignore
//! let options = payments.payment_options_for(&request.user).await?;
//! Ok(Json(options))
//! ```

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::ids::{AccountId, ProviderId};
use crate::model::{PaymentFlow, PaymentMethod};

/// How the client completes a payment (URL-free, catalog-safe form).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PaymentFlowKind {
    /// Client SDK confirms a server-created intent (Stripe Elements, Adyen Drop-in, ...).
    ClientSdk,
    /// Browser redirects to a provider-hosted page.
    Redirect,
    /// Client renders a QR code.
    QrCode,
    /// Provider app SDK is opened with the returned payload.
    App,
}

impl PaymentFlowKind {
    /// Convert a concrete [`PaymentFlow`] into its catalog-safe kind.
    pub fn of(flow: &PaymentFlow) -> Self {
        match flow {
            PaymentFlow::ClientSdk { .. } => Self::ClientSdk,
            PaymentFlow::Redirect { .. } => Self::Redirect,
            PaymentFlow::QrCode => Self::QrCode,
            PaymentFlow::App { .. } => Self::App,
        }
    }
}

/// One selectable payment option for the frontend.
///
/// `client_config` is provider-specific **public** configuration (publishable
/// key, app id, client key, location id, environment, ...). Adapters must never
/// put secret keys into it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PaymentOption {
    /// Provider serving the option.
    pub provider: ProviderId,
    /// Human-readable provider name.
    pub provider_name: String,
    /// Merchant account the option belongs to.
    pub account: AccountId,
    /// Payment method/channel.
    pub method: PaymentMethod,
    /// Default display name (localizable by the frontend).
    pub display_name: String,
    /// Default description (localizable by the frontend).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Stable icon key mapped to a frontend asset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    /// Optional accent color (`#RRGGBB`) for the method tile.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub brand_color: Option<String>,
    /// Flows the frontend can use with this method.
    pub flows: Vec<PaymentFlowKind>,
    /// Public client configuration for the provider SDK, when needed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_config: Option<Value>,
    /// Optional human-readable label for display.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// Display order hint (lower comes first); ties keep adapter order.
    #[serde(default)]
    pub sort_order: i32,
}

impl PaymentOption {
    /// Build an option with provider/method display defaults.
    pub fn new(
        provider: ProviderId,
        account: AccountId,
        method: PaymentMethod,
        flows: impl IntoIterator<Item = PaymentFlowKind>,
    ) -> Self {
        Self {
            provider_name: provider.display_name().to_string(),
            display_name: method.display_name().to_string(),
            description: Some(method.description().to_string()),
            icon: Some(method.icon_key().to_string()),
            brand_color: None,
            provider,
            account,
            method,
            flows: flows.into_iter().collect(),
            client_config: None,
            label: None,
            sort_order: 0,
        }
    }

    /// Override the display name (localization/custom branding).
    pub fn with_display_name(mut self, display_name: impl Into<String>) -> Self {
        self.display_name = display_name.into();
        self
    }

    /// Override the description.
    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    /// Override the icon key.
    pub fn with_icon(mut self, icon: impl Into<String>) -> Self {
        self.icon = Some(icon.into());
        self
    }

    /// Set an accent color (`#RRGGBB`).
    pub fn with_brand_color(mut self, brand_color: impl Into<String>) -> Self {
        self.brand_color = Some(brand_color.into());
        self
    }

    /// Set the display order hint.
    pub fn with_sort_order(mut self, sort_order: i32) -> Self {
        self.sort_order = sort_order;
        self
    }

    /// Attach public client configuration.
    pub fn with_client_config(mut self, client_config: Value) -> Self {
        self.client_config = Some(client_config);
        self
    }

    /// Attach a display label.
    pub fn with_label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn option_serializes_without_secrets_and_with_provider_names() {
        let option = PaymentOption::new(
            ProviderId::Stripe,
            AccountId::from("acct_1"),
            PaymentMethod::Card,
            [PaymentFlowKind::ClientSdk, PaymentFlowKind::Redirect],
        )
        .with_client_config(json!({ "publishable_key": "pk_test_123" }));
        let value = serde_json::to_value(&option).unwrap();
        assert_eq!(value["provider"], "stripe");
        assert_eq!(value["provider_name"], "Stripe");
        assert_eq!(value["account"], "acct_1");
        assert_eq!(value["method"], "card");
        assert_eq!(value["display_name"], "Card");
        assert_eq!(value["icon"], "card");
        assert!(value["description"].as_str().unwrap().contains("card"));
        assert_eq!(value["flows"], json!(["client_sdk", "redirect"]));
        assert_eq!(value["client_config"]["publishable_key"], "pk_test_123");
        assert!(serde_json::from_value::<PaymentOption>(value).is_ok());
    }

    #[test]
    fn flow_kind_drops_urls() {
        let kind = PaymentFlowKind::of(&PaymentFlow::Redirect {
            success_url: "https://example.test/ok".into(),
            cancel_url: None,
        });
        assert_eq!(kind, PaymentFlowKind::Redirect);
        assert_eq!(
            PaymentFlowKind::of(&PaymentFlow::QrCode),
            PaymentFlowKind::QrCode
        );
    }
}
