//! One-shot historical Deno LSP diagnostics, independent of the live kernel/worker.
use std::collections::HashMap;
use std::collections::HashSet;
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use serde_json::Value;
use serde_json::json;
use tokio::io::AsyncReadExt;
use tokio::io::AsyncWriteExt;
use tokio::process::Child;
use tokio::process::ChildStdin;
use tokio::process::ChildStdout;
use tokio::process::Command;
use tokio::task::JoinHandle;
use url::Url;

use crate::control::NotebookControlResult;
use crate::journal::CodeCell;
use crate::journal::Journal;
use crate::journal::bound_text;

const TIMEOUT: Duration = Duration::from_secs(30);
const RESPONSE_BUDGET: usize = 16 * 1024;
const TEXT_BUDGET: usize = 2 * 1024;
const MAX_MESSAGE: usize = 16 * 1024 * 1024;
const MAX_DIAGNOSTICS: usize = 16 * 1024;
const HOST_BINDINGS: &[&str] = &[
    "ALL_TOOLS",
    "exit",
    "generatedImage",
    "image",
    "load",
    "notify",
    "store",
    "text",
    "tools",
    "yield_control",
];

pub(crate) async fn diagnostics_with_runtime(
    deno_program: &Path,
    cwd: &Path,
    codex_home: &Path,
    thread_id: &str,
    runtime_health: &str,
    runtime_bindings: &[String],
) -> Result<NotebookControlResult, String> {
    if !matches!(
        runtime_health,
        "ready" | "running" | "invalidated" | "not_started"
    ) {
        return Err("invalid notebook runtime health".into());
    }
    let journal = Journal::new(codex_home, cwd, thread_id)?;
    let journal_path = journal.path.clone();
    let path = bound_text(&journal.path.display().to_string(), TEXT_BUDGET);
    let cells = match tokio::task::spawn_blocking(move || journal.code_cells())
        .await
        .map_err(|e| e.to_string())?
    {
        Ok(cells) => cells,
        Err(error) => {
            return Ok(failure(
                &path,
                0,
                runtime_health,
                &format!("could not read journal: {error}"),
            ));
        }
    };
    if cells.is_empty() {
        return Ok(format_diagnostics(&path, 0, runtime_health, Vec::new()));
    }
    let mut lsp = match Lsp::start(deno_program, cwd) {
        Ok(lsp) => lsp,
        Err(error) => return Ok(failure(&path, cells.len(), runtime_health, &error)),
    };
    let result = tokio::time::timeout(
        TIMEOUT,
        collect(&mut lsp, cwd, &journal_path, &cells, runtime_bindings),
    )
    .await;
    // Shutdown has its own finite grace. Cancellation drops Lsp and kills the child too.
    lsp.shutdown().await;
    Ok(match result {
        Ok(Ok(diagnostics)) => format_diagnostics(&path, cells.len(), runtime_health, diagnostics),
        Ok(Err(error)) => failure(&path, cells.len(), runtime_health, &error),
        Err(_) => failure(
            &path,
            cells.len(),
            runtime_health,
            "Deno diagnostics timed out after 30000ms",
        ),
    })
}

