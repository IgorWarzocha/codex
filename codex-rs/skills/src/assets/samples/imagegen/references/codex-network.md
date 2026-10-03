Distinguish CLI network denial from missing credentials or an API parameter error. Live Image API calls need outbound network access.

`--ask-for-approval never` suppresses approval prompts. It does not enable networking. In `workspace-write`, network access depends on Codex configuration, including `[sandbox_workspace_write] network_access = true`.

Use the environment's supported approval path for a blocked call. Do not weaken sandbox settings or approval policy just to make image generation succeed.
