use super::*;
use pretty_assertions::assert_eq;
use serde_json::json;

#[test]
fn returns_exact_length_only_within_budget() {
    let value = json!({
        "control\nkey": ["quote\"", "slash\\", "\u{0000}", "Zażółć"],
        "number": -12.5,
        "boolean": true,
    });
    let exact = serde_json::to_vec(&value).expect("serialize fixture").len();

    assert_eq!(bounded_json_serialized_len(&value, exact), Some(exact));
    assert_eq!(bounded_json_serialized_len(&value, exact - 1), None);
}

#[test]
fn rejects_huge_strings_at_the_byte_budget() {
    let value = Value::String("x".repeat(100_000));

    assert_eq!(bounded_json_serialized_len(&value, 128), None);
}

#[test]
fn rejects_excessive_nesting_even_with_a_large_byte_budget() {
    let mut value = Value::Null;
    for _ in 0..=MAX_JSON_NESTING_DEPTH {
        value = Value::Array(vec![value]);
    }

    assert_eq!(bounded_json_serialized_len(&value, usize::MAX), None);
}

#[test]
fn rejects_wide_values_before_visiting_every_child() {
    let value = Value::Array(vec![Value::Null; 100_000]);

    assert_eq!(bounded_json_serialized_len(&value, 128), None);
}
