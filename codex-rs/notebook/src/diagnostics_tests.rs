use super::*;

#[test]
fn static_errors_keep_cell_locations_and_only_runtime_false_positives_are_filtered() {
    let cell = CodeCell {
        id: "cell".into(),
        index: 2,
        source: "globalThis.dynamic\ntext(missing)".into(),
    };
    let range = json!({"start":{"line":0,"character":11},"end":{"line":0,"character":18}});
    let report = json!({"items":[
        {"range":range,"code":7017,"message":"Element implicitly has an any type","severity":1},
        {"range":range,"code":2304,"message":"Cannot find name 'text'.","severity":1},
        {"range":range,"code":2304,"message":"Cannot find name 'retained'.","severity":1},
        {"range":{"start":{"line":1,"character":5},"end":{"line":1,"character":12}},"code":2304,"message":"Cannot find name 'missing'.","severity":1}
    ]});
    let bindings = HashSet::from(["text", "retained"]);
    let mut diagnostics = Vec::new();
    parse_report(&report, &cell, &bindings, &mut diagnostics).unwrap();
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(diagnostics[0]["cellIndex"], 2);
    assert_eq!(diagnostics[0]["line"], 2);
    assert_eq!(diagnostics[0]["column"], 6);
    let result = format_diagnostics("/history.ipynb", 3, "ready", diagnostics);
    assert_eq!(result.details["runtime"]["state"], "ready");
    assert_eq!(result.details["diagnosticCount"], 1);
    assert!(result.message.contains("Historical static"));
    assert!(
        parse_report(
            &json!({"items":[{"range":range,"message":7}]}),
            &cell,
            &bindings,
            &mut Vec::new()
        )
        .is_err()
    );
}

#[test]
fn grouping_and_response_bounds_preserve_counts_and_health() {
    let mut diagnostics = Vec::new();
    for i in 0..100 {
        diagnostics.push(json!({"cellId":format!("cell-{i}"),"cellIndex":i,"line":1,"column":2,"endLine":1,"endColumn":3,"severity":"error","code":2451,"name":"x","message":"Cannot redeclare block-scoped variable 'x'."}));
    }
    let result = format_diagnostics("/history.ipynb", 100, "invalidated", diagnostics.clone());
    assert_eq!(result.details["diagnosticGroups"][0]["count"], 100);
    assert_eq!(
        result.details["diagnosticGroups"][0]["samples"]
            .as_array()
            .unwrap()
            .len(),
        3
    );
    for (i, item) in diagnostics.iter_mut().enumerate() {
        item["message"] = json!(format!("{i}{}", "😀".repeat(500)));
    }
    let result = format_diagnostics("/history.ipynb", 100, "invalidated", diagnostics);
    assert!(result.message.len() <= RESPONSE_BUDGET);
    assert!(result.details.to_string().len() <= RESPONSE_BUDGET);
    assert!(result.details["omittedGroups"].as_u64().unwrap() > 0);
    assert_eq!(result.details["diagnosticCount"], 100);
    assert_eq!(result.details["runtime"]["state"], "invalidated");
}

#[tokio::test]
#[ignore = "requires a local Deno executable"]
async fn real_deno_reports_notebook_static_errors_without_executing_cells() {
    let home = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let journal = Journal::new(home.path(), cwd.path(), "thread").unwrap();
    journal
        .append("first", "const fromPreviousCell = 42;", "ok", None, &[])
        .unwrap();
    journal.append("second","text(fromPreviousCell);\nconst wrong: string = fromPreviousCell;\nDeno.writeTextFileSync('must-not-exist', 'executed');","error",Some("old runtime failure"),&[]).unwrap();
    let deno = std::env::var_os("DENO_PROGRAM").unwrap_or_else(|| "deno".into());
    let result = diagnostics_with_runtime(
        Path::new(&deno),
        cwd.path(),
        home.path(),
        "thread",
        "ready",
        &[],
    )
    .await
    .unwrap();
    assert!(result.details.get("error").is_none(), "{}", result.message);
    let groups = result.details["diagnosticGroups"].as_array().unwrap();
    assert!(
        groups.iter().any(|g| g["code"] == 2322),
        "{}",
        result.message
    );
    assert!(
        !groups.iter().any(|g| g["code"] == 2304),
        "{}",
        result.message
    );
    assert_eq!(result.details["runtime"]["state"], "ready");
    assert!(!cwd.path().join("must-not-exist").exists());
    assert!(
        groups
            .iter()
            .any(|g| g["samples"][0]["cellId"] == "second" && g["samples"][0]["line"] == 2)
    );
}
