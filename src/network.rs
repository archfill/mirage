use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};

use tracing::info;

const ONLINE: u8 = 0;
const OFFLINE: u8 = 1;

/// Tracks whether the backend is reachable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NetworkState {
    Online,
    Offline,
}

/// Lock-free network state monitor shared across threads.
///
/// Uses `AtomicU8` so FUSE callbacks can read without contention.
pub struct NetworkMonitor {
    state: Arc<AtomicU8>,
}

impl NetworkMonitor {
    pub fn new() -> Self {
        Self {
            state: Arc::new(AtomicU8::new(ONLINE)),
        }
    }

    pub fn state(&self) -> NetworkState {
        if self.state.load(Ordering::Relaxed) == ONLINE {
            NetworkState::Online
        } else {
            NetworkState::Offline
        }
    }

    pub fn set_online(&self) {
        let prev = self.state.swap(ONLINE, Ordering::Relaxed);
        if prev == OFFLINE {
            info!("network state changed: offline -> online");
        }
    }

    pub fn set_offline(&self) {
        let prev = self.state.swap(OFFLINE, Ordering::Relaxed);
        if prev == ONLINE {
            info!("network state changed: online -> offline");
        }
    }

    /// Get a shared handle to the underlying atomic for cross-thread sharing.
    pub fn shared(&self) -> Arc<AtomicU8> {
        Arc::clone(&self.state)
    }
}

impl Default for NetworkMonitor {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn starts_online() {
        let mon = NetworkMonitor::new();
        assert_eq!(mon.state(), NetworkState::Online);
    }

    #[test]
    fn transition_to_offline_and_back() {
        let mon = NetworkMonitor::new();
        mon.set_offline();
        assert_eq!(mon.state(), NetworkState::Offline);
        mon.set_online();
        assert_eq!(mon.state(), NetworkState::Online);
    }

    #[test]
    fn shared_handle_reflects_changes() {
        let mon = NetworkMonitor::new();
        let shared = mon.shared();
        mon.set_offline();
        assert_eq!(shared.load(Ordering::Relaxed), OFFLINE);
        mon.set_online();
        assert_eq!(shared.load(Ordering::Relaxed), ONLINE);
    }
}
