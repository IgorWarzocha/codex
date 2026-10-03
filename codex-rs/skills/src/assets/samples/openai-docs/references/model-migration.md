Preserve an explicitly named target and retrieve its current official guidance. Do not run the latest-model resolver or load another model's guide for a named target.

For an unspecified, latest, or current migration target, first search and fetch `https://developers.openai.com/api/docs/guides/latest-model`. If dynamic target metadata is needed, resolve `<skill-dir>` to this installed package and run:

```sh
sh <skill-dir>/scripts/resolve-latest-model-info
```

On Windows, use Node.js 18+:

```text
node <skill-dir>\scripts\resolve-latest-model-info.cjs
```

If Windows lacks Node and `load_workspace_dependencies` is callable, use its returned runtime and retry once. Do not execute the POSIX wrapper on Windows.

Keep resolver stdout visible. Success requires JSON with nonempty `model`, `migrationGuideUrl`, and `promptingGuideUrl`. On command failure or missing fields, retry once, then use current official documentation. Do not fall through to a hardcoded latest model.

Fetch returned URLs exactly, treating them as opaque. Retry the same URL if it yields only a title or no substantive body:
- Migration: fetch `migrationGuideUrl`.
- Requested prompting or a necessary prompt change: fetch `promptingGuideUrl`, extracting `## Prompting best practices` through the next H2.
- Current-model prompting with no explicit target: use the same dynamic resolution. Named-model prompting needs its own guide, not a resolver.

Only for an actual GPT-6 Astra migration needing compatibility or tier-routing judgment, read `references/upgrading-to-gpt-6-astra.md`. For GPT-6 Astra prompt work when live guidance is unavailable, read `references/prompting-guide.md` and disclose the fallback. Neither reference establishes current API facts or applies to another model.

Preserve workload roles, effective reasoning, endpoints, tools, and output contracts. Change only requested active defaults and tied surfaces. Do not collapse tiered routers, replace pinned fallbacks, or rewrite historical fixtures and eval baselines. Verify current prices, limits, and capability metadata before registry changes. If compatibility requires out-of-scope endpoint, schema, or tool-handler changes, report the exact blocker and smallest follow-up.
