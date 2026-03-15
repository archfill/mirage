// Activity window powered by egui/eframe.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use eframe::egui;

use super::ipc::{self, StatusInfo};
use crate::sync::progress::{ProgressInfo, SyncPhase};

const LOG_LEVELS: &[&str] = &["", "error", "warn", "info", "debug", "trace"];

#[derive(PartialEq)]
enum Tab {
    Status,
    Settings,
}

/// Activity window application.
pub struct MirageApp {
    mount_point: PathBuf,
    window_open: Arc<AtomicBool>,
    status: Option<StatusInfo>,
    progress: Option<ProgressInfo>,
    last_poll: Instant,
    tab: Tab,
    // Settings fields
    settings_sync_interval: String,
    settings_cache_limit_mb: String,
    settings_remote_path: String,
    settings_log_level: usize,
    settings_loaded: bool,
    settings_save_msg: Option<(String, Instant)>,
}

impl MirageApp {
    fn new(mount_point: PathBuf, window_open: Arc<AtomicBool>) -> Self {
        let status = try_get_status();
        let progress = try_get_progress();
        Self {
            mount_point,
            window_open,
            status,
            progress,
            last_poll: Instant::now(),
            tab: Tab::Status,
            settings_sync_interval: String::new(),
            settings_cache_limit_mb: String::new(),
            settings_remote_path: String::new(),
            settings_log_level: 0,
            settings_loaded: false,
            settings_save_msg: None,
        }
    }

    fn with_tab(mount_point: PathBuf, window_open: Arc<AtomicBool>, tab: Tab) -> Self {
        let mut app = Self::new(mount_point, window_open);
        app.tab = tab;
        app
    }

    fn poll_if_needed(&mut self) {
        if self.last_poll.elapsed().as_secs() >= 2 {
            self.status = try_get_status();
            self.progress = try_get_progress();
            self.last_poll = Instant::now();
        }
    }
}

fn try_get_status() -> Option<StatusInfo> {
    match ipc::send_request(&ipc::TrayRequest::GetStatus) {
        Ok(ipc::TrayResponse::Status(info)) => Some(info),
        _ => None,
    }
}

fn try_get_progress() -> Option<ProgressInfo> {
    match ipc::send_request(&ipc::TrayRequest::GetProgress) {
        Ok(ipc::TrayResponse::Progress(info)) => Some(info),
        _ => None,
    }
}

