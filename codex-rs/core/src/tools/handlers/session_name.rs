use crate::function_tool::FunctionCallError;
use crate::tools::context::FunctionToolOutput;
use crate::tools::context::ToolInvocation;
use crate::tools::context::ToolPayload;
use crate::tools::context::boxed_tool_output;
use crate::tools::handlers::parse_arguments;
use crate::tools::handlers::session_name_spec::RENAME_SESSION_TOOL_NAME;
use crate::tools::handlers::session_name_spec::create_rename_session_tool;
use crate::tools::registry::CoreToolRuntime;
use crate::tools::registry::ToolExecutor;
use codex_tools::ToolName;
use codex_tools::ToolSpec;
use serde::Deserialize;
use serde::Serialize;
use std::fmt::Write as _;

pub struct RenameSessionHandler;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
struct RenameSessionArgs {
    name: String,
    #[serde(default)]
    overwrite_existing: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct RenameSessionResponse {
    thread_name: String,
}

#[async_trait::async_trait]
impl ToolExecutor<ToolInvocation> for RenameSessionHandler {
    fn tool_name(&self) -> ToolName {
        ToolName::plain(RENAME_SESSION_TOOL_NAME)
    }

    fn spec(&self) -> ToolSpec {
        create_rename_session_tool()
    }

    async fn handle(
        &self,
        invocation: ToolInvocation,
    ) -> Result<Box<dyn crate::tools::context::ToolOutput>, FunctionCallError> {
        let ToolInvocation {
            session,
            turn,
            payload,
            ..
        } = invocation;

        let arguments = match payload {
            ToolPayload::Function { arguments } => arguments,
            _ => {
                return Err(FunctionCallError::RespondToModel(
                    "rename_session handler received unsupported payload".to_string(),
                ));
            }
        };

        let args: RenameSessionArgs = parse_arguments(&arguments)?;
        let thread_name = session
            .rename_thread_from_tool(turn.as_ref(), &args.name, args.overwrite_existing)
            .await
            .map_err(|err| FunctionCallError::RespondToModel(format_rename_error(err)))?;
        let response = serde_json::to_string_pretty(&RenameSessionResponse { thread_name })
            .map_err(|err| FunctionCallError::Fatal(err.to_string()))?;
        Ok(boxed_tool_output(FunctionToolOutput::from_text(
            response,
            Some(true),
        )))
    }
}

impl CoreToolRuntime for RenameSessionHandler {}

fn format_rename_error(err: anyhow::Error) -> String {
    let mut message = err.to_string();
    for cause in err.chain().skip(1) {
        let _ = write!(message, ": {cause}");
    }
    message
}
