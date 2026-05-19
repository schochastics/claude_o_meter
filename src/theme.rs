//! Detect macOS Light/Dark appearance and provide a matching `bars::Theme`.

use crate::bars::Theme;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Appearance {
    Light,
    Dark,
}

impl Appearance {
    /// Read the current effective appearance from `NSApp`. Falls back to
    /// `Light` if `NSApp` isn't available (e.g. running outside an .app
    /// bundle or before the run loop has started).
    pub fn detect() -> Self {
        detect_via_nsapp().unwrap_or(Appearance::Light)
    }

    pub fn theme(self) -> Theme {
        match self {
            Appearance::Light => Theme {
                empty_bg: [0xE5, 0xE5, 0xE7, 0xFF],
            },
            Appearance::Dark => Theme {
                empty_bg: [0x3A, 0x3A, 0x3C, 0xFF],
            },
        }
    }
}

#[cfg(target_os = "macos")]
fn detect_via_nsapp() -> Option<Appearance> {
    use objc2::rc::autoreleasepool;
    use objc2_app_kit::{
        NSAppearance, NSAppearanceNameAqua, NSAppearanceNameDarkAqua, NSApplication,
    };
    use objc2_foundation::NSArray;

    autoreleasepool(|_| unsafe {
        let mtm = objc2_foundation::MainThreadMarker::new()?;
        let app = NSApplication::sharedApplication(mtm);
        let appearance = app.effectiveAppearance();
        let names = NSArray::from_retained_slice(&[
            NSAppearanceNameAqua.to_owned(),
            NSAppearanceNameDarkAqua.to_owned(),
        ]);
        let best = NSAppearance::bestMatchFromAppearancesWithNames(&appearance, &names)?;
        let s = best.to_string();
        if s == NSAppearanceNameDarkAqua.to_string() {
            Some(Appearance::Dark)
        } else {
            Some(Appearance::Light)
        }
    })
}

#[cfg(not(target_os = "macos"))]
fn detect_via_nsapp() -> Option<Appearance> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn light_bg_is_lighter_than_dark_bg() {
        let l = Appearance::Light.theme().empty_bg;
        let d = Appearance::Dark.theme().empty_bg;
        // Compare luma. Light should be brighter.
        let l_sum = l[0] as u32 + l[1] as u32 + l[2] as u32;
        let d_sum = d[0] as u32 + d[1] as u32 + d[2] as u32;
        assert!(l_sum > d_sum);
    }
}