async fn collect(
    lsp: &mut Lsp,
    cwd: &Path,
    path: &Path,
    cells: &[CodeCell],
    runtime_bindings: &[String],
) -> Result<Vec<Value>, String> {
    let root = Url::from_directory_path(cwd).map_err(|()| "invalid diagnostics workspace path")?;
    lsp.request("initialize", json!({
        "processId":std::process::id(), "clientInfo":{"name":"codex-notebook-diagnostics"},
        "rootUri":root.as_str(), "workspaceFolders":[{"uri":root.as_str(),"name":"workspace"}],
        "capabilities":{
            "workspace":{"configuration":false,"workspaceFolders":false},
            "textDocument":{"diagnostic":{},"publishDiagnostics":{"relatedInformation":true}},
            "notebookDocument":{"synchronization":{"dynamicRegistration":false,"executionSummarySupport":false}}
        }, "initializationOptions":{"enable":true}
    })).await?;
    lsp.notify("initialized", json!({})).await?;
    let notebook_uri =
        Url::from_file_path(path).map_err(|()| "invalid diagnostics journal path")?;
    let uris: Vec<String> = cells
        .iter()
        .map(|cell| {
            let mut uri = notebook_uri.clone();
            uri.set_fragment(Some(&format!("{}-{}", cell.index + 1, cell.id)));
            format!(
                "deno-notebook-cell:{}",
                uri.as_str()["file:".len()..].trim_start_matches("//")
            )
        })
        .collect();
    lsp.notify("notebookDocument/didOpen", json!({
        "notebookDocument": {"uri":notebook_uri.as_str(), "notebookType":"jupyter-notebook", "version":1,
            "cells":uris.iter().map(|uri| json!({"kind":2,"document":uri})).collect::<Vec<_>>()},
        "cellTextDocuments": cells.iter().zip(&uris).map(|(cell,uri)| json!({"uri":uri,"languageId":"typescript","version":1,"text":cell.source})).collect::<Vec<_>>()
    })).await?;
    let bindings: HashSet<&str> = HOST_BINDINGS
        .iter()
        .copied()
        .chain(runtime_bindings.iter().map(String::as_str))
        .collect();
    let mut diagnostics = Vec::new();
    for (cell, uri) in cells.iter().zip(&uris) {
        let report = lsp
            .request(
                "textDocument/diagnostic",
                json!({"textDocument":{"uri":uri}}),
            )
            .await?;
        parse_report(&report, cell, &bindings, &mut diagnostics)?;
    }
    lsp.notify("notebookDocument/didClose", json!({"notebookDocument":{"uri":notebook_uri.as_str()}, "cellTextDocuments":uris.iter().map(|uri|json!({"uri":uri})).collect::<Vec<_>>() })).await?;
    Ok(diagnostics)
}

fn parse_report(
    report: &Value,
    cell: &CodeCell,
    bindings: &HashSet<&str>,
    output: &mut Vec<Value>,
) -> Result<(), String> {
    let items = report["items"]
        .as_array()
        .ok_or("Deno LSP returned an invalid diagnostic report")?;
    for item in items {
        let Some(message) = item["message"].as_str() else {
            return Err("invalid Deno diagnostic message".into());
        };
        let start = position(&item["range"]["start"])?;
        let end = position(&item["range"]["end"])?;
        if runtime_only(item, message, cell, start, bindings) {
            continue;
        }
        if output.len() == MAX_DIAGNOSTICS {
            return Err(format!(
                "Historical diagnostics exceed the {MAX_DIAGNOSTICS} diagnostic processing budget"
            ));
        }
        let severity = match item["severity"].as_u64() {
            Some(1) => "error",
            Some(2) => "warning",
            Some(3) => "information",
            Some(4) => "hint",
            _ => "unknown",
        };
        let mut diagnostic = json!({
            "cellId":cell.id,"cellIndex":cell.index,"line":start.0+1,"column":start.1+1,"endLine":end.0+1,"endColumn":end.1+1,
            "severity":severity,"message":bound_text(message,TEXT_BUDGET)
        });
        if item["code"].is_string() || item["code"].is_number() {
            diagnostic["code"] = if let Some(code) = item["code"].as_str() {
                json!(bound_text(code, TEXT_BUDGET))
            } else {
                item["code"].clone()
            };
        }
        if let Some(source) = item["source"].as_str() {
            diagnostic["source"] = json!(bound_text(source, TEXT_BUDGET));
        }
        for prefix in [
            "Cannot redeclare block-scoped variable ",
            "Duplicate identifier ",
        ] {
            if let Some(name) = quoted_name(message, prefix) {
                diagnostic["name"] = json!(bound_text(name, TEXT_BUDGET));
            }
        }
        output.push(diagnostic);
    }
    Ok(())
}

