//! A `serde::Deserializer` over a parsed [`MultipartForm`].
//!
//! This is what lets ordinary `#[derive(Deserialize)]` request structs bind
//! `multipart/form-data` without a second derive or a parallel binder: the
//! framework hands the derive a map of field name to value, where text parts
//! become strings and file parts become a handle to the parsed [`FormFile`].
//!
//! File handles are resolved through a task-local scope installed by
//! [`crate::route::bind::bind_form_request`], so a `FormFile` produced this way
//! keeps its own reference-counted ownership of the uploaded bytes and stays
//! valid after the form itself is dropped. A JSON body can never satisfy that
//! handle, which is what keeps `FormFile` from becoming an arbitrary-file-read
//! primitive.
//!
//! Route parameters are merged in as text values, so a route such as
//! `POST /api/users/{id}/avatar` binds `{id}` alongside the uploaded file.

use super::{ActiveForm, FormFile, MultipartForm};
use serde::de::{
    self, DeserializeSeed, EnumAccess, Error as _, MapAccess, SeqAccess, VariantAccess, Visitor,
};
use serde::forward_to_deserialize_any;
use serde::Deserializer;
use std::collections::HashMap;
use std::fmt;

/// Key used inside the `FormFile` metadata map to carry the form handle.
const FILE_HANDLE_KEY: &str = "$webx_form_file";

/// Error produced while binding a `multipart/form-data` request.
#[derive(Debug, Clone)]
pub struct FormError(String);

impl FormError {
    /// The human-readable reason binding failed.
    pub fn message(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for FormError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for FormError {}

impl de::Error for FormError {
    fn custom<T: fmt::Display>(msg: T) -> Self {
        FormError(msg.to_string())
    }
}

/// One value slot in the form, in insertion order.
enum ValueRef<'a> {
    Text(&'a str),
    /// A file part: its index into [`MultipartForm::files`].
    File(usize),
}

struct Group<'a> {
    name: &'a str,
    values: Vec<ValueRef<'a>>,
}

/// Deserializer over a [`MultipartForm`].
///
/// Build one with [`FormDeserializer::new`] and pass it to `T::deserialize`.
/// Prefer [`crate::route::bind::bind_form_request`], which also installs the
/// task-local scope that file handles need.
pub struct FormDeserializer<'a> {
    groups: Vec<Group<'a>>,
}

impl<'a> FormDeserializer<'a> {
    /// Build a deserializer over `form`.
    ///
    /// `route_params` are merged in as text values and take precedence over
    /// same-named form fields, mirroring how JSON body binding overlays path
    /// parameters.
    pub fn new(form: &'a MultipartForm, route_params: &'a HashMap<String, String>) -> Self {
        let mut groups: Vec<Group<'a>> = Vec::new();
        let mut index: HashMap<&'a str, usize> = HashMap::new();

        {
            let mut push = |groups: &mut Vec<Group<'a>>, name: &'a str, value: ValueRef<'a>| {
                match index.get(name) {
                    Some(&at) => groups[at].values.push(value),
                    None => {
                        index.insert(name, groups.len());
                        groups.push(Group {
                            name,
                            values: vec![value],
                        });
                    }
                }
            };

            for (name, value) in route_params {
                push(&mut groups, name.as_str(), ValueRef::Text(value.as_str()));
            }
            for field in form.fields() {
                if route_params.contains_key(&field.name) {
                    continue;
                }
                push(
                    &mut groups,
                    field.name.as_str(),
                    ValueRef::Text(field.value.as_str()),
                );
            }
            for (position, file) in form.files().iter().enumerate() {
                let name = file.field_name();
                if route_params.contains_key(name) {
                    continue;
                }
                push(&mut groups, name, ValueRef::File(position));
            }
        }

        Self { groups }
    }
}

impl<'de, 'a> Deserializer<'de> for FormDeserializer<'a> {
    type Error = FormError;

    fn deserialize_any<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, FormError> {
        self.deserialize_map(visitor)
    }

    fn deserialize_map<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, FormError> {
        visitor.visit_map(FormMapAccess {
            groups: self.groups,
            index: 0,
        })
    }

    fn deserialize_struct<V: Visitor<'de>>(
        self,
        _name: &'static str,
        _fields: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, FormError> {
        self.deserialize_map(visitor)
    }

