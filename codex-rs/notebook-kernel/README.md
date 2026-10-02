# Deno notebook kernel

`codex-notebook-kernel` runs a persistent Deno JS/TS isolate through Jupyter's
authenticated ZeroMQ protocol. It needs a Deno executable, not Python,
JupyterLab, a kernelspec installation, or npm packages.

This milestone supports Linux and macOS. Other platforms return
`KernelError::UnsupportedPlatform` before spawning. Windows support requires
job-object ownership for descendant cleanup.

**Deno's Jupyter kernel runs with allow-all permissions. This crate is not a
sandbox.** Callers own tool authorization and any injected HTTP tool bridge.
The connection file lives in a private temporary directory with a random HMAC
key. On Unix the directory is mode 0700 and the file is mode 0600.

Use `Kernel::start(KernelOptions::default()).await`, then
`kernel.execute(source).await` for each cell. Set `KernelOptions.deno` and `cwd`
to choose the executable and working directory. Startup proves both shell
responsiveness and IOPub subscription readiness before returning.

`execute_streaming(source, cancellation, on_output)` delivers ordered stdout,
stderr, MIME display, execution result, clear, and error events while the cell
runs. The callback must not block. The returned `ExecutionResult.outputs`
contains those same events, so do not render them twice. Display updates and
clear-output semantics belong to the consumer.

Output retention and callback delivery share a byte budget. Oversized events
are omitted and `output_truncated` is set. Exception details have an additional
bounded allowance of at most 16 KiB. This limits retained output, not the size
of a single incoming ZeroMQ frame.

Cells complete only after a correlated shell reply and IOPub idle event.
JavaScript exceptions return `ExecutionStatus::Error` without discarding
bindings. Transport failures, execution timeouts, and cancellation terminate
the kernel's Unix process group. Dropping an active execution future also kills the group and
invalidates the kernel. Call `shutdown().await` to await reaping and credential
cleanup. Dropping `Kernel` provides emergency kill-on-drop cleanup through
Tokio's process reaper. Shutdown is idempotent and uses a protocol request,
then bounded process waiting and force-kill if needed.
Ordinary descendants such as awaited `Deno.Command` children are terminated
even if the Deno leader exits first. This is lifecycle cleanup, not containment
of code that deliberately detaches into another process group.

Run deterministic tests with `cargo test -p codex-notebook-kernel`. Run the
focused real-Deno lifecycle test with:

```sh
cargo test -p codex-notebook-kernel -- --include-ignored
```

Set `DENO_KERNEL_TEST_BIN` when Deno is not on `PATH`. The test covers persistent
TypeScript state, isolated kernels, errors, MIME output, streaming,
cancellation, output limits, descendant cleanup, and shutdown. Its Unix
descendant probes use `/bin/sh` and `ps`. It makes no model calls.
