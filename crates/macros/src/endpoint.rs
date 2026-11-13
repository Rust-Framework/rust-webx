use proc_macro::TokenStream;
use quote::{format_ident, quote};
use syn::{
    parse::{Parse, ParseStream},
    parse_macro_input,
    punctuated::Punctuated,
    Attribute, Expr, GenericArgument, ItemImpl, Lit, LitStr, Meta, PathArguments, Token, Type,
};

// =====================================================================
// Generic #[endpoint(HttpMethod, "/path")] — full form
// =====================================================================

pub fn endpoint_impl(attr: TokenStream, item: TokenStream) -> TokenStream {
    emit_endpoint(attr, item)
}

// =====================================================================
// Shortcut macros: #[get("/path")], #[post("/path")], etc.
// =====================================================================

pub fn shortcut_get(attr: TokenStream, item: TokenStream) -> TokenStream {
    let path = parse_shortcut_attr(&attr.to_string());
    let attr_str = format!("Get, \"{}\"", path);
    emit_endpoint(attr_str.parse().unwrap(), item)
}

pub fn shortcut_post(attr: TokenStream, item: TokenStream) -> TokenStream {
    let path = parse_shortcut_attr(&attr.to_string());
    let attr_str = format!("Post, \"{}\"", path);
    emit_endpoint(attr_str.parse().unwrap(), item)
}

pub fn shortcut_put(attr: TokenStream, item: TokenStream) -> TokenStream {
    let path = parse_shortcut_attr(&attr.to_string());
    let attr_str = format!("Put, \"{}\"", path);
    emit_endpoint(attr_str.parse().unwrap(), item)
}

pub fn shortcut_delete(attr: TokenStream, item: TokenStream) -> TokenStream {
    let path = parse_shortcut_attr(&attr.to_string());
    let attr_str = format!("Delete, \"{}\"", path);
    emit_endpoint(attr_str.parse().unwrap(), item)
}

// =====================================================================
// Internal helpers
// =====================================================================

fn parse_shortcut_attr(attr: &str) -> String {
    let s = attr.trim();
    if s.is_empty() {
        "/".to_string()
    } else {
        s.trim_matches('"').trim_matches('\'').to_string()
    }
}

/// Generate a RouteEntry AND a RouteDispatch from an `impl IRequest<Response> for Name` block.
fn emit_endpoint(attr: TokenStream, item: TokenStream) -> TokenStream {
    let item_impl = parse_macro_input!(item as ItemImpl);
    let (method_str, path_str) = parse_endpoint_attr(&attr.to_string());
    match expand_endpoint(&method_str, &path_str, &item_impl) {
        Ok(tokens) => tokens,
        Err(error) => error.to_compile_error().into(),
    }
}

