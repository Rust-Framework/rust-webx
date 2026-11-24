//! Media-plane adapter contract (2.4-3).
//!
//! Core owns the **contract**, never the media stack: an SFU / media server
//! (LiveKit, mediasoup, Janus, a recording cluster, ...) implements
//! [`IMediaPlaneAdapter`] and the application talks to it through this port.
//! The framework maps adapter QoS samples into the metrics port (`media_*`
//! series) so media health shows up next to HTTP/WebSocket health.
//!
//! A deterministic in-memory adapter ships with core: it satisfies the same
//! contract test any real adapter must pass, and doubles as a fake for tests.

use std::collections::{HashMap, HashSet};
use std::sync::Mutex;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::metrics::IMeter;

/// Kind of media track.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MediaTrackKind {
    /// Audio track.
    Audio,
    /// Video track.
    Video,
    /// Screen share / data channel.
    Screen,
}

impl MediaTrackKind {
    /// Stable metric label.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Audio => "audio",
            Self::Video => "video",
            Self::Screen => "screen",
        }
    }
}

/// One published/subscribed media track.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MediaTrackInfo {
    /// Track identifier (adapter-scoped).
    pub id: String,
    /// Track kind.
    pub kind: MediaTrackKind,
    /// Optional human-readable label.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// Whether the producer paused the track.
    #[serde(default)]
    pub paused: bool,
}

impl MediaTrackInfo {
    /// Build a track descriptor.
    pub fn new(id: impl Into<String>, kind: MediaTrackKind) -> Self {
        Self {
            id: id.into(),
            kind,
            label: None,
            paused: false,
        }
    }
}

/// Media session: one participant joined to one room.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MediaSession {
    /// Adapter-issued session id.
    pub session_id: String,
    /// Room the participant joined.
    pub room_id: String,
    /// Authenticated user id.
    pub user_id: String,
    /// Tracks this session publishes.
    pub published: Vec<MediaTrackInfo>,
    /// Tracks this session subscribes to.
    pub subscribed: Vec<MediaTrackInfo>,
}

/// QoS sample for one media session.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct MediaStats {
    /// Round-trip time in milliseconds.
    pub rtt_ms: f64,
    /// Jitter in milliseconds.
    pub jitter_ms: f64,
    /// Packet loss ratio (0.0 = none, 1.0 = 100%).
    pub packet_loss_ratio: f64,
    /// Estimated bitrate in kbit/s.
    pub bitrate_kbps: f64,
    /// Dropped frames since the last sample.
    pub frames_dropped: u64,
    /// Negotiated video resolution, when the session is a video call.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolution: Option<(u32, u32)>,
}

impl MediaStats {
    /// Zero-loss baseline sample (adapters fill real numbers).
    pub fn baseline() -> Self {
        Self {
            rtt_ms: 0.0,
            jitter_ms: 0.0,
            packet_loss_ratio: 0.0,
            bitrate_kbps: 0.0,
            frames_dropped: 0,
            resolution: None,
        }
    }

    /// Clamp negative/invalid values and loss into `0.0..=1.0` before exposing
    /// them to metrics.
    pub fn sanitized(&self) -> Self {
        Self {
            rtt_ms: self.rtt_ms.max(0.0),
            jitter_ms: self.jitter_ms.max(0.0),
            packet_loss_ratio: self.packet_loss_ratio.clamp(0.0, 1.0),
            bitrate_kbps: self.bitrate_kbps.max(0.0),
            frames_dropped: self.frames_dropped,
            resolution: self.resolution,
        }
    }
}

/// Media-plane adapter contract.
///
/// The framework never interprets SDP/RTP: adapters own negotiation and media
/// transport; this port only carries control operations and QoS samples.
#[async_trait]
pub trait IMediaPlaneAdapter: Send + Sync {
    /// Join `user_id` to `room_id`; returns the adapter session.
    async fn join(&self, room_id: &str, user_id: &str) -> Result<MediaSession>;

    /// Leave the session (idempotent).
    async fn leave(&self, session_id: &str) -> Result<()>;

    /// Publish tracks from a session (replaces the published set).
    async fn publish(&self, session_id: &str, tracks: Vec<MediaTrackInfo>) -> Result<()>;

    /// Subscribe a session to tracks (replaces the subscribed set).
    async fn subscribe(&self, session_id: &str, tracks: Vec<MediaTrackInfo>) -> Result<()>;

    /// Latest QoS sample for a session.
    async fn stats(&self, session_id: &str) -> Result<MediaStats>;

    /// Destroy a room and every session in it.
    async fn close_room(&self, room_id: &str) -> Result<()>;

