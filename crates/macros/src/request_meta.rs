use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::{format_ident, quote};
use syn::{
    parenthesized, parse_macro_input, Attribute, Data, DeriveInput, Field, Fields, GenericArgument,
    Ident, LitStr, Meta, PathArguments, Token, Type,
};

/// `#[derive(WebxRequestMeta)]` — declares where each request field comes from.
///
/// The derive compiles one declaration into both the runtime binder
/// (`webx::RequestModel`) and the OpenAPI parameter metadata, so the two cannot
/// disagree.
///
/// # Field sources
///
/// | Attribute | Source |
/// |-----------|--------|
/// | `#[frompath("id")]` | the `{id}` route segment |
/// | `#[fromquery("q")]` | query string (repeatable, comma lists expand) |
/// | `#[frombody]` | the JSON body (key from `#[serde(rename)]`) |
/// | `#[fromform("title")]` | a `multipart/form-data` text part |
/// | `#[fromfile("avatar")]` | a `multipart/form-data` file part |
/// | `#[fromheader("X-Name")]` | an HTTP header (case-insensitive) |
/// | `#[fromclaim("tenant")]` | a claim of the authenticated principal |
///
/// Every marker except `#[frombody]` takes an optional source key; the field
/// name is used when it is omitted (`x_request_id` → `x-request-id` for
/// headers). `#[fromfile]` requires a `FormFile`-shaped field.
///
/// Unmarked fields follow the request method:
///
/// | Request | Order |
/// |---------|-------|
/// | `GET` / `HEAD` / `DELETE` / `OPTIONS` | query, then route |
/// | `POST` / `PUT` / `PATCH` | body, then form, then query |
/// | `multipart/form-data` | form, then query |
///
/// Header and claim values are never implicit.
///
/// # Body fields
///
/// Body fields bind one by one from the JSON object, so presence is exact:
/// `rename`, `rename_all`, `alias`, `default`, `flatten` and custom
/// deserializers all keep their serde meaning, and an absent key falls through
/// to the next source in the chain.
///
/// # Types
///
/// Known scalars (`String`, integers, floats, `bool`, `char`) bind from any
/// source. `Option<T>` and `Vec<T>` are optional. Any other type is a composite
/// (`ComponentModel`, `Vec<ComponentModel>`, `#[serde(flatten)]` fields) and
/// binds from the JSON body; a custom scalar (`Uuid`, an enum) binds from
/// query/path/header/claim only when marked explicitly.
///
/// `#[request(queryall)]` forces unmarked fields to the query string.
pub fn derive_request_meta(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    match expand(&input) {
        Ok(tokens) => tokens.into(),
        Err(error) => error.to_compile_error().into(),
    }
}

// ---------------------------------------------------------------------------
// Source classification
// ---------------------------------------------------------------------------

/// Where a field's value comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Src {
    Path,
    Query,
    Form,
    File,
    Header,
    Claim,
    Body,
    /// No marker: resolved from the request method.
    Auto,
}

impl Src {
    fn as_str(self) -> &'static str {
        match self {
            Src::Path => "path",
            Src::Query => "query",
            Src::Form | Src::File => "form",
            Src::Header => "header",
            Src::Claim => "claim",
            Src::Body => "body",
            Src::Auto => "",
        }
    }
}

/// The shape of a field's type, which decides how values are collected.
#[derive(Clone)]
enum Shape {
    /// A single value parsed with `FromStr`.
    Scalar(Type),
    /// `Option<T>`.
    Optional(Type),
    /// `Vec<T>`.
    Many(Type),
    /// `Option<Vec<T>>`.
    OptionalMany(Type),
    /// `FormFile`.
    File,
    /// `Option<FormFile>`.
    OptionalFile,
    /// `Vec<FormFile>`.
    Files,
}

impl Shape {
    fn value_ty(&self) -> Option<&Type> {
        match self {
            Shape::Scalar(ty) | Shape::Optional(ty) | Shape::Many(ty) | Shape::OptionalMany(ty) => {
                Some(ty)
            }
            Shape::File | Shape::OptionalFile | Shape::Files => None,
        }
    }

    fn is_file(&self) -> bool {
        matches!(self, Shape::File | Shape::OptionalFile | Shape::Files)
    }

    /// Whether an absent value is acceptable (and therefore not "required").
    fn is_optional(&self) -> bool {
        matches!(
            self,
            Shape::Optional(_)
                | Shape::Many(_)
                | Shape::OptionalMany(_)
                | Shape::OptionalFile
                | Shape::Files
        )
    }

    fn is_scalar(&self) -> bool {
        matches!(self, Shape::Scalar(_))
    }
}

/// Classify `ty` as scalar / optional / list / file.
fn shape_of(ty: &Type) -> Shape {
    let Type::Path(tp) = ty else {
        return Shape::Scalar(ty.clone());
    };
    let Some(last) = tp.path.segments.last() else {
        return Shape::Scalar(ty.clone());
    };

    match last.ident.to_string().as_str() {
        "FormFile" => Shape::File,
        "Option" => match first_generic(last) {
            Some(inner) => match shape_of(inner) {
                Shape::File => Shape::OptionalFile,
                Shape::Many(item) => Shape::OptionalMany(item),
                _ => Shape::Optional(inner.clone()),
            },
            None => Shape::Optional(ty.clone()),
        },
        "Vec" => match first_generic(last) {
            Some(inner) if matches!(shape_of(inner), Shape::File) => Shape::Files,
            Some(inner) => Shape::Many(inner.clone()),
            None => Shape::Many(ty.clone()),
        },
        _ => Shape::Scalar(ty.clone()),
    }
}

/// Whether `ty` is the framework's `CurrentUser` identity (or `Option` of it).
fn is_current_user(ty: &Type) -> bool {
    match shape_of(ty) {
        Shape::Scalar(inner) | Shape::Optional(inner) => path_ends_with(&inner, "CurrentUser"),
        _ => false,
    }
}

fn path_ends_with(ty: &Type, name: &str) -> bool {
    let Type::Path(path) = ty else {
        return false;
    };
    path.path
        .segments
        .last()
        .map(|segment| segment.ident == name)
        .unwrap_or(false)
}

fn first_generic(segment: &syn::PathSegment) -> Option<&Type> {
    match &segment.arguments {
        PathArguments::AngleBracketed(args) => match args.args.first() {
            Some(GenericArgument::Type(ty)) => Some(ty),
            _ => None,
        },
        _ => None,
    }
}

