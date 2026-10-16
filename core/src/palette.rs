//! Every color sits on one 24-step brightness ramp, in exact RGB so every
//! screen shows the same shades. Hierarchy comes from brightness, bold and
//! dim. There is a dark and a light ramp.
//!
//! Every role and every person, channel and key chip gets its own hue, all
//! at the luminance of the gray step they replace. Under a grayscale filter
//! the screen reads as the plain gray ramp.

/// What a color is for. Each role has its own hue.
#[derive(Clone, Copy)]
pub enum Role {
    /// Canvas, cards, rules, chip text.
    Surface,
    /// Body text.
    Text,
    /// Selected tab, key chips, badges, mentions.
    Accent,
    /// Secondary text: times, hints.
    Muted,
    /// Code blocks.
    Code,
}

/// (hue in degrees, OKLab chroma) per role in `Role` order. Surfaces and
/// body text stay faint so reading is calm; the rest carry color.
const ROLES: [(f32, f32); 5] = [
    (265.0, 0.016),
    (80.0, 0.02),
    (330.0, 0.12),
    (200.0, 0.07),
    (150.0, 0.11),
];

/// Hues handed out to names, channels and chips, spread around the wheel.
const WHEEL: [f32; 12] = [
    0.0, 140.0, 260.0, 40.0, 180.0, 300.0, 80.0, 220.0, 340.0, 110.0, 20.0, 240.0,
];

/// Chroma for named things. Asked for high; `tint` shrinks it until the
/// color fits sRGB at that luminance.
pub const VIVID: f32 = 0.14;

/// The `i`th hue on the wheel, for things in a row (chips, flowers).
pub fn nth(i: usize) -> f32 {
    WHEEL[i % WHEEL.len()]
}

/// A stable hue for a key (pubkey, channel id): the same person or channel
/// always gets the same color.
pub fn hue(key: &str) -> f32 {
    let h = key.bytes().fold(0xcbf2_9ce4_8422_2325u64, |h, b| {
        (h ^ b as u64).wrapping_mul(0x100_0000_01b3)
    });
    nth(((h >> 40) % WHEEL.len() as u64) as usize)
}

/// Step `level` (0 = canvas, 23 = strongest text) for `role`: the ramp's
/// gray, tinted by the role's hue.
pub fn tone(role: Role, level: u8, light: bool) -> [u8; 3] {
    let (hue, chroma) = ROLES[role as usize];
    shade(hue, chroma, level, light)
}

/// Step `level` in `hue` at full color.
pub fn ink(hue: f32, level: u8, light: bool) -> [u8; 3] {
    shade(hue, VIVID, level, light)
}

/// Step `level` in `hue` at `chroma`. Dark runs near black to near white;
/// light is the same steps mirrored.
pub fn shade(hue: f32, chroma: f32, level: u8, light: bool) -> [u8; 3] {
    let l = level.min(23);
    let v = if light { 250 - 10 * l } else { 8 + 10 * l };
    if chroma == 0.0 {
        return [v, v, v];
    }
    tint(v, hue, chroma)
}

fn lin(c: f32) -> f32 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

fn enc(c: f32) -> f32 {
    if c <= 0.0031308 {
        c * 12.92
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    }
}

/// Relative luminance (Rec. 709) of linear RGB.
fn luma(c: [f32; 3]) -> f32 {
    0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2]
}

/// OKLab to linear sRGB (Björn Ottosson's matrices).
fn oklab(l: f32, a: f32, b: f32) -> [f32; 3] {
    let l_ = (l + 0.396_337_78 * a + 0.215_803_76 * b).powi(3);
    let m_ = (l - 0.105_561_346 * a - 0.063_854_17 * b).powi(3);
    let s_ = (l - 0.089_484_18 * a - 1.291_485_5 * b).powi(3);
    [
        4.076_741_7 * l_ - 3.307_711_6 * m_ + 0.230_969_94 * s_,
        -1.268_438 * l_ + 2.609_757_4 * m_ - 0.341_319_38 * s_,
        -0.004_196_086 * l_ - 0.703_418_6 * m_ + 1.707_614_7 * s_,
    ]
}

/// The gray `v` with hue and chroma added, then OKLab lightness solved so
/// the luminance equals the gray's. Chroma shrinks until the color fits sRGB.
fn tint(v: u8, hue: f32, chroma: f32) -> [u8; 3] {
    let y = lin(v as f32 / 255.0);
    let (sin, cos) = hue.to_radians().sin_cos();
    let mut c = chroma;
    for _ in 0..12 {
        let (a, b) = (c * cos, c * sin);
        let (mut lo, mut hi) = (0.0f32, 1.0f32);
        for _ in 0..30 {
            let mid = (lo + hi) / 2.0;
            if luma(oklab(mid, a, b)) < y {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        let rgb = oklab((lo + hi) / 2.0, a, b);
        if rgb.iter().all(|&x| (-0.0005..=1.0005).contains(&x)) {
            return rgb.map(|x| (enc(x.clamp(0.0, 1.0)) * 255.0).round() as u8);
        }
        c *= 0.75;
    }
    [v, v, v]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every palette step keeps its gray's luminance, so a grayscale filter
    /// shows exactly the plain ramp (within rounding).
    #[test]
    fn palettes_match_the_gray_ramp() {
        let wheel = WHEEL.map(|h| (h, VIVID));
        for (name, set) in [("roles", &ROLES[..]), ("wheel", &wheel[..])] {
            for v in (8..=250).step_by(10) {
                let y = lin(v as f32 / 255.0);
                for &(hue, chroma) in set {
                    let c = tint(v as u8, hue, chroma);
                    let got = luma(c.map(|x| lin(x as f32 / 255.0)));
                    let gray_of = enc(got) * 255.0;
                    assert!(
                        (gray_of - v as f32).abs() < 1.5,
                        "{name} {v} {hue}: {c:?} reads as {gray_of}, y {y}"
                    );
                }
            }
        }
    }
}
