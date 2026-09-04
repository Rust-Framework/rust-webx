use proc_macro::TokenStream;
use quote::quote;
use syn::{parse_macro_input, Attribute, Data, DeriveInput, Fields, Meta, Type};

/// `#[derive(WebxRequestMeta)]` — registers OpenAPI parameter metadata for a request struct.
///
/// Fields annotated with `#[from_query]`, `#[from_route]`, or `#[from_body]` are included.
/// Unmarked fields on structs with `#[webx_request(query_all)]` are treated as query parameters
/// (excluding `claims` and `#[serde(skip)]` fields).
pub fn derive_request_meta(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    let type_name = input.ident.to_string();
    let query_all = has_query_all(&input.attrs);

    let fields = match &input.data {
        Data::Struct(data) => match &data.fields {
            Fields::Named(fields) => &fields.named,
            _ => {
                return syn::Error::new_spanned(
                    &input.ident,
                    "WebxRequestMeta only supports structs with named fields",
                )
                .to_compile_error()
                .into();
            }
        },
        _ => {
            return syn::Error::new_spanned(&input.ident, "WebxRequestMeta only supports structs")
                .to_compile_error()
                .into();
        }
    };

    let mut params_tokens = Vec::new();

    for field in fields {
        let Some(ident) = &field.ident else {
            continue;
        };
        let field_name = ident.to_string();

        if field_name == "claims" || field_has_serde_skip(&field.attrs) {
            continue;
        }

        let source = if has_path_attr(&field.attrs, "from_query") {
            "query"
        } else if has_path_attr(&field.attrs, "from_route") {
            "path"
        } else if has_path_attr(&field.attrs, "from_body") {
            "body"
        } else if query_all {
            "query"
        } else {
            continue;
        };

        let type_hint = type_hint_for(&field.ty);
        let name = field_name.as_str();

        params_tokens.push(quote! {
            ::rust_webx::ParamMeta {
                name: #name,
                source: #source,
                type_hint: #type_hint,
            }
        });
    }

    let expanded = quote! {
        ::inventory::submit! {
            ::rust_webx::RequestParamEntry {
                request_type: #type_name,
                params: &[#(#params_tokens),*],
            }
        }
    };

    TokenStream::from(expanded)
}

fn has_query_all(attrs: &[Attribute]) -> bool {
    attrs.iter().any(|attr| {
        if !attr.path().is_ident("webx_request") {
            return false;
        }
        matches!(&attr.meta, Meta::Path(_))
            || matches!(
                &attr.meta,
                Meta::List(list) if list.tokens.to_string().contains("query_all")
            )
    })
}

fn has_path_attr(attrs: &[Attribute], name: &str) -> bool {
    attrs.iter().any(|attr| attr.path().is_ident(name))
}

fn field_has_serde_skip(attrs: &[Attribute]) -> bool {
    attrs.iter().any(|attr| {
        attr.path().is_ident("serde")
            && attr
                .meta
                .require_list()
                .map(|list| list.tokens.to_string().contains("skip"))
                .unwrap_or(false)
    })
}

fn type_hint_for(ty: &Type) -> &'static str {
    match ty {
        Type::Path(tp) => {
            let name = tp
                .path
                .segments
                .last()
                .map(|s| s.ident.to_string())
                .unwrap_or_default();
            match name.as_str() {
                "String" | "str" => "string",
                "i8" | "i16" | "i32" | "i64" | "u8" | "u16" | "u32" | "u64" | "isize" | "usize" => {
                    "integer"
                }
                "f32" | "f64" => "number",
                "bool" => "boolean",
                "Option" => "string",
                "Vec" => "array",
                _ => "string",
            }
        }
        _ => "string",
    }
}
