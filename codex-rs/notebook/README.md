# Native Deno notebook sessions

`codex-notebook` implements Codex's `CodeModeSessionProvider` with one Deno Jupyter kernel per session. JavaScript top-level bindings and JSON values stored with `store` persist between cells. Separate sessions have separate kernels. See [Notebook runtime](../../docs/notebook.md) for CLI configuration, durable state, profiles and recovery.

Deno Jupyter runs with unrestricted filesystem, network, and subprocess access. This provider is not a sandbox. The integrating caller must require explicit `danger-full-access` before exposing it. The kernel must run on the same local machine as the localhost tool bridge.

Construct `DenoNotebookSessionProvider::new(deno_program, cwd)` for an explicit executable, or `from_config(None, cwd, codex_home, thread_id)` for PATH discovery and managed installation. Availability checks never download or execute Deno. Session creation resolves the executable only after the caller's access validation. Use the existing Code Mode `exec`, `wait`, and delegate contracts.

`with_default_profile` selects a saved profile for fresh threads. Restored bindings take precedence, and resumed private checkpoints bypass the profile. Ephemeral threads may read a profile without writing Notebook state. See the runtime guide for configuration and managed Deno verification.

## Cell behavior

Only one cell can run at a time. Another `exec` is rejected until the running cell completes or is terminated. Foreground timeouts, preemption, and `await yield_control()` yield an observation without stopping execution. `wait` drains only output not previously observed, including output buffered after a yield and before completion.

Ordinary JavaScript exceptions preserve the kernel. Termination, a kernel failure, or the 24-hour execution deadline invalidates live bindings. Use `notebook restart` to recover checkpointed state or `notebook reset` to discard private state and restore project bindings. Neither operation replays cells. Shutdown cancels callbacks and shuts down the bridge and kernel with bounded waits.

Pending output is capped at 4 MiB, with a visible truncation notice. The integrating caller applies each `exec` or `wait` token budget. This provider does not carry the initial `exec` budget into later observations.

## Globals

- `tools.*` invokes the actual cell's `CodeModeSessionDelegate` over an authenticated localhost bridge. Tool functions expose `description` and supplied `input_schema` as `usage`. `ALL_TOOLS` lists normalized names, descriptions and schemas.
- `text(value)` appends text. `image(value, detail?)` accepts a `data:image` URL, an image item, or an MCP image block. `generatedImage({ image_url, output_hint? })` forwards an image and its optional hint.
- `store(key, value)` saves a JSON-serializable value. `load(key)` returns a copy, or `undefined` for a missing key.
- `await notify(value)` calls the real delegate notification hook. `await yield_control()` requests an immediate foreground yield while execution continues.
- `exit()` ends the current cell without discarding the kernel.
- Native Deno APIs and timers are available. Console output and Jupyter image displays are forwarded. Bare expression results are discarded.

Await all asynchronous work within its cell. Native Deno timers and IO are not cell-scoped and may outlive evaluation. Captured tool methods retain their original cell identity and cannot call a later cell's delegate. In-flight delegate calls are cancelled when their cell closes.

The separate `notebook` control tool manages checkpoints, pins, profiles, hooks, diagnostics and recovery. Non-default protocol resource limits and `audio()` are explicitly unsupported. Jupyter display updates append new items instead of replacing earlier observations. `clear_output` and unsupported display MIME types produce explicit notices.

## Validation

From `codex-rs`, run `cargo test -p codex-notebook` for in-process contract tests. To include real Deno persistence, isolation, delegate, output, and cancellation tests, run:

```sh
cargo test -p codex-notebook -- --include-ignored
```

Set `DENO_PROGRAM` if Deno is not on `PATH`. These tests do not call a model.
