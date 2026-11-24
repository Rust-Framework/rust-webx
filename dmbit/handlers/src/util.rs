//! Shared utilities.

use std::time::{SystemTime, UNIX_EPOCH};

use webx::{Error, RequestContext, Result};

pub fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

pub fn parse_id(s: &str) -> Result<String> {
    let t = s.trim();
    if t.is_empty() {
        return Err(Error::validation("标识不能为空"));
    }
    if t.chars().count() > 36 {
        return Err(Error::validation("标识格式无效"));
    }
    Ok(t.to_string())
}

pub fn operator_id() -> Option<String> {
    RequestContext::operator_id()
}