    fn deserialize_option<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, FormError> {
        visitor.visit_some(self)
    }

    fn deserialize_unit<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, FormError> {
        visitor.visit_unit()
    }

    fn deserialize_unit_struct<V: Visitor<'de>>(
        self,
        _name: &'static str,
        visitor: V,
    ) -> Result<V::Value, FormError> {
        visitor.visit_unit()
    }

    fn deserialize_newtype_struct<V: Visitor<'de>>(
        self,
        _name: &'static str,
        visitor: V,
    ) -> Result<V::Value, FormError> {
        visitor.visit_newtype_struct(self)
    }

    forward_to_deserialize_any! {
        bool i8 i16 i32 i64 i128 u8 u16 u32 u64 u128 f32 f64 char str string
        bytes byte_buf seq tuple tuple_struct enum identifier ignored_any
    }
}

struct FormMapAccess<'a> {
    groups: Vec<Group<'a>>,
    index: usize,
}

impl<'de, 'a> MapAccess<'de> for FormMapAccess<'a> {
    type Error = FormError;

    fn next_key_seed<K: DeserializeSeed<'de>>(
        &mut self,
        seed: K,
    ) -> Result<Option<K::Value>, FormError> {
        if self.index >= self.groups.len() {
            return Ok(None);
        }
        let key = seed.deserialize(StrDeserializer(self.groups[self.index].name))?;
        Ok(Some(key))
    }

    fn next_value_seed<V: DeserializeSeed<'de>>(&mut self, seed: V) -> Result<V::Value, FormError> {
        let values = &self.groups[self.index].values;
        self.index += 1;
        seed.deserialize(FormValueDeserializer { values })
    }
}

struct FormValueDeserializer<'a> {
    values: &'a [ValueRef<'a>],
}

impl<'a> FormValueDeserializer<'a> {
    /// First text value for this key.
    ///
    /// Repeated text fields bind to the first value for scalar targets, which
    /// matches how browsers submit checkboxes and how other web frameworks
    /// bind repeated keys.
    fn first_text(&self) -> Result<&'a str, FormError> {
        self.values
            .iter()
            .find_map(|value| match value {
                ValueRef::Text(text) => Some(*text),
                ValueRef::File(_) => None,
            })
            .ok_or_else(|| FormError::custom("expected a text field, found a file part"))
    }
}

macro_rules! parse_text {
    ($method:ident, $visit:ident, $ty:ty, $label:literal) => {
        fn $method<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, FormError> {
            let text = self.first_text()?;
            let value: $ty = text.parse().map_err(|_| {
                FormError::custom(format!("cannot parse {text:?} as {} for {}", $label, stringify!($method)))
            })?;
            visitor.$visit(value)
        }
    };
}

impl<'de, 'a> Deserializer<'de> for FormValueDeserializer<'a> {
    type Error = FormError;

    fn deserialize_any<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, FormError> {
        match self.values {
            [] => visitor.visit_unit(),
            [ValueRef::Text(text)] => visitor.visit_str(text),
            [ValueRef::File(position)] => {
                visitor.visit_map(FormFileAccess::single(*position))
            }
            many => visitor.visit_seq(FormSeqAccess { values: many, index: 0 }),
        }
    }

    fn deserialize_option<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, FormError> {
        if self.values.is_empty() {
            visitor.visit_none()
        } else {
            visitor.visit_some(self)
        }
    }

    fn deserialize_seq<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, FormError> {
        visitor.visit_seq(FormSeqAccess {
            values: self.values,
            index: 0,
        })
    }

    fn deserialize_map<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, FormError> {
        match self.values {
            [ValueRef::File(position)] => {
                visitor.visit_map(FormFileAccess::single(*position))
            }
            _ => Err(FormError::custom(
                "expected a single file part for this field",
            )),
        }
    }

    fn deserialize_struct<V: Visitor<'de>>(
        self,
        _name: &'static str,
        _fields: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, FormError> {
        self.deserialize_map(visitor)
    }