/// Whether a type can be parsed from a string.
fn is_known_scalar(ty: &Type) -> bool {
    let Type::Path(tp) = ty else {
        return false;
    };
    let Some(last) = tp.path.segments.last() else {
        return false;
    };
    matches!(
        last.ident.to_string().as_str(),
        "String"
            | "str"
            | "char"
            | "bool"
            | "i8"
            | "i16"
            | "i32"
            | "i64"
            | "i128"
            | "isize"
            | "u8"
            | "u16"
            | "u32"
            | "u64"
            | "u128"
            | "usize"
            | "f32"
            | "f64"
    )
}

/// A short OpenAPI / error-message type name.
fn type_hint(ty: &Type) -> &'static str {
    let Type::Path(tp) = ty else {
        return "string";
    };
    let Some(last) = tp.path.segments.last() else {
        return "string";
    };
    match last.ident.to_string().as_str() {
        "String" | "str" | "char" => "string",
        "i8" | "i16" | "i32" | "i64" | "i128" | "isize" | "u8" | "u16" | "u32" | "u64" | "u128"
        | "usize" => "integer",
        "f32" | "f64" => "number",
        "bool" => "boolean",
        "Option" | "Vec" => first_generic(last).map(type_hint).unwrap_or("string"),
        "FormFile" => "binary",
        _ => "string",
    }
}

// ---------------------------------------------------------------------------
// serde attributes
// ---------------------------------------------------------------------------

#[derive(Clone)]
enum DefaultKind {
    Bare,
    Path(syn::Path),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum RenameRule {
    Lower,
    Upper,
    Pascal,
    Camel,
    Snake,
    ScreamingSnake,
    Kebab,
    ScreamingKebab,
}

impl RenameRule {
    fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "lowercase" => RenameRule::Lower,
            "UPPERCASE" => RenameRule::Upper,
            "PascalCase" => RenameRule::Pascal,
            "camelCase" => RenameRule::Camel,
            "snake_case" => RenameRule::Snake,
            "SCREAMING_SNAKE_CASE" => RenameRule::ScreamingSnake,
            "kebab-case" => RenameRule::Kebab,
            "SCREAMING-KEBAB-CASE" => RenameRule::ScreamingKebab,
            _ => return None,
        })
    }

    fn apply(self, field: &str) -> String {
        let words: Vec<String> = field
            .split('_')
            .filter(|word| !word.is_empty())
            .map(|word| word.to_ascii_lowercase())
            .collect();
        let capitalize = |word: &str| -> String {
            let mut chars = word.chars();
            match chars.next() {
                Some(first) => first.to_ascii_uppercase().to_string() + chars.as_str(),
                None => String::new(),
            }
        };
        match self {
            RenameRule::Lower => words.concat(),
            RenameRule::Upper => words.concat().to_ascii_uppercase(),
            RenameRule::Snake => words.join("_"),
            RenameRule::ScreamingSnake => words.join("_").to_ascii_uppercase(),
            RenameRule::Kebab => words.join("-"),
            RenameRule::ScreamingKebab => words.join("-").to_ascii_uppercase(),
            RenameRule::Pascal => words.iter().map(|w| capitalize(w)).collect(),
            RenameRule::Camel => words
                .iter()
                .enumerate()
                .map(|(index, word)| {
                    if index == 0 {
                        word.clone()
                    } else {
                        capitalize(word)
                    }
                })
                .collect(),
        }
    }
}

/// Container-level `#[serde(...)]` facts.
#[derive(Default)]
struct ContainerSerde {
    rename_all: Option<RenameRule>,
    deny_unknown_fields: bool,
    default: Option<DefaultKind>,
    transparent: bool,
}

fn container_serde(attrs: &[Attribute]) -> syn::Result<ContainerSerde> {
    let mut flags = ContainerSerde::default();
    for attr in attrs {
        if !attr.path().is_ident("serde") {
            continue;
        }
        attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("rename_all") {
                let lit: LitStr = meta.value()?.parse()?;
                flags.rename_all = RenameRule::parse(&lit.value());
                if flags.rename_all.is_none() {
                    return Err(syn::Error::new_spanned(
                        &lit,
                        "unsupported `rename_all` rule; expected lowercase, UPPERCASE, \
                         PascalCase, camelCase, snake_case, SCREAMING_SNAKE_CASE, kebab-case \
                         or SCREAMING-KEBAB-CASE",
                    ));
                }
                return Ok(());
            }
            if meta.path.is_ident("deny_unknown_fields") {
                flags.deny_unknown_fields = true;
                return Ok(());
            }
            if meta.path.is_ident("transparent") {
                flags.transparent = true;
                return Ok(());
            }
            if meta.path.is_ident("default") {
                flags.default = Some(default_kind(meta)?);
                return Ok(());
            }
            consume_meta_value(meta)
        })?;
    }
    Ok(flags)
}

fn default_kind(meta: syn::meta::ParseNestedMeta<'_>) -> syn::Result<DefaultKind> {
    if meta.input.peek(Token![=]) {
        let lit: LitStr = meta.value()?.parse()?;
        let path: syn::Path = lit.parse()?;
        Ok(DefaultKind::Path(path))
    } else {
        Ok(DefaultKind::Bare)
    }
}

/// Field-level `#[serde(...)]` facts that affect binding.
#[derive(Default)]
struct FieldSerde {
    skip: bool,
    flatten: bool,
    custom: Option<CustomKind>,
    default: Option<DefaultKind>,
    rename: Option<String>,
    aliases: Vec<String>,
}

enum CustomKind {
    With(LitStr),
    DeserializeWith(LitStr),
}

