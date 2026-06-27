//! 共享工具：时间戳生成、ID 解析、审计字段填充辅助。

use std::time::{SystemTime, UNIX_EPOCH};

use rust_webapp::{Error, IClaims, Result};

/// 当前 Unix 秒（i64）。
pub fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// 将字符串路径参数解析为 i32。
pub fn parse_id(s: &str) -> Result<i32> {
    s.parse::<i32>()
        .map_err(|_| Error::Http(format!("Invalid id: {}", s)))
}

/// 从 claims 提取操作人 ID（i32），用于审计字段。
pub fn operator_id(claims: Option<&dyn IClaims>) -> Option<i32> {
    claims?.subject().parse::<i32>().ok()
}
