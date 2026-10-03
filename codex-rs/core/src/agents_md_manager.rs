//! Resolves global, thread, and repository instructions into one validated snapshot.
//! Refreshes are serialized; the state lock is never held while calling providers.

use crate::agents_md::LoadedAgentsMd;
use crate::agents_md::load_project_instructions;
use crate::agents_md::nested::InstructionTarget;
use crate::agents_md::nested::NestedAgentsMd;
use crate::agents_md::nested::load_nested_instructions;
use crate::config::Config;
use crate::context::world_state::WorldStateSnapshot;
use crate::environment_selection::TurnEnvironmentSnapshot;
use codex_extension_api::Instructions;
use codex_extension_api::ThreadInstructionsProvider;
use codex_extension_api::UserInstructionsProvider;
use codex_protocol::config_types::TrustLevel;
use codex_protocol::error::CodexErr;
use codex_protocol::error::Result as CodexResult;
use codex_protocol::protocol::TurnEnvironmentSelection;
use codex_utils_string::approx_bytes_for_tokens;
use codex_utils_string::approx_tokens_from_byte_count;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio::sync::Semaphore;

mod discovery;

/// Owns instruction sources, refresh serialization, and the applied snapshot.
pub(crate) struct AgentsMdManager {
    refresh_lock: Semaphore,
    state: Mutex<AgentsMdState>,
}

/// Subagents inherit applied snapshots, plus thread providers that opt into sharing.
#[derive(Clone, Default)]
pub(crate) struct SessionInstructions {
    pub(crate) user: Option<Instructions>,
    pub(crate) thread: Option<Instructions>,
    pub(crate) user_provider: Option<Arc<dyn UserInstructionsProvider>>,
    pub(crate) thread_provider: Option<Arc<dyn ThreadInstructionsProvider>>,
}

struct AgentsMdState {
    instructions: SessionInstructions,
    cache: AgentsMdCache,
    nested_targets: Vec<InstructionTarget>,
    discovery_processes: HashMap<i64, discovery::DiscoveryCommand>,
    nested_revision: u64,
    nested_cache: Option<(NestedCacheKey, NestedAgentsMd)>,
}

#[derive(PartialEq)]
struct NestedCacheKey {
    turn_id: String,
    selections: Vec<TurnEnvironmentSelection>,
    trust: Option<TrustLevel>,
    max_bytes: usize,
    startup: Option<LoadedAgentsMd>,
    revision: u64,
}

#[derive(Default)]
struct AgentsMdCache {
    selections: Option<Vec<TurnEnvironmentSelection>>,
    active_project_trust_level: Option<TrustLevel>,
    loaded: Option<Arc<LoadedAgentsMd>>,
}

impl AgentsMdManager {
    pub(crate) fn new(mut instructions: SessionInstructions) -> Self {
        instructions.user = normalize_instructions(instructions.user);
        instructions.thread = normalize_instructions(instructions.thread);
        Self {
            refresh_lock: Semaphore::new(/*permits*/ 1),
            state: Mutex::new(AgentsMdState {
                instructions,
                cache: AgentsMdCache::default(),
                nested_targets: Vec::new(),
                discovery_processes: HashMap::new(),
                nested_revision: 0,
                nested_cache: None,
            }),
        }
    }

    /// Resolves and validates one coherent snapshot for startup or a request boundary.
    /// Providers own fetching and caching. Warnings survive validation failures;
    /// only validated snapshots replace the applied host instructions.
    #[tracing::instrument(name = "agents_md.refresh", skip_all)]
    pub(crate) async fn refresh(
        &self,
        config: &Config,
        environments: &TurnEnvironmentSnapshot,
    ) -> (CodexResult<Option<Arc<LoadedAgentsMd>>>, Vec<String>) {
        // Serialize overlapping captures without blocking reads of the applied snapshot.
        let Ok(_refresh_guard) = self.refresh_lock.acquire().await else {
            return (
                Err(CodexErr::Fatal(
                    "instruction refresh semaphore closed".to_string(),
                )),
                Vec::new(),
            );
        };
        let selections = environments
            .turn_environments()
            .map(|environment| environment.selection.clone())
            .collect::<Vec<_>>();
        let active_project_trust_level = config.active_project.trust_level;
        let (mut instructions, cached, refresh_repository) = {
            let mut state = self.state.lock().await;
            let refresh_repository = state.cache.selections.as_ref() != Some(&selections)
                || state.cache.active_project_trust_level != active_project_trust_level;
            if refresh_repository {
                // Tightened read permissions must not leave inaccessible instructions visible,
                // even if discovery fails or the caller cancels the refresh.
                state.cache = AgentsMdCache::default();
            }
            (
                state.instructions.clone(),
                state.cache.loaded.clone(),
                refresh_repository,
            )
        };
        let mut warnings = Vec::new();
        if let Some(provider) = &instructions.user_provider {
            let loaded = provider.load_user_instructions().await;
            instructions.user = normalize_instructions(loaded.instructions);
            warnings.extend(loaded.warnings);
        }
        if let Some(provider) = &instructions.thread_provider {
            let loaded = provider.load_thread_instructions().await;
            instructions.thread = normalize_instructions(loaded.instructions);
            warnings.extend(loaded.warnings);
        }

        let result = async {
            if let Some(input) = &instructions.thread {
                validate_thread_instruction_size(input.text.len())?;
            }
            if !refresh_repository {
                let state = self.state.lock().await;
                if state.instructions.user == instructions.user
                    && state.instructions.thread == instructions.thread
                {
                    return Ok(cached);
                }
            }

            let loaded = if refresh_repository {
                load_project_instructions(config, /*user_instructions*/ None, environments)
                    .await?
                    .unwrap_or_default()
            } else {
                cached.as_deref().cloned().unwrap_or_default()
            }
            .with_instructions(instructions.user.clone(), instructions.thread.clone())
            .map(Arc::new);
            let mut state = self.state.lock().await;
            state.instructions = instructions;
            state.cache = AgentsMdCache {
                selections: Some(selections),
                active_project_trust_level,
                loaded: loaded.clone(),
            };
            Ok(loaded)
        }
        .await;
        (result, warnings)
    }

