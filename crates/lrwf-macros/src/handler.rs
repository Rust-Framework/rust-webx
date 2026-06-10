use proc_macro::TokenStream;
use quote::{format_ident, quote};
use syn::{parse_macro_input, GenericArgument, ItemImpl, PathArguments, Type};

/// `#[handler]` proc macro attribute — placed on `impl IRequestHandler<T, R> for Handler` blocks.
///
/// Generates compile-time inventory registration so the handler is auto-registered
/// into the DI container without any manual `register_handlers!` or `.singleton()` calls.
///
/// The handler struct MUST implement `Default`.
pub fn handler_impl(item: TokenStream) -> TokenStream {
    let item_impl = parse_macro_input!(item as ItemImpl);
    let handler_ty = &item_impl.self_ty;

    // Extract T (request type) and R (response type) from IRequestHandler<T, R>
    let (req_ty_opt, rsp_ty_opt) = extract_handler_types(&item_impl);

    let handler_ty_name = extract_type_name(handler_ty);
    let fn_name = format_ident!("__lrwf_regfn_{}", handler_ty_name.replace("::", "_"));
    let _static_name = format_ident!("__LRWF_HANDLER_{}", handler_ty_name.replace("::", "_"));

    let default_type = syn::parse_str::<Type>("()").unwrap();
    let req_ty = req_ty_opt.unwrap_or(&default_type);
    let rsp_ty = rsp_ty_opt.unwrap_or(&default_type);

    let expanded = quote! {
        #item_impl

        #[doc(hidden)]
        fn #fn_name(svc: ::lrdi::ServiceCollection) -> ::lrdi::ServiceCollection {
            svc.singleton::<dyn ::lrwf::IRequestHandler<#req_ty, #rsp_ty>>(
                |_| ::std::sync::Arc::new(<#handler_ty>::default()),
            )
        }

        ::inventory::submit! {
            ::lrwf::HandlerRegistration {
                register: #fn_name as fn(::lrdi::ServiceCollection) -> ::lrdi::ServiceCollection,
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