fn expand_endpoint(
    method_str: &str,
    path_str: &str,
    item_impl: &ItemImpl,
) -> syn::Result<TokenStream> {
    let self_ty = &item_impl.self_ty;
    let req_type_name = extract_type_name(self_ty);
    let method_ident = format_ident!("{}", method_str);

    let rsp_type_str = extract_response_type(item_impl).unwrap_or_else(|| "()".to_string());

    // Text priority: `#[display]` / `#[description]` first, then the impl
    // block's `///` doc comment. The doc's first line is the name, the full
    // comment the description.
    let doc = extract_doc_comments(&item_impl.attrs);
    let name = localized_name(&item_impl.attrs, &doc, &req_type_name)?;
    let description = localized_description(&item_impl.attrs, &doc)?;
    let category = localized_category(&item_impl.attrs)?;
    let (authorization, authorization_dynamic) = extract_authorization(&item_impl.attrs)?;

    // Parse response type back to a Type for use in generated code
    let rsp_type: syn::Type =
        syn::parse_str(&rsp_type_str).unwrap_or_else(|_| syn::parse_str("()").unwrap());

    // Parameter metadata for OpenAPI. Path parameters come from the route
    // template; every other source is declared on the request struct by
    // `#[derive(WebxRequestMeta)]`, so the two can never disagree.
    let params_tokens: Vec<proc_macro2::TokenStream> = extract_path_params(path_str)
        .into_iter()
        .map(|name| {
            quote! {
                ::webx::ParamMeta {
                    name: #name,
                    source: "path",
                    type_hint: "string",
                    required: true,
                    json_key: ::core::option::Option::None,
                }
            }
        })
        .collect();

    // Generate dispatch function
    let is_raw_response = is_raw_response_type(&rsp_type_str);
    let dispatch_fn =
        generate_dispatch_fn(self_ty, &rsp_type, rsp_type_str == "()", is_raw_response);

    let dispatch_fn_name = format_ident!("__lrwf_dispatch_{}", req_type_name.replace("::", "_"));

    // `#[authorize(dynamic)]` flips the endpoint to "static gate = authenticated
    // only, rules delegated to IDynamicAuthorizer". Emitted only for dynamic
    // endpoints, so the metadata of every other route is byte-for-byte
    // unchanged.
    let route_entry = quote! {
        ::webx::RouteEntry::with_type_id(
            ::webx::HttpMethod::#method_ident,
            #path_str,
            ::std::any::TypeId::of::<#self_ty>(),
            #req_type_name,
            #rsp_type_str,
            #name,
            #description,
            #category,
            &[#(#params_tokens),*],
            #authorization,
        )
    };
    let route_entry = if authorization_dynamic {
        quote! { #route_entry.with_dynamic_authorization() }
    } else {
        route_entry
    };

    let expanded = quote! {
        #item_impl

        #dispatch_fn

        ::webx::inventory::submit! {
            ::webx::RouteDispatch {
                handler_type: #req_type_name,
                dispatch: #dispatch_fn_name,
            }
        }

        ::webx::inventory::submit! {
            #route_entry
        }
    };

    Ok(expanded.into())
}

/// Generate the dispatch function that runs the full request lifecycle.
///
/// Binding is delegated to `webx::RequestModel`, which
/// `#[derive(WebxRequestMeta)]` implements from the fields' `#[from*]`
/// markers. One declaration therefore drives binding, validation errors and
/// OpenAPI metadata, and a request can mix path, query, header, claim, form and
/// body values without the method-based heuristics the dispatch used to apply.
///
/// When the response type is `Response` (`Box<dyn IActionResult>`) the handler's value is used as the
/// response verbatim, which is how an endpoint streams a file or sets custom
/// headers. Otherwise the value is serialized as JSON.
fn generate_dispatch_fn(
    ty: &Type,
    rsp_type: &syn::Type,
    is_unit_response: bool,
    is_raw_response: bool,
) -> proc_macro2::TokenStream {
    let fn_name = format_ident!("__lrwf_dispatch_{}", type_to_string(ty).replace("::", "_"));

    let response_expr = if is_raw_response {
        quote! { ::std::result::Result::Ok(result) }
    } else if is_unit_response {
        quote! {
            ::std::result::Result::Ok(
                ::std::boxed::Box::new(::webx::NoContentResult) as ::webx::Response
            )
        }
    } else {
        quote! {
            ::std::result::Result::Ok(
                ::std::boxed::Box::new(::webx::JsonResult::new(result)) as ::webx::Response
            )
        }
    };

    quote! {
        fn #fn_name<'__webx_dispatch>(
            ctx: &'__webx_dispatch mut dyn ::webx::IHttpContext,
        ) -> ::std::pin::Pin<
            Box<
                dyn ::std::future::Future<
                        Output = ::webx::Result<::webx::Response>,
                    > + Send
                    + '__webx_dispatch,
            >,
        > {
            Box::pin(async move {
                // One identity for the whole request: the binder, the handler and
                // anything reading RequestContext (audit helpers, persistence
                // callbacks) share this Arc instead of re-reading claims.
                let identity = ctx.claims().map(|claims| {
                    ::std::sync::Arc::new(::webx::IdentityUser::from_claims(claims))
                });
                // Same for the trace id: the tracing middleware stored it on the
                // context before the endpoint ran, so the handler reads the very
                // value that is written to the `x-trace-id` response header.
                let trace_id = ctx
                    .trace_id()
                    .map(|id| ::std::sync::Arc::<str>::from(id));

                ::webx::RequestContext::run_with_trace(identity, trace_id, async move {
                    // One declaration on the request struct decides every field's
                    // source; failures come back as a 400 with `errors` per field.
                    let request: #ty =
                        <#ty as ::webx::RequestModel>::bind(&mut *ctx).await?;

                    // HTTP adapter: construct request, then dispatch through the
                    // host-provided DI handles (same path as in-process calls). No ambient
                    // provider: the request context owns the handle.
                    let (provider, handler_cache) = {
                        let runtime = ctx.dispatch_runtime().ok_or_else(|| {
                            ::webx::Error::Internal(
                                "HTTP dispatch requires a request context built by webx::Host".to_string(),
                            )
                        })?;
                        (
                            ::std::sync::Arc::clone(runtime.provider()),
                            ::std::sync::Arc::clone(runtime.handler_cache()),
                        )
                    };
                    let mediator = ::webx::Mediator::new(provider, handler_cache);
                    let result: #rsp_type = mediator.send(request).await?;
                    #response_expr
                })
                .await
            })
        }
    }
}

/// Whether the declared response type is the response-control type.
///
/// Matches the [`Response`](webx::Response) alias (`Box<dyn IActionResult>`) and
/// any spelling of that type, so `Response`, `Box<dyn IActionResult>` and a
/// qualified path all trigger the raw path — the handler's result already is
/// the response, so the macro must not serialize it.
///
/// Keep in sync with `is_response_control_type` in `rust-webx-openapi`, which
/// makes the same decision for the OpenAPI response schema.
fn is_raw_response_type(rsp_type: &str) -> bool {
    let last = rsp_type.rsplit("::").next().unwrap_or(rsp_type);
    last.trim() == "Response" || rsp_type.contains("IActionResult")
}

/// Extract the response type from `impl IRequest<UserModel> for GetUserRequest`.
///
/// ItemImpl::trait_ is `Option<(T![!], Path, T![for])>`.
/// We reach the Path (index 1) and examine its first generic argument.
fn extract_response_type(item_impl: &ItemImpl) -> Option<String> {
    let (_bang, path, _for_token) = item_impl.trait_.as_ref()?;
    let last_seg = path.segments.last()?;
    match &last_seg.arguments {
        PathArguments::AngleBracketed(args) => {
            let gen_arg = args.args.first()?;
            match gen_arg {
                GenericArgument::Type(ty) => Some(type_to_string(ty)),
                _ => None,
            }
        }
        _ => None,
    }
}

fn type_to_string(ty: &Type) -> String {
    match ty {
        Type::Path(tp) => {
            let mut result = String::new();
            let segments: Vec<String> = tp
                .path
                .segments
                .iter()
                .map(|s| s.ident.to_string())
                .collect();
            result.push_str(&segments.join("::"));

            // Handle generic type arguments (e.g., Vec<UserModel>)
            if let Some(last_seg) = tp.path.segments.last() {
                if let PathArguments::AngleBracketed(args) = &last_seg.arguments {
                    let gen_args: Vec<String> = args
                        .args
                        .iter()
                        .filter_map(|a| match a {
                            GenericArgument::Type(t) => Some(type_to_string(t)),
                            _ => None,
                        })
                        .collect();
                    if !gen_args.is_empty() {
                        result.push('<');
                        result.push_str(&gen_args.join(", "));
                        result.push('>');
                    }
                }
            }

            result
        }
        Type::Tuple(t) if t.elems.is_empty() => "()".to_string(),
        Type::Reference(r) => {
            format!("&{}", type_to_string(&r.elem))
        }
        _ => format!("{}", quote! { #ty }),
    }
}

fn map_method_name(attr_name: &str) -> &str {
    match attr_name {
        "HttpGet" | "http_get" => "Get",
        "HttpPost" | "http_post" => "Post",
        "HttpPut" | "http_put" => "Put",
        "HttpDelete" | "http_delete" => "Delete",
        "HttpPatch" | "http_patch" => "Patch",
        other => other,
    }
}

fn parse_endpoint_attr(attr: &str) -> (String, String) {
    let s = attr.trim();
    if let Some(idx) = s.find(',') {
        let raw_method = s[..idx].trim();
        let method = map_method_name(raw_method).to_string();
        let path = s[idx + 1..]
            .trim()
            .trim_matches('"')
            .trim_matches('\'')
            .to_string();
        (method, path)
    } else {
        ("Get".to_string(), "/".to_string())
    }
}

fn extract_type_name(ty: &Type) -> String {
    match ty {
        Type::Path(type_path) => type_path
            .path
            .segments
            .iter()
            .map(|seg| seg.ident.to_string())
            .collect::<Vec<_>>()
            .join("::"),
        _ => "UnknownType".to_string(),
    }
}

/// Extract path parameter names from a route pattern like "/users/{id}".
///
/// Wildcard segments `{*rest}` yield the field name `rest` (asterisk stripped),
/// matching matchit's param key and the docs convention.
fn extract_path_params(path: &str) -> Vec<String> {
    let mut params = Vec::new();
    let bytes = path.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'{' {
            let start = i + 1;
            i += 1;
            while i < bytes.len() && bytes[i] != b'}' {
                i += 1;
            }
            if i < bytes.len() {
                let name = path[start..i].trim_start_matches('*');
                if !name.is_empty() {
                    params.push(name.to_string());
                }
            }
        }
        i = i.saturating_add(1);
    }
    params
}

/// Generate a fallback name when the endpoint has neither `#[display]` nor a
/// doc comment. "GetUserRequest" → "Get user", "ListUsersRequest" → "List users".
fn generate_name(type_name: &str) -> String {
    let name = type_name.strip_suffix("Request").unwrap_or(type_name);
    let mut result = String::new();
    for c in name.chars() {
        if c.is_ascii_uppercase() && !result.is_empty() {
            result.push(' ');
        }
        result.push(c.to_ascii_lowercase());
    }
    result
}

/// One `#[display]` / `#[description]` declaration: a resource key and/or a
/// literal text.
#[derive(Default)]
struct TextAttr {
    key: String,
    text: String,
}

impl Parse for TextAttr {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        if input.is_empty() {
            return Err(syn::Error::new(
                proc_macro2::Span::call_site(),
                "expected a string, `key = \"...\"`, or `text = \"...\"`",
            ));
        }

        let mut value = Self::default();

        // Positional form: `#[display("literal")]`.
        if input.peek(LitStr) {
            let text: LitStr = input.parse()?;
            value.text = text.value();
            if input.peek(Token![,]) {
                input.parse::<Token![,]>()?;
            }
        }

        // Named form: `key = "..."` / `text = "..."`.
        while !input.is_empty() {
            let name: syn::Ident = input.parse()?;
            input.parse::<Token![=]>()?;
            let lit: LitStr = input.parse()?;
            match name.to_string().as_str() {
                "key" => {
                    if !value.key.is_empty() {
                        return Err(syn::Error::new(name.span(), "duplicate `key`"));
                    }
                    value.key = lit.value();
                }
                "text" => {
                    if !value.text.is_empty() {
                        return Err(syn::Error::new(name.span(), "duplicate `text`"));
                    }
                    value.text = lit.value();
                }
                other => {
                    return Err(syn::Error::new(
                        name.span(),
                        format!("unknown key `{other}`; expected `key` or `text`"),
                    ))
                }
            }
            if input.peek(Token![,]) {
                input.parse::<Token![,]>()?;
            }
        }

        Ok(value)
    }
}

/// Parse the single `#[display]` / `#[description]` declaration on the impl.
///
/// Localized text is a resource lookup, not a set of per-language attributes:
/// declare the key once and keep translations in resource files.
fn text_attr(attrs: &[Attribute], names: &[&str]) -> syn::Result<Option<TextAttr>> {
    let attr_name = names.first().copied().unwrap_or("display");
    let mut found = None;
    for attr in attrs {
        let matches = attr
            .path()
            .segments
            .last()
            .map(|segment| names.iter().any(|name| segment.ident == *name))
            .unwrap_or(false);
        if !matches {
            continue;
        }
        if found.is_some() {
            return Err(syn::Error::new_spanned(
                attr,
                format!(
                    "declare `#[{attr_name}]` at most once; use `key` + `text`, \
                     not one attribute per language"
                ),
            ));
        }
        found = Some(match &attr.meta {
            Meta::List(list) => syn::parse2::<TextAttr>(list.tokens.clone())?,
            Meta::NameValue(meta) => match &meta.value {
                Expr::Lit(expr) => match &expr.lit {
                    Lit::Str(text) => TextAttr {
                        key: String::new(),
                        text: text.value(),
                    },
                    _ => {
                        return Err(syn::Error::new_spanned(
                            expr,
                            format!("`#[{attr_name}]` expects a string or `key = \"...\"`"),
                        ))
                    }
                },
                _ => {
                    return Err(syn::Error::new_spanned(
                        meta,
                        format!("`#[{attr_name}]` expects a string or `key = \"...\"`"),
                    ))
                }
            },
            Meta::Path(path) => {
                return Err(syn::Error::new_spanned(
                    path,
                    format!("`#[{attr_name}]` expects a string or `key = \"...\"`"),
                ))
            }
        });
    }
    Ok(found)
}

/// Render a `TextRef` literal for the generated code.
fn text_ref_tokens(key: String, text: String) -> proc_macro2::TokenStream {
    quote! {
        ::webx::TextRef {
            key: #key,
            text: #text,
        }
    }
}

fn localized_name(
    attrs: &[Attribute],
    doc: &str,
    req_type_name: &str,
) -> syn::Result<proc_macro2::TokenStream> {
    let (key, text) = match text_attr(attrs, &["display", "__webx_display"])? {
        // `#[display]` was declared: its `text` is the only literal fallback.
        Some(attr) => (attr.key, attr.text),
        None => (
            String::new(),
            doc_first_line(doc).unwrap_or_else(|| generate_name(req_type_name)),
        ),
    };
    Ok(text_ref_tokens(key, text))
}

fn localized_description(attrs: &[Attribute], doc: &str) -> syn::Result<proc_macro2::TokenStream> {
    let (key, text) = match text_attr(attrs, &["description", "__webx_description"])? {
        Some(attr) => (attr.key, attr.text),
        None => (String::new(), doc.to_string()),
    };
    Ok(text_ref_tokens(key, text))
}

fn localized_category(attrs: &[Attribute]) -> syn::Result<proc_macro2::TokenStream> {
    let (key, text) = match text_attr(attrs, &["category", "__webx_category"])? {
        Some(attr) => (attr.key, attr.text),
        None => (String::new(), String::new()),
    };
    Ok(text_ref_tokens(key, text))
}

/// The first non-empty line of the doc comment — the endpoint name fallback.
fn doc_first_line(doc: &str) -> Option<String> {
    doc.lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(str::to_string)
}

// ---------------------------------------------------------------------------
// Authorization rules
// ---------------------------------------------------------------------------

#[derive(Default)]
struct AuthorizeArgs {
    roles: Vec<String>,
    permissions: Vec<String>,
    /// `#[authorize(dynamic)]` — delegate the decision to `IDynamicAuthorizer`.
    /// The flag is endpoint-level: one dynamic declaration puts the whole
    /// endpoint in dynamic mode even when other `#[authorize]` attributes are
    /// mixed in.
    dynamic: bool,
}

impl Parse for AuthorizeArgs {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let mut args = Self::default();
        while !input.is_empty() {
            let key: syn::Ident = input.parse()?;
            if key == "dynamic" {
                if input.peek(Token![=]) {
                    return Err(syn::Error::new(
                        key.span(),
                        "`dynamic` is a flag, not a key; write `#[authorize(dynamic)]`",
                    ));
                }
                if args.dynamic {
                    return Err(syn::Error::new(key.span(), "duplicate `dynamic`"));
                }
                args.dynamic = true;
            } else {
                input.parse::<Token![=]>()?;
                match key.to_string().as_str() {
                    "role" | "roles" => parse_string_list(input, &mut args.roles)?,
                    "permission" | "permissions" => {
                        parse_string_list(input, &mut args.permissions)?
                    }
                    other => {
                        return Err(syn::Error::new(
                            key.span(),
                            format!(
                                "unknown `#[authorize]` key `{other}`; expected `dynamic`, \
                                 role(s) or permission(s)"
                            ),
                        ))
                    }
                }
            }
            if input.peek(Token![,]) {
                input.parse::<Token![,]>()?;
            }
        }
        Ok(args)
    }
}

