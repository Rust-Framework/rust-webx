//! RBAC read models — roles, resources (通用资源模型), and authorizations.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoleModel {
    pub id: String,
    pub name: String,
    pub description: String,
    pub created_at: i64,
    pub updated_at: i64,
}

/// 资源模型：通用 `type + value + properties` 三段式。
/// - `type`：应用/模块/页面/操作/数据/其他
/// - `value`：页面/操作类型时为路由
/// - `properties`：配置属性 JSON，如 `{"method":"GET"}`
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceModel {
    pub id: String,
    pub name: String,
    pub description: String,
    pub r#type: String,
    pub value: String,
    pub properties: String,
    pub created_at: i64,
    pub updated_at: i64,
}

/// One role ↔ resource link.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthorizeModel {
    pub id: String,
    pub role_id: String,
    pub resource_id: String,
}
