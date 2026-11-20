use proc_macro::TokenStream;
use quote::{format_ident, quote};
use syn::{parse_macro_input, GenericArgument, ItemImpl, PathArguments, Type};

/// `#[handler]` proc macro attribute — placed on `impl IRequestHandler<T, R> for Handler` blocks.
///
/// Generates compile-time inventory registration with a type-erased call bridge
/// so the handler can be dispatched without `#[async_trait]` overhead.
///
/// # DI injection
///
/// Use `#[handler(inject)]` when the handler struct has `#[derive(Inject)]`:
///
/// ```ignore
/// #[derive(Inject)]
/// pub struct MyHandler { ctx: DbContext }
///
/// #[handler(inject)]
/// #[async_trait]
/// impl IRequestHandler<MyReq, MyRsp> for MyHandler {
///     async fn handle(&mut self, req: MyReq) -> ActionResult<MyRsp> { ... }
/// }
/// ```
///
/// The factory calls `__rdi_construct_<Handler>(resolver)` per request, which
/// resolves bare owned fields (e.g. `ctx: DbContext`) via `resolver.get_owned`.
/// The resulting `Arc<Handler>` is `try_unwrap`-ed to obtain an owned `Handler`,
/// enabling `handle(&mut self, ...)` without `Arc<Mutex>`.
pub fn handler_impl(attr: TokenStream, item: TokenStream) -> TokenStream {
    let item_impl = parse_macro_input!(item as ItemImpl);
    let handler_ty = &item_impl.self_ty;

    // Event handlers are resolved through DI fan-out (`IMediator::publish`);
    // they do not have an HTTP request dispatch. Do not emit a bogus `()`
    // request registration for them.
    let is_request_handler = item_impl
        .trait_
        .as_ref()
        .and_then(|(_, path, _)| path.segments.last())
        .is_some_and(|segment| segment.ident == "IRequestHandler");
    if !is_request_handler {
        return quote!(#item_impl).into();
    }

    // Check for `#[handler(inject)]`.
    let use_inject = match parse_mode(attr) {
        Ok(use_inject) => use_inject,
        Err(error) => return error.to_compile_error().into(),
    };

    // Extract T (request type) and R (response type) from IRequestHandler<T, R>
    let (req_ty_opt, rsp_ty_opt) = extract_handler_types(&item_impl);

    let handler_ty_name = extract_type_name(handler_ty);

    let default_type = syn::parse_str::<Type>("()").unwrap();
    let req_ty = req_ty_opt.unwrap_or(&default_type);
    let rsp_ty = rsp_ty_opt.unwrap_or(&default_type);

    let req_ty_name = type_to_string(req_ty);
    let rsp_tokens = quote!(#rsp_ty).to_string();
    let is_action_response = rsp_tokens.contains("IActionResult")
        || matches!(
            rsp_ty,
            Type::Path(path) if path.path.segments.last().is_some_and(|segment| segment.ident == "Response")
        );
    let is_unit_response = rsp_tokens.trim() == "()";

    // The response encoder is provided by the route macro. Keeping it out of
    // this bridge is what lets route-less mediator handlers return a
    // non-`Serialize` value without an opt-in attribute, while a real HTTP
    // route still fails to compile if its declared response is not encodable.
    let response_expr = if is_action_response {
        quote! { ::core::result::Result::Ok(::webx::EndpointResult::Action(__result)) }
    } else if is_unit_response {
        quote! { ::core::result::Result::Ok(::webx::EndpointResult::Empty) }
    } else {
        quote! { (encoder)(&__result) }
    };

    let construct_handler = if use_inject {
        let try_constructor_fn =
            format_ident!("__rdi_try_construct_{}", handler_ty_name.replace("::", "_"));
        quote! {
            let __runtime = ctx.dispatch_runtime().ok_or_else(|| {
                ::webx::Error::Internal(
                    "direct dispatch requires a request context built by webx::Host".to_string()
                )
            })?;
            let __scope = __runtime.scope();
            let __arc = match #try_constructor_fn(&__scope) {
                ::core::result::Result::Ok(arc) => arc,
                ::core::result::Result::Err(error) => {
                    return ::core::result::Result::Err(::webx::Error::Di(
                        format!("cannot construct handler {}: {error}", #handler_ty_name)
                    ));
                }
            };
            let mut __handler: #handler_ty = match ::std::sync::Arc::try_unwrap(__arc) {
                ::core::result::Result::Ok(handler) => handler,
                ::core::result::Result::Err(_) => {
                    return ::core::result::Result::Err(::webx::Error::Internal(
                        format!(
                            "handler Arc must be uniquely owned after fresh construction: {}",
                            #handler_ty_name
                        )
                    ));
                }
            };
        }
    } else {
        quote! {
            let mut __handler: #handler_ty = ::std::default::Default::default();
        }
    };

    // Generate factory function
    let factory_fn = format_ident!("__lrwf_factory_{}", handler_ty_name.replace("::", "_"));
    // Generate call bridge function
    let call_fn = format_ident!("__lrwf_call_{}", handler_ty_name.replace("::", "_"));
    // Generate type-erased dispatch used by `IMediator::send_boxed`
    let boxed_fn = format_ident!(
        "__lrwf_boxed_dispatch_{}",
        handler_ty_name.replace("::", "_")
    );
    let direct_fn = format_ident!(
        "__lrwf_direct_dispatch_{}",
        handler_ty_name.replace("::", "_")
    );

    // Choose factory body: DI injection vs Default
    //
    // `__rdi_construct_<Handler>(resolver)` returns `Arc<Handler>` (per rust-dix 0.6).
    // The Arc is freshly created inside the constructor, so refcount == 1 and
    // `Arc::try_unwrap` succeeds — yielding an owned `Handler` that supports
    // `handle(&mut self, ...)`.
    let factory_body = if use_inject {
        let try_constructor_fn =
            format_ident!("__rdi_try_construct_{}", handler_ty_name.replace("::", "_"));
        quote! {
            // Typed construction: a missing/unresolvable dependency must be a
            // normal handler error, not a panic inside the connection task.
            let arc: ::std::sync::Arc<#handler_ty> = match #try_constructor_fn(resolver) {
                ::std::result::Result::Ok(arc) => arc,
                ::std::result::Result::Err(error) => {
                    return ::std::result::Result::Err(::webx::Error::Di(
                        format!("cannot construct handler {}: {error}", #handler_ty_name)
                    ));
                }
            };
            match ::std::sync::Arc::try_unwrap(arc) {
                Ok(owned) => Ok(::std::boxed::Box::new(owned) as ::std::boxed::Box<dyn ::std::any::Any + Send>),
                Err(_) => Err(::webx::Error::Internal(
                    format!("handler Arc must be uniquely owned after fresh construction: {}", #handler_ty_name)
                )),
            }
        }
    } else {
        quote! {
            Ok(::std::boxed::Box::new(<#handler_ty as ::std::default::Default>::default())
                as ::std::boxed::Box<dyn ::std::any::Any + Send>)
        }
    };

    let direct_registration = quote! {
        #[doc(hidden)]
        fn #direct_fn<'__webx_direct>(
            ctx: &'__webx_direct mut dyn ::webx::IHttpContext,
            encoder: ::webx::DirectResponseEncoderFn,
        ) -> ::std::pin::Pin<
            Box<
                dyn ::std::future::Future<
                        Output = ::webx::Result<::webx::EndpointResult>,
                    > + Send
                    + '__webx_direct,
            >,
        > {
            Box::pin(async move {
                let __identity = ctx.claims().map(|claims| {
                    ::std::sync::Arc::new(::webx::IdentityUser::from_claims(claims))
                });
                let __trace = ctx.trace_id().map(|id| ::std::sync::Arc::<str>::from(id));
                ::webx::RequestContext::run_with_trace(__identity, __trace, async move {
                    let __request: #req_ty =
                        ::webx::bind_request::<#req_ty>(&mut *ctx).await?;
                    #construct_handler
                    let __result: #rsp_ty = <#handler_ty as ::webx::IRequestHandler<
                        #req_ty,
                        #rsp_ty,
                    >>::handle(&mut __handler, __request)
                    .await?;
                    let _ = &encoder;
                    #response_expr
                })
                .await
            })
        }

        ::webx::inventory::submit! {
            ::webx::HandlerDispatch {
                req_type_id: ::std::any::TypeId::of::<#req_ty>(),
                rsp_type_id: ::std::any::TypeId::of::<#rsp_ty>(),
                req_type_name: #req_ty_name,
                needs_scope: #use_inject,
                dispatch: #direct_fn,
            }
        }
    };

    let expanded = quote! {
        #item_impl

        #direct_registration

        #[doc(hidden)]
        fn #factory_fn(
            resolver: &dyn ::webx::rust_dix::IServiceResolver,
        ) -> ::webx::Result<::std::boxed::Box<dyn ::std::any::Any + Send>> {
            #factory_body
        }

        #[doc(hidden)]
        fn #call_fn(
            handler: ::std::boxed::Box<dyn ::std::any::Any + Send>,
            request: ::std::boxed::Box<dyn ::std::any::Any + Send>,
        ) -> ::std::pin::Pin<Box<dyn ::std::future::Future<Output = ::webx::Result<::std::boxed::Box<dyn ::std::any::Any + Send>>> + Send>> {
            Box::pin(async move {
                let mut h = *handler
                    .downcast::<#handler_ty>()
                    .map_err(|_| ::webx::Error::Internal(
                        format!("Handler downcast failed for: {}", #handler_ty_name)
                    ))?;
                let req = *request
                    .downcast::<#req_ty>()
                    .map_err(|_| ::webx::Error::Internal(
                        format!("Request downcast failed for: {}", #req_ty_name)
                    ))?;
                let result: #rsp_ty = h.handle(req).await?;
                Ok(::std::boxed::Box::new(result) as ::std::boxed::Box<dyn ::std::any::Any + Send>)
            })
        }

        ::webx::inventory::submit! {
            ::webx::HandlerRegistration {
                req_type_id: ::std::any::TypeId::of::<#req_ty>(),
                req_type_name: #req_ty_name,
                factory: #factory_fn,
                call: #call_fn,
            }
        }

        #[doc(hidden)]
        fn #boxed_fn(
            provider: ::std::sync::Arc<::webx::rust_dix::ServiceProvider>,
            handler_cache: ::std::sync::Arc<::webx::HandlerCache>,
            request: ::std::boxed::Box<dyn ::std::any::Any + Send>,
        ) -> ::webx::BoxedDispatchFuture {
            ::std::boxed::Box::pin(async move {
                let req = *request.downcast::<#req_ty>().map_err(|_| {
                    ::webx::Error::Internal(format!(
                        "Request downcast failed for: {}",
                        #req_ty_name
                    ))
                })?;
                let result: #rsp_ty = ::webx::dispatch(&provider, &handler_cache, req).await?;
                Ok(::std::boxed::Box::new(result) as ::std::boxed::Box<dyn ::std::any::Any + Send>)
            })
        }

        ::webx::inventory::submit! {
            ::webx::BoxedDispatch {
                req_type_id: ::std::any::TypeId::of::<#req_ty>(),
                dispatch: #boxed_fn,
            }
        }
    };

    TokenStream::from(expanded)
}

