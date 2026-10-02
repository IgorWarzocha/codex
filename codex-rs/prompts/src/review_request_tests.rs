use super::*;
use pretty_assertions::assert_eq;

#[test]
fn review_prompt_template_renders_base_branch_backup_variant() {
    assert_eq!(
        render_review_prompt(&BASE_BRANCH_PROMPT_BACKUP_TEMPLATE, [("branch", "main")]),
        "Review changes against 'main'. Merge base with main's upstream: `git merge-base HEAD \"$(git rev-parse --abbrev-ref \"main@{upstream}\")\"`. Then `git diff` against that SHA. Prioritized, actionable findings"
    );
}

#[test]
fn review_prompt_template_renders_base_branch_variant() {
    assert_eq!(
        render_review_prompt(
            &BASE_BRANCH_PROMPT_TEMPLATE,
            [("base_branch", "main"), ("merge_base_sha", "abc123")]
        ),
        "Review changes against 'main'. Merge base: abc123. Diff: `git diff abc123`. Prioritized, actionable findings"
    );
}

#[test]
fn review_prompt_template_renders_commit_variant() {
    assert_eq!(
        review_prompt(
            &ReviewTarget::Commit {
                sha: "deadbeef".to_string(),
                title: None,
            },
            &AbsolutePathBuf::current_dir().expect("cwd"),
        )
        .expect("commit prompt should render"),
        "Review commit deadbeef. Prioritized, actionable findings"
    );
}

#[test]
fn review_prompt_template_renders_commit_variant_with_title() {
    assert_eq!(
        review_prompt(
            &ReviewTarget::Commit {
                sha: "deadbeef".to_string(),
                title: Some("Fix bug".to_string()),
            },
            &AbsolutePathBuf::current_dir().expect("cwd"),
        )
        .expect("commit prompt should render"),
        "Review commit deadbeef (\"Fix bug\"). Prioritized, actionable findings"
    );
}
