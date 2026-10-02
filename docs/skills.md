# Skills

Codex discovers skills through its native configured sources. The `skills` tool loads the catalog and instructions on demand instead of adding the full catalog to every session prompt.

In Code Mode and Notebook Mode:

```js
text(await tools.skills("list"))
text(await tools.skills("list code session"))
text(await tools.skills("read communication codebase-hygiene"))
text(await tools.skills("read codebase-hygiene testing"))
```

`list` groups skills by category. Repository skills appear under `session`. `read` accepts an exact skill name or a provided package locator. Additional exact skill names load those packages. Other selectors resolve Markdown references across the available skills.

A read containing only reference selectors returns those references and their source paths, without repeating the primary skill. Qualify ambiguous references with `skill-name/references/reference-name`, or use their listed source paths. Full package-contained resource locators also work for executor and cloud skills. Local package inventories include paths for scripts and assets. Executor inventories use authority-bearing resource locators and provide a named environment root for filesystem access. Cloud resource locators stay with their provider and are not local paths.

Reads return complete selected instructions. Output over 48 KiB is rejected. Read fewer packages or select references when a result is too large. Native skill disablement, explicit invocation and source access remain in effect.

For skill installation and authoring, refer to the [upstream documentation](https://developers.openai.com/codex/skills).
