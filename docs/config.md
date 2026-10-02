# Configuration

For basic configuration instructions, see [this documentation](https://developers.openai.com/codex/config-basic).

For advanced configuration instructions, see [this documentation](https://developers.openai.com/codex/config-advanced).

For a full configuration reference, see [this documentation](https://developers.openai.com/codex/config-reference).

## Context continuity

`context_strategy` defaults to `"notes"`. The agent saves useful task state in
remote notes and reads those notes after a context reset. Earlier conversation
remains available through history, but is not automatically carried into the new
window.

Notes requires an OpenAI Codex backend provider and Codex backend authentication.
An unsupported session fails at startup with an actionable error. It does not
silently switch to summarization. API-key sessions and other providers should
explicitly select ordinary compaction in `config.toml`:

```toml
context_strategy = "compaction"
```

The strategy selects continuity independently of the legacy context-management
and token-budget activation flags. Managed requirements can restrict the selected
strategy. Native token budgets and checkpoint reminders still apply in notes mode.

Idle rollover is optional and disabled when the setting is absent. To reset a
notes window before the next user turn after 25 minutes idle:

```toml
context_strategy = "notes"
context_idle_rollover_minutes = 25
```

The idle interval must be a positive integer. Idle rollover uses saved settlement
and checkpoint state, not a background timer. A fresh notes checkpoint is required
for idle rollover.

For ordinary compaction, `compaction_retention_tokens` accepts `16000`, `32000`,
or `64000`, with `64000` as the default user-message retention budget:

```toml
context_strategy = "compaction"
compaction_retention_tokens = 32000
```

V2 compaction uses a separate 872,000-token input budget, independent of
`model_context_window`. Request trimming occurs only when the estimated compaction
input exceeds that budget.

## Lifecycle hooks

Admins can set top-level `allow_managed_hooks_only = true` in
`requirements.toml` to ignore user, project, and session hook configs while
still allowing managed hooks from requirements and managed config layers. This
setting is only supported in `requirements.toml`; putting it in `config.toml`
does not enable managed-hooks-only mode.
