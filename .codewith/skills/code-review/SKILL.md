---
name: code-review
description: Run an adversarial final code review on a pull request by using the repo-local code-review-* helper skills. Use when asked to review a PR or branch for regressions, breaking changes, missing tests, context-window risks, or change-size concerns before merge.
---

Use subagents to review code using all code-review-* skills in this repository other than this orchestrator. One subagent per skill. Pass full skill path to subagents. Use xhigh reasoning.

You must return every single issue from every subagent. You can return an unlimited number of findings.
Use raw Markdown to report findings.
Number findings for ease of reference.
Each finding must include a specific file path and line number.

If the GitHub user running the review is the owner of the pull request add a `code-reviewed` label.
Do not leave GitHub comments unless explicitly asked.