/// A quoted string or an array of quoted strings.
fn parse_string_list(input: ParseStream, out: &mut Vec<String>) -> syn::Result<()> {
    if input.peek(syn::token::Bracket) {
        let content;
        syn::bracketed!(content in input);
        let values: Punctuated<LitStr, Token![,]> = Punctuated::parse_terminated(&content)?;
        out.extend(values.iter().map(LitStr::value));
    } else {
        let value: LitStr = input.parse()?;
        out.push(value.value());
    }
    Ok(())
}

/// Render the `&[AuthorizeRule]` literal for the generated code, plus whether
/// any `#[authorize]` declaration carried the `dynamic` flag.
///
/// Every attribute still contributes exactly one rule — `#[authorize(dynamic)]`
/// on its own contributes the "any authenticated user" rule, which is what the
/// dynamic mode's static gate enforces anyway. `dynamic` only changes *who*
/// evaluates the rules (the registered `IDynamicAuthorizer` instead of the
/// claim snapshot), never the declaration itself.
fn extract_authorization(
    attrs: &[Attribute],
) -> syn::Result<(proc_macro2::TokenStream, bool)> {
    let mut rules = Vec::new();
    let mut dynamic = false;
    for attr in attrs {
        if !attr.path().is_ident("authorize") {
            continue;
        }
        let parsed = match &attr.meta {
            // Bare `#[authorize]`: any authenticated principal.
            Meta::Path(_) => AuthorizeArgs::default(),
            Meta::List(list) => syn::parse2::<AuthorizeArgs>(list.tokens.clone())?,
            Meta::NameValue(meta) => {
                return Err(syn::Error::new_spanned(
                    meta,
                    "use `#[authorize]`, `#[authorize(dynamic)]` or \
                     `#[authorize(role = \"...\", permission = \"...\")]`",
                ))
            }
        };
        dynamic |= parsed.dynamic;
        rules.push(parsed);
    }

    let rendered = rules.iter().map(|rule| {
        let roles = &rule.roles;
        let permissions = &rule.permissions;
        quote! {
            ::webx::AuthorizeRule {
                roles: &[#(#roles),*],
                permissions: &[#(#permissions),*],
            }
        }
    });

    Ok((quote! { &[#(#rendered),*] }, dynamic))
}

/// Extract `///` doc comments from a list of attributes.
///
/// Rust converts each `///` line into a separate `#[doc = "..."]` attribute.
/// This function collects all such attributes, trims whitespace from each line,
/// and joins them with newlines so the first line can serve as the endpoint name.
///
/// Returns the joined text, or an empty string if no doc comments exist.
fn extract_doc_comments(attrs: &[Attribute]) -> String {
    let lines: Vec<String> = attrs
        .iter()
        .filter(|attr| attr.path().is_ident("doc"))
        .filter_map(|attr| match &attr.meta {
            Meta::NameValue(nv) => {
                if let Expr::Lit(el) = &nv.value {
                    if let Lit::Str(ls) = &el.lit {
                        return Some(ls.value().trim().to_string());
                    }
                }
                None
            }
            _ => None,
        })
        .collect();

    lines.join("\n")
}
