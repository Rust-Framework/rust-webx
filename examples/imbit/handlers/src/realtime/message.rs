//! `/ws/im` — message-as-request handler.
//!
//! One client frame is one request: the framework decoded it into
//! [`ImRequest`], opened a per-message DI scope and dispatched it here through
//! the same `IRequestHandler` pipeline HTTP uses. The identity is the one
//! captured by the **authenticated upgrade** (`RequestContext::identity()`) — a
//! cookie session and a bearer token look identical from here. A frame never
//! carries its own identity, roles or client address.
//!
//! **Fan-out goes through the event transport**, never by walking local
//! connections in the handler: a stored room message publishes
//! [`RealtimeFanoutEvent`] and 1:1 signaling publishes
//! [`RealtimeUserFanoutEvent`]. Both are broadcast-class framework events; the
//! host's event consumer routes each one to the framework handler that writes
//! this node's `room_connections` / `user_connections`. The default in-memory
//! queue makes that a same-process hop; a broker makes the same code
//! multi-instance without touching this file.
//!
//! Presence combines the room's membership (the domain store) with the shared
//! online-presence store the host populates from connection first/last
//! transitions: a room member is online when the shared `user:<id>` scope has a
//! live member.

use std::sync::Arc;

use webx::*;

use imbit_contracts::message::MessageModel;
use imbit_contracts::realtime::{ImEvent, ImReply, ImRequest};
use imbit_domain::mappers::ToModel;
use imbit_domain::model::Message;
use imbit_domain::{ImStore, StoreError};

use crate::common::{connection_registry, encode_event, CATCH_UP_LIMIT};

#[derive(Inject)]
pub struct ImMessageHandler {
    #[inject]
    store: Arc<ImStore>,
    #[inject]
    clock: Arc<dyn IClock>,
    /// Publishes framework broadcast events; the transport (in-memory queue by
    /// default) hands them to this node's fan-out handlers.
    #[inject]
    mediator: Arc<Mediator>,
    /// Shared online-presence authority the host bridges first/last connection
    /// transitions into (`user:<id>` scopes).
    #[inject]
    online_presence: Arc<dyn IOnlinePresence>,
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<ImRequest, ImReply> for ImMessageHandler {
    async fn handle(&mut self, request: ImRequest) -> ActionResult<ImReply> {
        let user = RequestContext::identity()
            .ok_or_else(|| Error::unauthorized("websocket login required"))?;

        match request {
            ImRequest::Join { room_id } => self.join(room_id, &user),
            ImRequest::Send {
                room_id,
                client_msg_id,
                text,
            } => self.send(room_id, client_msg_id, text, &user).await,
            ImRequest::History { room_id, after_seq } => self.history(room_id, after_seq, &user),
            ImRequest::Presence { room_id } => self.presence(room_id, &user).await,
            ImRequest::Signal {
                room_id,
                to,
                kind,
                payload,
            } => self.signal(room_id, to, kind, payload, &user).await,
        }
    }
}

impl ImMessageHandler {
    /// Join a room and register this connection for its fan-out.
    fn join(&self, room_id: String, user: &IdentityUser) -> ActionResult<ImReply> {
        let room = self
            .store
            .join_room(&room_id, &user.id, user.display_name())?;
        if let (Some(context), Some(registry)) =
            (RequestContext::connection(), connection_registry())
        {
            registry.join(context.meta().id, &room_id);
        }

        Ok(ImReply::Joined {
            room_id,
            members: room.member_count(),
        })
    }

    /// Store a message and publish its broadcast event.
    ///
    /// A retry of a known `client_msg_id` is acknowledged with the original
    /// `seq` and is **not** broadcast again. The event transport is
    /// at-most-once: if it rejects the event the request fails, and a client
    /// that reconnects later catches up through `history` (the write itself is
    /// already durable in the reference store).
    async fn send(
        &self,
        room_id: String,
        client_msg_id: String,
        text: String,
        user: &IdentityUser,
    ) -> ActionResult<ImReply> {
        let posted = self.store.post_message(
            &room_id,
            &user.id,
            user.display_name(),
            &client_msg_id,
            &text,
            self.clock.now_unix(),
        )?;
        if !posted.duplicate {
            self.fan_out(&room_id, &posted.message).await?;
        }

        Ok(ImReply::Ack {
            room_id,
            seq: posted.message.seq,
            duplicate: posted.duplicate,
        })
    }

    /// Offline catch-up on the live channel (same log as the HTTP endpoint).
    fn history(
        &self,
        room_id: String,
        after_seq: u64,
        user: &IdentityUser,
    ) -> ActionResult<ImReply> {
        let messages: Vec<MessageModel> = self
            .store
            .history(&room_id, &user.id, after_seq, CATCH_UP_LIMIT)?
            .iter()
            .map(ToModel::to_model)
            .collect();

        Ok(ImReply::History { room_id, messages })
    }