    /// Push one QoS sample into the metrics port.
    ///
    /// Default implementation emits the framework's `media_*` series with
    /// `room`/`user` labels; adapters may override for additional series.
    fn record_qos(&self, meter: &dyn IMeter, session: &MediaSession, stats: &MediaStats) {
        let stats = stats.sanitized();
        let tags = [
            ("room", session.room_id.as_str()),
            ("user", session.user_id.as_str()),
        ];
        meter.gauge("media_rtt_ms", &tags).set(stats.rtt_ms);
        meter.gauge("media_jitter_ms", &tags).set(stats.jitter_ms);
        meter
            .gauge("media_packet_loss_ratio", &tags)
            .set(stats.packet_loss_ratio);
        meter
            .gauge("media_bitrate_kbps", &tags)
            .set(stats.bitrate_kbps);
        if stats.frames_dropped > 0 {
            meter
                .counter("media_frames_dropped_total", &tags)
                .add(stats.frames_dropped);
        }
    }
}

#[derive(Default)]
struct MemoryMediaState {
    next_session: u64,
    rooms: HashMap<String, HashSet<String>>,
    sessions: HashMap<String, MediaSession>,
    stats: HashMap<String, MediaStats>,
}

/// Deterministic in-memory media plane (fake adapter + contract reference).
#[derive(Default)]
pub struct InMemoryMediaPlaneAdapter {
    state: Mutex<MemoryMediaState>,
}

impl InMemoryMediaPlaneAdapter {
    /// Empty adapter.
    pub fn new() -> Self {
        Self::default()
    }

    /// Override the QoS sample returned for `session_id` (tests/demos).
    pub fn set_stats(&self, session_id: &str, stats: MediaStats) {
        let mut state = self.lock();
        state
            .stats
            .insert(session_id.to_string(), stats.sanitized());
    }

    /// Rooms that currently have at least one session.
    pub fn open_rooms(&self) -> usize {
        self.lock()
            .rooms
            .values()
            .filter(|sessions| !sessions.is_empty())
            .count()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, MemoryMediaState> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn session<'a>(
        &self,
        state: &'a MemoryMediaState,
        session_id: &str,
    ) -> Result<&'a MediaSession> {
        state
            .sessions
            .get(session_id)
            .ok_or_else(|| Error::not_found(format!("media session '{session_id}' not found")))
    }
}

#[async_trait]
impl IMediaPlaneAdapter for InMemoryMediaPlaneAdapter {
    async fn join(&self, room_id: &str, user_id: &str) -> Result<MediaSession> {
        if room_id.trim().is_empty() || user_id.trim().is_empty() {
            return Err(Error::validation(
                "room_id and user_id are required to join a media session",
            ));
        }
        let mut state = self.lock();
        state.next_session += 1;
        let session = MediaSession {
            session_id: format!("ms-{}", state.next_session),
            room_id: room_id.to_string(),
            user_id: user_id.to_string(),
            published: Vec::new(),
            subscribed: Vec::new(),
        };
        state
            .rooms
            .entry(room_id.to_string())
            .or_default()
            .insert(session.session_id.clone());
        state
            .stats
            .insert(session.session_id.clone(), MediaStats::baseline());
        state
            .sessions
            .insert(session.session_id.clone(), session.clone());
        Ok(session)
    }

    async fn leave(&self, session_id: &str) -> Result<()> {
        let mut state = self.lock();
        let Some(session) = state.sessions.remove(session_id) else {
            return Ok(());
        };
        if let Some(sessions) = state.rooms.get_mut(&session.room_id) {
            sessions.remove(session_id);
            if sessions.is_empty() {
                state.rooms.remove(&session.room_id);
            }
        }
        state.stats.remove(session_id);
        Ok(())
    }

    async fn publish(&self, session_id: &str, tracks: Vec<MediaTrackInfo>) -> Result<()> {
        let mut state = self.lock();
        let session = self.session(&state, session_id)?.clone();
        let updated = MediaSession {
            published: tracks,
            ..session
        };
        state.sessions.insert(session_id.to_string(), updated);
        Ok(())
    }

    async fn subscribe(&self, session_id: &str, tracks: Vec<MediaTrackInfo>) -> Result<()> {
        let mut state = self.lock();
        let session = self.session(&state, session_id)?.clone();
        let updated = MediaSession {
            subscribed: tracks,
            ..session
        };
        state.sessions.insert(session_id.to_string(), updated);
        Ok(())
    }

    async fn stats(&self, session_id: &str) -> Result<MediaStats> {
        let state = self.lock();
        self.session(&state, session_id)?;
        Ok(state
            .stats
            .get(session_id)
            .copied()
            .unwrap_or_else(MediaStats::baseline))
    }

