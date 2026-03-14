pub mod ipc;

use std::sync::{Arc, Mutex};

use crate::error::{Error, Result};

/// Represents the visual state of the tray icon.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TrayIconState {
    Idle,
    Syncing,
    Offline,
    Error,
}

/// System tray icon for Mirage.
pub struct MirageTray {
    pub status: Arc<Mutex<ipc::StatusInfo>>,
    pub icon_state: TrayIconState,
}

impl ksni::Tray for MirageTray {
    fn icon_name(&self) -> String {
        match self.icon_state {
            TrayIconState::Idle => "folder-sync",
            TrayIconState::Syncing => "sync-synchronizing",
            TrayIconState::Offline => "network-offline",
            TrayIconState::Error => "dialog-warning",
        }
        .to_string()
    }

    fn title(&self) -> String {
        "Mirage".to_string()
    }

    fn menu(&self) -> Vec<ksni::MenuItem<Self>> {
        let status_label = {
            let s = self.status.lock().unwrap_or_else(|e| e.into_inner());
            let online_str = if s.online { "Online" } else { "Offline" };
            format!(
                "{}: {} synced, {} pending, {} conflicts",
                online_str, s.synced, s.pending, s.conflicts
            )
        };

        vec![
            ksni::MenuItem::Standard(ksni::menu::StandardItem {
                label: status_label,
                enabled: false,
                ..Default::default()
            }),
            ksni::MenuItem::Separator,
            ksni::MenuItem::Standard(ksni::menu::StandardItem {
                label: "Quit".to_string(),
                activate: Box::new(|_this: &mut MirageTray| {
                    std::process::exit(0);
                }),
                ..Default::default()
            }),
        ]
    }
}

/// Attempt to retrieve status from the running daemon via IPC.
///
/// Returns `None` if the daemon is not reachable.
fn try_get_status() -> Option<ipc::StatusInfo> {
    match ipc::send_request(&ipc::TrayRequest::GetStatus) {
        Ok(ipc::TrayResponse::Status(info)) => Some(info),
        _ => None,
    }
}

/// Start the system tray icon.
///
/// Checks that the mirage daemon is running, then creates and registers the tray
/// icon. A background thread polls the daemon every 10 seconds to refresh status
/// and raises a desktop notification when new conflicts are detected.
pub fn run_tray() -> Result<()> {
    // Verify the daemon is reachable before proceeding.
    let initial_status = try_get_status()
        .ok_or_else(|| Error::Config("mirage daemon is not running".to_string()))?;

    let status = Arc::new(Mutex::new(initial_status));

    let tray = MirageTray {
        status: Arc::clone(&status),
        icon_state: TrayIconState::Idle,
    };

    let service = ksni::TrayService::new(tray);
    let handle = service.handle();
    service.spawn();

    // Background thread: poll IPC every 10 seconds and update shared status.
    let poll_status = Arc::clone(&status);
    std::thread::spawn(move || {
        let mut prev_conflicts: u64 = {
            poll_status
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .conflicts
        };

        loop {
            std::thread::sleep(std::time::Duration::from_secs(10));

            if let Some(new_status) = try_get_status() {
                let new_conflicts = new_status.conflicts;

                {
                    let mut guard = poll_status.lock().unwrap_or_else(|e| e.into_inner());
                    *guard = new_status;
                }

                if new_conflicts > prev_conflicts {
                    let _ = notify_rust::Notification::new()
                        .summary("Mirage")
                        .body("New conflict detected")
                        .icon("dialog-warning")
                        .show();
                }

                prev_conflicts = new_conflicts;
            }
        }
    });

    // Block on the tray service handle.
    handle.shutdown();

    Ok(())
}
