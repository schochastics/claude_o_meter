//! Tray icons built by tinting a precomputed alpha mask of the Claude AI
//! symbol. The mask is rasterized from `assets/claude_symbol.svg` at build
//! time (see `build.rs`) and embedded as a raw byte blob — no SVG or raster
//! dependencies survive into the runtime binary.

use tray_icon::Icon;

const ICON_SIZE: u32 = 44;
const ICON_MASK: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/icon_mask.bin"));

// Alarm badge — a small red "!" disc stamped onto the top-right corner of the
// symbol when session usage is spiking. Drawn opaquely over whatever is there
// (including transparent background pixels) so it's always visible.
const BADGE_CX: i32 = ICON_SIZE as i32 - 9;
const BADGE_CY: i32 = 9;
const BADGE_R: i32 = 8; // outer dark ring radius
const BADGE_R_FILL: i32 = 7; // red fill radius
const BADGE_FILL: [u8; 4] = [0xFF, 0x3B, 0x30, 0xFF];
const BADGE_RING: [u8; 4] = [0x99, 0x20, 0x18, 0xFF];
const BADGE_GLYPH: [u8; 4] = [0xFF, 0xFF, 0xFF, 0xFF];

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum Band {
    Blue,
    Green,
    Yellow,
    Orange,
    Red,
}

impl Band {
    /// Pick the band by utilization (0..=1+). Above 100% counts as red.
    pub fn from_fraction(f: f64) -> Self {
        if f < 0.25 {
            Band::Blue
        } else if f < 0.50 {
            Band::Green
        } else if f < 0.75 {
            Band::Yellow
        } else if f < 0.95 {
            Band::Orange
        } else {
            Band::Red
        }
    }

    pub fn rgb(self) -> [u8; 3] {
        match self {
            Band::Blue => [0x1D, 0x48, 0x77],
            Band::Green => [0x1B, 0x8A, 0x5A],
            Band::Yellow => [0xFB, 0xB0, 0x21],
            Band::Orange => [0xF6, 0x88, 0x38],
            Band::Red => [0xEE, 0x3E, 0x32],
        }
    }
}

pub fn icon_for(band: Band) -> Icon {
    Icon::from_rgba(tinted(band), ICON_SIZE, ICON_SIZE).expect("valid RGBA buffer")
}

pub fn icon_for_split(left: Band, right: Band) -> Icon {
    Icon::from_rgba(tinted_split(left, right), ICON_SIZE, ICON_SIZE).expect("valid RGBA buffer")
}

/// Like [`icon_for_split`], but stamps an alarm badge when `badge` is true
/// (session usage spiking). When false the buffer is identical to
/// [`icon_for_split`] — the badge is purely additive.
pub fn icon_for_split_badged(left: Band, right: Band, badge: bool) -> Icon {
    let mut rgba = tinted_split(left, right);
    if badge {
        draw_alarm_badge(&mut rgba);
    }
    Icon::from_rgba(rgba, ICON_SIZE, ICON_SIZE).expect("valid RGBA buffer")
}

pub fn auth_required_icon() -> Icon {
    icon_for(Band::Red)
}

pub(crate) fn tinted(band: Band) -> Vec<u8> {
    let [r, g, b] = band.rgb();
    let mut rgba = Vec::with_capacity(ICON_MASK.len() * 4);
    for &a in ICON_MASK {
        rgba.extend_from_slice(&[r, g, b, a]);
    }
    rgba
}

/// Tint the left half of the mask with `left.rgb()` and the right half with
/// `right.rgb()`. Hard-edge split at the geometric midline.
pub(crate) fn tinted_split(left: Band, right: Band) -> Vec<u8> {
    let [lr, lg, lb] = left.rgb();
    let [rr, rg, rb] = right.rgb();
    let mid = ICON_SIZE / 2;
    let mut rgba = Vec::with_capacity(ICON_MASK.len() * 4);
    for (i, &a) in ICON_MASK.iter().enumerate() {
        let x = (i as u32) % ICON_SIZE;
        let [r, g, b] = if x < mid { [lr, lg, lb] } else { [rr, rg, rb] };
        rgba.extend_from_slice(&[r, g, b, a]);
    }
    rgba
}

