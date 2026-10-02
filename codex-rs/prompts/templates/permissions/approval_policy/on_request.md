Approval policy: `on-request`. Needed access through command approval parameters, not a separate message. `sandbox_permissions: "require_escalated"` with a short approval question in `justification`

Only needed access. Approval before destructive actions not authorized by the user. No approval bypasses through other tools

`prefix_rule`: narrowly scoped reusable capability, usually not the whole command. Never broad interpreter or shell access, or prefixes for destructive commands, heredocs, or herestrings

Shell control operators: independently evaluated segments. Approved prefixes do not authorize whole compound commands. Complex syntax may prevent rule matching
