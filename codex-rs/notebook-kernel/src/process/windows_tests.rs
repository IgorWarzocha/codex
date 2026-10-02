use super::*;
use std::os::windows::io::AsRawHandle;
use std::os::windows::io::FromRawHandle;
use std::os::windows::io::OwnedHandle;
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;
use tokio::process::Command;
use windows_sys::Win32::Foundation::WAIT_OBJECT_0;
use windows_sys::Win32::Foundation::WAIT_TIMEOUT;
use windows_sys::Win32::System::Threading::OpenProcess;
use windows_sys::Win32::System::Threading::PROCESS_SYNCHRONIZE;
use windows_sys::Win32::System::Threading::WaitForSingleObject;

const PROBE: &str = "process::windows::tests::tree_probe";
const ROLE: &str = "CODEX_NOTEBOOK_TREE_PROBE";
const DIRECTORY: &str = "CODEX_NOTEBOOK_TREE_DIRECTORY";

fn probe_command(directory: &Path, role: &str) -> io::Result<Command> {
    let mut command = Command::new(std::env::current_exe()?);
    command
        .args(["--exact", PROBE, "--nocapture"])
        .env(ROLE, role)
        .env(DIRECTORY, directory)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    Ok(command)
}

fn wait_for_file(path: &Path) -> io::Result<()> {
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while !path.exists() {
        if std::time::Instant::now() >= deadline {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                path.display().to_string(),
            ));
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    Ok(())
}

// Re-executed by contained children, never by a shell or a model.
#[test]
fn tree_probe() -> io::Result<()> {
    let Ok(role) = std::env::var(ROLE) else {
        return Ok(());
    };
    let directory = std::path::PathBuf::from(std::env::var_os(DIRECTORY).expect("probe directory"));
    match role.as_str() {
        "leader" => {
            let mut command = probe_command(&directory, "descendant")?;
            // This probe intentionally leaves its child alive when the leader exits.
            let descendant = command.as_std_mut().spawn()?;
            wait_for_file(&directory.join("descendant_started"))?;
            std::fs::write(
                directory.join("leader_started"),
                descendant.id().to_string(),
            )?;
            wait_for_file(&directory.join("exit_leader"))?;
        }
        "descendant" => {
            std::fs::write(directory.join("descendant_started"), "")?;
            wait_for_file(&directory.join("release_descendant"))?;
            std::fs::write(directory.join("survived"), "")?;
        }
        "rejected" => std::fs::write(directory.join("unowned_execution"), "")?,
        _ => panic!("unknown probe role"),
    }
    Ok(())
}

async fn wait_for_marker(path: &Path) -> io::Result<()> {
    tokio::time::timeout(Duration::from_secs(10), async {
        while !path.exists() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .map_err(io::Error::other)
}

fn descendant_handle(directory: &Path) -> io::Result<OwnedHandle> {
    let pid = std::fs::read_to_string(directory.join("leader_started"))?
        .parse::<u32>()
        .map_err(io::Error::other)?;
    // SAFETY: opening by PID is valid. The live probe is blocked until cleanup below.
    let handle = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, 0, pid) };
    if handle.is_null() {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: OpenProcess returned a new owned, non-null process handle.
    Ok(unsafe { OwnedHandle::from_raw_handle(handle.cast()) })
}

async fn assert_descendant_stopped(handle: &OwnedHandle, directory: &Path) -> io::Result<()> {
    let stopped = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            // SAFETY: the owned handle stays live across this nonblocking wait.
            let status = unsafe { WaitForSingleObject(handle.as_raw_handle().cast(), 0) };
            match status {
                WAIT_OBJECT_0 => return Ok::<_, io::Error>(()),
                WAIT_TIMEOUT => tokio::time::sleep(Duration::from_millis(10)).await,
                _ => return Err(io::Error::last_os_error()),
            }
        }
    })
    .await;
    // Release on failure too so an incorrect implementation cannot leave a probe behind.
    std::fs::write(directory.join("release_descendant"), "")?;
    stopped??;
    assert!(!directory.join("survived").exists());
    Ok(())
}

#[tokio::test]
async fn contained_tree_dies_on_kill_drop_and_idle_leader_exit() -> io::Result<()> {
    for mode in ["kill", "drop", "leader exit", "supervisor abort"] {
        let directory = tempfile::tempdir()?;
        let job = JobObject::create_without_breakaway()?;
        let child = job.spawn_contained(&mut probe_command(directory.path(), "leader")?)?;
        let mut owned = OwnedChild::new(child, job);
        wait_for_marker(&directory.path().join("leader_started")).await?;
        let descendant = descendant_handle(directory.path())?;
        match mode {
            "kill" => {
                owned.start_kill()?;
                tokio::time::timeout(Duration::from_secs(10), owned.wait()).await??;
                // Waiting is repeatable, matching Tokio Child's contract.
                owned.wait().await?;
                drop(owned);
            }
            "drop" => drop(owned),
            "supervisor abort" => {
                owned.supervisor.as_ref().expect("supervisor").abort();
                assert_descendant_stopped(&descendant, directory.path()).await?;
                drop(owned);
                continue;
            }
            _ => {
                std::fs::write(directory.path().join("exit_leader"), "")?;
                // No call to wait: cleanup must happen while the notebook is idle.
                tokio::time::timeout(Duration::from_secs(10), async {
                    while owned.try_wait()?.is_none() {
                        tokio::time::sleep(Duration::from_millis(10)).await;
                    }
                    Ok::<_, io::Error>(())
                })
                .await??;
                // Keep the job owner alive until after the survival assertion.
                assert_descendant_stopped(&descendant, directory.path()).await?;
                owned.wait().await?;
                continue;
            }
        }
        assert_descendant_stopped(&descendant, directory.path()).await?;
    }
    Ok(())
}

#[tokio::test]
async fn rejected_assignment_never_runs_child_code() -> io::Result<()> {
    use windows_sys::Win32::System::JobObjects::*;

    let directory = tempfile::tempdir()?;
    let job = JobObject::create_without_breakaway()?;
    // Fill a one-process job to force a real assignment failure, not a mocked API.
    let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
    limits.BasicLimitInformation.LimitFlags =
        JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE | JOB_OBJECT_LIMIT_ACTIVE_PROCESS;
    limits.BasicLimitInformation.ActiveProcessLimit = 1;
    // SAFETY: the job owns a valid handle and limits is initialized with its exact size.
    let configured = unsafe {
        SetInformationJobObject(
            job.as_raw_handle().cast(),
            JobObjectExtendedLimitInformation,
            std::ptr::addr_of!(limits).cast(),
            std::mem::size_of_val(&limits) as u32,
        )
    };
    if configured == 0 {
        return Err(io::Error::last_os_error());
    }
    let mut holder = job.spawn_contained(&mut probe_command(directory.path(), "descendant")?)?;
    wait_for_marker(&directory.path().join("descendant_started")).await?;
    assert!(
        job.spawn_contained(&mut probe_command(directory.path(), "rejected")?)
            .is_err()
    );
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(!directory.path().join("unowned_execution").exists());
    job.terminate()?;
    tokio::time::timeout(Duration::from_secs(10), holder.wait()).await??;
    Ok(())
}
