//! Exercises cloud-preferred name deduplication through native skill commands.

#![cfg(unix)]

use std::sync::Arc;

use anyhow::Result;
use codex_core::config::Config;
use codex_extension_api::ExtensionRegistryBuilder;
use codex_features::Feature;
use codex_protocol::capabilities::CapabilityRootLocation;
use codex_protocol::capabilities::SelectedCapabilityRoot;
use codex_protocol::protocol::EnvironmentConfigState;
use codex_skills_extension::SkillProvider;
use codex_skills_extension::SkillProviders;
use codex_skills_extension::SkillsExtensionConfig;
use codex_skills_extension::catalog::SkillAuthority;
use codex_skills_extension::catalog::SkillCatalog;
use codex_skills_extension::catalog::SkillCatalogEntry;
use codex_skills_extension::catalog::SkillPackageId;
use codex_skills_extension::catalog::SkillReadResult;
use codex_skills_extension::catalog::SkillResourceId;
use codex_skills_extension::catalog::SkillSearchResult;
use codex_skills_extension::catalog::SkillSourceKind;
use codex_skills_extension::install_with_providers;
use codex_skills_extension::provider::SkillListQuery;
use codex_skills_extension::provider::SkillProviderFuture;
use codex_skills_extension::provider::SkillReadContext;
use codex_skills_extension::provider::SkillReadRequest;
use codex_skills_extension::provider::SkillSearchRequest;
use core_test_support::context_snapshot;
use core_test_support::context_snapshot::ContextSnapshotOptions;
use core_test_support::responses;
use core_test_support::skip_if_no_network;
use core_test_support::test_codex::TestEnv;
use core_test_support::test_codex::environment_config_for_selection;
use core_test_support::test_codex::test_codex;
use pretty_assertions::assert_eq;

use super::super::multi_exec_server_sandbox::ExecServerProcess;
use super::super::skills_extension::CatalogSkillProvider;

struct CloudSkillProvider {
    catalog: SkillCatalog,
}

