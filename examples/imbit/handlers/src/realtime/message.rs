//! `/ws/im` — message-as-request handler.
//!
//! One client frame is one request: the framework decoded it into
//! [`ImRequest`], opened a per-message DI scope and dispatched it here through
//! the same `IRequestHandler` pipeline HTTP uses. The identity is the one
//! captured by the **authenticated upgrade** (`RequestContext::identity()`); a
//! frame never carries its own identity, roles or client address.
//!
//! Room fan-out and signaling go through the framework's `IConnectionRegistry`
//! (bounded outbound queues): one slow receiver cannot fail the sender.

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
            ImRequest::Presence { room_id } => self.presence(room_id, &user),
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

    /// Store a message and fan it out.
    ///
    /// A retry of a known `client_msg_id` is acknowledged with the original
    /// `seq` and is **not** broadcast again.
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

    /// Room presence: the identities of the live connections in this room plus
    /// the caller (who may be connected from another device).
    fn presence(&self, room_id: String, user: &IdentityUser) -> ActionResult<ImReply> {
        let mut online: Vec<String> = Vec::new();
        if let Some(registry) = connection_registry() {
            for peer in registry.room_connections(&room_id) {
                if let Some(identity) = peer.context().identity() {
                    online.push(self.display_name_of(&identity.id));
                }
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
    /// be a member too; `from` is stamped from the connection identity.
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

        let Some(registry) = connection_registry() else {
            return Ok(ImReply::Signaled {
                room_id,
                delivered: 0,
            });
        };
        let delivered = match &target_id {
            Some(target_id) => deliver(registry.user_connections(target_id), &frame).await,
            None => {
                let sender = RequestContext::connection().map(|context| context.meta().id);
                let peers = registry
                    .room_connections(&room_id)
                    .into_iter()
                    .filter(|peer| Some(peer.id()) != sender)
                    .collect();
                deliver(peers, &frame).await
            }
        };

        Ok(ImReply::Signaled { room_id, delivered })
    }

    /// Fan a stored message out to every connection in the room.
    ///
    /// Backpressure drops are counted by the connection itself: one slow
    /// receiver must not turn a successful write into an error for the sender.
    async fn fan_out(&self, room_id: &str, message: &Message) -> Result<()> {
        let frame = encode_event(&ImEvent::message(&message.to_model()))?;
        if let Some(registry) = connection_registry() {
            for peer in registry.room_connections(room_id) {
                let _ = peer.send_text(frame.clone()).await;
            }
        }

        Ok(())
    }

    /// Display name of a user id, falling back to the id itself.
    fn display_name_of(&self, user_id: &str) -> String {
        self.store
            .username_of(user_id)
            .unwrap_or_else(|| user_id.to_string())
    }
}

/// Send one encoded frame to every connection and count the writes that landed.
async fn deliver(peers: Vec<Arc<dyn IConnection>>, frame: &str) -> usize {
    let mut delivered = 0;
    for peer in peers {
        if peer.send_text(frame.to_string()).await.is_ok() {
            delivered += 1;
        }
    }
    delivered
}
