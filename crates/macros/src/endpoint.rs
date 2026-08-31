use proc_macro::TokenStream;
use quote::{format_ident, quote};
use syn::{
    parse_macro_input, Attribute, Expr, GenericArgument, ItemImpl, Lit, Meta, PathArguments, Type,
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
    let self_ty = &item_impl.self_ty;

    let (method_str, path_str) = parse_endpoint_attr(&attr.to_string());
    let req_type_name = extract_type_name(self_ty);
    let method_ident = format_ident!("{}", method_str);

    let rsp_type_str = extract_response_type(&item_impl).unwrap_or_else(|| "()".to_string());
    let summary = generate_summary(&req_type_name);

    // Extract doc comments (/// ...) from the impl block for OpenAPI description
    let description = extract_doc_comments(&item_impl.attrs);

    // Parse response type back to a Type for use in generated code
    let rsp_type: syn::Type =
        syn::parse_str(&rsp_type_str).unwrap_or_else(|_| syn::parse_str("()").unwrap());

    // Extract auth requirements from #[authorize] attributes
    let (required_role, required_permission) = extract_auth_requirements(&item_impl.attrs);

    // Parameter metadata for OpenAPI
    let mut params_tokens: Vec<proc_macro2::TokenStream> = extract_path_params(&path_str)
        .into_iter()
        .map(|name| {
            quote! {
                ::rust_webx::ParamMeta {
                    name: #name,
                    source: "path",
                    type_hint: "string",
                }
            }
        })
        .collect();

    let is_body_method = method_str == "Post" || method_str == "Put" || method_str == "Patch";
    if is_body_method {
        params_tokens.push(quote! {
            ::rust_webx::ParamMeta {
                name: "body",
                source: "body",
                type_hint: "object",
            }
        });
    }

    // Generate FromHttpContext impl for the request type
    let path_params: Vec<String> = extract_path_params(&path_str);

    // Generate dispatch function
    let dispatch_fn = generate_dispatch_fn(
        self_ty,
        &rsp_type,
        &req_type_name,
        &path_params,
        is_body_method,
        rsp_type_str == "()",
    );

    let dispatch_fn_name = format_ident!("__lrwf_dispatch_{}", req_type_name.replace("::", "_"));

    let expanded = quote! {
        #item_impl

        #dispatch_fn

        ::inventory::submit! {
            ::rust_webx::RouteDispatch {
                handler_type: #req_type_name,
                dispatch: #dispatch_fn_name,
            }
        }

        ::inventory::submit! {
            ::rust_webx::RouteEntry::new(
                ::rust_webx::HttpMethod::#method_ident,
                #path_str,
                #req_type_name,
                #rsp_type_str,
                #summary,
                #description,
                &[#(#params_tokens),*],
                #required_role,
                #required_permission,
            )
        }
    };

    TokenStream::from(expanded)
}

/// Generate the dispatch function that runs the full request lifecycle.
///
/// Looks up the `HandlerRegistration` (collected via `#[handler]`) by request
/// type name, calls its factory with a per-request scope to obtain an owned
/// handler (Scoped dependencies like `DbContext` are resolved via `get_owned`),
/// then invokes `handle(&mut self, req)` through the call bridge.
fn generate_dispatch_fn(
    ty: &Type,
    rsp_type: &syn::Type,
    type_name: &str,
    path_params: &[String],
    is_body: bool,
    is_unit_response: bool,
) -> proc_macro2::TokenStream {
    let fn_name = format_ident!("__lrwf_dispatch_{}", type_name.replace("::", "_"));

    // Generate request construction code
    let build_request = if is_body && !path_params.is_empty() {
        let overrides: Vec<proc_macro2::TokenStream> = path_params
            .iter()
            .map(|name| {
                let ident = format_ident!("{}", name);
                let name_str = name.as_str();
                quote! {
                    #ident: route_params.get(#name_str).cloned().unwrap_or(req.#ident),
                }
            })
            .collect();
        quote! {{
            let mut req: #ty = ::serde_json::from_slice(&body_bytes)
                .map_err(|e| ::rust_webx::Error::Serialization(e))?;
            req = #ty { #(#overrides)* ..req };
            req
        }}
    } else if is_body {
        quote! {
            ::serde_json::from_slice(&body_bytes).map_err(|e| ::rust_webx::Error::Serialization(e))?
        }
    } else if !path_params.is_empty() {
        let field_assignments: Vec<proc_macro2::TokenStream> = path_params
            .iter()
            .map(|name| {
                let ident = format_ident!("{}", name);
                let name_str = name.as_str();
                quote! {
                    #ident: route_params.get(#name_str).cloned().unwrap_or_default()
                }
            })
            .collect();
        quote! {
            {
                if let Some(req) = ::rust_webx::try_deserialize_from_params::<#ty>(
                    &route_params,
                    &_query_params,
                ) {
                    req
                } else {
                    #ty { #(#field_assignments,)* ..::std::default::Default::default() }
                }
            }
        }
    } else {
        quote! {
            {
                if let Some(req) = ::rust_webx::try_deserialize_from_params::<#ty>(
                    &route_params,
                    &_query_params,
                ) {
                    req
                } else {
                    <#ty as ::std::default::Default>::default()
                }
            }
        }
    };

    quote! {
        #[allow(clippy::needless_update)]
        fn #fn_name(
            body_bytes: Vec<u8>,
            route_params: ::std::collections::HashMap<String, String>,
            query_params: ::std::collections::HashMap<String, String>,
            claims: Option<Box<dyn ::rust_webx::IClaims>>,
        ) -> ::std::pin::Pin<Box<dyn ::std::future::Future<Output = ::rust_webx::Result<::rust_webx::ResponseData>> + Send>> {
            Box::pin(async move {
                let _query_params = query_params;
                let mut request: #ty = #build_request;

                let operator_id = claims.as_ref().map(|c| c.subject().to_string());

                // Inject claims into the request *before* dispatch (no-op if the
                // request type has no inherent `set_claims`).
                {
                    use ::rust_webx::IClaimsCarrier;
                    request.set_claims(claims);
                }

                ::rust_webx::RequestContext::run(operator_id, async move {
                // HTTP adapter: construct request, then dispatch via IMediator (same path as in-process calls).
                let mediator = ::rust_webx::Mediator::new(::rust_webx::dispatch_provider());
                let result: #rsp_type = mediator.send(request).await?;

                let status = if #is_unit_response { 204 } else { 200 };
                let json_bytes = if #is_unit_response {
                    Vec::new()
                } else {
                    ::serde_json::to_vec(&result)
                        .map_err(|e| ::rust_webx::Error::Internal(format!("response serialization failed: {}", e)))?
                };
                Ok(::rust_webx::ResponseData {
                    status,
                    content_type: "application/json".to_string(),
                    body: json_bytes,
                })
                }).await
            })
        }
    }
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
                let name = &path[start..i];
                params.push(name.to_string());
            }
        }
        i = i.saturating_add(1);
    }
    params
}

