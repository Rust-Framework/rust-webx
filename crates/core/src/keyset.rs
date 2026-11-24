//! Keyset (cursor) pagination contract types.
//!
//! Since 3.0 these are the shared `rust-core` contracts; this module only
//! re-exports them so the historical `webx_core::keyset::*` paths keep working.
//! Do not define a second copy here.
//!
//! The cursor is opaque and client-supplied: `Cursor::decode` only proves the
//! encoding is well-formed, not that the values are allowed. Validate every
//! component (parse timestamps/ids, reject unknown sort directions, scope the
//! key to the caller's tenant) before using it in a query.

pub use rust_core::keyset::{
    Cursor, CursorError, CursorPage, KeysetRequest, MAX_CURSOR_BYTES,
};

/// A malformed client cursor is a request error (`400`), not a server fault.
impl From<CursorError> for crate::error::Error {
    fn from(error: CursorError) -> Self {
        Self::Validation(error.to_string())
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    use data_encoding::BASE64URL_NOPAD;
    use serde::Serialize;
    #[test]
    fn cursor_errors_convert_to_validation_errors() {
        let error: crate::error::Error = CursorError::Format.into();
        assert_eq!(error.status_code(), 400);
    }

    #[test]
    fn from_parts_defaults_and_clamps() {
        let defaults = KeysetRequest::from_parts(None, None);
        assert_eq!(defaults.cursor, None);
        assert_eq!(defaults.page_size, 20);

        let zero = KeysetRequest::from_parts(Some("c".into()), Some(0));
        assert_eq!(zero.page_size, 1);
        assert_eq!(zero.cursor.as_deref(), Some("c"));

        let wide = KeysetRequest::from_parts(None, Some(1_000));
        assert_eq!(wide.page_size, 100);
    }

    #[test]
    fn cursor_round_trips_components() {
        let components = ["2026-11-22T12:00:00Z", "璁㈠崟-42", "x"];
        let cursor = Cursor::encode(&components);
        assert_eq!(
            Cursor::decode(&cursor).expect("decode"),
            components.map(str::to_string)
        );

        // Empty component list is a valid cursor (first page marker).
        let empty = Cursor::encode(&[]);
        assert!(Cursor::decode(&empty).expect("decode empty").is_empty());
    }

    #[test]
    fn malformed_cursors_fail_with_typed_errors() {
        assert_eq!(Cursor::decode("not base64!!"), Err(CursorError::Encoding));
        let object = BASE64URL_NOPAD.encode(br#"{"a":1}"#);
        assert_eq!(Cursor::decode(&object), Err(CursorError::Format));
        let number = BASE64URL_NOPAD.encode(b"[1,2]");
        assert_eq!(Cursor::decode(&number), Err(CursorError::Format));

        let long = "a".repeat(MAX_CURSOR_BYTES + 1);
        assert_eq!(Cursor::decode(&long), Err(CursorError::TooLong));
    }

    #[test]
    fn request_decoded_treats_absent_and_empty_as_first_page() {
        assert!(KeysetRequest::from_parts(None, None)
            .decoded()
            .expect("none")
            .is_empty());
        assert!(KeysetRequest::from_parts(Some(String::new()), None)
            .decoded()
            .expect("empty")
            .is_empty());
        assert!(KeysetRequest::from_parts(Some("bad!".into()), None)
            .decoded()
            .is_err());
    }

    #[test]
    fn pages_expose_an_exact_has_more_from_one_extra_row() {
        #[derive(Serialize)]
        struct Row {
            id: &'static str,
        }

        let rows: Vec<Row> = (0..3)
            .map(|index| Row {
                id: ["a", "b", "c"][index],
            })
            .collect();

        // page_size 2 with 3 fetched rows: two items, a cursor, has_more.
        let page = CursorPage::from_rows(rows, 2, |row| vec![row.id.to_string()]);
        assert_eq!(page.items.len(), 2);
        assert!(page.has_more);
        let cursor = page.cursor.clone().expect("cursor");
        assert_eq!(
            Cursor::decode(&cursor).expect("decode"),
            vec!["b".to_string()]
        );

        // Exactly page_size rows: no cursor, no more pages.
        let rows: Vec<Row> = vec![Row { id: "a" }, Row { id: "b" }];
        let page = CursorPage::from_rows(rows, 2, |row| vec![row.id.to_string()]);
        assert_eq!(page.items.len(), 2);
        assert!(!page.has_more);
        assert!(page.cursor.is_none());
    }

    #[test]
    fn from_entities_maps_only_kept_rows_and_never_needs_entity_serialize() {
        // A domain entity: deliberately not `Serialize`.
        struct Message {
            created_at: i64,
            id: &'static str,
        }

        #[derive(Serialize, PartialEq, Debug)]
        struct MessageModel {
            id: &'static str,
        }

        let rows = vec![
            Message {
                created_at: 0,
                id: "a",
            },
            Message {
                created_at: 1,
                id: "b",
            },
            Message {
                created_at: 2,
                id: "c",
            },
        ];
        let mut mapped = 0usize;

        let page: CursorPage<MessageModel> = CursorPage::from_entities(
            rows,
            2,
            |row| vec![row.created_at.to_string(), row.id.to_string()],
            |row| {
                mapped += 1;
                MessageModel { id: row.id }
            },
        );

        assert_eq!(
            page.items,
            vec![MessageModel { id: "a" }, MessageModel { id: "b" }]
        );
        // The probe row is dropped before mapping.
        assert_eq!(mapped, 2);
        assert!(page.has_more);
        assert_eq!(
            Cursor::decode(page.cursor.as_deref().expect("cursor")).expect("decode"),
            vec!["1".to_string(), "b".to_string()]
        );
    }

    #[test]
    fn serialized_page_omits_an_absent_cursor() {
        let page: CursorPage<u32> = CursorPage::new(vec![1, 2], None);
        let json = serde_json::to_value(&page).expect("serialize");
        assert_eq!(json["has_more"], false);
        assert!(json.get("cursor").is_none());
    }

    #[test]
    fn map_items_keeps_the_page_envelope() {
        #[derive(Serialize, PartialEq, Debug)]
        struct Model {
            id: u32,
        }

        let page: CursorPage<u32> = CursorPage::new(vec![1, 2], Some("next".to_string()));
        let mapped = page.map_items(|id| Model { id });
        assert_eq!(
            mapped.items,
            vec![Model { id: 1 }, Model { id: 2 }],
            "items are transformed in order"
        );
        assert_eq!(mapped.cursor.as_deref(), Some("next"));
        assert!(mapped.has_more, "has_more must not be recomputed/lost");

        let last: CursorPage<u32> = CursorPage::new(vec![3], None);
        let mapped = last.map_items(|id| Model { id });
        assert!(!mapped.has_more);
        assert!(mapped.cursor.is_none());
    }
}
