//! Focused lifecycle acceptance against real Deno and the production snapshot format.

use std::collections::BTreeMap;
use std::path::Path;
use std::path::PathBuf;

use tempfile::TempDir;

use super::*;

struct Disk {
    _temporary: TempDir,
    home: PathBuf,
    source: PathBuf,
    project: PathBuf,
}

impl Disk {
    fn new() -> Self {
        let temporary = tempfile::tempdir().unwrap();
        let home = temporary.path().join("home");
        let source = temporary.path().join("source");
        let project = temporary.path().join("project");
        std::fs::create_dir_all(source.join(".git")).unwrap();
        std::fs::create_dir_all(project.join(".git")).unwrap();
        Self {
            _temporary: temporary,
            home,
            source,
            project,
        }
    }

    fn store(&self, cwd: &Path, thread: &str) -> Store {
        Store::new(self.home.clone(), cwd, thread).unwrap()
    }

    fn profile_source(&self) -> Store {
        Store::for_profile_reads(
            self.home.clone(),
            &self.project,
            "profile-reader",
            PersistenceBudget::from_heap_mib(Some(512)),
        )
        .unwrap()
    }

    async fn start_at(&self, cwd: &Path, thread: &str) -> Lifecycle {
        start(cwd, Some(self.store(cwd, thread)), None)
            .await
            .unwrap()
    }

    async fn selected(
        &self,
        thread: &str,
        name: &str,
        ephemeral: bool,
    ) -> Result<Lifecycle, String> {
        start(
            &self.project,
            (!ephemeral).then(|| self.store(&self.project, thread)),
            Some((self.profile_source(), name.into())),
        )
        .await
    }

    async fn save_profile(&self) {
        let mut source = self.start_at(&self.source, "source").await;
        execute(
            &mut source,
            "globalThis.shared = 'profile'; globalThis.helper = (n) => n + 1; \
             helper.description = 'Add one'; helper.usage = 'helper(2)'; \
             globalThis.profileHook = () => { globalThis.unwantedHook = true; }; \
             globalThis.pending = Promise.resolve(42);",
        )
        .await;
        source
            .control(NotebookRequest::Pin {
                names: vec!["profileHook".into()],
                hook: Some(NotebookHook::Startup),
            })
            .await
            .unwrap();
        source
            .control(NotebookRequest::Save {
                name: "default".into(),
            })
            .await
            .unwrap();
        source.shutdown().await.unwrap();
    }

    fn profile_file(&self) -> PathBuf {
        self.home.join("notebook/profiles/default/profile.json")
    }
}

async fn start(
    cwd: &Path,
    store: Option<Store>,
    profile: Option<(Store, String)>,
) -> Result<Lifecycle, String> {
    let options = KernelOptions {
        deno: std::env::var_os("DENO_PROGRAM")
            .map(PathBuf::from)
            .unwrap_or_else(|| "deno".into()),
        cwd: Some(cwd.to_path_buf()),
        ..KernelOptions::default()
    };
    let bootstrap = include_str!("../bootstrap.js")
        .replace("__PLAIN_COMMAND_OUTPUT__", "false")
        .replace("__ENDPOINT__", "\"http://127.0.0.1:1\"")
        .replace("__CREDENTIAL__", "\"test\"");
    Lifecycle::start(options, None, bootstrap, store, profile, false).await
}

async fn execute(lifecycle: &mut Lifecycle, source: &str) {
    let result = lifecycle
        .kernel
        .as_mut()
        .unwrap()
        .execute(source)
        .await
        .unwrap();
    assert!(result_error(&result).is_none(), "{result:?}");
}

async fn read(lifecycle: &mut Lifecycle, expression: &str) -> Value {
    lifecycle.rpc_expression(expression).await.unwrap()
}

