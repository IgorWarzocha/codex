Fetch current GPT-6 Astra migration guidance before changing code:

https://developers.openai.com/api/docs/guides/latest-model/gpt-6-astra.md#migration-quickstart

This checklist preserves migration judgment, not current availability, parameters, prices, or limits. If live guidance is unavailable, disclose that gap and do not implement unverified API changes.

## Preserve roles before editing

Inventory active model defaults and their config, deployment, prompt, router, registry, parser, and test surfaces. Record each site's endpoint, effective reasoning, latency and cost role, tools, schemas, caching, replay, and multimodal inputs.

Classify sites as a simple model change, tier-aware routing change, compatibility change, measured prompt fix, optional feature adoption, or unchanged historical/pinned usage. Migrate quality-first sites to the explicit Astra target only when compatible. Preserve balanced and cheaper routes, verifying Terra and Luna against current documentation rather than mapping every site to Astra. Leave unrelated providers, fixtures, comparisons, and eval baselines unchanged.

Verify the `gpt-6` alias before using it. Record returned `response.model` during validation rather than assuming alias and explicit slug are identical in billing or analytics.

## Check compatibility

Confirm these known migration pressure points against live docs before applying them:
- Reasoning: preserve effective effort. The bundled guide mapped `none` or `minimal` to `low` for evaluation.
- Endpoint and tools: the bundled guide required Responses for Astra tool calling, despite Chat Completions support.
- Sampling and logprobs: check `temperature`, `top_p`, `top_logprobs`, Chat Completions `logprobs`, and Responses `include` entries.
- Cache: check boundaries, cache-write billing, and migration from `prompt_cache_retention` to `prompt_cache_options.ttl`.
- Service tier: verify Fast mode's EU residency restrictions and SLA before preserving fast or priority settings.
- State and tools: preserve call IDs, continuation, retries, replay, and pending work.
- Contracts: preserve structured-output schemas, refusals, parsers, citations, and required artifacts.
- Modalities and limits: verify image/PDF/file detail, context and output limits, and long-context cost for each route.

A baseline upgrade does not authorize adopting Pro mode, persisted reasoning, programmatic or async tool calling, mid-turn steering, `configuration_update`, or multi-agent behavior. Isolate intentional feature adoption from the baseline. Report out-of-scope compatibility blockers instead of weakening schemas or silently changing business behavior.

## Validate the affected behavior

Compare the old model, prompt, and settings with Astra using the same prompt and preserved supported effort. Then isolate any effort tuning, surgical prompt fix, or optional feature treatment.

Exercise each changed router role. Measure task success, parser validity, tool arguments and completion, retries, latency, token/cache use, and cost per successful task. Include long-context, replay, or visual traces where affected. Use existing tests and representative evals.

Report target mappings, actual changes, preserved sites, compatibility blockers, and validation gaps. A model-string replacement alone is not a completed migration.
