use super::*;
use pretty_assertions::assert_eq;
use std::collections::BTreeMap;

fn windows_shell_guidance_description() -> String {
    format!("\n\n{}", windows_shell_guidance())
}

fn has_parameter(tool: &ToolSpec, parameter_name: &str) -> bool {
    serde_json::to_value(tool)
        .expect("tool spec should serialize")
        .pointer(&format!("/parameters/properties/{parameter_name}"))
        .is_some()
}

#[test]
fn exec_command_tool_matches_expected_spec() {
    let tool = create_exec_command_tool(CommandToolOptions {
        allow_login_shell: true,
        exec_permission_approvals_enabled: false,
    });

    let description = if cfg!(windows) {
        format!(
            "Run a shell command. Returns output and a session ID while running.{}",
            windows_shell_guidance_description()
        )
    } else {
        "Run a shell command. Returns output and a session ID while running.".to_string()
    };
    let yield_time_ms_description = if cfg!(windows) {
        "Wait before yielding a running session, default 10000 ms, Windows range 10000-30000 ms. Finished commands return immediately"
    } else {
        "Wait before yielding, default 10000 ms, range 250-30000 ms"
    };

    let mut properties = BTreeMap::from([
        (
            "cmd".to_string(),
            JsonSchema::string(Some("Shell command".to_string())),
        ),
        (
            "workdir".to_string(),
            JsonSchema::string(Some("Cwd, defaults to turn cwd".to_string())),
        ),
        (
            "shell".to_string(),
            JsonSchema::string(Some("Shell binary, defaults to user's shell".to_string())),
        ),
        (
            "tty".to_string(),
            JsonSchema::boolean(Some(
                "Allocate a PTY for stdin interaction; otherwise use pipes".to_string(),
            )),
        ),
        (
            "yield_time_ms".to_string(),
            JsonSchema::number(Some(yield_time_ms_description.to_string())),
        ),
        (
            "max_output_tokens".to_string(),
            JsonSchema::number(Some(
                "Output tokens, default 10000, subject to policy caps".to_string(),
            )),
        ),
        (
            "login".to_string(),
            JsonSchema::boolean(Some(
                "Enable -l/-i shell semantics, default true".to_string(),
            )),
        ),
    ]);
    properties.extend(create_approval_parameters(
        /*exec_permission_approvals_enabled*/ false,
    ));

    assert_eq!(
        tool,
        ToolSpec::Function(ResponsesApiTool {
            name: "exec_command".to_string(),
            description,
            strict: false,
            defer_loading: None,
            parameters: JsonSchema::object(
                properties,
                Some(vec!["cmd".to_string()]),
                Some(false.into())
            ),
            output_schema: Some(unified_exec_output_schema().into()),
        })
    );
}

#[test]
fn exec_command_tool_can_hide_shell_parameter() {
    let tool = create_exec_command_tool_with_environment_id(
        CommandToolOptions {
            allow_login_shell: true,
            exec_permission_approvals_enabled: false,
        },
        /*include_environment_id*/ false,
        /*include_shell_parameter*/ false,
        /*include_windows_shell_guidance*/ cfg!(windows),
    );

    assert!(!has_parameter(&tool, "shell"));
    assert!(has_parameter(&tool, "cmd"));
}

#[test]
fn write_stdin_tool_matches_expected_spec() {
    let tool = create_write_stdin_tool();

    let properties = BTreeMap::from([
        (
            "session_id".to_string(),
            JsonSchema::number(Some("Session ID from exec_command".to_string())),
        ),
        (
            "chars".to_string(),
            JsonSchema::string(Some(
                "Stdin bytes, only for tty=true sessions. Empty or omitted polls".to_string(),
            )),
        ),
        (
            "yield_time_ms".to_string(),
            JsonSchema::number(Some(
                "Wait before yielding. Writes default to 250 ms, cap 30000 ms. Empty polls wait 5000-300000 ms by default".to_string(),
            )),
        ),
        (
            "max_output_tokens".to_string(),
            JsonSchema::number(Some(
                "Output tokens, default 10000, subject to policy caps".to_string(),
            )),
        ),
    ]);

    assert_eq!(
        tool,
        ToolSpec::Function(ResponsesApiTool {
            name: "write_stdin".to_string(),
            description: "Write stdin or poll a running command for output".to_string(),
            strict: false,
            defer_loading: None,
            parameters: JsonSchema::object(
                properties,
                Some(vec!["session_id".to_string()]),
                Some(false.into())
            ),
            output_schema: Some(unified_exec_output_schema().into()),
        })
    );
}

#[test]
fn request_permissions_tool_includes_full_permission_schema() {
    let tool =
        create_request_permissions_tool("Request extra permissions for this turn.".to_string());

    let properties = BTreeMap::from([
        (
            "reason".to_string(),
            JsonSchema::string(Some("Why additional permissions are needed".to_string())),
        ),
        (
            "environment_id".to_string(),
            JsonSchema::string(Some(
                "ID from <environment_context>, defaults to primary environment".to_string(),
            )),
        ),
        ("permissions".to_string(), permission_profile_schema()),
    ]);

    assert_eq!(
        tool,
        ToolSpec::Function(ResponsesApiTool {
            name: "request_permissions".to_string(),
            description: "Request extra permissions for this turn.".to_string(),
            strict: false,
            defer_loading: None,
            parameters: JsonSchema::object(
                properties,
                Some(vec!["permissions".to_string()]),
                Some(false.into())
            ),
            output_schema: None,
        })
    );
}
