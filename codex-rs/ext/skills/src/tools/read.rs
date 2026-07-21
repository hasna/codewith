use codex_extension_api::FunctionCallError;
use codex_extension_api::ToolCall;
use codex_extension_api::ToolExecutor;
use codex_extension_api::ToolName;
use codex_extension_api::ToolOutput;
use codex_extension_api::ToolSpec;
use codex_tools::ToolExposure;
use schemars::JsonSchema;
use serde::Deserialize;
use serde::Serialize;

use crate::catalog::SkillPackageId;
use crate::catalog::SkillResourceId;
use crate::provider::SkillReadRequest;

use super::MAX_HANDLE_BYTES;
use super::MAX_OUTPUT_BYTES;
use super::SkillToolAuthority;
use super::SkillToolContext;
use super::bounded_text;
use super::json_output;
use super::parse_args;
use super::serialized_len;
use super::skill_function_tool;
use super::skill_tool_name;
use super::validate_handle;

const TOOL_NAME: &str = "read";
const MAX_CONTENT_BYTES: usize = 24 * 1024;

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ReadArgs {
    authority: SkillToolAuthority,
    package: String,
    resource: String,
}

#[derive(Debug, Eq, JsonSchema, PartialEq, Serialize)]
#[schemars(deny_unknown_fields)]
struct ReadResponse {
    authority: SkillToolAuthority,
    package: String,
    resource: String,
    contents: String,
    truncated: bool,
}

#[derive(Clone)]
pub(super) struct ReadTool {
    pub(super) context: SkillToolContext,
}

#[async_trait::async_trait]
impl ToolExecutor<ToolCall> for ReadTool {
    fn tool_name(&self) -> ToolName {
        skill_tool_name(TOOL_NAME)
    }

    fn spec(&self) -> ToolSpec {
        skill_function_tool::<ReadArgs, ReadResponse>(
            TOOL_NAME,
            "Read one resource from an available skill package. Pass the exact opaque authority, package, and resource identifiers returned by the skills catalog or skills.search; never convert them into local paths. The returned contents are bounded.",
        )
    }

    fn exposure(&self) -> ToolExposure {
        ToolExposure::DirectModelOnly
    }

    fn supports_parallel_tool_calls(&self) -> bool {
        true
    }

    async fn handle(&self, call: ToolCall) -> Result<Box<dyn ToolOutput>, FunctionCallError> {
        let args: ReadArgs = parse_args(&call)?;
        let authority = args.authority.to_authority()?;
        validate_handle("package", &args.package, MAX_HANDLE_BYTES)?;
        validate_handle("resource", &args.resource, MAX_HANDLE_BYTES)?;
        let snapshot = self.context.snapshot(&call.turn_id)?;
        if self
            .context
            .available_package(&snapshot, &authority, &args.package)
            .is_none()
        {
            return Err(FunctionCallError::RespondToModel(
                "skill package is not available from the requested authority in this turn"
                    .to_string(),
            ));
        }

        let requested_resource = SkillResourceId(args.resource);
        let routes = snapshot.routes.clone();
        let result = routes
            .read(SkillReadRequest {
                authority: authority.clone(),
                package: SkillPackageId(args.package.clone()),
                resource: requested_resource.clone(),
                host: snapshot.host.clone(),
            })
            .await
            .map_err(|_| {
                FunctionCallError::RespondToModel(
                    "skill provider could not read the requested resource".to_string(),
                )
            })?;
        if result.resource != requested_resource {
            return Err(FunctionCallError::Fatal(
                "skill provider returned a different resource".to_string(),
            ));
        }

        let (contents, truncated) = bounded_text(&result.contents, MAX_CONTENT_BYTES);
        let mut response = ReadResponse {
            authority: SkillToolAuthority::from_authority(&authority),
            package: args.package,
            resource: result.resource.0,
            contents,
            truncated,
        };
        while serialized_len(&response)? > MAX_OUTPUT_BYTES {
            let serialized_bytes = serialized_len(&response)?;
            let bytes_to_remove = serialized_bytes.saturating_sub(MAX_OUTPUT_BYTES).max(1);
            let next_max = response.contents.len().saturating_sub(bytes_to_remove);
            let (contents, _) = bounded_text(&response.contents, next_max);
            if contents.len() == response.contents.len() {
                return Err(FunctionCallError::Fatal(
                    "bounded skill read metadata exceeds the tool output limit".to_string(),
                ));
            }
            response.contents = contents;
            response.truncated = true;
        }

        json_output(&response)
    }
}
