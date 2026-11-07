//! Cover-art decoding. Runs on the media worker thread, once per track change.
//!
//! The raw bytes (PNG / JPEG / WebP) come from the OS backend. We center-crop
//! to a square and downscale, so the GPU texture stays tiny (192x192 RGBA is
//! about 144 KiB) no matter how large the original cover is. Nothing is ever
//! written to disk.

use eframe::egui::ColorImage;
use image::imageops::FilterType;

/// Upper bound for the *encoded* cover (file / network response / SMTC stream).
pub const MAX_BYTES: usize = 8 * 1024 * 1024;

/// Edge length of the decoded, square texture.
const TARGET: u32 = 192;

pub fn decode(bytes: &[u8]) -> Option<ColorImage> {
    let img = image::load_from_memory(bytes).ok()?;
    let (w, h) = (img.width(), img.height());
    if w == 0 || h == 0 {
        return None;
    }

    // Center-crop to a square so the rounded artwork tile never distorts.
    let side = w.min(h);
    let square = img.crop_imm((w - side) / 2, (h - side) / 2, side, side);
    let square = if side > TARGET {
        square.resize_exact(TARGET, TARGET, FilterType::Triangle)
    } else {
        square
    };

    let rgba = square.to_rgba8();
    let size = [rgba.width() as usize, rgba.height() as usize];
    Some(ColorImage::from_rgba_unmultiplied(size, rgba.as_raw()))
}
