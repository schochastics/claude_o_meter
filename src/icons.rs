//! Tray icons drawn at runtime as raw RGBA buffers.
//!
//! The shape is a stylized 8-point sparkle (a "burst") meant to evoke
//! Claude's wordmark glyph: 4 long N/S/E/W rays plus 4 shorter diagonal
//! rays. The whole sparkle is tinted by the current `Band` so utilization
//! reads at a glance.

use tray_icon::Icon;

const SIZE: u32 = 22; // macOS menu bar standard height

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

    fn rgb(self) -> [u8; 3] {
        match self {
            Band::Blue => [0x4D, 0x9D, 0xE0],
            Band::Green => [0x3A, 0xC0, 0x6E],
            Band::Orange => [0xF2, 0x9E, 0x4C],
            Band::Red => [0xE6, 0x4A, 0x4A],
        }
    }
}

pub fn icon_for(band: Band) -> Icon {
    let rgba = render_sparkle(band, SIZE);
    Icon::from_rgba(rgba, SIZE, SIZE).expect("valid RGBA buffer")
}

pub fn auth_required_icon() -> Icon {
    icon_for(Band::Red)
}

/// Coverage (0..=1) of the sparkle at relative offset (dx, dy) from the
/// center. The sparkle is the union of two astroids (4-pointed stars with
/// pinched waists) — one cardinal-aligned (large) and one diagonal-aligned
/// (smaller). This gives the Claude-style 8-point burst.
fn sparkle_alpha(dx: f64, dy: f64, size: f64) -> f64 {
    let cardinal_r = size * 0.50;
    let diagonal_r = size * 0.32;

    let a_cardinal = astroid_alpha(dx, dy, cardinal_r);

    let cos45 = std::f64::consts::FRAC_1_SQRT_2;
    let u = dx * cos45 + dy * cos45;
    let v = -dx * cos45 + dy * cos45;
    let a_diagonal = astroid_alpha(u, v, diagonal_r);

    a_cardinal.max(a_diagonal)
}

/// Astroid (4-pointed star) coverage: |dx/r|^p + |dy/r|^p <= 1, with p < 1
/// giving concave sides and pointed cardinal tips.
fn astroid_alpha(dx: f64, dy: f64, r: f64) -> f64 {
    let p = 0.6_f64;
    let val = (dx.abs() / r).powf(p) + (dy.abs() / r).powf(p);
    let inner = 0.85;
    let outer = 1.05;
    if val <= inner {
        1.0
    } else if val >= outer {
        0.0
    } else {
        let t = (val - inner) / (outer - inner);
        let s = t * t * (3.0 - 2.0 * t);
        1.0 - s
    }
}

fn render_sparkle(band: Band, size: u32) -> Vec<u8> {
    let [r, g, b] = band.rgb();
    let mut buf = vec![0u8; (size * size * 4) as usize];
    let center = (size as f64 - 1.0) / 2.0;
    let s = size as f64;

    for y in 0..size {
        for x in 0..size {
            let dx = x as f64 - center;
            let dy = y as f64 - center;
            let alpha = sparkle_alpha(dx, dy, s);
            let i = ((y * size + x) * 4) as usize;
            buf[i] = r;
            buf[i + 1] = g;
            buf[i + 2] = b;
            buf[i + 3] = (alpha * 255.0).round() as u8;
        }
    }
    buf
}

#[cfg(test)]
mod tests {
    use super::*;

    fn alpha_at(buf: &[u8], size: u32, x: u32, y: u32) -> u8 {
        let i = ((y * size + x) * 4 + 3) as usize;
        buf[i]
    }

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
    fn sparkle_has_correct_size() {
        let buf = render_sparkle(Band::Blue, 22);
        assert_eq!(buf.len(), 22 * 22 * 4);
    }

    #[test]
    fn center_pixel_is_opaque() {
        let buf = render_sparkle(Band::Blue, 22);
        assert_eq!(alpha_at(&buf, 22, 11, 11), 255);
    }

    #[test]
    fn corner_pixel_is_transparent() {
        let buf = render_sparkle(Band::Blue, 22);
        assert_eq!(alpha_at(&buf, 22, 0, 0), 0);
        assert_eq!(alpha_at(&buf, 22, 21, 21), 0);
    }

    #[test]
    fn cardinal_arms_are_lit() {
        // Each cardinal arm should be clearly visible well past the body.
        let buf = render_sparkle(Band::Blue, 22);
        assert!(alpha_at(&buf, 22, 18, 11) > 100, "east arm");
        assert!(alpha_at(&buf, 22, 3, 11) > 100, "west arm");
        assert!(alpha_at(&buf, 22, 11, 18) > 100, "south arm");
        assert!(alpha_at(&buf, 22, 11, 3) > 100, "north arm");
    }

    #[test]
    fn diagonals_are_visible_but_shorter() {
        let buf = render_sparkle(Band::Blue, 22);
        // Mid-diagonal pixel inside the smaller diagonal star.
        assert!(alpha_at(&buf, 22, 14, 14) > 100);
        // The far corner is outside both the cardinal and diagonal stars.
        assert_eq!(alpha_at(&buf, 22, 19, 19), 0);
    }

    #[test]
    fn empty_band_between_cardinals_and_diagonals() {
        // Between an arm direction and a diagonal there is open space.
        // (x=18, y=14) sits between the east arm and the SE diagonal,
        // well beyond the radius of either star.
        let buf = render_sparkle(Band::Blue, 22);
        assert_eq!(alpha_at(&buf, 22, 18, 14), 0);
    }
}
