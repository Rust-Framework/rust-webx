//! `GET /api/users` — every registered user (admin).

use serde::Deserialize;
use webx::*;

use super::models::UserModel;

#[derive(Default, Deserialize, WebxRequestMeta)]
pub struct ListUsersRequest;

/// Returns a list of all registered users.
#[get("/api/users")]
#[authorize(role = "admin")]
impl IRequest<Vec<UserModel>> for ListUsersRequest {}
