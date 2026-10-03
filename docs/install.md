## Installing & building

Download Codex Lean from the [fork's releases page](https://github.com/IgorWarzocha/codex-lean/releases)
or build it from source. OpenAI's installers and packages install upstream Codex,
not this fork. See the [README](../README.md#first-launch) for first-launch settings
and the Notes and Notebook requirements.

### Release packages

Choose the archive matching both your operating system and CPU:

| Platform | Target |
| --- | --- |
| Linux, Intel or AMD 64-bit | `x86_64-unknown-linux-musl` |
| Linux, ARM64 | `aarch64-unknown-linux-musl` |
| macOS, Apple Silicon | `aarch64-apple-darwin` |
| Windows, Intel or AMD 64-bit | `x86_64-pc-windows-msvc` |

The initial packages are prereleases. Each platform is built and smoke-tested on
its native CI runner. That does not establish compatibility with every older OS
release. macOS and Windows packages are not developer-signed or notarized.

Linux packages require glibc 2.28 or newer for native voice, even though the CLI
itself uses musl. They are not fully static packages for musl-only distributions.

1. Download the archive and the release's SHA-256 checksums. Compare the archive's
   SHA-256 digest with its entry in that file.
2. Extract the archive into a new directory. Keep `codex-package.json`, `bin`,
   `codex-resources`, and `codex-path` together. The CLI needs the packaged helpers.
3. Run `./bin/codex --version`, then `./bin/codex settings`. On Windows, use
   `.\bin\codex.exe` instead.

The packages include the CLI, Code Mode host, ripgrep, platform-specific execution
helpers, and the native voice helper with its audio libraries. The CLI and voice
helper are built from the same source revision. Notebook can download its verified
Deno runtime on first use. Voice still requires an available microphone, audio
output, and account access to the realtime service.

To update, download and extract the next Codex Lean release. `codex update` points
to the fork's releases rather than running an installer that would replace the
fork with upstream Codex. Do not extract an update over a running package.

If you also run upstream Codex, use a separate home directory to avoid sharing
version-dependent caches and defaults:

```sh
CODEX_HOME="$HOME/.codex-lean" ./bin/codex
```

Use that same environment setting on subsequent launches. A separate home has
separate settings, history, and sign-in state.

Release maintainers: see [the release workflow](releases.md).

### Build from source

```bash
# Clone the repository and navigate to the root of the Cargo workspace.
git clone --branch lean https://github.com/IgorWarzocha/codex-lean.git
cd codex-lean/codex-rs

# Install the Rust toolchain, if necessary.
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
source "$HOME/.cargo/env"
rustup component add rustfmt
rustup component add clippy
# Install helper tools used by the workspace justfile:
cargo install --locked just
# DotSlash fetches pinned development tools such as buildifier on first use.
cargo install --locked dotslash
# Install nextest for the `just test` helper.
cargo install --locked cargo-nextest

# Build the fork's CLI.
CARGO_PROFILE_DEV_DEBUG=0 cargo build -p codex-cli --bin codex

# Inspect defaults without starting a thread.
./target/debug/codex settings
# For API-key sessions and sandbox-compatible Code Mode:
./target/debug/codex settings set context compaction
./target/debug/codex settings set code-mode v8

# Launch the TUI with a sample prompt.
./target/debug/codex "explain this codebase to me"

# After making changes, use the root justfile helpers (they default to codex-rs):
just fmt
just fix -p <crate-you-touched>

# Run the relevant tests (project-specific is fastest), for example:
just test -p codex-tui
# `just test` runs the test suite via nextest:
just test
# Avoid `--all-features` for routine local runs because it increases build
# time and `target/` disk usage by compiling additional feature combinations.
```

## Tracing / verbose logging

Codex is written in Rust, so it honors the `RUST_LOG` environment variable to configure its logging behavior.

The TUI records diagnostics in bounded local stores by default. Set `log_dir` explicitly to enable a plaintext TUI log for a run:

```bash
codex -c log_dir=./.codex-log
tail -F ./.codex-log/codex-tui.log
```

The non-interactive mode (`codex exec`) defaults to `RUST_LOG=error`, but messages are printed inline, so there is no need to monitor a separate file.

See the Rust documentation on [`RUST_LOG`](https://docs.rs/env_logger/latest/env_logger/#enabling-logging) for more information on the configuration options.
