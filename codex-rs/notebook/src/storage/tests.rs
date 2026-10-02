use std::error::Error;
use std::fs;
use std::path::Path;
use std::path::PathBuf;
use std::process::Child;
use std::process::Command;
use std::time::Duration;
use std::time::Instant;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use serde_json::Value;
use serde_json::json;
use tempfile::TempDir;

use super::Store;
use super::files;

type TestResult = Result<(), Box<dyn Error>>;

struct Disk {
    _temporary: TempDir,
    home: PathBuf,
    project: PathBuf,
}

impl Disk {
    fn new() -> Result<Self, Box<dyn Error>> {
        let temporary = tempfile::tempdir()?;
        let home = temporary.path().join("home");
        let project = temporary.path().join("project");
        fs::create_dir_all(project.join(".git"))?;
        fs::create_dir_all(project.join("nested"))?;
        Ok(Self {
            _temporary: temporary,
            home,
            project,
        })
    }

    fn store(&self, thread: &str) -> Result<Store, String> {
        Store::new(self.home.clone(), &self.project, thread)
    }
}

fn entry(name: &str, bytes: &str) -> Value {
    json!({ "name": name, "kind": "value", "data": STANDARD.encode(bytes), "length": bytes.len() })
}

fn snapshot(mut entries: Vec<Value>) -> Value {
    entries.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));
    json!({ "deno": "old-deno-provenance", "v8": "old-v8-provenance", "entries": entries, "skipped": [] })
}

fn binding<'a>(snapshot: &'a Value, name: &str) -> Option<&'a Value> {
    snapshot["entries"]
        .as_array()?
        .iter()
        .find(|entry| entry["name"] == name)
}

#[tokio::test]
async fn disk_forks_preserve_concurrent_additions_and_private_conflicts_across_restart()
-> TestResult {
    let disk = Disk::new()?;
    let initial = snapshot(vec![entry("shared", "initial")]);
    disk.store("seed")?.checkpoint(&initial, &initial).await?;
    let mut left = Store::new(disk.home.clone(), &disk.project.join("nested"), "left")?;
    let mut right = disk.store("right")?;
    assert_eq!(left.load_project().await?, Some(initial.clone()));
    assert_eq!(right.load_project().await?, Some(initial));
    let left_project = snapshot(vec![entry("shared", "left"), entry("leftAddition", "a")]);
    left.checkpoint(&left_project, &left_project).await?;
    let right_project = snapshot(vec![entry("shared", "right"), entry("rightAddition", "b")]);
    let private = snapshot(vec![
        entry("shared", "right"),
        entry("rightAddition", "b"),
        entry("lexicalSecret", "private"),
    ]);
    let details = right.checkpoint(&private, &right_project).await?;
    assert_eq!(details["conflicts"], json!(["shared"]));
    let shared = right.load_project().await?.ok_or("missing project")?;
    assert_eq!(binding(&shared, "shared"), Some(&entry("shared", "left")));
    assert!(binding(&shared, "leftAddition").is_some());
    assert!(binding(&shared, "rightAddition").is_some());
    assert!(binding(&shared, "lexicalSecret").is_none());
    assert_eq!(right.load_session().await?, Some(private.clone()));
    assert!(
        left.load_session()
            .await?
            .is_some_and(|snapshot| binding(&snapshot, "rightAddition").is_none())
    );

    // An unchanged losing fork is not a fresh write, in-process or after restart.
    assert_eq!(
        right.checkpoint(&private, &right_project).await?["conflicts"],
        json!([])
    );
    drop(right);
    let mut restarted = disk.store("right")?;
    assert_eq!(restarted.load_project().await?, Some(shared.clone()));
    assert_eq!(restarted.load_session().await?, Some(private.clone()));
    assert_eq!(
        restarted.checkpoint(&private, &right_project).await?["conflicts"],
        json!([])
    );
    assert_eq!(restarted.load_project().await?, Some(shared));
    assert!(
        disk.store("unrelated-thread")?
            .load_session()
            .await?
            .is_none()
    );
    Ok(())
}

