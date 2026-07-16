use crate::config_toml::ConfigToml;
use crate::types::RawMcpServerConfig;
use codex_features::FEATURES;
use codex_features::legacy_feature_keys;
use schemars::Schema;
use schemars::SchemaGenerator;
use schemars::generate::SchemaSettings;
use serde_json::Map;
use serde_json::Value;
use std::path::Path;

/// Schema for the `[features]` map with known + legacy keys only.
pub fn features_schema(schema_gen: &mut SchemaGenerator) -> Schema {
    let mut properties = Map::new();
    for feature in FEATURES {
        if feature.id == codex_features::Feature::Artifact {
            continue;
        }
        if feature.id == codex_features::Feature::CodeMode {
            properties.insert(
                feature.key.to_string(),
                schema_gen.subschema_for::<codex_features::FeatureToml<
                    codex_features::CodeModeConfigToml,
                >>().to_value(),
            );
            continue;
        }
        if feature.id == codex_features::Feature::MultiAgentV2 {
            properties.insert(
                feature.key.to_string(),
                schema_gen.subschema_for::<codex_features::FeatureToml<
                    codex_features::MultiAgentV2ConfigToml,
                >>().to_value(),
            );
            continue;
        }
        if feature.id == codex_features::Feature::AppsMcpPathOverride {
            properties.insert(
                feature.key.to_string(),
                schema_gen.subschema_for::<codex_features::FeatureToml<
                    codex_features::AppsMcpPathOverrideConfigToml,
                >>().to_value(),
            );
            continue;
        }
        if feature.id == codex_features::Feature::NetworkProxy {
            properties.insert(
                feature.key.to_string(),
                schema_gen.subschema_for::<codex_features::FeatureToml<
                    codex_features::NetworkProxyConfigToml,
                >>().to_value(),
            );
            continue;
        }
        properties.insert(
            feature.key.to_string(),
            schema_gen.subschema_for::<bool>().to_value(),
        );
    }
    for legacy_key in legacy_feature_keys() {
        properties.insert(
            legacy_key.to_string(),
            schema_gen.subschema_for::<bool>().to_value(),
        );
    }
    let mut object = Map::new();
    object.insert("type".to_string(), Value::String("object".to_string()));
    object.insert("properties".to_string(), Value::Object(properties));
    object.insert("additionalProperties".to_string(), Value::Bool(false));
    object.into()
}

/// Schema for the `[mcp_servers]` map using the raw input shape.
pub fn mcp_servers_schema(schema_gen: &mut SchemaGenerator) -> Schema {
    let mut object = Map::new();
    object.insert("type".to_string(), Value::String("object".to_string()));
    object.insert(
        "additionalProperties".to_string(),
        schema_gen.subschema_for::<RawMcpServerConfig>().to_value(),
    );
    object.into()
}

/// Build the config schema for `config.toml`.
pub fn config_schema() -> Schema {
    let schema = SchemaSettings::draft07()
        .into_generator()
        .into_root_schema_for::<ConfigToml>();
    let mut value = schema.to_value();
    remove_optional_null_types(&mut value);
    restore_flattened_config_maps(&mut value);
    match Schema::try_from(value) {
        Ok(schema) => schema,
        Err(error) => panic!("config schema should remain a valid JSON schema: {error}"),
    }
}

fn restore_flattened_config_maps(value: &mut Value) {
    let Some(definitions) = value.get_mut("definitions").and_then(Value::as_object_mut) else {
        return;
    };

    set_additional_property_ref(definitions, "AgentsToml", "AgentRoleToml");
    set_additional_property_ref(definitions, "AppToolsConfig", "AppToolConfig");
    set_additional_property_ref(definitions, "AppsConfigToml", "AppConfig");
}

fn set_additional_property_ref(
    definitions: &mut Map<String, Value>,
    schema_name: &str,
    additional_property_schema_name: &str,
) {
    let Some(Value::Object(schema)) = definitions.get_mut(schema_name) else {
        return;
    };

    let mut additional_properties = Map::new();
    additional_properties.insert(
        "$ref".to_string(),
        Value::String(format!("#/definitions/{additional_property_schema_name}")),
    );
    schema.insert(
        "additionalProperties".to_string(),
        Value::Object(additional_properties),
    );
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

/// Canonicalize a JSON value by sorting its keys.
pub fn canonicalize(value: &Value) -> Value {
    match value {
        Value::Array(items) => Value::Array(items.iter().map(canonicalize).collect()),
        Value::Object(map) => {
            let mut entries: Vec<_> = map.iter().collect();
            entries.sort_by_key(|(key, _)| *key);
            let mut sorted = Map::with_capacity(map.len());
            for (key, child) in entries {
                sorted.insert(key.clone(), canonicalize(child));
            }
            Value::Object(sorted)
        }
        _ => value.clone(),
    }
}

/// Render the config schema as pretty-printed JSON.
pub fn config_schema_json() -> anyhow::Result<Vec<u8>> {
    let schema = config_schema();
    let value = serde_json::to_value(schema)?;
    let value = canonicalize(&value);
    let json = serde_json::to_vec_pretty(&value)?;
    Ok(json)
}

/// Write the config schema fixture to disk.
pub fn write_config_schema(out_path: &Path) -> anyhow::Result<()> {
    let json = config_schema_json()?;
    std::fs::write(out_path, json)?;
    Ok(())
}
