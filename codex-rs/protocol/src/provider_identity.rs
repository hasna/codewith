//! Canonical provider identity for the built-in "known" providers.
//!
//! Provider IDs and base URLs for the shared known providers are defined here so
//! that the higher-level `codex-model-provider-info` registry and the
//! low-dependency `codex-known-provider-models` metadata crate consume a single
//! maintained boundary instead of repeating drift-prone literals. Adding or
//! changing a shared provider's ID/base URL should require an edit only in this
//! module.
//!
//! Only the providers shared between both registries live here. Provider entries
//! that are specific to app-server/runtime concerns (e.g. OpenAI/ChatGPT, the
//! Hasna gateway, Amazon Bedrock, Ollama) intentionally remain in
//! `codex-model-provider-info`.

pub const ANTHROPIC_PROVIDER_ID: &str = "anthropic";
pub const ANTHROPIC_BASE_URL: &str = "https://api.anthropic.com/v1";

pub const CEREBRAS_PROVIDER_ID: &str = "cerebras";
pub const CEREBRAS_BASE_URL: &str = "https://api.cerebras.ai/v1";

pub const DEEPSEEK_PROVIDER_ID: &str = "deepseek";
pub const DEEPSEEK_BASE_URL: &str = "https://api.deepseek.com/v1";

pub const GOOGLE_PROVIDER_ID: &str = "google";
pub const GOOGLE_BASE_URL: &str = "https://generativelanguage.googleapis.com/v1beta/openai";

pub const MINIMAX_PROVIDER_ID: &str = "minimax";
pub const MINIMAX_BASE_URL: &str = "https://api.minimax.io/v1";

pub const NVIDIA_PROVIDER_ID: &str = "nvidia";
pub const NVIDIA_BASE_URL: &str = "https://integrate.api.nvidia.com/v1";

pub const OPENROUTER_PROVIDER_ID: &str = "openrouter";
pub const OPENROUTER_BASE_URL: &str = "https://openrouter.ai/api/v1";

pub const QWEN_PROVIDER_ID: &str = "qwen";
pub const QWEN_BASE_URL: &str = "https://dashscope-intl.aliyuncs.com/compatible-mode/v1";

pub const XAI_PROVIDER_ID: &str = "xai";
pub const XAI_BASE_URL: &str = "https://api.x.ai/v1";

pub const XIAOMI_PROVIDER_ID: &str = "xiaomi";
pub const XIAOMI_BASE_URL: &str = "https://api.xiaomimimo.com/v1";

pub const ZAI_PROVIDER_ID: &str = "zai";
pub const ZAI_BASE_URL: &str = "https://api.z.ai/api/paas/v4";
