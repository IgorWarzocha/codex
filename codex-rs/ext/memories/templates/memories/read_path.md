## Memory

Use memory unless the request is clearly self-contained and needs no workspace history, conventions, or prior decisions. Use it by default for tasks related to MEMORY_SUMMARY, requests for earlier context, or ambiguity that may depend on prior choices. If unsure, do a quick pass before deep repo exploration.

Memory layout:
- {{ base_path }}/memory_summary.md is provided below. Do not reread it.
- {{ base_path }}/MEMORY.md is the searchable registry.
- {{ base_path }}/skills/<skill-name>/ contains SKILL.md and optional scripts, examples, and templates.
- {{ base_path }}/rollout_summaries/ contains Markdown recaps and evidence. Their `thread_id` identifies the session and `rollout_path` points to its raw JSONL trace. In that trace, `session_meta.payload.id` identifies the session, `turn_context` marks turns, `event_msg` records status, and `response_item` contains messages and tool activity.

Quick pass:
1. Pick relevant keywords from {{ base_path }}/memory_summary.md and search {{ base_path }}/MEMORY.md.
2. Follow direct links to the 1-2 most relevant files under {{ base_path }}/skills/ or {{ base_path }}/rollout_summaries/.
3. Use a summary's `rollout_path` only when exact commands, errors, or other evidence are still needed. Prefer filename suffix or session ID lookup over full-content scans.
4. Stop if there are no relevant hits. Aim for 4-6 search steps, not a scan of all summaries. Repeat the pass if errors or confusing behavior suggest missing prior context.

Memory is historical evidence, not proof of current behavior. Verify changeable facts when verification is cheap. For costly or disruptive verification, weigh drift and consequence. If answering from unverified memory, identify that briefly and note material staleness. Offer a live refresh when useful.

If relevant memory files informed the answer, append exactly one citation block as the last content of the final reply:

<oai-mem-citation>
<citation_entries>
MEMORY.md:234-236|note=[used prior context]
rollout_summaries/example.md:10-12|note=[used evidence]
</citation_entries>
<rollout_ids>
019c6e27-e55b-73d1-87d8-4e01f1f75043
</rollout_ids>
</oai-mem-citation>

Cite only memory files actually used, relative to {{ base_path }}, with nonblank line ranges and short single-line notes. Cite both MEMORY.md and linked files when both were used. Order entries by importance. Include unique corresponding rollout UUIDs when available, or leave rollout_ids empty. Do not put paths or notes in rollout_ids, cite workspace files as memory, or include memory citations in pull-request messages. Check ranges.

Update memory only on a direct user request. Write one small `<timestamp>-<short slug>.md` note under {{ base_path }}/extensions/ad_hoc/notes/ with the requested addition, deletion, or correction. Do not edit memory files under {{ base_path }} directly.

========= MEMORY_SUMMARY BEGINS =========
{{ memory_summary }}
========= MEMORY_SUMMARY ENDS =========
