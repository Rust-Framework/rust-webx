//! `POST /api/resources` — create a resource (admin).

use serde::Deserialize;
use webx::*;

use super::models::ResourceModel;

#[derive(Default, Deserialize, WebxRequestMeta)]
pub struct CreateResourceRequest {
    pub name: String,
    pub description: String,
    pub r#type: String,
    pub value: String,
    pub properties: String,
}

#[post("/api/resources")]
#[authorize(role = "admin")]
impl IRequest<ResourceModel> for CreateResourceRequest {}
