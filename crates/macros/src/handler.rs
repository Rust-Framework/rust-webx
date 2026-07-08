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
///     async fn handle(&mut self, req: MyReq) -> Result<MyRsp> { ... }
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

    // Check for #[handler(inject)] — signals DI-based construction via #[derive(Inject)]
    let use_inject = !attr.is_empty();

    // Extract T (request type) and R (response type) from IRequestHandler<T, R>
    let (req_ty_opt, rsp_ty_opt) = extract_handler_types(&item_impl);

    let handler_ty_name = extract_type_name(handler_ty);

    let default_type = syn::parse_str::<Type>("()").unwrap();
    let req_ty = req_ty_opt.unwrap_or(&default_type);
    let rsp_ty = rsp_ty_opt.unwrap_or(&default_type);

    let req_ty_name = type_to_string(req_ty);

    // Generate factory function
    let factory_fn = format_ident!("__lrwf_factory_{}", handler_ty_name.replace("::", "_"));
    // Generate call bridge function
    let call_fn = format_ident!("__lrwf_call_{}", handler_ty_name.replace("::", "_"));

    // Choose factory body: DI injection vs Default
    //
    // `__rdi_construct_<Handler>(resolver)` returns `Arc<Handler>` (per rust-dix 0.6).
    // The Arc is freshly created inside the constructor, so refcount == 1 and
    // `Arc::try_unwrap` succeeds — yielding an owned `Handler` that supports
    // `handle(&mut self, ...)`.
    let factory_body = if use_inject {
        let constructor_fn =
            format_ident!("__rdi_construct_{}", handler_ty_name.replace("::", "_"));
        quote! {
            let arc: ::std::sync::Arc<#handler_ty> = #constructor_fn(resolver);
            let owned: #handler_ty = match ::std::sync::Arc::try_unwrap(arc) {
                Ok(o) => o,
                Err(_) => panic!("handler Arc must be uniquely owned after fresh construction"),
            };
            ::std::boxed::Box::new(owned) as ::std::boxed::Box<dyn ::std::any::Any + Send>
        }
    } else {
        quote! {
            ::std::boxed::Box::new(<#handler_ty as ::std::default::Default>::default())
                as ::std::boxed::Box<dyn ::std::any::Any + Send>
        }
    };

    let expanded = quote! {
        #item_impl

        #[doc(hidden)]
        fn #factory_fn(
            resolver: &dyn ::rust_webx::rust_dix::IServiceResolver,
        ) -> ::std::boxed::Box<dyn ::std::any::Any + Send> {
            #factory_body
        }

        #[doc(hidden)]
        fn #call_fn(
            handler: ::std::boxed::Box<dyn ::std::any::Any + Send>,
            request: ::std::boxed::Box<dyn ::std::any::Any + Send>,
        ) -> ::std::pin::Pin<Box<dyn ::std::future::Future<Output = ::rust_webx::Result<::std::boxed::Box<dyn ::std::any::Any + Send>>> + Send>> {
            Box::pin(async move {
                let mut h = *handler
                    .downcast::<#handler_ty>()
                    .expect("Handler downcast failed");
                let req = *request
                    .downcast::<#req_ty>()
                    .expect("Request downcast failed");
                let result: #rsp_ty = h.handle(req).await?;
                Ok(::std::boxed::Box::new(result) as ::std::boxed::Box<dyn ::std::any::Any + Send>)
            })
        }

        ::inventory::submit! {
            ::rust_webx::HandlerRegistration {
                req_type_id: ::std::any::TypeId::of::<#req_ty>(),
                req_type_name: #req_ty_name,
                factory: #factory_fn,
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
