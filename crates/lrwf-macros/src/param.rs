use proc_macro::TokenStream;

/// Handles #[FromBody], #[FromRoute], #[FromQuery] on struct fields and method parameters.
///
/// These attributes are passed through at compile time. At runtime, the
/// parameter binding layer reads these attributes and performs the
/// appropriate deserialization.
///
/// ```ignore
/// struct GetUserRequest {
///     #[FromRoute]
///     id: String,
///     #[FromQuery]
///     format: Option<String>,
/// }
/// ```
pub fn from_attribute_impl(_source: &str, item: TokenStream) -> TokenStream {
    // Pass through unchanged.
    // The attribute serves as metadata for runtime parameter binding.
    item
}
