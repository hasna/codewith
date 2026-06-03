# Codewith Python SDK (Beta)

Build Python applications that start Codewith threads, run turns, stream progress,
and control workspace access.

## Install

Install the SDK:

```bash
pip install hasna-codewith-sdk
```

## Quickstart

The SDK reuses your existing Codewith authentication when one is already
available:

```python
from codewith import Codewith

with Codewith() as client:
    thread = client.thread_start()
    result = thread.run("Explain this repository in three bullets.")
    print(result.final_response)
```

`thread.run(...)` returns a `TurnResult` containing the final response,
collected items, and token usage.

## Authentication

Existing Codewith authentication is reused automatically. To start ChatGPT
browser login explicitly:

```python
from codewith import Codewith

with Codewith() as client:
    login = client.login_chatgpt()
    print(login.auth_url)
    print(login.wait().success)
```

For device-code login:

```python
with Codewith() as client:
    login = client.login_chatgpt_device_code()
    print(login.verification_url, login.user_code)
    login.wait()
```

For API-key login:

```python
with Codewith() as client:
    client.login_api_key("sk-...")
```

## Built-In Help

Use Python's standard `help(codewith)`, `help(Codewith)`, or
`python -m pydoc codewith` documentation tools.

## Documentation

- [Getting started](https://github.com/hasna/codewith/blob/main/sdk/python/docs/getting-started.md)
- [API reference](https://github.com/hasna/codewith/blob/main/sdk/python/docs/api-reference.md)
- [FAQ](https://github.com/hasna/codewith/blob/main/sdk/python/docs/faq.md)
- [Examples](https://github.com/hasna/codewith/blob/main/sdk/python/examples/README.md)

The package is licensed under the
[repository Apache License 2.0](https://github.com/hasna/codewith/blob/main/LICENSE).
