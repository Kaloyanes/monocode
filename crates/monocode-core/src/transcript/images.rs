//! Images an agent produced or published. The bytes live in the conversation's
//! folder; the transcript row records where, plus enough to lay it out before
//! the file is decoded.
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

pub const ROLE: &str = "image";
pub const MAX_BYTES: u64 = crate::composer::MAX_EMBED_BYTES;
const MAX_TITLE_CHARS: usize = 160;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageDocument {
    pub title: String,
    pub path: PathBuf,
    pub mime_type: String,
    pub width: u32,
    pub height: u32,
}

impl ImageDocument {
    pub fn validate(&self) -> Result<(), String> {
        if self.width == 0 || self.height == 0 || !self.path.is_absolute() {
            return Err("Invalid image".into());
        }
        Ok(())
    }
}

pub struct Inspected {
    pub mime_type: &'static str,
    pub extension: &'static str,
    pub width: u32,
    pub height: u32,
}

/// Reads the format and size from the header, rejecting anything that is not
/// a PNG, JPEG, GIF or WebP image within the size limit.
pub fn inspect(bytes: &[u8]) -> Result<Inspected, String> {
    if bytes.len() as u64 > MAX_BYTES {
        return Err("Image is larger than 20 MB".into());
    }
    let (mime_type, extension) = match image::guess_format(bytes) {
        Ok(image::ImageFormat::Png) => ("image/png", "png"),
        Ok(image::ImageFormat::Jpeg) => ("image/jpeg", "jpg"),
        Ok(image::ImageFormat::Gif) => ("image/gif", "gif"),
        Ok(image::ImageFormat::WebP) => ("image/webp", "webp"),
        _ => return Err("Only PNG, JPEG, GIF and WebP images are supported".into()),
    };
    let (width, height) = image::ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|e| e.to_string())?
        .into_dimensions()
        .map_err(|_| "The image could not be read".to_owned())?;
    if width == 0 || height == 0 {
        return Err("The image is empty".into());
    }
    Ok(Inspected {
        mime_type,
        extension,
        width,
        height,
    })
}

pub fn title(title: &str) -> String {
    let title = title.trim();
    if title.is_empty() {
        "Image".into()
    } else {
        title.chars().take(MAX_TITLE_CHARS).collect()
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) fn png(width: u32, height: u32) -> Vec<u8> {
        let mut bytes = Vec::new();
        image::RgbaImage::new(width, height)
            .write_to(
                &mut std::io::Cursor::new(&mut bytes),
                image::ImageFormat::Png,
            )
            .unwrap();
        bytes
    }

    #[test]
    fn inspection_reads_dimensions_and_rejects_non_images() {
        let found = inspect(&png(3, 2)).unwrap();
        assert_eq!(
            (found.mime_type, found.width, found.height),
            ("image/png", 3, 2)
        );
        assert!(inspect(b"<svg/>").is_err());
        assert!(inspect(b"not an image").is_err());
    }
}