/// Stamp an opaque red "!" disc onto the top-right corner of an RGBA buffer.
fn draw_alarm_badge(rgba: &mut [u8]) {
    let put = |buf: &mut [u8], x: i32, y: i32, rgba: [u8; 4]| {
        if x < 0 || y < 0 || x >= ICON_SIZE as i32 || y >= ICON_SIZE as i32 {
            return;
        }
        let off = ((y as u32 * ICON_SIZE + x as u32) * 4) as usize;
        buf[off..off + 4].copy_from_slice(&rgba);
    };

    // Filled disc with a darker ring for contrast against light menu bars.
    for dy in -BADGE_R..=BADGE_R {
        for dx in -BADGE_R..=BADGE_R {
            let d2 = dx * dx + dy * dy;
            let color = if d2 <= BADGE_R_FILL * BADGE_R_FILL {
                BADGE_FILL
            } else if d2 <= BADGE_R * BADGE_R {
                BADGE_RING
            } else {
                continue;
            };
            put(rgba, BADGE_CX + dx, BADGE_CY + dy, color);
        }
    }

    // White "!" — a 2px stem with a 2x2 dot beneath it.
    for y in (BADGE_CY - 4)..=(BADGE_CY + 1) {
        put(rgba, BADGE_CX - 1, y, BADGE_GLYPH);
        put(rgba, BADGE_CX, y, BADGE_GLYPH);
    }
    for y in (BADGE_CY + 3)..=(BADGE_CY + 4) {
        put(rgba, BADGE_CX - 1, y, BADGE_GLYPH);
        put(rgba, BADGE_CX, y, BADGE_GLYPH);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn band_thresholds() {
        assert_eq!(Band::from_fraction(0.0), Band::Blue);
        assert_eq!(Band::from_fraction(0.24), Band::Blue);
        assert_eq!(Band::from_fraction(0.25), Band::Green);
        assert_eq!(Band::from_fraction(0.49), Band::Green);
        assert_eq!(Band::from_fraction(0.50), Band::Yellow);
        assert_eq!(Band::from_fraction(0.74), Band::Yellow);
        assert_eq!(Band::from_fraction(0.75), Band::Orange);
        assert_eq!(Band::from_fraction(0.94), Band::Orange);
        assert_eq!(Band::from_fraction(0.95), Band::Red);
        assert_eq!(Band::from_fraction(1.5), Band::Red);
    }

    #[test]
    fn mask_has_correct_size() {
        assert_eq!(ICON_MASK.len(), (ICON_SIZE * ICON_SIZE) as usize);
    }

    #[test]
    fn mask_has_lit_and_unlit_pixels() {
        assert!(ICON_MASK.iter().any(|&a| a > 200), "no opaque pixels");
        assert!(ICON_MASK.contains(&0), "no transparent pixels");
    }

    #[test]
    fn tint_uses_band_color() {
        // Find an opaque mask pixel and check it carries the band's RGB.
        let opaque_idx = ICON_MASK.iter().position(|&a| a > 200).unwrap();
        let rgba = tinted(Band::Green);
        let off = opaque_idx * 4;
        assert_eq!(&rgba[off..off + 3], &Band::Green.rgb());
        assert!(rgba[off + 3] > 200);
    }

    #[test]
    fn tint_preserves_alpha() {
        let rgba = tinted(Band::Blue);
        for (i, &a) in ICON_MASK.iter().enumerate() {
            assert_eq!(rgba[i * 4 + 3], a);
        }
    }

    #[test]
    fn rgba_buffer_has_correct_length() {
        let rgba = tinted(Band::Red);
        assert_eq!(rgba.len(), (ICON_SIZE * ICON_SIZE * 4) as usize);
    }

    #[test]
    fn split_tint_left_half_uses_left_rgb() {
        let rgba = tinted_split(Band::Blue, Band::Red);
        let mid = ICON_SIZE / 2;
        let idx = ICON_MASK
            .iter()
            .enumerate()
            .find(|(i, a)| **a > 200 && (*i as u32) % ICON_SIZE < mid)
            .expect("an opaque mask pixel in left half")
            .0;
        let off = idx * 4;
        assert_eq!(&rgba[off..off + 3], &Band::Blue.rgb());
        assert!(rgba[off + 3] > 200);
    }

    #[test]
    fn split_tint_right_half_uses_right_rgb() {
        let rgba = tinted_split(Band::Blue, Band::Red);
        let mid = ICON_SIZE / 2;
        let idx = ICON_MASK
            .iter()
            .enumerate()
            .find(|(i, a)| **a > 200 && (*i as u32) % ICON_SIZE >= mid)
            .expect("an opaque mask pixel in the right half")
            .0;
        let off = idx * 4;
        assert_eq!(&rgba[off..off + 3], &Band::Red.rgb());
        assert!(rgba[off + 3] > 200);
    }

    #[test]
    fn split_tint_preserves_alpha() {
        let rgba = tinted_split(Band::Blue, Band::Red);
        for (i, &a) in ICON_MASK.iter().enumerate() {
            assert_eq!(rgba[i * 4 + 3], a);
        }
    }

    #[test]
    fn badged_icon_has_correct_length() {
        let mut rgba = tinted_split(Band::Blue, Band::Red);
        draw_alarm_badge(&mut rgba);
        assert_eq!(rgba.len(), (ICON_SIZE * ICON_SIZE * 4) as usize);
    }

    #[test]
    fn badge_writes_opaque_alarm_pixels() {
        // The badge centre pixel must carry the opaque alarm red.
        let mut rgba = tinted_split(Band::Blue, Band::Blue);
        draw_alarm_badge(&mut rgba);
        let off = ((BADGE_CY as u32 * ICON_SIZE + BADGE_CX as u32) * 4) as usize;
        // Centre is on the white glyph stem; an off-centre fill pixel is red.
        let fill_off = (((BADGE_CY + BADGE_R_FILL - 1) as u32 * ICON_SIZE + (BADGE_CX + 3) as u32)
            * 4) as usize;
        assert_eq!(&rgba[fill_off..fill_off + 4], &BADGE_FILL);
        assert_eq!(rgba[off + 3], 0xFF, "glyph pixel is opaque");
    }

    #[test]
    fn badge_is_purely_additive() {
        let plain = tinted_split(Band::Green, Band::Orange);
        let badged = {
            let mut b = plain.clone();
            draw_alarm_badge(&mut b);
            b
        };
        // Outside the badge bounding box the buffers must be identical.
        for y in 0..ICON_SIZE as i32 {
            for x in 0..ICON_SIZE as i32 {
                let outside = (x - BADGE_CX).pow(2) + (y - BADGE_CY).pow(2) > BADGE_R * BADGE_R;
                if outside {
                    let off = ((y as u32 * ICON_SIZE + x as u32) * 4) as usize;
                    assert_eq!(&plain[off..off + 4], &badged[off..off + 4]);
                }
            }
        }
    }
}
