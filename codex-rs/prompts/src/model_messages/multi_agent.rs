//! Resolves multi-agent role bases and mode text while preserving catalog provenance.
//! Consumers own role assembly, suppression, and effort-dependent mode selection.

use super::ResolvedMessage;
use codex_protocol::openai_models::MultiAgentMessages;

const DEFAULT_MULTI_AGENT_V2_ROOT_AGENT_USAGE_HINT_TEXT: &str = "You are `/root`, the primary agent. Own the result and coordinate delegated work. Agent messages arrive in analysis with type, recipient, sender, and payload. `send_message` contacts a running agent without starting a turn. `followup_task` starts one.";
const DEFAULT_MULTI_AGENT_V2_SUBAGENT_USAGE_HINT_TEXT: &str = "Complete the assigned task. Your final answer is delivered to your parent. Agent messages arrive in analysis with type, recipient, sender, and payload. `send_message` contacts a running agent without starting a turn. `followup_task` starts one.";
const EXPLICIT_REQUEST_ONLY_MULTI_AGENT_MODE_TEXT: &str = "This replaces earlier proactive-delegation guidance. Spawn agents only when the user or applicable AGENTS.md or skill instructions explicitly request delegation.";
const PROACTIVE_MULTI_AGENT_MODE_TEXT: &str = "Proactive delegation replaces earlier explicit-request-only guidance until a later developer mode message changes it. Delegate parallel work when it saves time or improves quality. User requests override this hint.";

/// Model-only role bases and mode alternatives for runtime selection.
#[derive(Debug, Clone, Copy)]
pub struct ResolvedMultiAgentMessages<'a> {
    pub root: ResolvedMessage<'a>,
    pub subagent: ResolvedMessage<'a>,
    pub explicit: ResolvedMessage<'a>,
    pub proactive: ResolvedMessage<'a>,
    /// A supplied hint overrides both effort-specific modes, including when empty.
    pub hint: Option<&'a str>,
}

impl<'a> ResolvedMultiAgentMessages<'a> {
    pub(crate) fn new(messages: Option<&'a MultiAgentMessages>) -> Self {
        let role = messages.and_then(|messages| messages.role.as_ref());
        let mode = messages.and_then(|messages| messages.mode.as_ref());
        Self {
            root: ResolvedMessage::new(
                role.and_then(|role| role.root.as_deref()),
                DEFAULT_MULTI_AGENT_V2_ROOT_AGENT_USAGE_HINT_TEXT,
            ),
            subagent: ResolvedMessage::new(
                role.and_then(|role| role.subagent.as_deref()),
                DEFAULT_MULTI_AGENT_V2_SUBAGENT_USAGE_HINT_TEXT,
            ),
            explicit: ResolvedMessage::new(
                mode.and_then(|mode| mode.explicit.as_deref()),
                EXPLICIT_REQUEST_ONLY_MULTI_AGENT_MODE_TEXT,
            ),
            proactive: ResolvedMessage::new(
                mode.and_then(|mode| mode.proactive.as_deref()),
                PROACTIVE_MULTI_AGENT_MODE_TEXT,
            ),
            hint: mode.and_then(|mode| mode.hint_text.as_deref()),
        }
    }
}
