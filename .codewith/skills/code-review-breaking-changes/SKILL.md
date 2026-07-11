---
name: code-review-breaking-changes
description: Review a Codewith diff for breaking changes in external integration surfaces. Use when checking PRs or branches that may affect app-server APIs, CLI arguments, config loading, persisted rollout/session compatibility, or other contracts consumed by users, scripts, or other agents.
---

Search for breaking changes in external integration surfaces:
- app-server APIs
- CLI parameters
- configuration loading
- resuming sessions from existing rollouts

Do not stop after finding one issue; analyze all possible ways breaking changes can happen.
