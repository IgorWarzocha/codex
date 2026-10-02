//! Native discovery still runs at the world-state boundary. The catalogue is
//! captured by the tools rather than emitted into the standing prompt.

use codex_exec_server::ResolvedSelectedCapabilityRoot;
use codex_extension_api::ExtensionEventSink;
use codex_extension_api::ExtensionWarning;
use codex_extension_api::SelectedPluginSnapshot;
use codex_extension_api::WorldStateContributionInput;

use crate::HostSkillsSnapshot;
use crate::catalog::SkillCatalog;
use crate::catalog::SkillSourceKind;
use crate::provider::SkillListQuery;
use crate::provider::attribute_executor_plugins;
use crate::sources::SkillProviders;
use crate::state::ExecutorSkillsStepState;
use crate::state::HostSkillsStepState;
use crate::state::SkillsThreadState;
use crate::warnings::bounded_warnings;

pub(crate) async fn discover_step_skills(
    providers: &SkillProviders,
    event_sink: &dyn ExtensionEventSink,
    input: WorldStateContributionInput<'_>,
) {
    let Some(state) = input.thread_store.get::<SkillsThreadState>() else {
        return;
    };
    let config = state.config();
    let host_snapshot = input.turn_store.get::<HostSkillsSnapshot>();
    let query = SkillListQuery {
        turn_id: input.turn_id.to_string(),
        executor_roots: input.ready_selected_capability_roots.to_vec(),
        resolved_executor_roots: input
            .step_store
            .get::<Vec<ResolvedSelectedCapabilityRoot>>()
            .map(|roots| roots.as_ref().clone())
            .unwrap_or_default(),
        host_snapshot: host_snapshot.clone(),
        include_host_skills: host_snapshot.is_some() && providers.has_host_provider(),
        include_bundled_skills: config.bundled_skills_enabled,
        include_cloud_skills: false,
        mcp_resources: None,
        executor_capability_discovery: input.executor_capability_discovery.cloned(),
    };
    let mut host_query = query.clone();
    host_query.executor_roots.clear();
    host_query.resolved_executor_roots.clear();
    host_query.executor_capability_discovery = None;
    let (mut executor, host) = futures::join!(
        state.refresh_executor_catalog(providers, query),
        providers.list_for_turn(host_query),
    );
    if let Some(plugins) = input.step_store.get::<SelectedPluginSnapshot>() {
        attribute_executor_plugins(&mut executor, &plugins);
    }
    input
        .turn_store
        .insert(ExecutorSkillsStepState(executor.clone()));
    if host_snapshot.is_some() {
        input.turn_store.insert(HostSkillsStepState(SkillCatalog {
            entries: host
                .entries
                .iter()
                .filter(|entry| entry.authority.kind == SkillSourceKind::Host)
                .cloned()
                .collect(),
            warnings: host.warnings.clone(),
        }));
    }
    let mut catalog = if state.cloud_skill_enabled() {
        state.cloud_catalog_snapshot()
    } else {
        SkillCatalog::default()
    };
    catalog.extend(executor);
    catalog.extend(host);
    for message in bounded_warnings(&catalog.warnings) {
        event_sink.emit_warning(ExtensionWarning {
            thread_id: input.thread_store.level_id().to_string(),
            turn_id: Some(input.turn_id.to_string()),
            message,
        });
    }
}
