use codex_extension_api::ToolEnvironment;
use codex_extension_api::ToolExecutor;
use codex_extension_api::ToolName;
use codex_extension_api::ToolOutput;
use pretty_assertions::assert_eq;

use super::*;

async fn run<'a>(
    tool: &dyn for<'call> ToolExecutor<ToolCall<'call>>,
    command: &str,
    environments: Vec<ToolEnvironment<'a>>,
) -> Result<Box<dyn ToolOutput>, FunctionCallError> {
    tool.handle(ToolCall {
        turn_id: "turn".to_string(),
        call_id: "call".to_string(),
        tool_name: ToolName::plain("skills"),
        model: "test".to_string(),
        codex_turn_metadata: None,
        truncation_policy: TruncationPolicy::Bytes(128 * 1024),
        source: ToolCallSource::Direct,
        conversation_history: ConversationHistory::default(),
        turn_item_emitter: Arc::new(NoopTurnItemEmitter),
        environments,
        payload: ToolPayload::Custom {
            input: command.to_string(),
        },
    })
    .await
}

fn write_skill(root: &std::path::Path, package: &str, name: &str, body: &str) -> TestResult {
    let directory = root.join(package);
    std::fs::create_dir_all(directory.join("references"))?;
    std::fs::write(
        directory.join("SKILL.md"),
        format!("---\nname: {name}\ndescription: Use {name}.\n---\n{body}\n"),
    )?;
    Ok(())
}

#[tokio::test]
async fn host_progressive_commands_use_native_discovery_and_package_access() -> TestResult {
    let temp = tempfile::tempdir()?;
    let global = temp.path().join("skills");
    let project = temp.path().join(".codex");
    write_skill(
        &global,
        "communication",
        "communication",
        "COMMUNICATION_BODY",
    )?;
    write_skill(&global, "code/hygiene", "hygiene", "HYGIENE_BODY")?;
    write_skill(
        &project.join("skills"),
        "session-skill",
        "session-skill",
        "SESSION_BODY",
    )?;
    std::fs::write(
        global.join("communication/references/conversation.md"),
        "CONVERSATION_REFERENCE",
    )?;
    std::fs::write(
        global.join("communication/references/shared.md"),
        "COMMUNICATION_SHARED",
    )?;
    std::fs::write(
        global.join("code/hygiene/references/testing.md"),
        "TESTING_REFERENCE",
    )?;
    std::fs::write(
        global.join("code/hygiene/references/shared.md"),
        "HYGIENE_SHARED",
    )?;
    std::fs::create_dir_all(global.join("communication/assets/deep"))?;
    std::fs::write(
        global.join("communication/assets/deep/hidden.txt"),
        "DEEP_ASSET",
    )?;
    std::fs::create_dir_all(global.join("communication/node_modules"))?;
    std::fs::write(
        global.join("communication/node_modules/hidden.txt"),
        "DEPENDENCY",
    )?;
    let cwd = AbsolutePathBuf::try_from(temp.path())?;
    let service = HostSkillsService::new_with_restriction_product(cwd.clone(), false, None);
    service.set_extra_roots(vec![AbsolutePathBuf::try_from(global.clone())?]);
    let layers = ConfigLayerStack::new(
        vec![ConfigLayerEntry::new(
            ConfigLayerSource::Project {
                dot_codex_folder: AbsolutePathBuf::try_from(project)?,
            },
            toml::from_str("[skills.bundled]\nenabled = false")?,
        )],
        Default::default(),
        ConfigRequirementsToml::default(),
    )?;
    let snapshot = service
        .snapshot_for_config(
            &HostSkillsLoadInput::new(cwd, vec![], layers),
            Some(Arc::clone(&LOCAL_FS)),
        )
        .await;
    assert!(snapshot.outcome().errors.is_empty());
    let mut builder = ExtensionRegistryBuilder::new();
    install(&mut builder, skills_extension_config);
    let registry = builder.build();
    let session = ExtensionData::new("session");
    let thread = ExtensionData::new("thread");
    registry.thread_lifecycle_contributors()[0]
        .on_thread_start(ThreadStartInput {
            config: &default_config(),
            session_source: &SessionSource::Cli,
            persistent_thread_state_available: true,
            environments: &[],
            mcp_resource_client: None,
            extension_metrics: None,
            session_store: &session,
            thread_store: &thread,
        })
        .await;
    let step = ExtensionData::new("turn");
    // Core captures the host snapshot before it constructs the tool router.
    step.insert(snapshot);
    let tools = registry.tool_contributors()[0].tools_for_step(&session, &thread, &step);
    assert_eq!(tools.len(), 1);
    let tool = tools[0].as_ref();
    let listed = run(tool, "list", vec![]).await?.log_output();
    assert_eq!(
        listed,
        "- communication: Use communication.\n\n# SESSION\n- session-skill: Use session-skill.\n\n# CODE\n- hygiene: Use hygiene."
    );
    assert_eq!(
        run(tool, "list code", vec![]).await?.log_output(),
        "# CODE\n- hygiene: Use hygiene."
    );
    assert!(
        matches!(run(tool, "list unknown", vec![]).await, Err(FunctionCallError::RespondToModel(message)) if message.contains("Available: code, session"))
    );
    let full = run(tool, "read communication", vec![]).await?.log_output();
    assert!(full.starts_with("COMMUNICATION_BODY\n\n---\nSkill paths"));
    assert!(
        full.contains(
            &global
                .join("communication/references/conversation.md")
                .display()
                .to_string()
        )
    );
    assert!(full.contains("assets/deep"));
    assert!(!full.contains("hidden.txt"));
    assert!(!full.contains("CONVERSATION_REFERENCE"));
    let mixed = run(tool, "read communication hygiene", vec![])
        .await?
        .log_output();
    assert!(mixed.contains("--- communication ---\nCOMMUNICATION_BODY"));
    assert!(mixed.contains("--- hygiene ---\nHYGIENE_BODY"));
    let references = run(tool, "read communication conversation testing", vec![])
        .await?
        .log_output();
    assert!(
        references
            .contains("--- communication/references/conversation ---\nCONVERSATION_REFERENCE")
    );
    assert!(references.contains("--- hygiene/references/testing ---\nTESTING_REFERENCE"));
    assert!(!references.contains("BODY"));
    assert!(!references.contains("Skill paths"));
    assert!(
        matches!(run(tool, "read communication shared", vec![]).await, Err(FunctionCallError::RespondToModel(message)) if message.contains("Ambiguous reference") && message.contains("hygiene/references/shared"))
    );
    let qualified = run(tool, "read communication hygiene/references/shared", vec![])
        .await?
        .log_output();
    assert!(qualified.starts_with("HYGIENE_SHARED\n\n---\nSources:"));
    let absolute = global.join("code/hygiene/references/testing.md");
    assert!(
        run(tool, &format!("read {}", absolute.display()), vec![])
            .await?
            .log_output()
            .starts_with("TESTING_REFERENCE")
    );
    assert!(matches!(
        run(tool, "read communication references/../SKILL.md", vec![]).await,
        Err(FunctionCallError::RespondToModel(_))
    ));
    Ok(())
}

