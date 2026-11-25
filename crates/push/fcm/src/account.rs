//! FCM service-account credentials.

use std::fmt;

use serde::Deserialize;
use webx_core::error::{Error, Result};

/// Google's documented OAuth2 token endpoint, used when `token_uri` is absent.
pub(crate) const DEFAULT_TOKEN_URI: &str = "https://oauth2.googleapis.com/token";

/// The subset of a Firebase service-account JSON file the adapter needs.
///
/// Unknown members (`type`, `private_key_id`, `client_id`, timestamps, ...) are
/// ignored, so the file can be deserialized as-is:
///
/// ```ignore
/// let account: FcmServiceAccount = serde_json::from_str(&json)?;
/// ```
#[derive(Deserialize)]
pub struct FcmServiceAccount {
    /// Firebase project id (the `messages:send` path segment).
    pub project_id: String,
    /// Service-account email (the OAuth2 assertion `iss`).
    pub client_email: String,
    /// PKCS#8 RSA private key PEM (`private_key` in the JSON file).
    #[serde(rename = "private_key")]
    pub private_key_pem: String,
    /// OAuth2 token endpoint; defaults to Google's when missing or empty.
    #[serde(default = "default_token_uri")]
    pub token_uri: String,
}

fn default_token_uri() -> String {
    DEFAULT_TOKEN_URI.to_owned()
}

impl FcmServiceAccount {
    /// Parse the JSON subset of a service-account file.
    ///
    /// The error is a fixed message: nothing derived from the key material is
    /// echoed back.
    pub fn from_json(json: &str) -> Result<Self> {
        serde_json::from_str(json).map_err(|_| {
            Error::internal(
                "the FCM service account JSON is invalid or missing project_id, client_email or private_key",
            )
        })
    }

    /// Trim, default and validate the credentials before any request is made.
    pub(crate) fn normalize(mut self) -> Result<Self> {
        if self.project_id.trim().is_empty()
            || self.client_email.trim().is_empty()
            || self.private_key_pem.trim().is_empty()
        {
            return Err(Error::internal(
                "the FCM service account is incomplete: project_id, client_email and private_key are required",
            ));
        }
        self.project_id = self.project_id.trim().to_owned();
        self.client_email = self.client_email.trim().to_owned();
        self.token_uri = if self.token_uri.trim().is_empty() {
            DEFAULT_TOKEN_URI.to_owned()
        } else {
            self.token_uri.trim().to_owned()
        };
        Ok(self)
    }
}

impl fmt::Debug for FcmServiceAccount {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FcmServiceAccount")
            .field("project_id", &self.project_id)
            .field("client_email", &self.client_email)
            .field("private_key_pem", &"<redacted>")
            .field("token_uri", &self.token_uri)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SERVICE_ACCOUNT_JSON: &str = r#"{
        "type": "service_account",
        "project_id": "demo-project",
        "private_key_id": "key-1",
        "private_key": "-----BEGIN PRIVATE KEY-----\nabc\n-----END PRIVATE KEY-----\n",
        "client_email": "svc@demo-project.iam.gserviceaccount.com",
        "client_id": "1234",
        "token_uri": "https://oauth2.googleapis.com/token"
    }"#;

    #[test]
    fn parses_the_service_account_subset_and_ignores_unknown_members() {
        let account = FcmServiceAccount::from_json(SERVICE_ACCOUNT_JSON).expect("account");
        assert_eq!(account.project_id, "demo-project");
        assert_eq!(
            account.client_email,
            "svc@demo-project.iam.gserviceaccount.com"
        );
        assert_eq!(account.token_uri, DEFAULT_TOKEN_URI);
        assert!(account.private_key_pem.contains("BEGIN PRIVATE KEY"));
    }

    #[test]
    fn a_missing_token_uri_falls_back_to_googles_endpoint() {
        let json = r#"{
            "project_id": "demo-project",
            "client_email": "svc@example.iam.gserviceaccount.com",
            "private_key": "-----BEGIN PRIVATE KEY-----\nabc\n-----END PRIVATE KEY-----\n"
        }"#;
        let account = FcmServiceAccount::from_json(json).expect("account");
        assert_eq!(account.token_uri, DEFAULT_TOKEN_URI);
    }

    #[test]
    fn an_incomplete_account_is_rejected_with_a_fixed_message() {
        let error = FcmServiceAccount::from_json(r#"{"project_id":"p"}"#)
            .expect_err("missing key fields must fail");
        assert_eq!(error.status_code(), 500);
        assert!(error.public_detail().is_none());
    }

    #[test]
    fn debug_output_never_contains_the_private_key() {
        let account = FcmServiceAccount::from_json(SERVICE_ACCOUNT_JSON).expect("account");
        let debug = format!("{account:?}");
        assert!(!debug.contains("BEGIN PRIVATE KEY"));
        assert!(debug.contains("<redacted>"));
    }
}
