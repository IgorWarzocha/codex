# Native Deno notebook sessions

`codex-notebook` implements Codex's `CodeModeSessionProvider` with one Deno Jupyter kernel per session. JavaScript top-level bindings and JSON values stored with `store` persist between cells. Separate sessions have separate kernels. See [Notebook runtime](../../docs/notebook.md) for CLI configuration, durable state, profiles and recovery.

Deno Jupyter runs with unrestricted filesystem, network, and subprocess access. This provider is not a sandbox. The integrating caller must require explicit `danger-full-access` before exposing it. The kernel must run on the same local machine as the localhost tool bridge.

Construct `DenoNotebookSessionProvider::new(deno_program, cwd)` for an explicit executable, or `from_config(None, cwd, codex_home, thread_id)` for PATH discovery and managed installation. Availability checks never download or execute Deno. Session creation resolves the executable only after the caller's access validation. Use the existing Code Mode `exec`, `wait`, and delegate contracts.

`with_default_profile` selects a saved profile to attempt after restoring project and private bindings. Collisions leave the entire profile unapplied. Reset skips the configured profile. Ephemeral threads may read a profile without writing Notebook state. `with_max_heap_mib` configures the heap and persistence budget. The default is 4096 MiB, with a supported range of 256 through 65536 MiB. See the runtime guide for configuration and managed Deno verification.

## Cell behavior

Only one cell can run at a time. Another `exec` is rejected until the running cell completes or is terminated. Foreground timeouts, preemption, and `await yield_control()` yield an observation without stopping execution. `wait` drains only output not previously observed, including output buffered after a yield and before completion.

Ordinary JavaScript exceptions preserve the kernel without checkpointing the failed cell. Termination invalidates live bindings, and the next execution restores the last completed checkpoint. Kernel failures trigger a bounded recovery attempt. `notebook restart` and `notebook reset` remain available for manual recovery. No recovery path replays cells. Normal shutdown checkpoints idle state and attempts bounded disposal before stopping the bridge and kernel. `shutdown_without_cleanup` skips user code when access has been revoked.

Pending output is capped at 4 MiB, with a visible truncation notice. The integrating caller applies each `exec` or `wait` token budget. This provider does not carry the initial `exec` budget into later observations.

## Globals

- `tools.*` invokes the actual cell's `CodeModeSessionDelegate` over an authenticated localhost bridge. Native tool help and schemas live in `tools.<name>.description` and the descriptions in `ALL_TOOLS`.
- `text(value)` appends text. `image(value, detail?)` accepts a `data:image` URL, an image item, or an MCP image block. `generatedImage({ image_url, output_hint? })` forwards an image and its optional hint.
- `store(key, value)` saves a JSON-serializable value. `load(key)` returns a copy, or `undefined` for a missing key.
- `await notify(value)` calls the real delegate notification hook. `await yield_control()` requests an immediate foreground yield while execution continues.
- `exit()` ends the current cell without discarding the kernel.
- Native Deno APIs and timers are available. Console output and Jupyter image displays are forwarded. Bare expression results are discarded.

`text` projects native command results without routine chunk and timing metadata. `with_plain_command_output(true)` prints their output below the metadata instead of embedding it in JSON. Returned objects and unrelated values are unchanged.

Await all asynchronous work within its cell. Native Deno timers and IO are not cell-scoped and may outlive evaluation. Captured tool methods retain their original cell identity and cannot call a later cell's delegate. In-flight delegate calls are cancelled when their cell closes.

The separate `notebook` control tool manages checkpoints, pins, profiles, hooks, diagnostics and recovery. Non-default protocol resource limits and `audio()` are explicitly unsupported. Jupyter display updates append new items instead of replacing earlier observations. `clear_output` and unsupported display MIME types produce explicit notices.

## Validation

From `codex-rs`, run `cargo test -p codex-notebook` for in-process contract tests. To include real Deno persistence, isolation, delegate, output, and cancellation tests, run:

```sh
cargo test -p codex-notebook -- --include-ignored
```

Set `DENO_PROGRAM` if Deno is not on `PATH`. These tests do not call a model.