#[tokio::test]
async fn cloud_reads_keep_authority_and_external_context_and_hide_opted_out_names() -> TestResult {
    let requests = Arc::new(Mutex::new(vec![]));
    let entries = vec![
        test_entry(
            SkillSourceKind::Cloud,
            "codex_apps",
            "cloud/demo",
            "cloud/demo/SKILL.md",
        ),
        test_entry(
            SkillSourceKind::Cloud,
            "codex_apps",
            "cloud/hidden",
            "cloud/hidden/SKILL.md",
        )
        .hidden_from_prompt(),
        test_entry(
            SkillSourceKind::Cloud,
            "codex_apps",
            "cloud/disabled",
            "cloud/disabled/SKILL.md",
        )
        .disabled(),
    ];
    let providers = SkillProviders::new().with_cloud_provider(Arc::new(StaticSkillProvider {
        catalog: SkillCatalog {
            entries,
            warnings: vec![],
        },
        read_requests: Arc::clone(&requests),
        list_calls: None,
        fail_first_list: false,
    }));
    let mut builder = ExtensionRegistryBuilder::new();
    install_with_providers(&mut builder, providers, skills_extension_config);
    let registry = builder.build();
    let session = ExtensionData::new("session");
    let thread = ExtensionData::new("thread");
    registry.thread_lifecycle_contributors()[0]
        .on_thread_start(ThreadStartInput {
            config: &default_config(),
            session_source: &SessionSource::Cli,
            persistent_thread_state_available: true,
            environments: &[],
            mcp_resource_client: None,
            extension_metrics: None,
            session_store: &session,
            thread_store: &thread,
        })
        .await;
    start_registered_turn(&registry, &session, &thread, "turn").await;
    let tools = registry.tool_contributors()[0].tools(&session, &thread);
    let list = run(tools[0].as_ref(), "list", vec![]).await?;
    assert_eq!(list.log_output(), "- demo: Fix lint errors.");
    assert!(list.contains_external_context());
    let reference = run(
        tools[0].as_ref(),
        "read cloud/demo cloud/demo/references/check.md",
        vec![],
    )
    .await?;
    assert!(reference.contains_external_context());
    assert_eq!(
        read_request_keys(&requests),
        vec![(
            SkillAuthority::new(SkillSourceKind::Cloud, "codex_apps"),
            SkillPackageId("cloud/demo".to_string()),
            SkillResourceId::new("cloud/demo/references/check.md")
        )]
    );
    assert!(run(tools[0].as_ref(), "read hidden", vec![]).await.is_err());
    assert!(
        run(tools[0].as_ref(), "read disabled", vec![])
            .await
            .is_err()
    );
    assert!(
        run(tools[0].as_ref(), "read cloud/hidden", vec![])
            .await
            .is_ok()
    );
    assert!(
        run(tools[0].as_ref(), "read cloud/disabled", vec![])
            .await
            .is_err()
    );
    Ok(())
}
