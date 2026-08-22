//! Shared loading of local image files into a provider-neutral data URL.
//!
//! Both `ViewImage` (hands the image to a vision-capable main model) and
//! `ReadImage` (delegates to a separate vision model and returns text) need the
//! exact same validation, so it lives here instead of being duplicated.

use std::borrow::Cow;
use std::path::Path;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use serde_json::Value;

use dream_engine_types::message::{ImageUrl, extension_to_image_media_type};

pub(crate) const MAX_IMAGE_SIZE_BYTES: u64 = 20 * 1024 * 1024;

/// Strip Windows' extended-length (`\\?\`) path prefix.
///
/// The Dream UI host injects attachment paths in exactly this verbatim form
/// (`\\?\C:\Users\...\image.png`). Verbatim paths also suppress Windows path
/// normalization, so callers get a plain path back and every downstream check
/// (`is_absolute`, extension parsing, error messages) behaves the same as for a
/// hand-typed path.
pub(crate) fn strip_verbatim_prefix(file_path: &str) -> Cow<'_, str> {
    if let Some(unc) = file_path.strip_prefix(r"\\?\UNC\") {
        // `\\?\UNC\server\share` is the verbatim spelling of `\\server\share`;
        // returning the tail alone would silently point at a different host.
        return Cow::Owned(format!(r"\\{unc}"));
    }
    match file_path.strip_prefix(r"\\?\") {
        Some(stripped) => Cow::Borrowed(stripped),
        None => Cow::Borrowed(file_path),
    }
}

/// Read the required `file_path` argument from a tool call payload.
pub(crate) fn image_path_argument(input: &Value) -> Result<Cow<'_, str>, String> {
    input
        .get("file_path")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|path| !path.is_empty())
        .map(strip_verbatim_prefix)
        .ok_or_else(|| "Missing required parameter: file_path".to_owned())
}

fn detect_image_media_type(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("image/png")
    } else if bytes.starts_with(b"\xff\xd8\xff") {
        Some("image/jpeg")
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some("image/gif")
    } else if bytes.len() >= 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WEBP" {
        Some("image/webp")
    } else {
        None
    }
}

/// Load and validate a local image file into a base64 data URL.
pub(crate) async fn load_image_url(file_path: &str) -> Result<ImageUrl, String> {
    let path = Path::new(file_path);
    if !path.is_absolute() {
        return Err("file_path must be an absolute path".to_owned());
    }

    let extension = path
        .extension()
        .and_then(|extension| extension.to_str())
        .ok_or_else(|| "Image path must have a supported extension".to_owned())?;
    let mime_type =
        extension_to_image_media_type(extension).ok_or_else(|| format!("Unsupported image extension: {extension}"))?;

    let metadata = tokio::fs::metadata(path)
        .await
        .map_err(|error| format!("Failed to read image metadata: {error}"))?;
    if !metadata.is_file() {
        return Err("Image path is not a regular file".to_owned());
    }
    if metadata.len() > MAX_IMAGE_SIZE_BYTES {
        return Err(format!("Image exceeds the {MAX_IMAGE_SIZE_BYTES} byte size limit"));
    }

    let bytes = tokio::fs::read(path)
        .await
        .map_err(|error| format!("Failed to read image: {error}"))?;
    if bytes.len() as u64 > MAX_IMAGE_SIZE_BYTES {
        return Err(format!("Image exceeds the {MAX_IMAGE_SIZE_BYTES} byte size limit"));
    }
    let detected_mime_type = detect_image_media_type(&bytes)
        .ok_or_else(|| "File content is not a supported JPEG, PNG, GIF, or WebP image".to_owned())?;
    if detected_mime_type != mime_type {
        return Err(format!(
            "Image content type {detected_mime_type} does not match extension type {mime_type}"
        ));
    }

    let image_url = ImageUrl {
        url: format!("data:{detected_mime_type};base64,{}", STANDARD.encode(bytes)),
    };
    image_url
        .validate()
        .map_err(|error| format!("Failed to prepare image input: {error}"))?;
    Ok(image_url)
}

#[cfg(test)]
#[path = "image_source_test.rs"]
mod image_source_test;
