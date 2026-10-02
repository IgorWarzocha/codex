Run JavaScript or TypeScript in a persistent Deno notebook. Send raw source, not JSON or Markdown fences.

- Bindings and imports survive cells and context rollover within this running thread. Resume after process exit and fork start fresh kernels. No disk checkpoints, pins or profiles yet.
- Deno APIs, Web APIs and imports are available. Code runs with full machine access. Never treat this runtime as a sandbox.
- Await all async work. Deno timers and direct I/O are not cell-scoped; do not leave them running across cells. Only one cell may run at a time. Use `wait` after a yielded cell before starting another.
- Cancellation terminates the kernel and loses its in-memory bindings. Start a new thread after terminal kernel failure.
- Optional first line: `// @exec: {"yield_time_ms": 10000, "max_output_tokens": 1000}`. Default yield timeout: {{ default_exec_yield_time_ms }} ms.
- `tools` contains the enabled nested tools. Example: `text(await tools.exec_command({cmd: "pwd"}))`. Tool names use normalized JavaScript identifiers. `ALL_TOOLS` contains their names and descriptions.
- `text(value)` emits text. `image(value)` emits an image. `generatedImage(value)` emits an image-generation result. Bare expression values are not a substitute for explicit output.
- `store(key, value)` and `load(key)` retain serializable values within this kernel.
- `await notify(value)` sends an immediate notification. `await yield_control()` yields output while execution continues.
- `exit()` and `audio()` are not supported in this first Notebook runtime.
