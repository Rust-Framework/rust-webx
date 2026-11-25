//! `GET /api/resources` — every RBAC resource (admin).

use serde::Deserialize;
use webx::*;

use super::models::ResourceModel;

#[derive(Default, Deserialize, WebxRequestMeta)]
pub struct ListResourcesRequest;

#[get("/api/resources")]
#[authorize(role = "admin")]
impl IRequest<Vec<ResourceModel>> for ListResourcesRequest {}
