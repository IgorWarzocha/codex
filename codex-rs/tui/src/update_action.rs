#[cfg(any(not(debug_assertions), test))]
use codex_install_context::InstallContext;
#[cfg(any(not(debug_assertions), test))]
use codex_install_context::InstallMethod;
#[cfg(any(not(debug_assertions), test))]
use codex_install_context::StandalonePlatform;

/// Update action the CLI should perform after the TUI exits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpdateAction {
    /// Replace the local daemon after restoring the terminal.
    Daemon(DaemonUpdateSource),
    /// Update via `npm install -g @openai/codex@latest`.
    NpmGlobalLatest,
    /// Update via `bun install -g @openai/codex@latest`.
    BunGlobalLatest,
    /// Update via `vp install -g @openai/codex@latest`.
    VitePlusGlobalLatest,
    /// Update via `pnpm add -g @openai/codex@latest`.
    PnpmGlobalLatest,
    /// Update via `brew upgrade codex`.
    BrewUpgrade,
    /// Update via `curl -fsSL https://chatgpt.com/codex/install.sh | CODEX_NON_INTERACTIVE=1 sh`.
    StandaloneUnix,
    /// Update via `$env:CODEX_NON_INTERACTIVE=1; irm https://chatgpt.com/codex/install.ps1 | iex`.
    StandaloneWindows,
}

impl UpdateAction {
    #[cfg(any(not(debug_assertions), test))]
    pub(crate) fn from_install_context(context: &InstallContext, version: &str) -> Option<Self> {
        if codex_build_info::is_fork_version(version) {
            return None;
        }
        match &context.method {
            InstallMethod::Npm => Some(UpdateAction::NpmGlobalLatest),
            InstallMethod::Bun => Some(UpdateAction::BunGlobalLatest),
            InstallMethod::VitePlus => Some(UpdateAction::VitePlusGlobalLatest),
            InstallMethod::Pnpm => Some(UpdateAction::PnpmGlobalLatest),
            InstallMethod::Brew => Some(UpdateAction::BrewUpgrade),
            InstallMethod::Standalone { platform, .. } => Some(match platform {
                StandalonePlatform::Unix => UpdateAction::StandaloneUnix,
                StandalonePlatform::Windows => UpdateAction::StandaloneWindows,
            }),
            InstallMethod::Other => None,
        }
    }

    /// Returns the list of command-line arguments for invoking the update.
    pub fn command_args(self) -> (&'static str, &'static [&'static str]) {
        match self {
            UpdateAction::Daemon(source) => ("codex", source.command_args()),
            UpdateAction::NpmGlobalLatest => ("npm", &["install", "-g", "@openai/codex"]),
            UpdateAction::BunGlobalLatest => ("bun", &["install", "-g", "@openai/codex"]),
            UpdateAction::VitePlusGlobalLatest => ("vp", &["install", "-g", "@openai/codex"]),
            UpdateAction::PnpmGlobalLatest => ("pnpm", &["add", "-g", "@openai/codex"]),
            UpdateAction::BrewUpgrade => ("brew", &["upgrade", "--cask", "codex"]),
            UpdateAction::StandaloneUnix => (
                "sh",
                &[
                    "-c",
                    "curl -fsSL https://chatgpt.com/codex/install.sh | CODEX_NON_INTERACTIVE=1 sh",
                ],
            ),
            UpdateAction::StandaloneWindows => (
                "powershell",
                &[
                    "-ExecutionPolicy",
                    "Bypass",
                    "-c",
                    "$env:CODEX_NON_INTERACTIVE=1; irm https://chatgpt.com/codex/install.ps1 | iex",
                ],
            ),
        }
    }

    /// Returns string representation of the command-line arguments for invoking the update.
    pub fn command_str(self) -> String {
        let (command, args) = self.command_args();
        shlex::try_join(std::iter::once(command).chain(args.iter().copied()))
            .unwrap_or_else(|_| format!("{command} {}", args.join(" ")))
    }
}

#[cfg(any(not(debug_assertions), test))]
pub fn get_update_action() -> Option<UpdateAction> {
    UpdateAction::from_install_context(InstallContext::current(), crate::version::CODEX_CLI_VERSION)
}

#[cfg(test)]
mod tests {
    use super::*;
    use codex_utils_absolute_path::AbsolutePathBuf;
    use pretty_assertions::assert_eq;