#[tokio::test]
async fn skipped_values_are_not_deletions_and_broken_hook_unpin_is_capture_free() -> TestResult {
    let disk = Disk::new()?;
    let mut hook = entry("startupHook", "() => { throw Error('broken startup'); }");
    hook["kind"] = json!("function");
    hook["pinned"] = json!(true);
    hook["hook"] = json!("startup");
    let initial = snapshot(vec![hook, entry("value", "initial")]);
    let mut owner = disk.store("owner")?;
    owner.checkpoint(&initial, &initial).await?;
    let mut other = disk.store("other")?;
    other.load_project().await?;
    let mut changed = initial.clone();
    changed["entries"][1] = entry("value", "concurrent");
    owner.checkpoint(&changed, &changed).await?;
    let mut capture_failed = snapshot(vec![]);
    capture_failed["skipped"] = json!([{ "name": "value", "reason": "cannot serialize" }]);
    let details = other.checkpoint(&capture_failed, &capture_failed).await?;
    assert_eq!(details["conflicts"], json!(["startupHook"]));
    let project = owner.load_project().await?.ok_or("missing project")?;
    assert_eq!(
        binding(&project, "value"),
        Some(&entry("value", "concurrent"))
    );
    assert!(binding(&project, "startupHook").is_some());

    // Recovery operates solely on disk, not by evaluating the throwing function.
    let recovery = owner.unpin(&["startupHook".to_owned()]).await?;
    assert_eq!(recovery["unpinned"], json!(["startupHook"]));
    for restored in [owner.load_project().await?, owner.load_session().await?] {
        let restored = restored.ok_or("missing restore")?;
        let hook = binding(&restored, "startupHook").ok_or("missing hook")?;
        assert!(hook.get("pinned").is_none());
        assert!(hook.get("hook").is_none());
    }
    let deletion = snapshot(vec![entry("value", "concurrent")]);
    assert_eq!(
        owner.checkpoint(&deletion, &deletion).await?["conflicts"],
        json!([])
    );
    assert!(
        binding(
            &owner.load_project().await?.ok_or("missing project")?,
            "startupHook"
        )
        .is_none()
    );
    Ok(())
}

#[tokio::test]
async fn profiles_are_global_by_value_and_invalid_capture_cannot_replace_them() -> TestResult {
    let disk = Disk::new()?;
    let source = snapshot(vec![entry("saved", "bytes, not executable cells")]);
    let store = disk.store("thread")?;
    store.save_profile("My.Profile-1", &source).await?;
    let unrelated = disk._temporary.path().join("unrelated");
    fs::create_dir(&unrelated)?;
    let unrelated_store = Store::new(disk.home.clone(), &unrelated, "../arbitrary-thread")?;
    assert!(unrelated_store.load_project().await?.is_none());
    assert_eq!(unrelated_store.load_profile("My.Profile-1").await?, source);
    assert_eq!(
        unrelated_store.list_profiles(Some("my.?rofile*")).await?["profiles"][0]["name"],
        "My.Profile-1"
    );
    assert_eq!(
        unrelated_store.list_profiles(Some("NoMatch*")).await?["profiles"],
        json!([])
    );
    assert!(store.save_profile("../escape", &source).await.is_err());
    let mut invalid_cases = Vec::new();
    let mut replay = source.clone();
    replay["cells"] = json!(["throw Error('must never replay')"]);
    invalid_cases.push(replay);
    let mut length = source.clone();
    length["entries"][0]["length"] = json!(1);
    invalid_cases.push(length);
    let mut base64 = source.clone();
    base64["entries"][0]["data"] = json!("not base64!");
    invalid_cases.push(base64);
    invalid_cases.push(snapshot(vec![entry("same", "a"), entry("same", "b")]));
    let mut hook = source.clone();
    hook["entries"][0]["hook"] = json!("startup");
    invalid_cases.push(hook);
    let mut metadata = source.clone();
    metadata["entries"][0]["description"] = json!("invalid\nmultiline");
    invalid_cases.push(metadata);
    for invalid in invalid_cases {
        assert!(store.save_profile("My.Profile-1", &invalid).await.is_err());
        assert_eq!(store.load_profile("My.Profile-1").await?, source);
    }
    Ok(())
}

#[tokio::test]
async fn corrupt_files_fail_closed_and_reset_removes_only_private_session() -> TestResult {
    let disk = Disk::new()?;
    let source = snapshot(vec![entry("project", "durable")]);
    let mut store = disk.store("thread")?;
    store.checkpoint(&source, &source).await?;
    let paths = files::Paths::new(disk.home.clone(), &disk.project, "thread")?;
    let project_path = paths.directory.join("project.json");
    let saved_project = fs::read(&project_path)?;
    let saved_session = fs::read(&paths.session)?;
    fs::write(&project_path, b"{ corrupt project")?;
    assert!(store.load_project().await.is_err());
    assert!(store.checkpoint(&source, &source).await.is_err());
    assert_eq!(fs::read(&paths.session)?, saved_session);
    assert_eq!(fs::read(&project_path)?, b"{ corrupt project");
    fs::write(&project_path, &saved_project)?;
    fs::write(&paths.session, b"{ corrupt session")?;
    assert!(store.load_session().await.is_err());
    store.reset_session().await?;
    store.reset_session().await?;
    assert!(store.load_session().await?.is_none());
    assert_eq!(fs::read(&project_path)?, saved_project);
    assert_eq!(store.load_project().await?, Some(source.clone()));
    store.save_profile("corrupt", &source).await?;
    fs::write(paths.profiles.join("corrupt/profile.json"), b"{}")?;
    assert!(store.load_profile("corrupt").await.is_err());
    assert!(store.list_profiles(None).await.is_err());
    Ok(())
}

