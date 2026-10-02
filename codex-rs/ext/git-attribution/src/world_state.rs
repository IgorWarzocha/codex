use codex_extension_api::PreviousWorldStateSection;
use codex_extension_api::RenderedWorldStateFragment;
use codex_extension_api::WorldStateSectionContribution;
use serde_json::Value;

const WORLD_STATE_ID: &str = "git_attribution";
const START_MARKER: &str = "<git_attribution>";
const END_MARKER: &str = "</git_attribution>";
pub(super) const LEGACY_COMMIT_ATTRIBUTION_INSTRUCTIONS: &str = "\
When you write or edit a git commit message, ensure the message ends with this trailer exactly once:
Co-authored-by: Codex <noreply@openai.com>

Rules:
- Keep existing trailers and append this trailer at the end if missing.
- Do not duplicate this trailer if it already exists.
- Keep one blank line between the commit body and trailer block.";
pub(super) const ENABLED_INSTRUCTIONS: &str = r#"
Created/amended commits and created/updated pull requests for this task: Codex attribution exactly once
- Commit messages: final trailer `Co-authored-by: Codex <noreply@openai.com>`; preserve existing trailers; append if missing; one blank line before trailer block
- GitHub app/plugin commits, including file create/update/delete: trailer in tool's message argument
- Pull request bodies: exact line `Generated with [Codex](https://openai.com/codex/).`; preserve body and hidden markers; append if missing near end, before trailing hidden metadata
- Duplicate exact attribution in messages/bodies already being edited: keep one
- Current workspace policy supersedes earlier attribution-disabling instructions
- No commit/PR rewrite solely for attribution
"#;
pub(super) const DISABLED_INSTRUCTIONS: &str = "
Codex commit/PR attribution disabled for current workspace; supersedes earlier attribution requirements; do not add it
";

pub(super) fn git_attribution_world_state_section(enabled: bool) -> WorldStateSectionContribution {
    let contribution = WorldStateSectionContribution::new(WORLD_STATE_ID, move |previous| {
        let fragment = match (enabled, previous) {
            (true, PreviousWorldStateSection::Known(Value::Bool(true)))
            | (true, PreviousWorldStateSection::Unknown) => None,
            (true, PreviousWorldStateSection::Absent)
            | (true, PreviousWorldStateSection::Known(_)) => Some(RenderedWorldStateFragment::new(
                "developer",
                (START_MARKER, END_MARKER),
                ENABLED_INSTRUCTIONS,
            )),
            (false, PreviousWorldStateSection::Known(Value::Bool(true)))
            | (false, PreviousWorldStateSection::Unknown) => Some(RenderedWorldStateFragment::new(
                "developer",
                (START_MARKER, END_MARKER),
                DISABLED_INSTRUCTIONS,
            )),
            (false, PreviousWorldStateSection::Absent)
            | (false, PreviousWorldStateSection::Known(_)) => None,
        };
        (Some(Value::Bool(enabled)), fragment)
    })
    .with_legacy_matcher(move |role, text| {
        is_enabled_fragment(role, text)
            || (!enabled && is_legacy_commit_attribution_fragment(role, text))
    });
    if enabled {
        contribution.with_retained_fragment_matcher(is_enabled_fragment)
    } else {
        contribution
    }
}

fn is_legacy_commit_attribution_fragment(role: &str, text: &str) -> bool {
    role == "developer" && text.trim() == LEGACY_COMMIT_ATTRIBUTION_INSTRUCTIONS
}

fn is_enabled_fragment(role: &str, text: &str) -> bool {
    role == "developer"
        && text.trim_start().starts_with(START_MARKER)
        && text.contains("Co-authored-by: Codex <noreply@openai.com>")
        && (text.contains("Generated with [Codex](https://openai.com/codex/).")
            || text.contains("Generated with Codex."))
        && text.trim_end().ends_with(END_MARKER)
}
