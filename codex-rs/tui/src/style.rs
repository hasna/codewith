use crate::color::blend;
use crate::color::is_light;
use crate::terminal_palette::StdoutColorLevel;
use crate::terminal_palette::best_color;
use crate::terminal_palette::default_bg;
use crate::terminal_palette::default_fg;
use crate::terminal_palette::rgb_color;
use crate::terminal_palette::stdout_color_level;
use ratatui::style::Color;
use ratatui::style::Style;
use ratatui::style::Stylize;
use std::io::IsTerminal;

const CODEWITH_EMERALD_RGB: (u8, u8, u8) = (5, 150, 105);
const CODEWITH_LINK_EMERALD_RGB: (u8, u8, u8) = (4, 120, 87);
// Decorative table rules should remain visible without competing with cell content.
const TABLE_SEPARATOR_FG_ALPHA: f32 = 0.20;

pub fn user_message_style() -> Style {
    user_message_style_for(adaptive_default_bg())
}

pub(crate) fn promptbar_style() -> Style {
    promptbar_style_for(promptbar_default_bg())
}

pub fn proposed_plan_style() -> Style {
    proposed_plan_style_for(adaptive_default_bg())
}

/// Returns a low-contrast rule style for separators within markdown tables.
pub(crate) fn table_separator_style() -> Style {
    table_separator_style_for(default_fg(), default_bg(), stdout_color_level())
}

/// Returns the shared accent style for active or selected TUI controls.
pub(crate) fn accent_style() -> Style {
    accent_style_for(default_bg())
}

/// Returns the shared Codewith accent color.
pub(crate) fn accent_color() -> Color {
    accent_color_for_level(stdout_color_level())
}

/// Returns the shared Codewith accent color as RGB.
pub(crate) fn accent_rgb() -> (u8, u8, u8) {
    CODEWITH_EMERALD_RGB
}

/// Returns the shared Codewith accent style for link-like text.
pub(crate) fn accent_link_style() -> Style {
    Style::default().fg(accent_link_color()).underlined()
}

fn adaptive_default_bg() -> Option<(u8, u8, u8)> {
    default_bg()
        .or_else(terminal_theme_bg_hint)
        .or_else(crate::render::highlight::current_theme_background_rgb)
}

fn promptbar_default_bg() -> Option<(u8, u8, u8)> {
    promptbar_default_bg_from_vars(
        default_bg(),
        std::io::stdout().is_terminal(),
        std::env::var("TERM_THEME").ok().as_deref(),
        std::env::var("VSCODE_THEME").ok().as_deref(),
        std::env::var("ANSI_LIGHT").ok().as_deref(),
        std::env::var("COLORFGBG").ok().as_deref(),
    )
}

fn terminal_theme_bg_hint() -> Option<(u8, u8, u8)> {
    promptbar_default_bg_from_vars(
        /*default_bg*/ None,
        std::io::stdout().is_terminal(),
        std::env::var("TERM_THEME").ok().as_deref(),
        std::env::var("VSCODE_THEME").ok().as_deref(),
        std::env::var("ANSI_LIGHT").ok().as_deref(),
        std::env::var("COLORFGBG").ok().as_deref(),
    )
}

fn promptbar_default_bg_from_vars(
    default_bg: Option<(u8, u8, u8)>,
    stdout_is_terminal: bool,
    term_theme: Option<&str>,
    vscode_theme: Option<&str>,
    ansi_light: Option<&str>,
    colorfgbg: Option<&str>,
) -> Option<(u8, u8, u8)> {
    default_bg.or_else(|| {
        if stdout_is_terminal {
            terminal_theme_bg_hint_from_vars(term_theme, vscode_theme, ansi_light, colorfgbg)
        } else {
            None
        }
    })
}

fn terminal_theme_bg_hint_from_vars(
    term_theme: Option<&str>,
    vscode_theme: Option<&str>,
    ansi_light: Option<&str>,
    colorfgbg: Option<&str>,
) -> Option<(u8, u8, u8)> {
    if ansi_light.is_some_and(|value| matches!(value, "1" | "true" | "TRUE" | "yes" | "YES")) {
        return Some((255, 255, 255));
    }

    for value in [term_theme, vscode_theme].into_iter().flatten() {
        let value = value.to_ascii_lowercase();
        if value.contains("light") {
            return Some((255, 255, 255));
        }
        if value.contains("dark") {
            return Some((0, 0, 0));
        }
    }

    let bg_index = colorfgbg?
        .rsplit(';')
        .next()
        .and_then(|value| value.trim().parse::<u8>().ok())?;
    match bg_index {
        0..=6 | 8 => Some((0, 0, 0)),
        7 | 9..=15 => Some((255, 255, 255)),
        _ => None,
    }
}

