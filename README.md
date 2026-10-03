# Codex Lean

An unofficial fork of OpenAI's Codex CLI for long-running agent work. Codex Lean combines compact instructions, a persistent JavaScript and TypeScript notebook, notes-based context rollover, shared agent coordination, and voice that can report progress without making you watch the terminal.

The defaults are deliberate, not mandatory. Use the CLI settings to choose ordinary compaction instead of Notes, choose sandbox-compatible tool execution instead of Notebook, or disable optional agent and voice features. Instruction changes are part of the fork, not a second set of feature switches.

## Install and start

Use the packages on the [Codex Lean releases page](https://github.com/IgorWarzocha/codex-lean/releases), or build from source below. OpenAI's installers, `@openai/codex`, and the Homebrew `codex` package install upstream Codex, **not Codex Lean**.

Release archives contain the CLI and its tool-execution helpers. Extract the whole archive into its own directory and run `bin/codex`, or `bin/codex.exe` on Windows. Do not copy only the executable out of the package. Verify the archive against the release's SHA-256 checksums before running it.

Release packages include the matching native voice helper and audio runtime. The release workflow targets Linux x64 and ARM64, Apple Silicon macOS, and Windows x64, and publishes only after every platform's package checks pass. macOS and Windows packages are not developer-signed. See [installation](docs/install.md) for platform and package details.

### Build from source

With the repository's Rust toolchain and build prerequisites installed:

```sh
git clone --branch lean https://github.com/IgorWarzocha/codex-lean.git
cd codex-lean/codex-rs
CARGO_PROFILE_DEV_DEBUG=0 cargo build -p codex-cli --bin codex
./target/debug/codex settings
```

### First launch

`codex settings` lists the available settings and choices without starting an agent thread. Use `codex settings set <setting> <choice>` to save a choice. Inside a running TUI, open `/settings` for the menus. See [configuration](docs/config.md) for the controls and when changes take effect. For a downloaded package, use `./bin/codex` instead of `./target/debug/codex` in the examples below.

Before your first thread:

- **Notes is the default context strategy.** It requires a supported OpenAI Codex backend and ChatGPT sign-in. For API keys or other providers, select **Compaction** first.
- **Notebook is the default tool runtime.** It requires a trusted, local, full-access session. It does not run inside a restricted sandbox. Select `v8` for sandbox-compatible Code Mode, or `off` for ordinary tool calls.

For ordinary compaction and sandbox-compatible Code Mode:

```sh
./target/debug/codex settings set context compaction
./target/debug/codex settings set code-mode v8
```

Then run `./target/debug/codex` and sign in. For Notebook, deliberately select full-access permissions for a trusted working directory. Selecting Notebook in settings does not grant those permissions.

See [building from source](docs/install.md) for setup and [Notebook](docs/notebook.md) for its launch commands and platform limits. A CLI-only source build is not a complete native voice package: voice also requires the matching packaged helper and audio runtime.

## What's different

<details>
<summary><strong>Persistent Notebook instead of disposable tool scripts</strong></summary>

Code Mode runs `exec` and `wait` through a persistent Deno kernel. Bindings survive cells and context rollover. Resuming a thread restores its serializable state; new threads in the same project can reuse explicitly pinned helpers and durable project bindings.

- Checkpoints restore values and function definitions without replaying old cells.
- Named profiles, startup hooks, diagnostics, and bounded journals help manage retained state.
- Cancellation and kernel recovery do not rerun failed work or undo external side effects.
- Live connections, imports, and closures are not magically restored. Unsupported values are reported.

Notebook uses Deno from `PATH`, or downloads and verifies a pinned runtime into Codex's private cache. No global Deno installation is required. It rejects restricted permissions, managed network proxies, and remote or multiple execution environments.

Choose `notebook`, `v8`, or `off` in settings. Disabling Code Mode does not delete saved notebooks. Explicit model-catalog tool choices can still override feature-selected behavior.

[Notebook operation and boundaries](docs/notebook.md)

</details>

<details>
<summary><strong>Notes-based context rollover, with ordinary compaction available</strong></summary>

Notes mode saves useful task state in remote notes before a context reset. The agent can retrieve earlier conversation through history instead of carrying the entire conversation into every new window. Notebook state and remote notes are separate: changing context strategy does not turn remote notes into local files.

Notes requires Codex backend authentication. Unsupported sessions report an error rather than silently switching strategies. Choose Compaction in settings to use ordinary summarization. The fork's compaction user-message retention budget defaults to 64,000 tokens, with 16,000 and 32,000 also supported.

Optional idle rollover starts a fresh notes window before a later user turn when a saved checkpoint is available. It is off by default and is not a background timer. Strategy changes do not migrate a running thread's history or checkpoints.

[Context controls and requirements](docs/config.md#context-continuity)

</details>

<details>
<summary><strong>Compact instructions and native nested AGENTS.md discovery</strong></summary>

New threads using the default model catalog get a [compact baseline](codex-rs/protocol/src/prompts/base_instructions/default.md). Tool descriptions and agent guidance are shorter too. Explicit instruction overrides and custom model catalogs remain authoritative; resumed threads retain their saved baseline.

Native file and directory discovery loads applicable nested `AGENTS.md` instructions as the agent reaches a subtree. This respects the existing trust and instruction-budget boundaries and works through direct tools and Notebook dispatch. It does not require an external post-tool hook.

These instruction changes have no separate settings toggle.

</details>

<details>
<summary><strong>Agent coordination and useful tool defaults</strong></summary>

The fork enables the V2 multi-agent workflow, dynamic-tool inheritance, and the shared agent message board by default. The board gives supported sessions a shared place for decisions, dependencies, and findings. It complements direct messages rather than replacing them. Posting to the board does not wake an idle agent.

The V2 `wait_agent` tool is disabled by default. Automatic child-result delivery remains available. Structured questions are available outside Plan mode. Notebook prewarming is enabled, but still respects permissions and does not download a runtime just to prewarm it. Patches preserve existing line endings by default.

Use settings for the optional fork controls and `/experimental` for available experiments. Enabling a feature does not bypass runtime, provider, or managed-policy requirements. General subagent availability, the V2 workflow, tool inheritance, and the board are distinct controls.

</details>

<details>
<summary><strong>Voice continuity and screenless progress</strong></summary>

Native CLI voice starts with bounded public context from the current thread and a brief greeting. It does not scan other threads or the workspace for its startup context.

Screenless mode forwards public progress and final results for typed and spoken work. Raw reasoning stays private. Completed public reasoning summaries require an explicit visibility setting. Permissions and structured questions still use their normal controls.

An established media call can be replaced once after transport loss. Context rollover refreshes the call serially after pending work settles, preserving mute state and finalized conversation text. A preparation failure leaves the old call usable. Reconnection and replacement do not repeat the greeting.

Voice settings expose the optional policies. Turn off screenless mode for the older final-only spoken-delegation behavior, or disable automatic media replacement and context refresh. These changes are native CLI behavior; rebuilding the CLI alone does not change the desktop app's voice implementation.

[Voice configuration](docs/config.md#voice-continuity)

</details>

<details>
<summary><strong>One communication-style file for text and voice</strong></summary>

Put your preferred tone and communication style in `~/.codex/codex_personality.md`, or the equivalent path under your `CODEX_HOME`. Codex appends it to native text and voice instructions rather than replacing them. No copied system prompt, version header, or rebuild is needed when you edit the file.

An absent default file or an empty file adds nothing. Text picks up edits when configuration loads; voice rereads the file for each new or replacement call. Existing text instructions and ongoing voice calls are not hot-updated. Invalid or unreadable configured files produce an error.

The repository also includes a separate, opt-in desktop personality patch and an optional pacman update hook. Neither is installed by the CLI. Desktop compatibility, installation, update behavior, and removal are documented separately.

[Native personality configuration](docs/config.md#communication-style) · [Desktop personality patch](scripts/desktop-voice-prompt/README.md)

</details>

## Documentation

- [Settings and configuration](docs/config.md)
- [Notebook runtime](docs/notebook.md)
- [Building from source](docs/install.md)
- [Contributing](docs/contributing.md)
- [Upstream Codex documentation](https://developers.openai.com/codex)

Based on [OpenAI Codex](https://github.com/openai/codex), under the [Apache-2.0 License](LICENSE). This is not an official OpenAI release.
