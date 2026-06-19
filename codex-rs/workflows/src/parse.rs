use serde_yaml::Value;

use crate::MAX_WORKFLOW_YAML_BYTES;
use crate::WorkflowSpec;
use crate::WorkflowSpecError;
use crate::WorkflowSpecResult;

pub fn parse_workflow_yaml(raw: &str) -> WorkflowSpecResult<WorkflowSpec> {
    if raw.len() > MAX_WORKFLOW_YAML_BYTES {
        return Err(WorkflowSpecError::DocumentTooLarge {
            bytes: raw.len(),
            max_bytes: MAX_WORKFLOW_YAML_BYTES,
        });
    }
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(WorkflowSpecError::EmptyDocument);
    }
    if trimmed.starts_with("```") || trimmed.contains("\n```") {
        return Err(WorkflowSpecError::MarkdownFence);
    }

    let value: Value = serde_yaml::from_str(raw).map_err(WorkflowSpecError::parse)?;
    reject_unsupported_yaml_features(&value, "$")?;

    let spec: WorkflowSpec = serde_yaml::from_value(value).map_err(WorkflowSpecError::parse)?;
    spec.validate()?;
    Ok(spec)
}

fn reject_unsupported_yaml_features(value: &Value, path: &str) -> WorkflowSpecResult<()> {
    match value {
        Value::Tagged(_) => Err(WorkflowSpecError::unsupported_yaml_feature(
            path,
            "custom YAML tags are not allowed",
        )),
        Value::Mapping(mapping) => {
            for (key, nested) in mapping {
                let Value::String(key) = key else {
                    return Err(WorkflowSpecError::unsupported_yaml_feature(
                        path,
                        "mapping keys must be strings",
                    ));
                };
                if key == "<<" {
                    return Err(WorkflowSpecError::unsupported_yaml_feature(
                        path,
                        "YAML merge keys are not allowed",
                    ));
                }
                let nested_path = format!("{path}.{key}");
                reject_unsupported_yaml_features(nested, &nested_path)?;
            }
            Ok(())
        }
        Value::Sequence(sequence) => {
            for (index, nested) in sequence.iter().enumerate() {
                let nested_path = format!("{path}[{index}]");
                reject_unsupported_yaml_features(nested, &nested_path)?;
            }
            Ok(())
        }
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => Ok(()),
    }
}
