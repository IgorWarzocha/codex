Inspect the local client's actual MCP support and configuration. Verify current upstream setup documentation before recommending the endpoint `https://developers.openai.com/mcp` or these stock Codex forms:

```sh
codex mcp add openaiDeveloperDocs --url https://developers.openai.com/mcp
```

```toml
[mcp_servers.openaiDeveloperDocs]
url = "https://developers.openai.com/mcp"
```

Check the MCP listing, enabled state, workspace or administrator policy, and documented authentication requirements. Confirm success from command output, configuration, or a callable documentation search and fetch. Recommend a client restart or new session only when observed behavior or current documentation requires it.

A configured server or skill dependency does not make tools callable in an existing session. Editing hosted-container configuration cannot install tools into the host's current inventory. Ordinary documentation requests use the official web fallback, without installation or escalation. Change local configuration only when requested.
