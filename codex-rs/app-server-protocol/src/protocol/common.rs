use std::path::Path;
use std::path::PathBuf;

use crate::JSONRPCNotification;
use crate::JSONRPCRequest;
use crate::RequestId;
use crate::export::GeneratedSchema;
use crate::export::write_json_schema;
use crate::protocol::v1;
use crate::protocol::v2;
use codex_experimental_api_macros::ExperimentalApi;
use schemars::JsonSchema;
use serde::Deserialize;
use serde::Serialize;
use strum_macros::Display;
use ts_rs::TS;

/// Authentication mode for OpenAI-backed providers.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Display, JsonSchema, TS)]
#[serde(rename_all = "lowercase")]
pub enum AuthMode {
    /// OpenAI API key provided by the caller and stored by Codewith.
    ApiKey,
    /// ChatGPT OAuth managed by Codewith (tokens persisted and refreshed by Codewith).
    Chatgpt,
    /// [UNSTABLE] FOR OPENAI INTERNAL USE ONLY - DO NOT USE.
    ///
    /// ChatGPT auth tokens are supplied by an external host app and are only
    /// stored in memory. Token refresh must be handled by the external host app.
    #[serde(rename = "chatgptAuthTokens")]
    #[ts(rename = "chatgptAuthTokens")]
    #[strum(serialize = "chatgptAuthTokens")]
    ChatgptAuthTokens,
    /// Programmatic Codewith auth backed by a registered Agent Identity.
    #[serde(rename = "agentIdentity")]
    #[ts(rename = "agentIdentity")]
    #[strum(serialize = "agentIdentity")]
    AgentIdentity,
    /// Programmatic Codex auth backed by a personal access token.
    #[serde(rename = "personalAccessToken")]
    #[ts(rename = "personalAccessToken")]
    #[strum(serialize = "personalAccessToken")]
    PersonalAccessToken,
}

impl AuthMode {
    /// Returns whether this mode represents an authenticated human ChatGPT account.
    pub fn has_chatgpt_account(self) -> bool {
        match self {
            Self::Chatgpt | Self::ChatgptAuthTokens | Self::PersonalAccessToken => true,
            Self::ApiKey | Self::AgentIdentity => false,
        }
    }
}

macro_rules! experimental_reason_expr {
    // If a request variant is explicitly marked experimental, that reason wins.
    (variant $variant:ident, #[experimental($reason:expr)] $params:ident $(, $inspect_params:tt)?) => {
        Some($reason)
    };
    // `inspect_params: true` is used when a method is mostly stable but needs
    // field-level gating from its params type (for example, ThreadStart).
    (variant $variant:ident, $params:ident, true) => {
        crate::experimental_api::ExperimentalApi::experimental_reason($params)
    };
    (variant $variant:ident, $params:ident $(, $inspect_params:tt)?) => {
        None
    };
}

macro_rules! experimental_method_entry {
    (#[experimental($reason:expr)] => $wire:literal) => {
        $wire
    };
    (#[experimental($reason:expr)]) => {
        $reason
    };
    ($($tt:tt)*) => {
        ""
    };
}

#[cfg(test)]
macro_rules! client_method_entry {
    ($variant:ident => $wire:literal) => {
        $wire
    };
    ($variant:ident) => {
        ""
    };
}