    pub(crate) async fn get_loaded(&self) -> Option<Arc<LoadedAgentsMd>> {
        self.state.lock().await.cache.loaded.clone()
    }

    /// Refresh after native filesystem tools or a new turn, not every sampling request.
    /// Visibility and content dedup belong to world state, including after window resets.
    pub(crate) async fn load_nested(
        &self,
        config: &Config,
        environments: &TurnEnvironmentSnapshot,
        startup: Option<&LoadedAgentsMd>,
        turn_id: &str,
    ) -> CodexResult<NestedAgentsMd> {
        let _guard = self
            .refresh_lock
            .acquire()
            .await
            .map_err(|_| CodexErr::Fatal("instruction refresh semaphore closed".to_string()))?;
        let (key, targets) = {
            let state = self.state.lock().await;
            let key = NestedCacheKey {
                turn_id: turn_id.to_string(),
                selections: environments
                    .turn_environments()
                    .map(|environment| environment.selection.clone())
                    .collect(),
                trust: config.active_project.trust_level,
                max_bytes: config.project_doc_max_bytes,
                startup: startup.cloned(),
                revision: state.nested_revision,
            };
            if let Some((previous, loaded)) = &state.nested_cache
                && previous == &key
            {
                return Ok(loaded.clone());
            }
            (key, state.nested_targets.clone())
        };
        let loaded = load_nested_instructions(config, environments, startup, targets).await?;
        self.state.lock().await.nested_cache = Some((key, loaded.clone()));
        Ok(loaded)
    }

    /// Restore discovery scope, not stale file contents, from the native replay baseline.
    pub(crate) async fn restore_nested_targets(&self, snapshot: &WorldStateSnapshot) {
        let mut sections = snapshot.clone().into_object();
        let Some(nested) = sections.remove("nested_agents_md") else {
            return;
        };
        match serde_json::from_value::<NestedAgentsMd>(nested) {
            Ok(nested) => {
                let mut state = self.state.lock().await;
                state.nested_targets = nested.targets;
                state.nested_cache = None;
            }
            Err(error) => {
                tracing::warn!(%error, "failed to restore nested instruction discovery scope")
            }
        }
    }

    pub(crate) async fn inherited_instructions(&self) -> SessionInstructions {
        let state = self.state.lock().await;
        SessionInstructions {
            user: state.instructions.user.clone(),
            thread: state.instructions.thread.clone(),
            thread_provider: state
                .instructions
                .thread_provider
                .clone()
                .filter(|provider| provider.share_with_subagents()),
            ..Default::default()
        }
    }
}

// Bound the new host-provided contribution independently of project_doc_max_bytes,
// which controls repository discovery. Existing global and combined instruction
// size policy is unchanged; reject oversized thread input rather than truncate it.
const MAX_THREAD_INSTRUCTIONS_TOKENS: usize = 10_000;

fn validate_thread_instruction_size(bytes: usize) -> CodexResult<()> {
    if bytes > approx_bytes_for_tokens(MAX_THREAD_INSTRUCTIONS_TOKENS) {
        let estimated_tokens = approx_tokens_from_byte_count(bytes);
        return Err(CodexErr::InvalidRequest(format!(
            "thread instructions exceed the limit of {MAX_THREAD_INSTRUCTIONS_TOKENS} estimated tokens ({estimated_tokens} estimated tokens provided)"
        )));
    }
    Ok(())
}

fn normalize_instructions(instructions: Option<Instructions>) -> Option<Instructions> {
    instructions.filter(|instructions| !instructions.text.trim().is_empty())
}