    #[test]
    fn fork_versions_never_select_upstream_installers() {
        for version in ["0.160.0-lean.1", "0.160.0-howaboua.1"] {
            for method in [
                InstallMethod::Npm,
                InstallMethod::Bun,
                InstallMethod::VitePlus,
                InstallMethod::Pnpm,
                InstallMethod::Brew,
                InstallMethod::Standalone {
                    platform: StandalonePlatform::Unix,
                    release_dir: AbsolutePathBuf::from_absolute_path(std::env::temp_dir())
                        .expect("absolute temp directory"),
                    resources_dir: None,
                },
                InstallMethod::Standalone {
                    platform: StandalonePlatform::Windows,
                    release_dir: AbsolutePathBuf::from_absolute_path(std::env::temp_dir())
                        .expect("absolute temp directory"),
                    resources_dir: None,
                },
            ] {
                assert_eq!(
                    UpdateAction::from_install_context(
                        &InstallContext {
                            method,
                            package_layout: None
                        },
                        version
                    ),
                    None
                );
            }
        }
    }

    #[test]
    fn maps_install_context_to_update_action() {
        let native_release_dir =
            AbsolutePathBuf::from_absolute_path(std::env::temp_dir().join("native-release"))
                .expect("temp dir path should be absolute");

        assert_eq!(
            UpdateAction::from_install_context(
                &InstallContext {
                    method: InstallMethod::Other,
                    package_layout: None,
                },
                "0.160.0"
            ),
            None
        );
        assert_eq!(
            UpdateAction::from_install_context(
                &InstallContext {
                    method: InstallMethod::Npm,
                    package_layout: None,
                },
                "0.160.0"
            ),
            Some(UpdateAction::NpmGlobalLatest)
        );
        assert_eq!(
            UpdateAction::from_install_context(
                &InstallContext {
                    method: InstallMethod::Bun,
                    package_layout: None,
                },
                "0.160.0"
            ),
            Some(UpdateAction::BunGlobalLatest)
        );
        assert_eq!(
            UpdateAction::from_install_context(
                &InstallContext {
                    method: InstallMethod::Pnpm,
                    package_layout: None,
                },
                "0.160.0"
            ),
            Some(UpdateAction::PnpmGlobalLatest)
        );
        assert_eq!(
            UpdateAction::from_install_context(
                &InstallContext {
                    method: InstallMethod::Brew,
                    package_layout: None,
                },
                "0.160.0"
            ),
            Some(UpdateAction::BrewUpgrade)
        );
        assert_eq!(
            UpdateAction::from_install_context(
                &InstallContext {
                    method: InstallMethod::Standalone {
                        platform: StandalonePlatform::Unix,
                        release_dir: native_release_dir.clone(),
                        resources_dir: Some(native_release_dir.join("codex-resources")),
                    },
                    package_layout: None,
                },
                "0.160.0"
            ),
            Some(UpdateAction::StandaloneUnix)
        );
        assert_eq!(
            UpdateAction::from_install_context(
                &InstallContext {
                    method: InstallMethod::Standalone {
                        platform: StandalonePlatform::Windows,
                        release_dir: native_release_dir.clone(),
                        resources_dir: Some(native_release_dir.join("codex-resources")),
                    },
                    package_layout: None,
                },
                "0.160.0"
            ),
            Some(UpdateAction::StandaloneWindows)
        );
    }

    #[test]
    fn standalone_update_commands_rerun_latest_installer() {
        assert_eq!(
            UpdateAction::StandaloneUnix.command_args(),
            (
                "sh",
                &[
                    "-c",
                    "curl -fsSL https://chatgpt.com/codex/install.sh | CODEX_NON_INTERACTIVE=1 sh"
                ][..],
            )
        );
        assert_eq!(
            UpdateAction::StandaloneWindows.command_args(),
            (
                "powershell",
                &[
                    "-ExecutionPolicy",
                    "Bypass",
                    "-c",
                    "$env:CODEX_NON_INTERACTIVE=1; irm https://chatgpt.com/codex/install.ps1 | iex"
                ][..],
            )
        );
    }
}

/// Package source explicitly selected by the user in the daemon menu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DaemonUpdateSource {
    PublicStable,
    ThisCli,
}

impl DaemonUpdateSource {
    pub fn command_args(self) -> &'static [&'static str] {
        match self {
            Self::PublicStable => &["app-server", "daemon", "update"],
            Self::ThisCli => &["app-server", "daemon", "update", "--from-cli", "--yes"],
        }
    }
}
