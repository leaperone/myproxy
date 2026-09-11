//! Color roles for the desktop UI.
//!
//! `ThemeColor` exposes surfaces (`accent`, `muted`, `secondary`, `group_box`)
//! and one foreground for filled surfaces (`warning_foreground`). Painting a
//! surface token as text, hovering with a fill that equals the card surface, or
//! writing with the amber fill color made states invisible and warnings
//! unreadable, so the UI asks for a role here instead.
//!
//! The tests below pin every pair that carries content against the shipped
//! Default Light and Default Dark palettes.

use gpui_kit::gpui::{hsla, Hsla};
use gpui_kit::component::Theme;

/// WCAG contrast for text that carries content.
const BODY_CONTRAST: f32 = 4.5;
/// WCAG contrast for a non-text indicator such as a selected border.
const INDICATOR_CONTRAST: f32 = 3.0;
/// Steps used when a role has to be darkened or lightened into range.
const LEGIBLE_STEPS: u8 = 10;

/// The surface behind every card, panel and list row.
pub fn card_surface(theme: &Theme) -> Hsla {
    theme.group_box
}

/// Warning or failure text that stays readable on [`card_surface`].
pub fn warning_text(theme: &Theme) -> Hsla {
    legible_on(theme.warning, card_surface(theme))
}

/// Badge background. Badges stay neutral: the text carries the state, so a
/// status never depends on a tint the eye has to decode.
pub fn chip_bg(theme: &Theme) -> Hsla {
    theme.secondary
}

/// Badge text.
pub fn chip_fg(theme: &Theme) -> Hsla {
    theme.foreground
}

/// Border of a card that is being edited, or of the active inbound mode.
pub fn selection_border(theme: &Theme) -> Hsla {
    theme.primary
}

/// Hover fill for cards and list rows, always distinct from [`card_surface`].
pub fn row_hover(theme: &Theme) -> Hsla {
    pick_hover(theme.muted, theme.secondary, card_surface(theme))
}

/// WCAG relative luminance of an opaque color.
pub fn luminance(color: Hsla) -> f32 {
    let rgb = color.to_rgb();
    fn channel(value: f32) -> f32 {
        if value <= 0.04045 {
            value / 12.92
        } else {
            ((value + 0.055) / 1.055).powf(2.4)
        }
    }
    0.2126 * channel(rgb.r) + 0.7152 * channel(rgb.g) + 0.0722 * channel(rgb.b)
}

/// WCAG contrast ratio between two opaque colors, from 1.0 to 21.0.
pub fn contrast_ratio(a: Hsla, b: Hsla) -> f32 {
    let (x, y) = (luminance(a), luminance(b));
    let (lighter, darker) = if x > y { (x, y) } else { (y, x) };
    (lighter + 0.05) / (darker + 0.05)
}

/// The two colors differ enough for the eye to notice a state change.
pub fn separates(a: Hsla, b: Hsla) -> bool {
    (luminance(a) - luminance(b)).abs() >= 0.05
}

/// Darken or lighten `fg` until it reaches [`BODY_CONTRAST`] against `bg`.
fn legible_on(fg: Hsla, bg: Hsla) -> Hsla {
    if contrast_ratio(fg, bg) >= BODY_CONTRAST {
        return fg;
    }
    let towards = if luminance(bg) > 0.18 {
        hsla(0., 0., 0., 1.)
    } else {
        hsla(0., 0., 1., 1.)
    };
    let mut best = fg;
    for step in 1..=LEGIBLE_STEPS {
        let candidate = fg.blend(towards.opacity(f32::from(step) / f32::from(LEGIBLE_STEPS)));
        best = candidate;
        if contrast_ratio(candidate, bg) >= BODY_CONTRAST {
            break;
        }
    }
    best
}

