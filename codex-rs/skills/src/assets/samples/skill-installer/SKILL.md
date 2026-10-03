---
name: skill-installer
description: "Use when asked to list installable skills or install skills from GitHub."
metadata:
  short-description: Install curated skills from openai/skills or other repos
---

Use the bundled helpers. Listing defaults to `openai/skills`, `skills/.curated`, ref `main`. Use `skills/.experimental` only when requested. A bare invocation lists choices rather than installing everything.

- List: `scripts/list-skills.py [--repo owner/repo] [--path directory] [--ref ref] [--format json]`.
- Install: `scripts/install-skill-from-github.py --repo owner/repo --path skill/path [another/path ...]`.
- A GitHub tree URL can replace `--repo` and `--path` via `--url`.
- Overrides: `--ref`, `--dest`, `--method auto|download|git`, and `--name` for a single skill.

Installation defaults to `$CODEX_HOME/skills/<name>`, or `~/.codex/skills/<name>`. The helper refuses existing destinations. Do not delete them to turn an install into an unrequested overwrite. Bundled `.system` skills are already installed; do not reinstall them routinely.

Automatic download falls back to sparse Git checkout on HTTP 401, 403 or 404, trying HTTPS then SSH. Private repositories use existing Git credentials or `GITHUB_TOKEN`/`GH_TOKEN`. These helpers require network access; follow the active sandbox policy rather than assuming escalation is available.

Report the source and already-installed annotations when listing. On failure, report the helper's error. After a successful install, tell the user the skill is available on the next turn.
