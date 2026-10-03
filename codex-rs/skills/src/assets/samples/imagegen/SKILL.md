---
name: "imagegen"
description: "Use for AI-generated or edited raster assets. Prefer native edits for existing SVG, vector, or code-based visuals."
---

1. Use built-in `image_gen` by default, including transparent output. It needs no `OPENAI_API_KEY`. Ordinary size, quality, destination, or "batch" requests do not select the API fallback. If the tool fails or is unavailable, explain the keyed CLI alternative and wait for explicit opt-in.
2. Generate when inputs are only style, composition, or subject references. Edit when parts of an existing image must survive. Label each input by index and role. For a local built-in edit target, load it with `view_image` first. Do not promise arbitrary filesystem-path edits or CLI-only controls through the tool.
3. Prompt with intended use, subject, framing, style, exact quoted text, and constraints. Normalize detailed requests without adding creative requirements. For sparse requests, add only useful framing or scene detail, not invented props, slogans, palettes, or story beats. For edits, state "change only X; keep Y unchanged" and repeat those invariants on every iteration.
4. Make one built-in call per distinct asset or requested variant. Do not pack distinct assets into one prompt. Request actual transparency for cutouts and preserve alpha.
5. Inspect subject, composition, text, edit invariants, and cutout edges. Correct one defect at a time. Raster requests need actual images, not SVG or HTML placeholders. Existing vector systems and deterministic code-native visuals should be edited natively instead.
6. Built-in outputs default under `$CODEX_HOME/generated_images/...`, not OS temp. Generate first, then copy or move selected finals to the user's destination or into the workspace for project use. Update consumers and retain every requested final, not discarded variants. Preview-only outputs may stay at the default path and be shown inline. Save sibling versions unless replacement was requested. Report final workspace paths, prompts, and execution mode.

Load only the relevant branch:

- Text-heavy layouts, asset-specific prompting, compositing, or identity-sensitive edits: `references/prompting.md`.
- Explicitly requested or confirmed CLI/API/model controls, including masks: `references/cli.md`. Use bundled `scripts/image_gen.py`, not a one-off SDK runner. Do not modify the helper. Never silently switch to `gpt-image-1.5`, even after CLI opt-in.
- CLI network or sandbox failure: `references/codex-network.md`.
