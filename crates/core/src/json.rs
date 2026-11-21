//! JSON response encoding helpers.
//!
//! `serde_json::to_vec` starts from an empty `Vec` and grows it while
//! serializing, so a typical DTO (well under 256 bytes) pays one or more
//! reallocations. The response hot path preallocates a small buffer instead:
//! one allocation for the common case, a normal grow for large payloads.

/// Serialize `value` into a JSON byte buffer sized for a typical DTO.
pub fn encode_json<T>(value: &T) -> Result<Vec<u8>, serde_json::Error>
where
    T: serde::Serialize + ?Sized,
{
    let mut buffer = Vec::with_capacity(256);
    serde_json::to_writer(&mut buffer, value)?;
    Ok(buffer)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_json_matches_to_vec() {
        #[derive(serde::Serialize)]
        struct View {
            message: &'static str,
            count: u32,
        }
        let view = View {
            message: "Hello, World!",
            count: 7,
        };
        let expected = serde_json::to_vec(&view).expect("to_vec");
        assert_eq!(encode_json(&view).expect("encode_json"), expected);
    }
}
