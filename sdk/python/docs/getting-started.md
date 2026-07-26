# Getting Started

This guide runs the Codewith Python SDK source preview from a repository
checkout with a multi-turn thread.

`hasna-codewith-sdk` is not published on PyPI, and this preview is not the
supported integration path. For supported integrations today, use the
[TypeScript SDK](../../typescript/README.md).

## 1. Set Up From Source

Requirements:

- Python `>=3.10`
- `uv` installed and available on `PATH`
- A checkout of this repository
- An explicit path to a local Codewith executable for Codewith experiments
- An existing Codewith account session, or one of the login flows below

From the repository root:

```bash
cd sdk/python
uv sync --extra dev
```

This creates a local environment for the checked-in SDK source and resolves the
upstream `openai-codex-cli-bin` version pinned in `pyproject.toml`. That
dependency starts upstream Codex by default; it is not a Codewith runtime, and
`uv sync` alone does not provide supported Codewith behavior.

Use `uv run` for Python commands so the project environment is selected without
platform-specific activation:

```bash
uv run python path/to/script.py
```

To experiment with Codewith, configure the explicit local executable once and
pass the config to every client shown below:

```python
from codewith import CodexConfig

codewith_config = CodexConfig(codex_bin="/absolute/path/to/codewith")
```

## 2. Authenticate When Needed

Existing Codewith authentication is reused automatically. For ChatGPT browser
login:

```python
from codewith import Codewith

with Codewith(config=codewith_config) as client:
    login = client.login_chatgpt()
    print(login.auth_url)
    print(login.wait().success)
```

For device-code login:

```python
with Codewith(config=codewith_config) as client:
    login = client.login_chatgpt_device_code()
    print(login.verification_url, login.user_code)
    print(login.wait().success)
```

For API-key login:

```python
with Codewith(config=codewith_config) as client:
    client.login_api_key("sk-...")
    print(client.account().account)
```

## 3. Run A Turn

```python
from codewith import Codewith, Sandbox

with Codewith(config=codewith_config) as client:
    thread = client.thread_start(sandbox=Sandbox.workspace_write)
    result = thread.run("Say hello in one sentence.")

    print("Thread:", thread.id)
    print("Text:", result.final_response)
    print("Items:", len(result.items))
```

`Thread.run(...)` starts a turn, waits for completion, and returns
`TurnResult`. Plain strings are shorthand for `TextInput(...)`.

Use `Thread.turn(...)` when you need a `TurnHandle` for streaming, steering,
or interrupting an active turn.

## 4. Choose Sandbox Access

Use one enum for the initial thread and later turn overrides:

```python
from codewith import Codewith, Sandbox

with Codewith(config=codewith_config) as client:
    thread = client.thread_start(sandbox=Sandbox.workspace_write)
    thread.run("Make the requested changes.")
    review = thread.run("Review the diff only.", sandbox=Sandbox.read_only)
```

Available presets:

- `Sandbox.read_only`: read files without allowing writes.
- `Sandbox.workspace_write`: read files and write inside the workspace and
  configured writable roots; this is the normal default for workspace work.
- `Sandbox.full_access`: run without filesystem access restrictions.

When `sandbox=` is omitted, Codewith uses its configured default. A turn override
also applies to subsequent turns on that thread.

## 5. Continue A Thread

```python
from codewith import Codewith

with Codewith(config=codewith_config) as client:
    thread = client.thread_start()
    thread.run("Summarize Rust ownership in two bullets.")
    result = thread.run("Now explain it to a Python developer.")
    print(result.final_response)
```

To resume a stored thread later:

```python
with Codewith(config=codewith_config) as client:
    thread = client.thread_resume("thr_123")
    print(thread.run("Continue where we left off.").final_response)
```

## 6. Use The Async Client

```python
import asyncio

from codewith import AsyncCodewith, Sandbox


async def main() -> None:
    async with AsyncCodewith(config=codewith_config) as client:
        thread = await client.thread_start(sandbox=Sandbox.workspace_write)
        result = await thread.run("Continue where we left off.")
        print(result.final_response)


asyncio.run(main())
```

## 7. Get Help

Python's built-in documentation tools cover the curated SDK surface:

```python
import codewith
from codewith import Codewith, CodexConfig

help(codewith)
help(Codewith)
help(CodexConfig)
```

```bash
uv run python -m pydoc codewith
```

## Next Stops

- [API reference](https://github.com/hasna/codewith/blob/main/sdk/python/docs/api-reference.md)
- [FAQ](https://github.com/hasna/codewith/blob/main/sdk/python/docs/faq.md)
- [Runnable examples](https://github.com/hasna/codewith/blob/main/sdk/python/examples/README.md)
