//! Theme-derived styling for the configurable footer statusline.

use ratatui::prelude::Stylize;
use ratatui::style::Color;
use ratatui::style::Style;
use ratatui::text::Line;
use ratatui::text::Span;

use super::status_line_setup::StatusLineItem;
use crate::render::highlight::foreground_style_for_scopes;
use crate::style::accent_color;

const STATUS_LINE_SEPARATOR: &str = " · ";
const STATUS_LINE_COLOR_SATURATION_PERCENT: u16 = 85;
const STATUS_LINE_COLOR_BRIGHTNESS_PERCENT: u16 = 100;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum StatusLineAccent {
    Model,
    Path,
    Branch,
    State,
    Usage,
    Limit,
    Metadata,
    Mode,
    Thread,
    Progress,
    Goal,
}

impl StatusLineAccent {
    fn for_item(item: StatusLineItem) -> Self {
        match item {
            StatusLineItem::ModelName
            | StatusLineItem::ModelWithReasoning
            | StatusLineItem::Reasoning => Self::Model,
            StatusLineItem::CurrentDir | StatusLineItem::ProjectRoot => Self::Path,
            StatusLineItem::GitBranch
            | StatusLineItem::PullRequestNumber
            | StatusLineItem::BranchChanges => Self::Branch,
            StatusLineItem::Status | StatusLineItem::ScheduleCountdown => Self::State,
            StatusLineItem::ContextRemaining
            | StatusLineItem::ContextUsed
            | StatusLineItem::ContextWindowSize
            | StatusLineItem::UsedTokens
            | StatusLineItem::TotalInputTokens
            | StatusLineItem::TotalOutputTokens => Self::Usage,
            StatusLineItem::FiveHourLimit | StatusLineItem::WeeklyLimit => Self::Limit,
            StatusLineItem::CodexVersion | StatusLineItem::SessionId => Self::Metadata,
            StatusLineItem::FastMode | StatusLineItem::RawOutput | StatusLineItem::AuthProfile => {
                Self::Mode
            }
            StatusLineItem::Permissions => Self::Mode,
            StatusLineItem::ApprovalMode => Self::Mode,
            StatusLineItem::ThreadTitle
            | StatusLineItem::GoalTitle
            | StatusLineItem::ActiveAgent => Self::Thread,
            StatusLineItem::TaskProgress => Self::Progress,
        }
    }

    fn scopes(self) -> &'static [&'static str] {
        match self {
            Self::Model => &["entity.name.type", "support.type", "variable"],
            Self::Path => &["string", "markup.underline.link"],
            Self::Branch => &["entity.name.function", "entity.name.tag"],
            Self::State => &["keyword.control", "keyword"],
            Self::Usage => &["constant.numeric", "constant"],
            Self::Limit => &["constant.language", "storage.type"],
            Self::Metadata => &["comment", "constant.other"],
            Self::Mode => &["storage.modifier", "keyword.operator"],
            Self::Thread => &["markup.heading", "entity.name.section"],
            Self::Progress => &["markup.inserted", "constant.numeric"],
            // Deliberately different scope set from `Thread` so the locked goal segment resolves to
            // a different theme color than the session/thread title it renders next to.
            Self::Goal => &["markup.inserted", "string", "constant.numeric"],
        }
    }

    fn fallback_style(self) -> Style {
        match self {
            Self::Model | Self::State | Self::Metadata | Self::Mode => {
                Style::default().fg(accent_color())
            }
            Self::Path | Self::Usage | Self::Progress | Self::Goal => Style::default().green(),
            Self::Branch | Self::Limit | Self::Thread => Style::default().magenta(),
        }
    }
}

pub(crate) fn status_line_from_segments<I>(
    segments: I,
    use_theme_colors: bool,
) -> Option<Line<'static>>
where
    I: IntoIterator<Item = (StatusLineItem, String)>,
{
    status_line_from_segments_with_resolver(segments, use_theme_colors, |accent| {
        foreground_style_for_scopes(accent.scopes())
    })
}

