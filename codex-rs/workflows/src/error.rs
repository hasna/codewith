use thiserror::Error;

pub type WorkflowSpecResult<T> = Result<T, WorkflowSpecError>;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum WorkflowSpecError {
    #[error("workflow YAML is empty")]
    EmptyDocument,
    #[error("workflow YAML is {bytes} bytes, which exceeds the {max_bytes} byte validation limit")]
    DocumentTooLarge { bytes: usize, max_bytes: usize },
    #[error("workflow YAML must be a raw YAML document; Markdown fences are not allowed")]
    MarkdownFence,
    #[error("failed to parse workflow YAML: {0}")]
    ParseYaml(String),
    #[error("workflow YAML uses unsupported YAML feature at {path}: {feature}")]
    UnsupportedYamlFeature { path: String, feature: String },
    #[error("workflow spec is invalid: {0}")]
    Invalid(String),
}

impl WorkflowSpecError {
    pub(crate) fn parse(source: impl std::fmt::Display) -> Self {
        Self::ParseYaml(source.to_string())
    }

    pub(crate) fn invalid(message: impl Into<String>) -> Self {
        Self::Invalid(message.into())
    }

    pub(crate) fn unsupported_yaml_feature(
        path: impl Into<String>,
        feature: impl Into<String>,
    ) -> Self {
        Self::UnsupportedYamlFeature {
            path: path.into(),
            feature: feature.into(),
        }
    }
}