impl SkillProvider for CloudSkillProvider {
    fn list(&self, _query: SkillListQuery) -> SkillProviderFuture<'_, SkillCatalog> {
        Box::pin(async { Ok(self.catalog.clone()) })
    }

    fn read<'a>(
        &'a self,
        request: SkillReadRequest<'a>,
    ) -> SkillProviderFuture<'a, SkillReadResult> {
        Box::pin(async move {
            let entry = self
                .catalog
                .entries
                .iter()
                .find(|entry| entry.id == request.package)
                .expect("read must select a cloud package");
            assert_eq!(request.authority, entry.authority);
            assert_eq!(request.resource, entry.main_prompt);
            assert!(matches!(request.context, SkillReadContext::Cloud { .. }));
            Ok(SkillReadResult {
                resource: request.resource,
                contents: format!("Cloud instructions for {}.", entry.name),
            })
        })
    }

    fn search(&self, _request: SkillSearchRequest) -> SkillProviderFuture<'_, SkillSearchResult> {
        Box::pin(async { Ok(SkillSearchResult::default()) })
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cloud_preference_deduplicates_native_lists_and_routes_name_reads() -> Result<()> {
    skip_if_no_network!(Ok(()));

    let server = responses::start_mock_server().await;
    // Cloud skills are intentionally unavailable in local-executor threads.
    // Use the real remote fixture so both native sources can participate.
    let executor = ExecServerProcess::start().await?;
    let executor_environment =
        TestEnv::local_with_exec_server_url(Some(executor.websocket_url.clone())).await?;
    let roots = ["skill://executor-a", "skill://executor-b"];
    let description = "Executor skill instructions.";
    let mut executor_entries = Vec::new();
    for (plugin, root) in ["demo", "other"].into_iter().zip(roots) {
        for index in 0..3 {
            let package = format!("{root}/s{index}");
            executor_entries.push(SkillCatalogEntry::new(
                SkillPackageId(package.clone()),
                SkillAuthority::new(SkillSourceKind::Executor, "executor"),
                format!("{plugin}:s{index}"),
                description,
                SkillResourceId::new(format!("{package}/SKILL.md")),
            ));
        }
    }
    let cloud_entries = (0..3)
        .map(|index| {
            SkillCatalogEntry::new(
                SkillPackageId(format!("skill://cloud/s{index}")),
                SkillAuthority::new(SkillSourceKind::Cloud, "cloud"),
                format!("demo:s{index}"),
                "Cloud skill instructions.",
                SkillResourceId::new(format!("skill://cloud/s{index}/SKILL.md")),
            )
        })
        .collect();
    let cloud = Arc::new(CloudSkillProvider {
        catalog: SkillCatalog {
            entries: cloud_entries,
            warnings: Vec::new(),
        },
    });
    let mut extensions = ExtensionRegistryBuilder::new();
    install_with_providers(
        &mut extensions,
        SkillProviders::new()
            .with_executor_provider(Arc::new(CatalogSkillProvider {
                catalog: SkillCatalog {
                    entries: executor_entries,
                    warnings: Vec::new(),
                },
            }))
            .with_cloud_provider(cloud),
        |config: &Config| SkillsExtensionConfig {
            include_instructions: config.include_skill_instructions,
            max_context_tokens: config.skill_max_context_tokens,
            bundled_skills_enabled: false,
            cloud_skill_enabled: config.cloud_skill_enabled,
            shadow_selection_enabled: false,
        },
    );
    let mock = responses::mount_sse_sequence(
        &server,
        (0..2)
            .flat_map(|index| {
                [
                    responses::sse(vec![
                        responses::ev_custom_tool_call(&format!("list-{index}"), "skills", "list"),
                        responses::ev_custom_tool_call(
                            &format!("read-{index}"),
                            "skills",
                            "read demo:s0",
                        ),
                        responses::ev_completed(&format!("tools-{index}")),
                    ]),
                    responses::sse(vec![
                        responses::ev_assistant_message(
                            &format!("message-{index}"),
                            "Skills are available.",
                        ),
                        responses::ev_completed(&format!("response-{index}")),
                    ]),
                ]
            })
            .collect(),
    )
    .await;
    let test = test_codex()
        .with_extensions(Arc::new(extensions.build()))
        .with_model("gpt-6-astra")
        .with_config(|config| {
            super::configure_scenario_catalog(config);
            config.cloud_skill_enabled = true;
            config
                .features
                .enable(Feature::ExecutorCapabilityDiscovery)
                .expect("enable executor capability discovery");
        })
        .build_with_environment(&server, executor_environment)
        .await?;
    let mut environment = test.executor_environment().selection().clone();
    let mut environment_config = environment_config_for_selection(&test.config, &environment);
    environment_config.selected_capability_roots = vec![SelectedCapabilityRoot {
        id: "executor-skills".to_string(),
        location: CapabilityRootLocation::Environment {
            environment_id: environment.environment_id.clone(),
            path: environment.cwd.clone(),
        },
    }];
    environment.config = EnvironmentConfigState::Ready(environment_config);
    for prompt in [
        "Which skills are available?",
        "Are those same skills still available?",
    ] {
        test.submit_turn_with_environments(prompt, Some(vec![environment.clone()]))
            .await?;
    }

    let requests = mock.requests();
    assert_eq!(requests.len(), 4);
    let expected_list = (0..3)
        .map(|index| format!("- demo:s{index}: Cloud skill instructions."))
        .chain((0..3).map(|index| format!("- other:s{index}: {description}")))
        .collect::<Vec<_>>()
        .join("\n");
    for (index, request) in [&requests[1], &requests[3]].into_iter().enumerate() {
        let output = |call_id: &str| {
            request
                .custom_tool_call_output_content_and_success(call_id)
                .expect("skills output")
                .0
                .expect("skills text")
        };
        assert_eq!(output(&format!("list-{index}")), expected_list);
        let read = output(&format!("read-{index}"));
        assert!(read.starts_with("Cloud instructions for demo:s0."));
        assert!(read.contains("Source: skill://cloud/s0/SKILL.md"));
    }
    insta::assert_snapshot!(
        "native_tool_dedup",
        context_snapshot::format_request_history_snapshot(
            "Native skills lists prefer cloud duplicates while retaining unique executor skills; name reads route to cloud instructions across turns.",
            &requests,
            &ContextSnapshotOptions::default().include_request_settings(),
        )
    );
    Ok(())
}