/// Extract `(req_type, rsp_type)` from `impl IRequestHandler<T, R> for Handler`.
fn extract_handler_types(item_impl: &ItemImpl) -> (Option<&Type>, Option<&Type>) {
    let (_, path, _) = match &item_impl.trait_ {
        Some(t) => t,
        None => return (None, None),
    };
    let last_seg = match path.segments.last() {
        Some(s) => s,
        None => return (None, None),
    };
    match &last_seg.arguments {
        PathArguments::AngleBracketed(args) => {
            let req = match args.args.first() {
                Some(GenericArgument::Type(t)) => Some(t),
                _ => None,
            };
            let rsp = match args.args.get(1) {
                Some(GenericArgument::Type(t)) => Some(t),
                _ => None,
            };
            (req, rsp)
        }
        _ => (None, None),
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
            .join("_"),
        _ => "UnknownType".to_string(),
    }
}

fn parse_mode(attr: TokenStream) -> syn::Result<bool> {
    if attr.is_empty() {
        return Ok(false);
    }
    use syn::parse::Parser;
    let flags =
        syn::punctuated::Punctuated::<syn::Path, syn::Token![,]>::parse_terminated.parse(attr)?;
    let mut use_inject = false;
    for flag in flags {
        if flag.is_ident("inject") {
            use_inject = true;
        } else {
            return Err(syn::Error::new_spanned(
                flag,
                "unsupported `#[handler]` option; expected `#[handler]` or `#[handler(inject)]`",
            ));
        }
    }
    Ok(use_inject)
}

fn type_to_string(ty: &Type) -> String {
    match ty {
        Type::Path(tp) => tp
            .path
            .segments
            .iter()
            .map(|s| s.ident.to_string())
            .collect::<Vec<_>>()
            .join("::"),
        _ => format!("{}", quote! { #ty }),
    }
}
