//! `GET /api/media/{*path}` — stream a stored file back to the client.
//!
//! Only the inline image allowlist is served inline; everything else is handed
//! over as a download, so a stored `.svg`/`.html` can never run in this origin.

use webx::*;

use docbit_contracts::media::GetMediaRequest;

use super::{resolve_inside_uploads, INLINE_IMAGE_EXTENSIONS};

#[derive(Inject)]
pub struct GetMediaHandler;

#[handler(inject)]
#[async_trait]
impl IRequestHandler<GetMediaRequest, Response> for GetMediaHandler {
    async fn handle(&mut self, req: GetMediaRequest) -> ActionResult<Response> {
        let path = resolve_inside_uploads(&req.path)
            .ok_or_else(|| Error::validation("invalid media path"))?;

        if !path.is_file() {
            return Err(Error::not_found("Media not found"));
        }

        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();

        let result = FileResult::path(path);
        Ok(Box::new(
            if INLINE_IMAGE_EXTENSIONS.contains(&ext.as_str()) {
                result
            } else {
                result.download_name(name)
            },
        ))
    }
}
