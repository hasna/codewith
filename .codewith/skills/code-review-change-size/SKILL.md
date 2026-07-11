---
name: code-review-change-size
description: Evaluate whether a Codewith change is small enough to review safely. Use during PR review when a diff may exceed roughly 800 changed lines, combines mechanical and logic edits, or should be split into smaller staged commits before merge.
---

Unless the change is mechanical the total number of changed lines should not exceed 800 lines.
For complex logic changes the size should be under 500 lines.

If the change is larger, explain whether it can be split into reviewable stages and identify the smallest coherent stage to land first.
Base the staging suggestion on the actual diff, dependencies, and affected call sites.
