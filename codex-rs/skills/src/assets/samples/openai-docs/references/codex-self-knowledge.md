Establish local behavior from the installed version, relevant source, active configuration, and applicable local guidance. Separate `config.toml` defaults, `requirements.toml` constraints, and managed policy. If verified local behavior conflicts with upstream documentation, name the difference instead of substituting stock claims.

For broad upstream Codex orientation, use the bundled manual helper when shell execution and a writable temporary cache are allowed. A narrow public feature, setting, error, or citation needs the root skill's official search and page retrieval instead.

## Retrieve the manual

Resolve `<skill-dir>` to the installed package:

```sh
node <skill-dir>/scripts/fetch-codex-manual.mjs
```

The helper verifies the current source and returns manual and outline paths, freshness status, and headings. Search only relevant headings and line ranges. Its cache preference is the `TMPDIR`, `TEMP`, then `TMP` environment variables, each with an `openai-docs-cache` child. On non-Windows hosts it then tries `/private/tmp/openai-docs-cache` and `/tmp/openai-docs-cache`.

Override only when needed with `--cache-dir <cache-dir>`. It handles HTTP(S) proxies and falls back to `curl`. Do not assume a sandbox restriction without evidence.

Reuse fresh paths within the same thread. Refresh after about a day, uncertain provenance, missing likely-current information, or a request to verify freshness. Skip when shell or permitted cache writes are unavailable. On failure or a material manual gap, search and open the exact official topic. Cite its official source page or known anchor. A manual cannot establish fork-specific behavior.

## Choose the owning surface

- One-off task constraints: current prompt or thread.
- Repository conventions and checks: applicable `AGENTS.md`.
- Trusted project defaults: project `.codex/config.toml`. Personal defaults: global configuration or guidance.
- Reusable workflow: skill. Installable bundle: plugin.
- Authorized external data and actions: MCP server or app connector. Private workspace data requires its authenticated connector, not web search.
- Recurring work: automation. Existing-thread continuity: heartbeat, when supported locally.
- Mechanical lifecycle enforcement: hook.

Separate mixed scopes rather than forcing them into one surface. For plugin or connector failures, inspect the installed bundle, enabled state, authorization, MCP configuration, refresh requirements, and workspace policy. An API key does not establish ChatGPT, cloud, connector, or account access.
