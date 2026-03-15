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
    pub progress: Arc<Mutex<crate::sync::progress::ProgressInfo>>,
    pub icon_state: TrayIconState,
}

impl ksni::Tray for MirageTray {
    fn icon_name(&self) -> String {
        "mirage".to_string()
    }

    fn overlay_icon_name(&self) -> String {
        match self.icon_state {
            TrayIconState::Idle => String::new(),
            TrayIconState::Syncing => "emblem-synchronizing".to_string(),
            TrayIconState::Offline => "network-offline".to_string(),
            TrayIconState::Error => "emblem-error".to_string(),
        }
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

        let progress_label = {
            let p = self.progress.lock().unwrap_or_else(|e| e.into_inner());
            match p.phase {
                crate::sync::progress::SyncPhase::Idle => None,
                crate::sync::progress::SyncPhase::Scanning => Some("Scanning...".to_string()),
                _ => {
                    let file_info = if p.files_total > 0 {
                        format!("{}/{} files", p.files_done, p.files_total)
                    } else {
                        String::new()
                    };
                    let byte_info = if p.bytes_total > 0 {
                        format!(
                            " ({} / {})",
                            format_bytes_short(p.bytes_done),
                            format_bytes_short(p.bytes_total)
                        )
                    } else {
                        String::new()
                    };
                    let phase = match p.phase {
                        crate::sync::progress::SyncPhase::Downloading => "Downloading",
                        crate::sync::progress::SyncPhase::Uploading => "Uploading",
                        _ => "Syncing",
                    };
                    Some(format!("{}: {}{}", phase, file_info, byte_info))
                }
            }
        };

        let mut items = vec![ksni::MenuItem::Standard(ksni::menu::StandardItem {
            label: status_label,
            enabled: false,
            ..Default::default()
        })];

        if let Some(label) = progress_label {
            items.push(ksni::MenuItem::Standard(ksni::menu::StandardItem {
                label,
                enabled: false,
                ..Default::default()
            }));
        }

        items.push(ksni::MenuItem::Separator);
        items.push(ksni::MenuItem::Standard(ksni::menu::StandardItem {
            label: "Quit".to_string(),
            activate: Box::new(|_this: &mut MirageTray| {
                std::process::exit(0);
            }),
            ..Default::default()
        }));

        items
    }
}

fn format_bytes_short(bytes: u64) -> String {
    const GB: u64 = 1_073_741_824;
    const MB: u64 = 1_048_576;
    const KB: u64 = 1_024;
    if bytes >= GB {
        format!("{:.1} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.1} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.1} KB", bytes as f64 / KB as f64)
    } else {
        format!("{bytes} B")
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

fn try_get_progress() -> Option<crate::sync::progress::ProgressInfo> {
    match ipc::send_request(&ipc::TrayRequest::GetProgress) {
        Ok(ipc::TrayResponse::Progress(info)) => Some(info),
        _ => None,
    }
}

/// Start the system tray icon.
///
/// Checks that the mirage daemon is running, then creates and registers the tray
/// icon. A background thread polls the daemon every 10 seconds to refresh status
/// and raises a desktop notification when new conflicts are detected.
pub fn run_tray() -> Result<()> {
    let initial_status = try_get_status()
        .ok_or_else(|| Error::Config("mirage daemon is not running".to_string()))?;

    let status = Arc::new(Mutex::new(initial_status));
    let progress = Arc::new(Mutex::new(crate::sync::progress::ProgressInfo::default()));

    let tray = MirageTray {
        status: Arc::clone(&status),
        progress: Arc::clone(&progress),
        icon_state: TrayIconState::Idle,
    };

    let service = ksni::TrayService::new(tray);
    let handle = service.handle();
    service.spawn();

    let poll_status = Arc::clone(&status);
    let poll_progress = Arc::clone(&progress);
    let poll_handle = std::thread::spawn(move || {
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
                let is_online = new_status.online;
                let has_conflicts = new_status.conflicts > 0;

                {
                    let mut guard = poll_status.lock().unwrap_or_else(|e| e.into_inner());
                    *guard = new_status;
                }

                if new_conflicts > prev_conflicts {
                    let _ = notify_rust::Notification::new()
                        .summary("Mirage")
                        .body("New conflict detected")
                        .icon("mirage")
                        .show();
                }

                prev_conflicts = new_conflicts;

                // Update overlay icon based on status
                let new_icon_state = if has_conflicts {
                    TrayIconState::Error
                } else if !is_online {
                    TrayIconState::Offline
                } else {
                    TrayIconState::Idle
                };
                handle.update(|tray| {
                    tray.icon_state = new_icon_state;
                });
            } else {
                // Daemon not reachable
                handle.update(|tray| {
                    tray.icon_state = TrayIconState::Offline;
                });
            }

            if let Some(new_progress) = try_get_progress() {
                let is_syncing = new_progress.phase != crate::sync::progress::SyncPhase::Idle;
                {
                    let mut guard = poll_progress.lock().unwrap_or_else(|e| e.into_inner());
                    *guard = new_progress;
                }
                if is_syncing {
                    handle.update(|tray| {
                        tray.icon_state = TrayIconState::Syncing;
                    });
                }
            }
        }
    });

    // Block until the polling thread exits (which only happens on process exit).
    let _ = poll_handle.join();

    Ok(())
}
