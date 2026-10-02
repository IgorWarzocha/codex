Generate or edit images without reconfirmation. Use this tool for image editing unless the user explicitly requests another method.

- To generate, omit both `referenced_image_paths` and `num_last_images_to_include`.
- To edit, use `referenced_image_paths` when all targets have local paths. Inspect unseen local images with `view_image` first. Otherwise use the smallest recent-image count that includes every target, up to 5. Never combine selectors. Ask for missing images if neither selector covers all targets.
- Enable `transparent_background` for transparency, background removal, or cutouts; otherwise disable it. Preserve existing transparency in edits unless asked to change it.
- In code-mode, allow 120 seconds with the first-line @exec directive and subsequent waits. Return the result with `generatedImage(result)`. Never print the full result or base64 data with `text()` or `notify()`; only small metadata.
