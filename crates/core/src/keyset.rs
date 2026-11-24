//! Keyset (cursor) pagination.
//!
//! Offset pagination ([`PagedRequest`](crate::pagination::PagedRequest)) shifts
//! when rows are inserted/deleted and gets expensive for deep pages. Keyset
//! pagination asks for "the next N rows after this key" instead:
//!
//! ```ignore
//! // GET /api/messages?cursor=<opaque>&page_size=50
//! let request = KeysetRequest::from_parts(cursor, page_size);
//! let sort_key = request.decoded().unwrap_or_default(); // validate + scope it
//! // WHERE (created_at, id) > (cursor_created_at, cursor_id) ORDER BY ... LIMIT page_size + 1
//! let rows = query_after(sort_key, request.page_size() + 1).await?;
//! let page = CursorPage::from_rows(rows, request.page_size(), |row| {
//!     vec![row.created_at.to_string(), row.id.clone()]
//! });
//! ```
//!
//! The cursor is **opaque and client-supplied**: [`Cursor::decode`] only proves
//! the encoding is well-formed, not that the values are allowed. Validate every
//! component (parse timestamps/ids, reject unknown sort directions, scope the
//! key to the caller's tenant) before using it in a query.
//!
//! A cursor only makes sense for a **stable total order** (for example
//! `(created_at DESC, id DESC)`); without a tie-breaker, rows with equal sort
//! values can be skipped or repeated at page boundaries.

use data_encoding::BASE64URL_NOPAD;
use serde::{Deserialize, Serialize};

/// Longest accepted cursor, in bytes (guards against hostile input).
pub const MAX_CURSOR_BYTES: usize = 4_096;

/// Cursor decoding failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum CursorError {
    /// The cursor is longer than [`MAX_CURSOR_BYTES`].
    #[error("cursor is too long")]
    TooLong,
    /// The cursor is not valid base64url.
    #[error("cursor is not valid base64url")]
    Encoding,
    /// The decoded payload is not a JSON array of strings.
    #[error("cursor is not a cursor for this endpoint")]
    Format,
}

/// One keyset page request.
///
/// Binding: take the two optional query fields and normalize with
/// [`from_parts`](Self::from_parts) (`cursor` absent = first page; `page_size`
/// defaults to `20` and is clamped to `1..=100`, like offset pagination).
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct KeysetRequest {
    /// Opaque cursor for the next page (`None` = first page).
    #[serde(default)]
    pub cursor: Option<String>,
    /// Page size, clamped to `1..=100`.
    #[serde(default = "default_page_size")]
    pub page_size: u32,
}

fn default_page_size() -> u32 {
    20
}

impl KeysetRequest {
    /// Build from an explicit cursor/page size and clamp the size.
    pub fn new(cursor: Option<String>, page_size: u32) -> Self {
        Self { cursor, page_size }.clamped()
    }

    /// Build from the optional query fields of a list endpoint.
    ///
    /// `None` falls back to the framework defaults (no cursor, page size `20`)
    /// and the result is clamped, so the binding layer can hand over
    /// `Option<String>`/`Option<u32>` directly.
    pub fn from_parts(cursor: Option<String>, page_size: Option<u32>) -> Self {
        Self::new(cursor, page_size.unwrap_or(20))
    }

    /// Clamp the page size to `1..=100`.
    pub fn clamped(mut self) -> Self {
        self.page_size = self.page_size.clamp(1, 100);
        self
    }

    /// Effective page size (`1..=100`).
    pub fn page_size(&self) -> u32 {
        self.page_size.clamp(1, 100)
    }

    /// The opaque cursor, if any.
    pub fn cursor(&self) -> Option<&str> {
        self.cursor.as_deref()
    }

    /// Decode the cursor into its sort-key components.
    ///
    /// `Ok(Vec::new())` means "first page". The values are untrusted input:
    /// validate them (and scope them to the caller) before a query.
    pub fn decoded(&self) -> Result<Vec<String>, CursorError> {
        match self.cursor() {
            Some(cursor) if !cursor.is_empty() => Cursor::decode(cursor),
            _ => Ok(Vec::new()),
        }
    }
}

/// Opaque cursor codec.
///
/// A cursor is a base64url (no padding) JSON array of strings: one component per
/// stable sort-key column (timestamp, id, ...). It is not signed: tampering only
/// produces values the handler validates anyway.
pub struct Cursor;

impl Cursor {
    /// Encode sort-key components into a cursor.
    pub fn encode(components: &[&str]) -> String {
        let json = serde_json::to_vec(components).unwrap_or_else(|_| b"[]".to_vec());
        BASE64URL_NOPAD.encode(&json)
    }