#[tokio::test]
#[ignore = "requires a Deno Jupyter executable, DENO_PROGRAM defaults to deno"]
async fn fresh_seed_is_private_captured_and_never_registers_profile_hooks() {
    let disk = Disk::new();
    disk.save_profile().await;
    let mut fresh = disk.selected("fresh", "default", false).await.unwrap();
    assert_eq!(
        read(
            &mut fresh,
            "[shared, helper(2), helper.description, helper.usage]"
        )
        .await,
        json!(["profile", 3, "Add one", "helper(2)"])
    );
    assert_eq!(
        read(&mut fresh, "[typeof unwantedHook, typeof pending]").await,
        json!(["undefined", "undefined"])
    );
    assert!(
        fresh.status["bindings"]
            .as_array()
            .unwrap()
            .iter()
            .all(|e| !pinned(e))
    );
    assert_eq!(
        fresh.status["defaultProfile"]["skipped"],
        json!([{"name":"pending","reason":"promise"}])
    );
    let status = fresh
        .control(NotebookRequest::Status { query: None })
        .await
        .unwrap();
    assert!(status.message.contains("pending"));
    assert!(status.message.contains("promise"));
    let store = disk.store(&disk.project, "fresh");
    assert_eq!(
        entries(&store.load_session().await.unwrap().unwrap()).len(),
        3
    );
    assert!(entries(&store.load_project().await.unwrap().unwrap()).is_empty());

    // Loading manually must still reject collisions rather than seed missing names.
    let error = fresh
        .control(NotebookRequest::Load {
            name: "default".into(),
        })
        .await
        .err()
        .unwrap();
    assert!(
        error.contains("profile conflicts with existing bindings"),
        "{error}"
    );
    fresh
        .control(NotebookRequest::Pin {
            names: vec!["helper".into()],
            hook: None,
        })
        .await
        .unwrap();
    assert_eq!(
        entries(&store.load_project().await.unwrap().unwrap())[0]["name"],
        "helper"
    );
    fresh.shutdown().await.unwrap();
}

#[tokio::test]
#[ignore = "requires a Deno Jupyter executable, DENO_PROGRAM defaults to deno"]
async fn any_project_or_bootstrap_collision_rejects_the_entire_profile_before_hooks() {
    let disk = Disk::new();
    disk.save_profile().await;
    // Include a bootstrap collision in an otherwise valid, real captured profile.
    let source = disk.store(&disk.source, "source");
    let mut snapshot = source.load_profile("default").await.unwrap();
    let mut tools_entry = entries(&snapshot)
        .into_iter()
        .find(|e| e["name"] == "shared")
        .unwrap();
    tools_entry["name"] = json!("tools");
    snapshot["entries"]
        .as_array_mut()
        .unwrap()
        .push(tools_entry);
    source.save_profile("default", &snapshot).await.unwrap();
    let mut owner = disk.start_at(&disk.project, "owner").await;
    execute(&mut owner, "globalThis.shared = 'project'; globalThis.onStart = () => { globalThis.observed = [shared, typeof helper]; };").await;
    owner
        .control(NotebookRequest::Pin {
            names: vec!["shared".into()],
            hook: None,
        })
        .await
        .unwrap();
    owner
        .control(NotebookRequest::Pin {
            names: vec!["onStart".into()],
            hook: Some(NotebookHook::Startup),
        })
        .await
        .unwrap();
    owner.shutdown().await.unwrap();

    let mut fresh = disk.selected("fresh", "default", false).await.unwrap();
    assert_eq!(
        read(&mut fresh, "observed").await,
        json!(["project", "undefined"])
    );
    assert_eq!(read(&mut fresh, "typeof tools").await, "object");
    assert_eq!(fresh.status["defaultProfile"]["applied"], false);
    let collisions = fresh.status["defaultProfile"]["collisions"]
        .as_array()
        .unwrap();
    for name in ["shared", "tools"] {
        assert!(collisions.iter().any(|e| e == name));
    }
    // Hook effects belong to the initial private checkpoint too.
    let private = disk
        .store(&disk.project, "fresh")
        .load_session()
        .await
        .unwrap()
        .unwrap();
    assert!(entries(&private).iter().any(|e| e["name"] == "observed"));
    fresh.shutdown().await.unwrap();
}

