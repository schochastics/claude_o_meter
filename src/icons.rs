//! Tray icons built by tinting a precomputed alpha mask of the Claude AI
//! symbol. The mask is rasterized from `assets/claude_symbol.svg` at build
//! time (see `build.rs`) and embedded as a raw byte blob — no SVG or raster
//! dependencies survive into the runtime binary.

use tray_icon::Icon;

const ICON_SIZE: u32 = 44;
const ICON_MASK: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/icon_mask.bin"));

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum Band {
    Blue,
    Green,
    Orange,
    Red,
}

impl Band {
    /// Pick the band by utilization (0..=1+). Above 100% counts as red.
    pub fn from_fraction(f: f64) -> Self {
        if f < 0.50 {
            Band::Blue
        } else if f < 0.75 {
            Band::Green
        } else if f < 0.90 {
            Band::Orange
        } else {
            Band::Red
        }
    }

    pub fn rgb(self) -> [u8; 3] {
        match self {
            Band::Blue => [0x4D, 0x9D, 0xE0],
            Band::Green => [0x3A, 0xC0, 0x6E],
            Band::Orange => [0xF2, 0x9E, 0x4C],
            Band::Red => [0xE6, 0x4A, 0x4A],
        }
    }
}

pub fn icon_for(band: Band) -> Icon {
    Icon::from_rgba(tinted(band), ICON_SIZE, ICON_SIZE).expect("valid RGBA buffer")
}

pub fn icon_for_split(left: Band, right: Band) -> Icon {
    Icon::from_rgba(tinted_split(left, right), ICON_SIZE, ICON_SIZE)
        .expect("valid RGBA buffer")
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn band_thresholds() {
        assert_eq!(Band::from_fraction(0.0), Band::Blue);
        assert_eq!(Band::from_fraction(0.49), Band::Blue);
        assert_eq!(Band::from_fraction(0.50), Band::Green);
        assert_eq!(Band::from_fraction(0.74), Band::Green);
        assert_eq!(Band::from_fraction(0.75), Band::Orange);
        assert_eq!(Band::from_fraction(0.89), Band::Orange);
        assert_eq!(Band::from_fraction(0.90), Band::Red);
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
}
