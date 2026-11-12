//! Business result envelope for command-style endpoints.
//!
//! [`PagedResult`](crate::pagination::PagedResult) answers "here is one page of
//! data"; `OperationResult` answers "the operation ran — did it succeed, what
//! does the business call this outcome, and what should we tell the caller".
//! Both are ordinary DTOs the framework ships so that a module does not define
//! its own `{ success, status, message }` shape:
//!
//! ```ignore
//! #[put("/users/{id}")]
//! impl IRequest<OperationResult<UserDto>> for UpdateUserRequest {}
//!
//! async fn handle(&mut self, req: UpdateUserRequest) -> Result<OperationResult<UserDto>> {
//!     let user = self.update(req).await?;
//!     Ok(OperationResult::ok(user, "user updated"))
//! }
//! ```
//!
//! The envelope is business data, not a transport concern: the HTTP status is
//! still the framework's (200 with the serialized envelope, or `problem+json`
//! when the handler returns `Err(webx::Error)`). [`status`](OperationResult::status)
//! is whatever the business assigns — an error code, a workflow state — and the
//! framework never interprets it. `OperationResult` serializes as
//! `{"success":true,"status":0,"message":"ok"}`; `OperationResult<T>` adds the
//! `result` member.

use serde::Serialize;

/// Outcome of an operation: success flag, business status, message, and — for
/// [`OperationResult<T>`](OperationResult) — the business payload.
///
/// `OperationResult` (the default `T = ()`) has no `result` member at all, so
/// update/delete-style endpoints can report the outcome without declaring a
/// one-off type. The fields are public; the constructors keep `success` and
/// `status` agreeing with each other.
#[derive(Debug, Clone, Serialize)]
pub struct OperationResult<T = ()> {
    /// Whether the operation achieved its goal.
    pub success: bool,
    /// Business status code, defined by the caller (the constructors use `0`
    /// when no explicit code is given). It is not an HTTP status: the transport
    /// layer never reads it.
    pub status: i32,
    /// Human-readable outcome for the caller (UI toast, CLI output, logs).
    pub message: String,
    /// The business payload; omitted from JSON when absent (the unit form, or
    /// a [`failed`](Self::failed) result).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<T>,
}

impl<T> OperationResult<T> {
    /// A successful result carrying `result`, with the default business status
    /// `0`.
    pub fn ok(result: T, message: impl Into<String>) -> Self {
        Self::ok_with_status(0, result, message)
    }

    /// A successful result carrying `result` and an explicit business status.
    pub fn ok_with_status(status: i32, result: T, message: impl Into<String>) -> Self {
        Self {
            success: true,
            status,
            message: message.into(),
            result: Some(result),
        }
    }

    /// A failed result with the business status that explains why. No payload is
    /// attached.
    pub fn failed(status: i32, message: impl Into<String>) -> Self {
        Self {
            success: false,
            status,
            message: message.into(),
            result: None,
        }
    }
}

impl OperationResult<()> {
    /// A successful result with no payload, and the default business status `0`.
    ///
    /// The JSON body is `{"success":true,"status":0,"message":"..."}`.
    pub fn success(message: impl Into<String>) -> Self {
        Self::success_with_status(0, message)
    }

    /// A successful result with no payload and an explicit business status.
    pub fn success_with_status(status: i32, message: impl Into<String>) -> Self {
        Self {
            success: true,
            status,
            message: message.into(),
            result: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Serialize)]
    struct UserDto {
        id: u32,
    }

    #[test]
    fn unit_result_omits_the_result_member() {
        let value = serde_json::to_value(OperationResult::success("deleted")).unwrap();
        assert_eq!(
            value,
            serde_json::json!({ "success": true, "status": 0, "message": "deleted" })
        );
    }

    #[test]
    fn generic_result_carries_the_payload_and_its_business_status() {
        let value = serde_json::to_value(OperationResult::ok_with_status(
            7,
            UserDto { id: 7 },
            "created",
        ))
        .unwrap();
        assert_eq!(
            value,
            serde_json::json!({
                "success": true,
                "status": 7,
                "message": "created",
                "result": { "id": 7 }
            })
        );
    }

    #[test]
    fn failed_result_keeps_the_business_status_and_drops_the_payload() {
        let value =
            serde_json::to_value(OperationResult::<UserDto>::failed(1001, "state conflict"))
                .unwrap();
        assert_eq!(
            value,
            serde_json::json!({
                "success": false,
                "status": 1001,
                "message": "state conflict"
            })
        );
    }
}
