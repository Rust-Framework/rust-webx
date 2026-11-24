//! IM handlers: HTTP login/rooms/history plus the WebSocket message handler.
//!
//! All crypto goes through the framework ports (`ITokenIssuer`,
//! `IPasswordHasher`); all identity comes from the authenticated request
//! (`#[fromclaim]` on HTTP, connection identity on WebSocket).

use std::sync::Arc;

use webx::*;

use crate::contracts::*;
use crate::store::{ImStore, StoredMessage};

/// Map domain-string errors to HTTP-shaped framework errors.
fn store_error(message: String) -> Error {
    if message.contains("not found") {
        Error::not_found(message)
    } else if message.contains("not a member") {
        Error::forbidden(message)
    } else {
        Error::validation(message)
    }
}

fn room_view(store: &ImStore, room: crate::store::RoomRecord) -> RoomView {
    RoomView {
        id: room.id.clone(),
        name: room.name,
        owner_id: room.owner_id,
        members: store.member_names(&room.id),
    }
}

// ---------------------------------------------------------------------------
// HTTP: login
// ---------------------------------------------------------------------------

#[derive(Inject)]
pub struct LoginHandler {
    #[inject]
    tokens: Arc<dyn ITokenIssuer>,
    #[inject]
    hasher: Arc<dyn IPasswordHasher>,
    #[inject]
    store: Arc<ImStore>,
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<LoginRequest, LoginResponse> for LoginHandler {
    async fn handle(&mut self, req: LoginRequest) -> ActionResult<LoginResponse> {
        let username = req.username.trim().to_string();
        if username.is_empty() {
            return Err(Error::validation("username is required"));
        }
        if username.len() > 64 {
            return Err(Error::validation("username is too long"));
        }
        if req.password.len() < 4 {
            return Err(Error::validation("password must be at least 4 characters"));
        }
        let (user_id, known) = match self.store.credentials(&username) {
            Some((user_id, hash)) => {
                if !self.hasher.verify(&req.password, &hash).await? {
                    return Err(Error::unauthorized("invalid credentials"));
                }
                (user_id, true)
            }
            None => {
                let hash = self.hasher.hash(&req.password).await?;
                (self.store.create_user(&username, hash), false)
            }
        };
        let _ = known;
        let issued = self
            .tokens
            .issue(TokenClaims::new(&user_id).with_claim("name", username.clone()))
            .await?;
        Ok(LoginResponse {
            token: issued.token,
            user_id,
            username,
        })
    }
}

// ---------------------------------------------------------------------------
// HTTP: rooms
// ---------------------------------------------------------------------------

#[derive(Inject)]
pub struct CreateRoomHandler {
    #[inject]
    store: Arc<ImStore>,
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<CreateRoomRequest, RoomView> for CreateRoomHandler {
    async fn handle(&mut self, req: CreateRoomRequest) -> ActionResult<RoomView> {
        let owner_name = self
            .store
            .username_of(&req.user.id)
            .unwrap_or_else(|| req.user.name.clone());
        let room = self.store.create_room(&req.user.id, &owner_name, &req.name);
        Ok(room_view(&self.store, room))
    }
}

#[derive(Inject)]
pub struct JoinRoomHandler {
    #[inject]
    store: Arc<ImStore>,
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<JoinRoomRequest, RoomView> for JoinRoomHandler {
    async fn handle(&mut self, req: JoinRoomRequest) -> ActionResult<RoomView> {
        self.store
            .join_room(&req.room_id, &req.user.id)
            .map_err(store_error)?;
        let room = self
            .store
            .room(&req.room_id)
            .ok_or_else(|| Error::not_found(format!("room '{}' not found", req.room_id)))?;
        Ok(room_view(&self.store, room))
    }
}

#[derive(Inject)]
pub struct ListRoomsHandler {
    #[inject]
    store: Arc<ImStore>,
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<ListRoomsRequest, Vec<RoomView>> for ListRoomsHandler {
    async fn handle(&mut self, req: ListRoomsRequest) -> ActionResult<Vec<RoomView>> {
        Ok(self
            .store
            .rooms_of(&req.user.id)
            .into_iter()
            .map(|room| room_view(&self.store, room))
            .collect())
    }
}

// ---------------------------------------------------------------------------
// HTTP: offline catch-up
// ---------------------------------------------------------------------------

#[derive(Inject)]
pub struct HistoryHandler {
    #[inject]
    store: Arc<ImStore>,
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<HistoryRequest, HistoryResponse> for HistoryHandler {
    async fn handle(&mut self, req: HistoryRequest) -> ActionResult<HistoryResponse> {
        let messages: Vec<MessageView> = self
            .store
            .history(&req.room_id, &req.user.id, req.after_seq.unwrap_or(0), 200)
            .map_err(store_error)?
            .into_iter()
            .map(MessageView::from)
            .collect();
        Ok(HistoryResponse {
            room_id: req.room_id,
            messages,
        })
    }
}

// ---------------------------------------------------------------------------
// HTTP: TURN credentials (2.4-1)
// ---------------------------------------------------------------------------

#[derive(Inject)]
pub struct TurnCredentialsHandler {
    #[inject]
    config: Arc<TurnRestConfig>,
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<TurnRequest, TurnCredentials> for TurnCredentialsHandler {
    async fn handle(&mut self, req: TurnRequest) -> ActionResult<TurnCredentials> {
        let mut config = (*self.config).clone();
        if let Some(ttl) = req.ttl_seconds {
            config.ttl_seconds = ttl.clamp(1, MAX_TURN_TTL_SECONDS);
        }
        Ok(config.credentials_for(&req.user.id)?)
    }
}

// ---------------------------------------------------------------------------
// WebSocket: message-as-request
// ---------------------------------------------------------------------------

#[derive(Inject)]
pub struct ImMessageHandler {
    #[inject]
    store: Arc<ImStore>,
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<ImRequest, ImReply> for ImMessageHandler {
    async fn handle(&mut self, req: ImRequest) -> ActionResult<ImReply> {
        let user = RequestContext::identity()
            .ok_or_else(|| Error::unauthorized("websocket login required"))?;
        match req {
            ImRequest::Join { room_id } => {
                let members = self
                    .store
                    .join_room(&room_id, &user.id)
                    .map_err(store_error)?;
                // Register this connection with the in-process registry so room
                // fan-out can reach it.
                if let Some(context) = RequestContext::connection() {
                    if let Some(registry) = context.resolve::<dyn IConnectionRegistry>() {
                        registry.join(context.meta().id, &room_id);
                    }
                }
                Ok(ImReply::Joined { room_id, members })
            }
            ImRequest::Send {
                room_id,
                client_msg_id,
                text,
            } => {
                let from = self
                    .store
                    .username_of(&user.id)
                    .unwrap_or_else(|| user.id.clone());
                let posted = self
                    .store
                    .post_message(&room_id, &user.id, &from, &client_msg_id, &text)
                    .map_err(store_error)?;
                if !posted.duplicate {
                    self.broadcast(&room_id, &posted.message).await?;
                }
                Ok(ImReply::Ack {
                    room_id,
                    seq: posted.message.seq,
                    duplicate: posted.duplicate,
                })
            }
            ImRequest::History { room_id, after_seq } => {
                let messages: Vec<MessageView> = self
                    .store
                    .history(&room_id, &user.id, after_seq, 200)
                    .map_err(store_error)?
                    .into_iter()
                    .map(MessageView::from)
                    .collect();
                Ok(ImReply::History { room_id, messages })
            }
            ImRequest::Presence { room_id } => {
                let name = self
                    .store
                    .username_of(&user.id)
                    .unwrap_or_else(|| user.id.clone());
                let mut online = Vec::new();
                if let Some(context) = RequestContext::connection() {
                    if let Some(registry) = context.resolve::<dyn IConnectionRegistry>() {
                        for peer in registry.room_connections(&room_id) {
                            if let Some(identity) = peer.context().identity() {
                                if let Some(peer_name) = self.store.username_of(&identity.id) {
                                    online.push(peer_name);
                                }
                            }
                        }
                    }
                }
                if !online.iter().any(|peer| peer == &name) {
                    online.push(name);
                }
                online.sort();
                online.dedup();
                Ok(ImReply::Presence { room_id, online })
            }
            ImRequest::Signal {
                room_id,
                to,
                kind,
                payload,
            } => {
                // Only room members may negotiate media for that room.
                let room = self
                    .store
                    .room(&room_id)
                    .ok_or_else(|| Error::not_found(format!("room '{room_id}' not found")))?;
                if !room.members.contains(&user.id) {
                    return Err(Error::forbidden(format!(
                        "user is not a member of room '{room_id}'"
                    )));
                }
                let from = self
                    .store
                    .username_of(&user.id)
                    .unwrap_or_else(|| user.id.clone());
                let Some(context) = RequestContext::connection() else {
                    return Ok(ImReply::Signaled {
                        room_id,
                        delivered: 0,
                    });
                };
                let Some(registry) = context.resolve::<dyn IConnectionRegistry>() else {
                    return Ok(ImReply::Signaled {
                        room_id,
                        delivered: 0,
                    });
                };

                let (target_id, event_to) = match to {
                    Some(target_username) => {
                        let target_id =
                            self.store.user_id_of(&target_username).ok_or_else(|| {
                                Error::not_found(format!("user '{target_username}' not found"))
                            })?;
                        if !room.members.contains(&target_id) {
                            return Err(Error::forbidden(format!(
                                "user '{target_username}' is not a member of room '{room_id}'"
                            )));
                        }
                        (Some(target_id), Some(target_username))
                    }
                    None => (None, None),
                };
                let message =
                    SignalingMessage::new(room_id.clone(), from, target_id.clone(), kind, payload);
                let event = ImEvent::Signal {
                    room_id: room_id.clone(),
                    message,
                };
                let payload_json = serde_json::to_string(&event)
                    .map_err(|error| Error::internal(format!("signal encode failed: {error}")))?;
                let mut delivered = 0usize;
                match target_id {
                    Some(target_id) => {
                        for peer in registry.user_connections(&target_id) {
                            if peer.send_text(payload_json.clone()).await.is_ok() {
                                delivered += 1;
                            }
                        }
                    }
                    None => {
                        for peer in registry.room_connections(&room_id) {
                            if peer.id() == context.meta().id {
                                continue;
                            }
                            if peer.send_text(payload_json.clone()).await.is_ok() {
                                delivered += 1;
                            }
                        }
                    }
                }
                let _ = event_to;
                Ok(ImReply::Signaled { room_id, delivered })
            }
        }
    }
}

impl ImMessageHandler {
    /// Fan the stored message out to every connection currently in the room.
    async fn broadcast(&self, room_id: &str, message: &StoredMessage) -> Result<()> {
        let Some(context) = RequestContext::connection() else {
            return Ok(());
        };
        let Some(registry) = context.resolve::<dyn IConnectionRegistry>() else {
            return Ok(());
        };
        let event = ImEvent::Message {
            room_id: message.room_id.clone(),
            seq: message.seq,
            from: message.from_name.clone(),
            text: message.text.clone(),
            client_msg_id: message.client_msg_id.clone(),
            at_unix: message.at_unix,
        };
        let payload = serde_json::to_string(&event)
            .map_err(|error| Error::internal(format!("event encode failed: {error}")))?;
        for peer in registry.room_connections(room_id) {
            // Backpressure drops are counted by the connection; fan-out must not
            // fail the sender because one receiver is slow.
            let _ = peer.send_text(payload.clone()).await;
        }
        Ok(())
    }
}