fn field_serde(attrs: &[Attribute]) -> syn::Result<FieldSerde> {
    let mut flags = FieldSerde::default();
    for attr in attrs {
        if !attr.path().is_ident("serde") {
            continue;
        }
        attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("skip") || meta.path.is_ident("skip_deserializing") {
                flags.skip = true;
                return Ok(());
            }
            if meta.path.is_ident("flatten") {
                flags.flatten = true;
                return Ok(());
            }
            if meta.path.is_ident("with") {
                flags.custom = Some(CustomKind::With(meta.value()?.parse()?));
                return Ok(());
            }
            if meta.path.is_ident("deserialize_with") {
                flags.custom = Some(CustomKind::DeserializeWith(meta.value()?.parse()?));
                return Ok(());
            }
            if meta.path.is_ident("default") {
                flags.default = Some(default_kind(meta)?);
                return Ok(());
            }
            if meta.path.is_ident("alias") {
                let lit: LitStr = meta.value()?.parse()?;
                flags.aliases.push(lit.value());
                return Ok(());
            }
            if meta.path.is_ident("rename") {
                // `rename = "x"` or `rename(serialize = "...", deserialize = "...")`.
                if meta.input.peek(Token![=]) {
                    let lit: LitStr = meta.value()?.parse()?;
                    flags.rename = Some(lit.value());
                } else if meta.input.peek(syn::token::Paren) {
                    meta.parse_nested_meta(|inner| {
                        let value: LitStr = inner.value()?.parse()?;
                        if inner.path.is_ident("deserialize") {
                            flags.rename = Some(value.value());
                        }
                        Ok(())
                    })?;
                }
                return Ok(());
            }
            consume_meta_value(meta)
        })?;
    }
    Ok(flags)
}

/// Consume an attribute argument we do not interpret.
fn consume_meta_value(meta: syn::meta::ParseNestedMeta<'_>) -> syn::Result<()> {
    if meta.input.peek(Token![=]) {
        let _: syn::Expr = meta.value()?.parse()?;
    } else if meta.input.peek(syn::token::Paren) {
        let content;
        parenthesized!(content in meta.input);
        let _: TokenStream2 = content.parse()?;
    }
    Ok(())
}

/// Read `#[request(queryall)]`; reject unknown options.
fn query_all_option(input: &DeriveInput) -> syn::Result<bool> {
    let mut query_all = false;
    for attr in &input.attrs {
        if !attr.path().is_ident("request") {
            continue;
        }
        match &attr.meta {
            Meta::List(_) => attr.parse_nested_meta(|meta| {
                if meta.path.is_ident("queryall") {
                    query_all = true;
                    Ok(())
                } else {
                    Err(meta.error(
                        "unknown `#[request]` option; expected `queryall` \
                         (`#[request(queryall)]`)",
                    ))
                }
            })?,
            Meta::Path(_) => {
                return Err(syn::Error::new_spanned(
                    attr,
                    "`#[request]` needs an argument; write `#[request(queryall)]`",
                ))
            }
            Meta::NameValue(meta) => {
                return Err(syn::Error::new_spanned(
                    meta,
                    "`#[request]` takes an option list, not `=`",
                ))
            }
        }
    }
    Ok(query_all)
}

/// The source declared on `field`, if any.
fn declared_source(field: &Field) -> syn::Result<Option<(Src, Option<String>)>> {
    let mut found: Option<(Src, Option<String>)> = None;

    for attr in &field.attrs {
        let src = if attr.path().is_ident("frompath") {
            Src::Path
        } else if attr.path().is_ident("fromquery") {
            Src::Query
        } else if attr.path().is_ident("frombody") {
            Src::Body
        } else if attr.path().is_ident("fromform") {
            Src::Form
        } else if attr.path().is_ident("fromfile") {
            Src::File
        } else if attr.path().is_ident("fromheader") {
            Src::Header
        } else if attr.path().is_ident("fromclaim") {
            Src::Claim
        } else {
            continue;
        };

        if found.is_some() {
            return Err(syn::Error::new_spanned(
                attr,
                "a field may declare only one source; remove the extra `#[from*]`",
            ));
        }

        let name = match (&attr.meta, src) {
            (Meta::Path(_), _) => None,
            (Meta::List(list), Src::Body) => {
                return Err(syn::Error::new_spanned(
                    list,
                    "`#[frombody]` takes no key; control the JSON key with `#[serde(rename)]`",
                ))
            }
            (Meta::List(list), _) => {
                let lit: LitStr = list.parse_args()?;
                let value = lit.value();
                if value.is_empty() {
                    return Err(syn::Error::new_spanned(
                        &lit,
                        "a source key must not be empty",
                    ));
                }
                Some(value)
            }
            (Meta::NameValue(_), _) => {
                return Err(syn::Error::new_spanned(
                    attr,
                    "pass the source key as an argument: `#[fromquery(\"q\")]`",
                ))
            }
        };

        found = Some((src, name));
    }

    Ok(found)
}

// ---------------------------------------------------------------------------
// Field model
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq)]
enum BodyKind {
    Normal,
    Flatten,
    Custom,
}

struct FieldSpec {
    ident: Ident,
    ty: Type,
    name: String,
    shape: Shape,
    src: Src,
    /// Route segment / query key / form part / header name / claim key.
    source_key: String,
    /// JSON key resolved from `rename` / `rename_all`.
    json_key: String,
    aliases: Vec<String>,
    body_kind: BodyKind,
    /// Effective default (`#[serde(default)]`, field or container level).
    default: Option<DefaultKind>,
    /// Container-level default: the value comes from a shared default instance.
    container_default: bool,
    skipped: bool,
    flattened: bool,
    required: bool,
    hint: &'static str,
    custom: Option<CustomKind>,
    /// Name of the hidden helper struct for latten / custom deserializers.
    helper: Ident,
}

impl FieldSpec {
    /// The documented source key (OpenAPI parameter name).
    fn public_name(&self) -> &str {
        &self.source_key
    }

    /// A composite field has no string form; it can only come from the body.
    fn body_only(&self) -> bool {
        if self.skipped {
            return false;
        }
        if self.flattened {
            return true;
        }
        if self.src != Src::Auto || self.shape.is_file() {
            return false;
        }
        match &self.shape {
            Shape::Scalar(ty) | Shape::Optional(ty) | Shape::Many(ty) | Shape::OptionalMany(ty) => {
                !is_known_scalar(ty)
            }
            Shape::File | Shape::OptionalFile | Shape::Files => false,
        }
    }
}

