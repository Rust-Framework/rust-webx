//! Signaling control plane and TURN REST credentials (2.4-1).
//!
//! The framework owns the **control plane** only: SDP offers/answers and ICE
//! candidates travel as typed [`SignalingMessage`]s through the real-time
//! channel (typically message-as-request), and TURN credentials are derived
//! per the coturn REST API (`username = expiry:user`, `credential =
//! base64(HMAC-SHA1(secret, username))`).
//!
//! Media itself (SFU, codecs, RTP) is deliberately **not** in core; an SFU or
//! media server integrates through the media adapter contract (2.4-3) and gets
//! its credentials from [`TurnRestConfig`].

use std::time::{SystemTime, UNIX_EPOCH};

use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha1::Sha1;

use crate::error::{Error, Result};

type HmacSha1 = Hmac<Sha1>;

/// Default TURN credential lifetime (seconds).
pub const DEFAULT_TURN_TTL_SECONDS: u64 = 3600;

/// Hard upper bound for credential lifetime (a day).
pub const MAX_TURN_TTL_SECONDS: u64 = 86_400;

/// Kind of signaling payload carried by [`SignalingMessage`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SignalingKind {
    /// SDP offer from the caller.
    Offer,
    /// SDP answer from the callee.
    Answer,
    /// One ICE candidate (trickle ICE).
    IceCandidate,
}

/// One signaling message between call participants.
///
/// `to` targets a specific user (1:1 call / targeted negotiation); `None`
/// broadcasts to the whole room (group call). `from` is filled by the server
/// from the authenticated connection — clients must not be trusted for it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignalingMessage {
    /// Room (call) this message belongs to.
    pub room_id: String,
    /// Sender user id, stamped by the server.
    pub from: String,
    /// Target user id; `None` = broadcast to the room.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to: Option<String>,
    /// Offer / answer / ICE candidate.
    pub kind: SignalingKind,
    /// Opaque payload (SDP or candidate JSON).
    pub payload: serde_json::Value,
}

impl SignalingMessage {
    /// Build a server-stamped message.
    pub fn new(
        room_id: impl Into<String>,
        from: impl Into<String>,
        to: Option<String>,
        kind: SignalingKind,
        payload: serde_json::Value,
    ) -> Self {
        Self {
            room_id: room_id.into(),
            from: from.into(),
            to,
            kind,
            payload,
        }
    }
}

/// coturn `use-auth-secret` (REST API) configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TurnRestConfig {
    /// Shared secret from `static-auth-secret=`.
    pub secret: String,
    /// TURN/STUN URLs handed to clients (e.g. `turn:turn.example.com:3478`).
    pub urls: Vec<String>,
    /// Credential lifetime in seconds (default 1h, max 24h).
    #[serde(default = "default_ttl")]
    pub ttl_seconds: u64,
    /// Optional TURN realm echoed to clients.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub realm: Option<String>,
}

fn default_ttl() -> u64 {
    DEFAULT_TURN_TTL_SECONDS
}

impl TurnRestConfig {
    /// Configure with a secret and at least one URL.
    pub fn new(secret: impl Into<String>, urls: Vec<String>) -> Self {
        Self {
            secret: secret.into(),
            urls,
            ttl_seconds: DEFAULT_TURN_TTL_SECONDS,
            realm: None,
        }
    }

    /// Override the credential lifetime.
    pub fn with_ttl(mut self, ttl_seconds: u64) -> Self {
        self.ttl_seconds = ttl_seconds;
        self
    }

    /// Attach a realm.
    pub fn with_realm(mut self, realm: impl Into<String>) -> Self {
        self.realm = Some(realm.into());
        self
    }

    /// Reject unusable configuration instead of issuing broken credentials.
    ///
    /// Configuration errors are operator-facing: they surface as 500s (the app
    /// logs them at startup/request time), not as client validation failures.
    pub fn validate(&self) -> Result<()> {
        if self.secret.is_empty() {
            return Err(Error::Internal("Turn.Secret must not be empty".to_string()));
        }
        if self.urls.is_empty() {
            return Err(Error::Internal(
                "Turn.Urls must contain at least one TURN/STUN URL".to_string(),
            ));
        }
        if self.ttl_seconds == 0 || self.ttl_seconds > MAX_TURN_TTL_SECONDS {
            return Err(Error::Internal(format!(
                "Turn.TtlSeconds must be in 1..={MAX_TURN_TTL_SECONDS}"
            )));
        }
        Ok(())
    }

    /// Build the runtime configuration from the `Turn` configuration section.
    ///
    /// Returns a validated [`TurnRestConfig`]; a partially filled section is a
    /// startup error, never a silently disabled feature.
    pub fn from_section(section: &crate::config::TurnSection) -> Result<Self> {
        let config = Self {
            secret: section.secret.clone(),
            urls: section.urls.clone(),
            ttl_seconds: section.ttl_seconds,
            realm: section.realm.clone(),
        };
        config.validate()?;
        Ok(config)
    }

