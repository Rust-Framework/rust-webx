//! HTTP parameter merging for endpoint dispatch.

use serde::de::DeserializeOwned;
use std::collections::HashMap;

/// Try to build a request from merged route + query parameters when `T: DeserializeOwned`.
pub fn try_deserialize_from_params<T: DeserializeOwned>(
    route_params: &HashMap<String, String>,
    query_params: &HashMap<String, String>,
) -> Option<T> {
    let mut map = serde_json::Map::new();
    for (k, v) in query_params {
        map.insert(k.clone(), serde_json::Value::String(v.clone()));
    }
    for (k, v) in route_params {
        map.insert(k.clone(), serde_json::Value::String(v.clone()));
    }
    serde_json::from_value(serde_json::Value::Object(map)).ok()
}
