use serde_json::Value;

const MAX_JSON_NESTING_DEPTH: usize = 64;

struct ByteBudget {
    bytes: usize,
    max_bytes: usize,
}

impl ByteBudget {
    fn spend(&mut self, bytes: usize) -> bool {
        let Some(total) = self.bytes.checked_add(bytes) else {
            return false;
        };
        if total > self.max_bytes {
            return false;
        }
        self.bytes = total;
        true
    }
}

enum Frame<'a> {
    Value(&'a Value, usize),
    Array(std::slice::Iter<'a, Value>, usize),
    Object(serde_json::map::Iter<'a>, usize),
}

fn spend_json_string(value: &str, budget: &mut ByteBudget) -> bool {
    if !budget.spend(2) {
        return false;
    }
    for character in value.chars() {
        let bytes = match character {
            '"' | '\\' | '\u{0008}' | '\t' | '\n' | '\u{000c}' | '\r' => 2,
            '\u{0000}'..='\u{001f}' => 6,
            character => character.len_utf8(),
        };
        if !budget.spend(bytes) {
            return false;
        }
    }
    true
}

/// Returns the exact compact serialized length when `value` fits.
///
/// Traversal stops at the byte budget and rejects excessive nesting, so callers
/// can validate untrusted values without recursively serializing the whole tree.
pub fn bounded_json_serialized_len(value: &Value, max_bytes: usize) -> Option<usize> {
    let mut budget = ByteBudget {
        bytes: 0,
        max_bytes,
    };
    let mut stack = vec![Frame::Value(value, /*depth*/ 0)];

    while let Some(frame) = stack.pop() {
        match frame {
            Frame::Value(_, depth) if depth > MAX_JSON_NESTING_DEPTH => return None,
            Frame::Value(Value::Null, _) => {
                if !budget.spend(4) {
                    return None;
                }
            }
            Frame::Value(Value::Bool(value), _) => {
                if !budget.spend(if *value { 4 } else { 5 }) {
                    return None;
                }
            }
            Frame::Value(Value::Number(value), _) => {
                if !budget.spend(value.to_string().len()) {
                    return None;
                }
            }
            Frame::Value(Value::String(value), _) => {
                if !spend_json_string(value, &mut budget) {
                    return None;
                }
            }
            Frame::Value(Value::Array(values), depth) => {
                let punctuation = 2usize.checked_add(values.len().saturating_sub(1))?;
                if !budget.spend(punctuation) {
                    return None;
                }
                stack.push(Frame::Array(values.iter(), depth + 1));
            }
            Frame::Value(Value::Object(values), depth) => {
                let separators = values.len().checked_mul(2)?.saturating_sub(1);
                let punctuation = 2usize.checked_add(separators)?;
                if !budget.spend(punctuation) {
                    return None;
                }
                stack.push(Frame::Object(values.iter(), depth + 1));
            }
            Frame::Array(mut values, depth) => {
                if let Some(value) = values.next() {
                    stack.push(Frame::Array(values, depth));
                    stack.push(Frame::Value(value, depth));
                }
            }
            Frame::Object(mut values, depth) => {
                if let Some((key, value)) = values.next() {
                    if !spend_json_string(key, &mut budget) {
                        return None;
                    }
                    stack.push(Frame::Object(values, depth));
                    stack.push(Frame::Value(value, depth));
                }
            }
        }
    }

    Some(budget.bytes)
}

#[cfg(test)]
#[path = "bounded_json_tests.rs"]
mod tests;