/// Generate a human-readable summary from a type name.
/// "GetUserRequest" → "Get user", "ListUsersRequest" → "List users".
fn generate_summary(type_name: &str) -> String {
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

/// Extract auth metadata from `#[authorize]`, `#[authorize(role = "...")]`,
/// or `#[authorize(permission = "...")]`.
fn extract_auth_requirements(attrs: &[Attribute]) -> (proc_macro2::TokenStream, proc_macro2::TokenStream) {
    let mut role = quote! { "" };
    let mut permission = quote! { "" };

    for attr in attrs {
        if !attr.path().is_ident("authorize") {
            continue;
        }
        match &attr.meta {
            Meta::List(list) => {
                let tokens_str = list.tokens.to_string();
                if let Some(role_val) = tokens_str.trim().strip_prefix("role = \"") {
                    if let Some(end) = role_val.find('"') {
                        let value = &role_val[..end];
                        role = quote! { #value };
                    }
                } else if let Some(perm_val) = tokens_str.trim().strip_prefix("permission = \"") {
                    if let Some(end) = perm_val.find('"') {
                        let value = &perm_val[..end];
                        permission = quote! { #value };
                    }
                }
            }
            Meta::Path(_) => role = quote! { "authenticated" },
            _ => {}
        }
    }

    (role, permission)
}

/// Extract `///` doc comments from a list of attributes.
///
/// Rust converts each `///` line into a separate `#[doc = "..."]` attribute.
/// This function collects all such attributes, trims whitespace from each line,
/// and joins multi-line comments with a single space.
///
/// Returns the joined description string, or an empty string if no doc comments exist.
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