#[tokio::test]
#[ignore = "requires a Deno Jupyter executable, DENO_PROGRAM defaults to deno"]
async fn resume_attempts_changed_removed_and_invalid_profiles_without_overwriting_state() {
    let disk = Disk::new();
    disk.save_profile().await;
    let mut fresh = disk.selected("thread", "default", false).await.unwrap();
    fresh.shutdown().await.unwrap();
    let mut source = disk.start_at(&disk.source, "source").await;
    execute(
        &mut source,
        "shared = 'changed'; globalThis.newProfileValue = 7;",
    )
    .await;
    source
        .control(NotebookRequest::Save {
            name: "default".into(),
        })
        .await
        .unwrap();
    source.shutdown().await.unwrap();
    let mut resumed = disk.selected("thread", "default", false).await.unwrap();
    assert_eq!(read(&mut resumed, "shared").await, "profile");
    assert_eq!(
        read(&mut resumed, "[typeof unwantedHook, typeof pending]").await,
        json!(["undefined", "undefined"])
    );
    assert_eq!(
        read(&mut resumed, "typeof newProfileValue").await,
        "undefined"
    );
    assert_eq!(resumed.status["defaultProfile"]["applied"], false);
    assert!(
        resumed.status["defaultProfile"]["collisionCount"]
            .as_u64()
            .unwrap()
            > 0
    );
    resumed.shutdown().await.unwrap();
    std::fs::remove_dir_all(disk.profile_file().parent().unwrap()).unwrap();
    let mut resumed = disk.selected("thread", "default", false).await.unwrap();
    assert_eq!(read(&mut resumed, "shared").await, "profile");
    assert!(
        resumed.status["defaultProfile"]["error"]
            .as_str()
            .unwrap()
            .contains("not found")
    );
    resumed
        .control(NotebookRequest::Release {
            names: vec!["shared".into(), "helper".into(), "profileHook".into()],
        })
        .await
        .unwrap();
    resumed.shutdown().await.unwrap();
    let mut empty = disk.selected("thread", "../invalid", false).await.unwrap();
    assert_eq!(empty.status["defaultProfile"]["applied"], false);
    assert!(empty.status["defaultProfile"]["error"].is_string());
    assert_eq!(
        read(
            &mut empty,
            "[typeof shared, typeof helper, typeof profileHook]"
        )
        .await,
        json!(["undefined", "undefined", "undefined"])
    );
    empty.shutdown().await.unwrap();
}

#[tokio::test]
#[ignore = "requires a Deno Jupyter executable, DENO_PROGRAM defaults to deno"]
async fn restart_and_recovery_retry_the_configured_profile_but_reset_skips_it() {
    let disk = Disk::new();
    disk.save_profile().await;
    let mut fresh = disk.selected("fresh", "default", false).await.unwrap();
    fresh
        .control(NotebookRequest::Release {
            names: vec!["shared".into()],
        })
        .await
        .unwrap();
    fresh.control(NotebookRequest::Restart).await.unwrap();
    assert_eq!(read(&mut fresh, "typeof shared").await, "undefined");
    assert_eq!(fresh.status["defaultProfile"]["applied"], false);
    // Lexical release forces the internal restore path too.
    execute(&mut fresh, "const lexical = 42;").await;
    let result = fresh
        .control(NotebookRequest::Release {
            names: vec!["lexical".into()],
        })
        .await
        .unwrap();
    assert_eq!(result.details["restartRequired"], true);
    assert_eq!(read(&mut fresh, "typeof shared").await, "undefined");
    fresh.control(NotebookRequest::Reset).await.unwrap();
    assert_eq!(
        read(&mut fresh, "[typeof shared, typeof helper]").await,
        json!(["undefined", "undefined"])
    );
    fresh.control(NotebookRequest::Restart).await.unwrap();
    assert_eq!(read(&mut fresh, "helper(4)").await, 5);
    assert_eq!(fresh.status["defaultProfile"]["applied"], true);
    fresh.checkpoint(&[]).await.unwrap();
    fresh.shutdown().await.unwrap();
    fresh.ensure().await.unwrap();
    assert_eq!(read(&mut fresh, "helper(4)").await, 5);
    assert_eq!(fresh.status["defaultProfile"]["applied"], false);
    fresh.shutdown().await.unwrap();
}

