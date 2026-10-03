//! Pre-thread recovery surface for the same defaults shown by TUI settings.

use anyhow::Context;
use codex_config::cli_settings::CliSetting;
use codex_core::config::LoaderOverrides;
use codex_core::config::edit::ConfigEdit;
use codex_core::config::edit::ConfigEditsBuilder;
use codex_core::config::find_codex_home;
use codex_utils_cli::CliConfigOverrides;
use serde_json::Value;

#[derive(Debug, clap::Parser)]
pub(crate) struct SettingsCommand {
    #[command(subcommand)]
    command: Option<SettingsAction>,
}

#[derive(Debug, clap::Subcommand)]
enum SettingsAction {
    /// Save a default. Run `codex settings` to see settings and choices.
    Set { setting: String, choice: String },
}

pub(crate) async fn run(
    command: SettingsCommand,
    overrides: &CliConfigOverrides,
) -> anyhow::Result<()> {
    let home = find_codex_home()?;
    // Load the normal managed config without constructing a session, acquiring Deno or
    // applying Notes' session authentication requirements.
    let read_config = async || {
        let config =
            crate::cloud_config::load_config(overrides, LoaderOverrides::default()).await?;
        Ok::<_, anyhow::Error>(serde_json::to_value(
            config.config_layer_stack.effective_config(),
        )?)
    };
    match command.command {
        None => {
            let values = read_config().await?;
            println!("Configured defaults for this project. Running threads are unchanged.\n");
            for setting in CliSetting::ALL {
                println!(
                    "{} = {}\n  Choices: {}\n  {}",
                    setting.key(),
                    setting.configured_choice(&values),
                    setting.choices().join(", "),
                    setting.description()
                );
            }
            println!(
                "\nSave: codex settings set <setting> <choice>\nExperimental tools: /experimental in the TUI, or codex features list"
            );
        }
        Some(SettingsAction::Set { setting, choice }) => {
            let setting = CliSetting::parse(&setting).with_context(|| {
                format!("Unknown setting `{setting}`. Run `codex settings` for choices.")
            })?;
            let requested = setting.edits(&choice).map_err(anyhow::Error::msg)?;
            // Validate the proposed config through the owning managed ConfigBuilder before
            // writing. This neither starts a thread nor changes permission profiles.
            let mut proposed = CliConfigOverrides {
                raw_overrides: overrides.raw_overrides.clone(),
            };
            for (path, value) in &requested {
                if !value.is_null() {
                    proposed
                        .raw_overrides
                        .push(format!("{path}={}", serde_json::to_string(value)?));
                }
            }
            let proposed_config =
                crate::cloud_config::load_config(&proposed, LoaderOverrides::default())
                    .await
                    .context("Setting rejected by configuration or managed requirements")?;
            let proposed_toml = proposed_config
                .config_layer_stack
                .effective_config()
                .try_into()?;
            codex_core::config::validate_feature_requirements_for_config_toml(
                &proposed_toml,
                proposed_config
                    .config_layer_stack
                    .requirements()
                    .feature_requirements
                    .as_ref(),
            )
            .context("Setting rejected by managed feature requirements")?;
            let edits = requested
                .into_iter()
                .map(|(path, value)| local_edit(path, value))
                .collect::<anyhow::Result<Vec<_>>>()?;
            ConfigEditsBuilder::new(home.as_path())
                .with_edits(edits)
                .apply()
                .await?;
            let values = read_config()
                .await
                .context("Setting saved, but configured readback failed")?;
            if !setting.matches_edits(&values, &choice) {
                anyhow::bail!(
                    "Setting saved, but overridden. Configured {}: {}. Check project, profile or command-line settings.",
                    setting.key(),
                    setting.configured_choice(&values)
                );
            }
            println!(
                "Saved {}: {choice}. {} Running threads are unchanged.",
                setting.key(),
                setting.description()
            );
        }
    }
    Ok(())
}

fn local_edit(path: &str, value: Value) -> anyhow::Result<ConfigEdit> {
    let segments = path.split('.').map(str::to_owned).collect();
    let value = match value {
        Value::Null => return Ok(ConfigEdit::ClearPath { segments }),
        Value::Bool(value) => toml_edit::value(value),
        Value::String(value) => toml_edit::value(value),
        Value::Number(value) => {
            toml_edit::value(value.as_i64().context("Setting requires an integer")?)
        }
        _ => anyhow::bail!("Unsupported setting value"),
    };
    Ok(ConfigEdit::SetPath { segments, value })
}
