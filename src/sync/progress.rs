// Shared sync progress state for IPC reporting.

use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};

/// Current phase of the sync process.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum SyncPhase {
    Idle,
    Scanning,
    Downloading,
    Uploading,
}

/// Progress information for an ongoing sync operation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProgressInfo {
    pub phase: SyncPhase,
    pub current_file: Option<String>,
    pub files_done: u64,
    pub files_total: u64,
    pub bytes_done: u64,
    pub bytes_total: u64,
}

impl Default for ProgressInfo {
    fn default() -> Self {
        Self {
            phase: SyncPhase::Idle,
            current_file: None,
            files_done: 0,
            files_total: 0,
            bytes_done: 0,
            bytes_total: 0,
        }
    }
}

/// Thread-safe handle to shared sync progress.
#[derive(Debug, Clone)]
pub struct SyncProgress {
    inner: Arc<Mutex<ProgressInfo>>,
}

impl Default for SyncProgress {
    fn default() -> Self {
        Self::new()
    }
}

impl SyncProgress {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(ProgressInfo::default())),
        }
    }

    pub fn set_idle(&self) {
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        *guard = ProgressInfo::default();
    }

    pub fn set_scanning(&self) {
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        guard.phase = SyncPhase::Scanning;
        guard.current_file = None;
    }

    pub fn set_downloading(&self, file: &str, done: u64, total: u64) {
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        guard.phase = SyncPhase::Downloading;
        guard.current_file = Some(file.to_owned());
        guard.files_done = done;
        guard.files_total = total;
    }

    pub fn set_uploading(&self, file: &str, done: u64, total: u64) {
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        guard.phase = SyncPhase::Uploading;
        guard.current_file = Some(file.to_owned());
        guard.files_done = done;
        guard.files_total = total;
    }

    pub fn set_bytes(&self, done: u64, total: u64) {
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        guard.bytes_done = done;
        guard.bytes_total = total;
    }

    pub fn snapshot(&self) -> ProgressInfo {
        self.inner.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
}