fn disk_contents(path: &Path) -> BTreeMap<PathBuf, Option<Vec<u8>>> {
    fn walk(root: &Path, path: &Path, found: &mut BTreeMap<PathBuf, Option<Vec<u8>>>) {
        for entry in std::fs::read_dir(path).unwrap() {
            let path = entry.unwrap().path();
            found.insert(
                path.strip_prefix(root).unwrap().to_path_buf(),
                if path.is_file() {
                    Some(std::fs::read(&path).unwrap())
                } else {
                    None
                },
            );
            if path.is_dir() {
                walk(root, &path, found);
            }
        }
    }
    let mut found = BTreeMap::new();
    walk(path, path, &mut found);
    found
}

#[tokio::test]
#[ignore = "requires a Deno Jupyter executable, DENO_PROGRAM defaults to deno"]
async fn ephemeral_profile_reads_and_lifecycle_controls_never_change_disk() {
    let disk = Disk::new();
    disk.save_profile().await;
    std::fs::remove_file(disk.profile_file().parent().unwrap().join("write.lock")).unwrap();
    let before = disk_contents(&disk.home);
    let mut ephemeral = disk.selected("ephemeral", "default", true).await.unwrap();
    assert_eq!(read(&mut ephemeral, "helper(4)").await, 5);
    execute(&mut ephemeral, "shared = 'memory';").await;
    ephemeral
        .control(NotebookRequest::Checkpoint)
        .await
        .unwrap();
    ephemeral.control(NotebookRequest::Restart).await.unwrap();
    assert_eq!(read(&mut ephemeral, "shared").await, "memory");
    ephemeral.control(NotebookRequest::Reset).await.unwrap();
    assert_eq!(read(&mut ephemeral, "typeof helper").await, "undefined");
    assert!(
        ephemeral
            .control(NotebookRequest::Save {
                name: "forbidden".into()
            })
            .await
            .is_err()
    );
    ephemeral.shutdown().await.unwrap();
    assert_eq!(disk_contents(&disk.home), before);
}

#[tokio::test]
#[ignore = "requires a Deno Jupyter executable, DENO_PROGRAM defaults to deno"]
async fn unreadable_profiles_report_not_loaded_but_invalid_values_fail_before_hooks() {
    let disk = Disk::new();
    disk.save_profile().await;
    let marker = disk.project.join("hook-ran");
    let mut owner = disk.start_at(&disk.project, "owner").await;
    execute(
        &mut owner,
        &format!(
            "globalThis.onStart = () => Deno.writeTextFileSync({}, 'ran');",
            json!(marker)
        ),
    )
    .await;
    owner
        .control(NotebookRequest::Pin {
            names: vec!["onStart".into()],
            hook: Some(NotebookHook::Startup),
        })
        .await
        .unwrap();
    owner.shutdown().await.unwrap();
    for name in ["missing", "../invalid"] {
        let mut started = disk.selected(name, name, false).await.unwrap();
        assert_eq!(started.status["defaultProfile"]["applied"], false);
        assert!(started.status["defaultProfile"]["error"].is_string());
        assert!(marker.exists());
        assert!(
            disk.store(&disk.project, name)
                .load_session()
                .await
                .unwrap()
                .is_some()
        );
        started.shutdown().await.unwrap();
        std::fs::remove_file(&marker).unwrap();
    }
    let bytes = std::fs::read(disk.profile_file()).unwrap();
    std::fs::write(disk.profile_file(), b"not json").unwrap();
    let mut corrupt = disk.selected("corrupt", "default", false).await.unwrap();
    let error = corrupt.status["defaultProfile"]["error"].as_str().unwrap();
    assert!(error.contains("Corrupt notebook state"), "{error}");
    assert!(marker.exists());
    corrupt.shutdown().await.unwrap();
    std::fs::remove_file(&marker).unwrap();
    let mut invalid: Value = serde_json::from_slice(&bytes).unwrap();
    invalid["snapshot"]["entries"][0]["data"] = json!("anVuaw==");
    invalid["snapshot"]["entries"][0]["length"] = json!(4);
    std::fs::write(disk.profile_file(), serde_json::to_vec(&invalid).unwrap()).unwrap();
    assert!(
        disk.selected("invalid-value", "default", false)
            .await
            .is_err()
    );
    assert!(!marker.exists());
    assert!(
        disk.store(&disk.project, "invalid-value")
            .load_session()
            .await
            .unwrap()
            .is_none()
    );
}
