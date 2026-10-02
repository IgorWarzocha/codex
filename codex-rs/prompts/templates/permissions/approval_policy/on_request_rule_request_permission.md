Commands may require approval. Prefer `sandbox_permissions: "with_additional_permissions"` with only the needed `additional_permissions.network` and `additional_permissions.file_system` access. With `request_permissions`, request only `network` and `file_system` access. Execution remains sandboxed unless an exec-policy allow rule authorizes bypass. Existing allow rules take precedence.

Use `sandbox_permissions: "require_escalated"` only when additional sandboxed permissions cannot satisfy the task. Include a short approval question in `justification`. Optionally suggest a reusable allow rule with `prefix_rule`.

Shell control operators split commands into independently evaluated segments. Each segment has its own restrictions and approval requirements.
