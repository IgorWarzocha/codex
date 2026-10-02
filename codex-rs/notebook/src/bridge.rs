use std::sync::Arc;
use std::time::Duration;

use axum::Json;
use axum::Router;
use axum::extract::DefaultBodyLimit;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::http::StatusCode;
use axum::middleware;
use axum::routing::post;
use codex_code_mode_protocol::CellId;
use codex_code_mode_protocol::CodeModeNestedToolCall;
use codex_code_mode_protocol::FunctionCallOutputContentItem;
use serde::Deserialize;
use serde_json::Value;
use serde_json::json;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::cell::MAX_PENDING_BYTES;
use crate::session::Registry;

pub(crate) struct Bridge {
    pub(crate) endpoint: String,
    pub(crate) credential: String,
    stop: CancellationToken,
    task: JoinHandle<Result<(), std::io::Error>>,
}

#[derive(Clone)]
struct BridgeState {
    registry: Arc<Registry>,
    credential: String,
}

#[derive(Deserialize)]
struct Request {
    cell_id: CellId,
    #[serde(flatten)]
    operation: Operation,
}

#[derive(Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
enum Operation {
    Output { item: FunctionCallOutputContentItem },
    Tool { name: String, input: Option<Value> },
    Notify { text: String },
    Yield,
}

impl Bridge {
    pub(crate) async fn start(registry: Arc<Registry>) -> Result<Self, String> {
        let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .map_err(|e| format!("start notebook bridge: {e}"))?;
        let address = listener.local_addr().map_err(|e| e.to_string())?;
        let credential = Uuid::new_v4().to_string();
        let stop = CancellationToken::new();
        let request_stop = stop.clone();
        let router = Router::new()
            .route("/rpc", post(rpc))
            .layer(DefaultBodyLimit::max(MAX_PENDING_BYTES))
            // Includes body extraction, so slow connections cannot outlive session shutdown.
            .layer(middleware::from_fn(move |request, next: middleware::Next| {
                let stop = request_stop.clone();
                async move {
                    tokio::select! {
                        response = next.run(request) => response,
                        _ = stop.cancelled() => axum::response::IntoResponse::into_response(StatusCode::SERVICE_UNAVAILABLE),
                    }
                }
            }))
            .with_state(BridgeState { registry, credential: credential.clone() });
        let shutdown = stop.clone();
        let task = tokio::spawn(async move {
            axum::serve(listener, router)
                .with_graceful_shutdown(shutdown.cancelled_owned())
                .await
        });
        Ok(Self {
            endpoint: format!("http://{address}/rpc"),
            credential,
            stop,
            task,
        })
    }

    pub(crate) async fn shutdown(&mut self) -> Result<(), String> {
        self.stop.cancel();
        match tokio::time::timeout(Duration::from_secs(3), &mut self.task).await {
            Ok(Ok(Ok(()))) => Ok(()),
            Ok(Ok(Err(error))) => Err(format!("notebook bridge failed: {error}")),
            Ok(Err(error)) => Err(format!("notebook bridge task failed: {error}")),
            Err(_) => {
                self.task.abort();
                let _ = (&mut self.task).await;
                Err("notebook bridge shutdown timed out".to_string())
            }
        }
    }
}

impl Drop for Bridge {
    fn drop(&mut self) {
        self.stop.cancel();
        self.task.abort();
    }
}

async fn rpc(
    State(state): State<BridgeState>,
    headers: HeaderMap,
    Json(request): Json<Request>,
) -> (StatusCode, Json<Value>) {
    if headers
        .get("authorization")
        .and_then(|value| value.to_str().ok())
        != Some(&format!("Bearer {}", state.credential))
    {
        return failure(
            StatusCode::UNAUTHORIZED,
            "invalid notebook bridge credential",
        );
    }
    let result = dispatch(&state.registry, request).await;
    match result {
        Ok(value) => (StatusCode::OK, Json(json!({ "ok": true, "value": value }))),
        Err(error) => failure(StatusCode::BAD_REQUEST, &error),
    }
}

fn failure(status: StatusCode, error: &str) -> (StatusCode, Json<Value>) {
    (status, Json(json!({ "ok": false, "error": error })))
}

async fn dispatch(registry: &Registry, request: Request) -> Result<Value, String> {
    let cell = registry.running_cell(&request.cell_id)?;
    match request.operation {
        Operation::Output { item } => {
            match &item {
                FunctionCallOutputContentItem::InputImage { image_url, .. }
                    if !image_url.starts_with("data:image/") =>
                {
                    return Err("only data:image URLs are supported".to_string());
                }
                FunctionCallOutputContentItem::InputAudio { .. } => {
                    return Err("audio is unsupported in notebook sessions".to_string());
                }
                _ => {}
            }
            cell.push(item);
            Ok(Value::Null)
        }
        Operation::Yield => {
            cell.yield_now();
            Ok(Value::Null)
        }
        Operation::Notify { text } => {
            tokio::select! {
                biased;
                _ = cell.cancellation.cancelled() => Err("notebook cell closed".to_string()),
                result = cell.delegate.notify(cell.call_id.clone(), cell.id.clone(), text, cell.cancellation.child_token()) => result.map(|()| Value::Null),
            }
        }
        Operation::Tool { name, input } => {
            let definition = cell
                .tools
                .get(&name)
                .ok_or_else(|| format!("tool is not enabled for this cell: {name}"))?;
            let invocation = CodeModeNestedToolCall {
                cell_id: cell.id.clone(),
                runtime_tool_call_id: Uuid::new_v4().to_string(),
                tool_name: definition.tool_name.clone(),
                tool_kind: definition.kind,
                input,
            };
            tokio::select! {
                biased;
                _ = cell.cancellation.cancelled() => Err("notebook cell closed".to_string()),
                result = cell.delegate.invoke_tool(invocation, cell.cancellation.child_token()) => result,
            }
        }
    }
}