/// Returns the style for a user-authored message using the provided terminal background.
pub fn user_message_style_for(terminal_bg: Option<(u8, u8, u8)>) -> Style {
    match terminal_bg {
        Some(bg) => Style::default().bg(user_message_bg(bg)),
        None => Style::default(),
    }
}

fn promptbar_style_for(terminal_bg: Option<(u8, u8, u8)>) -> Style {
    match terminal_bg {
        Some(bg) => Style::default().bg(promptbar_bg(bg)),
        None => Style::default(),
    }
}

fn promptbar_bg(terminal_bg: (u8, u8, u8)) -> Color {
    rgb_color(user_message_bg_rgb(terminal_bg))
}

pub fn proposed_plan_style_for(terminal_bg: Option<(u8, u8, u8)>) -> Style {
    match terminal_bg {
        Some(bg) => Style::default().bg(proposed_plan_bg(bg)),
        None => Style::default(),
    }
}

/// Returns the shared accent style for the provided terminal background.
pub(crate) fn accent_style_for(_terminal_bg: Option<(u8, u8, u8)>) -> Style {
    Style::default().fg(accent_color()).bold()
}

fn accent_color_for_level(color_level: StdoutColorLevel) -> Color {
    match color_level {
        StdoutColorLevel::TrueColor => rgb_color(CODEWITH_EMERALD_RGB),
        StdoutColorLevel::Ansi256 => best_color(CODEWITH_EMERALD_RGB),
        StdoutColorLevel::Ansi16 | StdoutColorLevel::Unknown => Color::Green,
    }
}

fn accent_link_color() -> Color {
    accent_link_color_for_level(stdout_color_level())
}

fn accent_link_color_for_level(color_level: StdoutColorLevel) -> Color {
    match color_level {
        StdoutColorLevel::TrueColor => rgb_color(CODEWITH_LINK_EMERALD_RGB),
        StdoutColorLevel::Ansi256 => best_color(CODEWITH_LINK_EMERALD_RGB),
        StdoutColorLevel::Ansi16 | StdoutColorLevel::Unknown => Color::Green,
    }
}

fn table_separator_style_for(
    terminal_fg: Option<(u8, u8, u8)>,
    terminal_bg: Option<(u8, u8, u8)>,
    color_level: StdoutColorLevel,
) -> Style {
    let (Some(fg), Some(bg)) = (terminal_fg, terminal_bg) else {
        return Style::default().dim();
    };
    let separator_rgb = blend(fg, bg, TABLE_SEPARATOR_FG_ALPHA);
    match color_level {
        StdoutColorLevel::TrueColor => Style::default().fg(rgb_color(separator_rgb)),
        StdoutColorLevel::Ansi256 => Style::default().fg(best_color(separator_rgb)),
        StdoutColorLevel::Ansi16 | StdoutColorLevel::Unknown => Style::default().dim(),
    }
}

#[allow(clippy::disallowed_methods)]
pub fn user_message_bg(terminal_bg: (u8, u8, u8)) -> Color {
    best_color(user_message_bg_rgb(terminal_bg))
}

fn user_message_bg_rgb(terminal_bg: (u8, u8, u8)) -> (u8, u8, u8) {
    let (top, alpha) = if is_light(terminal_bg) {
        ((0, 0, 0), 0.04)
    } else {
        ((255, 255, 255), 0.12)
    };
    blend(top, terminal_bg, alpha)
}

