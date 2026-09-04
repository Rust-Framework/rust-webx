//! 共享工具：时间戳生成、ID 解析、操作人 ID（来自 RequestContext）。

use std::time::{SystemTime, UNIX_EPOCH};

use rust_webx::{Error, RequestContext, Result};

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

/// 当前请求操作人 ID（JWT `sub`），由 HTTP dispatch 写入 RequestContext。
pub fn operator_id() -> Option<String> {
    RequestContext::operator_id()
}