fn format_bytes(bytes: u64) -> String {
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

/// Try to acquire the GUI lock file. Returns the lock path if successful, None if another instance
/// is already running.
fn acquire_gui_lock(name: &str) -> Option<PathBuf> {
    let lock_dir = dirs::cache_dir()
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join("mirage");
    let _ = fs::create_dir_all(&lock_dir);
    let lock_path = lock_dir.join(format!("{}.lock", name));

    // Check if another GUI instance is running
    if let Ok(pid_str) = fs::read_to_string(&lock_path)
        && let Ok(pid) = pid_str.trim().parse::<u32>()
        && Path::new(&format!("/proc/{}", pid)).exists()
    {
        return None; // Another instance is running
    }

    // Write our PID
    if let Ok(mut f) = fs::File::create(&lock_path) {
        let _ = write!(f, "{}", std::process::id());
    }

    Some(lock_path)
}

/// Release the GUI lock file.
fn release_gui_lock(path: &Path) {
    let _ = fs::remove_file(path);
}

impl eframe::App for MirageApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll_if_needed();

        // Request repaint every 2 seconds for polling
        ctx.request_repaint_after(std::time::Duration::from_secs(2));

        egui::CentralPanel::default().show(ctx, |ui| {
            // Tab bar
            ui.horizontal(|ui| {
                ui.selectable_value(&mut self.tab, Tab::Status, "Status");
                ui.selectable_value(&mut self.tab, Tab::Settings, "Settings");
            });
            ui.separator();

            if self.tab == Tab::Status {
                // Status section
                ui.horizontal(|ui| {
                    if let Some(ref status) = self.status {
                        let (color, label) = if status.online {
                            (egui::Color32::from_rgb(0, 180, 0), "Connected")
                        } else {
                            (egui::Color32::from_rgb(200, 0, 0), "Offline")
                        };
                        let (rect, _) =
                            ui.allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::hover());
                        ui.painter().circle_filled(rect.center(), 5.0, color);
                        ui.label(egui::RichText::new(label).size(16.0));
                    } else {
                        ui.label(
                            egui::RichText::new("● Disconnected")
                                .size(16.0)
                                .color(egui::Color32::GRAY),
                        );
                    }
                });

                ui.add_space(4.0);

                // Counters
                if let Some(ref status) = self.status {
                    ui.horizontal(|ui| {
                        ui.label(format!("{} synced", status.synced));
                        ui.separator();
                        ui.label(format!("{} pending", status.pending));
                        ui.separator();
                        if status.conflicts > 0 {
                            ui.label(
                                egui::RichText::new(format!("{} conflicts", status.conflicts))
                                    .color(egui::Color32::from_rgb(200, 0, 0)),
                            );
                        } else {
                            ui.label("0 conflicts");
                        }
                    });
                }

                ui.add_space(8.0);
                ui.separator();
                ui.add_space(8.0);

                // Progress section
                if let Some(ref progress) = self.progress {
                    if progress.phase != SyncPhase::Idle && progress.phase != SyncPhase::Paused {
                        let phase_str = match progress.phase {
                            SyncPhase::Scanning => "Scanning",
                            SyncPhase::Downloading => "Downloading",
                            SyncPhase::Uploading => "Uploading",
                            SyncPhase::Paused => "Paused",
                            SyncPhase::Idle => unreachable!(),
                        };

                        if let Some(ref file) = progress.current_file {
                            ui.label(format!("{}: {}", phase_str, file));
                        } else {
                            ui.label(format!("{}...", phase_str));
                        }

                        if progress.bytes_total > 0 {
                            let fraction = progress.bytes_done as f32 / progress.bytes_total as f32;
                            let text = format!(
                                "{}%  {}/{}",
                                (fraction * 100.0) as u32,
                                format_bytes(progress.bytes_done),
                                format_bytes(progress.bytes_total),
                            );
                            ui.add(
                                egui::ProgressBar::new(fraction)
                                    .text(text)
                                    .desired_width(ui.available_width()),
                            );
                        } else if progress.files_total > 0 {
                            let fraction = progress.files_done as f32 / progress.files_total as f32;
                            let text =
                                format!("{}/{} files", progress.files_done, progress.files_total);
                            ui.add(
                                egui::ProgressBar::new(fraction)
                                    .text(text)
                                    .desired_width(ui.available_width()),
                            );
                        }

                        ui.add_space(8.0);
                        ui.separator();
                        ui.add_space(8.0);
                    } else if progress.phase == SyncPhase::Paused {
                        ui.label(
                            egui::RichText::new("Sync paused")
                                .color(egui::Color32::from_rgb(200, 160, 0)),
                        );
                        ui.add_space(8.0);
                        ui.separator();
                        ui.add_space(8.0);
                    } else {
                        ui.label(
                            egui::RichText::new("Idle — everything is up to date")
                                .color(egui::Color32::GRAY),
                        );
                        ui.add_space(8.0);
                        ui.separator();
                        ui.add_space(8.0);
                    }
                }

                // Pause / Resume button
                let is_paused = self
                    .progress
                    .as_ref()
                    .is_some_and(|p| p.phase == SyncPhase::Paused);
                if is_paused {
                    if ui.button("Resume Sync").clicked() {
                        let _ = ipc::send_request(&ipc::TrayRequest::ResumeSync);
                        self.progress = try_get_progress();
                    }
                } else if ui.button("Pause Sync").clicked() {
                    let _ = ipc::send_request(&ipc::TrayRequest::PauseSync);
                    self.progress = try_get_progress();
                }

                ui.add_space(4.0);

                // Open Folder button
                if ui.button("Open Folder").clicked() {
                    let _ = std::process::Command::new("xdg-open")
                        .arg(&self.mount_point)
                        .spawn();
                }
            }

            if self.tab == Tab::Settings {
                // Load settings on first visit
                if !self.settings_loaded
                    && let Ok(ipc::TrayResponse::Config(cfg)) =
                        ipc::send_request(&ipc::TrayRequest::GetConfig)
                {
                    self.settings_sync_interval = cfg.sync_interval_secs.to_string();
                    self.settings_cache_limit_mb = (cfg.cache_limit_bytes / 1_048_576).to_string();
                    self.settings_remote_path = cfg.remote_base_path;
                    self.settings_log_level = LOG_LEVELS
                        .iter()
                        .position(|&l| l == cfg.log_level)
                        .unwrap_or(0);
                    self.settings_loaded = true;
                }

                egui::Grid::new("settings_grid")
                    .num_columns(2)
                    .spacing([12.0, 8.0])
                    .show(ui, |ui| {
                        ui.label("Sync interval (sec):");
                        ui.text_edit_singleline(&mut self.settings_sync_interval);
                        ui.end_row();

                        ui.label("Cache limit (MB):");
                        ui.text_edit_singleline(&mut self.settings_cache_limit_mb);
                        ui.end_row();

                        ui.label("Remote path:");
                        ui.text_edit_singleline(&mut self.settings_remote_path);
                        ui.end_row();

                        ui.label("Log level:");
                        egui::ComboBox::from_id_salt("log_level")
                            .selected_text(LOG_LEVELS[self.settings_log_level])
                            .show_ui(ui, |ui| {
                                for (i, level) in LOG_LEVELS.iter().enumerate() {
                                    let label = if level.is_empty() { "(default)" } else { level };
                                    ui.selectable_value(&mut self.settings_log_level, i, label);
                                }
                            });
                        ui.end_row();
                    });

                ui.add_space(8.0);

                if ui.button("Save").clicked() {
                    let sync_interval = match self.settings_sync_interval.parse::<u64>() {
                        Ok(v) => v,
                        Err(_) => {
                            self.settings_save_msg =
                                Some(("Invalid sync interval value".to_string(), Instant::now()));
                            return;
                        }
                    };
                    let cache_mb = match self.settings_cache_limit_mb.parse::<u64>() {
                        Ok(v) => v,
                        Err(_) => {
                            self.settings_save_msg =
                                Some(("Invalid cache limit value".to_string(), Instant::now()));
                            return;
                        }
                    };
                    let cache_bytes = match cache_mb.checked_mul(1_048_576) {
                        Some(v) => v,
                        None => {
                            self.settings_save_msg =
                                Some(("Cache limit value too large".to_string(), Instant::now()));
                            return;
                        }
                    };

                    let fields = vec![
                        ("sync_interval_secs".to_string(), sync_interval.to_string()),
                        ("cache_limit_bytes".to_string(), cache_bytes.to_string()),
                        (
                            "remote_base_path".to_string(),
                            self.settings_remote_path.clone(),
                        ),
                        (
                            "log_level".to_string(),
                            LOG_LEVELS[self.settings_log_level].to_string(),
                        ),
                    ];

                    let all_ok = matches!(
                        ipc::send_request(&ipc::TrayRequest::SetConfig { fields }),
                        Ok(ipc::TrayResponse::Ok)
                    );
                    self.settings_save_msg = Some((
                        if all_ok {
                            "Saved (restart daemon to apply)".to_string()
                        } else {
                            "Error saving some settings".to_string()
                        },
                        Instant::now(),
                    ));
                }

                if let Some((ref msg, at)) = self.settings_save_msg
                    && at.elapsed().as_secs() < 3
                {
                    ui.label(msg.as_str());
                }
            }
        });
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.window_open.store(false, Ordering::SeqCst);
    }
}

