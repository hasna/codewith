use std::collections::HashMap;

use codex_protocol::models::ResponseItem;

use super::normalize;

/// Returns the pair-safe prefix length nearest to the requested removal count.
///
/// Complete call/output pairs, including interleaved or duplicate identifiers, form one atomic
/// interval. The surviving suffix must also contain an item that prompt normalization will keep
/// without requiring an absent counterpart.
pub(super) fn oldest_pair_safe_removal_count(
    items: &[ResponseItem],
    requested_items: usize,
) -> usize {
    let item_count = items.len();
    if requested_items == 0 || item_count <= 1 {
        return 0;
    }

    let requested_items = requested_items.min(item_count - 1);
    let mut pair_bounds = HashMap::<HistoryPairKey<'_>, HistoryPairBounds>::new();
    for (index, item) in items.iter().enumerate() {
        let Some((key, side)) = history_pair_key(item) else {
            continue;
        };
        pair_bounds.entry(key).or_default().record(index, side);
    }

    // A boundary at index `n` removes items `0..n`. Mark every boundary crossed by a complete
    // pair as unsafe, including conservatively grouping duplicate call ids into one unit.
    let mut boundary_delta = vec![0isize; item_count + 1];
    for bounds in pair_bounds.values().filter(|bounds| bounds.is_complete()) {
        boundary_delta[bounds.first + 1] += 1;
        boundary_delta[bounds.last + 1] -= 1;
    }

    // Normalization drops orphan client-side outputs. Precompute whether each suffix retains any
    // logical unit so a pair-safe boundary cannot still produce an empty prompt.
    let mut suffix_has_logical_unit = vec![false; item_count + 1];
    for index in (0..item_count).rev() {
        suffix_has_logical_unit[index] = suffix_has_logical_unit[index + 1]
            || normalize::required_call_for_output(&items[index]).is_none();
    }

    let mut open_pairs = 0isize;
    let mut earlier_safe_boundary = None;
    for (boundary, delta) in boundary_delta.iter().enumerate().take(item_count).skip(1) {
        open_pairs += delta;
        if open_pairs != 0 || !suffix_has_logical_unit[boundary] {
            continue;
        }
        if boundary >= requested_items {
            return boundary;
        }
        earlier_safe_boundary = Some(boundary);
    }

    earlier_safe_boundary.unwrap_or(0)
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
enum HistoryPairKey<'a> {
    Function(&'a str),
    ToolSearch(&'a str),
    CustomTool(&'a str),
}

#[derive(Clone, Copy)]
enum HistoryPairSide {
    Call,
    Output,
}

#[derive(Default)]
struct HistoryPairBounds {
    first: usize,
    last: usize,
    has_item: bool,
    has_call: bool,
    has_output: bool,
}

impl HistoryPairBounds {
    fn record(&mut self, index: usize, side: HistoryPairSide) {
        if !self.has_item {
            self.first = index;
            self.has_item = true;
        }
        self.last = index;
        match side {
            HistoryPairSide::Call => self.has_call = true,
            HistoryPairSide::Output => self.has_output = true,
        }
    }

    fn is_complete(&self) -> bool {
        self.has_call && self.has_output
    }
}

fn history_pair_key(item: &ResponseItem) -> Option<(HistoryPairKey<'_>, HistoryPairSide)> {
    match item {
        ResponseItem::FunctionCall { call_id, .. }
        | ResponseItem::LocalShellCall {
            call_id: Some(call_id),
            ..
        } => Some((HistoryPairKey::Function(call_id), HistoryPairSide::Call)),
        ResponseItem::FunctionCallOutput { call_id, .. } => {
            Some((HistoryPairKey::Function(call_id), HistoryPairSide::Output))
        }
        ResponseItem::ToolSearchCall {
            call_id: Some(call_id),
            ..
        } => Some((HistoryPairKey::ToolSearch(call_id), HistoryPairSide::Call)),
        ResponseItem::ToolSearchOutput {
            call_id: Some(call_id),
            ..
        } => Some((HistoryPairKey::ToolSearch(call_id), HistoryPairSide::Output)),
        ResponseItem::CustomToolCall { call_id, .. } => {
            Some((HistoryPairKey::CustomTool(call_id), HistoryPairSide::Call))
        }
        ResponseItem::CustomToolCallOutput { call_id, .. } => {
            Some((HistoryPairKey::CustomTool(call_id), HistoryPairSide::Output))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use codex_protocol::models::ContentItem;
    use codex_protocol::models::FunctionCallOutputPayload;
    use pretty_assertions::assert_eq;

    use super::*;

    fn user(text: &str) -> ResponseItem {
        ResponseItem::Message {
            id: None,
            role: "user".to_string(),
            content: vec![ContentItem::InputText {
                text: text.to_string(),
            }],
            phase: None,
        }
    }

    fn call(call_id: &str) -> ResponseItem {
        ResponseItem::FunctionCall {
            id: None,
            name: "test".to_string(),
            namespace: None,
            arguments: "{}".to_string(),
            call_id: call_id.to_string(),
        }
    }

    fn output(call_id: &str) -> ResponseItem {
        ResponseItem::FunctionCallOutput {
            call_id: call_id.to_string(),
            output: FunctionCallOutputPayload::from_text("ok".to_string()),
        }
    }

    #[test]
    fn complete_pair_is_an_atomic_surviving_unit() {
        let items = vec![user("old"), call("last"), output("last")];

        assert_eq!(oldest_pair_safe_removal_count(&items, 1), 1);
        assert_eq!(oldest_pair_safe_removal_count(&items[1..], 1), 0);
    }

    #[test]
    fn interleaved_and_duplicate_ids_are_grouped_conservatively() {
        let interleaved = vec![
            call("a"),
            call("b"),
            output("a"),
            output("b"),
            user("survivor"),
        ];
        let duplicate = vec![
            call("same"),
            call("same"),
            output("same"),
            output("same"),
            user("survivor"),
        ];

        assert_eq!(oldest_pair_safe_removal_count(&interleaved, 1), 4);
        assert_eq!(oldest_pair_safe_removal_count(&duplicate, 1), 4);
    }

    #[test]
    fn orphan_output_cannot_be_the_only_survivor() {
        let items = vec![user("last logical unit"), output("orphan")];

        assert_eq!(oldest_pair_safe_removal_count(&items, 1), 0);
    }
}
