//! Saved setting defaults use server-owned config RPCs, never live thread overrides.

use super::*;
use codex_app_server_protocol::ClientRequest;
use codex_app_server_protocol::ConfigBatchWriteParams;
use codex_app_server_protocol::ConfigReadParams;
use codex_app_server_protocol::ConfigReadResponse;
use codex_app_server_protocol::ConfigWriteResponse;
use codex_app_server_protocol::RequestId;
use codex_config::cli_settings::CliSetting;
use std::time::Duration;

async fn read_settings(
    handle: &codex_app_server_client::AppServerRequestHandle,
    cwd: String,
) -> anyhow::Result<serde_json::Value> {
    let response: ConfigReadResponse = tokio::time::timeout(
        Duration::from_secs(5),
        handle.request_typed(ClientRequest::ConfigRead {
            request_id: RequestId::String("tui-settings-read".into()),
            params: ConfigReadParams {
                include_layers: false,
                cwd: Some(cwd),
            },
        }),
    )
    .await
    .map_err(|_| anyhow::anyhow!("Settings discovery timed out"))??;
    Ok(serde_json::to_value(response.config)?)
}

impl App {
    pub(super) fn open_cli_setting(&mut self, app_server: &AppServerSession, setting: CliSetting) {
        let Ok(guard) = self.feature_write_lock.clone().try_lock_owned() else {
            self.chat_widget.add_warning_message(
                "A settings operation is in progress. Retry after it finishes.".into(),
            );
            return;
        };
        let cwd = self.chat_widget.config_ref().cwd.display().to_string();
        let request_id = self.chat_widget.open_cli_setting_loading(setting);
        let handle = app_server.request_handle();
        let tx = self.app_event_tx.clone();
        tokio::spawn(async move {
            let result = read_settings(&handle, cwd.clone())
                .await
                .map(|config| setting.configured_choice(&config))
                .map_err(|error| format!("{error:#}"));
            drop(guard);
            tx.send(AppEvent::CliSettingDiscovered {
                request_id,
                setting,
                cwd,
                result,
            });
        });
    }

    pub(super) fn persist_cli_setting(
        &mut self,
        app_server: &AppServerSession,
        setting: CliSetting,
        choice: String,
        cwd: String,
    ) {
        let Ok(guard) = self.feature_write_lock.clone().try_lock_owned() else {
            self.chat_widget.add_warning_message(
                "A settings operation is in progress. Retry after it finishes.".into(),
            );
            return;
        };
        let edits = match setting.edits(&choice) {
            Ok(edits) => edits
                .into_iter()
                .map(|(path, value)| crate::config_update::replace_config_value(path, value))
                .collect(),
            Err(error) => {
                self.chat_widget.add_error_message(error);
                return;
            }
        };
        let handle = app_server.request_handle();
        let tx = self.app_event_tx.clone();
        self.chat_widget.add_info_message(
            format!("Saving {}… This thread is unchanged.", setting.title()),
            None,
        );
        tokio::spawn(async move {
            // A submitted save survives navigation. Fixed IDs bound unanswered retries.
            let result = async {
                let response: ConfigWriteResponse = tokio::time::timeout(Duration::from_secs(15), handle.request_typed(ClientRequest::ConfigBatchWrite {
                    request_id: RequestId::String("tui-settings-write".into()),
                    params: ConfigBatchWriteParams { edits, file_path: None, expected_version: None, reload_user_config: true },
                })).await.map_err(|_| anyhow::anyhow!("Save timed out and may still finish. Reopen /settings to check before retrying."))??;
                let config = read_settings(&handle, cwd).await.map_err(|error| anyhow::anyhow!("Saved, but configured readback failed: {error:#}"))?;
                if response.status != WriteStatus::Ok || !setting.matches_edits(&config, &choice) {
                    anyhow::bail!("Saved, but higher-priority config overrides the selection. Configured {}: {}. {}", setting.title(), setting.configured_choice(&config), super::config_persistence::overridden_write_message(&response));
                }
                Ok::<_, anyhow::Error>(format!("{} saved: {choice}. {} This thread is unchanged.", setting.title(), setting.description()))
            }.await;
            drop(guard);
            let cell: Box<dyn HistoryCell> = match result {
                Ok(message) => Box::new(history_cell::new_warning_event(message)),
                Err(error) => Box::new(history_cell::new_error_event(format!(
                    "{}: {error:#}",
                    setting.title()
                ))),
            };
            tx.send(AppEvent::InsertHistoryCell(cell));
        });
    }
}