    fn deserialize_newtype_struct<V: Visitor<'de>>(
        self,
        _name: &'static str,
        visitor: V,
    ) -> Result<V::Value, FormError> {
        visitor.visit_newtype_struct(self)
    }

    fn deserialize_enum<V: Visitor<'de>>(
        self,
        _name: &'static str,
        _variants: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, FormError> {
        visitor.visit_enum(FormEnumAccess(self.first_text()?))
    }

    fn deserialize_unit<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, FormError> {
        visitor.visit_unit()
    }

    fn deserialize_unit_struct<V: Visitor<'de>>(
        self,
        _name: &'static str,
        visitor: V,
    ) -> Result<V::Value, FormError> {
        visitor.visit_unit()
    }

    fn deserialize_str<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, FormError> {
        visitor.visit_str(self.first_text()?)
    }

    fn deserialize_string<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, FormError> {
        visitor.visit_str(self.first_text()?)
    }

    fn deserialize_identifier<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, FormError> {
        visitor.visit_str(self.first_text()?)
    }

    fn deserialize_bytes<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, FormError> {
        visitor.visit_bytes(self.first_text()?.as_bytes())
    }

    fn deserialize_byte_buf<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, FormError> {
        visitor.visit_byte_buf(self.first_text()?.as_bytes().to_vec())
    }

    parse_text!(deserialize_bool, visit_bool, bool, "a boolean");
    parse_text!(deserialize_i8, visit_i8, i8, "an integer");
    parse_text!(deserialize_i16, visit_i16, i16, "an integer");
    parse_text!(deserialize_i32, visit_i32, i32, "an integer");
    parse_text!(deserialize_i64, visit_i64, i64, "an integer");
    parse_text!(deserialize_u8, visit_u8, u8, "an integer");
    parse_text!(deserialize_u16, visit_u16, u16, "an integer");
    parse_text!(deserialize_u32, visit_u32, u32, "an integer");
    parse_text!(deserialize_u64, visit_u64, u64, "an integer");
    parse_text!(deserialize_f32, visit_f32, f32, "a number");
    parse_text!(deserialize_f64, visit_f64, f64, "a number");

    fn deserialize_char<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, FormError> {
        let text = self.first_text()?;
        let mut chars = text.chars();
        match (chars.next(), chars.next()) {
            (Some(ch), None) => visitor.visit_char(ch),
            _ => Err(FormError::custom(format!(
                "expected a single character, got {text:?}"
            ))),
        }
    }

    forward_to_deserialize_any! {
        i128 u128 tuple tuple_struct ignored_any
    }
}

struct FormSeqAccess<'a> {
    values: &'a [ValueRef<'a>],
    index: usize,
}

impl<'de, 'a> SeqAccess<'de> for FormSeqAccess<'a> {
    type Error = FormError;

    fn next_element_seed<T: DeserializeSeed<'de>>(
        &mut self,
        seed: T,
    ) -> Result<Option<T::Value>, FormError> {
        if self.index >= self.values.len() {
            return Ok(None);
        }
        let slice = &self.values[self.index..self.index + 1];
        self.index += 1;
        seed.deserialize(FormValueDeserializer { values: slice })
            .map(Some)
    }

    fn size_hint(&self) -> Option<usize> {
        Some(self.values.len() - self.index)
    }
}

// ---------------------------------------------------------------------------
// FormFile serde representation
// ---------------------------------------------------------------------------

/// Single-entry map carrying the form-local handle for one file part.
struct FormFileAccess {
    position: usize,
    index: usize,
}

impl FormFileAccess {
    fn single(position: usize) -> Self {
        Self { position, index: 0 }
    }
}

impl<'de> MapAccess<'de> for FormFileAccess {
    type Error = FormError;

    fn next_key_seed<K: DeserializeSeed<'de>>(
        &mut self,
        seed: K,
    ) -> Result<Option<K::Value>, FormError> {
        if self.index > 0 {
            return Ok(None);
        }
        seed.deserialize(StrDeserializer(FILE_HANDLE_KEY))
            .map(Some)
    }

    fn next_value_seed<V: DeserializeSeed<'de>>(&mut self, seed: V) -> Result<V::Value, FormError> {
        self.index += 1;
        seed.deserialize(U64Deserializer(self.position as u64))
    }

    fn size_hint(&self) -> Option<usize> {
        Some(1 - self.index.min(1))
    }
}

/// Deserializer for a bare `u64`.
struct U64Deserializer(u64);

impl<'de> Deserializer<'de> for U64Deserializer {
    type Error = FormError;

    fn deserialize_any<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, FormError> {
        visitor.visit_u64(self.0)
    }

