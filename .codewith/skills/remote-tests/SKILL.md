---
name: remote-tests
description: Set up and run Codewith integration tests that require the CODEX_TEST_REMOTE_ENV remote executor container. Use when validating remote-environment behavior, unified exec routing, remote apply_patch behavior, or tests that skip unless a remote executor is configured.
---

Some Codewith integration tests support a running against a remote executor.
This means that when CODEX_TEST_REMOTE_ENV environment variable is set they will attempt to start an executor process in a docker container CODEX_TEST_REMOTE_ENV points to and use it in tests.

The Docker container is built and initialized by sourcing the root helper:

```bash
source scripts/test-remote-env.sh
cd codex-rs
just test -p codex-core --test all <remote-test-filter>
codex_remote_env_cleanup
```

The helper requires Docker or Colima and a Linux-compatible container runtime. If you must use a separate Linux/devbox host, reuse an existing checkout when possible and confirm the SHA plus modified files are in sync between local and remote before trusting results.
