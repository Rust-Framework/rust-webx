//! RBAC mappers — `Role` / `Resource` / `Authorize` ↔ their read models.

use docbit_contracts::rbac::{
    AuthorizeModel, CreateAuthorizeRequest, CreateResourceRequest, CreateRoleRequest,
    ResourceModel, RoleModel, UpdateResourceRequest, UpdateRoleRequest,
};
use rust_ef::prelude::*;
use webx_core::{Error, IPatchable};

use crate::audit::operator_id;
use crate::entities::{Authorize, Resource, Role};
use crate::ids::new_id;
use crate::mappers::now_secs;

impl TryFrom<CreateRoleRequest> for Role {
    type Error = Error;

    fn try_from(req: CreateRoleRequest) -> Result<Self, Self::Error> {
        let name = req.name.trim();
        if name.is_empty() {
            return Err(Error::validation("Role name cannot be empty"));
        }

        let now = now_secs();
        let op = operator_id();
        Ok(Role {
            id: new_id(),
            name: name.to_string(),
            description: req.description,
            created_id: op.clone(),
            created_at: now,
            updated_id: op,
            updated_at: now,
            is_deleted: false,
            users: HasMany::new(),
            resources: HasMany::new(),
        })
    }
}

impl IPatchable<UpdateRoleRequest> for Role {
    fn patch(&mut self, patch: UpdateRoleRequest) {
        if let Some(v) = patch.name {
            self.name = v;
        }
        if let Some(v) = patch.description {
            self.description = v;
        }
        self.updated_id = operator_id();
        self.updated_at = now_secs();
    }
}

impl From<&Role> for RoleModel {
    fn from(e: &Role) -> Self {
        Self {
            id: e.id.clone(),
            name: e.name.clone(),
            description: e.description.clone(),
            created_at: e.created_at,
            updated_at: e.updated_at,
        }
    }
}

impl TryFrom<CreateResourceRequest> for Resource {
    type Error = Error;

    fn try_from(req: CreateResourceRequest) -> Result<Self, Self::Error> {
        let name = req.name.trim();
        if name.is_empty() {
            return Err(Error::validation("Resource name cannot be empty"));
        }

        let now = now_secs();
        let op = operator_id();
        Ok(Resource {
            id: new_id(),
            name: name.to_string(),
            description: req.description,
            resource_type: req.r#type,
            value: req.value,
            properties: req.properties,
            created_id: op.clone(),
            created_at: now,
            updated_id: op,
            updated_at: now,
            is_deleted: false,
            roles: HasMany::new(),
        })
    }
}

impl IPatchable<UpdateResourceRequest> for Resource {
    fn patch(&mut self, patch: UpdateResourceRequest) {
        if let Some(v) = patch.name {
            self.name = v;
        }
        if let Some(v) = patch.description {
            self.description = v;
        }
        if let Some(v) = patch.r#type {
            self.resource_type = v;
        }
        if let Some(v) = patch.value {
            self.value = v;
        }
        if let Some(v) = patch.properties {
            self.properties = v;
        }
        self.updated_id = operator_id();
        self.updated_at = now_secs();
    }
}

impl From<&Resource> for ResourceModel {
    fn from(e: &Resource) -> Self {
        Self {
            id: e.id.clone(),
            name: e.name.clone(),
            description: e.description.clone(),
            r#type: e.resource_type.clone(),
            value: e.value.clone(),
            properties: e.properties.clone(),
            created_at: e.created_at,
            updated_at: e.updated_at,
        }
    }
}

impl TryFrom<CreateAuthorizeRequest> for Authorize {
    type Error = Error;

    fn try_from(req: CreateAuthorizeRequest) -> Result<Self, Self::Error> {
        if req.role_id.trim().is_empty() {
            return Err(Error::validation("Authorize role_id cannot be empty"));
        }
        if req.resource_id.trim().is_empty() {
            return Err(Error::validation("Authorize resource_id cannot be empty"));
        }

        Ok(Authorize {
            id: new_id(),
            role_id: req.role_id,
            resource_id: req.resource_id,
            created_at: now_secs(),
        })
    }
}

impl From<&Authorize> for AuthorizeModel {
    fn from(e: &Authorize) -> Self {
        Self {
            id: e.id.clone(),
            role_id: e.role_id.clone(),
            resource_id: e.resource_id.clone(),
        }
    }
}
