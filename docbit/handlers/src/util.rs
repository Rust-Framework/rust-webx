//! 共享工具：时间戳生成、ID 解析、审计字段填充辅助。

use std::time::{SystemTime, UNIX_EPOCH};

use rust_webx::{Error, IClaims, Result};

/// 当前 Unix 秒（i64）。
pub fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// 将路径参数解析为 UUID 字符串主键。
pub fn parse_id(s: &str) -> Result<String> {
    if s.is_empty() {
        return Err(Error::Http(format!("Invalid id: {}", s)));
    }
    Ok(s.to_string())
}

/// 从 claims 提取操作人 ID（UUID 字符串），用于审计字段。
pub fn operator_id(claims: Option<&dyn IClaims>) -> Option<String> {
    claims.map(|c| c.subject().to_string())
}
