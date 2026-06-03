<p align="center"><strong>Codewith</strong> is a command-line coding agent from Hasna that runs locally on your computer.</p>

<p align="center">Codewith is a modified derivative of OpenAI Codex. Codewith is not affiliated with, sponsored by, or endorsed by OpenAI.</p>

---

## Quickstart

### Installing and running Codewith

Run the following on Mac, Linux, or Windows to install Codewith:

```shell
bun install -g @hasna/codewith
```

Codewith can also be installed through npm:

```shell
npm install -g @hasna/codewith
```

Then run `codewith` to get started. Codewith stores its local state in `~/.codewith` by default.

<details>
<summary>You can also download a platform binary from the latest Codewith GitHub Release.</summary>

Each GitHub Release contains many executables, but in practice, you likely want one of these:

- macOS
  - Apple Silicon/arm64: `codewith-aarch64-apple-darwin.tar.gz`
  - x86_64 (older Mac hardware): `codewith-x86_64-apple-darwin.tar.gz`
- Linux
  - x86_64: `codewith-x86_64-unknown-linux-musl.tar.gz`
  - arm64: `codewith-aarch64-unknown-linux-musl.tar.gz`

Each archive contains a single entry with the platform baked into the name (e.g., `codewith-x86_64-unknown-linux-musl`), so you likely want to rename it to `codewith` after extracting it.

</details>

### Using Codewith with your ChatGPT plan

Run `codewith` and select **Sign in with ChatGPT**. We recommend signing into your ChatGPT account to use Codewith as part of your Plus, Pro, Business, Edu, or Enterprise plan. [Learn more about what's included in your ChatGPT plan](https://help.openai.com/en/articles/11369540-codex-in-chatgpt).

You can also use Codewith with an API key.

## Docs

- [**Authentication**](./docs/authentication.md)
- [**Contributing**](./docs/contributing.md)
- [**Installing & building**](./docs/install.md)
- [**Open source fund**](./docs/open-source-fund.md)

This repository is licensed under the [Apache-2.0 License](LICENSE). See [NOTICE](NOTICE), [MODIFICATIONS.md](MODIFICATIONS.md), and [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md) for attribution, modification, and bundled third-party notices. The Apache-2.0 license covers this codebase; it does not grant rights to OpenAI trademarks, services, accounts, subscriptions, models, or APIs.