    async fn close_room(&self, room_id: &str) -> Result<()> {
        let mut state = self.lock();
        let sessions = state.rooms.remove(room_id).unwrap_or_default();
        for session_id in sessions {
            state.sessions.remove(&session_id);
            state.stats.remove(&session_id);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::metrics::MemoryMeter;

    /// Contract every media adapter must satisfy (run against the reference
    /// adapter below and against real SFU adapters in their own crates).
    async fn contract_suite(adapter: &dyn IMediaPlaneAdapter) {
        let first = adapter.join("room-1", "alice").await.expect("join alice");
        let second = adapter.join("room-1", "bob").await.expect("join bob");
        assert_ne!(first.session_id, second.session_id);
        assert_eq!(first.room_id, "room-1");
        assert!(matches!(
            adapter.join("", "eve").await,
            Err(error) if error.code() == "validation_failed"
        ));

        // Publish/subscribe replace the track sets.
        adapter
            .publish(
                &first.session_id,
                vec![MediaTrackInfo::new("cam", MediaTrackKind::Video)],
            )
            .await
            .expect("publish");
        adapter
            .subscribe(
                &first.session_id,
                vec![MediaTrackInfo::new("remote-cam", MediaTrackKind::Video)],
            )
            .await
            .expect("subscribe");
        assert!(matches!(
            adapter
                .publish(
                    "missing",
                    vec![MediaTrackInfo::new("x", MediaTrackKind::Audio)]
                )
                .await,
            Err(error) if error.code() == "not_found"
        ));

        // Baseline stats are well-formed; loss is clamped into 0..=1.
        let stats = adapter.stats(&first.session_id).await.expect("stats");
        assert!(stats.rtt_ms >= 0.0);
        assert!((0.0..=1.0).contains(&stats.packet_loss_ratio));
        assert!(matches!(
            adapter.stats("missing").await,
            Err(error) if error.code() == "not_found"
        ));

        // QoS ingestion writes the framework's media_* series.
        let meter = MemoryMeter::new();
        let sample = MediaStats {
            rtt_ms: 42.0,
            jitter_ms: 3.5,
            packet_loss_ratio: 1.7, // clamped to 1.0
            bitrate_kbps: 900.0,
            frames_dropped: 3,
            resolution: Some((1280, 720)),
        };
        adapter.record_qos(
            &meter,
            &MediaSession {
                session_id: first.session_id.clone(),
                room_id: first.room_id.clone(),
                user_id: first.user_id.clone(),
                published: Vec::new(),
                subscribed: Vec::new(),
            },
            &sample,
        );
        let tags = [("room", "room-1"), ("user", "alice")];
        assert_eq!(meter.gauge_value("media_rtt_ms", &tags), 42.0);
        assert_eq!(meter.gauge_value("media_jitter_ms", &tags), 3.5);
        assert_eq!(
            meter.gauge_value("media_packet_loss_ratio", &tags),
            1.0,
            "loss is clamped"
        );
        assert_eq!(meter.gauge_value("media_bitrate_kbps", &tags), 900.0);
        assert_eq!(meter.counter_value("media_frames_dropped_total", &tags), 3);

        // Leave is idempotent; close_room removes every session.
        adapter.leave(&first.session_id).await.expect("leave");
        adapter.leave(&first.session_id).await.expect("leave twice");
        assert!(matches!(
            adapter.stats(&first.session_id).await,
            Err(error) if error.code() == "not_found"
        ));
        adapter.close_room("room-1").await.expect("close room");
        assert!(matches!(
            adapter.stats(&second.session_id).await,
            Err(error) if error.code() == "not_found"
        ));
    }

    #[tokio::test]
    async fn in_memory_adapter_satisfies_the_media_contract() {
        let adapter = InMemoryMediaPlaneAdapter::new();
        contract_suite(&adapter).await;
        assert_eq!(adapter.open_rooms(), 0);
    }

    #[tokio::test]
    async fn stats_override_and_room_index_are_observable() {
        let adapter = InMemoryMediaPlaneAdapter::new();
        let session = adapter.join("room-9", "carol").await.expect("join");
        assert_eq!(adapter.open_rooms(), 1);
        adapter.set_stats(
            &session.session_id,
            MediaStats {
                rtt_ms: 120.0,
                jitter_ms: 8.0,
                packet_loss_ratio: 0.02,
                bitrate_kbps: 2_000.0,
                frames_dropped: 0,
                resolution: Some((1920, 1080)),
            },
        );
        let stats = adapter.stats(&session.session_id).await.expect("stats");
        assert_eq!(stats.rtt_ms, 120.0);
        assert_eq!(stats.resolution, Some((1920, 1080)));
        adapter.close_room("room-9").await.expect("close");
        assert_eq!(adapter.open_rooms(), 0);
    }
}
