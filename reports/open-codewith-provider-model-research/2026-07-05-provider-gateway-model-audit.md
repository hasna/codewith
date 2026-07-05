# Provider, Gateway, and Model Source Audit

Task: `0950554e-31c4-4d72-884d-20be1f4038f5`

Checked: 2026-07-05 from the task worktree. No provider credentials were used.

## Scope

This is the research artifact requested by the task planner. It records current provider,
gateway, and model-list evidence from official or provider-primary sources and calls out
repo drift that should be handled by follow-up implementation tasks. Product code was not
changed in this worker step.

Source authority levels used below:

- Direct provider docs or public provider API responses are treated as primary evidence for
  that provider.
- Aggregator catalogs are treated as primary evidence only for that aggregator gateway.
- Authenticated-only `/models` responses prove endpoint shape and auth requirement, not the
  provider's current full catalog.
- Marketplace catalogs are useful cross-checks, but they are not treated as direct-provider
  proof for another provider.

## Local Registry Surface

- The built-in provider picker surface is limited to OpenAI, Anthropic, Cerebras, NVIDIA,
  OpenRouter, xAI, Xiaomi MiMo, DeepSeek, Alibaba Qwen, Google Gemini, Z.ai, and MiniMax
  (`codex-rs/model-provider-info/src/lib.rs:8`).
- Built-in provider IDs and base URLs are compiled in `codex-rs/model-provider-info/src/lib.rs:41`.
  The same known-provider base URLs are mirrored for fallback metadata in
  `codex-rs/known-provider-models/src/lib.rs:17`.
- Gateway semantics are explicit: `hasna` is the direct-provider gateway and `openrouter` is
  the aggregator gateway (`codex-rs/model-provider-info/src/lib.rs:132`). `model_gateway_for_provider`
  maps only OpenRouter to the `openrouter` gateway; every other built-in provider maps to `hasna`
  (`codex-rs/model-provider-info/src/lib.rs:161`).
- App-server fallback models emit route fields for `model_gateway`, `model_gateway_name`,
  `model_gateway_kind`, and `upstream_provider` (`codex-rs/app-server/src/models.rs:87`).
  `upstream_provider` is currently derived from slash-prefixed model IDs
  (`codex-rs/app-server/src/models.rs:213`).
- Fallback metadata is provider-specific and feeds model info through
  `known_provider_models::metadata_for_local_fallback` (`codex-rs/models-manager/src/model_info.rs:84`).
  Unknown slugs fall back to generic metadata (`codex-rs/models-manager/src/model_info.rs:135`).

## Gateway Findings

Current local gateway behavior is coherent with the source model:

- Built-in direct providers: OpenAI, Anthropic, Cerebras, NVIDIA, xAI, Xiaomi, DeepSeek, Qwen,
  Google, Z.ai, and MiniMax are represented behind the local `hasna` direct gateway. Amazon
  Bedrock is a special configured provider with its own AWS/Mantle handling, but it still uses the
  direct-provider gateway semantics when enabled.
- Aggregator provider: OpenRouter is represented behind the `openrouter` aggregator gateway.
- OpenRouter slash IDs such as `z-ai/glm-5.2` and `x-ai/grok-4.20` expose an upstream provider
  prefix in the app-server model response, while direct provider IDs do not need that derived
  upstream field.

No code-level gateway change is implied by the current research.

## Provider And Model Evidence

