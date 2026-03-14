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
    GetProgress,
    Quit,
}

/// Responses the daemon sends back.
#[derive(Debug, Serialize, Deserialize)]
pub enum TrayResponse {
    Status(StatusInfo),
    Progress(crate::sync::progress::ProgressInfo),
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
    progress: Option<crate::sync::progress::SyncProgress>,
}

impl IpcServer {
    /// Creates a new `IpcServer`.
    ///
    /// Removes any stale socket file at `socket_path()` before binding so that
    /// a crashed daemon does not block the next startup.
    ///
    /// `net_state` is an atomic byte shared with the network monitor.  A value
    /// of `0` means the daemon considers itself online.
    pub fn new(
        db: crate::db::Database,
        net_state: Arc<AtomicU8>,
        progress: Option<crate::sync::progress::SyncProgress>,
    ) -> Result<Self> {
        let path = socket_path();

        // Remove a stale socket so `bind` does not fail.
        if path.exists() {
            std::fs::remove_file(&path).map_err(Error::Io)?;
        }

        let listener = UnixListener::bind(&path).map_err(Error::Io)?;

        // Restrict socket permissions to owner only
        {
            use std::os::unix::fs::PermissionsExt;
            let perms = std::fs::Permissions::from_mode(0o600);
            std::fs::set_permissions(&path, perms).map_err(Error::Io)?;
        }

        tracing::info!(socket = %path.display(), "IPC server listening");

        Ok(Self {
            listener,
            db,
            net_state,
            path,
            progress,
        })
    }

