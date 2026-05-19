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

/// Render a stacked bar of `width` cells. Each `(count, char)` segment
/// occupies `round(count / total * width)` cells, in order. Any trailing
/// cells (rounding slack, or zero total) are filled with EMPTY.
pub fn render_stacked_bar(segments: &[(u64, char)], width: usize) -> String {
    let total: u64 = segments.iter().map(|(n, _)| *n).sum();
    let mut s = String::with_capacity(width * 4);
    if total == 0 {
        for _ in 0..width {
            s.push(EMPTY);
        }
        return s;
    }
    let mut used = 0usize;
    for (n, ch) in segments {
        let cells = ((*n as f64) / (total as f64) * width as f64).round() as usize;
        let cells = cells.min(width - used);
        for _ in 0..cells {
            s.push(*ch);
        }
        used += cells;
        if used >= width {
            break;
        }
    }
    for _ in used..width {
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

    #[test]
    fn stacked_bar_sums_to_width_when_total_nonzero() {
        let bar = render_stacked_bar(&[(10, 'A'), (10, 'B'), (10, 'C')], 9);
        assert_eq!(bar.chars().count(), 9);
    }

    #[test]
    fn stacked_bar_proportions_match_input() {
        let bar = render_stacked_bar(&[(50, 'A'), (50, 'B')], 10);
        assert_eq!(bar.matches('A').count(), 5);
        assert_eq!(bar.matches('B').count(), 5);
    }

    #[test]
    fn stacked_bar_handles_zero_total() {
        let bar = render_stacked_bar(&[(0, 'A'), (0, 'B')], 6);
        assert_eq!(bar, "░░░░░░");
    }

    #[test]
    fn stacked_bar_skips_zero_segments() {
        let bar = render_stacked_bar(&[(100, 'A'), (0, 'B'), (100, 'C')], 10);
        assert_eq!(bar.matches('A').count(), 5);
        assert_eq!(bar.matches('B').count(), 0);
        assert_eq!(bar.matches('C').count(), 5);
    }

    #[test]
    fn stacked_bar_caps_at_width_on_rounding_overflow() {
        // 1/3 each, width=10, naive rounding gives 3+3+3=9 — fine.
        // Edge case where rounding could push past width:
        let bar = render_stacked_bar(&[(1, 'A'), (1, 'B'), (1, 'C'), (1, 'D')], 5);
        assert!(bar.chars().count() <= 5);
    }
}