fn position(value: &Value) -> Result<(u64, u64), String> {
    let line = value["line"]
        .as_u64()
        .filter(|v| *v < 9_007_199_254_740_991)
        .ok_or("invalid diagnostic line")?;
    let column = value["character"]
        .as_u64()
        .filter(|v| *v < 9_007_199_254_740_991)
        .ok_or("invalid diagnostic column")?;
    Ok((line, column))
}

fn quoted_name<'a>(message: &'a str, prefix: &str) -> Option<&'a str> {
    let tail = message.strip_prefix(prefix)?;
    let quote = tail.chars().next()?;
    if !matches!(quote, '\'' | '"') {
        return None;
    }
    tail[1..].split(quote).next()
}

fn runtime_only(
    item: &Value,
    message: &str,
    cell: &CodeCell,
    start: (u64, u64),
    bindings: &HashSet<&str>,
) -> bool {
    if item["code"] == 2304
        && quoted_name(message, "Cannot find name ").is_some_and(|name| bindings.contains(name))
    {
        return true;
    }
    if item["code"] != 7017 {
        return false;
    }
    let Some(line) = cell.source.lines().nth(start.0 as usize) else {
        return false;
    };
    let prefix = String::from_utf16_lossy(
        &line
            .encode_utf16()
            .take(start.1 as usize)
            .collect::<Vec<_>>(),
    );
    prefix
        .trim_end()
        .strip_suffix('.')
        .is_some_and(|prefix| prefix.trim_end().ends_with("globalThis"))
}

fn health(state: &str) -> String {
    let explanation = match state {
        "ready" => "bootstrap is available",
        "running" => "a cell is active; historical static diagnostics do not interrupt it",
        "invalidated" => {
            "notebook restart will recreate it from the last completed checkpoint. Durable project bindings are preserved; external side effects were not rolled back"
        }
        _ => "exec or restart will create it from the last completed checkpoint",
    };
    format!("Notebook runtime health: {state}; {explanation}")
}

fn failure(path: &str, cells: usize, runtime: &str, error: &str) -> NotebookControlResult {
    let error = bound_text(error, TEXT_BUDGET);
    NotebookControlResult {
        message: format!(
            "{}\nHistorical static diagnostics could not complete: {error}",
            health(runtime)
        ),
        details: json!({"path":path,"cells":cells,"runtime":{"state":runtime},"error":error}),
    }
}

