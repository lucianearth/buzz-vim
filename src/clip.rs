//! Getting files into a message: the clipboard (Ctrl+V) and drag-and-drop
//! (the terminal pastes the dropped file's path). Nothing here uploads;
//! it only builds a `Pending` that the confirm box shows first.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

use crate::relay::{ALLOWED_MIMES, MAX_IMAGE_BYTES, MAX_VIDEO_BYTES};

pub struct Pending {
    pub name: String,
    pub mime: String,
    pub bytes: Vec<u8>,
    pub source: &'static str,
    pub dims: Option<(u32, u32)>,
}

impl Pending {
    /// Cache key for the confirm-box thumbnail.
    pub fn preview_key(&self) -> String {
        format!("pending:{}:{}", self.name, self.bytes.len())
    }

    pub fn size_label(&self) -> String {
        human_size(self.bytes.len())
    }
}

pub fn human_size(n: usize) -> String {
    match n {
        n if n >= 1 << 20 => format!("{:.1} MB", n as f64 / (1 << 20) as f64),
        n if n >= 1 << 10 => format!("{:.0} KB", n as f64 / 1024.0),
        n => format!("{n} B"),
    }
}

/// Sniff type from magic bytes, the same set the relay accepts.
pub fn sniff(bytes: &[u8]) -> Option<&'static str> {
    let m = match bytes {
        [0x89, b'P', b'N', b'G', ..] => "image/png",
        [0xFF, 0xD8, 0xFF, ..] => "image/jpeg",
        [b'G', b'I', b'F', b'8', ..] => "image/gif",
        [
            b'R',
            b'I',
            b'F',
            b'F',
            _,
            _,
            _,
            _,
            b'W',
            b'E',
            b'B',
            b'P',
            ..,
        ] => "image/webp",
        [_, _, _, _, b'f', b't', b'y', b'p', ..] => "video/mp4",
        _ => return None,
    };
    Some(m)
}

pub fn check(bytes: &[u8]) -> Result<&'static str> {
    check_head(bytes, bytes.len())
}

/// Type from the first bytes, size from `len`, so a file can be refused
/// before it is read.
fn check_head(head: &[u8], len: usize) -> Result<&'static str> {
    let Some(mime) = sniff(head) else {
        bail!("Buzz only takes png, jpeg, gif, webp and mp4 uploads")
    };
    debug_assert!(ALLOWED_MIMES.contains(&mime));
    let max = if mime.starts_with("video/") {
        MAX_VIDEO_BYTES
    } else {
        MAX_IMAGE_BYTES
    };
    if len > max {
        bail!("too big: {} (limit {})", human_size(len), human_size(max));
    }
    Ok(mime)
}

fn dims(bytes: &[u8]) -> Option<(u32, u32)> {
    image::ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()
        .ok()?
        .into_dimensions()
        .ok()
}

pub fn from_file(path: &Path, source: &'static str) -> Result<Pending> {
    use std::io::Read;
    let ctx = || format!("can't read {}", path.display());
    let mut f = std::fs::File::open(path).with_context(ctx)?;
    let len = f.metadata().with_context(ctx)?.len() as usize;
    let mut head = Vec::with_capacity(16);
    (&mut f).take(16).read_to_end(&mut head).with_context(ctx)?;
    let mime = check_head(&head, len)?;
    let mut bytes = head;
    f.read_to_end(&mut bytes).with_context(ctx)?;
    Ok(Pending {
        name: path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "file".into()),
        mime: mime.to_string(),
        dims: dims(&bytes),
        bytes,
        source,
    })
}

/// A paste that is really a dropped file: one or more paths, possibly
/// quoted or backslash-escaped the way terminals do it.
pub fn dropped_paths(paste: &str) -> Vec<PathBuf> {
    let paste = paste.trim();
    if paste.is_empty() || paste.len() > 4096 {
        return vec![];
    }
    let mut out = vec![];
    let mut cur = String::new();
    let mut chars = paste.chars().peekable();
    let mut quote: Option<char> = None;
    while let Some(c) = chars.next() {
        match (c, quote) {
            ('\\', None) => {
                if let Some(n) = chars.next() {
                    cur.push(n);
                }
            }
            ('\'' | '"', None) => quote = Some(c),
            (q, Some(open)) if q == open => quote = None,
            (c, None) if c.is_whitespace() => {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
            }
            (c, _) => cur.push(c),
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    let paths: Vec<PathBuf> = out
        .into_iter()
        .map(|s| PathBuf::from(s.strip_prefix("file://").map(str::to_string).unwrap_or(s)))
        .collect();
    // Only treat it as a drop if every token is an existing file.
    if paths.iter().all(|p| p.is_absolute() && p.is_file()) {
        paths
    } else {
        vec![]
    }
}

/// `f` on a code block: its text onto the system clipboard.
pub fn copy(text: &str) -> Result<()> {
    arboard::Clipboard::new()
        .context("no clipboard available")?
        .set_text(text)?;
    Ok(())
}

/// Ctrl+V. Order: copied files (Finder), then image data (screenshots),
/// then text that is a path.
/// `BUZZ_VIM_CLIPBOARD_FILE` stands in for the system clipboard in tests,
/// for machines that have none.
pub fn from_clipboard() -> Result<Pending> {
    if let Ok(p) = std::env::var("BUZZ_VIM_CLIPBOARD_FILE") {
        let mut pending = from_file(Path::new(&p), "clipboard")?;
        pending.name = "clipboard image".into();
        return Ok(pending);
    }
    let mut cb = arboard::Clipboard::new().context("no clipboard available")?;
    if let Ok(files) = cb.get().file_list() {
        if let Some(f) = files.first() {
            return from_file(f, "clipboard");
        }
    }
    if let Ok(img) = cb.get_image() {
        let rgba =
            image::RgbaImage::from_raw(img.width as u32, img.height as u32, img.bytes.into_owned())
                .context("clipboard image was malformed")?;
        let mut png = Vec::new();
        image::DynamicImage::ImageRgba8(rgba)
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)?;
        check(&png)?;
        return Ok(Pending {
            name: "pasted image.png".into(),
            mime: "image/png".into(),
            dims: Some((img.width as u32, img.height as u32)),
            bytes: png,
            source: "clipboard",
        });
    }
    if let Ok(text) = cb.get_text() {
        if let Some(p) = dropped_paths(&text).first() {
            return from_file(p, "clipboard");
        }
    }
    bail!("nothing pasteable on the clipboard (images, or files Buzz accepts)")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drops_and_sniffing() {
        let dir = std::env::temp_dir().join(format!("bv-clip-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("my shot.png");
        std::fs::write(&f, b"\x89PNG\r\n\x1a\nrest").unwrap();
        let escaped = f.to_string_lossy().replace(' ', "\\ ");
        assert_eq!(dropped_paths(&escaped), vec![f.clone()]);
        assert_eq!(
            dropped_paths(&format!("'{}'", f.display())),
            vec![f.clone()]
        );
        assert!(dropped_paths("just some text").is_empty());
        assert_eq!(sniff(b"\x89PNG...."), Some("image/png"));
        assert!(check(b"%PDF-1.7").is_err());
        let p = from_file(&f, "drop").unwrap();
        assert_eq!(
            (p.mime.as_str(), p.name.as_str()),
            ("image/png", "my shot.png")
        );
        std::fs::remove_dir_all(&dir).ok();
    }
}
