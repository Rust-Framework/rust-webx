//! User mappers — `User` ↔ `UserModel`.

use docbit_contracts::user::{CreateUserRequest, UpdateUserRequest, UserModel};
use rust_ef::prelude::*;
use webx_core::{Error, IPatchable};

use crate::audit::operator_id;
use crate::entities::User;
use crate::ids::new_id;
use crate::mappers::now_secs;

impl TryFrom<CreateUserRequest> for User {
    type Error = Error;

    fn try_from(req: CreateUserRequest) -> Result<Self, Self::Error> {
        let name = req.name.trim();
        if name.is_empty() {
            return Err(Error::validation("User name cannot be empty"));
        }
        let email = req.email.trim();
        if email.is_empty() {
            return Err(Error::validation("User email cannot be empty"));
        }

        let now = now_secs();
        let op = operator_id();
        Ok(User {
            id: new_id(),
            name: name.to_string(),
            email: email.to_string(),
            password_hash: String::new(),
            created_id: op.clone(),
            created_at: now,
            updated_id: op,
            updated_at: now,
            is_deleted: false,
            roles: HasMany::new(),
        })
    }
}

impl IPatchable<UpdateUserRequest> for User {
    fn patch(&mut self, patch: UpdateUserRequest) {
        if let Some(v) = patch.name {
            self.name = v;
        }
        if let Some(v) = patch.email {
            self.email = v;
        }
        self.updated_id = operator_id();
        self.updated_at = now_secs();
    }
}

impl From<&User> for UserModel {
    fn from(e: &User) -> Self {
        Self {
            id: e.id.clone(),
            name: e.name.clone(),
            email: e.email.clone(),
            roles: e.roles.items().iter().map(|r| r.name.clone()).collect(),
            created_at: e.created_at,
        }
    }
}