| Provider | Official or provider-primary evidence checked | Local state and notes |
| --- | --- | --- |
| OpenAI | Official latest-model docs identify `gpt-5.5` as the latest model family and describe feature continuity with GPT-5.4: <https://developers.openai.com/api/docs/guides/latest-model>. | Local OpenAI base URL is `https://api.openai.com/v1`. The built-in OpenAI model presets are outside this research report, but the latest-model signal is current for PR notes. |
| Amazon Bedrock / Mantle | OpenAI's AWS Bedrock cookbook uses a regionized Mantle endpoint such as `https://bedrock-mantle.us-west-2.api.aws/openai/v1/responses` and examples with `openai.gpt-5.4`: <https://developers.openai.com/cookbook/examples/partners/aws/openai_models_with_amazon_bedrock>. | Local Bedrock default base URL is `https://bedrock-mantle.us-east-1.api.aws/openai/v1`; local model IDs include `openai.gpt-5.5` and `openai.gpt-5.4` (`codex-rs/model-provider-info/src/lib.rs:83`). Treat endpoint region as configurable; do not infer that us-west-2 is the only valid region from the cookbook example. |
| Anthropic | Anthropic's model overview lists current Claude models including Claude Fable 5, Opus 4.8, Sonnet 5, Sonnet 4.6, and Haiku 4.5 variants: <https://docs.anthropic.com/en/docs/about-claude/models/overview>. | Local Anthropic fallback entries match the documented model family names checked in the report. Anthropic's direct `/v1` base URL remains consistent with local `https://api.anthropic.com/v1`. |
| Cerebras | Official model docs list production `gpt-oss-120b` and preview `zai-glm-4.7` and `gemma-4-31b`: <https://inference-docs.cerebras.ai/models/overview.md>. Public catalog endpoint rechecked: <https://api.cerebras.ai/public/v1/models?format=openrouter>. | Local Cerebras fallback IDs match the public catalog. The public endpoint returned those three IDs with 131072-token context windows on 2026-07-05. |
| NVIDIA | Provider-primary public catalog endpoint rechecked: <https://integrate.api.nvidia.com/v1/models>. Relevant current IDs included `deepseek-ai/deepseek-v4-flash`, `deepseek-ai/deepseek-v4-pro`, `minimaxai/minimax-m2.7`, `minimaxai/minimax-m3`, `nvidia/nemotron-3-ultra-550b-a55b`, `openai/gpt-oss-120b`, and `z-ai/glm-5.2`. | Local NVIDIA fallback includes `z-ai/glm-5.1`, but the unauthenticated NVIDIA catalog did not list that ID and did list `z-ai/glm-5.2`. This is clear NVIDIA-provider drift and should be handled by a narrow follow-up implementation task. |
| OpenRouter | Provider-primary aggregator catalog endpoint rechecked: <https://openrouter.ai/api/v1/models>. It listed local fallback IDs including `z-ai/glm-5.2`, `z-ai/glm-5.1`, `x-ai/grok-4.20`, `xiaomi/mimo-v2.5-pro`, and `nvidia/nemotron-3-ultra-550b-a55b`. | Local OpenRouter fallback remains broadly supported by the aggregator catalog. Note that OpenRouter returned `nvidia/nemotron-3-ultra-550b-a55b` with a model context length of 1000000 and a top-provider context length of 262144, so implementation should be deliberate about which field it uses if refreshing metadata. |
| xAI | xAI docs list Code API and Chat API model families, including `grok-build-0.1` and Grok 4.3 / Grok 4.20 variants: <https://docs.x.ai/docs/models.md>. xAI llms index confirms OpenAI-compatible base URL examples: <https://docs.x.ai/llms.txt>. | Local xAI fallback `grok-4.3` and `grok-build-0.1` is consistent with direct docs. `https://api.x.ai/v1/models` returned unauthenticated without credentials, so this report does not claim a complete live xAI model catalog. |
| Xiaomi MiMo | Official MiMo docs index describes OpenAI/Anthropic-compatible API formats: <https://mimo.mi.com/llms.txt>. The official model summary lists `mimo-v2.5-pro` and `mimo-v2.5` with 1M context and 128K max output: <https://mimo.mi.com/static/docs/quick-start/summary/model.md>. OpenAI-compatible chat endpoint docs use `https://api.xiaomimimo.com/v1/chat/completions`: <https://mimo.mi.com/static/docs/api/chat/openai-api.md>. | Local Xiaomi base URL matches the direct API host. Local fallback includes `mimo-v2.5-pro-ultraspeed`; the checked API model table did not list that ID, though Xiaomi's public product/news surfaces reference an UltraSpeed variant. Keep this as uncertain unless an authenticated `/models` list or API docs table confirms it. |
| DeepSeek | Official pricing/API docs list `deepseek-v4-pro` and `deepseek-v4-flash` with 1M context: <https://api-docs.deepseek.com/quick_start/pricing>. | Local DeepSeek fallback includes `deepseek-v4-flash` and `deepseek-v4-pro`, matching the current direct docs checked. `https://api.deepseek.com/v1/models` required authentication, so no complete live model catalog was captured. |
| Alibaba Qwen | Alibaba Model Studio model docs list current Qwen 3.7 and 3.6 models: <https://www.alibabacloud.com/help/en/model-studio/models>. The OpenAI-compatible Chat Completions docs show current workspace-specific base URLs and regional domains, and note migration away from `dashscope-intl.aliyuncs.com`: <https://www.alibabacloud.com/help/en/model-studio/qwen-api-via-openai-chat-completions>. API reference overview: <https://www.alibabacloud.com/help/en/model-studio/qwen-api-reference/>. | Local Qwen fallback model names are largely aligned with documented Qwen families, but the local base URL `https://dashscope-intl.aliyuncs.com/api/v2/apps/protocols/compatible-mode/v1` appears stale or at least legacy relative to the current workspace-specific docs. This should be verified and refreshed in a follow-up task. |
| Google Gemini | Google OpenAI-compatibility docs show the OpenAI-compatible base URL under `https://generativelanguage.googleapis.com/v1beta/openai/`, including model list/retrieve examples: <https://ai.google.dev/gemini-api/docs/openai>. Google model docs list Gemini 3.5 Flash, Gemini 3.1 Pro Preview, Gemini 3 Flash Preview, and Gemini 3.1 Flash Lite: <https://ai.google.dev/gemini-api/docs/models>. | Local Google fallback entries match the checked model docs and local base URL matches the documented OpenAI-compatible host. Unauthenticated model-list attempts were not usable, so this report relies on official docs rather than a public `/models` dump. |
| Z.ai | Z.ai docs index lists GLM-5.2, GLM-5.1, GLM-5, GLM-5-Turbo, and GLM-4.7 docs: <https://docs.z.ai/llms.txt>. Pricing docs list those GLM model families: <https://docs.z.ai/guides/overview/pricing.md>. OpenAI SDK docs confirm base URL `https://api.z.ai/api/paas/v4/` with `glm-5.2` examples: <https://docs.z.ai/guides/develop/openai/python.md>. | Local Z.ai fallback is consistent with direct docs. `https://api.z.ai/api/paas/v4/models` required authentication, so the current full live catalog was not enumerated. |
| MiniMax | `https://api.minimax.io/v1/models` required authentication. Public MiniMax platform docs checked at <https://platform.minimaxi.com/document/Model%20Overview.md> did not provide a clean current public listing for the local M2/M3 fallback set. | Local MiniMax fallback includes `MiniMax-M3`, `MiniMax-M2.7`, `MiniMax-M2.5`, `MiniMax-M2.1`, and `MiniMax-M2` variants. NVIDIA's marketplace catalog confirms `minimaxai/minimax-m3` and `minimaxai/minimax-m2.7`, but that is not direct MiniMax proof. Treat the direct MiniMax catalog as auth-only pending credentialed verification. |