    forward_to_deserialize_any! {
        bool i8 i16 i32 i64 i128 u8 u16 u32 u64 u128 f32 f64 char str string
        bytes byte_buf option unit unit_struct newtype_struct seq tuple tuple_struct
        map struct enum identifier ignored_any
    }
}

/// String deserializer used for map keys.
struct StrDeserializer<'a>(&'a str);

impl<'de, 'a> Deserializer<'de> for StrDeserializer<'a> {
    type Error = FormError;

    fn deserialize_any<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, FormError> {
        visitor.visit_str(self.0)
    }

    forward_to_deserialize_any! {
        bool i8 i16 i32 i64 i128 u8 u16 u32 u64 u128 f32 f64 char str string
        bytes byte_buf option unit unit_struct newtype_struct seq tuple tuple_struct
        map struct enum identifier ignored_any
    }
}

struct FormEnumAccess<'a>(&'a str);

impl<'de, 'a> EnumAccess<'de> for FormEnumAccess<'a> {
    type Error = FormError;
    type Variant = FormVariantAccess;

    fn variant_seed<V: DeserializeSeed<'de>>(
        self,
        seed: V,
    ) -> Result<(V::Value, Self::Variant), FormError> {
        let variant = seed.deserialize(StrDeserializer(self.0))?;
        Ok((variant, FormVariantAccess))
    }
}

struct FormVariantAccess;

impl<'de> VariantAccess<'de> for FormVariantAccess {
    type Error = FormError;

    fn unit_variant(self) -> Result<(), FormError> {
        Ok(())
    }

    fn newtype_variant_seed<T: DeserializeSeed<'de>>(self, _seed: T) -> Result<T::Value, FormError> {
        Err(FormError::custom(
            "form fields cannot carry a newtype enum variant; use a unit variant",
        ))
    }

    fn tuple_variant<V: Visitor<'de>>(
        self,
        _len: usize,
        _visitor: V,
    ) -> Result<V::Value, FormError> {
        Err(FormError::custom(
            "form fields cannot carry a tuple enum variant; use a unit variant",
        ))
    }

    fn struct_variant<V: Visitor<'de>>(
        self,
        _fields: &'static [&'static str],
        _visitor: V,
    ) -> Result<V::Value, FormError> {
        Err(FormError::custom(
            "form fields cannot carry a struct enum variant; use a unit variant",
        ))
    }
}

// ---------------------------------------------------------------------------
// Deserialize for FormFile
// ---------------------------------------------------------------------------

impl<'de> serde::Deserialize<'de> for FormFile {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_map(FormFileVisitor)
    }
}

struct FormFileVisitor;

