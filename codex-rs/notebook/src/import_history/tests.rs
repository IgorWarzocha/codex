use super::*;

#[test]
fn inventory_recognizes_only_exact_version_static_import_literals() {
    // These are Pi's scanner inputs, including escaped literals and lexical decoys.
    let source = r#"
import plain from "npm:alpha@1.2.3";
export { default } from 'npm:@scope/bravo@2.0.0-beta.1+build/subpath';
await import(/* comment */ `npm:charlie@3.4.5`);
await import("npm:delta@2.0.\x30");
await import("npm:echo@1.0.\u0030");
await import('npm:foxtrot@1.0.\u{30}');
await import("npm:alpha@1.2.3");
await import('npm:unpinned'); await import('npm:range@^1.2.3');
await import('npm:tag@latest'); await import('npm:partial@1.2');
await import(`npm:${name}@1.0.0`);
// await import('npm:comment@1.0.0');
/* import 'npm:block@1.0.0' */
const text = "import('npm:string@1.0.0')";
const pattern = /import\('npm:regex@1.0.0'\)/;
const klass = /["']npm:regex-class@1.0.0/;
tools.import('npm:method@1.0.0');
const ordinary = 'npm:value@1.0.0';
"#;
    assert_eq!(
        source::extract(source)
            .unwrap()
            .into_iter()
            .collect::<Vec<_>>(),
        [
            "npm:@scope/bravo@2.0.0-beta.1+build/subpath",
            "npm:alpha@1.2.3",
            "npm:charlie@3.4.5",
            "npm:delta@2.0.0",
            "npm:echo@1.0.0",
            "npm:foxtrot@1.0.0",
        ]
    );
    assert!(
        !source::exact_specifier(&format!("npm:{}@1.0.0", "x".repeat(MAX_SPECIFIER_BYTES)))
            .unwrap()
    );
    assert!(
        source::extract("import 'npm:unfinished@1.2.3")
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn project_inventory_merges_forks_and_rejects_untrusted_manifests() {
    let home = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    std::fs::create_dir(project.path().join(".git")).unwrap();
    let subdir = project.path().join("subdir");
    std::fs::create_dir(&subdir).unwrap();
    let paths = Paths::new(home.path().to_path_buf(), project.path(), "a").unwrap();
    let a = ImportHistory::new(paths.clone());
    let b = ImportHistory::new(Paths::new(home.path().to_path_buf(), &subdir, "b").unwrap());
    assert!(a.read().await.unwrap().is_empty());
    let (a_result, b_result) = tokio::join!(
        a.record("await import('npm:alpha@1.2.3')"),
        b.record("await import('npm:bravo@2.0.0')")
    );
    a_result.unwrap();
    b_result.unwrap();
    assert_eq!(
        a.read().await.unwrap(),
        ["npm:alpha@1.2.3", "npm:bravo@2.0.0"]
    );
    assert_eq!(b.read().await.unwrap(), a.read().await.unwrap());
    let other_project = tempfile::tempdir().unwrap();
    let other = ImportHistory::new(
        Paths::new(home.path().to_path_buf(), other_project.path(), "a").unwrap(),
    );
    assert!(other.read().await.unwrap().is_empty());
    let manifest = paths.directory.join("npm-imports.json");
    let wrong = Manifest {
        schema: 1,
        project: other_project.path().to_path_buf(),
        imports: BTreeSet::new(),
    };
    std::fs::write(&manifest, serde_json::to_vec(&wrong).unwrap()).unwrap();
    assert!(a.read().await.is_err());
    assert!(a.record("import 'npm:charlie@3.0.0'").await.is_err());
    // A cell without imports never overwrites a corrupt inventory.
    assert_eq!(a.record("text(42)").await.unwrap(), None);
    std::fs::write(&manifest, vec![b' '; MAX_FILE_BYTES + 1]).unwrap();
    assert!(a.read().await.unwrap_err().contains("too large"));
}

#[test]
fn startup_inventory_is_bounded_and_requests_approval_without_claiming_it() {
    assert_eq!(
        notice(&[]),
        "Available npm imports in this notebook: none. Ask before adding another, then use an exact-version npm: specifier"
    );
    let imports: Vec<_> = (0..MAX_IMPORTS)
        .map(|n| format!("npm:{}{n}@1.0.0", "x".repeat(100)))
        .collect();
    let message = notice(&imports);
    assert!(message.len() < MAX_LIST_BYTES + 200);
    assert!(message.contains(&imports[0]));
    assert!(message.contains(" more. Ask before adding another"));
    assert!(!message.contains(imports.last().unwrap()));
}
