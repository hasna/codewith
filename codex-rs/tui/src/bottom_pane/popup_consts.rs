//! Shared popup-related constants for bottom pane widgets.

use ratatui::text::Line;
use ratatui::text::Span;

use crate::key_hint;
use crate::key_hint::KeyBinding;
use crate::keymap::ListKeymap;
use crate::keymap::primary_binding;
use crossterm::event::KeyCode;

/// Maximum number of rows any popup should attempt to display.
/// Keep this consistent across all popups for a uniform feel.
pub(crate) const MAX_POPUP_ROWS: usize = 8;

/// Standard footer hint text used by popups.
pub(crate) fn standard_popup_hint_line() -> Line<'static> {
    Line::from(vec![
        "Press ".into(),
        key_hint::plain(KeyCode::Enter).into(),
        " to confirm or ".into(),
        key_hint::plain(KeyCode::Esc).into(),
        " to go back".into(),
    ])
}

pub(crate) fn standard_popup_hint_line_for_keymap(list_keymap: &ListKeymap) -> Line<'static> {
    accept_cancel_hint_line(
        primary_binding(&list_keymap.accept),
        "to confirm",
        primary_binding(&list_keymap.cancel),
        "to go back",
    )
}

/// Footer hint for a menu that participates in tree navigation (see
/// `SelectionViewParams::tree_navigation_enabled`).
///
/// Key labels are resolved from the live list keymap so rebound keys stay accurate; only the
/// verbs describing what each key does at this level of the tree are supplied by the caller.
pub(crate) struct TreeNavigationHint {
    /// Verb for the drill-in keys, e.g. "opens" for a menu whose rows open a sub-menu or
    /// "selects" for a leaf picker.
    pub accept_label: &'static str,
    /// Whether the drill-in hint should advertise `move_right`. Leaf pickers have no children to
    /// drill into, so they only advertise `accept`.
    pub include_move_right: bool,
    /// Whether the menu has toggle rows driven by space.
    pub include_space_toggle: bool,
    /// Verb for the go-back keys, e.g. "closes" at the root or "goes back" one level down.
    pub cancel_label: &'static str,
}

pub(crate) fn tree_navigation_hint_line(
    list_keymap: &ListKeymap,
    hint: TreeNavigationHint,
) -> Line<'static> {
    let mut spans: Vec<Span<'static>> = Vec::new();

    let accept_keys: Vec<KeyBinding> = hint
        .include_move_right
        .then(|| primary_binding(&list_keymap.move_right))
        .flatten()
        .into_iter()
        .chain(primary_binding(&list_keymap.accept))
        .collect();
    push_key_clause(&mut spans, &accept_keys, hint.accept_label);

    if hint.include_space_toggle {
        push_key_clause(
            &mut spans,
            &[key_hint::plain(KeyCode::Char(' '))],
            "toggles",
        );
    }

    let cancel_keys: Vec<KeyBinding> = primary_binding(&list_keymap.move_left)
        .into_iter()
        .chain(primary_binding(&list_keymap.cancel))
        .collect();
    push_key_clause(&mut spans, &cancel_keys, hint.cancel_label);

    Line::from(spans)
}

/// Append `<key>[ or <key>] <label>` to `spans`, separating clauses with `; `.
fn push_key_clause(spans: &mut Vec<Span<'static>>, keys: &[KeyBinding], label: &str) {
    if keys.is_empty() {
        return;
    }
    if !spans.is_empty() {
        spans.push("; ".into());
    }
    for (idx, key) in keys.iter().enumerate() {
        if idx > 0 {
            spans.push(" or ".into());
        }
        spans.push((*key).into());
    }
    spans.push(format!(" {label}").into());
}

pub(crate) fn accept_cancel_hint_line(
    accept: Option<KeyBinding>,
    accept_label: &'static str,
    cancel: Option<KeyBinding>,
    cancel_label: &'static str,
) -> Line<'static> {
    match (accept, cancel) {
        (Some(accept), Some(cancel)) => Line::from(vec![
            "Press ".into(),
            accept.into(),
            format!(" {accept_label} or ").into(),
            cancel.into(),
            format!(" {cancel_label}").into(),
        ]),
        (Some(accept), None) => Line::from(vec![
            "Press ".into(),
            accept.into(),
            format!(" {accept_label}").into(),
        ]),
        (None, Some(cancel)) => Line::from(vec![
            "Press ".into(),
            cancel.into(),
            format!(" {cancel_label}").into(),
        ]),
        (None, None) => Line::from(""),
    }
}