    /// Online members of a room: room membership (domain) × shared presence
    /// (host-maintained `user:<id>` scopes).
    ///
    /// The caller is connected by definition (its frame is being handled), so it
    /// is added even if the presence bridge has not observed it yet — the same
    /// tolerance the previous connection-registry implementation had.
    async fn presence(&self, room_id: String, user: &IdentityUser) -> ActionResult<ImReply> {
        // Non-members must not learn the room's presence: membership is part of
        // the room rule, not of the payload.
        let room = self.store.require_member(&room_id, &user.id)?;

        let mut online: Vec<String> = Vec::new();
        for member_id in room.member_ids() {
            let scope = PresenceScope::user(&member_id);
            if !self.online_presence.members(&scope).await?.is_empty() {
                online.push(self.display_name_of(&member_id));
            }
        }

        if !online.iter().any(|peer| peer == user.display_name()) {
            online.push(user.display_name().to_string());
        }
        online.sort();
        online.dedup();

        Ok(ImReply::Presence { room_id, online })
    }

    /// WebRTC signaling: 1:1 negotiation (`to`) or a room broadcast.
    ///
    /// Only members of the room may negotiate media for it, and the target must
    /// be a member too; `from` is stamped from the connection identity. The
    /// encoded frame is published on the event transport: a targeted signal
    /// addresses `user_connections` (all of the user's devices), a room signal
    /// addresses the room's connections.
    ///
    /// `delivered` counts the **local** connections the event targets: the
    /// transport is asynchronous, so cross-node delivery cannot be counted here
    /// (at-most-once, clients renegotiate on reconnect). A room broadcast is
    /// written to every local room connection, including the sender's; clients
    /// filter their own frames by the `from` field.
    async fn signal(
        &self,
        room_id: String,
        to: Option<String>,
        kind: SignalingKind,
        payload: serde_json::Value,
        user: &IdentityUser,
    ) -> ActionResult<ImReply> {
        let room = self.store.require_member(&room_id, &user.id)?;

        let target_id = match to.as_deref() {
            Some(target_username) => {
                let target_id = self.store.user_id_of(target_username).ok_or_else(|| {
                    StoreError::not_found(format!("user '{target_username}' not found"))
                })?;
                if !room.is_member(&target_id) {
                    return Err(StoreError::forbidden(format!(
                        "user '{target_username}' is not a member of room '{room_id}'"
                    ))
                    .into());
                }
                Some(target_id)
            }
            None => None,
        };

        let message = SignalingMessage::new(
            room_id.clone(),
            user.display_name().to_string(),
            target_id.clone(),
            kind,
            payload,
        );
        let event = ImEvent::Signal {
            room_id: room_id.clone(),
            message,
        };
        let frame = encode_event(&event)?;

        let delivered = match &target_id {
            Some(target_id) => {
                let delivered = local_user_connection_count(target_id);
                self.mediator
                    .publish(RealtimeUserFanoutEvent::to_user(target_id.clone(), frame))
                    .await?;
                delivered
            }
            None => {
                let sender = RequestContext::connection().map(|context| context.meta().id);
                let delivered = local_room_peer_count(&room_id, sender);
                self.mediator
                    .publish(RealtimeFanoutEvent::to_room(room_id.clone(), frame))
                    .await?;
                delivered
            }
        };

        Ok(ImReply::Signaled { room_id, delivered })
    }

    /// Publish a stored message as a member of every room member's nodes.
    async fn fan_out(&self, room_id: &str, message: &Message) -> Result<()> {
        let frame = encode_event(&ImEvent::message(&message.to_model()))?;
        self.mediator
            .publish(RealtimeFanoutEvent::to_room(room_id, frame))
            .await
    }

    /// Display name of a user id, falling back to the id itself.
    fn display_name_of(&self, user_id: &str) -> String {
        self.store
            .username_of(user_id)
            .unwrap_or_else(|| user_id.to_string())
    }
}

/// Local connections of `user_id` the published event will write to.
fn local_user_connection_count(user_id: &str) -> usize {
    connection_registry().map_or(0, |registry| registry.user_connections(user_id).len())
}

/// Local room connections other than the sender (the reply's peer count).
fn local_room_peer_count(room_id: &str, sender: Option<ConnectionId>) -> usize {
    connection_registry().map_or(0, |registry| {
        registry
            .room_connections(room_id)
            .iter()
            .filter(|peer| Some(peer.id()) != sender)
            .count()
    })
}
