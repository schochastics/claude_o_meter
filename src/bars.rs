//! Unicode block-bar rendering.

const FULL: char = '🟧'; // Claude-orange emoji square — renders in color
const EMPTY: char = '░';

/// Render a horizontal bar with `width` cells. `fraction` is clamped to 0..=1.
pub fn render_bar(fraction: f64, width: usize) -> String {
    let f = fraction.clamp(0.0, 1.0);
    let filled = (f * width as f64).round() as usize;
    let filled = filled.min(width);
    let mut s = String::with_capacity(width * 4);
    for _ in 0..filled {
        s.push(FULL);
    }
    for _ in filled..width {
        s.push(EMPTY);
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_zero() {
        assert_eq!(render_bar(0.0, 5), "░░░░░");
    }

    #[test]
    fn full_one() {
        assert_eq!(render_bar(1.0, 5), "🟧🟧🟧🟧🟧");
    }

    #[test]
    fn half() {
        assert_eq!(render_bar(0.5, 4), "🟧🟧░░");
    }

    #[test]
    fn rounds_up() {
        assert_eq!(render_bar(0.51, 4), "🟧🟧░░");
        assert_eq!(render_bar(0.6, 5), "🟧🟧🟧░░");
    }

    #[test]
    fn clamps_over_one() {
        assert_eq!(render_bar(1.7, 5), "🟧🟧🟧🟧🟧");
    }

    #[test]
    fn clamps_negative() {
        assert_eq!(render_bar(-0.3, 5), "░░░░░");
    }
}
