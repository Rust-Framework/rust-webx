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
///
/// # Event handlers
///
/// On `impl IEventHandler<T>`, `#[handler]` constructs with `Default` and
/// `#[handler(inject)]` uses the `#[derive(Inject)]` constructor, exactly like
/// request handlers. Both are auto-discovered; no DI registration is needed.
pub fn handler_impl(attr: TokenStream, item: TokenStream) -> TokenStream {
    let item_impl = parse_macro_input!(item as ItemImpl);
    let handler_ty = &item_impl.self_ty;

    // The attribute is only meaningful on a trait impl. `IRequestHandler` is
    // the HTTP/mediator dispatch bridge; `IEventHandler` handlers are resolved
    // through DI fan-out (`IMediator::publish`) and pass through untouched.
    // Anything else is a mistake that used to be silently accepted.
    let Some((_, trait_path, _)) = &item_impl.trait_ else {
        let error = syn::Error::new_spanned(
            handler_ty,
            "#[handler] must be applied to a trait impl \
             (`impl IRequestHandler<Req, Rsp> for Handler`), not an inherent impl",
        )
        .to_compile_error();
        return quote!(#error #item_impl).into();
    };
    let Some(trait_segment) = trait_path.segments.last() else {
        return syn::Error::new_spanned(
            trait_path,
            "#[handler] requires `impl IRequestHandler<Req, Rsp> for Handler`",
        )
        .to_compile_error()
        .into();
    };
    match trait_segment.ident.to_string().as_str() {
        "IRequestHandler" => {}
        "IEventHandler" => return event_handler_impl(attr, item_impl),
        other => {
            let error = syn::Error::new_spanned(
                trait_path,
                format!(
                    "#[handler] expects `impl IRequestHandler<Req, Rsp> for Handler` \
                     (or `IEventHandler`); found trait `{other}`"
                ),
            )
            .to_compile_error();
            return quote!(#error #item_impl).into();
        }
    }

    // Check for `#[handler(inject)]`.
    let use_inject = match parse_mode(attr) {
        Ok(use_inject) => use_inject,
        Err(error) => return error.to_compile_error().into(),
    };

    // Extract T (request type) and R (response type) from IRequestHandler<T, R>
    let (Some(req_ty), Some(rsp_ty)) = extract_handler_types(&item_impl) else {
        return syn::Error::new_spanned(
            trait_path,
            "`IRequestHandler` needs both types: `IRequestHandler<Req, Rsp>`",
        )
        .to_compile_error()
        .into();
    };

    let handler_ty_name = extract_type_name(handler_ty);

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

/// `#[handler]` on `impl IEventHandler<T> for Handler`.
///
/// Always emits an `EventRegistration` routing entry **and** an
/// `EventHandlerRegistration` that constructs the handler per event, using the
/// same two modes as request handlers:
///
/// - `#[handler]`: `Default::default()`;
/// - `#[handler(inject)]`: the handler's `#[derive(Inject)]` constructor.
///
/// Event handlers are not registered as DI services; state belongs in injected
/// dependencies.
fn event_handler_impl(attr: TokenStream, item_impl: ItemImpl) -> TokenStream {
    let handler_ty = &item_impl.self_ty;
    let Some((_, trait_path, _)) = &item_impl.trait_ else {
        return quote!(#item_impl).into();
    };

    let mode = match parse_event_mode(attr) {
        Ok(mode) => mode,
        Err(error) => return error.to_compile_error().into(),
    };

    let Some(event_ty) = extract_handler_types(&item_impl).0 else {
        return syn::Error::new_spanned(
            trait_path,
            "`IEventHandler` needs its event type: `IEventHandler<Event>`",
        )
        .to_compile_error()
        .into();
    };

    let handler_ty_name = extract_type_name(handler_ty).replace("::", "_");
    let event_ty_name = type_to_string(event_ty).replace("::", "_");
    let dispatch_fn = format_ident!(
        "__webx_event_dispatch_{}_{}",
        handler_ty_name,
        event_ty_name
    );
    let factory_fn = format_ident!("__webx_event_factory_{}_{}", handler_ty_name, event_ty_name);
    let call_fn = format_ident!("__webx_event_call_{}_{}", handler_ty_name, event_ty_name);

    let factory_body = match mode {
        EventMode::Default => quote! {
            ::core::result::Result::Ok(::std::sync::Arc::new(
                <#handler_ty as ::std::default::Default>::default(),
            ))
        },
        EventMode::Inject => {
            let try_constructor_fn =
                format_ident!("__rdi_try_construct_{}", handler_ty_name.replace("::", "_"));
            quote! {
                let arc = match #try_constructor_fn(resolver) {
                    ::core::result::Result::Ok(arc) => arc,
                    ::core::result::Result::Err(error) => {
                        return ::core::result::Result::Err(::webx::Error::Di(format!(
                            "cannot construct event handler {}: {error}",
                            ::std::stringify!(#handler_ty)
                        )));
                    }
                };
                ::core::result::Result::Ok(arc)
            }
        }
    };
    let auto_registration = quote! {
        #[doc(hidden)]
        fn #factory_fn(
            resolver: &dyn ::webx::rust_dix::IServiceResolver,
        ) -> ::webx::Result<
            ::std::sync::Arc<
                dyn ::std::any::Any + ::std::marker::Send + ::std::marker::Sync,
            >,
        > {
            #factory_body
        }

        #[doc(hidden)]
        fn #call_fn(
            handler: ::std::sync::Arc<
                dyn ::std::any::Any + ::std::marker::Send + ::std::marker::Sync,
            >,
            event: ::std::boxed::Box<dyn ::std::any::Any + ::std::marker::Send>,
        ) -> ::webx::BoxedEventHandlerCall {
            ::std::boxed::Box::pin(async move {
                let handler = handler
                    .downcast::<#handler_ty>()
                    .map_err(|_| ::webx::Error::Internal(format!(
                        "event handler type mismatch for {}",
                        ::std::stringify!(#handler_ty)
                    )))?;
                let event = event
                    .downcast::<#event_ty>()
                    .map_err(|_| ::webx::Error::Internal(format!(
                        "event type mismatch for {}",
                        ::std::stringify!(#event_ty)
                    )))?;
                handler.handle(*event).await
            })
        }

        ::webx::inventory::submit! {
            ::webx::EventHandlerRegistration {
                event_name: <#event_ty as ::webx::IEventRequest>::event_name,
                event_type_id: ::std::any::TypeId::of::<#event_ty>(),
                handler_name: ::std::stringify!(#handler_ty),
                factory: #factory_fn,
                call: #call_fn,
            }
        }
    };

    let expanded = quote! {
        #item_impl

        #[doc(hidden)]
        fn #dispatch_fn(
            provider: ::std::sync::Arc<::webx::rust_dix::ServiceProvider>,
            payload: ::std::vec::Vec<u8>,
        ) -> ::webx::BoxedEventDispatch {
            ::std::boxed::Box::pin(async move {
                let event: #event_ty = ::webx::serde_json::from_slice(&payload)
                    .map_err(|error| ::webx::Error::Internal(format!(
                        "event payload for '{}' is not valid JSON: {error}",
                        <#event_ty as ::webx::IEventRequest>::event_name()
                    )))?;
                let scope = <::webx::rust_dix::ServiceProvider as ::webx::rust_dix::ScopeFactory>::create_scope(
                    provider.as_ref(),
                );
                ::webx::dispatch_event_handlers(&scope, event).await
            })
        }

        ::webx::inventory::submit! {
            ::webx::EventRegistration {
                event_name: <#event_ty as ::webx::IEventRequest>::event_name,
                event_type_id: ::std::any::TypeId::of::<#event_ty>(),
                dispatch: #dispatch_fn,
            }
        }

        #auto_registration
    };

    expanded.into()
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

#[derive(Clone, Copy, PartialEq, Eq)]
enum EventMode {
    Default,
    Inject,
}

fn parse_event_mode(attr: TokenStream) -> syn::Result<EventMode> {
    if attr.is_empty() {
        return Ok(EventMode::Default);
    }
    use syn::parse::Parser;
    let flags =
        syn::punctuated::Punctuated::<syn::Path, syn::Token![,]>::parse_terminated.parse(attr)?;
    let mut mode = EventMode::Default;
    for flag in flags {
        if flag.is_ident("inject") {
            mode = EventMode::Inject;
        } else {
            return Err(syn::Error::new_spanned(
                flag,
                "unsupported `#[handler]` event option; expected `#[handler]` or `#[handler(inject)]`",
            ));
        }
    }
    Ok(mode)
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