/// Open the activity window. Blocks until the window is closed.
///
/// Call from a dedicated thread — this function runs the eframe event loop.
pub fn open_activity_window(mount_point: PathBuf, window_open: Arc<AtomicBool>) {
    let Some(lock_path) = acquire_gui_lock("gui-activity") else {
        return;
    };

    window_open.store(true, Ordering::SeqCst);

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Mirage")
            .with_inner_size([400.0, 300.0])
            .with_min_inner_size([300.0, 220.0]),
        event_loop_builder: Some(Box::new(|builder| {
            use winit::platform::wayland::EventLoopBuilderExtWayland;
            use winit::platform::x11::EventLoopBuilderExtX11;
            EventLoopBuilderExtX11::with_any_thread(builder, true);
            EventLoopBuilderExtWayland::with_any_thread(builder, true);
        })),
        ..Default::default()
    };

    let wo = Arc::clone(&window_open);
    let result = eframe::run_native(
        "Mirage",
        options,
        Box::new(move |_cc| Ok(Box::new(MirageApp::new(mount_point, wo)))),
    );

    if let Err(e) = result {
        tracing::error!("Activity window error: {}", e);
    }

    release_gui_lock(&lock_path);
    window_open.store(false, Ordering::SeqCst);
}

/// Open the settings window directly. Blocks until the window is closed.
pub fn open_settings_window(mount_point: PathBuf) {
    let Some(lock_path) = acquire_gui_lock("gui-settings") else {
        return;
    };

    let window_open = Arc::new(AtomicBool::new(false));
    window_open.store(true, Ordering::SeqCst);

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Mirage Settings")
            .with_inner_size([400.0, 300.0])
            .with_min_inner_size([300.0, 220.0]),
        event_loop_builder: Some(Box::new(|builder| {
            use winit::platform::wayland::EventLoopBuilderExtWayland;
            use winit::platform::x11::EventLoopBuilderExtX11;
            EventLoopBuilderExtX11::with_any_thread(builder, true);
            EventLoopBuilderExtWayland::with_any_thread(builder, true);
        })),
        ..Default::default()
    };

    let wo = Arc::clone(&window_open);
    let result = eframe::run_native(
        "Mirage Settings",
        options,
        Box::new(move |_cc| {
            Ok(Box::new(MirageApp::with_tab(
                mount_point,
                wo,
                Tab::Settings,
            )))
        }),
    );

    if let Err(e) = result {
        tracing::error!("Settings window error: {}", e);
    }

    release_gui_lock(&lock_path);
    window_open.store(false, Ordering::SeqCst);
}