    /// Decode a cursor into its sort-key components.
    pub fn decode(cursor: &str) -> Result<Vec<String>, CursorError> {
        let cursor = cursor.trim();
        if cursor.len() > MAX_CURSOR_BYTES {
            return Err(CursorError::TooLong);
        }
        let bytes = BASE64URL_NOPAD
            .decode(cursor.as_bytes())
            .map_err(|_| CursorError::Encoding)?;
        serde_json::from_slice::<Vec<String>>(&bytes).map_err(|_| CursorError::Format)
    }
}

/// One keyset page.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CursorPage<T: Serialize> {
    /// Rows of this page.
    pub items: Vec<T>,
    /// Cursor for the next page (`None` = no more rows).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
    /// Whether another page exists.
    pub has_more: bool,
}

impl<T: Serialize> CursorPage<T> {
    /// Build a page from items and the cursor that fetches the next page.
    pub fn new(items: Vec<T>, cursor: Option<String>) -> Self {
        Self {
            items,
            has_more: cursor.is_some(),
            cursor,
        }
    }

    /// Build a page from up to `page_size + 1` fetched rows.
    ///
    /// The extra row proves there is a next page; it is dropped from the page and
    /// the last returned row's sort key (built by `sort_key`) becomes the cursor.
    /// Fetching one extra row is what makes `has_more` exact without a `COUNT`.
    pub fn from_rows(
        mut rows: Vec<T>,
        page_size: u32,
        sort_key: impl Fn(&T) -> Vec<String>,
    ) -> Self {
        let page_size = page_size.clamp(1, 100) as usize;
        let has_more = rows.len() > page_size;
        if has_more {
            rows.truncate(page_size);
        }
        let cursor = rows.last().filter(|_| has_more).map(|last| {
            let key = sort_key(last);
            let refs: Vec<&str> = key.iter().map(String::as_str).collect();
            Cursor::encode(&refs)
        });
        Self {
            items: rows,
            cursor,
            has_more,
        }
    }

    /// Build a page from up to `page_size + 1` fetched **entities**, mapping each
    /// kept entity to the response item.
    ///
    /// The entity type does not have to be `Serialize`, and the response item
    /// does not have to carry the raw sort values: the cursor is computed from
    /// the last kept entity through `sort_key`, and `map` runs only for the rows
    /// that stay on the page (the probe row is dropped before mapping). Use it
    /// when the list query returns domain entities and the endpoint answers with
    /// DTOs:
    ///
    /// ```ignore
    /// let rows = load_after(&mut self.ctx, &req.room_id, key, paging.page_size() + 1).await?;
    /// Ok(CursorPage::from_entities(rows, paging.page_size(),
    ///     |row| vec![row.created_at.to_string(), row.id.clone()],
    ///     MessageDto::from_model))
    /// ```
    pub fn from_entities<E>(
        mut rows: Vec<E>,
        page_size: u32,
        sort_key: impl Fn(&E) -> Vec<String>,
        map: impl FnMut(E) -> T,
    ) -> Self {
        let page_size = page_size.clamp(1, 100) as usize;
        let has_more = rows.len() > page_size;
        if has_more {
            rows.truncate(page_size);
        }
        let cursor = rows.last().filter(|_| has_more).map(|last| {
            let key = sort_key(last);
            let refs: Vec<&str> = key.iter().map(String::as_str).collect();
            Cursor::encode(&refs)
        });
        Self {
            items: rows.into_iter().map(map).collect(),
            cursor,
            has_more,
        }
    }

    /// Map the kept items to another response type, preserving `cursor` and
    /// `has_more`.
    ///
    /// Rebuilding a page by hand (`CursorPage::new(mapped, page.cursor)`)
    /// silently recomputes `has_more` from `cursor.is_some()`; this helper keeps
    /// the page envelope intact and only transforms the items.
    pub fn map_items<U, F>(self, mut map: F) -> CursorPage<U>
    where
        U: Serialize,
        F: FnMut(T) -> U,
    {
        CursorPage {
            items: self.items.into_iter().map(&mut map).collect(),
            cursor: self.cursor,
            has_more: self.has_more,
        }
    }
}

impl From<CursorError> for crate::error::Error {
    /// A malformed client cursor is a request error (`400`), not a server fault.
    fn from(error: CursorError) -> Self {
        Self::Validation(error.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let components = ["2026-11-22T12:00:00Z", "订单-42", "x"];
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
