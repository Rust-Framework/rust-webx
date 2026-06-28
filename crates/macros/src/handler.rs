use proc_macro::TokenStream;
use quote::{format_ident, quote};
use syn::{parse_macro_input, GenericArgument, ItemImpl, PathArguments, Type};

/// `#[handler]` proc macro attribute â€” placed on `impl IRequestHandler<T, R> for Handler` blocks.
///
/// Generates compile-time inventory registration with a type-erased call bridge
/// so the handler can be dispatched without `#[async_trait]` overhead.
///
/// The handler struct MUST implement `Default`.
///
/// # DI injection
///
/// Use `#[handler(inject)]` when the handler struct has `#[rust_dicore::inject_attr]`:
///
/// ```ignore
/// #[rust_dicore::inject_attr(singleton, as = dyn IRequestHandler<MyReq, MyRsp>)]
/// pub struct MyHandler { ctx: Arc<AppDbContext> }
///
/// #[handler(inject)]
/// #[async_trait]
/// impl IRequestHandler<MyReq, MyRsp> for MyHandler { ... }
/// ```
pub fn handler_impl(attr: TokenStream, item: TokenStream) -> TokenStream {
    let item_impl = parse_macro_input!(item as ItemImpl);
    let handler_ty = &item_impl.self_ty;

    // Check for #[handler(inject)] â€” signals DI-based construction via #[inject_attr]
    let use_inject = !attr.is_empty();

    // Extract T (request type) and R (response type) from IRequestHandler<T, R>
    let (req_ty_opt, rsp_ty_opt) = extract_handler_types(&item_impl);

    let handler_ty_name = extract_type_name(handler_ty);

    let default_type = syn::parse_str::<Type>("()").unwrap();
    let req_ty = req_ty_opt.unwrap_or(&default_type);
    let _rsp_ty = rsp_ty_opt.unwrap_or(&default_type);

    let req_ty_name = type_to_string(req_ty);

    // Generate factory function
    let factory_fn = format_ident!("__lrwf_factory_{}", handler_ty_name.replace("::", "_"));
    // Generate call bridge function
    let call_fn = format_ident!("__lrwf_call_{}", handler_ty_name.replace("::", "_"));

    // Choose factory body: DI injection vs Default
    let factory_body = if use_inject {
        let constructor_fn =
            format_ident!("__rdi_construct_{}", handler_ty_name.replace("::", "_"));
        quote! {
            let provider = ::rust_webapp::global_provider();
            let handler: ::std::sync::Arc<#handler_ty> = #constructor_fn(provider.as_ref() as &dyn rust_dicore::IServiceResolver);
            ::std::sync::Arc::new(handler) as ::std::sync::Arc<dyn ::std::any::Any + Send + Sync>
        }
    } else {
        quote! {
            ::std::sync::Arc::new(::std::sync::Arc::new(<#handler_ty>::default())) as ::std::sync::Arc<dyn ::std::any::Any + Send + Sync>
        }
    };

    let expanded = quote! {
        #item_impl

        #[doc(hidden)]
        fn #factory_fn() -> ::std::sync::Arc<dyn ::std::any::Any + Send + Sync> {
            #factory_body
        }

        #[doc(hidden)]
        fn #call_fn(
            handler: &::std::sync::Arc<dyn ::std::any::Any + Send + Sync>,
            request: Box<dyn ::std::any::Any + Send>,
            _claims: Option<Box<dyn ::rust_webapp::IClaims>>,
        ) -> ::std::pin::Pin<Box<dyn ::std::future::Future<Output = ::rust_webapp::Result<::rust_webapp::ResponseData>> + Send>> {
            let handler = ::std::sync::Arc::clone(handler);
            Box::pin(async move {
                let h = handler
                    .downcast_ref::<::std::sync::Arc<#handler_ty>>()
                    .expect("Handler downcast failed");
                let mut req = *request
                    .downcast::<#req_ty>()
                    .expect("Request downcast failed");
                // Inject claims (no-op if the request has no inherent set_claims),
                // then dispatch via handle.
                {
                    use ::rust_webapp::IClaimsCarrier;
                    req.set_claims(_claims);
                }
                let result = h.handle(req).await?;
                let json_bytes = ::serde_json::to_vec(&result).unwrap_or_default();
                Ok(::rust_webapp::ResponseData {
                    status: 200,
                    content_type: "application/json".to_string(),
                    body: json_bytes,
                })
            })
        }

        ::inventory::submit! {
            ::rust_webapp::HandlerRegistration {
                req_type_name: #req_ty_name,
                factory: #factory_fn as fn() -> ::std::sync::Arc<dyn ::std::any::Any + Send + Sync>,
                call: #call_fn,
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