macro_rules! experimental_type_entry {
    (#[experimental($reason:expr)] $ty:ty) => {
        stringify!($ty)
    };
    ($ty:ty) => {
        ""
    };
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClientRequestSerializationScope {
    Global(&'static str),
    GlobalSharedRead(&'static str),
    Agent { agent_id: String },
    ActivePeer { peer_id: String },
    Thread { thread_id: String },
    ThreadPath { path: PathBuf },
    CommandExecProcess { process_id: String },
    Process { process_handle: String },
    FuzzyFileSearchSession { session_id: String },
    FsWatch { watch_id: String },
    McpOauth { server_name: String },
}

macro_rules! serialization_scope_expr {
    ($actual_params:ident, None) => {
        None
    };
    ($actual_params:ident, global($key:literal)) => {
        Some(ClientRequestSerializationScope::Global($key))
    };
    ($actual_params:ident, global_shared_read($key:literal)) => {
        Some(ClientRequestSerializationScope::GlobalSharedRead($key))
    };
    ($actual_params:ident, agent_id($params:ident . $field:ident)) => {
        Some(ClientRequestSerializationScope::Agent {
            agent_id: $actual_params.$field.clone(),
        })
    };
    ($actual_params:ident, active_session_target($params:ident . $peer_field:ident, $params2:ident . $thread_field:ident)) => {
        active_session_target_serialization_scope(
            $actual_params.$peer_field.as_deref(),
            $actual_params.$thread_field.as_deref(),
        )
    };
    ($actual_params:ident, thread_id($params:ident . $field:ident)) => {
        thread_id_serialization_scope(&$actual_params.$field)
    };
    ($actual_params:ident, optional_thread_id($params:ident . $field:ident)) => {
        $actual_params
            .$field
            .as_ref()
            .map(|thread_id| ClientRequestSerializationScope::Thread {
                thread_id: canonical_thread_id_for_serialization(thread_id),
            })
    };
    ($actual_params:ident, thread_or_path($params:ident . $thread_field:ident, $params2:ident . $path_field:ident)) => {
        if !$actual_params.$thread_field.is_empty() {
            Some(ClientRequestSerializationScope::Thread {
                thread_id: canonical_thread_id_for_serialization(&$actual_params.$thread_field),
            })
        } else if let Some(path) = $actual_params.$path_field.clone() {
            Some(ClientRequestSerializationScope::ThreadPath { path })
        } else {
            Some(ClientRequestSerializationScope::Thread {
                thread_id: canonical_thread_id_for_serialization(&$actual_params.$thread_field),
            })
        }
    };
    ($actual_params:ident, optional_command_process_id($params:ident . $field:ident)) => {
        $actual_params
            .$field
            .clone()
            .map(|process_id| ClientRequestSerializationScope::CommandExecProcess { process_id })
    };
    ($actual_params:ident, command_process_id($params:ident . $field:ident)) => {
        Some(ClientRequestSerializationScope::CommandExecProcess {
            process_id: $actual_params.$field.clone(),
        })
    };
    ($actual_params:ident, process_handle($params:ident . $field:ident)) => {
        Some(ClientRequestSerializationScope::Process {
            process_handle: $actual_params.$field.clone(),
        })
    };
    ($actual_params:ident, fuzzy_session_id($params:ident . $field:ident)) => {
        Some(ClientRequestSerializationScope::FuzzyFileSearchSession {
            session_id: $actual_params.$field.clone(),
        })
    };
    ($actual_params:ident, fs_watch_id($params:ident . $field:ident)) => {
        Some(ClientRequestSerializationScope::FsWatch {
            watch_id: $actual_params.$field.clone(),
        })
    };
    ($actual_params:ident, mcp_oauth_server($params:ident . $field:ident)) => {
        Some(ClientRequestSerializationScope::McpOauth {
            server_name: $actual_params.$field.clone(),
        })
    };
}

fn canonical_thread_id_for_serialization(thread_id: &str) -> String {
    codex_protocol::ThreadId::from_string(thread_id)
        .map(|thread_id| thread_id.to_string())
        .unwrap_or_else(|_| thread_id.to_string())
}

/// Produces a client-request serialization scope from thread id fields.
///
/// Implementations are intentionally limited to thread id payload shapes used
/// by protocol request params. Required thread ids should return a thread
/// scope, while optional thread ids should return `None` when the caller did
/// not provide a target thread.
trait ThreadIdSerializationSource {
    fn serialization_scope(&self) -> Option<ClientRequestSerializationScope>;
}

impl ThreadIdSerializationSource for String {
    fn serialization_scope(&self) -> Option<ClientRequestSerializationScope> {
        Some(ClientRequestSerializationScope::Thread {
            thread_id: canonical_thread_id_for_serialization(self),
        })
    }
}

impl ThreadIdSerializationSource for Option<String> {
    fn serialization_scope(&self) -> Option<ClientRequestSerializationScope> {
        self.as_ref()
            .map(ThreadIdSerializationSource::serialization_scope)?
    }
}

fn thread_id_serialization_scope(
    thread_id: &impl ThreadIdSerializationSource,
) -> Option<ClientRequestSerializationScope> {
    thread_id.serialization_scope()
}

fn active_session_target_serialization_scope(
    target_peer_id: Option<&str>,
    target_thread_id: Option<&str>,
) -> Option<ClientRequestSerializationScope> {
    if let Some(peer_id) = target_peer_id.filter(|peer_id| !peer_id.is_empty()) {
        let canonical_peer_id = canonical_thread_id_for_serialization(peer_id);
        if canonical_peer_id != peer_id {
            return Some(ClientRequestSerializationScope::Thread {
                thread_id: canonical_peer_id,
            });
        }
        if codex_protocol::ThreadId::from_string(peer_id).is_ok() {
            return Some(ClientRequestSerializationScope::Thread {
                thread_id: peer_id.to_string(),
            });
        }
        return Some(ClientRequestSerializationScope::ActivePeer {
            peer_id: peer_id.to_string(),
        });
    }

    target_thread_id.map(|thread_id| ClientRequestSerializationScope::Thread {
        thread_id: canonical_thread_id_for_serialization(thread_id),
    })
}

/// Generates an `enum ClientRequest` where each variant is a request that the
/// client can send to the server. Each variant has associated `params` and
/// `response` types. Also generates a `export_client_responses()` function to
/// export all response types to TypeScript.
macro_rules! client_request_definitions {
    (
        $(
            $(#[experimental($reason:expr)])?
            $(#[doc = $variant_doc:literal])*
            $variant:ident $(=> $wire:literal)? {
                params: $(#[$params_meta:meta])* $params:ty,
                $(inspect_params: $inspect_params:tt,)?
                serialization: $serialization:ident $( ( $($serialization_args:tt)* ) )?,
                $(manual_payload_conversion: $manual_payload_conversion:ident,)?
                response: $response:ty,
            }
        ),* $(,)?
    ) => {
        /// Request from the client to the server.
        #[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
        #[allow(clippy::large_enum_variant)]
        #[serde(tag = "method", rename_all = "camelCase")]
        pub enum ClientRequest {
            $(
                $(#[doc = $variant_doc])*
                $(#[serde(rename = $wire)] #[ts(rename = $wire)])?
                $variant {
                    #[serde(rename = "id")]
                    request_id: RequestId,
                    $(#[$params_meta])*
                    params: $params,
                },
            )*
        }

        impl ClientRequest {
            pub fn id(&self) -> &RequestId {
                match self {
                    $(Self::$variant { request_id, .. } => request_id,)*
                }
            }

            pub fn method(&self) -> String {
                serde_json::to_value(self)
                    .ok()
                    .and_then(|value| {
                        value
                            .get("method")
                            .and_then(serde_json::Value::as_str)
                            .map(str::to_owned)
                    })
                    .unwrap_or_else(|| "<unknown>".to_string())
            }

            pub fn serialization_scope(&self) -> Option<ClientRequestSerializationScope> {
                match self {
                    $(
                        Self::$variant { params, .. } => {
                            let _ = params;
                            serialization_scope_expr!(
                                params, $serialization $( ( $($serialization_args)* ) )?
                            )
                        }
                    )*
                }
            }
        }

        /// Typed response from the server to the client.
        #[derive(Serialize, Deserialize, Debug, Clone)]
        #[allow(clippy::large_enum_variant)]
        #[serde(tag = "method", rename_all = "camelCase")]
        pub enum ClientResponse {
            $(
                $(#[doc = $variant_doc])*
                $(#[serde(rename = $wire)])?
                $variant {
                    #[serde(rename = "id")]
                    request_id: RequestId,
                    response: $response,
                },
            )*
        }

        impl ClientResponse {
            pub fn id(&self) -> &RequestId {
                match self {
                    $(Self::$variant { request_id, .. } => request_id,)*
                }
            }

            pub fn method(&self) -> String {
                serde_json::to_value(self)
                    .ok()
                    .and_then(|value| {
                        value
                            .get("method")
                            .and_then(serde_json::Value::as_str)
                            .map(str::to_owned)
                    })
                    .unwrap_or_else(|| "<unknown>".to_string())
            }

            pub fn into_jsonrpc_parts(
                self,
            ) -> std::result::Result<(RequestId, crate::Result), serde_json::Error> {
                match self {
                    $(
                        Self::$variant { request_id, response } => {
                            serde_json::to_value(response).map(|result| (request_id, result))
                        }
                    )*
                }
            }
        }

        #[derive(Debug, Clone)]
        #[allow(clippy::large_enum_variant)]
        pub enum ClientResponsePayload {
            $( $variant($response), )*
            InterruptConversation(v1::InterruptConversationResponse),
        }

        impl ClientResponsePayload {
            pub fn into_jsonrpc_parts_and_payload(
                self,
                request_id: RequestId,
            ) -> std::result::Result<
                (RequestId, crate::Result, Option<ClientResponsePayload>),
                serde_json::Error,
            > {
                match self {
                    $(
                        Self::$variant(response) => {
                            let result = serde_json::to_value(&response)?;
                            Ok((request_id, result, Some(Self::$variant(response))))
                        }
                    )*
                    Self::InterruptConversation(response) => {
                        serde_json::to_value(response).map(|result| (request_id, result, None))
                    }
                }
            }

            pub fn into_client_response(self, request_id: RequestId) -> Option<ClientResponse> {
                match self {
                    $(
                        Self::$variant(response) => {
                            Some(ClientResponse::$variant {
                                request_id,
                                response,
                            })
                        }
                    )*
                    Self::InterruptConversation(_) => None,
                }
            }

            pub fn into_jsonrpc_parts(
                self,
                request_id: RequestId,
            ) -> std::result::Result<(RequestId, crate::Result), serde_json::Error> {
                self.to_jsonrpc_parts(request_id)
            }

            pub fn to_jsonrpc_parts(
                &self,
                request_id: RequestId,
            ) -> std::result::Result<(RequestId, crate::Result), serde_json::Error> {
                match self {
                    $(
                        Self::$variant(response) => {
                            serde_json::to_value(response).map(|result| (request_id, result))
                        }
                    )*
                    Self::InterruptConversation(response) => {
                        serde_json::to_value(response).map(|result| (request_id, result))
                    }
                }
            }
        }

        impl From<v1::InterruptConversationResponse> for ClientResponsePayload {
            fn from(response: v1::InterruptConversationResponse) -> Self {
                Self::InterruptConversation(response)
            }
        }

        $(
            client_response_payload_from_impl!(
                $variant,
                $response
                $(, $manual_payload_conversion)?
            );
        )*

        impl crate::experimental_api::ExperimentalApi for ClientRequest {
            fn experimental_reason(&self) -> Option<&'static str> {
                match self {
                    $(
                        Self::$variant { params: _params, .. } => {
                            experimental_reason_expr!(
                                variant $variant,
                                $(#[experimental($reason)])?
                                _params
                                $(, $inspect_params)?
                            )
                        }
                    )*
                }
            }
        }

        pub(crate) const EXPERIMENTAL_CLIENT_METHODS: &[&str] = &[
            $(
                experimental_method_entry!($(#[experimental($reason)])? $(=> $wire)?),
            )*
        ];
        #[cfg(test)]
        pub(crate) const CLIENT_METHODS: &[&str] = &[
            $(
                client_method_entry!($variant $(=> $wire)?),
            )*
        ];
        pub(crate) const EXPERIMENTAL_CLIENT_METHOD_PARAM_TYPES: &[&str] = &[
            $(
                experimental_type_entry!($(#[experimental($reason)])? $params),
            )*
        ];
        pub(crate) const EXPERIMENTAL_CLIENT_METHOD_RESPONSE_TYPES: &[&str] = &[
            $(
                experimental_type_entry!($(#[experimental($reason)])? $response),
            )*
        ];

        pub fn export_client_responses(
            out_dir: &::std::path::Path,
        ) -> ::std::result::Result<(), ::ts_rs::ExportError> {
            $(
                <$response as ::ts_rs::TS>::export_all_to(out_dir)?;
            )*
            Ok(())
        }

        pub(crate) fn visit_client_response_types(v: &mut impl ::ts_rs::TypeVisitor) {
            $(
                v.visit::<$response>();
            )*
        }

        #[allow(clippy::vec_init_then_push)]
        pub fn export_client_response_schemas(
            out_dir: &::std::path::Path,
        ) -> ::anyhow::Result<Vec<GeneratedSchema>> {
            let mut schemas = Vec::new();
            $(
                schemas.push(write_json_schema::<$response>(out_dir, stringify!($response))?);
            )*
            Ok(schemas)
        }

        #[allow(clippy::vec_init_then_push)]
        pub fn export_client_param_schemas(
            out_dir: &::std::path::Path,
        ) -> ::anyhow::Result<Vec<GeneratedSchema>> {
            let mut schemas = Vec::new();
            $(
                schemas.push(write_json_schema::<$params>(out_dir, stringify!($params))?);
            )*
            Ok(schemas)
        }
    };
}

macro_rules! client_response_payload_from_impl {
    ($variant:ident, $response:ty) => {
        impl From<$response> for ClientResponsePayload {
            fn from(response: $response) -> Self {
                Self::$variant(response)
            }
        }
    };
    ($variant:ident, $response:ty, manual) => {};
}

client_request_definitions! {
    Initialize {
        params: v1::InitializeParams,
        serialization: None,
        response: v1::InitializeResponse,
    },

    /// NEW APIs
    // Thread lifecycle
    // Uses `inspect_params` because only some fields are experimental.
    ThreadStart => "thread/start" {
        params: v2::ThreadStartParams,
        inspect_params: true,
        serialization: None,
        response: v2::ThreadStartResponse,
    },
    ThreadResume => "thread/resume" {
        params: v2::ThreadResumeParams,
        inspect_params: true,
        serialization: thread_or_path(params.thread_id, params.path),
        response: v2::ThreadResumeResponse,
    },
    ThreadFork => "thread/fork" {
        params: v2::ThreadForkParams,
        inspect_params: true,
        serialization: thread_or_path(params.thread_id, params.path),
        response: v2::ThreadForkResponse,
    },
    ThreadArchive => "thread/archive" {
        params: v2::ThreadArchiveParams,
        serialization: thread_id(params.thread_id),
        response: v2::ThreadArchiveResponse,
    },
    ThreadUnsubscribe => "thread/unsubscribe" {
        params: v2::ThreadUnsubscribeParams,
        serialization: thread_id(params.thread_id),
        response: v2::ThreadUnsubscribeResponse,
    },
    #[experimental("thread/increment_elicitation")]
    /// Increment the thread-local out-of-band elicitation counter.
    ///
    /// This is used by external helpers to pause timeout accounting while a user
    /// approval or other elicitation is pending outside the app-server request flow.
    ThreadIncrementElicitation => "thread/increment_elicitation" {
        params: v2::ThreadIncrementElicitationParams,
        serialization: thread_id(params.thread_id),
        response: v2::ThreadIncrementElicitationResponse,
    },
    #[experimental("thread/decrement_elicitation")]
    /// Decrement the thread-local out-of-band elicitation counter.
    ///
    /// When the count reaches zero, timeout accounting resumes for the thread.
    ThreadDecrementElicitation => "thread/decrement_elicitation" {
        params: v2::ThreadDecrementElicitationParams,
        serialization: thread_id(params.thread_id),
        response: v2::ThreadDecrementElicitationResponse,
    },
    ThreadSetName => "thread/name/set" {
        params: v2::ThreadSetNameParams,
        serialization: thread_id(params.thread_id),
        response: v2::ThreadSetNameResponse,
    },
    ThreadGoalSet => "thread/goal/set" {
        params: v2::ThreadGoalSetParams,
        serialization: thread_id(params.thread_id),
        response: v2::ThreadGoalSetResponse,
    },
    ThreadGoalGet => "thread/goal/get" {
        params: v2::ThreadGoalGetParams,
        serialization: thread_id(params.thread_id),
        response: v2::ThreadGoalGetResponse,
    },
    ThreadGoalList => "thread/goal/list" {
        params: v2::ThreadGoalListParams,
        serialization: thread_id(params.thread_id),
        response: v2::ThreadGoalListResponse,
    },
    ThreadGoalPlanActivateNode => "thread/goalPlan/activateNode" {
        params: v2::ThreadGoalPlanActivateNodeParams,
        serialization: thread_id(params.thread_id),
        response: v2::ThreadGoalPlanActivateNodeResponse,
    },
    ThreadGoalPlanAddGoal => "thread/goalPlan/addGoal" {
        params: v2::ThreadGoalPlanAddGoalParams,
        serialization: thread_id(params.thread_id),
        response: v2::ThreadGoalPlanAddGoalResponse,
    },
    ThreadGoalClear => "thread/goal/clear" {
        params: v2::ThreadGoalClearParams,
        serialization: thread_id(params.thread_id),
        response: v2::ThreadGoalClearResponse,
    },
    ThreadScheduleCreate => "thread/schedule/create" {
        params: v2::ThreadScheduleCreateParams,
        serialization: thread_id(params.thread_id),
        response: v2::ThreadScheduleCreateResponse,
    },
    ThreadScheduleList => "thread/schedule/list" {
        params: v2::ThreadScheduleListParams,
        serialization: thread_id(params.thread_id),
        response: v2::ThreadScheduleListResponse,
    },
    ThreadScheduleGet => "thread/schedule/get" {
        params: v2::ThreadScheduleGetParams,
        serialization: thread_id(params.thread_id),
        response: v2::ThreadScheduleGetResponse,
    },
    ThreadScheduleUpdate => "thread/schedule/update" {
        params: v2::ThreadScheduleUpdateParams,
        serialization: thread_id(params.thread_id),
        response: v2::ThreadScheduleUpdateResponse,
    },
    ThreadSchedulePause => "thread/schedule/pause" {
        params: v2::ThreadSchedulePauseParams,
        serialization: thread_id(params.thread_id),
        response: v2::ThreadSchedulePauseResponse,
    },
    ThreadScheduleResume => "thread/schedule/resume" {
        params: v2::ThreadScheduleResumeParams,
        serialization: thread_id(params.thread_id),
        response: v2::ThreadScheduleResumeResponse,
    },
    ThreadScheduleDelete => "thread/schedule/delete" {
        params: v2::ThreadScheduleDeleteParams,
        serialization: thread_id(params.thread_id),
        response: v2::ThreadScheduleDeleteResponse,
    },
    ThreadScheduleRunNow => "thread/schedule/runNow" {
        params: v2::ThreadScheduleRunNowParams,
        serialization: thread_id(params.thread_id),
        response: v2::ThreadScheduleRunNowResponse,
    },
    ThreadMonitorCreate => "thread/monitor/create" {
        params: v2::ThreadMonitorCreateParams,
        serialization: thread_id(params.thread_id),
        response: v2::ThreadMonitorCreateResponse,
    },
    ThreadMonitorList => "thread/monitor/list" {
        params: v2::ThreadMonitorListParams,
        serialization: thread_id(params.thread_id),
        response: v2::ThreadMonitorListResponse,
    },
    ThreadMonitorRead => "thread/monitor/read" {
        params: v2::ThreadMonitorReadParams,
        serialization: thread_id(params.thread_id),
        response: v2::ThreadMonitorReadResponse,
    },
    ThreadMonitorStop => "thread/monitor/stop" {
        params: v2::ThreadMonitorStopParams,
        serialization: thread_id(params.thread_id),
        response: v2::ThreadMonitorStopResponse,
    },
    ThreadMonitorRestart => "thread/monitor/restart" {
        params: v2::ThreadMonitorRestartParams,
        serialization: thread_id(params.thread_id),
        response: v2::ThreadMonitorRestartResponse,
    },
    ThreadMonitorDelete => "thread/monitor/delete" {
        params: v2::ThreadMonitorDeleteParams,
        serialization: thread_id(params.thread_id),
        response: v2::ThreadMonitorDeleteResponse,
    },
    WebhookEventList => "webhook/event/list" {
        params: v2::WebhookEventListParams,
        serialization: global_shared_read("webhook-event"),
        response: v2::WebhookEventListResponse,
    },
    WebhookEventRead => "webhook/event/read" {
        params: v2::WebhookEventReadParams,
        serialization: global_shared_read("webhook-event"),
        response: v2::WebhookEventReadResponse,
    },
    WebhookEventMark => "webhook/event/mark" {
        params: v2::WebhookEventMarkParams,
        serialization: global("webhook-event"),
        response: v2::WebhookEventMarkResponse,
    },
    WebhookEventIngest => "webhook/event/ingest" {
        params: v2::WebhookEventIngestParams,
        serialization: global("webhook-event"),
        response: v2::WebhookEventIngestResponse,
    },
    #[experimental("thread/workflow/create")]
    ThreadWorkflowCreate => "thread/workflow/create" {
        params: v2::ThreadWorkflowCreateParams,
        serialization: thread_id(params.thread_id),
        response: v2::ThreadWorkflowCreateResponse,
    },
    #[experimental("thread/workflow/get")]
    ThreadWorkflowGet => "thread/workflow/get" {
        params: v2::ThreadWorkflowGetParams,
        serialization: thread_id(params.thread_id),
        response: v2::ThreadWorkflowGetResponse,
    },
    #[experimental("thread/workflow/list")]
    ThreadWorkflowList => "thread/workflow/list" {
        params: v2::ThreadWorkflowListParams,
        serialization: thread_id(params.thread_id),
        response: v2::ThreadWorkflowListResponse,
    },
    #[experimental("thread/workflow/run/list")]
    ThreadWorkflowRunList => "thread/workflow/run/list" {
        params: v2::ThreadWorkflowRunListParams,
        serialization: thread_id(params.thread_id),
        response: v2::ThreadWorkflowRunListResponse,
    },
    #[experimental("thread/workflow/run/get")]
    ThreadWorkflowRunGet => "thread/workflow/run/get" {
        params: v2::ThreadWorkflowRunGetParams,
        serialization: thread_id(params.thread_id),
        response: v2::ThreadWorkflowRunGetResponse,
    },
    #[experimental("thread/workflow/run/start")]
    ThreadWorkflowRunStart => "thread/workflow/run/start" {
        params: v2::ThreadWorkflowRunStartParams,
        serialization: thread_id(params.thread_id),
        response: v2::ThreadWorkflowRunStartResponse,
    },
    #[experimental("thread/workflow/run/pause")]
    ThreadWorkflowRunPause => "thread/workflow/run/pause" {
        params: v2::ThreadWorkflowRunPauseParams,
        serialization: thread_id(params.thread_id),
        response: v2::ThreadWorkflowRunPauseResponse,
    },
    #[experimental("thread/workflow/run/resume")]
    ThreadWorkflowRunResume => "thread/workflow/run/resume" {
        params: v2::ThreadWorkflowRunResumeParams,
        serialization: thread_id(params.thread_id),
        response: v2::ThreadWorkflowRunResumeResponse,
    },
    #[experimental("thread/workflow/run/cancel")]
    ThreadWorkflowRunCancel => "thread/workflow/run/cancel" {
        params: v2::ThreadWorkflowRunCancelParams,
        serialization: thread_id(params.thread_id),
        response: v2::ThreadWorkflowRunCancelResponse,
    },
    #[experimental("thread/mailbox/enqueue")]
    ThreadMailboxEnqueue => "thread/mailbox/enqueue" {
        params: v2::ThreadMailboxEnqueueParams,
        serialization: thread_id(params.target_thread_id),
        response: v2::ThreadMailboxEnqueueResponse,
    },
    #[experimental("thread/mailbox/list")]
    ThreadMailboxList => "thread/mailbox/list" {
        params: v2::ThreadMailboxListParams,
        serialization: thread_id(params.target_thread_id),
        response: v2::ThreadMailboxListResponse,
    },
    #[experimental("thread/mailbox/read")]
    ThreadMailboxRead => "thread/mailbox/read" {
        params: v2::ThreadMailboxReadParams,
        serialization: thread_id(params.target_thread_id),
        response: v2::ThreadMailboxReadResponse,
    },
    #[experimental("thread/mailbox/claim")]
    ThreadMailboxClaim => "thread/mailbox/claim" {
        params: v2::ThreadMailboxClaimParams,
        serialization: thread_id(params.target_thread_id),
        response: v2::ThreadMailboxClaimResponse,
    },
    #[experimental("thread/mailbox/ack")]
    ThreadMailboxAck => "thread/mailbox/ack" {
        params: v2::ThreadMailboxAckParams,
        serialization: thread_id(params.target_thread_id),
        response: v2::ThreadMailboxAckResponse,
    },
    #[experimental("thread/mailbox/fail")]
    ThreadMailboxFail => "thread/mailbox/fail" {
        params: v2::ThreadMailboxFailParams,
        serialization: thread_id(params.target_thread_id),
        response: v2::ThreadMailboxFailResponse,
    },
    #[experimental("thread/mailbox/receipts/list")]
    ThreadMailboxReceiptsList => "thread/mailbox/receipts/list" {
        params: v2::ThreadMailboxReceiptsListParams,
        serialization: thread_id(params.target_thread_id),
        response: v2::ThreadMailboxReceiptsListResponse,
    },
    #[experimental("thread/pendingInteraction/list")]
    ThreadPendingInteractionList => "thread/pendingInteraction/list" {
        params: v2::ThreadPendingInteractionListParams,
        serialization: optional_thread_id(params.thread_id),
        response: v2::ThreadPendingInteractionListResponse,
    },
    #[experimental("thread/pendingInteraction/read")]
    ThreadPendingInteractionRead => "thread/pendingInteraction/read" {
        params: v2::ThreadPendingInteractionReadParams,
        serialization: optional_thread_id(params.thread_id),
        response: v2::ThreadPendingInteractionReadResponse,
    },
    #[experimental("thread/pendingInteraction/respond")]
    ThreadPendingInteractionRespond => "thread/pendingInteraction/respond" {
        params: v2::ThreadPendingInteractionRespondParams,
        serialization: optional_thread_id(params.thread_id),
        response: v2::ThreadPendingInteractionRespondResponse,
    },
    MissionControlOverview => "missionControl/overview" {
        params: v2::MissionControlOverviewParams,
        serialization: global("mission_control"),
        response: v2::MissionControlOverviewResponse,
    },
    MissionControlEnqueueInstruction => "missionControl/enqueueInstruction" {
        params: v2::MissionControlEnqueueInstructionParams,
        serialization: thread_id(params.target_thread_id),
        response: v2::MissionControlEnqueueInstructionResponse,
    },
    MissionControlMailboxReceipts => "missionControl/mailboxReceipts" {
        params: v2::MissionControlMailboxReceiptsParams,
        serialization: thread_id(params.target_thread_id),
        response: v2::MissionControlMailboxReceiptsResponse,
    },
    MissionControlRespondInteraction => "missionControl/respondInteraction" {
        params: v2::MissionControlRespondInteractionParams,
        serialization: optional_thread_id(params.thread_id),
        response: v2::MissionControlRespondInteractionResponse,
    },
    #[experimental("remoteDispatch/negotiate")]
    RemoteDispatchNegotiate => "remoteDispatch/negotiate" {
        params: v2::RemoteDispatchNegotiateParams,
        serialization: global_shared_read("remote-dispatch"),
        response: v2::RemoteDispatchNegotiateResponse,
    },
    #[experimental("remoteDispatch/submit")]
    RemoteDispatchSubmit => "remoteDispatch/submit" {
        params: v2::RemoteDispatchSubmitParams,
        serialization: global("remote-dispatch"),
        response: v2::RemoteDispatchSubmitResponse,
    },
    #[experimental("remoteDispatch/receipt/read")]
    RemoteDispatchReceiptRead => "remoteDispatch/receipt/read" {
        params: v2::RemoteDispatchReceiptReadParams,
        serialization: global_shared_read("remote-dispatch"),
        response: v2::RemoteDispatchReceiptReadResponse,
    },
    #[experimental("agent/start")]
    AgentStart => "agent/start" {
        params: v2::AgentStartParams,
        serialization: global("agent"),
        response: v2::AgentStartResponse,
    },
    #[experimental("agent/list")]
    AgentList => "agent/list" {
        params: v2::AgentListParams,
        serialization: global("agent"),
        response: v2::AgentListResponse,
    },
    #[experimental("agent/read")]
    AgentRead => "agent/read" {
        params: v2::AgentReadParams,
        serialization: global("agent"),
        response: v2::AgentReadResponse,
    },
    #[experimental("agent/attach")]
    AgentAttach => "agent/attach" {
        params: v2::AgentAttachParams,
        serialization: global("agent"),
        response: v2::AgentAttachResponse,
    },
    #[experimental("agent/detach")]
    AgentDetach => "agent/detach" {
        params: v2::AgentDetachParams,
        serialization: global("agent"),
        response: v2::AgentDetachResponse,
    },
    #[experimental("agent/stop")]
    AgentStop => "agent/stop" {
        params: v2::AgentStopParams,
        serialization: global("agent"),
        response: v2::AgentStopResponse,
    },
    #[experimental("agent/delete")]
    AgentDelete => "agent/delete" {
        params: v2::AgentDeleteParams,
        serialization: global("agent"),
        response: v2::AgentDeleteResponse,
    },
    #[experimental("agent/events/list")]
    AgentEventsList => "agent/events/list" {
        params: v2::AgentEventsListParams,
        serialization: global("agent"),
        response: v2::AgentEventsListResponse,
    },
    #[experimental("agent/pendingInteraction/respond")]
    AgentPendingInteractionRespond => "agent/pendingInteraction/respond" {
        params: v2::AgentPendingInteractionRespondParams,
        serialization: global("agent"),
        response: v2::AgentPendingInteractionRespondResponse,
    },
    #[experimental("agent/daemon/diagnostics")]
    AgentDaemonDiagnostics => "agent/daemon/diagnostics" {
        params: v2::AgentDaemonDiagnosticsParams,
        serialization: global("agent"),
        response: v2::AgentDaemonDiagnosticsResponse,
    },
    WorktreeList => "worktree/list" {
        params: v2::WorktreeListParams,
        serialization: global("worktree"),
        response: v2::WorktreeListResponse,
    },
    WorktreeRead => "worktree/read" {
        params: v2::WorktreeReadParams,
        serialization: global("worktree"),
        response: v2::WorktreeReadResponse,
    },
    WorktreeCreate => "worktree/create" {
        params: v2::WorktreeCreateParams,
        serialization: global("worktree"),
        response: v2::WorktreeCreateResponse,
    },
    WorktreeReconcile => "worktree/reconcile" {
        params: v2::WorktreeReconcileParams,
        serialization: global("worktree"),
        response: v2::WorktreeReconcileResponse,
    },
    WorktreeAttach => "worktree/attach" {
        params: v2::WorktreeAttachParams,
        serialization: global("worktree"),
        response: v2::WorktreeAttachResponse,
    },
    WorktreeDetach => "worktree/detach" {
        params: v2::WorktreeDetachParams,
        serialization: global("worktree"),
        response: v2::WorktreeDetachResponse,
    },
    WorktreeRelease => "worktree/release" {
        params: v2::WorktreeReleaseParams,
        serialization: global("worktree"),
        response: v2::WorktreeReleaseResponse,
    },
    WorktreeCleanup => "worktree/cleanup" {
        params: v2::WorktreeCleanupParams,
        serialization: global("worktree"),
        response: v2::WorktreeCleanupResponse,
    },
    WorktreeMergeCandidateList => "worktree/mergeCandidate/list" {
        params: v2::WorktreeMergeCandidateListParams,
        serialization: global("worktree"),
        response: v2::WorktreeMergeCandidateListResponse,
    },
    WorktreeMergeCandidateRefresh => "worktree/mergeCandidate/refresh" {
        params: v2::WorktreeMergeCandidateRefreshParams,
        serialization: global("worktree"),
        response: v2::WorktreeMergeCandidateRefreshResponse,
    },
    WorktreeMergeCandidateApply => "worktree/mergeCandidate/apply" {
        params: v2::WorktreeMergeCandidateApplyParams,
        serialization: global("worktree"),
        response: v2::WorktreeMergeCandidateApplyResponse,
    },
    WorktreeMergeCandidateDismiss => "worktree/mergeCandidate/dismiss" {
        params: v2::WorktreeMergeCandidateDismissParams,
        serialization: global("worktree"),
        response: v2::WorktreeMergeCandidateDismissResponse,
    },
    #[experimental("machineRegistry/list")]
    MachineRegistryList => "machineRegistry/list" {
        params: v2::MachineRegistryListParams,
        serialization: global_shared_read("machine-registry"),
        response: v2::MachineRegistryListResponse,
    },
    #[experimental("machineRegistry/read")]
    MachineRegistryRead => "machineRegistry/read" {
        params: v2::MachineRegistryReadParams,
        serialization: global_shared_read("machine-registry"),
        response: v2::MachineRegistryReadResponse,
    },
    #[experimental("machineRegistry/upsert")]
    MachineRegistryUpsert => "machineRegistry/upsert" {
        params: v2::MachineRegistryUpsertParams,
        serialization: global("machine-registry"),
        response: v2::MachineRegistryUpsertResponse,
    },
    #[experimental("machineRegistry/disable")]
    MachineRegistryDisable => "machineRegistry/disable" {
        params: v2::MachineRegistryDisableParams,
        serialization: global("machine-registry"),
        response: v2::MachineRegistryDisableResponse,
    },
    #[experimental("machineRegistry/updateTrust")]
    MachineRegistryUpdateTrust => "machineRegistry/updateTrust" {
        params: v2::MachineRegistryUpdateTrustParams,
        serialization: global("machine-registry"),
        response: v2::MachineRegistryUpdateTrustResponse,
    },
    #[experimental("machineRegistry/forget")]
    MachineRegistryForget => "machineRegistry/forget" {
        params: v2::MachineRegistryForgetParams,
        serialization: global("machine-registry"),
        response: v2::MachineRegistryForgetResponse,
    },
    ThreadMetadataUpdate => "thread/metadata/update" {
        params: v2::ThreadMetadataUpdateParams,
        serialization: thread_id(params.thread_id),
        response: v2::ThreadMetadataUpdateResponse,
    },
    #[experimental("thread/settings/update")]
    ThreadSettingsUpdate => "thread/settings/update" {
        params: v2::ThreadSettingsUpdateParams,
        inspect_params: true,
        serialization: thread_id(params.thread_id),
        response: v2::ThreadSettingsUpdateResponse,
    },
    #[experimental("thread/memoryMode/set")]
    ThreadMemoryModeSet => "thread/memoryMode/set" {
        params: v2::ThreadMemoryModeSetParams,
        serialization: thread_id(params.thread_id),
        response: v2::ThreadMemoryModeSetResponse,
    },
    #[experimental("memory/reset")]
    MemoryReset => "memory/reset" {
        params: #[ts(type = "undefined")] #[serde(skip_serializing_if = "Option::is_none")] Option<()>,
        serialization: global("memory"),
        response: v2::MemoryResetResponse,
    },
    ThreadUnarchive => "thread/unarchive" {
        params: v2::ThreadUnarchiveParams,
        serialization: thread_id(params.thread_id),
        response: v2::ThreadUnarchiveResponse,
    },
    ThreadCompactStart => "thread/compact/start" {
        params: v2::ThreadCompactStartParams,
        serialization: thread_id(params.thread_id),
        response: v2::ThreadCompactStartResponse,
    },
    ThreadRecap => "thread/recap" {
        params: v2::ThreadRecapParams,
        serialization: thread_id(params.thread_id),
        response: v2::ThreadRecapResponse,
    },
    ThreadShellCommand => "thread/shellCommand" {
        params: v2::ThreadShellCommandParams,
        serialization: thread_id(params.thread_id),
        response: v2::ThreadShellCommandResponse,
    },
    ThreadQueuedMessageList => "thread/queuedMessage/list" {
        params: v2::ThreadQueuedMessageListParams,
        serialization: thread_id(params.thread_id),
        response: v2::ThreadQueuedMessageListResponse,
    },
    ThreadQueuedMessageUpdate => "thread/queuedMessage/update" {
        params: v2::ThreadQueuedMessageUpdateParams,
        serialization: thread_id(params.thread_id),
        response: v2::ThreadQueuedMessageUpdateResponse,
    },
    ThreadQueuedMessageMove => "thread/queuedMessage/move" {
        params: v2::ThreadQueuedMessageMoveParams,
        serialization: thread_id(params.thread_id),
        response: v2::ThreadQueuedMessageMoveResponse,
    },
    #[experimental("thread/externalAgent/start")]
    ThreadExternalAgentStart => "thread/externalAgent/start" {
        params: v2::ThreadExternalAgentStartParams,
        serialization: thread_id(params.thread_id),
        response: v2::ThreadExternalAgentStartResponse,
    },
    #[experimental("thread/externalAgent/cancel")]
    ThreadExternalAgentCancel => "thread/externalAgent/cancel" {
        params: v2::ThreadExternalAgentCancelParams,
        serialization: thread_id(params.thread_id),
        response: v2::ThreadExternalAgentCancelResponse,
    },
    #[experimental("thread/externalAgent/permission/respond")]
    ThreadExternalAgentPermissionRespond => "thread/externalAgent/permission/respond" {
        params: v2::ThreadExternalAgentPermissionRespondParams,
        serialization: thread_id(params.thread_id),
        response: v2::ThreadExternalAgentPermissionRespondResponse,
    },
    ThreadApproveGuardianDeniedAction => "thread/approveGuardianDeniedAction" {
        params: v2::ThreadApproveGuardianDeniedActionParams,
        serialization: thread_id(params.thread_id),
        response: v2::ThreadApproveGuardianDeniedActionResponse,
    },
    #[experimental("thread/backgroundTerminals/clean")]
    ThreadBackgroundTerminalsClean => "thread/backgroundTerminals/clean" {
        params: v2::ThreadBackgroundTerminalsCleanParams,
        serialization: thread_id(params.thread_id),
        response: v2::ThreadBackgroundTerminalsCleanResponse,
    },
    ThreadRollback => "thread/rollback" {
        params: v2::ThreadRollbackParams,
        serialization: thread_id(params.thread_id),
        response: v2::ThreadRollbackResponse,
    },
    ThreadList => "thread/list" {
        params: v2::ThreadListParams,
        serialization: None,
        response: v2::ThreadListResponse,
    },
    #[experimental("thread/search")]
    ThreadSearch => "thread/search" {
        params: v2::ThreadSearchParams,
        serialization: None,
        response: v2::ThreadSearchResponse,
    },
    ThreadLoadedList => "thread/loaded/list" {
        params: v2::ThreadLoadedListParams,
        serialization: None,
        response: v2::ThreadLoadedListResponse,
    },
    #[experimental("localSession/list")]
    LocalSessionList => "localSession/list" {
        params: v2::LocalSessionListParams,
        serialization: None,
        response: v2::LocalSessionListResponse,
    },
    ActiveSessionList => "activeSession/list" {
        params: v2::ActiveSessionListParams,
        serialization: None,
        response: v2::ActiveSessionListResponse,
    },
    ActiveSessionSend => "activeSession/send" {
        params: v2::ActiveSessionSendParams,
        serialization: active_session_target(params.target_peer_id, params.target_thread_id),
        response: v2::ActiveSessionSendResponse,
    },
    ThreadRead => "thread/read" {
        params: v2::ThreadReadParams,
        serialization: thread_id(params.thread_id),
        response: v2::ThreadReadResponse,
    },
    #[experimental("thread/turns/list")]
    ThreadTurnsList => "thread/turns/list" {
        params: v2::ThreadTurnsListParams,
        // Explicitly concurrent: this primarily reads append-only rollout storage.
        serialization: None,
        response: v2::ThreadTurnsListResponse,
    },
    #[experimental("thread/turns/items/list")]
    ThreadTurnsItemsList => "thread/turns/items/list" {
        params: v2::ThreadTurnsItemsListParams,
        // Explicitly concurrent: this primarily reads append-only rollout storage.
        serialization: None,
        response: v2::ThreadTurnsItemsListResponse,
    },
    /// Append raw Responses API items to the thread history without starting a user turn.
    ThreadInjectItems => "thread/inject_items" {
        params: v2::ThreadInjectItemsParams,
        serialization: thread_id(params.thread_id),
        response: v2::ThreadInjectItemsResponse,
    },
    SkillsList => "skills/list" {
        params: v2::SkillsListParams,
        serialization: global_shared_read("config"),
        response: v2::SkillsListResponse,
    },
    SkillsExtraRootsSet => "skills/extraRoots/set" {
        params: v2::SkillsExtraRootsSetParams,
        serialization: global("config"),
        response: v2::SkillsExtraRootsSetResponse,
    },
    HooksList => "hooks/list" {
        params: v2::HooksListParams,
        serialization: global("config"),
        response: v2::HooksListResponse,
    },
    MarketplaceAdd => "marketplace/add" {
        params: v2::MarketplaceAddParams,
        serialization: global("config"),
        response: v2::MarketplaceAddResponse,
    },
    MarketplaceRemove => "marketplace/remove" {
        params: v2::MarketplaceRemoveParams,
        serialization: global("config"),
        response: v2::MarketplaceRemoveResponse,
    },
    MarketplaceUpgrade => "marketplace/upgrade" {
        params: v2::MarketplaceUpgradeParams,
        serialization: global("config"),
        response: v2::MarketplaceUpgradeResponse,
    },
    PluginList => "plugin/list" {
        params: v2::PluginListParams,
        serialization: None,
        response: v2::PluginListResponse,
    },
    PluginInstalled => "plugin/installed" {
        params: v2::PluginInstalledParams,
        serialization: None,
        response: v2::PluginInstalledResponse,
    },
    PluginRead => "plugin/read" {
        params: v2::PluginReadParams,
        serialization: None,
        response: v2::PluginReadResponse,
    },
    PluginSkillRead => "plugin/skill/read" {
        params: v2::PluginSkillReadParams,
        serialization: global("config"),
        response: v2::PluginSkillReadResponse,
    },
    PluginShareSave => "plugin/share/save" {
        params: v2::PluginShareSaveParams,
        serialization: global("config"),
        response: v2::PluginShareSaveResponse,
    },
    PluginShareUpdateTargets => "plugin/share/updateTargets" {
        params: v2::PluginShareUpdateTargetsParams,
        serialization: global("config"),
        response: v2::PluginShareUpdateTargetsResponse,
    },
    PluginShareList => "plugin/share/list" {
        params: v2::PluginShareListParams,
        serialization: global("config"),
        response: v2::PluginShareListResponse,
    },
    PluginShareCheckout => "plugin/share/checkout" {
        params: v2::PluginShareCheckoutParams,
        serialization: global("config"),
        response: v2::PluginShareCheckoutResponse,
    },
    PluginShareDelete => "plugin/share/delete" {
        params: v2::PluginShareDeleteParams,
        serialization: global("config"),
        response: v2::PluginShareDeleteResponse,
    },
    AppsList => "app/list" {
        params: v2::AppsListParams,
        serialization: None,
        response: v2::AppsListResponse,
    },
    // File system requests are intentionally concurrent. Desktop already treats local
    // file system operations as concurrent, and app-server remote fs mirrors that model.
    FsReadFile => "fs/readFile" {
        params: v2::FsReadFileParams,
        serialization: None,
        response: v2::FsReadFileResponse,
    },
    FsWriteFile => "fs/writeFile" {
        params: v2::FsWriteFileParams,
        serialization: None,
        response: v2::FsWriteFileResponse,
    },
    FsCreateDirectory => "fs/createDirectory" {
        params: v2::FsCreateDirectoryParams,
        serialization: None,
        response: v2::FsCreateDirectoryResponse,
    },
    FsGetMetadata => "fs/getMetadata" {
        params: v2::FsGetMetadataParams,
        serialization: None,
        response: v2::FsGetMetadataResponse,
    },
    FsReadDirectory => "fs/readDirectory" {
        params: v2::FsReadDirectoryParams,
        serialization: None,
        response: v2::FsReadDirectoryResponse,
    },
    FsRemove => "fs/remove" {
        params: v2::FsRemoveParams,
        serialization: None,
        response: v2::FsRemoveResponse,
    },
    FsCopy => "fs/copy" {
        params: v2::FsCopyParams,
        serialization: None,
        response: v2::FsCopyResponse,
    },
    FsWatch => "fs/watch" {
        params: v2::FsWatchParams,
        serialization: fs_watch_id(params.watch_id),
        response: v2::FsWatchResponse,
    },
    FsUnwatch => "fs/unwatch" {
        params: v2::FsUnwatchParams,
        serialization: fs_watch_id(params.watch_id),
        response: v2::FsUnwatchResponse,
    },
    SkillsConfigWrite => "skills/config/write" {
        params: v2::SkillsConfigWriteParams,
        serialization: global("config"),
        response: v2::SkillsConfigWriteResponse,
    },
    PluginInstall => "plugin/install" {
        params: v2::PluginInstallParams,
        serialization: global("config"),
        response: v2::PluginInstallResponse,
    },
    PluginUninstall => "plugin/uninstall" {
        params: v2::PluginUninstallParams,
        serialization: global("config"),
        response: v2::PluginUninstallResponse,
    },
    TurnStart => "turn/start" {
        params: v2::TurnStartParams,
        inspect_params: true,
        serialization: thread_id(params.thread_id),
        response: v2::TurnStartResponse,
    },
    TurnSteer => "turn/steer" {
        params: v2::TurnSteerParams,
        inspect_params: true,
        serialization: thread_id(params.thread_id),
        response: v2::TurnSteerResponse,
    },
    TurnInterrupt => "turn/interrupt" {
        params: v2::TurnInterruptParams,
        serialization: thread_id(params.thread_id),
        response: v2::TurnInterruptResponse,
    },
    #[experimental("thread/realtime/start")]
    ThreadRealtimeStart => "thread/realtime/start" {
        params: v2::ThreadRealtimeStartParams,
        serialization: thread_id(params.thread_id),
        response: v2::ThreadRealtimeStartResponse,
    },
    #[experimental("thread/realtime/appendAudio")]
    ThreadRealtimeAppendAudio => "thread/realtime/appendAudio" {
        params: v2::ThreadRealtimeAppendAudioParams,
        serialization: thread_id(params.thread_id),
        response: v2::ThreadRealtimeAppendAudioResponse,
    },
    #[experimental("thread/realtime/appendText")]
    ThreadRealtimeAppendText => "thread/realtime/appendText" {
        params: v2::ThreadRealtimeAppendTextParams,
        serialization: thread_id(params.thread_id),
        response: v2::ThreadRealtimeAppendTextResponse,
    },
    #[experimental("thread/realtime/stop")]
    ThreadRealtimeStop => "thread/realtime/stop" {
        params: v2::ThreadRealtimeStopParams,
        serialization: thread_id(params.thread_id),
        response: v2::ThreadRealtimeStopResponse,
    },
    #[experimental("thread/realtime/listVoices")]
    ThreadRealtimeListVoices => "thread/realtime/listVoices" {
        params: v2::ThreadRealtimeListVoicesParams,
        serialization: None,
        response: v2::ThreadRealtimeListVoicesResponse,
    },
    ReviewStart => "review/start" {
        params: v2::ReviewStartParams,
        serialization: thread_id(params.thread_id),
        response: v2::ReviewStartResponse,
    },

    ModelList => "model/list" {
        params: v2::ModelListParams,
        serialization: None,
        response: v2::ModelListResponse,
    },
    ModelGatewayList => "modelGateway/list" {
        params: v2::ModelGatewayListParams,
        serialization: global_shared_read("config"),
        response: v2::ModelGatewayListResponse,
    },
    ModelProviderList => "modelProvider/list" {
        params: v2::ModelProviderListParams,
        serialization: global_shared_read("config"),
        response: v2::ModelProviderListResponse,
    },
    ModelProviderCapabilitiesRead => "modelProvider/capabilities/read" {
        params: v2::ModelProviderCapabilitiesReadParams,
        serialization: None,
        response: v2::ModelProviderCapabilitiesReadResponse,
    },
    ExperimentalFeatureList => "experimentalFeature/list" {
        params: v2::ExperimentalFeatureListParams,
        serialization: global("config"),
        response: v2::ExperimentalFeatureListResponse,
    },
    PermissionProfileList => "permissionProfile/list" {
        params: v2::PermissionProfileListParams,
        serialization: global_shared_read("config"),
        response: v2::PermissionProfileListResponse,
    },
    ExperimentalFeatureEnablementSet => "experimentalFeature/enablement/set" {
        params: v2::ExperimentalFeatureEnablementSetParams,
        serialization: global("config"),
        response: v2::ExperimentalFeatureEnablementSetResponse,
    },
    #[experimental("remoteControl/enable")]
    RemoteControlEnable => "remoteControl/enable" {
        params: #[ts(type = "undefined")] #[serde(skip_serializing_if = "Option::is_none")] Option<()>,
        serialization: global("remote-control"),
        response: v2::RemoteControlEnableResponse,
    },
    #[experimental("remoteControl/disable")]
    RemoteControlDisable => "remoteControl/disable" {
        params: #[ts(type = "undefined")] #[serde(skip_serializing_if = "Option::is_none")] Option<()>,
        serialization: global("remote-control"),
        response: v2::RemoteControlDisableResponse,
    },
    #[experimental("remoteControl/status/read")]
    RemoteControlStatusRead => "remoteControl/status/read" {
        params: #[ts(type = "undefined")] #[serde(skip_serializing_if = "Option::is_none")] Option<()>,
        serialization: global_shared_read("remote-control"),
        response: v2::RemoteControlStatusReadResponse,
    },
    #[experimental("remoteControl/pairing/start")]
    RemoteControlPairingStart => "remoteControl/pairing/start" {
        params: v2::RemoteControlPairingStartParams,
        serialization: global("remote-control-pairing"),
        response: v2::RemoteControlPairingStartResponse,
    },
    #[experimental("remoteControl/pairing/status")]
    RemoteControlPairingStatus => "remoteControl/pairing/status" {
        params: v2::RemoteControlPairingStatusParams,
        serialization: global_shared_read("remote-control-pairing"),
        response: v2::RemoteControlPairingStatusResponse,
    },
    #[experimental("remoteControl/client/list")]
    RemoteControlClientsList => "remoteControl/client/list" {
        params: v2::RemoteControlClientsListParams,
        serialization: global_shared_read("remote-control-clients"),
        response: v2::RemoteControlClientsListResponse,
    },
    #[experimental("remoteControl/client/revoke")]
    RemoteControlClientsRevoke => "remoteControl/client/revoke" {
        params: v2::RemoteControlClientsRevokeParams,
        serialization: global("remote-control-clients"),
        response: v2::RemoteControlClientsRevokeResponse,
    },
    #[experimental("collaborationMode/list")]
    /// Lists collaboration mode presets.
    CollaborationModeList => "collaborationMode/list" {
        params: v2::CollaborationModeListParams,
        serialization: None,
        response: v2::CollaborationModeListResponse,
    },
    #[experimental("mock/experimentalMethod")]
    /// Test-only method used to validate experimental gating.
    MockExperimentalMethod => "mock/experimentalMethod" {
        params: v2::MockExperimentalMethodParams,
        serialization: None,
        response: v2::MockExperimentalMethodResponse,
    },
    #[experimental("environment/add")]
    /// Adds or replaces a remote environment by id for later selection.
    EnvironmentAdd => "environment/add" {
        params: v2::EnvironmentAddParams,
        serialization: global("environment"),
        response: v2::EnvironmentAddResponse,
    },

    McpServerOauthLogin => "mcpServer/oauth/login" {
        params: v2::McpServerOauthLoginParams,
        serialization: mcp_oauth_server(params.name),
        response: v2::McpServerOauthLoginResponse,
    },

    McpServerRefresh => "config/mcpServer/reload" {
        params: #[ts(type = "undefined")] #[serde(skip_serializing_if = "Option::is_none")] Option<()>,
        serialization: global("mcp-registry"),
        response: v2::McpServerRefreshResponse,
    },

    McpServerStatusList => "mcpServerStatus/list" {
        params: v2::ListMcpServerStatusParams,
        serialization: global("mcp-registry"),
        response: v2::ListMcpServerStatusResponse,
    },

    McpResourceRead => "mcpServer/resource/read" {
        params: v2::McpResourceReadParams,
        serialization: optional_thread_id(params.thread_id),
        response: v2::McpResourceReadResponse,
    },

    McpServerToolCall => "mcpServer/tool/call" {
        params: v2::McpServerToolCallParams,
        serialization: thread_id(params.thread_id),
        response: v2::McpServerToolCallResponse,
    },

    WindowsSandboxSetupStart => "windowsSandbox/setupStart" {
        params: v2::WindowsSandboxSetupStartParams,
        serialization: global("windows-sandbox-setup"),
        response: v2::WindowsSandboxSetupStartResponse,
    },
    WindowsSandboxReadiness => "windowsSandbox/readiness" {
        params: #[ts(type = "undefined")] #[serde(skip_serializing_if = "Option::is_none")] Option<()>,
        serialization: global("config"),
        response: v2::WindowsSandboxReadinessResponse,
    },

    LoginAccount => "account/login/start" {
        params: v2::LoginAccountParams,
        inspect_params: true,
        serialization: global("account-auth"),
        response: v2::LoginAccountResponse,
    },

    CancelLoginAccount => "account/login/cancel" {
        params: v2::CancelLoginAccountParams,
        serialization: global("account-auth"),
        response: v2::CancelLoginAccountResponse,
    },

    AuthProfileList => "authProfile/list" {
        params: v2::AuthProfileListParams,
        serialization: global_shared_read("account-auth"),
        response: v2::AuthProfileListResponse,
    },

    AuthProfileSaveCurrent => "authProfile/saveCurrent" {
        params: v2::AuthProfileSaveCurrentParams,
        serialization: global("account-auth"),
        response: v2::AuthProfileSaveCurrentResponse,
    },

    AuthProfileSwitch => "authProfile/switch" {
        params: v2::AuthProfileSwitchParams,
        serialization: global("account-auth"),
        response: v2::AuthProfileSwitchResponse,
    },

    LogoutAccount => "account/logout" {
        params: #[ts(type = "undefined")] #[serde(skip_serializing_if = "Option::is_none")] Option<()>,
        serialization: global("account-auth"),
        response: v2::LogoutAccountResponse,
    },

    GetAccountRateLimits => "account/rateLimits/read" {
        params: #[serde(default, deserialize_with = "crate::protocol::serde_helpers::deserialize_null_default")] v2::GetAccountRateLimitsParams,
        serialization: None,
        response: v2::GetAccountRateLimitsResponse,
    },

    ConsumeAccountRateLimitResetCredit => "account/rateLimitResetCredit/consume" {
        params: v2::ConsumeAccountRateLimitResetCreditParams,
        serialization: global("account-auth"),
        response: v2::ConsumeAccountRateLimitResetCreditResponse,
    },

    GetAccountTokenUsage => "account/usage/read" {
        params: #[ts(type = "undefined")] #[serde(skip_serializing_if = "Option::is_none")] Option<()>,
        serialization: None,
        response: v2::GetAccountTokenUsageResponse,
    },

    SendAddCreditsNudgeEmail => "account/sendAddCreditsNudgeEmail" {
        params: v2::SendAddCreditsNudgeEmailParams,
        serialization: global("account-auth"),
        response: v2::SendAddCreditsNudgeEmailResponse,
    },

    FeedbackUpload => "feedback/upload" {
        params: v2::FeedbackUploadParams,
        serialization: None,
        response: v2::FeedbackUploadResponse,
    },

    /// Execute a standalone command (argv vector) under the server's sandbox.
    OneOffCommandExec => "command/exec" {
        params: v2::CommandExecParams,
        inspect_params: true,
        serialization: optional_command_process_id(params.process_id),
        response: v2::CommandExecResponse,
    },
    /// Write stdin bytes to a running `command/exec` session or close stdin.
    CommandExecWrite => "command/exec/write" {
        params: v2::CommandExecWriteParams,
        serialization: command_process_id(params.process_id),
        response: v2::CommandExecWriteResponse,
    },
    /// Terminate a running `command/exec` session by client-supplied `processId`.
    CommandExecTerminate => "command/exec/terminate" {
        params: v2::CommandExecTerminateParams,
        serialization: command_process_id(params.process_id),
        response: v2::CommandExecTerminateResponse,
    },
    /// Resize a running PTY-backed `command/exec` session by client-supplied `processId`.
    CommandExecResize => "command/exec/resize" {
        params: v2::CommandExecResizeParams,
        serialization: command_process_id(params.process_id),
        response: v2::CommandExecResizeResponse,
    },
    #[experimental("process/spawn")]
    /// Spawn a standalone process (argv vector) without a Codewith sandbox.
    ProcessSpawn => "process/spawn" {
        params: v2::ProcessSpawnParams,
        serialization: process_handle(params.process_handle),
        response: v2::ProcessSpawnResponse,
    },
    #[experimental("process/writeStdin")]
    /// Write stdin bytes to a running `process/spawn` session or close stdin.
    ProcessWriteStdin => "process/writeStdin" {
        params: v2::ProcessWriteStdinParams,
        serialization: process_handle(params.process_handle),
        response: v2::ProcessWriteStdinResponse,
    },
    #[experimental("process/kill")]
    /// Terminate a running `process/spawn` session by client-supplied `processHandle`.
    ProcessKill => "process/kill" {
        params: v2::ProcessKillParams,
        serialization: process_handle(params.process_handle),
        response: v2::ProcessKillResponse,
    },
    #[experimental("process/resizePty")]
    /// Resize a running PTY-backed `process/spawn` session by client-supplied `processHandle`.
    ProcessResizePty => "process/resizePty" {
        params: v2::ProcessResizePtyParams,
        serialization: process_handle(params.process_handle),
        response: v2::ProcessResizePtyResponse,
    },

    ConfigRead => "config/read" {
        params: v2::ConfigReadParams,
        serialization: global_shared_read("config"),
        response: v2::ConfigReadResponse,
    },
    ExternalAgentConfigDetect => "externalAgentConfig/detect" {
        params: v2::ExternalAgentConfigDetectParams,
        serialization: global("config"),
        response: v2::ExternalAgentConfigDetectResponse,
    },
    ExternalAgentConfigImport => "externalAgentConfig/import" {
        params: v2::ExternalAgentConfigImportParams,
        serialization: global("config"),
        response: v2::ExternalAgentConfigImportResponse,
    },
    ConfigValueWrite => "config/value/write" {
        params: v2::ConfigValueWriteParams,
        serialization: global("config"),
        manual_payload_conversion: manual,
        response: v2::ConfigWriteResponse,
    },
    ConfigBatchWrite => "config/batchWrite" {
        params: v2::ConfigBatchWriteParams,
        serialization: global("config"),
        manual_payload_conversion: manual,
        response: v2::ConfigWriteResponse,
    },

    ConfigRequirementsRead => "configRequirements/read" {
        params: #[ts(type = "undefined")] #[serde(skip_serializing_if = "Option::is_none")] Option<()>,
        serialization: global("config"),
        response: v2::ConfigRequirementsReadResponse,
    },

    GetAccount => "account/read" {
        params: v2::GetAccountParams,
        serialization: global("account-auth"),
        response: v2::GetAccountResponse,
    },

    /// DEPRECATED APIs below
    GetConversationSummary {
        params: v1::GetConversationSummaryParams,
        serialization: None,
        response: v1::GetConversationSummaryResponse,
    },
    GitDiffToRemote {
        params: v1::GitDiffToRemoteParams,
        serialization: None,
        response: v1::GitDiffToRemoteResponse,
    },
    /// DEPRECATED in favor of GetAccount
    GetAuthStatus {
        params: v1::GetAuthStatusParams,
        serialization: global("account-auth"),
        response: v1::GetAuthStatusResponse,
    },
    // Legacy fuzzy search cancellation is intentionally concurrent: clients reuse a
    // cancellation token so a newer request can cancel an older in-flight search.
    FuzzyFileSearch {
        params: FuzzyFileSearchParams,
        serialization: None,
        response: FuzzyFileSearchResponse,
    },
    #[experimental("fuzzyFileSearch/sessionStart")]
    FuzzyFileSearchSessionStart => "fuzzyFileSearch/sessionStart" {
        params: FuzzyFileSearchSessionStartParams,
        serialization: fuzzy_session_id(params.session_id),
        response: FuzzyFileSearchSessionStartResponse,
    },
    #[experimental("fuzzyFileSearch/sessionUpdate")]
    FuzzyFileSearchSessionUpdate => "fuzzyFileSearch/sessionUpdate" {
        params: FuzzyFileSearchSessionUpdateParams,
        serialization: fuzzy_session_id(params.session_id),
        response: FuzzyFileSearchSessionUpdateResponse,
    },
    #[experimental("fuzzyFileSearch/sessionStop")]
    FuzzyFileSearchSessionStop => "fuzzyFileSearch/sessionStop" {
        params: FuzzyFileSearchSessionStopParams,
        serialization: fuzzy_session_id(params.session_id),
        response: FuzzyFileSearchSessionStopResponse,
    },
}

/// Generates an `enum ServerRequest` where each variant is a request that the
/// server can send to the client along with the corresponding params and
/// response types. It also generates helper types used by the app/server
/// infrastructure (payload enum, request constructor, and export helpers).
macro_rules! server_request_definitions {
    (
        $(
            $(#[$variant_meta:meta])*
            $variant:ident $(=> $wire:literal)? {
                params: $params:ty,
                response: $response:ty,
            }
        ),* $(,)?
    ) => {
        /// Request initiated from the server and sent to the client.
        #[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
        #[allow(clippy::large_enum_variant)]
        #[serde(tag = "method", rename_all = "camelCase")]
        pub enum ServerRequest {
            $(
                $(#[$variant_meta])*
                $(#[serde(rename = $wire)] #[ts(rename = $wire)])?
                $variant {
                    #[serde(rename = "id")]
                    request_id: RequestId,
                    params: $params,
                },
            )*
        }

        impl ServerRequest {
            pub fn id(&self) -> &RequestId {
                match self {
                    $(Self::$variant { request_id, .. } => request_id,)*
                }
            }

            pub fn response_from_result(
                &self,
                result: crate::Result,
            ) -> serde_json::Result<ServerResponse> {
                match self {
                    $(
                        Self::$variant { request_id, .. } => {
                            let response = serde_json::from_value::<$response>(result)?;
                            Ok(ServerResponse::$variant {
                                request_id: request_id.clone(),
                                response,
                            })
                        }
                    )*
                }
            }
        }

        /// Typed response from the client to the server.
        #[derive(Serialize, Deserialize, Debug, Clone)]
        #[serde(tag = "method", rename_all = "camelCase")]
        pub enum ServerResponse {
            $(
                $(#[$variant_meta])*
                $(#[serde(rename = $wire)])?
                $variant {
                    #[serde(rename = "id")]
                    request_id: RequestId,
                    response: $response,
                },
            )*
        }

        impl ServerResponse {
            pub fn id(&self) -> &RequestId {
                match self {
                    $(Self::$variant { request_id, .. } => request_id,)*
                }
            }

            pub fn method(&self) -> String {
                serde_json::to_value(self)
                    .ok()
                    .and_then(|value| {
                        value
                            .get("method")
                            .and_then(serde_json::Value::as_str)
                            .map(str::to_owned)
                    })
                    .unwrap_or_else(|| "<unknown>".to_string())
            }
        }

        #[derive(Debug, Clone, PartialEq, JsonSchema)]
        #[allow(clippy::large_enum_variant)]
        pub enum ServerRequestPayload {
            $( $variant($params), )*
        }

        impl ServerRequestPayload {
            pub fn request_with_id(self, request_id: RequestId) -> ServerRequest {
                match self {
                    $(Self::$variant(params) => ServerRequest::$variant { request_id, params },)*
                }
            }
        }

        pub fn export_server_responses(
            out_dir: &::std::path::Path,
        ) -> ::std::result::Result<(), ::ts_rs::ExportError> {
            $(
                <$response as ::ts_rs::TS>::export_all_to(out_dir)?;
            )*
            Ok(())
        }

        pub(crate) fn visit_server_response_types(v: &mut impl ::ts_rs::TypeVisitor) {
            $(
                v.visit::<$response>();
            )*
        }

        #[allow(clippy::vec_init_then_push)]
        pub fn export_server_response_schemas(
            out_dir: &Path,
        ) -> ::anyhow::Result<Vec<GeneratedSchema>> {
            let mut schemas = Vec::new();
            $(
                schemas.push(crate::export::write_json_schema::<$response>(
                    out_dir,
                    concat!(stringify!($variant), "Response"),
                )?);
            )*
            Ok(schemas)
        }

        #[allow(clippy::vec_init_then_push)]
        pub fn export_server_param_schemas(
            out_dir: &Path,
        ) -> ::anyhow::Result<Vec<GeneratedSchema>> {
            let mut schemas = Vec::new();
            $(
                schemas.push(crate::export::write_json_schema::<$params>(
                    out_dir,
                    concat!(stringify!($variant), "Params"),
                )?);
            )*
            Ok(schemas)
        }
    };
}

/// Generates `ServerNotification` enum and helpers, including a JSON Schema
/// exporter for each notification.
macro_rules! server_notification_definitions {
    (
        $(
            $(#[$variant_meta:meta])*
            $variant:ident $(=> $wire:literal)? ( $payload:ty )
        ),* $(,)?
    ) => {
        /// Notification sent from the server to the client.
        #[derive(
            Serialize,
            Deserialize,
            Debug,
            Clone,
            JsonSchema,
            TS,
            Display,
            ExperimentalApi,
        )]
        #[allow(clippy::large_enum_variant)]
        #[serde(tag = "method", content = "params", rename_all = "camelCase")]
        #[strum(serialize_all = "camelCase")]
        pub enum ServerNotification {
            $(
                $(#[$variant_meta])*
                $(#[serde(rename = $wire)] #[ts(rename = $wire)] #[strum(serialize = $wire)])?
                $variant($payload),
            )*
        }

        impl ServerNotification {
            pub fn to_params(self) -> Result<serde_json::Value, serde_json::Error> {
                match self {
                    $(Self::$variant(params) => serde_json::to_value(params),)*
                }
            }
        }

        impl TryFrom<JSONRPCNotification> for ServerNotification {
            type Error = serde_json::Error;

            fn try_from(value: JSONRPCNotification) -> Result<Self, serde_json::Error> {
                serde_json::from_value(serde_json::to_value(value)?)
            }
        }

        #[allow(clippy::vec_init_then_push)]
        pub fn export_server_notification_schemas(
            out_dir: &::std::path::Path,
        ) -> ::anyhow::Result<Vec<GeneratedSchema>> {
            let mut schemas = Vec::new();
            $(schemas.push(crate::export::write_json_schema::<$payload>(out_dir, stringify!($payload))?);)*
            Ok(schemas)
        }
    };
}
/// Notifications sent from the client to the server.
macro_rules! client_notification_definitions {
    (
        $(
            $(#[$variant_meta:meta])*
            $variant:ident $( ( $payload:ty ) )?
        ),* $(,)?
    ) => {
        #[derive(Serialize, Deserialize, Debug, Clone, JsonSchema, TS, Display)]
        #[serde(tag = "method", content = "params", rename_all = "camelCase")]
        #[strum(serialize_all = "camelCase")]
        pub enum ClientNotification {
            $(
                $(#[$variant_meta])*
                $variant $( ( $payload ) )?,
            )*
        }

        pub fn export_client_notification_schemas(
            _out_dir: &::std::path::Path,
        ) -> ::anyhow::Result<Vec<GeneratedSchema>> {
            let schemas = Vec::new();
            $( $(schemas.push(crate::export::write_json_schema::<$payload>(_out_dir, stringify!($payload))?);)? )*
            Ok(schemas)
        }
    };
}

impl TryFrom<JSONRPCRequest> for ServerRequest {
    type Error = serde_json::Error;

    fn try_from(value: JSONRPCRequest) -> Result<Self, Self::Error> {
        serde_json::from_value(serde_json::to_value(value)?)
    }
}

server_request_definitions! {
    /// NEW APIs
    /// Sent when approval is requested for a specific command execution.
    /// This request is used for Turns started via turn/start.
    CommandExecutionRequestApproval => "item/commandExecution/requestApproval" {
        params: v2::CommandExecutionRequestApprovalParams,
        response: v2::CommandExecutionRequestApprovalResponse,
    },

    /// Sent when approval is requested for a specific file change.
    /// This request is used for Turns started via turn/start.
    FileChangeRequestApproval => "item/fileChange/requestApproval" {
        params: v2::FileChangeRequestApprovalParams,
        response: v2::FileChangeRequestApprovalResponse,
    },

    /// EXPERIMENTAL - Request input from the user for a tool call.
    ToolRequestUserInput => "item/tool/requestUserInput" {
        params: v2::ToolRequestUserInputParams,
        response: v2::ToolRequestUserInputResponse,
    },

    /// Request input for an MCP server elicitation.
    McpServerElicitationRequest => "mcpServer/elicitation/request" {
        params: v2::McpServerElicitationRequestParams,
        response: v2::McpServerElicitationRequestResponse,
    },

    /// Request approval for additional permissions from the user.
    PermissionsRequestApproval => "item/permissions/requestApproval" {
        params: v2::PermissionsRequestApprovalParams,
        response: v2::PermissionsRequestApprovalResponse,
    },

    /// Execute a dynamic tool call on the client.
    DynamicToolCall => "item/tool/call" {
        params: v2::DynamicToolCallParams,
        response: v2::DynamicToolCallResponse,
    },

    ChatgptAuthTokensRefresh => "account/chatgptAuthTokens/refresh" {
        params: v2::ChatgptAuthTokensRefreshParams,
        response: v2::ChatgptAuthTokensRefreshResponse,
    },

    /// Generate a fresh upstream attestation result on demand.
    AttestationGenerate => "attestation/generate" {
        params: v2::AttestationGenerateParams,
        response: v2::AttestationGenerateResponse,
    },

    /// DEPRECATED APIs below
    /// Request to approve a patch.
    /// This request is used for Turns started via the legacy APIs (i.e. SendUserTurn, SendUserMessage).
    ApplyPatchApproval {
        params: v1::ApplyPatchApprovalParams,
        response: v1::ApplyPatchApprovalResponse,
    },
    /// Request to exec a command.
    /// This request is used for Turns started via the legacy APIs (i.e. SendUserTurn, SendUserMessage).
    ExecCommandApproval {
        params: v1::ExecCommandApprovalParams,
        response: v1::ExecCommandApprovalResponse,
    },
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct FuzzyFileSearchParams {
    pub query: String,
    pub roots: Vec<String>,
    // if provided, will cancel any previous request that used the same value
    pub cancellation_token: Option<String>,
}

/// Superset of [`codex_file_search::FileMatch`]
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
pub struct FuzzyFileSearchResult {
    pub root: String,
    pub path: String,
    pub match_type: FuzzyFileSearchMatchType,
    pub file_name: String,
    pub score: u32,
    pub indices: Option<Vec<u32>>,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub enum FuzzyFileSearchMatchType {
    File,
    Directory,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
pub struct FuzzyFileSearchResponse {
    pub files: Vec<FuzzyFileSearchResult>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct FuzzyFileSearchSessionStartParams {
    pub session_id: String,
    pub roots: Vec<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS, Default)]
pub struct FuzzyFileSearchSessionStartResponse {}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct FuzzyFileSearchSessionUpdateParams {
    pub session_id: String,
    pub query: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS, Default)]
pub struct FuzzyFileSearchSessionUpdateResponse {}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct FuzzyFileSearchSessionStopParams {
    pub session_id: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS, Default)]
pub struct FuzzyFileSearchSessionStopResponse {}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct FuzzyFileSearchSessionUpdatedNotification {
    pub session_id: String,
    pub query: String,
    pub files: Vec<FuzzyFileSearchResult>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct FuzzyFileSearchSessionCompletedNotification {
    pub session_id: String,
}

server_notification_definitions! {
    /// NEW NOTIFICATIONS
    Error => "error" (v2::ErrorNotification),
    ThreadStarted => "thread/started" (v2::ThreadStartedNotification),
    ThreadStatusChanged => "thread/status/changed" (v2::ThreadStatusChangedNotification),
    ThreadArchived => "thread/archived" (v2::ThreadArchivedNotification),
    ThreadUnarchived => "thread/unarchived" (v2::ThreadUnarchivedNotification),
    ThreadClosed => "thread/closed" (v2::ThreadClosedNotification),
    SkillsChanged => "skills/changed" (v2::SkillsChangedNotification),
    ThreadNameUpdated => "thread/name/updated" (v2::ThreadNameUpdatedNotification),
    ThreadGoalUpdated => "thread/goal/updated" (v2::ThreadGoalUpdatedNotification),
    ThreadGoalPlanUpdated => "thread/goalPlan/updated" (v2::ThreadGoalPlanUpdatedNotification),
    ThreadGoalCleared => "thread/goal/cleared" (v2::ThreadGoalClearedNotification),
    ThreadScheduleUpdated => "thread/schedule/updated" (v2::ThreadScheduleUpdatedNotification),
    ThreadScheduleDeleted => "thread/schedule/deleted" (v2::ThreadScheduleDeletedNotification),
    ThreadScheduleRunUpdated => "thread/schedule/run/updated" (v2::ThreadScheduleRunUpdatedNotification),
    ThreadMonitorUpdated => "thread/monitor/updated" (v2::ThreadMonitorUpdatedNotification),
    ThreadMonitorDeleted => "thread/monitor/deleted" (v2::ThreadMonitorDeletedNotification),
    ThreadMonitorEvent => "thread/monitor/event" (v2::ThreadMonitorEventNotification),
    #[experimental("thread/externalAgent/event")]
    ThreadExternalAgentEvent => "thread/externalAgent/event" (v2::ThreadExternalAgentEventNotification),
    #[experimental("thread/settings/updated")]
    ThreadSettingsUpdated => "thread/settings/updated" (v2::ThreadSettingsUpdatedNotification),
    ThreadTokenUsageUpdated => "thread/tokenUsage/updated" (v2::ThreadTokenUsageUpdatedNotification),
    TurnStarted => "turn/started" (v2::TurnStartedNotification),
    HookStarted => "hook/started" (v2::HookStartedNotification),
    TurnCompleted => "turn/completed" (v2::TurnCompletedNotification),
    HookCompleted => "hook/completed" (v2::HookCompletedNotification),
    TurnDiffUpdated => "turn/diff/updated" (v2::TurnDiffUpdatedNotification),
    TurnPlanUpdated => "turn/plan/updated" (v2::TurnPlanUpdatedNotification),
    ItemStarted => "item/started" (v2::ItemStartedNotification),
    ItemGuardianApprovalReviewStarted => "item/autoApprovalReview/started" (v2::ItemGuardianApprovalReviewStartedNotification),
    ItemGuardianApprovalReviewCompleted => "item/autoApprovalReview/completed" (v2::ItemGuardianApprovalReviewCompletedNotification),
    ItemCompleted => "item/completed" (v2::ItemCompletedNotification),
    /// This event is internal-only. Used by Codewith Cloud.
    RawResponseItemCompleted => "rawResponseItem/completed" (v2::RawResponseItemCompletedNotification),
    AgentMessageDelta => "item/agentMessage/delta" (v2::AgentMessageDeltaNotification),
    /// EXPERIMENTAL - proposed plan streaming deltas for plan items.
    PlanDelta => "item/plan/delta" (v2::PlanDeltaNotification),
    /// Stream base64-encoded stdout/stderr chunks for a running `command/exec` session.
    CommandExecOutputDelta => "command/exec/outputDelta" (v2::CommandExecOutputDeltaNotification),
    /// Stream base64-encoded stdout/stderr chunks for a running `process/spawn` session.
    #[experimental("process/outputDelta")]
    ProcessOutputDelta => "process/outputDelta" (v2::ProcessOutputDeltaNotification),
    /// Final exit notification for a `process/spawn` session.
    #[experimental("process/exited")]
    ProcessExited => "process/exited" (v2::ProcessExitedNotification),
    CommandExecutionOutputDelta => "item/commandExecution/outputDelta" (v2::CommandExecutionOutputDeltaNotification),
    TerminalInteraction => "item/commandExecution/terminalInteraction" (v2::TerminalInteractionNotification),
    FileChangePatchUpdated => "item/fileChange/patchUpdated" (v2::FileChangePatchUpdatedNotification),
    ServerRequestResolved => "serverRequest/resolved" (v2::ServerRequestResolvedNotification),
    McpToolCallProgress => "item/mcpToolCall/progress" (v2::McpToolCallProgressNotification),
    McpServerOauthLoginCompleted => "mcpServer/oauthLogin/completed" (v2::McpServerOauthLoginCompletedNotification),
    McpServerStatusUpdated => "mcpServer/startupStatus/updated" (v2::McpServerStatusUpdatedNotification),
    AccountUpdated => "account/updated" (v2::AccountUpdatedNotification),
    AccountRateLimitsUpdated => "account/rateLimits/updated" (v2::AccountRateLimitsUpdatedNotification),
    AppListUpdated => "app/list/updated" (v2::AppListUpdatedNotification),
    RemoteControlStatusChanged => "remoteControl/status/changed" (v2::RemoteControlStatusChangedNotification),
    ExternalAgentConfigImportCompleted => "externalAgentConfig/import/completed" (v2::ExternalAgentConfigImportCompletedNotification),
    FsChanged => "fs/changed" (v2::FsChangedNotification),
    ReasoningSummaryTextDelta => "item/reasoning/summaryTextDelta" (v2::ReasoningSummaryTextDeltaNotification),
    ReasoningSummaryPartAdded => "item/reasoning/summaryPartAdded" (v2::ReasoningSummaryPartAddedNotification),
    ReasoningTextDelta => "item/reasoning/textDelta" (v2::ReasoningTextDeltaNotification),
    /// Deprecated: Use `ContextCompaction` item type instead.
    ContextCompacted => "thread/compacted" (v2::ContextCompactedNotification),
    ModelRerouted => "model/rerouted" (v2::ModelReroutedNotification),
    ModelVerification => "model/verification" (v2::ModelVerificationNotification),
    #[experimental("turn/moderationMetadata")]
    TurnModerationMetadata => "turn/moderationMetadata" (v2::TurnModerationMetadataNotification),
    Warning => "warning" (v2::WarningNotification),
    GuardianWarning => "guardianWarning" (v2::GuardianWarningNotification),
    DeprecationNotice => "deprecationNotice" (v2::DeprecationNoticeNotification),
    ConfigWarning => "configWarning" (v2::ConfigWarningNotification),
    FuzzyFileSearchSessionUpdated => "fuzzyFileSearch/sessionUpdated" (FuzzyFileSearchSessionUpdatedNotification),
    FuzzyFileSearchSessionCompleted => "fuzzyFileSearch/sessionCompleted" (FuzzyFileSearchSessionCompletedNotification),
    #[experimental("thread/realtime/started")]
    ThreadRealtimeStarted => "thread/realtime/started" (v2::ThreadRealtimeStartedNotification),
    #[experimental("thread/realtime/itemAdded")]
    ThreadRealtimeItemAdded => "thread/realtime/itemAdded" (v2::ThreadRealtimeItemAddedNotification),
    #[experimental("thread/realtime/transcript/delta")]
    ThreadRealtimeTranscriptDelta => "thread/realtime/transcript/delta" (v2::ThreadRealtimeTranscriptDeltaNotification),
    #[experimental("thread/realtime/transcript/done")]
    ThreadRealtimeTranscriptDone => "thread/realtime/transcript/done" (v2::ThreadRealtimeTranscriptDoneNotification),
    #[experimental("thread/realtime/outputAudio/delta")]
    ThreadRealtimeOutputAudioDelta => "thread/realtime/outputAudio/delta" (v2::ThreadRealtimeOutputAudioDeltaNotification),
    #[experimental("thread/realtime/sdp")]
    ThreadRealtimeSdp => "thread/realtime/sdp" (v2::ThreadRealtimeSdpNotification),
    #[experimental("thread/realtime/error")]
    ThreadRealtimeError => "thread/realtime/error" (v2::ThreadRealtimeErrorNotification),
    #[experimental("thread/realtime/closed")]
    ThreadRealtimeClosed => "thread/realtime/closed" (v2::ThreadRealtimeClosedNotification),

    /// Notifies the user of world-writable directories on Windows, which cannot be protected by the sandbox.
    WindowsWorldWritableWarning => "windows/worldWritableWarning" (v2::WindowsWorldWritableWarningNotification),
    WindowsSandboxSetupCompleted => "windowsSandbox/setupCompleted" (v2::WindowsSandboxSetupCompletedNotification),

    #[serde(rename = "account/login/completed")]
    #[ts(rename = "account/login/completed")]
    #[strum(serialize = "account/login/completed")]
    AccountLoginCompleted(v2::AccountLoginCompletedNotification),

}

client_notification_definitions! {
    Initialized,
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::Result;
    use codex_protocol::ThreadId;
    use codex_protocol::account::PlanType;
    use codex_protocol::models::BUILT_IN_PERMISSION_PROFILE_READ_ONLY;
    use codex_protocol::parse_command::ParsedCommand;
    use codex_protocol::protocol::RealtimeConversationVersion;
    use codex_protocol::protocol::RealtimeOutputModality;
    use codex_protocol::protocol::RealtimeVoice;
    use codex_utils_absolute_path::AbsolutePathBuf;
    use codex_utils_absolute_path::test_support::PathBufExt;
    use codex_utils_absolute_path::test_support::test_path_buf;
    use pretty_assertions::assert_eq;
    use serde_json::json;
    use std::path::PathBuf;

    fn absolute_path_string(path: &str) -> String {
        let path = format!("/{}", path.trim_start_matches('/'));
        test_path_buf(&path).display().to_string()
    }

    fn absolute_path(path: &str) -> AbsolutePathBuf {
        let path = format!("/{}", path.trim_start_matches('/'));
        test_path_buf(&path).abs()
    }

    fn request_id() -> RequestId {
        const REQUEST_ID: i64 = 1;
        RequestId::Integer(REQUEST_ID)
    }

    #[test]
    fn client_request_serialization_scope_covers_keyed_families() {
        let thread_id = "thread-1".to_string();
        let thread_resume = ClientRequest::ThreadResume {
            request_id: request_id(),
            params: v2::ThreadResumeParams {
                thread_id: thread_id.clone(),
                ..Default::default()
            },
        };
        assert_eq!(
            thread_resume.serialization_scope(),
            Some(ClientRequestSerializationScope::Thread {
                thread_id: thread_id.clone()
            })
        );

        let thread_resume_with_path = ClientRequest::ThreadResume {
            request_id: request_id(),
            params: v2::ThreadResumeParams {
                thread_id: thread_id.clone(),
                path: Some(PathBuf::from("/tmp/resume-thread.jsonl")),
                ..Default::default()
            },
        };
        assert_eq!(
            thread_resume_with_path.serialization_scope(),
            Some(ClientRequestSerializationScope::Thread {
                thread_id: thread_id.clone()
            })
        );

        let thread_fork = ClientRequest::ThreadFork {
            request_id: request_id(),
            params: v2::ThreadForkParams {
                thread_id: thread_id.clone(),
                path: Some(PathBuf::from("/tmp/source-thread.jsonl")),
                ..Default::default()
            },
        };
        assert_eq!(
            thread_fork.serialization_scope(),
            Some(ClientRequestSerializationScope::Thread { thread_id })
        );

        let command_exec = ClientRequest::OneOffCommandExec {
            request_id: request_id(),
            params: v2::CommandExecParams {
                command: vec!["sleep".to_string(), "10".to_string()],
                process_id: Some("proc-1".to_string()),
                tty: false,
                stream_stdin: false,
                stream_stdout_stderr: false,
                output_bytes_cap: None,
                disable_output_cap: false,
                disable_timeout: false,
                timeout_ms: None,
                cwd: None,
                env: None,
                size: None,
                sandbox_policy: None,
                permission_profile: None,
            },
        };
        assert_eq!(
            command_exec.serialization_scope(),
            Some(ClientRequestSerializationScope::CommandExecProcess {
                process_id: "proc-1".to_string()
            })
        );

        let fuzzy_update = ClientRequest::FuzzyFileSearchSessionUpdate {
            request_id: request_id(),
            params: FuzzyFileSearchSessionUpdateParams {
                session_id: "search-1".to_string(),
                query: "lib".to_string(),
            },
        };
        assert_eq!(
            fuzzy_update.serialization_scope(),
            Some(ClientRequestSerializationScope::FuzzyFileSearchSession {
                session_id: "search-1".to_string()
            })
        );

        let fs_watch = ClientRequest::FsWatch {
            request_id: request_id(),
            params: v2::FsWatchParams {
                watch_id: "watch-1".to_string(),
                path: absolute_path("/tmp/repo"),
            },
        };
        assert_eq!(
            fs_watch.serialization_scope(),
            Some(ClientRequestSerializationScope::FsWatch {
                watch_id: "watch-1".to_string()
            })
        );

        let plugin_install = ClientRequest::PluginInstall {
            request_id: request_id(),
            params: v2::PluginInstallParams {
                marketplace_path: Some(absolute_path("/tmp/marketplace")),
                remote_marketplace_name: None,
                plugin_name: "plugin-a".to_string(),
            },
        };
        assert_eq!(
            plugin_install.serialization_scope(),
            Some(ClientRequestSerializationScope::Global("config"))
        );

        let skills_list = ClientRequest::SkillsList {
            request_id: request_id(),
            params: v2::SkillsListParams {
                cwds: Vec::new(),
                force_reload: false,
            },
        };
        assert_eq!(
            skills_list.serialization_scope(),
            Some(ClientRequestSerializationScope::GlobalSharedRead("config"))
        );

        let skills_extra_roots_set = ClientRequest::SkillsExtraRootsSet {
            request_id: request_id(),
            params: v2::SkillsExtraRootsSetParams {
                extra_roots: vec![absolute_path("/tmp/skills")],
            },
        };
        assert_eq!(
            skills_extra_roots_set.serialization_scope(),
            Some(ClientRequestSerializationScope::Global("config"))
        );

        let plugin_list = ClientRequest::PluginList {
            request_id: request_id(),
            params: v2::PluginListParams {
                cwds: None,
                marketplace_kinds: None,
            },
        };
        assert_eq!(plugin_list.serialization_scope(), None);

        let plugin_read = ClientRequest::PluginRead {
            request_id: request_id(),
            params: v2::PluginReadParams {
                marketplace_path: Some(absolute_path("/tmp/marketplace")),
                remote_marketplace_name: None,
                plugin_name: "plugin-a".to_string(),
            },
        };
        assert_eq!(plugin_read.serialization_scope(), None);

        let plugin_installed = ClientRequest::PluginInstalled {
            request_id: request_id(),
            params: v2::PluginInstalledParams {
                cwds: None,
                install_suggestion_plugin_names: None,
            },
        };
        assert_eq!(plugin_installed.serialization_scope(), None);

        let plugin_uninstall = ClientRequest::PluginUninstall {
            request_id: request_id(),
            params: v2::PluginUninstallParams {
                plugin_id: "plugin-a".to_string(),
            },
        };
        assert_eq!(
            plugin_uninstall.serialization_scope(),
            Some(ClientRequestSerializationScope::Global("config"))
        );

        let mcp_oauth = ClientRequest::McpServerOauthLogin {
            request_id: request_id(),
            params: v2::McpServerOauthLoginParams {
                name: "server-a".to_string(),
                scopes: None,
                timeout_secs: None,
            },
        };
        assert_eq!(
            mcp_oauth.serialization_scope(),
            Some(ClientRequestSerializationScope::McpOauth {
                server_name: "server-a".to_string()
            })
        );

        let mcp_resource_read = ClientRequest::McpResourceRead {
            request_id: request_id(),
            params: v2::McpResourceReadParams {
                thread_id: Some("thread-1".to_string()),
                server: "server-a".to_string(),
                uri: "file:///tmp/resource".to_string(),
            },
        };
        assert_eq!(
            mcp_resource_read.serialization_scope(),
            Some(ClientRequestSerializationScope::Thread {
                thread_id: "thread-1".to_string()
            })
        );

        let config_read = ClientRequest::ConfigRead {
            request_id: request_id(),
            params: v2::ConfigReadParams {
                include_layers: false,
                cwd: None,
            },
        };
        assert_eq!(
            config_read.serialization_scope(),
            Some(ClientRequestSerializationScope::GlobalSharedRead("config"))
        );

        let account_read = ClientRequest::GetAccount {
            request_id: request_id(),
            params: v2::GetAccountParams {
                refresh_token: false,
            },
        };
        assert_eq!(
            account_read.serialization_scope(),
            Some(ClientRequestSerializationScope::Global("account-auth"))
        );

        let thread_goal_set = ClientRequest::ThreadGoalSet {
            request_id: request_id(),
            params: v2::ThreadGoalSetParams {
                thread_id: "goal-thread".to_string(),
                objective: Some("ship it".to_string()),
                title: None,
                status: None,
                token_budget: None,
            },
        };
        assert_eq!(
            thread_goal_set.serialization_scope(),
            Some(ClientRequestSerializationScope::Thread {
                thread_id: "goal-thread".to_string()
            })
        );

        let guardian_approval = ClientRequest::ThreadApproveGuardianDeniedAction {
            request_id: request_id(),
            params: v2::ThreadApproveGuardianDeniedActionParams {
                thread_id: "guardian-thread".to_string(),
                event: json!({ "type": "guardian" }),
            },
        };
        assert_eq!(
            guardian_approval.serialization_scope(),
            Some(ClientRequestSerializationScope::Thread {
                thread_id: "guardian-thread".to_string()
            })
        );

        let marketplace_remove = ClientRequest::MarketplaceRemove {
            request_id: request_id(),
            params: v2::MarketplaceRemoveParams {
                marketplace_name: "marketplace".to_string(),
            },
        };
        assert_eq!(
            marketplace_remove.serialization_scope(),
            Some(ClientRequestSerializationScope::Global("config"))
        );

        let add_credits_nudge = ClientRequest::SendAddCreditsNudgeEmail {
            request_id: request_id(),
            params: v2::SendAddCreditsNudgeEmailParams {
                credit_type: v2::AddCreditsNudgeCreditType::Credits,
            },
        };
        assert_eq!(
            add_credits_nudge.serialization_scope(),
            Some(ClientRequestSerializationScope::Global("account-auth"))
        );

        let environment_add = ClientRequest::EnvironmentAdd {
            request_id: request_id(),
            params: v2::EnvironmentAddParams {
                environment_id: "remote-a".to_string(),
                exec_server_url: "ws://127.0.0.1:8765".to_string(),
            },
        };
        assert_eq!(
            environment_add.serialization_scope(),
            Some(ClientRequestSerializationScope::Global("environment"))
        );
    }

    #[test]
    fn active_session_send_serialization_scope_normalizes_thread_ids() {
        let thread_id = ThreadId::new().to_string();
        let upper_thread_id = thread_id.to_ascii_uppercase();

        let legacy_thread_target = ClientRequest::ActiveSessionSend {
            request_id: request_id(),
            params: v2::ActiveSessionSendParams {
                target_thread_id: Some(upper_thread_id.clone()),
                target_peer_id: None,
                message: "hello".to_string(),
                sender_thread_id: None,
                sender_label: None,
                delivery: Some(v2::ActiveSessionMessageDelivery::QueueOnly),
            },
        };
        assert_eq!(
            legacy_thread_target.serialization_scope(),
            Some(ClientRequestSerializationScope::Thread {
                thread_id: thread_id.clone()
            })
        );

        let peer_thread_target = ClientRequest::ActiveSessionSend {
            request_id: request_id(),
            params: v2::ActiveSessionSendParams {
                target_thread_id: None,
                target_peer_id: Some(upper_thread_id),
                message: "hello".to_string(),
                sender_thread_id: None,
                sender_label: None,
                delivery: Some(v2::ActiveSessionMessageDelivery::QueueOnly),
            },
        };
        assert_eq!(
            peer_thread_target.serialization_scope(),
            Some(ClientRequestSerializationScope::Thread { thread_id })
        );

        let bridge_peer_target = ClientRequest::ActiveSessionSend {
            request_id: request_id(),
            params: v2::ActiveSessionSendParams {
                target_thread_id: None,
                target_peer_id: Some("claude:session-1".to_string()),
                message: "hello".to_string(),
                sender_thread_id: None,
                sender_label: None,
                delivery: Some(v2::ActiveSessionMessageDelivery::QueueOnly),
            },
        };
        assert_eq!(
            bridge_peer_target.serialization_scope(),
            Some(ClientRequestSerializationScope::ActivePeer {
                peer_id: "claude:session-1".to_string()
            })
        );
    }

    #[test]
    fn active_session_wire_contract_keeps_peer_and_message_ids_explicit() -> Result<()> {
        let target_thread_id = ThreadId::new().to_string();
        let sender_thread_id = ThreadId::new().to_string();
        let cwd = absolute_path("workspace");
        let cwd_string = absolute_path_string("workspace");

        let list_response = v2::ActiveSessionListResponse {
            data: vec![v2::ActiveSessionPeer {
                peer_id: "bridge:session-1".to_string(),
                kind: v2::ActiveSessionPeerKind::BridgeAdapter,
                thread_id: target_thread_id.clone(),
                session_id: "sess-live-1".to_string(),
                cwd,
                display_name: Some("remote review".to_string()),
                agent_path: Some("/review".to_string()),
                auth_profile: Some("work".to_string()),
                auth_profile_kind: v2::AuthProfileKind::Named,
                capabilities: vec![
                    v2::ActiveSessionCapability::ReceiveMessage,
                    v2::ActiveSessionCapability::QueueMessage,
                    v2::ActiveSessionCapability::TriggerTurn,
                    v2::ActiveSessionCapability::ClaudeChannelBridge,
                ],
                last_seen_at: 1_781_790_000,
            }],
            next_cursor: Some("bridge:session-1".to_string()),
        };

        assert_eq!(
            serde_json::to_value(list_response)?,
            json!({
                "data": [{
                    "peerId": "bridge:session-1",
                    "kind": "bridgeAdapter",
                    "threadId": target_thread_id,
                    "sessionId": "sess-live-1",
                    "cwd": cwd_string,
                    "displayName": "remote review",
                    "agentPath": "/review",
                    "authProfile": "work",
                    "authProfileKind": "named",
                    "capabilities": [
                        "receiveMessage",
                        "queueMessage",
                        "triggerTurn",
                        "claudeChannelBridge"
                    ],
                    "lastSeenAt": 1781790000
                }],
                "nextCursor": "bridge:session-1"
            })
        );

        let send_params = v2::ActiveSessionSendParams {
            target_thread_id: None,
            target_peer_id: Some("bridge:session-1".to_string()),
            message: "Please inspect retry handling.".to_string(),
            sender_thread_id: Some(sender_thread_id.clone()),
            sender_label: Some("coordinator".to_string()),
            delivery: Some(v2::ActiveSessionMessageDelivery::TriggerTurn),
        };

        assert_eq!(
            serde_json::to_value(send_params)?,
            json!({
                "targetThreadId": null,
                "targetPeerId": "bridge:session-1",
                "message": "Please inspect retry handling.",
                "senderThreadId": sender_thread_id,
                "senderLabel": "coordinator",
                "delivery": "triggerTurn"
            })
        );

        let send_response = v2::ActiveSessionSendResponse {
            status: v2::ActiveSessionSendStatus::Delivered,
            message_id: "msg-active-1".to_string(),
            target_peer_id: "bridge:session-1".to_string(),
            target_thread_id: Some(target_thread_id.clone()),
            sender_thread_id: Some(sender_thread_id.clone()),
            reason: None,
        };

        assert_eq!(
            serde_json::to_value(send_response)?,
            json!({
                "status": "delivered",
                "messageId": "msg-active-1",
                "targetPeerId": "bridge:session-1",
                "targetThreadId": target_thread_id,
                "senderThreadId": sender_thread_id,
                "reason": null
            })
        );

        let legacy_list_response: v2::ActiveSessionListResponse = serde_json::from_value(json!({
            "data": [{
                "threadId": target_thread_id,
                "cwd": cwd_string,
                "displayName": "local session",
                "agentPath": null,
                "lastSeenAt": 1781790000
            }],
            "nextCursor": null
        }))?;
        assert_eq!(legacy_list_response.data[0].peer_id, "");
        assert_eq!(
            legacy_list_response.data[0].kind,
            v2::ActiveSessionPeerKind::CodewithSession
        );
        assert_eq!(legacy_list_response.data[0].session_id, "");
        assert_eq!(legacy_list_response.data[0].capabilities, Vec::new());

        let legacy_send_response: v2::ActiveSessionSendResponse = serde_json::from_value(json!({
            "status": "delivered",
            "messageId": "msg-active-1",
            "targetThreadId": null,
            "senderThreadId": null,
            "reason": null
        }))?;
        assert_eq!(legacy_send_response.target_peer_id, "");

        Ok(())
    }

    #[test]
    fn local_session_wire_contract_separates_durable_and_live_identity() -> Result<()> {
        let thread_id = ThreadId::new().to_string();
        let peer_id = thread_id.clone();
        let cwd = absolute_path("workspace");
        let cwd_string = absolute_path_string("workspace");

        let response = v2::LocalSessionListResponse {
            data: vec![v2::LocalSession {
                thread_id: thread_id.clone(),
                runtime_session_id: Some("sess-live-1".to_string()),
                peer: Some(v2::LocalSessionPeer {
                    peer_id: thread_id.clone(),
                    kind: v2::ActiveSessionPeerKind::CodewithSession,
                    capabilities: vec![
                        v2::ActiveSessionCapability::ReceiveMessage,
                        v2::ActiveSessionCapability::QueueMessage,
                    ],
                    last_seen_at: 1_781_790_000,
                }),
                status: v2::LocalSessionStatus::Active,
                active_flags: vec![v2::ThreadActiveFlag::WaitingOnUserInput],
                cwd,
                display_name: Some("coordinator".to_string()),
                agent_path: None,
                model_provider: "openai".to_string(),
                model: Some("gpt-5.2".to_string()),
                auth_profile: Some("work".to_string()),
                auth_profile_kind: v2::AuthProfileKind::Named,
                account_label: Some("work@example.com".to_string()),
                source: v2::SessionSource::Cli,
                thread_source: Some(v2::ThreadSource::User),
                created_at: 1_781_700_000,
                updated_at: 1_781_790_000,
                path: None,
                git_info: Some(v2::LocalSessionGitInfo {
                    sha: Some("abc123".to_string()),
                    branch: Some("main".to_string()),
                }),
                redactions: vec![
                    v2::LocalSessionRedaction::GitOriginUrl,
                    v2::LocalSessionRedaction::ProcessDetails,
                ],
            }],
            next_cursor: None,
        };

        assert_eq!(
            serde_json::to_value(response)?,
            json!({
                "data": [{
                    "threadId": thread_id,
                    "runtimeSessionId": "sess-live-1",
                    "peer": {
                        "peerId": peer_id,
                        "kind": "codewithSession",
                        "capabilities": ["receiveMessage", "queueMessage"],
                        "lastSeenAt": 1781790000
                    },
                    "status": "active",
                    "activeFlags": ["waitingOnUserInput"],
                    "cwd": cwd_string,
                    "displayName": "coordinator",
                    "agentPath": null,
                    "modelProvider": "openai",
                    "model": "gpt-5.2",
                    "authProfile": "work",
                    "authProfileKind": "named",
                    "accountLabel": "work@example.com",
                    "source": "cli",
                    "threadSource": "user",
                    "createdAt": 1781700000,
                    "updatedAt": 1781790000,
                    "path": null,
                    "gitInfo": {
                        "sha": "abc123",
                        "branch": "main"
                    },
                    "redactions": ["gitOriginUrl", "processDetails"]
                }],
                "nextCursor": null
            })
        );

        Ok(())
    }

    #[test]
    fn client_request_serialization_scope_covers_unkeyed_representatives() {
        let initialize = ClientRequest::Initialize {
            request_id: request_id(),
            params: v1::InitializeParams {
                client_info: v1::ClientInfo {
                    name: "test".to_string(),
                    title: None,
                    version: "0.1.0".to_string(),
                },
                capabilities: None,
            },
        };
        assert_eq!(initialize.serialization_scope(), None);

        let thread_start = ClientRequest::ThreadStart {
            request_id: request_id(),
            params: v2::ThreadStartParams::default(),
        };
        assert_eq!(thread_start.serialization_scope(), None);

        let command_exec = ClientRequest::OneOffCommandExec {
            request_id: request_id(),
            params: v2::CommandExecParams {
                command: vec!["true".to_string()],
                process_id: None,
                tty: false,
                stream_stdin: false,
                stream_stdout_stderr: false,
                output_bytes_cap: None,
                disable_output_cap: false,
                disable_timeout: false,
                timeout_ms: None,
                cwd: None,
                env: None,
                size: None,
                sandbox_policy: None,
                permission_profile: None,
            },
        };
        assert_eq!(command_exec.serialization_scope(), None);

        let fs_read = ClientRequest::FsReadFile {
            request_id: request_id(),
            params: v2::FsReadFileParams {
                path: absolute_path("/tmp/file.txt"),
            },
        };
        assert_eq!(fs_read.serialization_scope(), None);

        let thread_turns_list = ClientRequest::ThreadTurnsList {
            request_id: request_id(),
            params: v2::ThreadTurnsListParams {
                thread_id: "thread-1".to_string(),
                cursor: None,
                limit: None,
                sort_direction: None,
                items_view: None,
            },
        };
        assert_eq!(thread_turns_list.serialization_scope(), None);

        let thread_turns_items_list = ClientRequest::ThreadTurnsItemsList {
            request_id: request_id(),
            params: v2::ThreadTurnsItemsListParams {
                thread_id: "thread-1".to_string(),
                turn_id: "turn-1".to_string(),
                cursor: None,
                limit: None,
                sort_direction: None,
            },
        };
        assert_eq!(thread_turns_items_list.serialization_scope(), None);

        let mcp_resource_read = ClientRequest::McpResourceRead {
            request_id: request_id(),
            params: v2::McpResourceReadParams {
                thread_id: None,
                server: "server-a".to_string(),
                uri: "file:///tmp/resource".to_string(),
            },
        };
        assert_eq!(mcp_resource_read.serialization_scope(), None);

        let remote_control_pairing_start = ClientRequest::RemoteControlPairingStart {
            request_id: request_id(),
            params: v2::RemoteControlPairingStartParams::default(),
        };
        assert_eq!(
            remote_control_pairing_start.serialization_scope(),
            Some(ClientRequestSerializationScope::Global(
                "remote-control-pairing"
            ))
        );
        let remote_control_pairing_status = ClientRequest::RemoteControlPairingStatus {
            request_id: request_id(),
            params: v2::RemoteControlPairingStatusParams {
                pairing_code: Some("pairing-code".to_string()),
                manual_pairing_code: None,
            },
        };
        assert_eq!(
            remote_control_pairing_status.serialization_scope(),
            Some(ClientRequestSerializationScope::GlobalSharedRead(
                "remote-control-pairing"
            ))
        );
        let remote_control_clients_list = ClientRequest::RemoteControlClientsList {
            request_id: request_id(),
            params: v2::RemoteControlClientsListParams::default(),
        };
        assert_eq!(
            remote_control_clients_list.serialization_scope(),
            Some(ClientRequestSerializationScope::GlobalSharedRead(
                "remote-control-clients"
            ))
        );
        let remote_control_clients_revoke = ClientRequest::RemoteControlClientsRevoke {
            request_id: request_id(),
            params: v2::RemoteControlClientsRevokeParams {
                environment_id: "environment-id".to_string(),
                client_id: "client-id".to_string(),
            },
        };
        assert_eq!(
            remote_control_clients_revoke.serialization_scope(),
            Some(ClientRequestSerializationScope::Global(
                "remote-control-clients"
            ))
        );
    }

    #[test]
    fn serialize_get_conversation_summary() -> Result<()> {
        let request = ClientRequest::GetConversationSummary {
            request_id: RequestId::Integer(42),
            params: v1::GetConversationSummaryParams::ThreadId {
                conversation_id: ThreadId::from_string("67e55044-10b1-426f-9247-bb680e5fe0c8")?,
            },
        };
        assert_eq!(
            json!({
                "method": "getConversationSummary",
                "id": 42,
                "params": {
                    "conversationId": "67e55044-10b1-426f-9247-bb680e5fe0c8"
                }
            }),
            serde_json::to_value(&request)?,
        );
        Ok(())
    }

    #[test]
    fn serialize_initialize_with_opt_out_notification_methods() -> Result<()> {
        let request = ClientRequest::Initialize {
            request_id: RequestId::Integer(42),
            params: v1::InitializeParams {
                client_info: v1::ClientInfo {
                    name: "codex_vscode".to_string(),
                    title: Some("Codex VS Code Extension".to_string()),
                    version: "0.1.0".to_string(),
                },
                capabilities: Some(v1::InitializeCapabilities {
                    experimental_api: true,
                    request_attestation: true,
                    opt_out_notification_methods: Some(vec![
                        "thread/started".to_string(),
                        "item/agentMessage/delta".to_string(),
                    ]),
                }),
            },
        };

        assert_eq!(
            json!({
                "method": "initialize",
                "id": 42,
                "params": {
                    "clientInfo": {
                        "name": "codex_vscode",
                        "title": "Codex VS Code Extension",
                        "version": "0.1.0"
                    },
                    "capabilities": {
                        "experimentalApi": true,
                        "requestAttestation": true,
                        "optOutNotificationMethods": [
                            "thread/started",
                            "item/agentMessage/delta"
                        ]
                    }
                }
            }),
            serde_json::to_value(&request)?,
        );
        Ok(())
    }

    #[test]
    fn deserialize_initialize_with_opt_out_notification_methods() -> Result<()> {
        let request: ClientRequest = serde_json::from_value(json!({
            "method": "initialize",
            "id": 42,
            "params": {
                "clientInfo": {
                    "name": "codex_vscode",
                    "title": "Codex VS Code Extension",
                    "version": "0.1.0"
                },
                "capabilities": {
                    "experimentalApi": true,
                    "requestAttestation": true,
                    "optOutNotificationMethods": [
                        "thread/started",
                        "item/agentMessage/delta"
                    ]
                }
            }
        }))?;

        assert_eq!(
            request,
            ClientRequest::Initialize {
                request_id: RequestId::Integer(42),
                params: v1::InitializeParams {
                    client_info: v1::ClientInfo {
                        name: "codex_vscode".to_string(),
                        title: Some("Codex VS Code Extension".to_string()),
                        version: "0.1.0".to_string(),
                    },
                    capabilities: Some(v1::InitializeCapabilities {
                        experimental_api: true,
                        request_attestation: true,
                        opt_out_notification_methods: Some(vec![
                            "thread/started".to_string(),
                            "item/agentMessage/delta".to_string(),
                        ]),
                    }),
                },
            }
        );
        Ok(())
    }

    #[test]
    fn conversation_id_serializes_as_plain_string() -> Result<()> {
        let id = ThreadId::from_string("67e55044-10b1-426f-9247-bb680e5fe0c8")?;

        assert_eq!(
            json!("67e55044-10b1-426f-9247-bb680e5fe0c8"),
            serde_json::to_value(id)?
        );
        Ok(())
    }

    #[test]
    fn conversation_id_deserializes_from_plain_string() -> Result<()> {
        let id: ThreadId = serde_json::from_value(json!("67e55044-10b1-426f-9247-bb680e5fe0c8"))?;

        assert_eq!(
            ThreadId::from_string("67e55044-10b1-426f-9247-bb680e5fe0c8")?,
            id,
        );
        Ok(())
    }

    #[test]
    fn serialize_client_notification() -> Result<()> {
        let notification = ClientNotification::Initialized;
        // Note there is no "params" field for this notification.
        assert_eq!(
            json!({
                "method": "initialized",
            }),
            serde_json::to_value(&notification)?,
        );
        Ok(())
    }

    #[test]
    fn serialize_server_request() -> Result<()> {
        let conversation_id = ThreadId::from_string("67e55044-10b1-426f-9247-bb680e5fe0c8")?;
        let params = v1::ExecCommandApprovalParams {
            conversation_id,
            call_id: "call-42".to_string(),
            approval_id: Some("approval-42".to_string()),
            command: vec!["echo".to_string(), "hello".to_string()],
            cwd: PathBuf::from("/tmp"),
            reason: Some("because tests".to_string()),
            parsed_cmd: vec![ParsedCommand::Unknown {
                cmd: "echo hello".to_string(),
            }],
        };
        let request = ServerRequest::ExecCommandApproval {
            request_id: RequestId::Integer(7),
            params: params.clone(),
        };

        assert_eq!(
            json!({
                "method": "execCommandApproval",
                "id": 7,
                "params": {
                    "conversationId": "67e55044-10b1-426f-9247-bb680e5fe0c8",
                    "callId": "call-42",
                    "approvalId": "approval-42",
                    "command": ["echo", "hello"],
                    "cwd": "/tmp",
                    "reason": "because tests",
                    "parsedCmd": [
                        {
                            "type": "unknown",
                            "cmd": "echo hello"
                        }
                    ]
                }
            }),
            serde_json::to_value(&request)?,
        );

        let payload = ServerRequestPayload::ExecCommandApproval(params);
        assert_eq!(request.id(), &RequestId::Integer(7));
        assert_eq!(payload.request_with_id(RequestId::Integer(7)), request);
        Ok(())
    }

    #[test]
    fn serialize_chatgpt_auth_tokens_refresh_request() -> Result<()> {
        let request = ServerRequest::ChatgptAuthTokensRefresh {
            request_id: RequestId::Integer(8),
            params: v2::ChatgptAuthTokensRefreshParams {
                reason: v2::ChatgptAuthTokensRefreshReason::Unauthorized,
                previous_account_id: Some("org-123".to_string()),
            },
        };
        assert_eq!(
            json!({
                "method": "account/chatgptAuthTokens/refresh",
                "id": 8,
                "params": {
                    "reason": "unauthorized",
                    "previousAccountId": "org-123"
                }
            }),
            serde_json::to_value(&request)?,
        );
        Ok(())
    }

    #[test]
    fn serialize_attestation_generate_request() -> Result<()> {
        let params = v2::AttestationGenerateParams {};
        let request = ServerRequest::AttestationGenerate {
            request_id: RequestId::Integer(9),
            params: params.clone(),
        };
        assert_eq!(
            json!({
                "method": "attestation/generate",
                "id": 9,
                "params": {}
            }),
            serde_json::to_value(&request)?,
        );

        let payload = ServerRequestPayload::AttestationGenerate(params);
        assert_eq!(request.id(), &RequestId::Integer(9));
        assert_eq!(payload.request_with_id(RequestId::Integer(9)), request);
        Ok(())
    }

    #[test]
    fn serialize_server_response() -> Result<()> {
        let response = ServerResponse::CommandExecutionRequestApproval {
            request_id: RequestId::Integer(8),
            response: v2::CommandExecutionRequestApprovalResponse {
                decision: v2::CommandExecutionApprovalDecision::AcceptForSession,
            },
        };

        assert_eq!(response.id(), &RequestId::Integer(8));
        assert_eq!(response.method(), "item/commandExecution/requestApproval");
        assert_eq!(
            json!({
                "method": "item/commandExecution/requestApproval",
                "id": 8,
                "response": {
                    "decision": "acceptForSession"
                }
            }),
            serde_json::to_value(&response)?,
        );
        Ok(())
    }

    #[test]
    fn serialize_mcp_server_elicitation_request() -> Result<()> {
        let requested_schema: v2::McpElicitationSchema = serde_json::from_value(json!({
            "type": "object",
            "properties": {
                "confirmed": {
                    "type": "boolean"
                }
            },
            "required": ["confirmed"]
        }))?;
        let params = v2::McpServerElicitationRequestParams {
            thread_id: "thr_123".to_string(),
            turn_id: Some("turn_123".to_string()),
            server_name: "codex_apps".to_string(),
            request: v2::McpServerElicitationRequest::Form {
                meta: None,
                message: "Allow this request?".to_string(),
                requested_schema,
            },
        };
        let request = ServerRequest::McpServerElicitationRequest {
            request_id: RequestId::Integer(9),
            params: params.clone(),
        };

        assert_eq!(
            json!({
                "method": "mcpServer/elicitation/request",
                "id": 9,
                "params": {
                    "threadId": "thr_123",
                    "turnId": "turn_123",
                    "serverName": "codex_apps",
                    "mode": "form",
                    "_meta": null,
                    "message": "Allow this request?",
                    "requestedSchema": {
                        "type": "object",
                        "properties": {
                            "confirmed": {
                                "type": "boolean"
                            }
                        },
                        "required": ["confirmed"]
                    }
                }
            }),
            serde_json::to_value(&request)?,
        );

        let payload = ServerRequestPayload::McpServerElicitationRequest(params);
        assert_eq!(request.id(), &RequestId::Integer(9));
        assert_eq!(payload.request_with_id(RequestId::Integer(9)), request);
        Ok(())
    }

    #[test]
    fn serialize_get_account_rate_limits() -> Result<()> {
        let request = ClientRequest::GetAccountRateLimits {
            request_id: RequestId::Integer(1),
            params: v2::GetAccountRateLimitsParams::default(),
        };
        assert_eq!(request.id(), &RequestId::Integer(1));
        assert_eq!(request.method(), "account/rateLimits/read");
        assert_eq!(
            json!({
                "method": "account/rateLimits/read",
                "id": 1,
                "params": {},
            }),
            serde_json::to_value(&request)?,
        );
        Ok(())
    }

    #[test]
    fn serialize_get_account_rate_limits_with_auth_profile() -> Result<()> {
        let request = ClientRequest::GetAccountRateLimits {
            request_id: RequestId::Integer(1),
            params: v2::GetAccountRateLimitsParams {
                auth_profile: Some(Some("work".to_string())),
                ..Default::default()
            },
        };
        assert_eq!(request.id(), &RequestId::Integer(1));
        assert_eq!(request.method(), "account/rateLimits/read");
        assert_eq!(
            json!({
                "method": "account/rateLimits/read",
                "id": 1,
                "params": {
                    "authProfile": "work",
                },
            }),
            serde_json::to_value(&request)?,
        );
        Ok(())
    }

    #[test]
    fn serialize_get_account_rate_limits_with_root_auth_profile() -> Result<()> {
        let request = ClientRequest::GetAccountRateLimits {
            request_id: RequestId::Integer(1),
            params: v2::GetAccountRateLimitsParams {
                auth_profile: Some(None),
                ..Default::default()
            },
        };
        assert_eq!(request.id(), &RequestId::Integer(1));
        assert_eq!(request.method(), "account/rateLimits/read");
        assert_eq!(
            json!({
                "method": "account/rateLimits/read",
                "id": 1,
                "params": {
                    "authProfile": null,
                },
            }),
            serde_json::to_value(&request)?,
        );
        Ok(())
    }

    #[test]
    fn deserialize_get_account_rate_limits_with_null_params() -> Result<()> {
        let request = serde_json::from_value::<ClientRequest>(json!({
            "method": "account/rateLimits/read",
            "id": 1,
            "params": null,
        }))?;

        assert_eq!(
            request,
            ClientRequest::GetAccountRateLimits {
                request_id: RequestId::Integer(1),
                params: v2::GetAccountRateLimitsParams::default(),
            }
        );
        Ok(())
    }

    #[test]
    fn serialize_consume_account_rate_limit_reset_credit() -> Result<()> {
        let request = ClientRequest::ConsumeAccountRateLimitResetCredit {
            request_id: RequestId::Integer(1),
            params: v2::ConsumeAccountRateLimitResetCreditParams {
                idempotency_key: "redeem-123".to_string(),
                credit_id: None,
                auth_profile: Some(Some("work".to_string())),
                expected_account_identity_fingerprint: "opaque:account".to_string(),
            },
        };
        assert_eq!(request.id(), &RequestId::Integer(1));
        assert_eq!(request.method(), "account/rateLimitResetCredit/consume");
        assert_eq!(
            json!({
                "method": "account/rateLimitResetCredit/consume",
                "id": 1,
                "params": {
                    "idempotencyKey": "redeem-123",
                    "authProfile": "work",
                    "expectedAccountIdentityFingerprint": "opaque:account",
                },
            }),
            serde_json::to_value(&request)?,
        );
        Ok(())
    }

    #[test]
    fn serialize_consume_account_rate_limit_reset_credit_outcome_as_camel_case() -> Result<()> {
        let response = v2::ConsumeAccountRateLimitResetCreditResponse {
            outcome: v2::ConsumeAccountRateLimitResetCreditOutcome::AlreadyRedeemed,
            account_identity_fingerprint: "sha256:test-account".to_string(),
        };

        assert_eq!(
            json!({
                "outcome": "alreadyRedeemed",
                "accountIdentityFingerprint": "sha256:test-account",
            }),
            serde_json::to_value(response)?,
        );
        Ok(())
    }

    #[test]
    fn serialize_get_account_token_usage() -> Result<()> {
        let request = ClientRequest::GetAccountTokenUsage {
            request_id: RequestId::Integer(1),
            params: None,
        };
        assert_eq!(request.id(), &RequestId::Integer(1));
        assert_eq!(request.method(), "account/usage/read");
        assert_eq!(
            json!({
                "method": "account/usage/read",
                "id": 1,
            }),
            serde_json::to_value(&request)?,
        );
        Ok(())
    }

    #[test]
    fn serialize_client_response() -> Result<()> {
        let cwd = absolute_path("/tmp");
        let response = ClientResponse::ThreadStart {
            request_id: RequestId::Integer(7),
            response: v2::ThreadStartResponse {
                thread: v2::Thread {
                    id: "67e55044-10b1-426f-9247-bb680e5fe0c8".to_string(),
                    session_id: "67e55044-10b1-426f-9247-bb680e5fe0c7".to_string(),
                    forked_from_id: None,
                    parent_thread_id: None,
                    preview: "first prompt".to_string(),
                    ephemeral: true,
                    model_provider: "openai".to_string(),
                    created_at: 1,
                    updated_at: 2,
                    status: v2::ThreadStatus::Idle,
                    path: None,
                    cwd: cwd.clone(),
                    cli_version: "0.0.0".to_string(),
                    source: v2::SessionSource::Exec,
                    thread_source: None,
                    agent_nickname: None,
                    agent_role: None,
                    git_info: None,
                    auth_profile: None,
                    auth_profile_kind: v2::AuthProfileKind::Unknown,
                    name: None,
                    turns: Vec::new(),
                },
                model: "gpt-5".to_string(),
                model_provider: "openai".to_string(),
                service_tier: None,
                cwd,
                runtime_workspace_roots: Vec::new(),
                profile_workspace_roots: Vec::new(),
                instruction_sources: vec![absolute_path("/tmp/AGENTS.md")],
                approval_policy: v2::AskForApproval::OnFailure,
                approvals_reviewer: v2::ApprovalsReviewer::User,
                sandbox: v2::SandboxPolicy::DangerFullAccess,
                active_permission_profile: None,
                auth_profile: None,
                reasoning_effort: None,
            },
        };

        assert_eq!(response.id(), &RequestId::Integer(7));
        assert_eq!(response.method(), "thread/start");
        assert_eq!(
            json!({
                "method": "thread/start",
                "id": 7,
                "response": {
                    "thread": {
                        "id": "67e55044-10b1-426f-9247-bb680e5fe0c8",
                        "sessionId": "67e55044-10b1-426f-9247-bb680e5fe0c7",
                        "forkedFromId": null,
                        "parentThreadId": null,
                        "preview": "first prompt",
                        "ephemeral": true,
                        "modelProvider": "openai",
                        "createdAt": 1,
                        "updatedAt": 2,
                        "status": {
                            "type": "idle"
                        },
                        "path": null,
                        "cwd": absolute_path_string("tmp"),
                        "cliVersion": "0.0.0",
                        "source": "exec",
                        "threadSource": null,
                        "agentNickname": null,
                        "agentRole": null,
                        "gitInfo": null,
                        "authProfile": null,
                        "authProfileKind": "unknown",
                        "name": null,
                        "turns": []
                    },
                    "model": "gpt-5",
                    "modelProvider": "openai",
                    "serviceTier": null,
                    "cwd": absolute_path_string("tmp"),
                    "runtimeWorkspaceRoots": [],
                    "profileWorkspaceRoots": [],
                    "instructionSources": [absolute_path_string("tmp/AGENTS.md")],
                    "approvalPolicy": "on-failure",
                    "approvalsReviewer": "user",
                    "sandbox": {
                        "type": "dangerFullAccess"
                    },
                    "activePermissionProfile": null,
                    "authProfile": null,
                    "reasoningEffort": null
                }
            }),
            serde_json::to_value(&response)?,
        );
        Ok(())
    }

    #[test]
    fn serialize_config_requirements_read() -> Result<()> {
        let request = ClientRequest::ConfigRequirementsRead {
            request_id: RequestId::Integer(1),
            params: None,
        };
        assert_eq!(
            json!({
                "method": "configRequirements/read",
                "id": 1,
            }),
            serde_json::to_value(&request)?,
        );
        Ok(())
    }

    #[test]
    fn serialize_account_login_api_key() -> Result<()> {
        let request = ClientRequest::LoginAccount {
            request_id: RequestId::Integer(2),
            params: v2::LoginAccountParams::ApiKey {
                api_key: "secret".to_string(),
            },
        };
        assert_eq!(
            json!({
                "method": "account/login/start",
                "id": 2,
                "params": {
                    "type": "apiKey",
                    "apiKey": "secret"
                }
            }),
            serde_json::to_value(&request)?,
        );
        Ok(())
    }

    #[test]
    fn serialize_account_login_chatgpt() -> Result<()> {
        let request = ClientRequest::LoginAccount {
            request_id: RequestId::Integer(3),
            params: v2::LoginAccountParams::Chatgpt {
                codex_streamlined_login: false,
            },
        };
        assert_eq!(
            json!({
                "method": "account/login/start",
                "id": 3,
                "params": {
                    "type": "chatgpt"
                }
            }),
            serde_json::to_value(&request)?,
        );
        Ok(())
    }

    #[test]
    fn serialize_account_login_chatgpt_streamlined() -> Result<()> {
        let request = ClientRequest::LoginAccount {
            request_id: RequestId::Integer(3),
            params: v2::LoginAccountParams::Chatgpt {
                codex_streamlined_login: true,
            },
        };
        assert_eq!(
            json!({
                "method": "account/login/start",
                "id": 3,
                "params": {
                    "type": "chatgpt",
                    "codexStreamlinedLogin": true
                }
            }),
            serde_json::to_value(&request)?,
        );
        Ok(())
    }

    #[test]
    fn serialize_account_login_chatgpt_device_code() -> Result<()> {
        let request = ClientRequest::LoginAccount {
            request_id: RequestId::Integer(4),
            params: v2::LoginAccountParams::ChatgptDeviceCode,
        };
        assert_eq!(
            json!({
                "method": "account/login/start",
                "id": 4,
                "params": {
                    "type": "chatgptDeviceCode"
                }
            }),
            serde_json::to_value(&request)?,
        );
        Ok(())
    }

    #[test]
    fn serialize_account_logout() -> Result<()> {
        let request = ClientRequest::LogoutAccount {
            request_id: RequestId::Integer(5),
            params: None,
        };
        assert_eq!(
            json!({
                "method": "account/logout",
                "id": 5,
            }),
            serde_json::to_value(&request)?,
        );
        Ok(())
    }

    #[test]
    fn serialize_account_login_chatgpt_auth_tokens() -> Result<()> {
        let request = ClientRequest::LoginAccount {
            request_id: RequestId::Integer(6),
            params: v2::LoginAccountParams::ChatgptAuthTokens {
                access_token: "access-token".to_string(),
                chatgpt_account_id: "org-123".to_string(),
                chatgpt_plan_type: Some("business".to_string()),
            },
        };
        assert_eq!(
            json!({
                "method": "account/login/start",
                "id": 6,
                "params": {
                    "type": "chatgptAuthTokens",
                    "accessToken": "access-token",
                    "chatgptAccountId": "org-123",
                    "chatgptPlanType": "business"
                }
            }),
            serde_json::to_value(&request)?,
        );
        Ok(())
    }

    #[test]
    fn serialize_auth_profile_list() -> Result<()> {
        let request = ClientRequest::AuthProfileList {
            request_id: RequestId::Integer(6),
            params: v2::AuthProfileListParams {
                cursor: None,
                limit: None,
            },
        };
        assert_eq!(request.id(), &RequestId::Integer(6));
        assert_eq!(request.method(), "authProfile/list");
        assert_eq!(
            json!({
                "method": "authProfile/list",
                "id": 6,
                "params": {
                    "cursor": null,
                    "limit": null,
                },
            }),
            serde_json::to_value(&request)?,
        );
        Ok(())
    }

    #[test]
    fn serialize_auth_profile_save_current() -> Result<()> {
        let request = ClientRequest::AuthProfileSaveCurrent {
            request_id: RequestId::Integer(7),
            params: v2::AuthProfileSaveCurrentParams {
                name: "work".to_string(),
            },
        };
        assert_eq!(request.id(), &RequestId::Integer(7));
        assert_eq!(request.method(), "authProfile/saveCurrent");
        assert_eq!(
            json!({
                "method": "authProfile/saveCurrent",
                "id": 7,
                "params": {
                    "name": "work",
                },
            }),
            serde_json::to_value(&request)?,
        );
        Ok(())
    }

    #[test]
    fn serialize_auth_profile_switch() -> Result<()> {
        let request = ClientRequest::AuthProfileSwitch {
            request_id: RequestId::Integer(8),
            params: v2::AuthProfileSwitchParams {
                name: "work".to_string(),
            },
        };
        assert_eq!(request.id(), &RequestId::Integer(8));
        assert_eq!(request.method(), "authProfile/switch");
        assert_eq!(
            json!({
                "method": "authProfile/switch",
                "id": 8,
                "params": {
                    "name": "work",
                },
            }),
            serde_json::to_value(&request)?,
        );
        Ok(())
    }

    #[test]
    fn serialize_get_account() -> Result<()> {
        let request = ClientRequest::GetAccount {
            request_id: RequestId::Integer(6),
            params: v2::GetAccountParams {
                refresh_token: false,
            },
        };
        assert_eq!(
            json!({
                "method": "account/read",
                "id": 6,
                "params": {}
            }),
            serde_json::to_value(&request)?,
        );
        let request = ClientRequest::GetAccount {
            request_id: RequestId::Integer(7),
            params: v2::GetAccountParams {
                refresh_token: true,
            },
        };
        assert_eq!(
            json!({
                "method": "account/read",
                "id": 7,
                "params": {
                    "refreshToken": true
                }
            }),
            serde_json::to_value(&request)?,
        );
        Ok(())
    }

    #[test]
    fn account_serializes_fields_in_camel_case() -> Result<()> {
        let api_key = v2::Account::ApiKey {};
        assert_eq!(
            json!({
                "type": "apiKey",
            }),
            serde_json::to_value(&api_key)?,
        );

        let chatgpt = v2::Account::Chatgpt {
            email: "user@example.com".to_string(),
            plan_type: PlanType::Plus,
        };
        assert_eq!(
            json!({
                "type": "chatgpt",
                "email": "user@example.com",
                "planType": "plus",
            }),
            serde_json::to_value(&chatgpt)?,
        );

        Ok(())
    }

    #[test]
    fn serialize_list_models() -> Result<()> {
        let request = ClientRequest::ModelList {
            request_id: RequestId::Integer(6),
            params: v2::ModelListParams::default(),
        };
        assert_eq!(
            json!({
                "method": "model/list",
                "id": 6,
                "params": {
                    "limit": null,
                    "cursor": null,
                    "includeHidden": null,
                    "modelProvider": null,
                    "modelGateway": null,
                    "upstreamProvider": null
                }
            }),
            serde_json::to_value(&request)?,
        );
        Ok(())
    }

    #[test]
    fn serialize_model_gateway_list() -> Result<()> {
        let request = ClientRequest::ModelGatewayList {
            request_id: RequestId::Integer(7),
            params: v2::ModelGatewayListParams::default(),
        };
        assert_eq!(
            json!({
                "method": "modelGateway/list",
                "id": 7,
                "params": {}
            }),
            serde_json::to_value(&request)?,
        );
        Ok(())
    }

    #[test]
    fn serialize_model_provider_list() -> Result<()> {
        let request = ClientRequest::ModelProviderList {
            request_id: RequestId::Integer(7),
            params: v2::ModelProviderListParams::default(),
        };
        assert_eq!(
            json!({
                "method": "modelProvider/list",
                "id": 7,
                "params": {
                    "modelGateway": null
                }
            }),
            serde_json::to_value(&request)?,
        );
        Ok(())
    }

    #[test]
    fn serialize_model_provider_capabilities_read() -> Result<()> {
        let request = ClientRequest::ModelProviderCapabilitiesRead {
            request_id: RequestId::Integer(7),
            params: v2::ModelProviderCapabilitiesReadParams {},
        };
        assert_eq!(
            json!({
                "method": "modelProvider/capabilities/read",
                "id": 7,
                "params": {}
            }),
            serde_json::to_value(&request)?,
        );
        Ok(())
    }

    #[test]
    fn serialize_list_collaboration_modes() -> Result<()> {
        let request = ClientRequest::CollaborationModeList {
            request_id: RequestId::Integer(7),
            params: v2::CollaborationModeListParams::default(),
        };
        assert_eq!(
            json!({
                "method": "collaborationMode/list",
                "id": 7,
                "params": {}
            }),
            serde_json::to_value(&request)?,
        );
        Ok(())
    }

    #[test]
    fn serialize_list_apps() -> Result<()> {
        let request = ClientRequest::AppsList {
            request_id: RequestId::Integer(8),
            params: v2::AppsListParams::default(),
        };
        assert_eq!(
            json!({
                "method": "app/list",
                "id": 8,
                "params": {
                    "cursor": null,
                    "limit": null,
                    "threadId": null
                }
            }),
            serde_json::to_value(&request)?,
        );
        Ok(())
    }

    #[test]
    fn serialize_environment_add() -> Result<()> {
        let request = ClientRequest::EnvironmentAdd {
            request_id: RequestId::Integer(9),
            params: v2::EnvironmentAddParams {
                environment_id: "remote-a".to_string(),
                exec_server_url: "ws://127.0.0.1:8765".to_string(),
            },
        };
        assert_eq!(
            json!({
                "method": "environment/add",
                "id": 9,
                "params": {
                    "environmentId": "remote-a",
                    "execServerUrl": "ws://127.0.0.1:8765"
                }
            }),
            serde_json::to_value(&request)?,
        );
        Ok(())
    }

    #[test]
    fn serialize_fs_get_metadata() -> Result<()> {
        let request = ClientRequest::FsGetMetadata {
            request_id: RequestId::Integer(10),
            params: v2::FsGetMetadataParams {
                path: absolute_path("tmp/example"),
            },
        };
        assert_eq!(
            json!({
                "method": "fs/getMetadata",
                "id": 10,
                "params": {
                    "path": absolute_path_string("tmp/example")
                }
            }),
            serde_json::to_value(&request)?,
        );
        Ok(())
    }

    #[test]
    fn serialize_fs_watch() -> Result<()> {
        let request = ClientRequest::FsWatch {
            request_id: RequestId::Integer(10),
            params: v2::FsWatchParams {
                watch_id: "watch-git".to_string(),
                path: absolute_path("tmp/repo/.git"),
            },
        };
        assert_eq!(
            json!({
                "method": "fs/watch",
                "id": 10,
                "params": {
                    "watchId": "watch-git",
                    "path": absolute_path_string("tmp/repo/.git")
                }
            }),
            serde_json::to_value(&request)?,
        );
        Ok(())
    }

    #[test]
    fn serialize_list_experimental_features() -> Result<()> {
        let request = ClientRequest::ExperimentalFeatureList {
            request_id: RequestId::Integer(8),
            params: v2::ExperimentalFeatureListParams::default(),
        };
        assert_eq!(
            json!({
                "method": "experimentalFeature/list",
                "id": 8,
                "params": {
                    "cursor": null,
                    "limit": null,
                    "threadId": null
                }
            }),
            serde_json::to_value(&request)?,
        );
        Ok(())
    }

    #[test]
    fn serialize_list_experimental_features_with_thread_id() -> Result<()> {
        let request = ClientRequest::ExperimentalFeatureList {
            request_id: RequestId::Integer(8),
            params: v2::ExperimentalFeatureListParams {
                cursor: Some("3".to_string()),
                limit: Some(2),
                thread_id: Some("00000000-0000-4000-8000-000000000001".to_string()),
            },
        };
        assert_eq!(
            json!({
                "method": "experimentalFeature/list",
                "id": 8,
                "params": {
                    "cursor": "3",
                    "limit": 2,
                    "threadId": "00000000-0000-4000-8000-000000000001"
                }
            }),
            serde_json::to_value(&request)?,
        );
        Ok(())
    }

    #[test]
    fn serialize_thread_background_terminals_clean() -> Result<()> {
        let request = ClientRequest::ThreadBackgroundTerminalsClean {
            request_id: RequestId::Integer(8),
            params: v2::ThreadBackgroundTerminalsCleanParams {
                thread_id: "thr_123".to_string(),
            },
        };
        assert_eq!(
            json!({
                "method": "thread/backgroundTerminals/clean",
                "id": 8,
                "params": {
                    "threadId": "thr_123"
                }
            }),
            serde_json::to_value(&request)?,
        );
        Ok(())
    }

    #[test]
    fn serialize_thread_realtime_start() -> Result<()> {
        let request = ClientRequest::ThreadRealtimeStart {
            request_id: RequestId::Integer(9),
            params: v2::ThreadRealtimeStartParams {
                thread_id: "thr_123".to_string(),
                output_modality: RealtimeOutputModality::Audio,
                prompt: Some(Some("You are on a call".to_string())),
                realtime_session_id: Some("sess_456".to_string()),
                transport: None,
                voice: Some(RealtimeVoice::Marin),
            },
        };
        assert_eq!(
            json!({
                "method": "thread/realtime/start",
                "id": 9,
                "params": {
                    "threadId": "thr_123",
                    "outputModality": "audio",
                    "prompt": "You are on a call",
                    "realtimeSessionId": "sess_456",
                    "transport": null,
                    "voice": "marin"
                }
            }),
            serde_json::to_value(&request)?,
        );
        Ok(())
    }

    #[test]
    fn serialize_thread_realtime_start_prompt_default_and_null() -> Result<()> {
        let default_prompt_request = ClientRequest::ThreadRealtimeStart {
            request_id: RequestId::Integer(9),
            params: v2::ThreadRealtimeStartParams {
                thread_id: "thr_123".to_string(),
                output_modality: RealtimeOutputModality::Audio,
                prompt: None,
                realtime_session_id: None,
                transport: None,
                voice: None,
            },
        };
        assert_eq!(
            json!({
                "method": "thread/realtime/start",
                "id": 9,
                "params": {
                    "threadId": "thr_123",
                    "outputModality": "audio",
                    "realtimeSessionId": null,
                    "transport": null,
                    "voice": null
                }
            }),
            serde_json::to_value(&default_prompt_request)?,
        );

        let null_prompt_request = ClientRequest::ThreadRealtimeStart {
            request_id: RequestId::Integer(9),
            params: v2::ThreadRealtimeStartParams {
                thread_id: "thr_123".to_string(),
                output_modality: RealtimeOutputModality::Audio,
                prompt: Some(None),
                realtime_session_id: None,
                transport: None,
                voice: None,
            },
        };
        assert_eq!(
            json!({
                "method": "thread/realtime/start",
                "id": 9,
                "params": {
                    "threadId": "thr_123",
                    "outputModality": "audio",
                    "prompt": null,
                    "realtimeSessionId": null,
                    "transport": null,
                    "voice": null
                }
            }),
            serde_json::to_value(&null_prompt_request)?,
        );

        let default_prompt_value = json!({
            "method": "thread/realtime/start",
            "id": 9,
            "params": {
                "threadId": "thr_123",
                "outputModality": "audio",
                "realtimeSessionId": null,
                "transport": null,
                "voice": null
            }
        });
        assert_eq!(
            serde_json::from_value::<ClientRequest>(default_prompt_value)?,
            default_prompt_request,
        );

        let null_prompt_value = json!({
            "method": "thread/realtime/start",
            "id": 9,
            "params": {
                "threadId": "thr_123",
                "outputModality": "audio",
                "prompt": null,
                "realtimeSessionId": null,
                "transport": null,
                "voice": null
            }
        });
        assert_eq!(
            serde_json::from_value::<ClientRequest>(null_prompt_value)?,
            null_prompt_request,
        );

        Ok(())
    }

    #[test]
    fn serialize_thread_status_changed_notification() -> Result<()> {
        let notification =
            ServerNotification::ThreadStatusChanged(v2::ThreadStatusChangedNotification {
                thread_id: "thr_123".to_string(),
                status: v2::ThreadStatus::Idle,
            });
        assert_eq!(
            json!({
                "method": "thread/status/changed",
                "params": {
                    "threadId": "thr_123",
                    "status": {
                        "type": "idle"
                    },
                }
            }),
            serde_json::to_value(&notification)?,
        );
        Ok(())
    }

    #[test]
    fn serialize_thread_realtime_output_audio_delta_notification() -> Result<()> {
        let notification = ServerNotification::ThreadRealtimeOutputAudioDelta(
            v2::ThreadRealtimeOutputAudioDeltaNotification {
                thread_id: "thr_123".to_string(),
                audio: v2::ThreadRealtimeAudioChunk {
                    data: "AQID".to_string(),
                    sample_rate: 24_000,
                    num_channels: 1,
                    samples_per_channel: Some(512),
                    item_id: None,
                },
            },
        );
        assert_eq!(
            json!({
                "method": "thread/realtime/outputAudio/delta",
                "params": {
                    "threadId": "thr_123",
                    "audio": {
                        "data": "AQID",
                        "sampleRate": 24000,
                        "numChannels": 1,
                        "samplesPerChannel": 512,
                        "itemId": null
                    }
                }
            }),
            serde_json::to_value(&notification)?,
        );
        Ok(())
    }

    #[test]
    fn mock_experimental_method_is_marked_experimental() {
        let request = ClientRequest::MockExperimentalMethod {
            request_id: RequestId::Integer(1),
            params: v2::MockExperimentalMethodParams::default(),
        };
        let reason = crate::experimental_api::ExperimentalApi::experimental_reason(&request);
        assert_eq!(reason, Some("mock/experimentalMethod"));
    }

    #[test]
    fn local_session_list_is_marked_experimental() {
        let request = ClientRequest::LocalSessionList {
            request_id: RequestId::Integer(1),
            params: v2::LocalSessionListParams::default(),
        };
        let reason = crate::experimental_api::ExperimentalApi::experimental_reason(&request);
        assert_eq!(reason, Some("localSession/list"));
    }

    #[test]
    fn mission_control_methods_are_not_marked_experimental() {
        for method in [
            "missionControl/overview",
            "missionControl/enqueueInstruction",
            "missionControl/mailboxReceipts",
            "missionControl/respondInteraction",
        ] {
            assert!(
                !EXPERIMENTAL_CLIENT_METHODS.contains(&method),
                "{method} should not be experimental"
            );
        }
    }

    #[test]
    fn remote_dispatch_methods_are_marked_experimental() {
        for method in [
            "remoteDispatch/negotiate",
            "remoteDispatch/submit",
            "remoteDispatch/receipt/read",
        ] {
            assert!(
                EXPERIMENTAL_CLIENT_METHODS.contains(&method),
                "{method} should be experimental"
            );
        }
    }

    #[test]
    fn agent_methods_are_marked_experimental() {
        for method in [
            "agent/start",
            "agent/list",
            "agent/read",
            "agent/attach",
            "agent/detach",
            "agent/stop",
            "agent/delete",
            "agent/events/list",
            "agent/pendingInteraction/respond",
            "agent/daemon/diagnostics",
        ] {
            assert!(
                EXPERIMENTAL_CLIENT_METHODS.contains(&method),
                "{method} should be experimental"
            );
        }
    }

    #[test]
    fn machine_registry_methods_are_marked_experimental() {
        for method in [
            "machineRegistry/list",
            "machineRegistry/read",
            "machineRegistry/upsert",
            "machineRegistry/disable",
            "machineRegistry/updateTrust",
            "machineRegistry/forget",
        ] {
            assert!(
                EXPERIMENTAL_CLIENT_METHODS.contains(&method),
                "{method} should be experimental"
            );
        }
    }

    #[test]
    fn environment_add_is_marked_experimental() {
        let request = ClientRequest::EnvironmentAdd {
            request_id: RequestId::Integer(1),
            params: v2::EnvironmentAddParams {
                environment_id: "remote-a".to_string(),
                exec_server_url: "ws://127.0.0.1:8765".to_string(),
            },
        };
        let reason = crate::experimental_api::ExperimentalApi::experimental_reason(&request);
        assert_eq!(reason, Some("environment/add"));
    }

    #[test]
    fn command_exec_permission_profile_is_marked_experimental() {
        let request = ClientRequest::OneOffCommandExec {
            request_id: RequestId::Integer(1),
            params: v2::CommandExecParams {
                command: vec!["pwd".to_string()],
                process_id: None,
                tty: false,
                stream_stdin: false,
                stream_stdout_stderr: false,
                output_bytes_cap: None,
                disable_output_cap: false,
                disable_timeout: false,
                timeout_ms: None,
                cwd: None,
                env: None,
                size: None,
                sandbox_policy: None,
                permission_profile: Some(BUILT_IN_PERMISSION_PROFILE_READ_ONLY.to_string()),
            },
        };

        let reason = crate::experimental_api::ExperimentalApi::experimental_reason(&request);
        assert_eq!(reason, Some("command/exec.permissionProfile"));
    }

    #[test]
    fn thread_realtime_start_is_marked_experimental() {
        let request = ClientRequest::ThreadRealtimeStart {
            request_id: RequestId::Integer(1),
            params: v2::ThreadRealtimeStartParams {
                thread_id: "thr_123".to_string(),
                output_modality: RealtimeOutputModality::Audio,
                prompt: Some(Some("You are on a call".to_string())),
                realtime_session_id: None,
                transport: None,
                voice: None,
            },
        };
        let reason = crate::experimental_api::ExperimentalApi::experimental_reason(&request);
        assert_eq!(reason, Some("thread/realtime/start"));
    }

    #[test]
    fn thread_goal_methods_are_not_marked_experimental() {
        let set_request = ClientRequest::ThreadGoalSet {
            request_id: RequestId::Integer(1),
            params: v2::ThreadGoalSetParams {
                thread_id: "thr_123".to_string(),
                objective: Some("ship goal mode".to_string()),
                title: None,
                status: Some(v2::ThreadGoalStatus::Active),
                token_budget: Some(Some(10_000)),
            },
        };
        let get_request = ClientRequest::ThreadGoalGet {
            request_id: RequestId::Integer(2),
            params: v2::ThreadGoalGetParams {
                thread_id: "thr_123".to_string(),
            },
        };
        let list_request = ClientRequest::ThreadGoalList {
            request_id: RequestId::Integer(3),
            params: v2::ThreadGoalListParams {
                thread_id: "thr_123".to_string(),
                cursor: None,
                limit: None,
            },
        };
        let activate_node_request = ClientRequest::ThreadGoalPlanActivateNode {
            request_id: RequestId::Integer(4),
            params: v2::ThreadGoalPlanActivateNodeParams {
                thread_id: "thr_123".to_string(),
                node_id: "node_123".to_string(),
            },
        };
        let add_goal_request = ClientRequest::ThreadGoalPlanAddGoal {
            request_id: RequestId::Integer(5),
            params: v2::ThreadGoalPlanAddGoalParams {
                thread_id: "thr_123".to_string(),
                objective: "queue goal mode".to_string(),
            },
        };
        let clear_request = ClientRequest::ThreadGoalClear {
            request_id: RequestId::Integer(6),
            params: v2::ThreadGoalClearParams {
                thread_id: "thr_123".to_string(),
            },
        };

        assert_eq!(
            crate::experimental_api::ExperimentalApi::experimental_reason(&set_request),
            None
        );
        assert_eq!(
            crate::experimental_api::ExperimentalApi::experimental_reason(&get_request),
            None
        );
        assert_eq!(
            crate::experimental_api::ExperimentalApi::experimental_reason(&list_request),
            None
        );
        assert_eq!(
            crate::experimental_api::ExperimentalApi::experimental_reason(&activate_node_request),
            None
        );
        assert_eq!(
            crate::experimental_api::ExperimentalApi::experimental_reason(&add_goal_request),
            None
        );
        assert_eq!(
            crate::experimental_api::ExperimentalApi::experimental_reason(&clear_request),
            None
        );
    }

    #[test]
    fn thread_workflow_methods_are_marked_experimental() {
        let create_request = ClientRequest::ThreadWorkflowCreate {
            request_id: RequestId::Integer(1),
            params: v2::ThreadWorkflowCreateParams {
                thread_id: "thr_123".to_string(),
                yaml: "schema_version: workflow.codex.codewith/v0".to_string(),
            },
        };
        let get_request = ClientRequest::ThreadWorkflowGet {
            request_id: RequestId::Integer(2),
            params: v2::ThreadWorkflowGetParams {
                thread_id: "thr_123".to_string(),
                workflow_record_id: "workflow_123".to_string(),
            },
        };
        let list_request = ClientRequest::ThreadWorkflowList {
            request_id: RequestId::Integer(3),
            params: v2::ThreadWorkflowListParams {
                thread_id: "thr_123".to_string(),
                cursor: None,
                limit: None,
            },
        };
        let run_list_request = ClientRequest::ThreadWorkflowRunList {
            request_id: RequestId::Integer(4),
            params: v2::ThreadWorkflowRunListParams {
                thread_id: "thr_123".to_string(),
                cursor: None,
                limit: None,
            },
        };
        let run_get_request = ClientRequest::ThreadWorkflowRunGet {
            request_id: RequestId::Integer(5),
            params: v2::ThreadWorkflowRunGetParams {
                thread_id: "thr_123".to_string(),
                run_id: "run_123".to_string(),
            },
        };
        let run_start_request = ClientRequest::ThreadWorkflowRunStart {
            request_id: RequestId::Integer(6),
            params: v2::ThreadWorkflowRunStartParams {
                thread_id: "thr_123".to_string(),
                workflow_record_id: "workflow_123".to_string(),
                idempotency_key: None,
            },
        };
        let run_pause_request = ClientRequest::ThreadWorkflowRunPause {
            request_id: RequestId::Integer(7),
            params: v2::ThreadWorkflowRunPauseParams {
                thread_id: "thr_123".to_string(),
                run_id: "run_123".to_string(),
                reason: None,
            },
        };
        let run_resume_request = ClientRequest::ThreadWorkflowRunResume {
            request_id: RequestId::Integer(8),
            params: v2::ThreadWorkflowRunResumeParams {
                thread_id: "thr_123".to_string(),
                run_id: "run_123".to_string(),
            },
        };
        let run_cancel_request = ClientRequest::ThreadWorkflowRunCancel {
            request_id: RequestId::Integer(9),
            params: v2::ThreadWorkflowRunCancelParams {
                thread_id: "thr_123".to_string(),
                run_id: "run_123".to_string(),
                reason: None,
            },
        };

        assert_eq!(
            crate::experimental_api::ExperimentalApi::experimental_reason(&create_request),
            Some("thread/workflow/create")
        );
        assert_eq!(
            crate::experimental_api::ExperimentalApi::experimental_reason(&get_request),
            Some("thread/workflow/get")
        );
        assert_eq!(
            crate::experimental_api::ExperimentalApi::experimental_reason(&list_request),
            Some("thread/workflow/list")
        );
        assert_eq!(
            crate::experimental_api::ExperimentalApi::experimental_reason(&run_list_request),
            Some("thread/workflow/run/list")
        );
        assert_eq!(
            crate::experimental_api::ExperimentalApi::experimental_reason(&run_get_request),
            Some("thread/workflow/run/get")
        );
        assert_eq!(
            crate::experimental_api::ExperimentalApi::experimental_reason(&run_start_request),
            Some("thread/workflow/run/start")
        );
        assert_eq!(
            crate::experimental_api::ExperimentalApi::experimental_reason(&run_pause_request),
            Some("thread/workflow/run/pause")
        );
        assert_eq!(
            crate::experimental_api::ExperimentalApi::experimental_reason(&run_resume_request),
            Some("thread/workflow/run/resume")
        );
        assert_eq!(
            crate::experimental_api::ExperimentalApi::experimental_reason(&run_cancel_request),
            Some("thread/workflow/run/cancel")
        );
    }

    #[test]
    fn worktree_methods_are_not_marked_experimental() {
        let list_request = ClientRequest::WorktreeList {
            request_id: RequestId::Integer(1),
            params: v2::WorktreeListParams {
                base_repo_path: None,
                include_deleted: None,
                cursor: None,
                limit: None,
            },
        };
        let read_request = ClientRequest::WorktreeRead {
            request_id: RequestId::Integer(2),
            params: v2::WorktreeReadParams {
                worktree_id: "wt_123".to_string(),
                base_repo_path: None,
            },
        };
        let attach_request = ClientRequest::WorktreeAttach {
            request_id: RequestId::Integer(3),
            params: v2::WorktreeAttachParams {
                worktree_id: "wt_123".to_string(),
                thread_id: Some("thr_123".to_string()),
                agent_run_id: None,
            },
        };
        let detach_request = ClientRequest::WorktreeDetach {
            request_id: RequestId::Integer(4),
            params: v2::WorktreeDetachParams {
                worktree_id: "wt_123".to_string(),
                thread_id: Some("thr_123".to_string()),
                agent_run_id: None,
            },
        };
        let create_request = ClientRequest::WorktreeCreate {
            request_id: RequestId::Integer(5),
            params: v2::WorktreeCreateParams {
                base_repo_path: None,
                name: Some("feature".to_string()),
                branch: None,
                start_point: None,
                cleanup_policy: None,
                thread_id: None,
            },
        };
        let reconcile_request = ClientRequest::WorktreeReconcile {
            request_id: RequestId::Integer(6),
            params: v2::WorktreeReconcileParams {
                base_repo_path: None,
            },
        };
        let release_request = ClientRequest::WorktreeRelease {
            request_id: RequestId::Integer(7),
            params: v2::WorktreeReleaseParams {
                worktree_id: "wt_123".to_string(),
                cleanup_policy: None,
                force_delete: None,
            },
        };
        let cleanup_request = ClientRequest::WorktreeCleanup {
            request_id: RequestId::Integer(8),
            params: v2::WorktreeCleanupParams {
                worktree_id: "wt_123".to_string(),
                force_delete: None,
            },
        };
        let merge_list_request = ClientRequest::WorktreeMergeCandidateList {
            request_id: RequestId::Integer(9),
            params: v2::WorktreeMergeCandidateListParams {
                worktree_id: "wt_123".to_string(),
                status: None,
                limit: None,
            },
        };
        let merge_refresh_request = ClientRequest::WorktreeMergeCandidateRefresh {
            request_id: RequestId::Integer(10),
            params: v2::WorktreeMergeCandidateRefreshParams {
                worktree_id: "wt_123".to_string(),
                target_ref: None,
            },
        };
        let merge_apply_request = ClientRequest::WorktreeMergeCandidateApply {
            request_id: RequestId::Integer(11),
            params: v2::WorktreeMergeCandidateApplyParams {
                candidate_id: "cand_123".to_string(),
            },
        };
        let merge_dismiss_request = ClientRequest::WorktreeMergeCandidateDismiss {
            request_id: RequestId::Integer(12),
            params: v2::WorktreeMergeCandidateDismissParams {
                candidate_id: "cand_123".to_string(),
            },
        };

        assert_eq!(
            crate::experimental_api::ExperimentalApi::experimental_reason(&list_request),
            None
        );
        assert_eq!(
            crate::experimental_api::ExperimentalApi::experimental_reason(&read_request),
            None
        );
        assert_eq!(
            crate::experimental_api::ExperimentalApi::experimental_reason(&attach_request),
            None
        );
        assert_eq!(
            crate::experimental_api::ExperimentalApi::experimental_reason(&detach_request),
            None
        );
        assert_eq!(
            crate::experimental_api::ExperimentalApi::experimental_reason(&create_request),
            None
        );
        assert_eq!(
            crate::experimental_api::ExperimentalApi::experimental_reason(&reconcile_request),
            None
        );
        assert_eq!(
            crate::experimental_api::ExperimentalApi::experimental_reason(&release_request),
            None
        );
        assert_eq!(
            crate::experimental_api::ExperimentalApi::experimental_reason(&cleanup_request),
            None
        );
        assert_eq!(
            crate::experimental_api::ExperimentalApi::experimental_reason(&merge_list_request),
            None
        );
        assert_eq!(
            crate::experimental_api::ExperimentalApi::experimental_reason(&merge_refresh_request),
            None
        );
        assert_eq!(
            crate::experimental_api::ExperimentalApi::experimental_reason(&merge_apply_request),
            None
        );
        assert_eq!(
            crate::experimental_api::ExperimentalApi::experimental_reason(&merge_dismiss_request),
            None
        );
    }

    fn test_thread_schedule() -> v2::ThreadSchedule {
        v2::ThreadSchedule {
            thread_id: "thr_123".to_string(),
            schedule_id: "sch_123".to_string(),
            parent_schedule_id: None,
            nesting_depth: 1,
            prompt: "check the deploy".to_string(),
            prompt_source: v2::ThreadSchedulePromptSource::Inline,
            schedule: v2::ThreadScheduleSpec::Interval {
                amount: 5,
                unit: v2::ThreadScheduleIntervalUnit::Minutes,
            },
            timezone: "UTC".to_string(),
            status: v2::ThreadScheduleStatus::Active,
            next_run_at: Some(1_700_000_300),
            last_run_at: None,
            expires_at: Some(1_700_604_800),
            failure_count: 0,
            lease_expires_at: None,
            created_at: 1_700_000_000,
            updated_at: 1_700_000_000,
        }
    }

    fn test_thread_schedule_run() -> v2::ThreadScheduleRun {
        v2::ThreadScheduleRun {
            thread_id: "thr_123".to_string(),
            schedule_id: "sch_123".to_string(),
            run_id: "run_123".to_string(),
            status: v2::ThreadScheduleRunStatus::Running,
            lease_id: "lease_123".to_string(),
            turn_id: Some("turn_123".to_string()),
            error: None,
            scheduled_for_at: Some(1_700_000_300),
            started_at: 1_700_000_301,
            completed_at: None,
        }
    }

    #[test]
    fn thread_schedule_methods_are_not_marked_experimental() {
        let requests = [
            ClientRequest::ThreadScheduleCreate {
                request_id: RequestId::Integer(1),
                params: v2::ThreadScheduleCreateParams {
                    thread_id: "thr_123".to_string(),
                    parent_schedule_id: None,
                    prompt: "check the deploy".to_string(),
                    prompt_source: Some(v2::ThreadSchedulePromptSource::Inline),
                    schedule: v2::ThreadScheduleSpec::Interval {
                        amount: 5,
                        unit: v2::ThreadScheduleIntervalUnit::Minutes,
                    },
                    timezone: Some("UTC".to_string()),
                    next_run_at: Some(1_700_000_300),
                    expires_at: None,
                },
            },
            ClientRequest::ThreadScheduleList {
                request_id: RequestId::Integer(2),
                params: v2::ThreadScheduleListParams {
                    thread_id: "thr_123".to_string(),
                    cursor: None,
                    limit: Some(50),
                },
            },
            ClientRequest::ThreadScheduleGet {
                request_id: RequestId::Integer(3),
                params: v2::ThreadScheduleGetParams {
                    thread_id: "thr_123".to_string(),
                    schedule_id: "sch_123".to_string(),
                },
            },
            ClientRequest::ThreadScheduleUpdate {
                request_id: RequestId::Integer(4),
                params: v2::ThreadScheduleUpdateParams {
                    thread_id: "thr_123".to_string(),
                    schedule_id: "sch_123".to_string(),
                    prompt: Some("check the rollout".to_string()),
                    schedule: None,
                    timezone: None,
                    status: Some(v2::ThreadScheduleStatus::Paused),
                    next_run_at: Some(None),
                    expires_at: None,
                },
            },
            ClientRequest::ThreadSchedulePause {
                request_id: RequestId::Integer(5),
                params: v2::ThreadSchedulePauseParams {
                    thread_id: "thr_123".to_string(),
                    schedule_id: "sch_123".to_string(),
                },
            },
            ClientRequest::ThreadScheduleResume {
                request_id: RequestId::Integer(6),
                params: v2::ThreadScheduleResumeParams {
                    thread_id: "thr_123".to_string(),
                    schedule_id: "sch_123".to_string(),
                },
            },
            ClientRequest::ThreadScheduleDelete {
                request_id: RequestId::Integer(7),
                params: v2::ThreadScheduleDeleteParams {
                    thread_id: "thr_123".to_string(),
                    schedule_id: "sch_123".to_string(),
                },
            },
            ClientRequest::ThreadScheduleRunNow {
                request_id: RequestId::Integer(8),
                params: v2::ThreadScheduleRunNowParams {
                    thread_id: "thr_123".to_string(),
                    schedule_id: "sch_123".to_string(),
                },
            },
        ];

        for request in requests {
            assert_eq!(
                crate::experimental_api::ExperimentalApi::experimental_reason(&request),
                None
            );
            assert_eq!(
                request.serialization_scope(),
                Some(ClientRequestSerializationScope::Thread {
                    thread_id: "thr_123".to_string()
                })
            );
        }
    }

    #[test]
    fn thread_schedule_notifications_are_not_marked_experimental() {
        let updated =
            ServerNotification::ThreadScheduleUpdated(v2::ThreadScheduleUpdatedNotification {
                thread_id: "thr_123".to_string(),
                schedule: test_thread_schedule(),
            });
        let deleted =
            ServerNotification::ThreadScheduleDeleted(v2::ThreadScheduleDeletedNotification {
                thread_id: "thr_123".to_string(),
                schedule_id: "sch_123".to_string(),
            });
        let run_updated = ServerNotification::ThreadScheduleRunUpdated(
            v2::ThreadScheduleRunUpdatedNotification {
                thread_id: "thr_123".to_string(),
                run: test_thread_schedule_run(),
            },
        );

        assert_eq!(
            crate::experimental_api::ExperimentalApi::experimental_reason(&updated),
            None
        );
        assert_eq!(
            crate::experimental_api::ExperimentalApi::experimental_reason(&deleted),
            None
        );
        assert_eq!(
            crate::experimental_api::ExperimentalApi::experimental_reason(&run_updated),
            None
        );
    }

    #[test]
    fn thread_goal_notifications_are_not_marked_experimental() {
        let goal = v2::ThreadGoal {
            thread_id: "thr_123".to_string(),
            goal_id: "goal_123".to_string(),
            objective: "ship goal mode".to_string(),
            title: None,
            status: v2::ThreadGoalStatus::Active,
            token_budget: Some(10_000),
            tokens_used: 123,
            time_used_seconds: 45,
            created_at: 1_700_000_000,
            updated_at: 1_700_000_123,
        };
        let updated = ServerNotification::ThreadGoalUpdated(v2::ThreadGoalUpdatedNotification {
            thread_id: "thr_123".to_string(),
            turn_id: None,
            goal,
        });
        let cleared = ServerNotification::ThreadGoalCleared(v2::ThreadGoalClearedNotification {
            thread_id: "thr_123".to_string(),
        });

        assert_eq!(
            crate::experimental_api::ExperimentalApi::experimental_reason(&updated),
            None
        );
        assert_eq!(
            crate::experimental_api::ExperimentalApi::experimental_reason(&cleared),
            None
        );
    }

    #[test]
    fn thread_settings_updated_notification_is_marked_experimental() {
        let notification =
            ServerNotification::ThreadSettingsUpdated(v2::ThreadSettingsUpdatedNotification {
                thread_id: "thr_123".to_string(),
                thread_settings: v2::ThreadSettings {
                    cwd: absolute_path("/tmp/repo"),
                    approval_policy: v2::AskForApproval::Never,
                    approvals_reviewer: v2::ApprovalsReviewer::User,
                    sandbox_policy: v2::SandboxPolicy::DangerFullAccess,
                    active_permission_profile: None,
                    auth_profile: None,
                    model: "gpt-5.4".to_string(),
                    model_provider: "openai".to_string(),
                    service_tier: None,
                    effort: None,
                    summary: None,
                    collaboration_mode: codex_protocol::config_types::CollaborationMode {
                        mode: codex_protocol::config_types::ModeKind::Default,
                        settings: codex_protocol::config_types::Settings {
                            model: "gpt-5.4".to_string(),
                            reasoning_effort: None,
                            developer_instructions: None,
                        },
                    },
                    personality: None,
                    session_prompt: None,
                    worktree_mode: codex_protocol::protocol::SessionWorktreeMode::Manual,
                },
            });

        assert_eq!(
            crate::experimental_api::ExperimentalApi::experimental_reason(&notification),
            Some("thread/settings/updated")
        );
    }

    #[test]
    fn turn_moderation_metadata_notification_is_marked_experimental() {
        let notification =
            ServerNotification::TurnModerationMetadata(v2::TurnModerationMetadataNotification {
                thread_id: "thr_123".to_string(),
                turn_id: "turn_123".to_string(),
                metadata: json!({"presentation": "inline"}),
            });

        assert_eq!(
            crate::experimental_api::ExperimentalApi::experimental_reason(&notification),
            Some("turn/moderationMetadata")
        );
    }

    #[test]
    fn thread_realtime_started_notification_is_marked_experimental() {
        let notification =
            ServerNotification::ThreadRealtimeStarted(v2::ThreadRealtimeStartedNotification {
                thread_id: "thr_123".to_string(),
                realtime_session_id: Some("sess_456".to_string()),
                version: RealtimeConversationVersion::V1,
            });
        let reason = crate::experimental_api::ExperimentalApi::experimental_reason(&notification);
        assert_eq!(reason, Some("thread/realtime/started"));
    }

    #[test]
    fn thread_realtime_output_audio_delta_notification_is_marked_experimental() {
        let notification = ServerNotification::ThreadRealtimeOutputAudioDelta(
            v2::ThreadRealtimeOutputAudioDeltaNotification {
                thread_id: "thr_123".to_string(),
                audio: v2::ThreadRealtimeAudioChunk {
                    data: "AQID".to_string(),
                    sample_rate: 24_000,
                    num_channels: 1,
                    samples_per_channel: Some(512),
                    item_id: None,
                },
            },
        );
        let reason = crate::experimental_api::ExperimentalApi::experimental_reason(&notification);
        assert_eq!(reason, Some("thread/realtime/outputAudio/delta"));
    }

    #[test]
    fn command_execution_request_approval_additional_permissions_is_marked_experimental() {
        let params = v2::CommandExecutionRequestApprovalParams {
            thread_id: "thr_123".to_string(),
            turn_id: "turn_123".to_string(),
            item_id: "call_123".to_string(),
            started_at_ms: 0,
            approval_id: None,
            reason: None,
            network_approval_context: None,
            command: Some("cat file".to_string()),
            cwd: None,
            command_actions: None,
            additional_permissions: Some(v2::AdditionalPermissionProfile {
                network: None,
                file_system: Some(v2::AdditionalFileSystemPermissions {
                    read: Some(vec![absolute_path("/tmp/allowed")]),
                    write: None,
                    glob_scan_max_depth: None,
                    entries: None,
                }),
            }),
            proposed_execpolicy_amendment: None,
            proposed_network_policy_amendments: None,
            available_decisions: None,
        };
        let reason = crate::experimental_api::ExperimentalApi::experimental_reason(&params);
        assert_eq!(
            reason,
            Some("item/commandExecution/requestApproval.additionalPermissions")
        );
    }
}

#[cfg(test)]
#[path = "common_tests.rs"]
mod common_tests;