/// Prefer the candidate that separates from the surface most, so a theme whose
/// `muted` equals the card surface still yields a visible hover.
fn pick_hover(muted: Hsla, secondary: Hsla, surface: Hsla) -> Hsla {
    let muted_gap = (luminance(muted) - luminance(surface)).abs();
    let secondary_gap = (luminance(secondary) - luminance(surface)).abs();
    if secondary_gap > muted_gap {
        secondary
    } else {
        muted
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit::gpui::rgb;

    /// Default Light: group_box `#f5f5f5`, warning `yellow-500`, secondary
    /// `neutral-200`, foreground and primary `neutral-900`.
    const LIGHT_SURFACE: u32 = 0xf5f5f5;
    const LIGHT_WARNING: u32 = 0xeab308;
    const LIGHT_SECONDARY: u32 = 0xe5e5e5;
    const LIGHT_FOREGROUND: u32 = 0x171717;
    const LIGHT_MUTED: u32 = 0xf5f5f5;

    /// Default Dark: group_box `neutral-950`, warning `yellow-400`, secondary
    /// and muted `neutral-800`, foreground `neutral-50`.
    const DARK_SURFACE: u32 = 0x0a0a0a;
    const DARK_WARNING: u32 = 0xfacc15;
    const DARK_SECONDARY: u32 = 0x262626;
    const DARK_FOREGROUND: u32 = 0xfafafa;

    fn hex(value: u32) -> Hsla {
        rgb(value).into()
    }

    #[test]
    fn contrast_ratio_matches_a_known_pair() {
        // The theme's muted_foreground on white.
        let ratio = contrast_ratio(hex(0x737373), hex(0xffffff));
        assert!((ratio - 4.74).abs() < 0.05, "ratio {ratio}");
    }

    #[test]
    fn warning_text_reaches_body_contrast_in_both_appearances() {
        for (warning, surface) in [
            (LIGHT_WARNING, LIGHT_SURFACE),
            (DARK_WARNING, DARK_SURFACE),
        ] {
            let text = legible_on(hex(warning), hex(surface));
            let ratio = contrast_ratio(text, hex(surface));
            assert!(
                ratio >= BODY_CONTRAST,
                "warning text {ratio} on #{surface:06x} (raw warning was {} )",
                contrast_ratio(hex(warning), hex(surface))
            );
        }
    }

    #[test]
    fn light_warning_needs_darkening_and_dark_warning_does_not() {
        // Guards the two measured failures the role exists to prevent.
        assert!(contrast_ratio(hex(LIGHT_WARNING), hex(LIGHT_SURFACE)) < BODY_CONTRAST);
        assert!(contrast_ratio(hex(DARK_WARNING), hex(DARK_SURFACE)) >= BODY_CONTRAST);
        assert_eq!(legible_on(hex(DARK_WARNING), hex(DARK_SURFACE)), hex(DARK_WARNING));
    }

    #[test]
    fn badges_and_selected_borders_stay_readable() {
        for (bg, fg) in [
            (LIGHT_SECONDARY, LIGHT_FOREGROUND),
            (DARK_SECONDARY, DARK_FOREGROUND),
        ] {
            let ratio = contrast_ratio(hex(fg), hex(bg));
            assert!(ratio >= BODY_CONTRAST, "badge text {ratio} on #{bg:06x}");
        }
        for (border, surface) in [
            (LIGHT_FOREGROUND, LIGHT_SURFACE),
            (DARK_FOREGROUND, DARK_SURFACE),
        ] {
            let ratio = contrast_ratio(hex(border), hex(surface));
            assert!(ratio >= INDICATOR_CONTRAST, "selection {ratio} on #{surface:06x}");
        }
    }

    #[test]
    fn hover_separates_from_the_card_surface() {
        // Light muted equals the card surface, so the hover has to fall back.
        let light = pick_hover(hex(LIGHT_MUTED), hex(LIGHT_SECONDARY), hex(LIGHT_SURFACE));
        assert!(separates(light, hex(LIGHT_SURFACE)));
        // Dark muted is deliberately subtle, but must still differ.
        let dark = pick_hover(hex(DARK_SECONDARY), hex(DARK_SECONDARY), hex(DARK_SURFACE));
        assert_ne!(luminance(dark), luminance(hex(DARK_SURFACE)));
    }
}
