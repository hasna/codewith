use codex_tools::AdditionalProperties;
use codex_tools::JsonSchema;
use std::collections::BTreeMap;

#[test]
fn json_schema_preserves_legacy_exhaustive_struct_construction() {
    let schema = JsonSchema {
        schema_ref: None,
        schema_type: None,
        description: None,
        encrypted: None,
        enum_values: None,
        items: None,
        properties: None,
        required: None,
        additional_properties: Some(AdditionalProperties::Boolean(false)),
        any_of: None,
        defs: Some(BTreeMap::new()),
        definitions: Some(BTreeMap::new()),
    };

    assert_eq!(
        serde_json::to_value(schema).expect("legacy schema should serialize"),
        serde_json::json!({
            "additionalProperties": false,
            "$defs": {},
            "definitions": {},
        })
    );
}
