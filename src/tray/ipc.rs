// Unix domain socket IPC between mirage daemon and tray.
//
// The daemon runs an `IpcServer` that listens on `socket_path()`.
// Each connection carries exactly one JSON request line and receives one JSON
// response line back.  The tray (or any CLI tool) uses `send_request` to talk
// to the running daemon.

use std::io::{BufRead, BufReader, BufWriter, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};

use serde::{Deserialize, Serialize};

use crate::db::models::SyncState;
use crate::error::{Error, Result};

// ── Protocol types ────────────────────────────────────────────────────────────

/// Requests that the tray (or a CLI tool) can send to the daemon.
#[derive(Debug, Serialize, Deserialize)]
pub enum TrayRequest {
    GetStatus,
    Quit,
}

/// Responses the daemon sends back.
#[derive(Debug, Serialize, Deserialize)]
pub enum TrayResponse {
    Status(StatusInfo),
    Ok,
    Error(String),
}

/// A point-in-time snapshot of the daemon's sync status.
#[derive(Debug, Serialize, Deserialize)]
pub struct StatusInfo {
    pub online: bool,
    pub synced: u64,
    pub pending: u64,
    pub conflicts: u64,
}

// ── Socket path ───────────────────────────────────────────────────────────────

/// Returns the path of the Unix domain socket used by the daemon.
///
/// Uses `$XDG_RUNTIME_DIR/mirage.sock` when the environment variable is set,
/// falling back to `/tmp/mirage.sock`.
pub fn socket_path() -> PathBuf {
    if let Ok(runtime_dir) = std::env::var("XDG_RUNTIME_DIR") {
        PathBuf::from(runtime_dir).join("mirage.sock")
    } else {
        PathBuf::from("/tmp/mirage.sock")
    }
}

// ── IPC Server ────────────────────────────────────────────────────────────────

/// Listens on the Unix domain socket and dispatches `TrayRequest` messages.
pub struct IpcServer {
    listener: UnixListener,
    db: crate::db::Database,
    net_state: Arc<AtomicU8>,
    path: PathBuf,
}

impl IpcServer {
    /// Creates a new `IpcServer`.
    ///
    /// Removes any stale socket file at `socket_path()` before binding so that
    /// a crashed daemon does not block the next startup.
    ///
    /// `net_state` is an atomic byte shared with the network monitor.  A value
    /// of `0` means the daemon considers itself online.
    pub fn new(db: crate::db::Database, net_state: Arc<AtomicU8>) -> Result<Self> {
        let path = socket_path();

        // Remove a stale socket so `bind` does not fail.
        if path.exists() {
            std::fs::remove_file(&path).map_err(Error::Io)?;
        }

        let listener = UnixListener::bind(&path).map_err(Error::Io)?;
        tracing::info!(socket = %path.display(), "IPC server listening");

        Ok(Self {
            listener,
            db,
            net_state,
            path,
        })
    }

    /// Accepts connections in a loop.
    ///
    /// Each connection is handled synchronously: one request is read, one
    /// response is written, then the connection is closed.  `Quit` breaks the
    /// accept loop and returns.
    pub fn run(&self) {
        for stream in self.listener.incoming() {
            match stream {
                Err(e) => {
                    tracing::warn!(error = %e, "IPC accept error");
                }
                Ok(stream) => {
                    let quit = self.handle_connection(stream);
                    if quit {
                        tracing::info!("IPC server received Quit — shutting down");
                        break;
                    }
                }
            }
        }
    }

    /// Handle a single client connection.
    ///
    /// Returns `true` when the daemon should stop accepting new connections.
    fn handle_connection(&self, stream: UnixStream) -> bool {
        let reader_stream = match stream.try_clone() {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!(error = %e, "IPC: failed to clone stream");
                return false;
            }
        };

        let mut reader = BufReader::new(reader_stream);
        let mut writer = BufWriter::new(&stream);

        let mut line = String::new();
        if let Err(e) = reader.read_line(&mut line) {
            tracing::warn!(error = %e, "IPC: failed to read request");
            return false;
        }