fn build_fields(
    type_ident: &Ident,
    input: &DeriveInput,
    query_all: bool,
    container: &ContainerSerde,
) -> syn::Result<Vec<FieldSpec>> {
    let fields = match &input.data {
        Data::Struct(data) => &data.fields,
        Data::Enum(_) | Data::Union(_) => {
            return Err(syn::Error::new_spanned(
                &input.ident,
                "WebxRequestMeta can only be derived for a struct",
            ))
        }
    };

    let named = match fields {
        Fields::Named(named) => &named.named,
        Fields::Unit => return Ok(Vec::new()),
        Fields::Unnamed(_) => {
            return Err(syn::Error::new_spanned(
                &input.ident,
                "WebxRequestMeta requires named fields; tuple structs cannot be bound \
                 by field name",
            ))
        }
    };

    let mut specs = Vec::with_capacity(named.len());
    for field in named {
        let Some(ident) = field.ident.clone() else {
            continue;
        };
        let name = ident.to_string();
        let shape = shape_of(&field.ty);
        let serde = field_serde(&field.attrs)?;
        let declared = declared_source(field)?;

        // The injected `claims` field is filled by the dispatcher, never bound.
        if name == "claims" {
            specs.push(FieldSpec {
                ident,
                ty: field.ty.clone(),
                name,
                shape,
                src: Src::Auto,
                source_key: String::new(),
                json_key: String::new(),
                aliases: Vec::new(),
                body_kind: BodyKind::Normal,
                default: None,
                container_default: false,
                skipped: true,
                flattened: false,
                required: false,
                hint: "string",
                custom: None,
                helper: format_ident!("__WebxBody_claims"),
            });
            continue;
        }

        let has_declared = declared.is_some();
        let (src, extra) = match declared {
            Some((src, arg)) => (src, arg),
            None if query_all => (Src::Query, None),
            None if shape.is_file() => (Src::File, None),
            None => (Src::Auto, None),
        };

        let skipped = !has_declared && serde.skip;
        if is_current_user(&field.ty) {
            if src != Src::Claim {
                return Err(syn::Error::new_spanned(
                    field,
                    "a `CurrentUser` field carries the authenticated identity; mark it \
                     `#[fromclaim]`",
                ));
            }
            if extra.is_some() {
                return Err(syn::Error::new_spanned(
                    field,
                    "`#[fromclaim]` on a `CurrentUser` field takes no key; the whole \
                     identity is bound from the principal",
                ));
            }
        }
        if src == Src::Body && serde.skip {
            return Err(syn::Error::new_spanned(
                field,
                "a `#[frombody]` field cannot be `#[serde(skip)]`; the body would never \
                 carry it",
            ));
        }

        if serde.flatten && serde.custom.is_some() {
            return Err(syn::Error::new_spanned(
                field,
                "`#[serde(flatten)]` and `#[serde(with)]` cannot be combined",
            ));
        }
        if serde.flatten && !matches!(src, Src::Body | Src::Auto) {
            return Err(syn::Error::new_spanned(
                field,
                "a `#[serde(flatten)]` field is part of the JSON body; remove the \
                 `#[from*]` marker",
            ));
        }
        if src == Src::File && !shape.is_file() {
            return Err(syn::Error::new_spanned(
                field,
                "`#[fromfile]` requires a `FormFile`, `Option<FormFile>` or `Vec<FormFile>` \
                 field",
            ));
        }
        if shape.is_file() && matches!(src, Src::Header | Src::Claim | Src::Path | Src::Query) {
            return Err(syn::Error::new_spanned(
                field,
                "`FormFile` can only be bound from `multipart/form-data`; use \
                 `#[fromfile]` or leave the field unmarked",
            ));
        }

        let source_key = match src {
            Src::Path | Src::Query | Src::Form | Src::File => extra.unwrap_or_else(|| name.clone()),
            Src::Header => extra.unwrap_or_else(|| name.replace('_', "-")),
            Src::Claim => extra.unwrap_or_else(|| name.clone()),
            Src::Body | Src::Auto => name.clone(),
        };
        if matches!(src, Src::Header) && source_key.is_empty() {
            return Err(syn::Error::new_spanned(
                field,
                "`#[fromheader]` needs a non-empty header name",
            ));
        }

        let json_key = serde.rename.clone().unwrap_or_else(|| {
            container
                .rename_all
                .map(|rule| rule.apply(&name))
                .unwrap_or_else(|| name.clone())
        });

        let body_kind = if serde.flatten {
            BodyKind::Flatten
        } else if serde.custom.is_some() {
            BodyKind::Custom
        } else {
            BodyKind::Normal
        };

        let default = serde.default.clone().or_else(|| container.default.clone());
        let container_default = serde.default.is_none() && container.default.is_some();

        let hint = match &shape {
            Shape::File | Shape::OptionalFile => "binary",
            Shape::Files => "binary[]",
            Shape::Scalar(ty) | Shape::Optional(ty) | Shape::Many(ty) | Shape::OptionalMany(ty) => {
                type_hint(ty)
            }
        };

        let suffix = match body_kind {
            BodyKind::Flatten => "Flatten",
            BodyKind::Custom => "Custom",
            BodyKind::Normal => "Body",
        };
        let helper = Ident::new(
            &format!(
                "__WebxBody_{}_{}_{}",
                type_ident,
                name.replace("r#", ""),
                suffix
            ),
            ident.span(),
        );

        specs.push(FieldSpec {
            required: !skipped && !shape.is_optional() && default.is_none(),
            ident,
            ty: field.ty.clone(),
            name,
            shape,
            src,
            source_key,
            json_key,
            aliases: serde.aliases,
            body_kind,
            default,
            container_default,
            skipped,
            flattened: serde.flatten,
            hint,
            custom: serde.custom,
            helper,
        });
    }

    Ok(specs)
}

// ---------------------------------------------------------------------------
// Expansion
// ---------------------------------------------------------------------------

