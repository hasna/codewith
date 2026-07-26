# Python SDK Examples

These examples exercise the checked-in Python SDK source preview.
`hasna-codewith-sdk` is not published on PyPI, and these examples are not the
supported integration path. For supported integrations today, use the
[TypeScript SDK](../../typescript/README.md).

Each example folder contains runnable versions:

- `sync.py` (public sync surface: `Codewith`)
- `async.py` (public async surface: `AsyncCodewith`)

All examples intentionally use only public SDK exports from `codewith`
and `codewith.types`.

Examples use plain strings for text-only turns and typed input objects for
multimodal or structured input lists.

## Prerequisites

- Python `>=3.10`
- `uv` installed and available on `PATH`
- A checkout of this repository
- An explicit path to a local Codewith executable for Codewith experiments

## Run From A Checkout

From `sdk/python`, create the source preview's local environment:

```bash
uv sync --extra dev
```

Use `uv run` for every example command so the project environment is selected
without platform-specific activation.

The examples bootstrap local SDK imports from `sdk/python/src`. Their checked-in
bootstrap resolves the upstream `openai-codex-cli-bin` dependency, so running
the examples unchanged exercises upstream Codex, not Codewith.
The pinned runtime version comes from the SDK package dependency.

To adapt an example for a Codewith experiment, replace its `runtime_config()`
value with an explicit local executable:

```python
from codewith import CodexConfig

config = CodexConfig(codex_bin="/absolute/path/to/codewith")
```

Pass `config=config` to `Codewith` or `AsyncCodewith`.

## Run Source-Preview Examples

From `sdk/python`:

```bash
uv run python examples/<example-folder>/sync.py
uv run python examples/<example-folder>/async.py
```

The checked-in examples use the local SDK source tree and upstream runtime
dependency automatically.

## Recommended Source-Preview First Run

```bash
uv run python examples/01_quickstart_constructor/sync.py
uv run python examples/01_quickstart_constructor/async.py
```

## Index

- `01_quickstart_constructor/`
  - first run / sanity check
- `02_turn_run/`
  - inspect full turn output fields
- `03_turn_stream_events/`
  - stream a turn with a small curated event view
- `04_models_and_metadata/`
  - discover visible models for the connected runtime
- `05_existing_thread/`
  - resume a real existing thread (created in-script)
- `06_thread_lifecycle_and_controls/`
  - thread lifecycle + control calls
- `07_image_and_text/`
  - remote image URL + text multimodal turn
- `08_local_image_and_text/`
  - local image + text multimodal turn using a generated temporary sample image
- `09_async_parity/`
  - parity-style sync flow (see async parity in other examples)
- `10_error_handling_and_retry/`
  - overload retry pattern + typed error handling structure
- `11_cli_mini_app/`
  - interactive chat loop
- `12_turn_params_kitchen_sink/`
  - structured output with a curated advanced `turn(...)` configuration
- `13_model_select_and_turn_params/`
  - list models, pick highest model + highest supported reasoning effort, run turns, print message and usage
- `14_turn_controls/`
  - separate `steer()` and `interrupt()` demos with concise summaries
- `15_login_and_account/`
  - browser-login handle lifecycle, cancellation, and account inspection
