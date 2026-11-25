//! `PUT /api/resources/{id}` — update a resource (admin).

use serde::Deserialize;
use webx::*;

use super::models::ResourceModel;

#[derive(Default, Deserialize, WebxRequestMeta)]
pub struct UpdateResourceRequest {
    #[frompath]
    #[serde(default)]
    pub id: String,
    pub name: Option<String>,
    pub description: Option<String>,
    pub r#type: Option<String>,
    pub value: Option<String>,
    pub properties: Option<String>,
}

#[put("/api/resources/{id}")]
#[authorize(role = "admin")]
impl IRequest<ResourceModel> for UpdateResourceRequest {}