fn expand(input: &DeriveInput) -> syn::Result<TokenStream2> {
    if !input.generics.params.is_empty() {
        return Err(syn::Error::new_spanned(
            &input.generics,
            "WebxRequestMeta does not support generic request structs; use a concrete \
             request type (generics cannot be bound by field name)",
        ));
    }

    let type_ident = &input.ident;
    let is_unit = matches!(&input.data, Data::Struct(data) if matches!(data.fields, Fields::Unit));
    let query_all = query_all_option(input)?;
    let container = container_serde(&input.attrs)?;
    if container.transparent {
        return Err(syn::Error::new_spanned(
            type_ident,
            "`#[serde(transparent)]` request models are not supported; declare the \
             fields you want to bind",
        ));
    }

    let fields = build_fields(type_ident, input, query_all, &container)?;

    let has_explicit_form = fields
        .iter()
        .any(|f| !f.skipped && matches!(f.src, Src::Form | Src::File));
    let has_explicit_body = fields.iter().any(|f| !f.skipped && f.src == Src::Body);

    if has_explicit_form && has_explicit_body {
        return Err(syn::Error::new_spanned(
            type_ident,
            "a request cannot bind both `#[fromform]` and `#[frombody]`: a request \
             has one body",
        ));
    }

    let has_form = has_explicit_form
        || fields
            .iter()
            .any(|f| !f.skipped && f.src == Src::Auto && f.shape.is_file());
    let has_body = !has_form
        && fields
            .iter()
            .any(|f| !f.skipped && matches!(f.src, Src::Body | Src::Auto));

    let needs_claim = fields
        .iter()
        .any(|f| !f.skipped && f.src == Src::Claim && f.required);

    let has_flatten = fields.iter().any(|f| !f.skipped && f.flattened);
    let meta = param_meta(&fields, has_form);

    let plan = BindingPlan {
        has_form,
        has_body,
        has_explicit_body,
        needs_claim,
        is_unit,
        deny_unknown_fields: container.deny_unknown_fields && !has_flatten,
        has_default_instance: fields.iter().any(|f| !f.skipped && f.container_default),
        default_instance: container.default.clone(),
    };

    let helpers = helper_structs(&fields);
    let binder = bind_fn(&fields, &plan);

    let user_impl = fields
        .iter()
        .find(|f| !f.skipped && is_current_user(&f.ty))
        .map(|f| {
            let ident = &f.ident;
            let access = if f.shape.is_optional() {
                quote! { self.#ident.as_ref() }
            } else {
                quote! { ::core::option::Option::Some(&self.#ident) }
            };
            quote! {
                impl ::webx::RequestUser for #type_ident {
                    fn current_user(
                        &self,
                    ) -> ::core::option::Option<&::webx::CurrentUser> {
                        #access
                    }
                }
            }
        });

    Ok(quote! {
        #helpers

        impl ::webx::RequestModel for #type_ident {
            fn param_meta() -> &'static [::webx::ParamMeta] {
                &[#(#meta),*]
            }

            #binder
        }

        #user_impl

        ::webx::inventory::submit! {
            ::webx::RequestParamEntry {
                request_type: ::core::stringify!(#type_ident),
                params: &[#(#meta),*],
            }
        }
    })
}

/// Helper structs for fields that need serde machinery we cannot inline.
fn helper_structs(fields: &[FieldSpec]) -> TokenStream2 {
    let helpers = fields.iter().filter(|f| !f.skipped).filter_map(|f| {
        let name = &f.helper;
        let ty = &f.ty;
        match f.body_kind {
            BodyKind::Flatten => Some(quote! {
                #[doc(hidden)]
                #[allow(non_camel_case_types, dead_code)]
                #[derive(::webx::serde::Deserialize)]
                struct #name {
                    #[serde(flatten)]
                    v: #ty,
                }
            }),
            BodyKind::Custom => {
                let attr = match &f.custom {
                    Some(CustomKind::With(lit)) => quote! { #[serde(with = #lit)] },
                    Some(CustomKind::DeserializeWith(lit)) => {
                        quote! { #[serde(deserialize_with = #lit)] }
                    }
                    None => quote! {},
                };
                Some(quote! {
                    #[doc(hidden)]
                    #[allow(non_camel_case_types, dead_code)]
                    #[derive(::webx::serde::Deserialize)]
                    struct #name {
                        #attr
                        v: #ty,
                    }
                })
            }
            BodyKind::Normal => None,
        }
    });
    quote! { #(#helpers)* }
}

/// OpenAPI parameter metadata.
fn param_meta(fields: &[FieldSpec], has_form: bool) -> Vec<TokenStream2> {
    let mut entries = Vec::new();
    let mut body_documented = false;

    for f in fields.iter().filter(|f| !f.skipped) {
        if is_current_user(&f.ty) {
            continue;
        }
        if f.src == Src::Body || f.body_only() {
            body_documented = true;
            // Auto fields are resolved by the route method, so leave their
            // source empty; explicit body fields are documented once below.
            if f.src != Src::Body {
                continue;
            }
            continue;
        }
        let source = if f.src == Src::Auto && has_form {
            "form"
        } else {
            f.src.as_str()
        };
        let name = f.public_name();
        let hint = f.hint;
        let required = f.required;
        entries.push(quote! {
            ::webx::ParamMeta {
                name: #name,
                source: #source,
                type_hint: #hint,
                required: #required,
            }
        });
    }

    if body_documented {
        entries.push(quote! {
            ::webx::ParamMeta {
                name: "body",
                source: "body",
                type_hint: "object",
                required: true,
            }
        });
    }

    entries
}

struct BindingPlan {
    has_form: bool,
    has_body: bool,
    has_explicit_body: bool,
    needs_claim: bool,
    is_unit: bool,
    deny_unknown_fields: bool,
    has_default_instance: bool,
    default_instance: Option<DefaultKind>,
}

fn bind_fn(fields: &[FieldSpec], plan: &BindingPlan) -> TokenStream2 {
    let has_fields = fields.iter().any(|f| !f.skipped);

    if !has_fields {
        let construct = construct_expr(fields, plan.is_unit);
        return quote! {
            fn bind<'__webx>(
                _ctx: &'__webx mut dyn ::webx::IHttpContext,
            ) -> ::core::pin::Pin<
                ::std::boxed::Box<
                    dyn ::core::future::Future<Output = ::webx::Result<Self>>
                        + ::core::marker::Send
                        + '__webx,
                >,
            > {
                ::std::boxed::Box::pin(async move {
                    ::core::result::Result::Ok(#construct)
                })
            }
        };
    }

    let body_decl = if plan.has_body {
        quote! {
            let mut __body: ::core::option::Option<::webx::serde_json::Value> =
                ::core::option::Option::None;
            if __is_body_method && !__is_multipart {
                let __bytes = ctx.request_mut().body_bytes().await?;
                let __slice: &[u8] = if __bytes.is_empty() { b"{}" } else { &__bytes };
                let __value: ::webx::serde_json::Value =
                    ::webx::serde_json::from_slice(__slice)
                        .map_err(::webx::binding::body_error)?;
                if !__value.is_object() {
                    return ::core::result::Result::Err(::webx::Error::Http(
                        "the request body must be a JSON object".to_string(),
                    ));
                }
                __body = ::core::option::Option::Some(__value);
            }
        }
    } else {
        quote! {}
    };

    let defaults_decl = if plan.has_default_instance {
        match &plan.default_instance {
            Some(DefaultKind::Bare) => quote! {
                let __defaults: Self = <Self as ::core::default::Default>::default();
            },
            Some(DefaultKind::Path(path)) => quote! { let __defaults: Self = #path(); },
            None => quote! {},
        }
    } else {
        quote! {}
    };

    let form_guard = if plan.has_form {
        quote! {
            if !__is_multipart {
                return ::core::result::Result::Err(::webx::Error::UnsupportedMediaType(
                    "this endpoint binds `multipart/form-data`; send the request as a form"
                        .to_string(),
                ));
            }
        }
    } else {
        quote! {}
    };

    let json_form_guard = if !plan.has_form && plan.has_body {
        let explicit_body = plan.has_explicit_body;
        quote! {
            if __is_multipart && (#explicit_body || __is_body_method) {
                return ::core::result::Result::Err(::webx::Error::UnsupportedMediaType(
                    "this endpoint binds a JSON body; `multipart/form-data` is not supported"
                        .to_string(),
                ));
            }
        }
    } else {
        quote! {}
    };

    let explicit_body_guard = if plan.has_explicit_body {
        quote! {
            if !__is_body_method && !__is_multipart {
                return ::core::result::Result::Err(::webx::Error::Http(
                    "`#[frombody]` fields require a POST, PUT or PATCH request".to_string(),
                ));
            }
        }
    } else {
        quote! {}
    };

    let complex_fields: Vec<&str> = fields
        .iter()
        .filter(|f| !f.skipped && f.body_only() && f.required)
        .map(|f| f.name.as_str())
        .collect();
    let complex_guard = if complex_fields.is_empty() {
        quote! {}
    } else {
        let names = complex_fields.join(", ");
        quote! {
            if !__is_body_method && !__is_multipart {
                return ::core::result::Result::Err(::webx::Error::Http(format!(
                    "composite fields [{}] can only bind from a JSON body; \
                     mark them `#[fromquery]`/`#[frompath]` or use POST/PUT/PATCH",
                    #names
                )));
            }
        }
    };

    let form_decl = if plan.has_form {
        quote! {
            let mut __form: ::core::option::Option<
                ::std::sync::Arc<::webx::MultipartForm>,
            > = ::core::option::Option::None;
            if __is_multipart {
                __form = ::core::option::Option::Some(ctx.request_mut().multipart().await?);
            }
        }
    } else {
        quote! {
            let __form: ::core::option::Option<::std::sync::Arc<::webx::MultipartForm>> =
                ::core::option::Option::None;
        }
    };

    let claim_guard = if plan.needs_claim {
        quote! {
            if __in.claims.is_none() {
                return ::core::result::Result::Err(::webx::Error::Unauthorized(
                    "this endpoint binds claims; authenticate the request".to_string(),
                ));
            }
        }
    } else {
        quote! {}
    };

    let statements: Vec<TokenStream2> = fields
        .iter()
        .filter(|f| !f.skipped)
        .map(|f| bind_field(f, plan))
        .collect();

    let unknown_check = if plan.deny_unknown_fields {
        quote! {
            if let ::core::option::Option::Some(::webx::serde_json::Value::Object(__map)) =
                __body.as_ref()
            {
                if let ::core::option::Option::Some(__unknown) = __map.keys().next() {
                    __binder.unknown_field(__unknown);
                }
            }
        }
    } else {
        quote! {}
    };

    let construct = construct_expr(fields, plan.is_unit);

    quote! {
        fn bind<'__webx>(
            ctx: &'__webx mut dyn ::webx::IHttpContext,
        ) -> ::core::pin::Pin<
            ::std::boxed::Box<
                dyn ::core::future::Future<Output = ::webx::Result<Self>>
                    + ::core::marker::Send
                    + '__webx,
            >,
        > {
            ::std::boxed::Box::pin(async move {
                let __is_multipart = ctx.request().is_multipart();
                let __is_body_method = ::webx::binding::is_body_method(ctx.request().method());
                #form_guard
                #json_form_guard
                #explicit_body_guard
                #complex_guard
                #form_decl
                #body_decl
                #defaults_decl
                let __req = ctx.request();
                let __in = ::webx::binding::BindInput {
                    path: __req.route_params(),
                    query: __req.query(),
                    query_multi: __req.query_all(),
                    claims: ctx.claims(),
                    form: __form.as_deref(),
                    is_body: __is_body_method,
                    is_multipart: __is_multipart,
                };

                #claim_guard

                let mut __binder = ::webx::binding::Binder::new();
                #(#statements)*
                #unknown_check

                if __binder.has_errors() {
                    return ::core::result::Result::Err(__binder.into_error());
                }

                ::core::result::Result::Ok(#construct)
            })
        }
    }
}

fn construct_expr(fields: &[FieldSpec], is_unit: bool) -> TokenStream2 {
    if is_unit {
        return quote! { Self };
    }
    let inits = fields.iter().map(|f| {
        let ident = &f.ident;
        if f.skipped {
            quote! { #ident: ::core::default::Default::default() }
        } else {
            let local = format_ident!("__f_{}", ident);
            match f.shape {
                Shape::Scalar(_) | Shape::File => {
                    quote! { #ident: #local.expect("required field was bound") }
                }
                _ => quote! { #ident: #local },
            }
        }
    });
    quote! { Self { #(#inits),* } }
}

/// A value source in the fallback chain.
enum Read {
    Body,
    Query,
    Form,
    Path,
    Header(String),
    Claim(String),
}

fn bind_field(f: &FieldSpec, plan: &BindingPlan) -> TokenStream2 {
    let ident = &f.ident;
    let local = format_ident!("__f_{}", ident);

    let local_ty = match &f.shape {
        Shape::Scalar(ty) => quote! { ::core::option::Option<#ty> },
        Shape::Optional(ty) => quote! { ::core::option::Option<#ty> },
        Shape::Many(ty) => quote! { ::std::vec::Vec<#ty> },
        Shape::OptionalMany(ty) => quote! { ::core::option::Option<::std::vec::Vec<#ty>> },
        Shape::File | Shape::OptionalFile => quote! { ::core::option::Option<::webx::FormFile> },
        Shape::Files => quote! { ::std::vec::Vec<::webx::FormFile> },
    };

    if is_current_user(&f.ty) {
        let expr = if f.shape.is_optional() {
            quote! { __in.claims.map(::webx::CurrentUser::from_claims) }
        } else {
            quote! {
                match __in.claims {
                    ::core::option::Option::Some(__claims) => {
                        ::core::option::Option::Some(
                            ::webx::CurrentUser::from_claims(__claims),
                        )
                    }
                    ::core::option::Option::None => {
                        return ::core::result::Result::Err(::webx::Error::Unauthorized(
                            "authentication required for this endpoint".to_string(),
                        ));
                    }
                }
            }
        };
        return quote! { let #local: #local_ty = #expr; };
    }

    let expr = match &f.shape {
        Shape::File => {
            let key = LitStr::new(&f.source_key, ident.span());
            let public = LitStr::new(&f.name, ident.span());
            quote! { __binder.required_file(__in.file(#key), #public) }
        }
        Shape::OptionalFile => {
            let key = LitStr::new(&f.source_key, ident.span());
            quote! { __binder.optional_file(__in.file(#key)) }
        }
        Shape::Files => {
            let key = LitStr::new(&f.source_key, ident.span());
            quote! { __binder.files(__in.files(#key)) }
        }
        _ => {
            let reads = match f.src {
                Src::Path => vec![Read::Path],
                Src::Query => vec![Read::Query],
                Src::Form | Src::File => vec![Read::Form],
                Src::Header => vec![Read::Header(f.source_key.to_ascii_lowercase())],
                Src::Claim => vec![Read::Claim(f.source_key.clone())],
                Src::Body => vec![Read::Body],
                Src::Auto if f.body_only() => vec![Read::Body],
                Src::Auto if plan.has_form => vec![Read::Form, Read::Query],
                Src::Auto => {
                    let body = chain(f, &[Read::Body, Read::Query]);
                    let safe = chain(f, &[Read::Query, Read::Path]);
                    return quote! {
                        let #local: #local_ty = if __is_body_method {
                            #body
                        } else {
                            #safe
                        };
                    };
                }
            };
            chain(f, &reads)
        }
    };

    quote! {
        let #local: #local_ty = #expr;
    }
}

fn chain(f: &FieldSpec, reads: &[Read]) -> TokenStream2 {
    match &f.shape {
        Shape::Scalar(_) | Shape::Optional(_) => single_chain(f, reads),
        Shape::Many(_) | Shape::OptionalMany(_) => many_chain(f, reads),
        Shape::File | Shape::OptionalFile | Shape::Files => unreachable!("file shapes"),
    }
}

/// The value used when no source in the chain supplied anything.
fn terminal(f: &FieldSpec) -> TokenStream2 {
    let ident = &f.ident;
    let field = LitStr::new(&f.name, f.ident.span());

    // A default wins over "required": it always produces a value.
    if f.container_default {
        let defaults = quote! { __defaults.#ident };
        return match f.shape {
            Shape::Scalar(_) => quote! { ::core::option::Option::Some(#defaults) },
            _ => defaults,
        };
    }
    if let Some(default) = &f.default {
        let value = match default {
            DefaultKind::Bare => quote! { ::core::default::Default::default() },
            DefaultKind::Path(path) => quote! { #path() },
        };
        return match f.shape {
            Shape::Scalar(_) => quote! { ::core::option::Option::Some(#value) },
            _ => value,
        };
    }

    match f.shape {
        Shape::Scalar(_) => quote! {{
            __binder.missing(#field);
            ::core::option::Option::None
        }},
        Shape::Optional(_) | Shape::OptionalMany(_) => quote! { ::core::option::Option::None },
        Shape::Many(_) => quote! { ::std::vec::Vec::new() },
        Shape::File | Shape::OptionalFile | Shape::Files => unreachable!("file shapes"),
    }
}

/// The body key lookup, taking the value out of the object.
fn body_raw(f: &FieldSpec) -> TokenStream2 {
    let key = LitStr::new(&f.json_key, f.ident.span());
    let aliases: Vec<LitStr> = f
        .aliases
        .iter()
        .map(|alias| LitStr::new(alias, f.ident.span()))
        .collect();
    quote! {
        match __body.as_mut() {
            ::core::option::Option::Some(::webx::serde_json::Value::Object(__map)) => {
                ::webx::binding::take_body_field(__map, #key, &[#(#aliases),*])
            }
            _ => ::core::option::Option::None,
        }
    }
}

/// How a present body value becomes the field's local value.
fn body_present(f: &FieldSpec) -> TokenStream2 {
    let field = LitStr::new(&f.name, f.ident.span());
    let helper = &f.helper;
    let ty = &f.ty;

    match f.body_kind {
        BodyKind::Normal => {
            let parsed = quote! {
                ::webx::serde_json::from_value::<#ty>(__v)
                    .map_err(|__e| ::webx::binding::body_field_error(#field, __e))?
            };
            wrap_declared(f, parsed)
        }
        BodyKind::Custom => {
            let parsed = quote! {{
                let __wrapped = ::webx::serde_json::json!({ "v": __v });
                ::webx::serde_json::from_value::<#helper>(__wrapped)
                    .map_err(|__e| ::webx::binding::body_field_error(#field, __e))?.v
            }};
            wrap_declared(f, parsed)
        }
        BodyKind::Flatten => {
            let parsed = quote! {
                ::webx::serde_json::from_value::<#helper>(__v)
                    .map_err(|__e| ::webx::binding::body_field_error(#field, __e))?.v
            };
            wrap_declared(f, parsed)
        }
    }
}

fn wrap_declared(f: &FieldSpec, value: TokenStream2) -> TokenStream2 {
    if f.shape.is_scalar() {
        quote! { ::core::option::Option::Some(#value) }
    } else {
        value
    }
}

fn single_chain(f: &FieldSpec, reads: &[Read]) -> TokenStream2 {
    let field = LitStr::new(&f.name, f.ident.span());
    let kind = f.hint;
    let ty = f.shape.value_ty().expect("value shape");
    let optional = matches!(f.shape, Shape::Optional(_));

    let mut expr = terminal(f);

    for read in reads.iter().rev() {
        expr = match read {
            Read::Body => {
                let raw = if f.body_kind == BodyKind::Flatten {
                    quote! { __body.as_ref().cloned() }
                } else {
                    body_raw(f)
                };
                let present = body_present(f);
                let missing = expr;
                quote! { match #raw { Some(__v) => #present, None => #missing } }
            }
            other => {
                let fetch = single_fetch(f, other);
                let parse = if optional {
                    quote! { __binder.optional::<#ty>(Some(__v), #field, #kind) }
                } else {
                    quote! { __binder.required::<#ty>(Some(__v), #field, #kind) }
                };
                let missing = expr;
                quote! { match #fetch { Some(__v) => #parse, None => #missing } }
            }
        };
    }

    expr
}

fn many_chain(f: &FieldSpec, reads: &[Read]) -> TokenStream2 {
    let field = LitStr::new(&f.name, f.ident.span());
    let kind = f.hint;
    let ty = f.shape.value_ty().expect("value shape");
    let optional_many = matches!(f.shape, Shape::OptionalMany(_));

    let mut expr = terminal(f);

    for read in reads.iter().rev() {
        expr = match read {
            Read::Body => {
                let raw = if f.body_kind == BodyKind::Flatten {
                    quote! { __body.as_ref().cloned() }
                } else {
                    body_raw(f)
                };
                let present = body_present(f);
                let missing = expr;
                quote! { match #raw { Some(__v) => #present, None => #missing } }
            }
            other => {
                let fetch = list_fetch(f, other);
                let missing = expr;
                if optional_many {
                    quote! {{
                        let __values = #fetch;
                        if __values.is_empty() {
                            #missing
                        } else {
                            ::core::option::Option::Some(
                                __binder.list::<#ty>(&__values, #field, #kind),
                            )
                        }
                    }}
                } else {
                    quote! {{
                        let __values = #fetch;
                        if __values.is_empty() {
                            #missing
                        } else {
                            __binder.list::<#ty>(&__values, #field, #kind)
                        }
                    }}
                }
            }
        };
    }

    expr
}

fn single_fetch(f: &FieldSpec, read: &Read) -> TokenStream2 {
    let ident = &f.ident;
    let key = LitStr::new(&f.source_key, ident.span());
    match read {
        Read::Body => unreachable!("body values are not string sources"),
        Read::Query => quote! { __in.query_one(#key) },
        Read::Form => quote! { __in.form_one(#key) },
        Read::Path => quote! { __in.path_one(#key) },
        Read::Header(lower) => {
            let lower = LitStr::new(lower, ident.span());
            quote! { __req.header(#lower) }
        }
        Read::Claim(key) => {
            let key = LitStr::new(key, ident.span());
            quote! { __in.claim(#key) }
        }
    }
}

fn list_fetch(f: &FieldSpec, read: &Read) -> TokenStream2 {
    let ident = &f.ident;
    let key = LitStr::new(&f.source_key, ident.span());
    match read {
        Read::Body => unreachable!("body values are not string sources"),
        Read::Query => quote! { __in.query_list(#key) },
        Read::Form => quote! { __in.form_list(#key) },
        Read::Path => quote! { __in.path_list(#key) },
        Read::Header(lower) => {
            let lower = LitStr::new(lower, ident.span());
            quote! { ::webx::binding::split_csv(__req.header(#lower)) }
        }
        Read::Claim(key) => {
            let key = LitStr::new(key, ident.span());
            quote! { __in.claim_list(#key) }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn err(source: &str) -> String {
        let input: DeriveInput = syn::parse_str(source).expect("parse");
        match expand(&input) {
            Ok(_) => panic!("expected an error for {source}"),
            Err(error) => error.to_string(),
        }
    }

    fn ok(source: &str) {
        let input: DeriveInput = syn::parse_str(source).expect("parse");
        if let Err(error) = expand(&input) {
            panic!("unexpected error for {source}: {error}");
        }
    }

    #[test]
    fn accepts_the_documented_markers() {
        ok(r#"
            struct Req {
                #[frompath("id")] id: String,
                #[fromquery("q")] q: Option<String>,
                #[frombody] name: String,
                #[fromheader("X-A")] a: String,
                #[fromclaim("sub")] sub: String,
            }
        "#);
        ok(r#"
            struct Upload {
                #[fromfile("avatar")] avatar: FormFile,
                #[fromform("title")] title: String,
            }
        "#);
    }

    #[test]
    fn rejects_a_field_with_two_sources() {
        let message = err(r#"
            struct Req {
                #[fromquery]
                #[frompath]
                id: String,
            }
        "#);
        assert!(message.contains("only one source"), "{message}");
    }

    #[test]
    fn rejects_tuple_structs() {
        let message = err("struct Req(String);");
        assert!(message.contains("named fields"), "{message}");
    }

    #[test]
    fn rejects_generic_models() {
        let message = err("struct Req<T> { value: T }");
        assert!(message.contains("generic"), "{message}");
    }

    #[test]
    fn rejects_a_body_key_argument() {
        let message = err(r#"
            struct Req {
                #[frombody("x")] name: String,
            }
        "#);
        assert!(message.contains("takes no key"), "{message}");
    }

    #[test]
    fn rejects_fromfile_on_a_scalar() {
        let message = err(r#"
            struct Req {
                #[fromfile] name: String,
            }
        "#);
        assert!(message.contains("FormFile"), "{message}");
    }

    #[test]
    fn rejects_mixing_form_and_body() {
        let message = err(r#"
            struct Req {
                #[fromform] title: String,
                #[frombody] name: String,
            }
        "#);
        assert!(message.contains("cannot bind both"), "{message}");
    }

    #[test]
    fn rejects_unknown_request_options() {
        let message = err(r#"
            #[request(bogus)]
            struct Req { id: String }
        "#);
        assert!(message.contains("unknown"), "{message}");
    }

    #[test]
    fn rejects_transparent_models() {
        let message = err(r#"
            #[serde(transparent)]
            struct Req { id: String }
        "#);
        assert!(message.contains("transparent"), "{message}");
    }

    #[test]
    fn accepts_the_standard_identity_field() {
        ok(r#"
            struct Req {
                #[fromclaim] user: CurrentUser,
                #[fromclaim] viewer: Option<CurrentUser>,
            }
        "#);
    }

    #[test]
    fn rejects_current_user_without_a_source() {
        let message = err(r#"
            struct Req {
                user: CurrentUser,
            }
        "#);
        assert!(message.contains("CurrentUser"), "{message}");
    }

    #[test]
    fn rejects_a_key_on_the_standard_identity() {
        let message = err(r#"
            struct Req {
                #[fromclaim("sub")] user: CurrentUser,
            }
        "#);
        assert!(message.contains("takes no key"), "{message}");
    }

    #[test]
    fn rejects_a_header_on_a_file_field() {
        let message = err(r#"
            struct Req {
                #[fromheader("X-File")] file: FormFile,
            }
        "#);
        assert!(message.contains("multipart"), "{message}");
    }
}
