use proc_macro::TokenStream;

/// Handles #[HttpGet], #[HttpPost], #[HttpPut], #[HttpDelete] on controller methods.
pub fn http_method_impl(_method: &str, _attr: TokenStream, item: TokenStream) -> TokenStream {
    item
}