        let request: TrayRequest = match serde_json::from_str(line.trim()) {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!(error = %e, "IPC: failed to parse request");
                let _ = self.write_response(&mut writer, &TrayResponse::Error(e.to_string()));
                return false;
            }
        };

        match request {
            TrayRequest::GetStatus => {
                let response = self.build_status_response();
                let _ = self.write_response(&mut writer, &response);
                false
            }
            TrayRequest::Quit => {
                let _ = self.write_response(&mut writer, &TrayResponse::Ok);
                true
            }
        }
    }

    /// Serialize `response` as a JSON line and flush.
    fn write_response<W: Write>(&self, writer: &mut BufWriter<W>, response: &TrayResponse) -> bool {
        match serde_json::to_string(response) {
            Err(e) => {
                tracing::warn!(error = %e, "IPC: failed to serialize response");
                false
            }
            Ok(json) => {
                if let Err(e) = writeln!(writer, "{json}") {
                    tracing::warn!(error = %e, "IPC: failed to write response");
                    return false;
                }
                if let Err(e) = writer.flush() {
                    tracing::warn!(error = %e, "IPC: failed to flush response");
                    return false;
                }
                true
            }
        }
    }

    /// Query the database and build a `TrayResponse::Status`.
    fn build_status_response(&self) -> TrayResponse {
        let synced = match self.db.count_by_sync_state(SyncState::Synced) {
            Ok(n) => n,
            Err(e) => {
                tracing::warn!(error = %e, "IPC: db query failed (synced)");
                return TrayResponse::Error(e.to_string());
            }
        };

        let pending_up = match self.db.count_by_sync_state(SyncState::PendingUpload) {
            Ok(n) => n,
            Err(e) => {
                tracing::warn!(error = %e, "IPC: db query failed (pending_upload)");
                return TrayResponse::Error(e.to_string());
            }
        };

        let pending_down = match self.db.count_by_sync_state(SyncState::PendingDownload) {
            Ok(n) => n,
            Err(e) => {
                tracing::warn!(error = %e, "IPC: db query failed (pending_download)");
                return TrayResponse::Error(e.to_string());
            }
        };

        let conflicts = match self.db.count_by_sync_state(SyncState::Conflict) {
            Ok(n) => n,
            Err(e) => {
                tracing::warn!(error = %e, "IPC: db query failed (conflicts)");
                return TrayResponse::Error(e.to_string());
            }
        };

        let online = self.net_state.load(Ordering::Relaxed) == 0;

        TrayResponse::Status(StatusInfo {
            online,
            synced,
            pending: pending_up + pending_down,
            conflicts,
        })
    }
}

impl Drop for IpcServer {
    fn drop(&mut self) {
        if self.path.exists()
            && let Err(e) = std::fs::remove_file(&self.path)
        {
            tracing::warn!(
                error = %e,
                socket = %self.path.display(),
                "IPC: failed to remove socket on drop"
            );
        }
    }
}

// ── IPC Client ────────────────────────────────────────────────────────────────

/// Connect to the daemon's socket, send `req`, and return the parsed response.
///
/// All I/O errors are mapped to `String` so that tray code does not need to
/// depend on the daemon's error type.
pub fn send_request(req: &TrayRequest) -> std::result::Result<TrayResponse, String> {
    let path = socket_path();
    let stream = UnixStream::connect(&path)
        .map_err(|e| format!("failed to connect to {}: {e}", path.display()))?;

    let reader_stream = stream
        .try_clone()
        .map_err(|e| format!("failed to clone stream: {e}"))?;

    let mut writer = BufWriter::new(&stream);
    let mut reader = BufReader::new(reader_stream);

    let json = serde_json::to_string(req).map_err(|e| format!("serialize error: {e}"))?;
    writeln!(writer, "{json}").map_err(|e| format!("write error: {e}"))?;
    writer.flush().map_err(|e| format!("flush error: {e}"))?;

    let mut line = String::new();
    reader
        .read_line(&mut line)
        .map_err(|e| format!("read error: {e}"))?;

    let response: TrayResponse =
        serde_json::from_str(line.trim()).map_err(|e| format!("deserialize error: {e}"))?;

    Ok(response)
}