fn format_diagnostics(
    path: &str,
    cells: usize,
    runtime: &str,
    diagnostics: Vec<Value>,
) -> NotebookControlResult {
    let diagnostic_count = diagnostics.len();
    let mut groups: Vec<Value> = Vec::new();
    let mut group_indices: HashMap<String, usize> = HashMap::new();
    for mut diagnostic in diagnostics {
        let sample = json!({"cellId":diagnostic["cellId"],"cellIndex":diagnostic["cellIndex"],"line":diagnostic["line"],"column":diagnostic["column"],"endLine":diagnostic["endLine"],"endColumn":diagnostic["endColumn"]});
        if let Some(object) = diagnostic.as_object_mut() {
            for key in [
                "cellId",
                "cellIndex",
                "line",
                "column",
                "endLine",
                "endColumn",
            ] {
                object.remove(key);
            }
        }
        let key = json!([
            diagnostic["severity"],
            diagnostic["message"],
            diagnostic["code"],
            diagnostic["source"],
            diagnostic["name"]
        ])
        .to_string();
        if let Some(index) = group_indices.get(&key) {
            let group = &mut groups[*index];
            group["count"] = json!(group["count"].as_u64().unwrap_or(0) + 1);
            if let Some(samples) = group["samples"].as_array_mut()
                && samples.len() < 3
            {
                samples.push(sample);
            }
        } else {
            diagnostic["count"] = json!(1);
            diagnostic["samples"] = json!([sample]);
            group_indices.insert(key, groups.len());
            groups.push(diagnostic);
        }
    }
    let total_groups = groups.len();
    let mut message = format!(
        "{}\nHistorical static diagnostics for {path} ({cells} code cells):",
        health(runtime)
    );
    let mut details = json!({"path":path,"cells":cells,"runtime":{"state":runtime},"diagnosticCount":diagnostic_count,"diagnosticGroups":[],"omittedGroups":total_groups});
    let mut included = Vec::new();
    for group in groups {
        let samples = group["samples"]
            .as_array()
            .map(|samples| {
                samples
                    .iter()
                    .map(|s| {
                        format!(
                            "{} cell {}:{}:{}-{}:{}",
                            s["cellId"].as_str().unwrap_or("?"),
                            s["cellIndex"].as_u64().unwrap_or(0) + 1,
                            s["line"],
                            s["column"],
                            s["endLine"],
                            s["endColumn"]
                        )
                    })
                    .collect::<Vec<_>>()
                    .join(", ")
            })
            .unwrap_or_default();
        let line = format!(
            "\n- {} occurrences {} {}{}: {}; samples: {samples}",
            group["count"],
            group["severity"].as_str().unwrap_or("unknown"),
            group["code"],
            group["source"]
                .as_str()
                .map(|source| format!(" ({source})"))
                .unwrap_or_default(),
            group["message"].as_str().unwrap_or("").replace('\n', " ")
        );
        included.push(group);
        details["diagnosticGroups"] = json!(included);
        details["omittedGroups"] = json!(total_groups - included.len());
        if message.len() + line.len() + 100 > RESPONSE_BUDGET
            || details.to_string().len() > RESPONSE_BUDGET
        {
            included.pop();
            break;
        }
        message.push_str(&line);
    }
    let omitted = total_groups - included.len();
    details["diagnosticGroups"] = json!(included);
    details["omittedGroups"] = json!(omitted);
    if diagnostic_count == 0 {
        message.push_str("\nNo historical static Deno diagnostics");
    }
    if omitted > 0 {
        message.push_str(&format!(
            "\n{omitted} additional historical diagnostic groups omitted by the output bound"
        ));
    }
    NotebookControlResult { message, details }
}

/// Sequential JSON-RPC client. Header/body sizes, request lifetime and teardown are bounded.
struct Lsp {
    child: Child,
    input: ChildStdin,
    output: ChildStdout,
    stderr: JoinHandle<Vec<u8>>,
    next_id: u64,
}

