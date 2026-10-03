Fetch the live GPT-6 Astra `## Prompting best practices` section, stopping at the next H2:

https://developers.openai.com/api/docs/guides/latest-model/gpt-6-astra.md#prompting-best-practices

Use the following only as disclosed fallback judgment when live guidance is unavailable. It is condensed, not a synchronized documentation copy, and establishes no current model or API facts.

Tie each prompt edit to a representative failing trace:
- Unnecessary pauses: specify which authorized work should proceed autonomously and which missing decisions warrant a question.
- Conflicting instructions: audit loaded skills and `AGENTS.md`. Remove contradictions rather than adding another permission rule. Respect the actual instruction hierarchy.
- Excessive formatting or recurring phrases: specify the application's needed response structure and audience.
- Too little delegation: state when parallel work helps and what collaboration tools actually exist.
- Excessive testing: require checks that exercise changed contracts. Broaden or repeat only for new changes, failures, or unresolved risk.

Keep the working model/API baseline fixed while comparing prompt treatments. Edit the directly tied prompt surface, not unrelated runtime schemas or tests.