#[allow(clippy::disallowed_methods)]
pub fn proposed_plan_bg(terminal_bg: (u8, u8, u8)) -> Color {
    user_message_bg(terminal_bg)
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use ratatui::style::Modifier;

    #[test]
    fn accent_color_uses_codewith_emerald_or_green_fallback() {
        assert_eq!(
            accent_color_for_level(StdoutColorLevel::TrueColor),
            rgb_color(CODEWITH_EMERALD_RGB)
        );
        assert_eq!(accent_rgb(), CODEWITH_EMERALD_RGB);
        assert_eq!(
            accent_color_for_level(StdoutColorLevel::Ansi16),
            Color::Green
        );
        assert_eq!(
            accent_color_for_level(StdoutColorLevel::Unknown),
            Color::Green
        );
    }

    #[test]
    fn accent_style_uses_codewith_emerald_on_light_backgrounds() {
        let style = accent_style_for(Some((255, 255, 255)));

        assert_eq!(style.fg, Some(accent_color()));
        assert!(style.add_modifier.contains(Modifier::BOLD));
    }

    #[test]
    fn accent_style_uses_codewith_emerald_on_dark_or_unknown_backgrounds() {
        let expected = Style::default().fg(accent_color()).bold();

        assert_eq!(accent_style_for(Some((0, 0, 0))), expected);
        assert_eq!(accent_style_for(/*terminal_bg*/ None), expected);
    }

    #[test]
    fn accent_link_style_uses_darker_codewith_emerald_and_underline() {
        let expected = Style::default().fg(accent_link_color()).underlined();

        assert_eq!(
            accent_link_color_for_level(StdoutColorLevel::TrueColor),
            rgb_color(CODEWITH_LINK_EMERALD_RGB)
        );
        assert_eq!(accent_link_style(), expected);
    }

    #[test]
    fn terminal_theme_bg_hint_uses_explicit_light_markers() {
        assert_eq!(
            terminal_theme_bg_hint_from_vars(None, None, Some("1"), None),
            Some((255, 255, 255))
        );
        assert_eq!(
            terminal_theme_bg_hint_from_vars(Some("light"), None, None, None),
            Some((255, 255, 255))
        );
        assert_eq!(
            terminal_theme_bg_hint_from_vars(None, Some("Light Modern"), None, None),
            Some((255, 255, 255))
        );
    }

    #[test]
    fn promptbar_default_bg_ignores_theme_hints_without_terminal_stdout() {
        assert_eq!(
            promptbar_default_bg_from_vars(
                /*default_bg*/ None,
                /*stdout_is_terminal*/ false,
                Some("light"),
                Some("Light Modern"),
                Some("1"),
                Some("0;15"),
            ),
            None
        );
    }

    #[test]
    fn promptbar_default_bg_uses_light_theme_hints_with_terminal_stdout() {
        let default_bg = promptbar_default_bg_from_vars(
            /*default_bg*/ None,
            /*stdout_is_terminal*/ true,
            Some("light"),
            Some("Light Modern"),
            Some("1"),
            Some("0;15"),
        );

        assert_eq!(default_bg, Some((255, 255, 255)));
        assert_eq!(
            promptbar_style_for(default_bg).bg,
            Some(rgb_color((244, 244, 244)))
        );
    }

    #[test]
    fn terminal_theme_bg_hint_uses_dark_markers_and_colorfgbg() {
        assert_eq!(
            terminal_theme_bg_hint_from_vars(Some("dark"), None, None, None),
            Some((0, 0, 0))
        );
        assert_eq!(
            terminal_theme_bg_hint_from_vars(None, None, None, Some("15;0")),
            Some((0, 0, 0))
        );
        assert_eq!(
            terminal_theme_bg_hint_from_vars(None, None, None, Some("0;15")),
            Some((255, 255, 255))
        );
    }

    #[test]
    fn promptbar_style_uses_direct_rgb_background_for_known_light_theme() {
        assert_eq!(
            promptbar_style_for(Some((255, 255, 255))).bg,
            Some(rgb_color((244, 244, 244)))
        );
    }

    #[test]
    fn promptbar_style_uses_direct_rgb_background_for_known_dark_theme() {
        assert_eq!(
            promptbar_style_for(Some((0, 0, 0))).bg,
            Some(rgb_color((30, 30, 30)))
        );
    }

    #[test]
    fn promptbar_style_stays_default_without_background_evidence() {
        assert_eq!(promptbar_style_for(/*terminal_bg*/ None), Style::default());
    }

    #[test]
    fn promptbar_style_ignores_syntax_theme_background_without_terminal_evidence() {
        if std::io::stdout().is_terminal() {
            return;
        }

        assert_eq!(promptbar_style(), Style::default());
    }

    #[test]
    fn table_separator_blends_toward_dark_background() {
        let style = table_separator_style_for(
            Some((255, 255, 255)),
            Some((0, 0, 0)),
            StdoutColorLevel::TrueColor,
        );

        assert_eq!(style.fg, Some(rgb_color((51, 51, 51))));
    }

    #[test]
    fn table_separator_blends_toward_light_background() {
        let style = table_separator_style_for(
            Some((0, 0, 0)),
            Some((255, 255, 255)),
            StdoutColorLevel::TrueColor,
        );

        assert_eq!(style.fg, Some(rgb_color((204, 204, 204))));
    }

    #[test]
    fn table_separator_dims_when_palette_aware_color_is_unavailable() {
        let expected = Style::default().dim();

        assert_eq!(
            table_separator_style_for(
                Some((255, 255, 255)),
                Some((0, 0, 0)),
                StdoutColorLevel::Ansi16,
            ),
            expected
        );
        assert_eq!(
            table_separator_style_for(
                /*terminal_fg*/ None,
                Some((0, 0, 0)),
                StdoutColorLevel::TrueColor,
            ),
            expected
        );
    }
}
