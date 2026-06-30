use proc_macro::TokenStream;
use quote::quote;

/// Handles #[controller("/base")] on struct definitions or #[controller] on impl blocks.
///
/// For structs: stores the base path as a static associated with the struct.
/// For impl blocks: passes through for runtime method scanning.
///
/// ```ignore
/// #[controller("/api/users")]
/// #[derive(Inject)]
/// struct UserController { mediator: Arc<dyn IMediator> }
/// ```
pub fn controller_impl(attr: TokenStream, item: TokenStream) -> TokenStream {
    let base_path = parse_base_path(&attr.to_string());

    // Try to parse as a struct definition
    if let Ok(item_struct) =
        syn::parse2::<syn::ItemStruct>(proc_macro2::TokenStream::from(item.clone()))
    {
        let struct_name = &item_struct.ident;
        let meta_name = quote::format_ident!("__lrwf_ctrl_meta_{}", struct_name);
        let struct_vis = &item_struct.vis;

        let expanded = quote! {
            #item_struct

            #[doc(hidden)]
            #[allow(non_upper_case_globals)]
            #struct_vis static #meta_name: &str = #base_path;
        };

        return TokenStream::from(expanded);
    }

    // Try to parse as an impl block — pass through unchanged
    if syn::parse2::<syn::ItemImpl>(proc_macro2::TokenStream::from(item.clone())).is_ok() {
        return item;
    }

    // Fallback
    item
}

fn parse_base_path(attr: &str) -> String {
    let s = attr.trim();
    if s.is_empty() {
        String::new()
    } else {
        s.trim_matches('"').trim_matches('\'').to_string()
    }
}