## Drift And Follow-Up Candidates

1. NVIDIA provider fallback should be refreshed. Local `codex-rs/known-provider-models/src/nvidia.rs`
   includes `z-ai/glm-5.1`, while the NVIDIA public catalog checked on 2026-07-05 listed
   `z-ai/glm-5.2` and not `z-ai/glm-5.1`.
2. Alibaba Qwen base URL should be reviewed. Local provider constants use
   `https://dashscope-intl.aliyuncs.com/api/v2/apps/protocols/compatible-mode/v1`, while current
   Alibaba docs show workspace-specific regional OpenAI-compatible domains and a migration note
   away from `dashscope-intl.aliyuncs.com`.
3. Xiaomi `mimo-v2.5-pro-ultraspeed` should not be treated as fully verified from the checked API
   model table. It appears in official public product/news surfaces, but the model summary table
   checked for API usage lists only `mimo-v2.5-pro` and `mimo-v2.5`.
4. MiniMax current fallback set remains auth-only from the direct provider. Avoid claiming direct
   MiniMax public-catalog verification until an authenticated `/models` check or clearer official
   model table is available.

## Validation Notes

- Public endpoint rechecks were performed for Cerebras, NVIDIA, and OpenRouter because those
  endpoints expose unauthenticated model catalogs.
- Auth-only endpoint checks were performed for xAI, DeepSeek, Qwen, Google Gemini, Z.ai, MiniMax,
  and Xiaomi where practical. They were used only to confirm auth requirements and endpoint shape.
- No API keys, tokens, or authenticated provider responses were used or recorded.
- No Rust source files were changed by this report task.
