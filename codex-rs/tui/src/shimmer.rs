use std::sync::OnceLock;
use std::time::Duration;
use std::time::Instant;

use ratatui::style::Color;
use ratatui::style::Modifier;
use ratatui::style::Style;
use ratatui::text::Span;

use crate::color::blend;
use crate::color::is_light;
use crate::style::accent_color;
use crate::style::accent_rgb;
use crate::terminal_palette::default_bg;

static PROCESS_START: OnceLock<Instant> = OnceLock::new();

fn elapsed_since_start() -> Duration {
    let start = PROCESS_START.get_or_init(Instant::now);
    start.elapsed()
}

pub(crate) fn shimmer_spans(text: &str) -> Vec<Span<'static>> {
    let chars: Vec<char> = text.chars().collect();
    if chars.is_empty() {
        return Vec::new();
    }
    // Use time-based sweep synchronized to process start.
    let padding = 10usize;
    let period = chars.len() + padding * 2;
    let sweep_seconds = 2.0f32;
    let pos_f =
        (elapsed_since_start().as_secs_f32() % sweep_seconds) / sweep_seconds * (period as f32);
    let pos = pos_f as usize;
    let has_true_color = supports_color::on_cached(supports_color::Stream::Stdout)
        .map(|level| level.has_16m)
        .unwrap_or(false);
    let band_half_width = 5.0;

    let mut spans: Vec<Span<'static>> = Vec::with_capacity(chars.len());
    let base_color = accent_rgb();
    let highlight_color = shimmer_highlight_rgb(base_color, default_bg());
    for (i, ch) in chars.iter().enumerate() {
        let i_pos = i as isize + padding as isize;
        let pos = pos as isize;
        let dist = (i_pos - pos).abs() as f32;

        let t = if dist <= band_half_width {
            let x = std::f32::consts::PI * (dist / band_half_width);
            0.5 * (1.0 + x.cos())
        } else {
            0.0
        };
        let style = if has_true_color {
            let highlight = t.clamp(0.0, 1.0);
            let (r, g, b) = blend(highlight_color, base_color, highlight * 0.9);
            // Allow custom RGB colors, as the implementation is thoughtfully
            // adjusting the level of the default foreground color.
            #[allow(clippy::disallowed_methods)]
            {
                Style::default()
                    .fg(Color::Rgb(r, g, b))
                    .add_modifier(Modifier::BOLD)
            }
        } else {
            fallback_style_for_level(t)
        };
        spans.push(Span::styled(ch.to_string(), style));
    }
    spans
}

fn shimmer_highlight_rgb(
    base_color: (u8, u8, u8),
    terminal_bg: Option<(u8, u8, u8)>,
) -> (u8, u8, u8) {
    match terminal_bg {
        Some(bg) if is_light(bg) => blend((0, 0, 0), base_color, 0.20),
        _ => blend((255, 255, 255), base_color, 0.35),
    }
}

fn fallback_style_for_level(intensity: f32) -> Style {
    // Tune fallback styling so the shimmer band reads even without RGB support.
    if intensity < 0.2 {
        Style::default()
            .fg(accent_color())
            .add_modifier(Modifier::DIM)
    } else if intensity < 0.6 {
        Style::default().fg(accent_color())
    } else {
        Style::default()
            .fg(accent_color())
            .add_modifier(Modifier::BOLD)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn shimmer_highlight_stays_in_emerald_family() {
        assert_eq!(accent_rgb(), (5, 150, 105));
        assert_eq!(
            shimmer_highlight_rgb(accent_rgb(), Some((0, 0, 0))),
            (92, 186, 157)
        );
        assert_eq!(
            shimmer_highlight_rgb(accent_rgb(), Some((255, 255, 255))),
            (4, 120, 84)
        );
    }

    #[test]
    fn fallback_shimmer_uses_app_accent_color() {
        assert_eq!(fallback_style_for_level(0.0).fg, Some(accent_color()));
        assert_eq!(fallback_style_for_level(0.4).fg, Some(accent_color()));
        assert_eq!(fallback_style_for_level(1.0).fg, Some(accent_color()));
        assert!(
            fallback_style_for_level(0.0)
                .add_modifier
                .contains(Modifier::DIM)
        );
        assert!(
            fallback_style_for_level(1.0)
                .add_modifier
                .contains(Modifier::BOLD)
        );
    }
}
