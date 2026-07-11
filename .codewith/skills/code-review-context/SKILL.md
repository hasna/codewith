---
name: code-review-context
description: Review Codewith changes that affect model-visible context, prompt assembly, context fragments, history handling, or cache-sensitive injected items. Use when a PR touches context construction, compaction, skill/plugin instructions, environment context, or other data sent to model inference requests.
---

Codewith maintains a context (history of messages) that is sent to the model in inference requests.

1. No history rewrite - the context must be built up incrementally.
2. Avoid frequent changes to context that cause cache misses.
3. No unbounded items - everything injected in the model context must have a bounded size and a hard cap.
4. No items larger than 10K tokens.
5. Highlight new individual items that can cross >1k tokens as P0. These need an additional manual review.
6. All injected fragments must be bounded structs in `codex-rs/core/src/context` or `codex-rs/context-fragments` and implement the `ContextualUserFragment` trait.
