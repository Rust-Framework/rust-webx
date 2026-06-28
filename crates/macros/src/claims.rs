//! `#[claims]` attribute macro.
//!
//! Eliminates the boilerplate of declaring a `claims` field, marking it
//! `#[serde(skip)]`, and writing an inherent `set_claims` method on every
//! authenticated request struct.
//!
//! ## Usage
//!
//! ```ignore
//! use rust_webapp::*;
//! use serde::Deserialize;
//!
//! #[claims]
//! #[derive(Default, Deserialize)]
//! pub struct CreateCommentRequest {
//!     pub blog_id: i32,
//!     pub content: String,
//! }
//! ```
//!
//! ## Expansion
//!
//! The attribute runs **before** `#[derive(...)]` (place it on top), and:
//!
//! 1. Appends `#[serde(skip)] pub claims: Option<Box<dyn IClaims>>` to the
//!    struct's fields.
//! 2. Generates an inherent `set_claims` method that shadows the blanket
//!    no-op `IClaimsCarrier::set_claims` from the framework, so the
//!    dispatcher's `request.set_claims(claims)` call writes into the field.
//!
//! The macro must be the **outermost** attribute so that `#[derive(Default)]`
//! and `#[derive(Deserialize)]` see the injected field:
//!
//! ```ignore
//! #[claims]                         // ← runs first, injects field
//! #[derive(Default, Deserialize)]   // ← derives run on expanded struct
//! pub struct MyRequest { /* user fields */ }
//! ```

use proc_macro::TokenStream;
use quote::quote;
use syn::{parse_macro_input, ItemStruct};

/// Implementation of `#[claims]`.
///
/// Accepts structs with named fields or unit structs (which are normalized to
/// empty named-field structs). Tuple structs are rejected with a clear
/// compile-time error.
pub fn claims_impl(_attr: TokenStream, item: TokenStream) -> TokenStream {
    let mut input = parse_macro_input!(item as ItemStruct);

    let struct_name = input.ident.clone();

    // Normalize to named-fields form. Unit structs (`struct Foo;`) are
    // converted to empty named-fields structs (`struct Foo {}`) so the
    // `claims` field can be injected. Tuple structs remain unsupported.
    match &mut input.fields {
        syn::Fields::Named(_) => { /* already named */ }
        syn::Fields::Unit => {
            input.fields = syn::Fields::Named(syn::FieldsNamed {
                brace_token: syn::token::Brace::default(),
                named: syn::punctuated::Punctuated::new(),
            });
        }
        syn::Fields::Unnamed(_) => {
            return syn::Error::new_spanned(
                &input,
                "#[claims] requires a struct with named fields; \
                 tuple structs are not supported",
            )
            .to_compile_error()
            .into();
        }
    }

    let fields_named = match &mut input.fields {
        syn::Fields::Named(named) => named,
        _ => unreachable!("normalized to named fields above"),
    };

    // Detect whether `#[derive(...)]` includes `Deserialize` — only then does
    // the injected `claims` field need `#[serde(skip)]` to avoid serde trying
    // to deserialize `Box<dyn IClaims>` (which has no `Deserialize` impl).
    //
    // The `#[serde(skip)]` helper attribute is registered by
    // `#[derive(Deserialize)]`, so it can only be used when that derive is
    // present. For structs without `Deserialize` (e.g., route-param-only
    // requests), no skip attribute is needed.
    let derives_deserialize = input.attrs.iter().any(|attr| {
        if !attr.path().is_ident("derive") {
            return false;
        }
        let nested: syn::Result<syn::MetaList> = attr.meta.require_list().cloned();
        match nested {
            Ok(list) => list
                .parse_args_with(syn::punctuated::Punctuated::<syn::Path, syn::Token![,]>::parse_terminated)
                .map(|paths| {
                    paths
                        .iter()
                        .any(|p| p.is_ident("Deserialize") || p.is_ident("serde::Deserialize"))
                })
                .unwrap_or(false),
            Err(_) => false,
        }
    });

    let injected_field: syn::Field = if derives_deserialize {
        syn::parse_quote! {
            #[serde(skip)]
            pub claims: ::std::option::Option<::std::boxed::Box<dyn ::rust_webapp::IClaims>>
        }
    } else {
        syn::parse_quote! {
            pub claims: ::std::option::Option<::std::boxed::Box<dyn ::rust_webapp::IClaims>>
        }
    };
    fields_named.named.push(injected_field);

    let expanded = quote! {
        #input

        impl #struct_name {
            /// Injects authentication claims into the request. Called by the
            /// LRWF dispatcher before `IRequestHandler::handle`. Shadows the
            /// blanket no-op `IClaimsCarrier::set_claims` for this type.
            pub fn set_claims(&mut self, claims: ::std::option::Option<::std::boxed::Box<dyn ::rust_webapp::IClaims>>) {
                self.claims = claims;
            }
        }
    };

    expanded.into()
}