    /// Issue ephemeral credentials for `user_id` at the current time.
    pub fn credentials_for(&self, user_id: &str) -> Result<TurnCredentials> {
        self.credentials_for_at(user_id, now_unix())
    }

    /// Issue credentials at a fixed timestamp (tests / clock injection).
    pub fn credentials_for_at(&self, user_id: &str, now_unix: u64) -> Result<TurnCredentials> {
        self.validate()?;
        if user_id.is_empty() {
            return Err(Error::Validation("turn user id is required".to_string()));
        }
        let expiry = now_unix.saturating_add(self.ttl_seconds);
        let username = format!("{expiry}:{user_id}");
        let credential = hmac_sha1_base64(&self.secret, &username)?;
        Ok(TurnCredentials {
            urls: self.urls.clone(),
            username,
            credential,
            ttl_seconds: self.ttl_seconds,
            realm: self.realm.clone(),
        })
    }
}

/// Ephemeral TURN credentials for one client (safe to serialize to the client).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TurnCredentials {
    /// TURN/STUN URLs.
    pub urls: Vec<String>,
    /// coturn REST username (`<expiry-unix>:<user-id>`).
    pub username: String,
    /// `base64(HMAC-SHA1(secret, username))`.
    pub credential: String,
    /// Lifetime in seconds.
    pub ttl_seconds: u64,
    /// Realm, when configured.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub realm: Option<String>,
}

/// `base64(HMAC-SHA1(secret, message))` as used by the coturn REST API.
pub fn hmac_sha1_base64(secret: &str, message: &str) -> Result<String> {
    let mut mac = HmacSha1::new_from_slice(secret.as_bytes())
        .map_err(|error| Error::Internal(format!("TURN HMAC key is invalid: {error}")))?;
    mac.update(message.as_bytes());
    Ok(data_encoding::BASE64.encode(&mac.finalize().into_bytes()))
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn turn_rest_credentials_match_the_coturn_vector() {
        // coturn REST API: username = "<expiry>:<user>",
        // credential = base64(HMAC-SHA1(secret, username)).
        let config = TurnRestConfig::new("secret", vec!["turn:turn.example:3478".to_string()])
            .with_ttl(3600);
        // now = 1_700_000_000 - 3600 so the username expiry is 1_700_000_000.
        let credentials = config
            .credentials_for_at("alice", 1_699_996_400)
            .expect("credentials");
        assert_eq!(credentials.username, "1700000000:alice");
        assert_eq!(credentials.credential, "d8soP47RbdIKLDUOpnJPVQyq5Ts=");
        assert_eq!(credentials.ttl_seconds, 3600);
        assert_eq!(credentials.urls, vec!["turn:turn.example:3478"]);
    }

    #[test]
    fn turn_config_builds_from_the_app_options_section() {
        let section = crate::config::TurnSection {
            secret: "secret".to_string(),
            urls: vec!["turn:turn.example:3478".to_string()],
            ttl_seconds: 120,
            ..Default::default()
        };
        let config = TurnRestConfig::from_section(&section).expect("valid section");
        assert_eq!(config.ttl_seconds, 120);
        assert_eq!(config.urls, vec!["turn:turn.example:3478"]);

        // A half-filled section is a startup error, not a silent default.
        let empty = crate::config::TurnSection::default();
        assert!(TurnRestConfig::from_section(&empty).is_err());
    }

    #[test]
    fn turn_config_validation_rejects_unusable_values() {
        let empty_secret = TurnRestConfig::new("", vec!["turn:x".to_string()]);
        assert!(matches!(empty_secret.validate(), Err(Error::Internal(_))));

        let empty_urls = TurnRestConfig::new("secret", Vec::new());
        assert!(matches!(empty_urls.validate(), Err(Error::Internal(_))));

        let zero_ttl = TurnRestConfig::new("secret", vec!["turn:x".to_string()]).with_ttl(0);
        assert!(matches!(zero_ttl.validate(), Err(Error::Internal(_))));

        let too_long = TurnRestConfig::new("secret", vec!["turn:x".to_string()])
            .with_ttl(MAX_TURN_TTL_SECONDS + 1);
        assert!(matches!(too_long.validate(), Err(Error::Internal(_))));

        let valid = TurnRestConfig::new("secret", vec!["turn:x".to_string()]);
        valid.validate().expect("valid config");
        assert!(valid.credentials_for("").is_err(), "empty user rejected");
    }

    #[test]
    fn signaling_messages_round_trip_and_stamp_server_fields() {
        let message = SignalingMessage::new(
            "room-1",
            "user-1",
            Some("user-2".to_string()),
            SignalingKind::Offer,
            serde_json::json!({ "sdp": "v=0", "type": "offer" }),
        );
        let json = serde_json::to_value(&message).expect("serialize");
        assert_eq!(json["kind"], "offer");
        assert_eq!(json["to"], "user-2");
        let back: SignalingMessage = serde_json::from_value(json).expect("deserialize");
        assert_eq!(back.from, "user-1");
        assert_eq!(back.kind, SignalingKind::Offer);
    }
}