    /// Accepts connections in a loop.
    ///
    /// Each connection is handled synchronously: one request is read, one
    /// response is written, then the connection is closed.  `Quit` breaks the
    /// accept loop and returns.
    pub fn run(&self) {
        let mut consecutive_errors: u32 = 0;
        for stream in self.listener.incoming() {
            match stream {
                Err(e) => {
                    tracing::warn!(error = %e, "IPC accept error");
                    consecutive_errors += 1;
                    let backoff_ms =
                        std::cmp::min(10u64 * 2u64.saturating_pow(consecutive_errors), 5000);
                    std::thread::sleep(std::time::Duration::from_millis(backoff_ms));
                }
                Ok(stream) => {
                    consecutive_errors = 0;
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
            TrayRequest::GetProgress => {
                let response = match &self.progress {
                    Some(p) => TrayResponse::Progress(p.snapshot()),
                    None => TrayResponse::Progress(crate::sync::progress::ProgressInfo::default()),
                };
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_request_serialization() {
        let req = TrayRequest::GetStatus;
        let json = serde_json::to_string(&req).unwrap();
        let parsed: TrayRequest = serde_json::from_str(&json).unwrap();
        assert!(matches!(parsed, TrayRequest::GetStatus));
    }

    #[test]
    fn progress_request_serialization() {
        let req = TrayRequest::GetProgress;
        let json = serde_json::to_string(&req).unwrap();
        let parsed: TrayRequest = serde_json::from_str(&json).unwrap();
        assert!(matches!(parsed, TrayRequest::GetProgress));
    }

    #[test]
    fn quit_request_serialization() {
        let req = TrayRequest::Quit;
        let json = serde_json::to_string(&req).unwrap();
        let parsed: TrayRequest = serde_json::from_str(&json).unwrap();
        assert!(matches!(parsed, TrayRequest::Quit));
    }

    #[test]
    fn status_response_serialization() {
        let resp = TrayResponse::Status(StatusInfo {
            online: true,
            synced: 10,
            pending: 2,
            conflicts: 1,
        });
        let json = serde_json::to_string(&resp).unwrap();
        let parsed: TrayResponse = serde_json::from_str(&json).unwrap();
        match parsed {
            TrayResponse::Status(info) => {
                assert!(info.online);
                assert_eq!(info.synced, 10);
                assert_eq!(info.pending, 2);
                assert_eq!(info.conflicts, 1);
            }
            _ => panic!("expected Status response"),
        }
    }

    #[test]
    fn progress_response_serialization() {
        let info = crate::sync::progress::ProgressInfo {
            phase: crate::sync::progress::SyncPhase::Downloading,
            current_file: Some("test.txt".to_owned()),
            files_done: 5,
            files_total: 10,
            bytes_done: 1024,
            bytes_total: 2048,
        };
        let resp = TrayResponse::Progress(info);
        let json = serde_json::to_string(&resp).unwrap();
        let parsed: TrayResponse = serde_json::from_str(&json).unwrap();
        match parsed {
            TrayResponse::Progress(p) => {
                assert_eq!(p.phase, crate::sync::progress::SyncPhase::Downloading);
                assert_eq!(p.current_file.as_deref(), Some("test.txt"));
                assert_eq!(p.files_done, 5);
                assert_eq!(p.bytes_total, 2048);
            }
            _ => panic!("expected Progress response"),
        }
    }

    #[test]
    fn error_response_serialization() {
        let resp = TrayResponse::Error("something went wrong".to_owned());
        let json = serde_json::to_string(&resp).unwrap();
        let parsed: TrayResponse = serde_json::from_str(&json).unwrap();
        match parsed {
            TrayResponse::Error(msg) => assert_eq!(msg, "something went wrong"),
            _ => panic!("expected Error response"),
        }
    }

    #[test]
    fn malformed_request_parsing() {
        let result: std::result::Result<TrayRequest, _> = serde_json::from_str("not json");
        assert!(result.is_err());
    }

    #[test]
    fn ipc_server_handles_status_request() {
        let tmp = tempfile::tempdir().unwrap();
        let sock_path = tmp.path().join("test.sock");

        let net_state = Arc::new(AtomicU8::new(0));

        let listener = UnixListener::bind(&sock_path).unwrap();
        let net_clone = Arc::clone(&net_state);

        let handle = std::thread::spawn(move || {
            let _net = net_clone;
            let (stream, _) = listener.accept().unwrap();
            let reader_stream = stream.try_clone().unwrap();
            let mut reader = BufReader::new(reader_stream);
            let mut writer = BufWriter::new(&stream);

            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            let _req: TrayRequest = serde_json::from_str(line.trim()).unwrap();

            let response = TrayResponse::Status(StatusInfo {
                online: true,
                synced: 0,
                pending: 0,
                conflicts: 0,
            });
            let json = serde_json::to_string(&response).unwrap();
            writeln!(writer, "{json}").unwrap();
            writer.flush().unwrap();
        });

        // Client side
        let stream = UnixStream::connect(&sock_path).unwrap();
        let reader_stream = stream.try_clone().unwrap();
        let mut writer = BufWriter::new(&stream);
        let mut reader = BufReader::new(reader_stream);

        let req = serde_json::to_string(&TrayRequest::GetStatus).unwrap();
        writeln!(writer, "{req}").unwrap();
        writer.flush().unwrap();

        let mut resp_line = String::new();
        reader.read_line(&mut resp_line).unwrap();
        let resp: TrayResponse = serde_json::from_str(resp_line.trim()).unwrap();

        match resp {
            TrayResponse::Status(info) => assert!(info.online),
            _ => panic!("expected Status"),
        }

        handle.join().unwrap();
    }

    #[test]
    fn ipc_server_handles_progress_request() {
        let tmp = tempfile::tempdir().unwrap();
        let sock_path = tmp.path().join("test_progress.sock");

        let progress = crate::sync::progress::SyncProgress::new();
        progress.set_scanning();

        let listener = UnixListener::bind(&sock_path).unwrap();
        let progress_clone = progress.clone();

        let handle = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let reader_stream = stream.try_clone().unwrap();
            let mut reader = BufReader::new(reader_stream);
            let mut writer = BufWriter::new(&stream);

            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            let _req: TrayRequest = serde_json::from_str(line.trim()).unwrap();

            let response = TrayResponse::Progress(progress_clone.snapshot());
            let json = serde_json::to_string(&response).unwrap();
            writeln!(writer, "{json}").unwrap();
            writer.flush().unwrap();
        });

        let stream = UnixStream::connect(&sock_path).unwrap();
        let reader_stream = stream.try_clone().unwrap();
        let mut writer = BufWriter::new(&stream);
        let mut reader = BufReader::new(reader_stream);

        let req = serde_json::to_string(&TrayRequest::GetProgress).unwrap();
        writeln!(writer, "{req}").unwrap();
        writer.flush().unwrap();

        let mut resp_line = String::new();
        reader.read_line(&mut resp_line).unwrap();
        let resp: TrayResponse = serde_json::from_str(resp_line.trim()).unwrap();

        match resp {
            TrayResponse::Progress(info) => {
                assert_eq!(info.phase, crate::sync::progress::SyncPhase::Scanning);
            }
            _ => panic!("expected Progress"),
        }

        handle.join().unwrap();
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
