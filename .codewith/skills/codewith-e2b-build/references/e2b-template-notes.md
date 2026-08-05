# E2B template notes for Codewith Rust builds

Facts discovered while building this skill. Update if templates change.

## Templates evaluated

| Template | vCPU/RAM | Toolchain visible to command runner | Codewith checkout | Warm target | Verdict |
|---|---|---|---|---|---|
| `codewith-pr-drain` | 8 / 8 GB | yes — `/opt/rust` (cargo, rustup, just, cargo-nextest) | `/opt/codewith` (+ warm cargo registry cache) | `/opt/codewith-target` (~1.3 GB, release profile) | **DEFAULT — chosen** |
| `open-pr-rust-8g` | 8 / 8 GB | no — nothing under `/opt` for the `user` account; toolchain/checkout not reachable by the command runner | not found by runner | none found | not usable as-is |
| `codewith-rust-validation-4g` | 4 / 4 GB | not probed in depth; only 4 vCPU (slower) | — | — | fallback / lower quota |

`codewith-pr-drain` won because it is the only template that exposes a working
toolchain **and** a warm codewith checkout to the E2B command runner under `/opt`.

## The exit-127 root cause and fix

`sbx.commands.run('rustc --version')` returns **exit 127** because the E2B
command runner runs as the unprivileged user `user` with:

- `PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin` (no `/opt/rust/cargo/bin`)
- `HOME=/home/user` (no rustup config; `RUSTUP_HOME`/`CARGO_HOME` unset)

It does **not** inherit the Docker image `ENV`, and even a login shell
(`bash -lc`) doesn't help — rustup's default toolchain lives in
`/opt/rust/rustup/settings.toml`, invisible without `RUSTUP_HOME`.

Fix: run commands as **root** with the toolchain env passed explicitly:

```
user: 'root'
CARGO_HOME=/opt/rust/cargo
RUSTUP_HOME=/opt/rust/rustup
CARGO_TARGET_DIR=/opt/codewith-target
RUST_MIN_STACK=8388608
PATH=/opt/rust/cargo/bin:/root/.bun/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin
```

With this env: `rustc 1.97.0`, `cargo 1.97.0`, `just 1.56.0`,
`cargo-nextest 0.9.140`, default toolchain `stable-x86_64-unknown-linux-gnu`.

## Filesystem / permissions

- `/opt/codewith` is **root-owned**; the `user` account cannot write it, and git
  reports "dubious ownership". Running as root + `git config --global --add
  safe.directory /opt/codewith` lets the box fetch/checkout in place.
- `/opt/codewith-target` is writable and is the persistent `CARGO_TARGET_DIR`.
- `/tmp` is user-writable — use it for uploaded patches (`--worktree`), then
  `git apply` as root.

## Build timing observed (small touched crate)

- Crate `codex-image-generation-extension` via `just test-fast-target
  /opt/codewith-target -p codex-image-generation-extension`.
- **Cold** dev-profile compile (warm cache was release-only, so debug rebuilt
  the dep graph): ~9m14s compile, then 11 tests all PASS, ~9.4 min wall total.
- The cargo registry cache under `CARGO_HOME` is warm, so **no crate downloads**.
- Reusing the same box (`--sandbox <id>`) keeps the debug target warm → later
  rebuilds recompile only changed crates and are far faster.

## E2B SDK primitives used

- `Sandbox.create(template, { apiKey, timeoutMs })` — spin a box.
- `Sandbox.connect(id, { apiKey })` — reuse a running/paused box.
- `Sandbox.list({ apiKey })` — discover running/paused boxes.
- `sbx.commands.run(cmd, { user, envs, cwd, timeoutMs, onStdout, onStderr })`.
- `sbx.setTimeout(ms)` — keepalive / extend lifetime.
- `sbx.betaPause()` — snapshot+pause (warm resume via connect).
- `sbx.files.write(path, data)` — upload the `--worktree` patch (runs as `user`).
- `sbx.kill()` — destroy the box.

## Concurrency limits

E2B concurrent-sandbox caps: **Hobby ≈ 20, Pro ≈ 100** (add-on up to ~1,100).
Keep the number of parallel workers under the account cap. The Hasna `sandboxes`
CLI wrapper is broken (Hasna-cloud 400, bug 8da65aca) — use the E2B SDK directly.