#[cfg(unix)]
#[tokio::test]
async fn private_modes_and_symlink_boundaries() -> TestResult {
    use std::os::unix::fs::PermissionsExt;
    use std::os::unix::fs::symlink;
    let disk = Disk::new()?;
    let source = snapshot(vec![entry("private", "secret")]);
    let mut store = disk.store("thread")?;
    store.checkpoint(&source, &snapshot(vec![])).await?;
    let paths = files::Paths::new(disk.home.clone(), &disk.project, "thread")?;
    assert_eq!(
        fs::metadata(&paths.session)?.permissions().mode() & 0o777,
        0o600
    );
    assert_eq!(
        fs::metadata(&paths.directory)?.permissions().mode() & 0o777,
        0o700
    );
    fs::remove_file(&paths.session)?;
    let outside = disk._temporary.path().join("outside");
    fs::write(&outside, b"keep me")?;
    symlink(&outside, &paths.session)?;
    assert!(store.load_session().await.is_err());
    assert!(store.reset_session().await.is_err());
    assert!(store.checkpoint(&source, &snapshot(vec![])).await.is_err());
    assert_eq!(fs::read(&outside)?, b"keep me");
    symlink(disk._temporary.path(), paths.profiles.join("linked"))?;
    assert!(store.save_profile("linked", &source).await.is_err());
    assert!(store.load_profile("linked").await.is_err());
    Ok(())
}

struct Writer(Child);

impl Drop for Writer {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn await_file(path: &Path) -> TestResult {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !path.exists() {
        if Instant::now() >= deadline {
            return Err(format!("Timed out waiting for {}", path.display()).into());
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    Ok(())
}

#[tokio::test]
async fn os_lock_serializes_real_processes_without_losing_additions() -> TestResult {
    let disk = Disk::new()?;
    let mut children = Vec::new();
    let barrier = disk._temporary.path().join("go");
    for name in ["first", "second"] {
        let ready = disk._temporary.path().join(format!("{name}-ready"));
        let attempted = disk._temporary.path().join(format!("{name}-attempted"));
        let config = json!({ "home": disk.home, "cwd": disk.project, "name": name, "ready": ready, "attempted": attempted, "go": barrier });
        children.push(Writer(
            Command::new(std::env::current_exe()?)
                .args([
                    "--exact",
                    "storage::tests::cross_process_writer",
                    "--ignored",
                    "--nocapture",
                ])
                .env("CODEX_NOTEBOOK_STORAGE_CHILD", config.to_string())
                .spawn()?,
        ));
        await_file(&ready)?;
    }
    let paths = files::Paths::new(disk.home.clone(), &disk.project, "observer")?;
    let held_lock = files::lock(&paths.directory)?;
    fs::write(&barrier, b"go")?;
    for name in ["first", "second"] {
        await_file(&disk._temporary.path().join(format!("{name}-attempted")))?;
    }
    std::thread::sleep(Duration::from_millis(150));
    assert!(
        !paths.directory.join("project.json").exists(),
        "writer bypassed the OS lock"
    );
    for child in &mut children {
        assert!(child.0.try_wait()?.is_none());
    }
    drop(held_lock);
    for child in &mut children {
        assert!(child.0.wait()?.success());
    }
    let project = disk
        .store("observer")?
        .load_project()
        .await?
        .ok_or("missing project")?;
    assert!(binding(&project, "first").is_some());
    assert!(binding(&project, "second").is_some());
    Ok(())
}

#[test]
#[ignore = "subprocess entry point for os_lock_serializes_real_processes_without_losing_additions"]
fn cross_process_writer() -> TestResult {
    let Ok(config) = std::env::var("CODEX_NOTEBOOK_STORAGE_CHILD") else {
        return Ok(());
    };
    let config: Value = serde_json::from_str(&config)?;
    let string = |key: &str| {
        config[key]
            .as_str()
            .ok_or("missing subprocess configuration")
    };
    let mut store = Store::new(
        PathBuf::from(string("home")?),
        Path::new(string("cwd")?),
        string("name")?,
    )?;
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?
        .block_on(async {
            store.load_project().await?;
            fs::write(string("ready")?, b"ready")?;
            await_file(Path::new(string("go")?))?;
            fs::write(string("attempted")?, b"attempted")?;
            let candidate = snapshot(vec![entry(string("name")?, "child value")]);
            store.checkpoint(&candidate, &candidate).await?;
            Ok(())
        })
}
