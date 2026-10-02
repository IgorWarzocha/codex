//! Cold selection and lazy, privately cached Deno acquisition.
//! Callers must validate native full-access permission before calling `resolve`.

mod assets;
mod install;
#[cfg(test)]
mod tests;

use std::path::Path;
use std::path::PathBuf;

enum Selection {
    Existing(PathBuf),
    Managed,
}

/// Checks selection only. Never runs Deno, hashes a cache, or starts a download.
pub(crate) fn availability(
    configured: Option<&Path>,
    cwd: &Path,
    codex_home: Option<&Path>,
) -> Result<(), String> {
    select(configured, cwd, codex_home, std::env::var_os("PATH")).map(|_| ())
}

pub(crate) async fn resolve(
    configured: Option<&Path>,
    cwd: &Path,
    codex_home: Option<&Path>,
) -> Result<PathBuf, String> {
    match select(configured, cwd, codex_home, std::env::var_os("PATH"))? {
        Selection::Existing(path) => Ok(path),
        Selection::Managed => {
            let home = codex_home.ok_or_else(missing_home)?;
            install::resolve(home, assets::current()?).await.map_err(|error| {
                format!(
                    "Cannot acquire notebook Deno {}: {error}. Set features.code_mode.deno_program to an installed Deno executable to override automatic acquisition",
                    assets::VERSION
                )
            })
        }
    }
}

/// Startup warmup only reads existing runtimes. Installation remains authorized-use only.
pub(crate) async fn resolve_for_prewarm(
    configured: Option<&Path>,
    cwd: &Path,
    codex_home: Option<&Path>,
) -> Result<Option<PathBuf>, String> {
    match select(configured, cwd, codex_home, std::env::var_os("PATH"))? {
        Selection::Existing(path) => Ok(Some(path)),
        Selection::Managed => {
            install::resolve_cached(codex_home.ok_or_else(missing_home)?, assets::current()?).await
        }
    }
}

fn select(
    configured: Option<&Path>,
    cwd: &Path,
    codex_home: Option<&Path>,
    search_path: Option<std::ffi::OsString>,
) -> Result<Selection, String> {
    if let Some(program) = configured {
        // Preserve which_in's handling of explicit names and cwd-relative paths.
        // A configured failure is authoritative, never a reason to download.
        return which::which_in(program, search_path, cwd)
            .map(Selection::Existing)
            .map_err(|error| {
                format!(
                    "Configured Deno program unavailable: {}: {error}",
                    program.display()
                )
            });
    }
    match which::which_in("deno", search_path, cwd) {
        Ok(path) => Ok(Selection::Existing(path)),
        Err(which::Error::CannotFindBinaryPath) => {
            assets::current()?;
            codex_home.ok_or_else(missing_home)?;
            Ok(Selection::Managed)
        }
        Err(error) => Err(format!("Cannot search PATH for notebook Deno: {error}")),
    }
}

fn missing_home() -> String {
    "Deno is missing from PATH and automatic installation needs CODEX_HOME. Set features.code_mode.deno_program to an installed Deno executable".into()
}
