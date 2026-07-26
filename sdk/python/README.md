# Codewith Python SDK (Beta) - Source Preview

Build Python applications that start Codewith threads, run turns, stream progress,
and control workspace access.

The Python SDK is currently a source preview. Its code is available in this
repository, but `hasna-codewith-sdk` is not published on PyPI and this preview
is not the supported integration path. For supported integrations today, use
the [TypeScript SDK](../typescript/README.md).

## Set Up From Source

Prerequisites:

- Python `>=3.10`
- `uv` installed and available on `PATH`
- A checkout of this repository

From the repository root:

```bash
cd sdk/python
uv sync --extra dev
```

`uv sync` resolves the upstream `openai-codex-cli-bin` version pinned in
`pyproject.toml`. That dependency starts upstream Codex by default; it is not a
Codewith runtime, and environment setup alone does not provide supported
Codewith behavior.

Use `uv run` to invoke Python in the project environment without activating a
platform-specific virtual environment.

## Quickstart

To experiment with the source preview against Codewith, pass an explicit local
Codewith executable through `CodexConfig.codex_bin`:

```python
from codewith import Codewith, CodexConfig

config = CodexConfig(codex_bin="/absolute/path/to/codewith")

with Codewith(config=config) as client:
    thread = client.thread_start()
    result = thread.run("Explain this repository in three bullets.")
    print(result.final_response)
```

Run your Python entry point with `uv run python path/to/script.py`.

`thread.run(...)` returns a `TurnResult` containing the final response,
collected items, and token usage.

## Authentication

With the explicit Codewith executable configured, existing Codewith
authentication is reused automatically. To start ChatGPT browser login:

```python
with Codewith(config=config) as client:
    login = client.login_chatgpt()
    print(login.auth_url)
    print(login.wait().success)
```

For device-code login:

```python
with Codewith(config=config) as client:
    login = client.login_chatgpt_device_code()
    print(login.verification_url, login.user_code)
    login.wait()
```

For API-key login:

```python
with Codewith(config=config) as client:
    client.login_api_key("sk-...")
```

## Built-In Help

Use Python's standard `help(codewith)`, `help(Codewith)`, or
`uv run python -m pydoc codewith` documentation tools.

## Documentation

- [Getting started](https://github.com/hasna/codewith/blob/main/sdk/python/docs/getting-started.md)
- [API reference](https://github.com/hasna/codewith/blob/main/sdk/python/docs/api-reference.md)
- [FAQ](https://github.com/hasna/codewith/blob/main/sdk/python/docs/faq.md)
- [Examples](https://github.com/hasna/codewith/blob/main/sdk/python/examples/README.md)

The source preview is licensed under the
[repository Apache License 2.0](https://github.com/hasna/codewith/blob/main/LICENSE).
