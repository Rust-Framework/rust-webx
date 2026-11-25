//! Business result envelope for command-style endpoints.
//!
//! Since 2.9.1 this is the shared `rust-core` contract; this module only
//! re-exports it so the historical `webx_core::operation::*` path keeps working.
//! Do not define a second copy here.
//!
//! `OperationResult<T>` answers "the operation ran — did it succeed, what does
//! the business call this outcome, and what should we tell the caller". A list
//! endpoint uses the flat `PagedResult` instead (it inlines the same outcome
//! fields); neither is a transport status.

pub use rust_core::operation::OperationResult;
#[cfg(test)]
mod tests {
    use super::*;

    use serde::Serialize;
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
        let value = serde_json::to_value(
            OperationResult::success("created")
                .with_status(7)
                .with_result(UserDto { id: 7 }),
        )
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

    #[test]
    fn setters_adjust_an_outcome_decided_while_the_operation_runs() {
        let mut outcome = OperationResult::success("published");
        if true {
            outcome = outcome.with_status(1001).with_message("already published");
        }
        // The fields stay public for in-place adjustment too.
        outcome.status = 1001;

        let value = serde_json::to_value(outcome).unwrap();
        assert_eq!(
            value,
            serde_json::json!({
                "success": true,
                "status": 1001,
                "message": "already published"
            })
        );
    }

    #[test]
    fn with_result_attaches_the_payload_to_a_unit_envelope() {
        let value = serde_json::to_value(
            OperationResult::success("created").with_result(UserDto { id: 9 }),
        )
        .unwrap();
        assert_eq!(
            value,
            serde_json::json!({
                "success": true,
                "status": 0,
                "message": "created",
                "result": { "id": 9 }
            })
        );
    }
}
