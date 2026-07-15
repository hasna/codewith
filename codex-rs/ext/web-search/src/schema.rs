use codex_api::SearchCommands;
use schemars::generate::SchemaSettings;
use serde_json::Map;
use serde_json::Value;

pub(crate) fn commands_schema() -> Value {
    let schema = SchemaSettings::draft2019_09()
        .with(|settings| {
            settings.inline_subschemas = true;
        })
        .into_generator()
        .into_root_schema_for::<SearchCommands>();
    let schema = match serde_json::to_value(schema) {
        Ok(schema) => schema,
        Err(err) => panic!("search commands schema should serialize: {err}"),
    };
    let mut schema = schema;
    remove_optional_null_types(&mut schema);
    let Value::Object(mut schema) = schema else {
        unreachable!("search commands schema must be an object");
    };

    let mut tool_schema = Map::new();
    for key in [
        "properties",
        "required",
        "type",
        "additionalProperties",
        "$defs",
        "definitions",
    ] {
        if let Some(value) = schema.remove(key) {
            tool_schema.insert(key.to_string(), value);
        }
    }
    Value::Object(tool_schema)
}

fn remove_optional_null_types(value: &mut Value) {
    match value {
        Value::Object(map) => {
            remove_null_union_variant(map.get_mut("anyOf"));
            remove_null_union_variant(map.get_mut("oneOf"));
            if let Some(type_value) = map.get_mut("type") {
                remove_null_type(type_value);
            }
            if let Some(enum_value) = map.get_mut("enum") {
                remove_null_enum_variant(enum_value);
            }
            for value in map.values_mut() {
                remove_optional_null_types(value);
            }
        }
        Value::Array(values) => {
            for value in values {
                remove_optional_null_types(value);
            }
        }
        _ => {}
    }
}

fn remove_null_union_variant(value: Option<&mut Value>) {
    let Some(Value::Array(values)) = value else {
        return;
    };

    values.retain(|value| {
        !matches!(
            value,
            Value::Object(object)
                if matches!(object.get("type"), Some(Value::String(value)) if value == "null")
        )
    });
}

fn remove_null_type(type_value: &mut Value) {
    let Value::Array(types) = type_value else {
        return;
    };

    let non_null_types = types
        .iter()
        .filter(|value| !matches!(value, Value::String(value) if value == "null"))
        .cloned()
        .collect::<Vec<_>>();

    if non_null_types.len() == types.len() || non_null_types.is_empty() {
        return;
    }

    if let [only_type] = non_null_types.as_slice() {
        *type_value = only_type.clone();
    } else {
        *types = non_null_types;
    }
}

fn remove_null_enum_variant(enum_value: &mut Value) {
    let Value::Array(values) = enum_value else {
        return;
    };

    values.retain(|value| !value.is_null());
}