fn status_line_from_segments_with_resolver<I, F>(
    segments: I,
    use_theme_colors: bool,
    theme_style_for_accent: F,
) -> Option<Line<'static>>
where
    I: IntoIterator<Item = (StatusLineItem, String)>,
    F: Fn(StatusLineAccent) -> Option<Style>,
{
    let mut spans = Vec::new();
    for (item, text) in segments {
        if !spans.is_empty() {
            spans.push(STATUS_LINE_SEPARATOR.dim());
        }
        let style = if use_theme_colors {
            let accent = StatusLineAccent::for_item(item);
            soften_status_line_style(
                theme_style_for_accent(accent).unwrap_or_else(|| accent.fallback_style()),
            )
        } else {
            Style::default().dim()
        };
        let style = if item == StatusLineItem::PullRequestNumber {
            style.underlined()
        } else {
            style
        };
        spans.push(Span::styled(text, style));
    }

    (!spans.is_empty()).then(|| Line::from(spans))
}

/// Text style for the locked, always-on goal-pursuit segment appended inline to the status line.
///
/// The goal-pursuit indicator ("Pursuing goal N/M (…)") is not a configurable `/statusline` item,
/// so it does not flow through [`StatusLineAccent::for_item`]. It is styled here with the
/// dedicated [`StatusLineAccent::Goal`] category, which is deliberately DISTINCT from
/// [`StatusLineItem::ThreadTitle`]'s `Thread` accent so the goal text does not blur into the
/// session/thread title when both render inline on the same row. It is theme-aware and softened
/// for readability exactly like the other themed segments; the surrounding ` · ` separators stay
/// dim (applied by the caller).
pub(crate) fn goal_status_line_style() -> Style {
    status_line_accent_style(StatusLineAccent::Goal)
}

/// Resolves the softened, theme-derived text style for a single accent, falling back to the
/// accent's built-in color when the active theme does not define a matching scope.
fn status_line_accent_style(accent: StatusLineAccent) -> Style {
    soften_status_line_style(
        foreground_style_for_scopes(accent.scopes()).unwrap_or_else(|| accent.fallback_style()),
    )
}

fn soften_status_line_style(mut style: Style) -> Style {
    if let Some(fg) = style.fg {
        style.fg = Some(soften_status_line_color(fg));
    }
    style
}

#[allow(clippy::disallowed_methods)]
fn soften_status_line_color(color: Color) -> Color {
    match color {
        Color::Rgb(r, g, b) if is_blue_or_cyan_rgb(r, g, b) => softened_status_line_accent_color(),
        Color::Rgb(r, g, b) => soften_status_line_rgb_color(r, g, b),
        Color::LightRed => Color::Red,
        Color::LightGreen => Color::Green,
        Color::LightYellow => Color::Yellow,
        Color::LightBlue => softened_status_line_accent_color(),
        Color::LightMagenta => Color::Magenta,
        Color::LightCyan => softened_status_line_accent_color(),
        Color::White => Color::Gray,
        Color::Blue | Color::Cyan => softened_status_line_accent_color(),
        Color::Indexed(index) if indexed_color_is_blue_or_cyan(index) => {
            softened_status_line_accent_color()
        }
        Color::Reset
        | Color::Black
        | Color::Red
        | Color::Green
        | Color::Yellow
        | Color::Magenta
        | Color::Gray
        | Color::DarkGray
        | Color::Indexed(_) => color,
    }
}

fn softened_status_line_accent_color() -> Color {
    match accent_color() {
        Color::Rgb(r, g, b) => soften_status_line_rgb_color(r, g, b),
        color => color,
    }
}

#[allow(clippy::disallowed_methods)]
fn soften_status_line_rgb_color(r: u8, g: u8, b: u8) -> Color {
    let luma = weighted_luma(r, g, b);
    Color::Rgb(
        soften_rgb_channel(r, luma),
        soften_rgb_channel(g, luma),
        soften_rgb_channel(b, luma),
    )
}

fn indexed_color_is_blue_or_cyan(index: u8) -> bool {
    if matches!(index, 4 | 6 | 12 | 14) {
        return true;
    }

    let Some((r, g, b)) = xterm_256_rgb(index) else {
        return false;
    };
    is_blue_or_cyan_rgb(r, g, b)
}

