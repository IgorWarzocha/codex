use super::*;

#[test]
fn completed_cells_survive_reopen_and_rotation_without_replay() {
    let home = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let journal =
        Journal::with_budget(home.path(), project.path(), "thread/../../a", 8192).unwrap();
    journal.begin("first", "const retained = 1;").unwrap();
    assert_eq!(
        journal.read().unwrap()["cells"][0]["metadata"]["codex"]["status"],
        "running"
    );
    let items = [
        FunctionCallOutputContentItem::InputText {
            text: "printed".into(),
        },
        FunctionCallOutputContentItem::InputImage {
            image_url: "data:image/png;base64,aGVsbG8=".into(),
            detail: None,
        },
    ];
    journal
        .append(
            "first",
            "const retained = 1;",
            "error",
            Some("boom\ntrace"),
            &items,
        )
        .unwrap();
    let reopened =
        Journal::with_budget(home.path(), project.path(), "thread/../../a", 8192).unwrap();
    let document = reopened.read().unwrap();
    assert_eq!(document["cells"].as_array().unwrap().len(), 1);
    assert_eq!(document["cells"][0]["outputs"][0]["text"], "printed");
    assert_eq!(
        document["cells"][0]["outputs"][1]["data"]["image/png"],
        "aGVsbG8="
    );
    assert_eq!(
        document["cells"][0]["outputs"][2]["ename"],
        "NotebookCellError"
    );
    for index in 0..16 {
        reopened
            .append(&format!("cell-{index}"), &"x".repeat(1500), "ok", None, &[])
            .unwrap();
    }
    assert!(fs::metadata(&journal.path).unwrap().len() <= 8192);
    let previous = journal.path.with_extension("previous.ipynb");
    assert!(fs::metadata(previous).unwrap().len() <= 8192);
    assert!(journal.code_cells().unwrap().len() < 16);
    assert_ne!(
        journal.path,
        Journal::new(home.path(), project.path(), "other")
            .unwrap()
            .path
    );
}

#[test]
fn invalid_or_oversized_history_never_gets_silently_replaced() {
    let home = tempfile::tempdir().unwrap();
    let journal = Journal::with_budget(home.path(), home.path(), "thread", 8192).unwrap();
    journal
        .append("cell", "const x = 1;", "ok", None, &[])
        .unwrap();
    let before = fs::read(&journal.path).unwrap();
    assert!(journal.begin("too-big", &"x".repeat(8192)).is_err());
    assert_eq!(fs::read(&journal.path).unwrap(), before);
    let mut invalid = journal.read().unwrap();
    invalid["cells"][0]["source"] = json!([1]);
    fs::write(&journal.path, invalid.to_string()).unwrap();
    assert!(journal.code_cells().is_err());
    assert!(journal.append("next", "valid", "ok", None, &[]).is_err());
    assert_eq!(
        fs::read_to_string(&journal.path).unwrap(),
        invalid.to_string()
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        use std::os::unix::fs::symlink;
        for directory in journal.path.parent().unwrap().ancestors().take(3) {
            assert_eq!(
                fs::metadata(directory).unwrap().permissions().mode() & 0o777,
                0o700
            );
        }
        fs::remove_file(&journal.path).unwrap();
        let target = home.path().join("outside");
        fs::write(&target, &before).unwrap();
        // Both live and dangling file symlinks must fail, never look like empty history.
        let missing = home.path().join("missing");
        for target in [&target, &missing] {
            symlink(target, &journal.path).unwrap();
            assert!(journal.code_cells().is_err());
            assert!(journal.begin("next", "source").is_err());
            fs::remove_file(&journal.path).unwrap();
        }
        let previous = journal.path.with_extension("previous.ipynb");
        symlink(&target, &previous).unwrap();
        assert!(journal.code_cells().is_err());
        assert!(journal.begin("next", "source").is_err());
        fs::remove_file(previous).unwrap();
        // Check ancestors on every access, including replacement after Journal construction.
        let directory = journal.path.parent().unwrap();
        fs::remove_dir(directory).unwrap();
        symlink(home.path(), directory).unwrap();
        assert!(journal.code_cells().is_err());
        assert!(journal.begin("next", "source").is_err());
        fs::remove_file(directory).unwrap();
        let home_alias = tempfile::tempdir().unwrap();
        let alias = home_alias.path().join("home-link");
        symlink(home.path(), &alias).unwrap();
        let from_alias = Journal::new(&alias, home.path(), "thread").unwrap();
        assert_eq!(from_alias.path, journal.path);
        assert!(from_alias.code_cells().unwrap().is_empty());
        assert!(
            !directory.exists(),
            "read-only diagnostics created journal directories"
        );
        assert_eq!(fs::read(target).unwrap(), before);
    }
}

#[test]
fn output_budget_preserves_utf8_and_standard_error_shape() {
    let home = tempfile::tempdir().unwrap();
    let journal = Journal::with_budget(home.path(), home.path(), "thread", 8192).unwrap();
    let items = [FunctionCallOutputContentItem::InputText {
        text: "😀\u{0000}".repeat(10_000),
    }];
    journal
        .append(
            "cell",
            "throw new Error('x')",
            "error",
            Some(&"é".repeat(10_000)),
            &items,
        )
        .unwrap();
    let document = journal.read().unwrap();
    assert!(fs::metadata(&journal.path).unwrap().len() <= 8192);
    let outputs = document["cells"][0]["outputs"].as_array().unwrap();
    assert_eq!(outputs.last().unwrap()["output_type"], "error");
    assert!(outputs.iter().any(|item| item["name"] == "stderr"));
    for size in 0..20 {
        assert!(bound_text("😀😀😀😀😀😀", size).len() <= size);
    }
}
