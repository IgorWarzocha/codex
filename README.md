# Codex Lean

An unofficial fork of OpenAI Codex for long-running agent work. Persistent tools, notes-based context resets, shorter instructions, and voice that keeps you informed without watching the terminal.

## What changes from upstream

- **Tools keep their state.** A persistent JavaScript and TypeScript Notebook retains variables and reusable helpers across tool calls and context resets. Saved state restores without replaying old commands.
- **Context resets use notes.** The agent checkpoints useful state, starts a fresh context, and retrieves older conversation when needed. Notes is the default. Ordinary compaction remains available.
- **Less context spent on instructions.** Shorter system and tool prompts, skills read on demand, and nested `AGENTS.md` instructions loaded when the agent reaches the relevant files.
- **Coordinated subagents.** V2 agents inherit tools and share a message board for decisions, dependencies, and findings. Direct messages handle targeted coordination.
- **Voice follows the work.** Spoken progress and results, context refresh after rollover, and recovery from a dropped call. These changes belong to the CLI, not the ChatGPT desktop app.
- **Your preferences, not a copied system prompt.** One `codex_personality.md` file sets the tone for text and voice. `/settings` exposes the fork's context, tool-runtime, and agent controls.

[Notebook](docs/notebook.md) · [Context management](docs/config.md#context-continuity) · [Voice](docs/config.md#voice-continuity) · [Settings](docs/config.md)

## Download

**Release 0.160.0-lean.1.** Download the complete package for your machine:

| Platform | Download |
| --- | --- |
| Linux, Intel or AMD 64-bit | [Linux x64](https://github.com/IgorWarzocha/codex-lean/releases/download/lean-v0.160.0-lean.1/codex-lean-0.160.0-lean.1-x86_64-unknown-linux-musl.tar.gz) |
| Linux, ARM64 | [Linux ARM64](https://github.com/IgorWarzocha/codex-lean/releases/download/lean-v0.160.0-lean.1/codex-lean-0.160.0-lean.1-aarch64-unknown-linux-musl.tar.gz) |
| macOS, Apple Silicon | [macOS ARM64](https://github.com/IgorWarzocha/codex-lean/releases/download/lean-v0.160.0-lean.1/codex-lean-0.160.0-lean.1-aarch64-apple-darwin.tar.gz) |
| Windows, Intel or AMD 64-bit | [Windows x64](https://github.com/IgorWarzocha/codex-lean/releases/download/lean-v0.160.0-lean.1/codex-lean-0.160.0-lean.1-x86_64-pc-windows-msvc.zip) |

[Latest release and checksums](https://github.com/IgorWarzocha/codex-lean/releases/latest)

Extract the archive into its own folder and keep the whole package together. It includes the CLI, tool helpers, and native voice runtime. **No Rust build is needed.** Notebook downloads a verified Deno runtime on first use if needed.

Linux voice requires glibc 2.28 or newer. macOS and Windows packages are not developer-signed. See [platform requirements and download verification](docs/install.md).

OpenAI's installers, npm package, and Homebrew package install upstream Codex, not this fork.

## Start

The default setup uses **ChatGPT sign-in for Notes** and **full access for Notebook**. Only use Notebook in a trusted local project: its code can access your filesystem, network, and subprocesses. Prefer a sandbox or use an API key? Follow the [alternative setup](docs/install.md#sandbox-and-api-key-setup) instead.

Open a terminal in the extracted package folder, sign in, then launch in your project. Replace the example project path with your own.

**Linux and macOS**

```sh
./bin/codex login
./bin/codex --sandbox danger-full-access --cd /path/to/project
```

**Windows PowerShell**

```powershell
.\bin\codex.exe login
.\bin\codex.exe --sandbox danger-full-access --cd C:\path\to\project
```

Use `/settings` inside Codex to change defaults. `codex settings` also works before starting a thread. To run `codex` from anywhere, add the package's `bin` directory to your `PATH`; don't move the executable out of its package.

## Updates and source builds

Download a new package from [this fork's releases](https://github.com/IgorWarzocha/codex-lean/releases/latest) to update. Codex Lean does not run upstream's replacement installer.

The `lean` source branch includes automatic parent resumption when a child finishes. Release `0.160.0-lean.1` predates that fix. Stopping the parent prevents automatic resumption; the optional `wait_agent` tool remains disabled by default.

[Build from source](docs/install.md#build-from-source) · [Run alongside upstream Codex](docs/install.md#run-alongside-upstream-codex) · [Report a fork issue](https://github.com/IgorWarzocha/codex-lean/issues)

Based on [OpenAI Codex](https://github.com/openai/codex), under the [Apache-2.0 License](LICENSE). This is not an official OpenAI release.