impl<'de> Visitor<'de> for FormFileVisitor {
    type Value = FormFile;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("an uploaded file part")
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<FormFile, A::Error> {
        let mut handle: Option<u64> = None;
        while let Some(key) = map.next_key::<String>()? {
            if key == FILE_HANDLE_KEY {
                handle = Some(map.next_value()?);
            } else {
                let _: de::IgnoredAny = map.next_value()?;
            }
        }

        let Some(position) = handle else {
            return Err(de::Error::custom(
                "FormFile can only be bound from a multipart/form-data request",
            ));
        };

        ActiveForm::resolve(position as usize).ok_or_else(|| {
            de::Error::custom("FormFile can only be bound from a multipart/form-data request")
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::form::FormFileBuilder;
    use serde::Deserialize;

    #[derive(Debug, Deserialize, PartialEq)]
    struct Profile {
        username: String,
        age: u32,
        verified: bool,
        nickname: Option<String>,
    }

    fn text_form() -> MultipartForm {
        let mut form = MultipartForm::new();
        form.push_field("username", "ada");
        form.push_field("age", "36");
        form.push_field("verified", "true");
        form
    }

    fn bind<T: serde::de::DeserializeOwned>(form: MultipartForm) -> Result<T, FormError> {
        let form = std::sync::Arc::new(form);
        let params = HashMap::new();
        ActiveForm::run(std::sync::Arc::clone(&form), || {
            T::deserialize(FormDeserializer::new(&form, &params))
        })
    }

    #[test]
    fn binds_text_fields_to_a_struct() {
        let profile: Profile = bind(text_form()).expect("bind failed");
        assert_eq!(
            profile,
            Profile {
                username: "ada".into(),
                age: 36,
                verified: true,
                nickname: None,
            }
        );
    }

    #[test]
    fn reports_a_clear_error_for_bad_numbers() {
        let mut form = MultipartForm::new();
        form.push_field("username", "ada");
        form.push_field("age", "not-a-number");
        form.push_field("verified", "true");
        let err: FormError = bind::<Profile>(form).unwrap_err();
        assert!(err.to_string().contains("cannot parse"), "got {err}");
    }

    #[test]
    fn route_params_override_form_fields() {
        let form = std::sync::Arc::new(text_form());
        let mut params = HashMap::new();
        params.insert("username".to_string(), "grace".to_string());
        let profile = ActiveForm::run(std::sync::Arc::clone(&form), || {
            Profile::deserialize(FormDeserializer::new(&form, &params))
        })
        .unwrap();
        assert_eq!(profile.username, "grace");
    }

    #[derive(Debug, Deserialize)]
    struct Upload {
        title: String,
        avatar: FormFile,
    }

    #[tokio::test]
    async fn binds_memory_backed_file_parts() {
        let mut builder =
            FormFileBuilder::new("avatar", "../evil/photo.png", Some("image/png".into()));
        builder.write(b"\x89PNG").await.unwrap();
        let mut form = MultipartForm::new();
        form.push_field("title", "hi");
        form.push_file(builder.finish().await.unwrap());

        let upload: Upload = bind(form).unwrap();
        assert_eq!(upload.title, "hi");
        assert_eq!(upload.avatar.file_name(), "photo.png");
        assert_eq!(upload.avatar.size(), 4);
        assert_eq!(upload.avatar.read_bytes().await.unwrap(), b"\x89PNG");
    }

    #[derive(Debug, Deserialize)]
    struct Gallery {
        photos: Vec<FormFile>,
    }

    #[tokio::test]
    async fn binds_repeated_file_parts_into_a_vec() {
        let mut form = MultipartForm::new();
        for (name, body) in [("a.png", &b"aaa"[..]), ("b.png", &b"bbbb"[..])] {
            let mut builder = FormFileBuilder::new("photos", name, None);
            builder.write(body).await.unwrap();
            form.push_file(builder.finish().await.unwrap());
        }
        let gallery: Gallery = bind(form).unwrap();
        assert_eq!(gallery.photos.len(), 2);
        assert_eq!(gallery.photos[0].read_bytes().await.unwrap(), b"aaa");
        assert_eq!(gallery.photos[1].read_bytes().await.unwrap(), b"bbbb");
    }

    #[tokio::test]
    async fn spooled_files_outlive_the_form() {
        let dir = tempfile::tempdir().unwrap();
        let mut builder = FormFileBuilder::new("avatar", "big.bin", None)
            .memory_threshold(1)
            .spool_dir(dir.path().to_path_buf());
        builder.write(b"0123456789").await.unwrap();
        let mut form = MultipartForm::new();
        form.push_file(builder.finish().await.unwrap());

        let upload: Upload = {
            let mut form = form.clone();
            form.push_field("title", "hi");
            bind(form).unwrap()
        };
        // The form itself is gone; the file handle must still own the bytes.
        assert_eq!(upload.avatar.read_bytes().await.unwrap(), b"0123456789");
    }

    #[test]
    fn json_cannot_smuggle_a_file_handle() {
        let json = r#"{"title":"x","avatar":{"$webx_form_file":0}}"#;
        let err = serde_json::from_str::<Upload>(json).unwrap_err();
        assert!(
            err.to_string().contains("multipart/form-data"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn json_with_arbitrary_keys_is_rejected() {
        let json = r#"{"title":"x","avatar":{"path":"/etc/passwd"}}"#;
        let err = serde_json::from_str::<Upload>(json).unwrap_err();
        assert!(
            err.to_string().contains("multipart/form-data"),
            "unexpected error: {err}"
        );
    }

    #[derive(Debug, Deserialize, PartialEq)]
    enum Kind {
        Disk,
        Ssd,
    }

    #[derive(Debug, Deserialize)]
    struct WithEnum {
        kind: Kind,
    }

    #[test]
    fn binds_unit_enum_variants() {
        let mut form = MultipartForm::new();
        form.push_field("kind", "Ssd");
        let bound: WithEnum = bind(form).unwrap();
        assert_eq!(bound.kind, Kind::Ssd);
    }
}
