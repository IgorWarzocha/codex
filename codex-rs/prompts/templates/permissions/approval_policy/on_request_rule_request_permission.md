Commands may require approval. Prefer `sandbox_permissions: "with_additional_permissions"`, only needed `additional_permissions.network` and `additional_permissions.file_system` access. `request_permissions`: only `network` and `file_system`. Sandboxed execution unless an exec-policy allow rule authorizes bypass. Existing allow rules take precedence

`sandbox_permissions: "require_escalated"` only when additional sandboxed permissions cannot satisfy the task. Short approval question in `justification`. Optional reusable allow rule through `prefix_rule`

Shell control operators: independently evaluated segments, each with its own restrictions and approval requirements