fn xterm_256_rgb(index: u8) -> Option<(u8, u8, u8)> {
    let index = index.checked_sub(16)?;
    if index >= 216 {
        return None;
    }

    let r = index / 36;
    let g = (index % 36) / 6;
    let b = index % 6;
    Some((
        xterm_256_channel(r),
        xterm_256_channel(g),
        xterm_256_channel(b),
    ))
}

fn xterm_256_channel(value: u8) -> u8 {
    if value == 0 { 0 } else { 55 + value * 40 }
}

fn is_blue_or_cyan_rgb(r: u8, g: u8, b: u8) -> bool {
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    if max.saturating_sub(min) < 40 {
        return false;
    }

    let blue = b >= r.saturating_add(30) && b >= g.saturating_add(20);
    let cyan = g >= r.saturating_add(30) && b >= r.saturating_add(30) && g.abs_diff(b) <= 40;
    blue || cyan
}

fn weighted_luma(r: u8, g: u8, b: u8) -> u16 {
    (77 * u16::from(r) + 150 * u16::from(g) + 29 * u16::from(b)) / 256
}

fn soften_rgb_channel(channel: u8, luma: u16) -> u8 {
    let channel = u16::from(channel);
    let softened = (channel * STATUS_LINE_COLOR_SATURATION_PERCENT
        + luma * (100 - STATUS_LINE_COLOR_SATURATION_PERCENT)
        + 50)
        / 100;

    ((softened * STATUS_LINE_COLOR_BRIGHTNESS_PERCENT + 50) / 100) as u8
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use ratatui::style::Modifier;

    fn line_text(line: &Line<'static>) -> String {
        line.spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect::<String>()
    }

    #[test]
    fn status_line_segments_preserve_order_and_plain_text() {
        let line = status_line_from_segments_with_resolver(
            [
                (StatusLineItem::ModelName, "gpt-5".to_string()),
                (StatusLineItem::CurrentDir, "/repo".to_string()),
                (StatusLineItem::GitBranch, "main".to_string()),
            ],
            /*use_theme_colors*/ true,
            |_| None,
        )
        .expect("status line");

        assert_eq!(line_text(&line), "gpt-5 · /repo · main");
        assert_eq!(
            line.spans[0].style.fg,
            Some(soften_status_line_color(accent_color()))
        );
        assert!(!line.spans[0].style.add_modifier.contains(Modifier::DIM));
        assert_eq!(line.spans[2].style.fg, Some(Color::Green));
        assert!(!line.spans[2].style.add_modifier.contains(Modifier::DIM));
        assert_eq!(line.spans[4].style.fg, Some(Color::Magenta));
        assert!(!line.spans[4].style.add_modifier.contains(Modifier::DIM));
    }

    #[test]
    fn status_line_segments_dim_separators_and_use_theme_styles_first() {
        let line = status_line_from_segments_with_resolver(
            [
                (StatusLineItem::ModelName, "gpt-5".to_string()),
                (StatusLineItem::ContextUsed, "Context 12% used".to_string()),
            ],
            /*use_theme_colors*/ true,
            |accent| match accent {
                StatusLineAccent::Model => Some(Style::default().red()),
                _ => None,
            },
        )
        .expect("status line");

        assert_eq!(line.spans[0].style.fg, Some(Color::Red));
        assert!(!line.spans[0].style.add_modifier.contains(Modifier::DIM));
        assert!(line.spans[1].style.add_modifier.contains(Modifier::DIM));
        assert_eq!(line.spans[2].style.fg, Some(Color::Green));
        assert!(!line.spans[2].style.add_modifier.contains(Modifier::DIM));
    }

    #[test]
    #[allow(clippy::disallowed_methods)]
    fn status_line_segments_soften_rgb_theme_styles_without_dimming_text() {
        let line = status_line_from_segments_with_resolver(
            [(StatusLineItem::ModelName, "gpt-5".to_string())],
            /*use_theme_colors*/ true,
            |_| Some(Style::default().fg(Color::Rgb(255, 0, 0))),
        )
        .expect("status line");

        assert_eq!(line.spans[0].style.fg, Some(Color::Rgb(228, 11, 11)));
        assert!(!line.spans[0].style.add_modifier.contains(Modifier::DIM));
    }

    #[test]
    #[allow(clippy::disallowed_methods)]
    fn status_line_theme_blue_and_cyan_colors_use_accent() {
        let expected = softened_status_line_accent_color();

        for color in [
            Color::Blue,
            Color::LightBlue,
            Color::Cyan,
            Color::LightCyan,
            Color::Rgb(0, 0, 95),
            Color::Rgb(0, 95, 95),
            Color::Rgb(3, 102, 214),
            Color::Rgb(42, 161, 152),
        ] {
            assert_eq!(soften_status_line_color(color), expected);
        }
    }

    #[test]
    #[allow(clippy::disallowed_methods)]
    fn status_line_indexed_blue_and_cyan_colors_use_accent() {
        let expected = softened_status_line_accent_color();

        for color in [
            Color::Indexed(4),
            Color::Indexed(6),
            Color::Indexed(12),
            Color::Indexed(14),
            Color::Indexed(17),
            Color::Indexed(23),
            Color::Indexed(33),
            Color::Indexed(45),
        ] {
            assert_eq!(soften_status_line_color(color), expected);
        }
    }

    #[test]
    #[allow(clippy::disallowed_methods)]
    fn status_line_saturated_green_does_not_count_as_cyan() {
        let color = Color::Rgb(0, 255, 128);

        assert_eq!(
            soften_status_line_color(color),
            soften_status_line_rgb_color(/*r*/ 0, /*g*/ 255, /*b*/ 128)
        );
    }

    #[test]
    fn status_line_segments_can_disable_theme_colors() {
        let line = status_line_from_segments_with_resolver(
            [
                (StatusLineItem::ModelName, "gpt-5".to_string()),
                (StatusLineItem::ContextUsed, "Context 12% used".to_string()),
            ],
            /*use_theme_colors*/ false,
            |_| Some(Style::default().red()),
        )
        .expect("status line");

        assert_eq!(line_text(&line), "gpt-5 · Context 12% used");
        assert_eq!(line.spans[0].style.fg, None);
        assert!(line.spans[0].style.add_modifier.contains(Modifier::DIM));
        assert!(line.spans[1].style.add_modifier.contains(Modifier::DIM));
        assert_eq!(line.spans[2].style.fg, None);
        assert!(line.spans[2].style.add_modifier.contains(Modifier::DIM));
    }

    #[test]
    fn pull_request_number_uses_link_style() {
        let line = status_line_from_segments_with_resolver(
            [(StatusLineItem::PullRequestNumber, "PR #20252".to_string())],
            /*use_theme_colors*/ false,
            |_| None,
        )
        .expect("status line");

        assert_eq!(line.spans[0].style.fg, None);
        assert!(line.spans[0].style.add_modifier.contains(Modifier::DIM));
        assert!(
            line.spans[0]
                .style
                .add_modifier
                .contains(Modifier::UNDERLINED)
        );
    }

    #[test]
    fn status_line_segments_return_none_when_empty() {
        assert_eq!(
            status_line_from_segments_with_resolver(
                Vec::<(StatusLineItem, String)>::new(),
                /*use_theme_colors*/ true,
                |_| None,
            ),
            None
        );
    }

    #[test]
    fn goal_status_line_style_differs_from_thread_title_style() {
        // The locked goal segment renders next to the session/thread title, so its color must not
        // collapse into the thread title's `Self::Thread` accent (both previously magenta).
        let goal = goal_status_line_style();
        let thread = status_line_accent_style(StatusLineAccent::Thread);
        assert_ne!(
            goal.fg, thread.fg,
            "goal segment must use a color distinct from the thread-title segment"
        );
        // The thread-title accent maps `ThreadTitle` (and `GoalTitle`) to `Self::Thread`, so guard
        // against a future regression that would route the goal color through it again.
        assert_ne!(
            goal.fg,
            status_line_accent_style(StatusLineAccent::for_item(StatusLineItem::ThreadTitle)).fg,
        );
    }
}
