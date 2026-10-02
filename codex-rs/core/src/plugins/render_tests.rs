use super::*;
use pretty_assertions::assert_eq;

#[test]
fn render_plugins_section_returns_none_for_empty_plugins() {
    assert_eq!(render_plugins_section(&[]), None);
}

#[test]
fn render_plugins_section_keeps_plugin_usage_guidance_without_listing_plugins() {
    let rendered = render_plugins_section(&[PluginCapabilitySummary {
        config_name: "sample@test".to_string(),
        display_name: "sample".to_string(),
        plugin_namespace: None,
        description: Some("inspect sample data".to_string()),
        has_skills: true,
        ..PluginCapabilitySummary::default()
    }])
    .expect("plugin section should render");

    let expected = "<plugins_instructions>\nPlugins are not called directly. Use their exposed skills, MCP tools, and apps. Prefer a named plugin's capabilities when relevant. If unavailable, say so and use an available alternative. Plugin skills have a `plugin_name:` prefix. MCP provenance identifies plugin tools.\n</plugins_instructions>";

    assert_eq!(rendered, expected);
}

#[test]
fn explicit_plugin_instructions_use_manifest_namespace_for_skills() {
    let rendered = render_explicit_plugin_instructions(
        &PluginCapabilitySummary {
            config_name: "acme.tools@test".to_string(),
            display_name: "Acme Developer Tools".to_string(),
            plugin_namespace: Some("acme.tools".to_string()),
            has_skills: true,
            ..PluginCapabilitySummary::default()
        },
        &[],
        &[],
    )
    .expect("skill capability should render");

    assert!(rendered.contains("`acme.tools:`"));
    assert!(!rendered.contains("`Acme Developer Tools:`"));
    assert!(!rendered.contains("tool_search"));
}

#[test]
fn explicit_plugin_instructions_search_available_apps_before_fallback() {
    let rendered = render_explicit_plugin_instructions(
        &PluginCapabilitySummary {
            config_name: "app-adobe@openai-curated-remote".to_string(),
            display_name: "Adobe".to_string(),
            ..PluginCapabilitySummary::default()
        },
        &[],
        &["Adobe".to_string()],
    )
    .expect("app capability should render");

    assert_eq!(
        rendered,
        "Capabilities from the `Adobe` plugin:\n\
         - For the user request that explicitly selected this plugin, and only for that request, \
         if `tool_search` is available and an app from this plugin may help, search for its \
         tools before falling back to unrelated or built-in tools.\n\
         - Apps from this plugin available in this session: `Adobe`.\n\
         Use these plugin-associated capabilities to help solve the task."
    );
}

#[test]
fn explicit_plugin_instructions_are_bounded() {
    let servers = (0..1_024)
        .map(|index| format!("server-{index}"))
        .collect::<Vec<_>>();
    let apps = (0..1_024)
        .map(|index| format!("app-{index}"))
        .collect::<Vec<_>>();

    let rendered = render_explicit_plugin_instructions(
        &PluginCapabilitySummary {
            config_name: "sample@test".to_string(),
            display_name: "sample".to_string(),
            has_skills: true,
            ..PluginCapabilitySummary::default()
        },
        &servers,
        &apps,
    )
    .expect("MCP capability should render");

    assert!(rendered.len() <= MAX_EXPLICIT_PLUGIN_INSTRUCTIONS_BYTES);
    assert!(rendered.contains("only for that request"));
    assert!(rendered.contains("if `tool_search` is available"));
    assert!(rendered.contains("Skills from this plugin"));
    assert!(rendered.contains("`app-0`"));
    assert!(rendered.ends_with(TRUNCATED_PLUGIN_INSTRUCTIONS_SUFFIX));
}
