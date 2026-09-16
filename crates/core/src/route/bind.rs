//! Binding a request struct from a `multipart/form-data` body.

use crate::error::{Error, Result};
use crate::form::{ActiveForm, FormDeserializer, MultipartForm};
use crate::http::IHttpContext;
use serde::de::DeserializeOwned;
use std::sync::Arc;

/// Bind `T` from a `multipart/form-data` body.
///
/// Returns `Ok(None)` when the request does not declare
/// `Content-Type: multipart/form-data`, so the caller can fall back to JSON or
/// parameter binding.
///
/// Route parameters are merged into the form as text values, so
/// `POST /api/users/{id}/avatar` binds `{id}` alongside the uploaded file.
/// Binding failures are reported as [`Error::Validation`] (HTTP 400) with the
/// offending field named.
pub async fn bind_form_request<T>(ctx: &mut dyn IHttpContext) -> Result<Option<T>>
where
    T: DeserializeOwned + Send + 'static,
{
    if !ctx.request().is_multipart() {
        return Ok(None);
    }

    let route_params = ctx.request().route_params().clone();
    let form: Arc<MultipartForm> = ctx.request_mut().multipart().await?;

    let bound = ActiveForm::run(Arc::clone(&form), || {
        T::deserialize(FormDeserializer::new(&form, &route_params))
    })
    .map_err(|err| Error::Validation(format!("invalid multipart form data: {err}")))?;

    Ok(Some(bound))
}
