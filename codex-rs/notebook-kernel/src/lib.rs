//! Persistent Deno Jupyter execution. Deno's Jupyter kernel has allow-all permissions.
//! This controller is not a sandbox. Tool bridges belong to the caller.

mod kernel;
mod output;
mod process;

pub use kernel::Kernel;
pub use output::ExecutionError;
pub use output::ExecutionResult;
pub use output::ExecutionStatus;
pub use output::Output;
pub use output::StreamName;

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::PathBuf;
use std::time::Duration;

pub const DEFAULT_MAX_HEAP_MIB: u32 = 4096;

#[derive(Clone, Debug)]
pub struct KernelOptions {
    pub deno: PathBuf,
    pub cwd: Option<PathBuf>,
    /// Overrides inherited environment variables.
    pub env: BTreeMap<OsString, OsString>,
    pub max_heap_mib: Option<u32>,
    pub startup_timeout: Duration,
    pub execute_timeout: Duration,
    pub shutdown_timeout: Duration,
    pub max_output_bytes: usize,
}

impl Default for KernelOptions {
    fn default() -> Self {
        Self {
            deno: PathBuf::from("deno"),
            cwd: None,
            env: BTreeMap::new(),
            max_heap_mib: Some(DEFAULT_MAX_HEAP_MIB),
            startup_timeout: Duration::from_secs(30),
            execute_timeout: Duration::from_secs(3600),
            shutdown_timeout: Duration::from_secs(2),
            max_output_bytes: 4 * 1024 * 1024,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum KernelError {
    #[error("Deno notebook kernels require Unix process groups or Windows job objects")]
    UnsupportedPlatform,
    #[error("Deno kernel I/O failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("Deno kernel protocol failed: {0}")]
    Protocol(#[from] jupyter_zmq_client::RuntimeError),
    #[error("Deno kernel JSON failed: {0}")]
    Json(#[from] serde_json::Error),
    #[error("Deno kernel {0} timed out")]
    Timeout(&'static str),
    #[error("Deno kernel execution cancelled; kernel terminated")]
    Cancelled,
    #[error("Deno kernel is closed")]
    Closed,
    #[error("Deno kernel exited: {status}\n{stderr}")]
    Exited { status: String, stderr: String },
    #[error("Deno kernel returned unexpected reply: {0}")]
    UnexpectedReply(String),
    #[error("Invalid Deno kernel options: {0}")]
    InvalidOptions(&'static str),
}
