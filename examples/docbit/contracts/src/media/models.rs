//! Media read model — the stored asset as the client references it.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MediaAsset {
    /// Public URL to reference from content, e.g. `/api/media/images/ab/ab12….png`.
    pub url: String,
    /// Sanitised original file name.
    pub name: String,
    pub content_type: String,
    pub size: u64,
    /// Markdown ready to paste: an image tag for images, a link otherwise.
    pub markdown: String,
}
