//! Pictures in the terminal. Downloads are authenticated Blossom GETs;
//! decoding and protocol encoding run off the UI task. iTerm2, Kitty,
//! WezTerm and Ghostty get real images; other terminals get half-blocks.

use std::collections::HashMap;
use std::sync::Arc;

use image::DynamicImage;
use ratatui::layout::Size;
use ratatui_image::Resize;
use ratatui_image::picker::{Picker, ProtocolType};
use ratatui_image::protocol::Protocol;

pub enum Slot {
    Loading,
    Ready(Arc<DynamicImage>),
    Failed(String),
}

#[derive(Default)]
pub struct Media {
    pub images: HashMap<String, Slot>,
    /// (url, cols, rows) -> encoded protocol, ready to draw
    pub encoded: HashMap<(String, u16, u16), Protocol>,
    pub encoding: std::collections::HashSet<(String, u16, u16)>,
    /// Downloaded files, so `O` doesn't fetch them again.
    pub bytes: HashMap<String, Arc<Vec<u8>>>,
    /// url -> (bytes so far, total if known) while it downloads
    pub progress: HashMap<String, (u64, Option<u64>)>,
}

/// "1.2 / 2.7 MB" for the viewer's loading line.
pub fn progress_label(got: u64, total: Option<u64>) -> String {
    let mb = |n: u64| format!("{:.1}", n as f64 / 1_000_000.0);
    match total {
        Some(t) => format!("{} / {} MB", mb(got), mb(t)),
        None => format!("{} MB", mb(got)),
    }
}

/// Pick the image protocol. `BUZZ_VIM_IMAGES=iterm2|kitty|sixel|halfblocks|off`
/// overrides detection (the test harness uses it).
pub fn picker() -> Option<Picker> {
    let forced = std::env::var("BUZZ_VIM_IMAGES").ok();
    if forced.as_deref() == Some("off") {
        return None;
    }
    let mut p = match forced.as_deref() {
        Some(_) => Picker::halfblocks(),
        None => Picker::from_query_stdio().unwrap_or_else(|_| Picker::halfblocks()),
    };
    let iterm = std::env::var("TERM_PROGRAM").is_ok_and(|t| t.contains("iTerm"))
        || std::env::var("LC_TERMINAL").is_ok_and(|t| t.contains("iTerm"));
    match forced.as_deref() {
        Some("iterm2") => p.set_protocol_type(ProtocolType::Iterm2),
        Some("kitty") => p.set_protocol_type(ProtocolType::Kitty),
        Some("sixel") => p.set_protocol_type(ProtocolType::Sixel),
        Some(_) => p.set_protocol_type(ProtocolType::Halfblocks),
        None if iterm => p.set_protocol_type(ProtocolType::Iterm2),
        None => {}
    }
    Some(p)
}

pub fn decode(bytes: &[u8]) -> anyhow::Result<DynamicImage> {
    Ok(image::ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()?
        .decode()?)
}

pub fn encode(
    picker: &Picker,
    img: &DynamicImage,
    cols: u16,
    rows: u16,
) -> anyhow::Result<Protocol> {
    Ok(picker.new_protocol(img.clone(), Size::new(cols, rows), Resize::Fit(None))?)
}

/// Rows a thumbnail gets in the timeline, from its pixel size.
pub fn thumb_rows(dim: Option<(u32, u32)>, cols: u16, max_rows: u16) -> u16 {
    match dim {
        // Cells are about twice as tall as wide.
        Some((w, h)) if w > 0 => {
            (((h as f32 / w as f32) * cols as f32 / 2.0).ceil() as u16).clamp(3, max_rows)
        }
        _ => max_rows,
    }
}