impl Lsp {
    fn start(deno: &Path, cwd: &Path) -> Result<Self, String> {
        let mut child = Command::new(deno)
            .args(["lsp", "--quiet"])
            .current_dir(cwd)
            .env("DENO_NO_UPDATE_CHECK", "1")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| format!("Deno LSP failed to start: {e}"))?;
        let input = child.stdin.take().ok_or("Deno LSP stdin missing")?;
        let output = child.stdout.take().ok_or("Deno LSP stdout missing")?;
        let mut stderr = child.stderr.take().ok_or("Deno LSP stderr missing")?;
        let stderr = tokio::spawn(async move {
            let mut tail = Vec::new();
            let mut buffer = [0; 4096];
            while let Ok(count) = stderr.read(&mut buffer).await {
                if count == 0 {
                    break;
                }
                tail.extend_from_slice(&buffer[..count]);
                if tail.len() > RESPONSE_BUDGET {
                    tail.drain(..tail.len() - RESPONSE_BUDGET);
                }
            }
            tail
        });
        Ok(Self {
            child,
            input,
            output,
            stderr,
            next_id: 0,
        })
    }

    async fn send(&mut self, message: Value) -> Result<(), String> {
        let body = serde_json::to_vec(&message).map_err(|e| e.to_string())?;
        if body.len() > MAX_MESSAGE {
            return Err("Deno LSP request exceeds message budget".into());
        }
        self.input
            .write_all(format!("Content-Length: {}\r\n\r\n", body.len()).as_bytes())
            .await
            .map_err(|e| e.to_string())?;
        self.input
            .write_all(&body)
            .await
            .map_err(|e| e.to_string())?;
        self.input.flush().await.map_err(|e| e.to_string())
    }

    async fn notify(&mut self, method: &str, params: Value) -> Result<(), String> {
        let mut message = json!({"jsonrpc":"2.0","method":method});
        if !params.is_null() {
            message["params"] = params;
        }
        self.send(message).await
    }

    async fn request(&mut self, method: &str, params: Value) -> Result<Value, String> {
        self.next_id += 1;
        let id = self.next_id;
        let mut message = json!({"jsonrpc":"2.0","id":id,"method":method});
        if !params.is_null() {
            message["params"] = params;
        }
        self.send(message).await?;
        loop {
            let message = self.receive().await?;
            if let Some(method) = message["method"].as_str() {
                if !message["id"].is_null() {
                    let result = if method == "workspace/configuration" {
                        json!(
                            message["params"]["items"]
                                .as_array()
                                .map(|items| items
                                    .iter()
                                    .map(|_| json!({"enable":true}))
                                    .collect::<Vec<_>>())
                                .unwrap_or_default()
                        )
                    } else {
                        Value::Null
                    };
                    self.send(json!({"jsonrpc":"2.0","id":message["id"],"result":result}))
                        .await?;
                }
            } else if message["id"] == id {
                if !message["error"].is_null() {
                    return Err(format!(
                        "Deno LSP request failed: {}",
                        bound_text(&message["error"].to_string(), TEXT_BUDGET)
                    ));
                }
                return message
                    .get("result")
                    .cloned()
                    .ok_or("Deno LSP response missing result".into());
            }
        }
    }

    async fn receive(&mut self) -> Result<Value, String> {
        let mut header = Vec::new();
        loop {
            header.push(
                self.output
                    .read_u8()
                    .await
                    .map_err(|e| format!("Deno LSP output failed: {e}"))?,
            );
            if header.len() > 8192 {
                return Err("Deno LSP returned an oversized header".into());
            }
            if header.ends_with(b"\r\n\r\n") {
                break;
            }
        }
        let header = std::str::from_utf8(&header).map_err(|e| e.to_string())?;
        let length = header
            .lines()
            .find_map(|line| {
                let (key, value) = line.split_once(':')?;
                key.eq_ignore_ascii_case("Content-Length")
                    .then(|| value.trim().parse::<usize>().ok())
                    .flatten()
            })
            .filter(|length| *length <= MAX_MESSAGE)
            .ok_or("Deno LSP returned an invalid message length")?;
        let mut body = vec![0; length];
        self.output
            .read_exact(&mut body)
            .await
            .map_err(|e| e.to_string())?;
        let message: Value = serde_json::from_slice(&body).map_err(|e| e.to_string())?;
        if message["jsonrpc"] != "2.0" || !message.is_object() {
            return Err("Deno LSP returned invalid JSON-RPC".into());
        }
        Ok(message)
    }

    async fn shutdown(&mut self) {
        let _ = tokio::time::timeout(Duration::from_secs(1), async {
            let _ = self.request("shutdown", Value::Null).await;
            let _ = self.notify("exit", Value::Null).await;
            let _ = self.child.wait().await;
        })
        .await;
        let _ = self.child.start_kill();
        let _ = tokio::time::timeout(Duration::from_secs(1), self.child.wait()).await;
        self.stderr.abort();
    }
}

impl Drop for Lsp {
    fn drop(&mut self) {
        self.stderr.abort();
    }
}

#[cfg(test)]
#[path = "diagnostics_tests.rs"]
mod tests;
