//! Colored-dot tray icons generated at runtime as raw RGBA buffers.
//!
//! `tray_icon::Icon::from_rgba` accepts unencoded RGBA bytes, so we draw the
//! dot with simple geometry — no PNG decoding, no asset files to bundle.

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

/// Build a tray icon as a filled antialiased circle in the band's color.
pub fn icon_for(band: Band) -> Icon {
    let rgba = render_dot(band, SIZE);
    Icon::from_rgba(rgba, SIZE, SIZE).expect("valid RGBA buffer")
}

/// Build the "needs login" icon — a red dot with a hollow center.
pub fn auth_required_icon() -> Icon {
    let rgba = render_dot(Band::Red, SIZE);
    Icon::from_rgba(rgba, SIZE, SIZE).expect("valid RGBA buffer")
}

fn render_dot(band: Band, size: u32) -> Vec<u8> {
    let [r, g, b] = band.rgb();
    let mut buf = vec![0u8; (size * size * 4) as usize];
    let center = (size as f64 - 1.0) / 2.0;
    let radius = (size as f64) * 0.42;
    let edge = 1.0; // antialias band width in pixels

    for y in 0..size {
        for x in 0..size {
            let dx = x as f64 - center;
            let dy = y as f64 - center;
            let d = (dx * dx + dy * dy).sqrt();
            let alpha = if d <= radius - edge {
                1.0
            } else if d >= radius {
                0.0
            } else {
                (radius - d) / edge
            };
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
    fn render_dot_has_correct_size() {
        let buf = render_dot(Band::Blue, 22);
        assert_eq!(buf.len(), 22 * 22 * 4);
    }

    #[test]
    fn center_pixel_is_opaque() {
        let buf = render_dot(Band::Blue, 22);
        let i = ((11 * 22 + 11) * 4) as usize;
        assert_eq!(buf[i + 3], 255);
    }

    #[test]
    fn corner_pixel_is_transparent() {
        let buf = render_dot(Band::Blue, 22);
        assert_eq!(buf[3], 0);
    }
}
