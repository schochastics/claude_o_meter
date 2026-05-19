//! Pure-Rust RGBA bar rendering for menu-item icons.
//!
//! Every bar is rasterized onto a fixed-size canvas so the text to the
//! right of the icon lands at a consistent x-position across rows. macOS
//! scales the image to 18 pt menu-row height while preserving the aspect
//! ratio, so the on-screen size is 100×18 pt regardless of retina.

/// 100×18 pt at 2x retina.
pub const CANVAS_W: u32 = 200;
pub const CANVAS_H: u32 = 36;

#[derive(Debug, Clone, Copy)]
pub struct Theme {
    /// Background of the "empty" portion of the bar.
    pub empty_bg: [u8; 4],
}

/// Render a horizontal stacked bar onto a CANVAS_W × CANVAS_H RGBA buffer.
///
/// `segments` are `(count, rgb)` pairs laid out left-to-right. The filled
/// width is `(sum(counts) / scale_basis) × CANVAS_W`, clamped to the
/// canvas. The remainder of the row is `theme.empty_bg`.
pub fn render_bar_rgba(segments: &[(u64, [u8; 3])], scale_basis: u64, theme: &Theme) -> Vec<u8> {
    let mut buf = fill(theme.empty_bg);
    let total: u64 = segments.iter().map(|(n, _)| *n).sum();
    if total == 0 || scale_basis == 0 {
        return buf;
    }
    let basis = scale_basis.max(total);
    let mut x = 0u32;
    for (n, rgb) in segments {
        if *n == 0 {
            continue;
        }
        let segment_px = ((*n as f64 / basis as f64) * CANVAS_W as f64).round() as u32;
        let end = (x + segment_px).min(CANVAS_W);
        paint_band(&mut buf, x, end, *rgb);
        x = end;
        if x >= CANVAS_W {
            break;
        }
    }
    buf
}

/// Single-color bar — convenience for session / weekly / per-model / per-project.
pub fn render_solid_bar_rgba(fraction: f64, color: [u8; 3], theme: &Theme) -> Vec<u8> {
    let f = fraction.clamp(0.0, 1.0);
    let cells = (f * CANVAS_W as f64).round() as u64;
    let basis = CANVAS_W as u64;
    render_bar_rgba(&[(cells, color)], basis, theme)
}

fn fill(rgba: [u8; 4]) -> Vec<u8> {
    let mut buf = Vec::with_capacity((CANVAS_W * CANVAS_H * 4) as usize);
    for _ in 0..(CANVAS_W * CANVAS_H) {
        buf.extend_from_slice(&rgba);
    }
    buf
}

fn paint_band(buf: &mut [u8], x_start: u32, x_end: u32, rgb: [u8; 3]) {
    if x_end <= x_start {
        return;
    }
    for y in 0..CANVAS_H {
        let row_start = (y * CANVAS_W * 4) as usize;
        for x in x_start..x_end {
            let i = row_start + (x * 4) as usize;
            buf[i] = rgb[0];
            buf[i + 1] = rgb[1];
            buf[i + 2] = rgb[2];
            buf[i + 3] = 0xFF;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn light_theme() -> Theme {
        Theme {
            empty_bg: [0xE5, 0xE5, 0xE7, 0xFF],
        }
    }

    fn pixel(buf: &[u8], x: u32, y: u32) -> [u8; 4] {
        let i = ((y * CANVAS_W + x) * 4) as usize;
        [buf[i], buf[i + 1], buf[i + 2], buf[i + 3]]
    }

    #[test]
    fn canvas_has_correct_byte_length() {
        let buf = render_bar_rgba(&[(50, [255, 0, 0])], 100, &light_theme());
        assert_eq!(buf.len(), (CANVAS_W * CANVAS_H * 4) as usize);
    }

    #[test]
    fn bar_fills_proportionally() {
        let buf = render_bar_rgba(&[(60, [255, 0, 0])], 100, &light_theme());
        // First 60% should be red, last 40% should be empty_bg.
        let mid_filled = pixel(&buf, CANVAS_W / 4, CANVAS_H / 2);
        let near_end = pixel(&buf, (CANVAS_W * 9) / 10, CANVAS_H / 2);
        assert_eq!(mid_filled[..3], [255, 0, 0]);
        assert_eq!(near_end, light_theme().empty_bg);
    }

    #[test]
    fn zero_total_is_empty_bg_only() {
        let buf = render_bar_rgba(&[(0, [255, 0, 0])], 100, &light_theme());
        assert_eq!(pixel(&buf, 0, 0), light_theme().empty_bg);
        assert_eq!(
            pixel(&buf, CANVAS_W - 1, CANVAS_H - 1),
            light_theme().empty_bg
        );
    }

    #[test]
    fn overflow_clamps_to_canvas() {
        // Sum 200, scale basis 100 → would want 2× width; clamps to canvas.
        let buf = render_bar_rgba(&[(200, [0, 255, 0])], 100, &light_theme());
        assert_eq!(pixel(&buf, 0, 0)[..3], [0, 255, 0]);
        assert_eq!(pixel(&buf, CANVAS_W - 1, 0)[..3], [0, 255, 0]);
    }

    #[test]
    fn stack_order_matches_input() {
        let buf = render_bar_rgba(
            &[(25, [255, 0, 0]), (25, [0, 255, 0]), (25, [0, 0, 255])],
            100,
            &light_theme(),
        );
        // First 25% red, next 25% green, next 25% blue, last 25% empty.
        assert_eq!(pixel(&buf, CANVAS_W / 8, 0)[..3], [255, 0, 0]);
        assert_eq!(pixel(&buf, (CANVAS_W * 3) / 8, 0)[..3], [0, 255, 0]);
        assert_eq!(pixel(&buf, (CANVAS_W * 5) / 8, 0)[..3], [0, 0, 255]);
        assert_eq!(pixel(&buf, (CANVAS_W * 7) / 8, 0), light_theme().empty_bg);
    }

    #[test]
    fn solid_bar_at_fraction() {
        let buf = render_solid_bar_rgba(0.5, [255, 128, 0], &light_theme());
        assert_eq!(pixel(&buf, CANVAS_W / 4, 0)[..3], [255, 128, 0]);
        assert_eq!(pixel(&buf, (CANVAS_W * 3) / 4, 0), light_theme().empty_bg);
    }
}
