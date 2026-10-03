Resolve `IMAGE_GEN` to the bundled `scripts/image_gen.py` beside this skill. Installed location:

```bash
export IMAGE_GEN="${CODEX_HOME:-$HOME/.codex}/skills/.system/imagegen/scripts/image_gen.py"
python "$IMAGE_GEN" generate --prompt "Test" --out output/imagegen/test.png --dry-run
```

Live calls need network access, `OPENAI_API_KEY`, and `openai` in the active Python environment. Install with that environment's package manager, using `uv pip install openai` in uv environments. Pillow is needed for downscaling and chroma-key removal. If the key is missing, have the user set it locally from https://platform.openai.com/api-keys, never paste it in chat. Dry-run requires neither the key nor SDK and prints payloads and paths without an API call.

Use `generate` for new images, `edit` for changes, and `generate-batch` for distinct JSONL jobs. Consult the subcommand's `--help` for flag inventory rather than writing a runner.

```bash
python "$IMAGE_GEN" edit --image input.png \
  --prompt "Replace only the background. Keep the product and its edges unchanged." \
  --out output/imagegen/product-edited.png
python "$IMAGE_GEN" generate-batch --input tmp/imagegen/prompts.jsonl \
  --out-dir output/imagegen/batch --concurrency 5
```

Batch lines may be plain prompts or JSON objects. Use one job per distinct asset:
```json
{"prompt":"A matte ceramic mug on stone","size":"1536x1024","out":"mug.png"}
```

Per-job fields override shared defaults, including model, size, quality, output format, and prompt augmentation. Batch `out` is a filename under required `--out-dir`. `--n` produces variants of one prompt, not distinct assets. Batch concurrency defaults to 5, with retry and fail-fast controls. Use `tmp/imagegen/` for intermediates and remove them when done. Finals default to `output/imagegen/`. Existing targets fail unless `--force` is passed. Downscaled copies use `-web` by default.

## Model constraints

These flags are CLI controls, not built-in `image_gen` arguments.

Default: `gpt-image-2`, size `auto`, quality `medium`, PNG output. Use `low` for drafts, then `medium`, `high`, or `auto` for final assets, dense labels, and identity-sensitive edits.

- `gpt-image-2` inputs always use high fidelity. Do not set `--input-fidelity`.
- Size is `auto` or `WIDTHxHEIGHT`: both edges multiples of 16, maximum edge 3840, aspect ratio at most 3:1, total pixels 655,360 through 8,294,400. Output above 2560x1440 total pixels is experimental.
- Use `1024x1024` for quick square drafts, `2048x1152` for 2K landscape, `3840x2160` or `2160x3840` for 4K.
- Older GPT Image models use `auto`, `1024x1024`, `1536x1024`, or `1024x1536`. On supporting models, edit-only `--input-fidelity high` improves preservation but increases input token usage. `gpt-image-1` and `gpt-image-1-mini` give the first image richer detail. `gpt-image-1.5` gives the first five higher fidelity.

## Masks

Pass repeated `--image` flags in prompt-index order. Edits support up to 16 inputs and one `--mask`, applied to the first image. Image and mask must share size and format and each be under 50MB. The mask needs alpha, preferably PNG. Check this yourself: the helper performs file checks and warnings, not full mask preflight. Masking is prompt-guided, not a pixel-perfect boundary guarantee. Repeat change-only and keep-unchanged constraints.

## Transparency and failures

`gpt-image-2` does not support `--background transparent`. If local alpha extraction is acceptable, generate a flat chroma-key background and use bundled `scripts/remove_chroma_key.py`:

```bash
python "$(dirname "$IMAGE_GEN")/remove_chroma_key.py" \
  --input keyed.png --out output/imagegen/cutout.png --key-color '#00ff00'
```

Choose a key color absent from the subject. Inspect edges, spill, missing subject pixels, and actual alpha. Local extraction is not native transparency. If native transparency is required, the subject is too complex, or removal fails validation, explain the tradeoff and ask before switching to `gpt-image-1.5`, unless that model was explicitly requested. Use `--background transparent --output-format png` or `webp` with the confirmed model.

If an API option is unsupported, retry without it only when it is not a user requirement. Do not drop required transparency or silently change models. For network failures, load `references/codex-network.md`.
