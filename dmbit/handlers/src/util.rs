//! Shared utilities.

use std::time::{SystemTime, UNIX_EPOCH};

use rust_webx::{Error, RequestContext, Result};

pub fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

pub fn parse_id(s: &str) -> Result<String> {
    if s.is_empty() {
        return Err(Error::Http(format!("Invalid id: {}", s)));
    }
    Ok(s.to_string())
}

pub fn operator_id() -> Option<String> {
    RequestContext::operator_id()
}
