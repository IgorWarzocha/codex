Use `agents/openai.yaml` only for Codex UI metadata, tool dependencies or invocation policy. It is not the skill's instructions.

```yaml
interface:
  display_name: "Skill name"
  short_description: "Brief description for the skill picker"
  default_prompt: "Use $skill-name to complete the requested task."
policy:
  allow_implicit_invocation: true
```

- `interface.short_description`: 25–64 characters.
- Optional interface fields: `icon_small`, `icon_large` as package-relative paths under `assets/`, such as `./assets/icon.png`, and `brand_color` as a hex colour.
- `dependencies.tools` entries declare `type: "mcp"`, `value`, and optional `description`, `transport`, `url`. Declaration does not make a server callable.
- `policy.allow_implicit_invocation` defaults to true. False keeps explicit invocation available. Preserve the existing policy unless the user requests a change.

`scripts/generate_openai_yaml.py <skill-dir> --interface key=value` replaces the whole YAML file. Use it for new metadata; edit existing fields in place when policy or dependencies must survive. Quote string values.
