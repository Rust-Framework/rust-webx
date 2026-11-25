//! `POST /api/media` — store one upload under `uploads/`, return URL + markdown.
//!
//! Images and attachments are told apart by `Content-Type`; the stored name is a
//! fresh UUID sharded two hex characters deep, so a client-supplied name can
//! never choose the path.

use std::fs;

use webx::*;

use docbit_contracts::media::{MediaAsset, UploadMediaRequest};
use docbit_domain::new_id;

use super::resolve_inside_uploads;

#[derive(Inject)]
pub struct UploadMediaHandler;

#[handler(inject)]
#[async_trait]
impl IRequestHandler<UploadMediaRequest, MediaAsset> for UploadMediaHandler {
    async fn handle(&mut self, req: UploadMediaRequest) -> ActionResult<MediaAsset> {
        if req.file.is_empty() {
            return Err(Error::validation("no file was uploaded"));
        }

        let content_type = req
            .file
            .content_type()
            .unwrap_or("application/octet-stream")
            .to_string();
        let is_image = content_type.starts_with("image/");
        let bucket = if is_image { "images" } else { "files" };

        let id = new_id();
        // Two hex characters of the id as a shard directory.
        let shard = &id[..2.min(id.len())];
        let name = req.file.file_name().to_string();
        let ext = crate::media::extension_for(&name, &content_type);

        let relative = format!("{}/{}/{}.{}", bucket, shard, id, ext);
        let target = resolve_inside_uploads(&relative)
            .ok_or_else(|| Error::internal("generated media path was rejected"))?;

        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| Error::internal(format!("Cannot create media directory: {}", e)))?;
        }

        req.file
            .save_as(&target)
            .await
            .map_err(|e| Error::internal(format!("Cannot store media file: {}", e)))?;

        let url = format!("/api/media/{}", relative);
        let markdown = if is_image {
            format!("![{}]({})", name, url)
        } else {
            format!("[{}]({})", name, url)
        };

        tracing::info!(
            "[Media] Stored {} ({} bytes) -> {}",
            name,
            req.file.size(),
            url
        );

        Ok(MediaAsset {
            url,
            name,
            content_type,
            size: req.file.size(),
            markdown,
        })
    }
}
