pub mod backend;
pub mod cache;
pub mod cli;
pub mod config;
pub mod db;
pub mod error;
#[cfg(target_os = "linux")]
pub mod fuse;
pub mod lock;
pub mod network;
pub mod resolve;
pub mod sync;
pub mod tray;
pub mod upload;

use std::path::Path;

use cli::{Command, ConfigAction};
use error::{Error, Result};

/// Run the application with the parsed CLI command.
pub fn run(command: &Command) -> Result<()> {
    match command {
        Command::Mount { mountpoint } => {
            run_mount(mountpoint)?;
        }
        Command::Unmount => {
            run_unmount()?;
        }
        Command::Status => {
            run_status()?;
        }
        Command::Pin { path, recursive } => {
            run_pin(path, true, *recursive)?;
        }
        Command::Unpin { path, recursive } => {
            run_pin(path, false, *recursive)?;
        }
        Command::Config { action } => {
            run_config(action)?;
        }
        Command::Conflicts => {
            run_conflicts()?;
        }
        Command::Resolve { path, strategy } => {
            run_resolve(path, strategy)?;
        }
        Command::Daemon { action } => {
            run_daemon(action)?;
        }
        Command::Tray => {
            tray::run_tray()?;
        }
        Command::Settings => {
            let cfg = config::Config::load()?;
            tray::gui::open_settings_window(cfg.mount_point);
        }
        Command::Gui => {
            let cfg = config::Config::load()?;
            tray::gui::open_activity_window(
                cfg.mount_point,
                std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
            );
        }
        Command::Logs { follow, lines } => {
            run_logs(*follow, *lines)?;
        }
        Command::Setup => {
            run_setup()?;
        }
    }
    Ok(())
}

fn prompt_input(prompt: &str, default: &str) -> Result<String> {
    use std::io::Write;
    if default.is_empty() {
        print!("{prompt}: ");
    } else {
        print!("{prompt} [{default}]: ");
    }
    std::io::stdout().flush().map_err(Error::Io)?;
    let mut input = String::new();
    std::io::stdin().read_line(&mut input).map_err(Error::Io)?;
    let input = input.trim();
    if input.is_empty() {
        Ok(default.to_owned())
    } else {
        Ok(input.to_owned())
    }
}

fn run_setup() -> Result<()> {
    use backend::Backend as _;

    // Load existing config if available, otherwise use defaults
    let existing = config::Config::load().ok();
    let config_path = config::Config::config_path()?;

    println!("Mirage setup");
    println!();

    let server_url = prompt_input(
        "Server URL",
        existing
            .as_ref()
            .map(|c| c.server_url.as_str())
            .unwrap_or("https://cloud.example.com"),
    )?;
    if server_url.is_empty() || !server_url.starts_with("http") {
        return Err(Error::Config("invalid server URL".into()));
    }

    let username = prompt_input(
        "Username",
        existing.as_ref().map(|c| c.username.as_str()).unwrap_or(""),
    )?;
    if username.is_empty() {
        return Err(Error::Config("username cannot be empty".into()));
    }

    print!("Password: ");
    std::io::Write::flush(&mut std::io::stdout()).map_err(Error::Io)?;
    let password = rpassword::read_password()
        .map_err(|e| Error::Config(format!("failed to read password: {e}")))?;
    if password.is_empty() {
        return Err(Error::Config("password cannot be empty".into()));
    }

    // Test connection
    println!("Testing connection...");
    let test_cfg = config::Config {
        server_url: server_url.clone(),
        username: username.clone(),
        password: Some(password.clone()),
        ..existing.clone().unwrap_or_else(|| config::Config {
            server_url: server_url.clone(),
            username: username.clone(),
            password: None,
            cache_dir: dirs::cache_dir().unwrap_or_default().join("mirage"),
            cache_limit_bytes: 1_073_741_824,
            mount_point: dirs::home_dir().unwrap_or_default().join("Cloud"),
            sync_interval_secs: 300,
            retry_base_secs: 30,
            retry_max_secs: 600,
            always_local_paths: vec![],
            connect_timeout_secs: 10,
            request_timeout_secs: 60,
            ignore_file: None,
            remote_base_path: None,
            log_level: None,
        })
    };
    let nc = backend::nextcloud::NextcloudClient::new(&test_cfg)?;
    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(nc.ping())?;
    println!("Connection successful.");

    // Save config.toml (without password)
    let save_cfg = config::Config {
        password: None,
        ..test_cfg
    };
    if let Some(parent) = config_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let toml_str = toml::to_string_pretty(&save_cfg)
        .map_err(|e| Error::Config(format!("failed to serialize config: {e}")))?;
    std::fs::write(&config_path, &toml_str)?;
    println!("Config saved: {}", config_path.display());

    // Store password in keyring with fallback to config file
    let keyring_ok = match keyring::Entry::new("mirage", &username) {
        Ok(entry) => match entry.set_password(&password) {
            Ok(()) => {
                // Verify we can read it back
                match entry.get_password() {
                    Ok(pw) if pw == password => {
                        println!("Password stored in system keyring.");
                        true
                    }
                    _ => {
                        eprintln!(
                            "Warning: password was written to keyring but could not be read back."
                        );
                        false
                    }
                }
            }
            Err(e) => {
                eprintln!("Warning: failed to store password in keyring: {e}");
                false
            }
        },
        Err(e) => {
            eprintln!("Warning: failed to access keyring: {e}");
            false
        }
    };

    if !keyring_ok {
        eprintln!(
            "Falling back to storing password in credentials file.\n\
             Tip: for better security, set the MIRAGE_PASSWORD environment variable instead."
        );
        config::Config::save_credentials(&password)?;
        println!(
            "Password saved to {}",
            config::Config::credentials_path()?.display()
        );
    }

    Ok(())
}

fn run_logs(follow: bool, lines: u32) -> Result<()> {
    let mut cmd = std::process::Command::new("journalctl");
    cmd.arg("--user-unit=mirage.service")
        .arg(format!("-n{lines}"));

    if follow {
        cmd.arg("-f");
    }

    let status = cmd.status().map_err(Error::Io)?;
    if !status.success() {
        return Err(Error::Config("journalctl command failed".into()));
    }
    Ok(())
}

fn run_status() -> Result<()> {
    use db::models::SyncState;

    let cfg = config::Config::load()?;
    let db_path = cfg.cache_dir.join("metadata.db");
    let db = db::Database::open_readonly(&db_path)?;

    // Sum cache directory file sizes (excluding metadata.db)
    let cache_bytes: u64 = if cfg.cache_dir.exists() {
        std::fs::read_dir(&cfg.cache_dir)
            .map_err(Error::Io)?
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name() != "metadata.db")
            .filter_map(|e| e.metadata().ok())
            .map(|m| m.len())
            .sum()
    } else {
        0
    };

    let total = db.count_total()?;
    let cached = db.count_cached()?;
    let synced = db.count_by_sync_state(SyncState::Synced)?;
    let pending_dl = db.count_by_sync_state(SyncState::PendingDownload)?;
    let pending_ul = db.count_by_sync_state(SyncState::PendingUpload)?;
    let conflicts = db.count_by_sync_state(SyncState::Conflict)?;

    let limit = cfg.cache_limit_bytes;
    let pct = if limit > 0 {
        cache_bytes * 100 / limit
    } else {
        0
    };

    println!(
        "Cache: {} / {} ({}%)",
        format_bytes(cache_bytes),
        format_bytes(limit),
        pct
    );
    println!("Files: {total} total, {cached} cached");
    println!(
        "Sync:  {synced} synced, {pending_dl} pending_download, {pending_ul} pending_upload, {conflicts} conflict"
    );

    Ok(())
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

fn resolve_path_to_inode(db: &db::Database, mount_point: &Path, target: &Path) -> Result<u64> {
    let relative = target
        .strip_prefix(mount_point)
        .map_err(|_| Error::NotFound(target.to_path_buf()))?;

    if relative.as_os_str().is_empty() {
        return Ok(1);
    }

    let mut current_inode = 1u64;
    for component in relative.components() {
        let name = component
            .as_os_str()
            .to_str()
            .ok_or_else(|| Error::NotFound(target.to_path_buf()))?;
        let entry = db
            .lookup(current_inode, name)
            .map_err(|_| Error::NotFound(target.to_path_buf()))?;
        current_inode = entry.inode;
    }

    Ok(current_inode)
}

fn run_pin(path: &Path, pinned: bool, recursive: bool) -> Result<()> {
    let cfg = config::Config::load()?;
    let db_path = cfg.cache_dir.join("metadata.db");
    let db = db::Database::open(&db_path)?;

    let inode = resolve_path_to_inode(&db, &cfg.mount_point, path)?;

    if recursive {
        let count = db.set_pinned_recursive(inode, pinned)?;
        if pinned {
            tracing::info!(path = %path.display(), inode, count, "pinned recursively");
        } else {
            tracing::info!(path = %path.display(), inode, count, "unpinned recursively");
        }
    } else {
        db.set_pinned(inode, pinned)?;
        if pinned {
            tracing::info!(path = %path.display(), inode, "pinned");
        } else {
            tracing::info!(path = %path.display(), inode, "unpinned");
        }
    }

    Ok(())
}

fn run_daemon(action: &cli::DaemonAction) -> Result<()> {
    match action {
        cli::DaemonAction::Start => run_mount(&config::Config::load()?.mount_point),
        cli::DaemonAction::Stop => run_daemon_stop(),
        cli::DaemonAction::Status => run_daemon_status(),
    }
}

fn run_daemon_stop() -> Result<()> {
    let cfg = config::Config::load()?;
    let lock_path = lock::default_lock_path(&cfg.cache_dir);

    match lock::read_pid(&lock_path)? {
        Some(pid) if lock::is_held(&lock_path) => {
            let ret = unsafe { libc::kill(pid as i32, libc::SIGTERM) };
            if ret != 0 {
                return Err(Error::Config(format!(
                    "failed to send SIGTERM to PID {pid}"
                )));
            }
            println!("Sent stop signal to mirage (PID: {pid})");
            Ok(())
        }
        _ => {
            println!("mirage is not running");
            Ok(())
        }
    }
}

fn run_daemon_status() -> Result<()> {
    let cfg = config::Config::load()?;
    let lock_path = lock::default_lock_path(&cfg.cache_dir);

    if lock::is_held(&lock_path) {
        match lock::read_pid(&lock_path)? {
            Some(pid) => println!("mirage is running (PID: {pid})"),
            None => println!("mirage is running (PID unknown)"),
        }
    } else {
        println!("mirage is not running");
    }
    Ok(())
}

fn run_unmount() -> Result<()> {
    #[cfg(target_os = "linux")]
    {
        let cfg = config::Config::load()?;
        let mount = &cfg.mount_point;

        let status = std::process::Command::new("fusermount3")
            .arg("-u")
            .arg(mount)
            .status();

        match status {
            Ok(s) if s.success() => return Ok(()),
            _ => {
                // fallback to umount
                std::process::Command::new("umount")
                    .arg(mount)
                    .status()
                    .map_err(Error::Io)?;
            }
        }
    }
    #[cfg(not(target_os = "linux"))]
    return Err(Error::Config("unmount is only supported on Linux".into()));

    Ok(())
}

fn run_config(action: &Option<ConfigAction>) -> Result<()> {
    let path = config::Config::config_path()?;
    match action {
        Some(ConfigAction::List) => {
            let cfg = config::Config::load()?;
            let keys = [
                "server_url",
                "username",
                "password",
                "cache_dir",
                "cache_limit_bytes",
                "mount_point",
                "sync_interval_secs",
                "retry_base_secs",
                "retry_max_secs",
                "always_local_paths",
                "connect_timeout_secs",
                "request_timeout_secs",
                "ignore_file",
                "remote_base_path",
                "log_level",
            ];
            for key in keys {
                let value = cfg.get_field(key)?;
                println!("{key:<24}= {value}");
            }
        }
        Some(ConfigAction::Get { key }) => {
            let cfg = config::Config::load()?;
            let value = cfg.get_field(key)?;
            println!("{value}");
        }
        Some(ConfigAction::Set { key, value }) => {
            let mut cfg = config::Config::load()?;
            cfg.set_field(key, value)?;
            cfg.save()?;
            println!("{key} = {value}");
        }
        Some(ConfigAction::Init { force }) => {
            if path.exists() && !force {
                return Err(Error::Config(format!(
                    "config already exists at {}. Use --force to overwrite.",
                    path.display()
                )));
            }
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let template = config::Config::generate_template();
            std::fs::write(&path, template)?;
            println!("Config file created: {}", path.display());
        }
        Some(ConfigAction::Path) | None => {
            println!("{}", path.display());
        }
    }
    Ok(())
}

fn run_conflicts() -> Result<()> {
    use db::models::SyncState;

    let cfg = config::Config::load()?;
    let db_path = cfg.cache_dir.join("metadata.db");
    let db = db::Database::open_readonly(&db_path)?;

    let entries = db.get_by_sync_state(SyncState::Conflict)?;
    if entries.is_empty() {
        println!("No conflicts.");
        return Ok(());
    }

    println!("{} conflict(s):", entries.len());
    for entry in &entries {
        let etag = entry.etag.as_deref().unwrap_or("-");
        println!(
            "  inode={} name={:?} etag={}",
            entry.inode, entry.name, etag
        );
    }
    Ok(())
}

fn run_resolve(path: &std::path::Path, strategy: &cli::ResolveStrategy) -> Result<()> {
    use db::models::SyncState;

    let cfg = config::Config::load()?;
    let db_path = cfg.cache_dir.join("metadata.db");
    let db = db::Database::open(&db_path)?;

    let inode = resolve_path_to_inode(&db, &cfg.mount_point, path)?;
    let entry = db.get_by_inode(inode)?;

    if entry.sync_state != SyncState::Conflict {
        return Err(Error::Config(format!(
            "{} is not in conflict state (current: {})",
            path.display(),
            entry.sync_state
        )));
    }

    let nc = backend::nextcloud::NextcloudClient::new(&cfg)?;
    let backend = std::sync::Arc::new(nc);

    let rt = tokio::runtime::Runtime::new()?;

    // Build remote path
    let mut parts = Vec::new();
    let mut current = inode;
    loop {
        let e = db.get_by_inode(current)?;
        if e.inode == 1 {
            break;
        }
        parts.push(e.name.clone());
        current = e.parent_inode;
    }
    parts.reverse();
    let remote_path = parts.join("/");

    let resolve_strategy = match strategy {
        cli::ResolveStrategy::KeepLocal => resolve::Strategy::KeepLocal,
        cli::ResolveStrategy::KeepRemote => resolve::Strategy::KeepRemote,
        cli::ResolveStrategy::KeepBoth => resolve::Strategy::KeepBoth,
    };

    rt.block_on(resolve::resolve_conflict(
        &db,
        &backend,
        &cfg.cache_dir,
        inode,
        &remote_path,
        resolve_strategy,
    ))?;

    tracing::info!(path = %path.display(), strategy = ?resolve_strategy, "conflict resolved");
    Ok(())
}

#[cfg(target_os = "linux")]
fn run_mount(mountpoint: &std::path::Path) -> Result<()> {
    use std::sync::Arc;
    use std::time::Duration;

    use backend::Backend;

    // Detect and clean up stale mounts from a previous crash.
    // We check /proc/mounts instead of `mountpoint -q` because the latter
    // fails with ENOTCONN on dead FUSE mounts (Transport endpoint not connected).
    if let Ok(mounts) = std::fs::read_to_string("/proc/mounts") {
        let canonical =
            std::fs::canonicalize(mountpoint).unwrap_or_else(|_| mountpoint.to_path_buf());
        let canonical_str = canonical.to_string_lossy();
        if mounts.lines().any(|line| {
            line.split_whitespace()
                .nth(1)
                .is_some_and(|mp| mp == canonical_str.as_ref())
        }) {
            tracing::warn!(
                path = %mountpoint.display(),
                "stale mount detected, cleaning up with lazy unmount"
            );
            let _ = std::process::Command::new("fusermount3")
                .args(["-uz", canonical_str.as_ref()])
                .status();
        }
    }

    let cfg = config::Config::load()?;
    let sync_interval_secs = cfg.sync_interval_secs;

    let lock_path = lock::default_lock_path(&cfg.cache_dir);
    let _lock = lock::LockFile::acquire(&lock_path)?;

    let rt = tokio::runtime::Runtime::new()?;

    let db_path = cfg.cache_dir.join("metadata.db");
    if let Some(parent) = db_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let database = db::Database::open(&db_path)?;

    let cache_manager = rt.block_on(cache::CacheManager::open(
        cfg.cache_dir.clone(),
        cfg.cache_limit_bytes,
        database,
    ))?;

    let nc = backend::nextcloud::NextcloudClient::new(&cfg)?;
    let backend = Arc::new(nc);

    let net_monitor = network::NetworkMonitor::new();

    // Separate DB connection for sync engine (SQLite WAL allows concurrent access).
    // rusqlite::Connection is Send but !Sync, so we run the sync loop on a
    // dedicated thread with its own current_thread Tokio runtime.
    let sync_db = db::Database::open(&db_path)?;
    let sync_backend = Arc::clone(&backend);
    let sync_cache_dir = cfg.cache_dir.clone();
    let sync_net_state = net_monitor.shared();
    let sync_always_local = cfg.always_local_paths.clone();
    let sync_paused = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let sync_progress = sync::progress::SyncProgress::new();
    let ipc_progress = sync_progress.clone();
    let ignore_path = cfg.ignore_file.clone().unwrap_or_else(|| {
        dirs::config_dir()
            .map(|d| d.join("mirage").join(".mirageignore"))
            .unwrap_or_default()
    });
    let thread_sync_paused = Arc::clone(&sync_paused);
    let sync_ping_backend = Arc::clone(&backend);
    std::thread::spawn(move || {
        let mut sync_engine =
            sync::SyncEngine::new(sync_db, sync_backend, sync_cache_dir, sync_always_local);
        sync_engine.set_progress(sync_progress);
        sync_engine.set_ignore(sync::ignore::IgnoreRules::load(&ignore_path));
        let sync_rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("failed to build sync runtime");
        sync_rt.block_on(async move {
            if let Err(e) = sync_engine.full_sync().await {
                tracing::error!(error = %e, "initial sync failed");
                if e.is_transient() {
                    sync_net_state.store(1, std::sync::atomic::Ordering::Relaxed);
                }
            } else {
                sync_net_state.store(0, std::sync::atomic::Ordering::Relaxed);
            }
            let mut interval = tokio::time::interval(Duration::from_secs(sync_interval_secs));
            loop {
                interval.tick().await;
                if thread_sync_paused.load(std::sync::atomic::Ordering::SeqCst) {
                    continue;
                }
                // Skip sync when offline — ping first
                if sync_net_state.load(std::sync::atomic::Ordering::Relaxed) != 0 {
                    match sync_ping_backend.ping().await {
                        Ok(()) => {
                            sync_net_state.store(0, std::sync::atomic::Ordering::Relaxed);
                        }
                        Err(_) => {
                            tracing::debug!("still offline, skipping sync");
                            continue;
                        }
                    }
                }
                match sync_engine.full_sync().await {
                    Ok(r) => {
                        sync_net_state.store(0, std::sync::atomic::Ordering::Relaxed);
                        tracing::info!(
                            added = r.added,
                            updated = r.updated,
                            deleted = r.deleted,
                            pinned_downloads = r.pinned_downloads,
                            "sync completed"
                        );
                    }
                    Err(e) => {
                        if e.is_transient() {
                            sync_net_state.store(1, std::sync::atomic::Ordering::Relaxed);
                        }
                        tracing::error!(error = %e, "sync failed");
                    }
                }
            }
        });
    });

    let uid = unsafe { libc::getuid() };
    let gid = unsafe { libc::getgid() };

    // Spawn upload worker thread.
    let (upload_tx, upload_rx) = std::sync::mpsc::channel::<upload::UploadMessage>();
    let upload_db = db::Database::open(&db_path)?;
    let upload_backend = Arc::clone(&backend);
    let upload_cache_dir = cfg.cache_dir.clone();
    let upload_rt_handle = rt.handle().clone();
    let retry_base_secs = cfg.retry_base_secs;
    let retry_max_secs = cfg.retry_max_secs;
    let upload_net_state = net_monitor.shared();
    let upload_handle = std::thread::spawn(move || {
        let worker = upload::UploadWorker::new(
            upload_db,
            upload_backend,
            upload_cache_dir,
            upload_rx,
            upload_rt_handle,
            retry_base_secs,
            retry_max_secs,
            upload_net_state,
        );
        worker.run();
    });

    // Register Ctrl+C handler to unmount FUSE and shut down upload worker.
    let shutdown_tx = upload_tx.clone();
    let shutdown_mountpoint = mountpoint.to_path_buf();
    ctrlc::set_handler(move || {
        tracing::info!("received signal, shutting down");
        // Unmount FUSE to unblock fuser::mount2 on the main thread.
        let _ = std::process::Command::new("fusermount3")
            .arg("-u")
            .arg(&shutdown_mountpoint)
            .status();
        let _ = shutdown_tx.send(upload::UploadMessage::Shutdown);
    })
    .map_err(|e| crate::error::Error::Config(format!("failed to set signal handler: {e}")))?;

    // Spawn IPC server thread for tray communication.
    let ipc_db = db::Database::open(&db_path)?;
    let ipc_net_state = net_monitor.shared();
    let ipc_mount_point = mountpoint.to_path_buf();
    let ipc_sync_paused = Arc::clone(&sync_paused);
    std::thread::spawn(move || {
        match tray::ipc::IpcServer::new(
            ipc_db,
            ipc_net_state,
            Some(ipc_progress),
            ipc_mount_point,
            ipc_sync_paused,
        ) {
            Ok(server) => server.run(),
            Err(e) => tracing::warn!(error = %e, "failed to start IPC server"),
        }
    });

    std::fs::create_dir_all(mountpoint)?;
    tracing::info!(path = %mountpoint.display(), "mounting FUSE filesystem");

    let fuse_net_state = net_monitor.shared();
    let fs = fuse::MirageFs::new(
        cache_manager,
        backend,
        rt.handle().clone(),
        uid,
        gid,
        upload_tx,
        fuse_net_state,
    );
    fs.mount(mountpoint)?;

    // Wait for upload worker to finish draining pending uploads.
    let _ = upload_handle.join();

    // Explicitly unmount the FUSE filesystem after the main loop exits.
    tracing::info!(path = %mountpoint.display(), "unmounting FUSE filesystem");
    let unmount_status = std::process::Command::new("fusermount3")
        .arg("-u")
        .arg(mountpoint)
        .status();
    match unmount_status {
        Ok(s) if s.success() => {
            tracing::info!("FUSE filesystem unmounted successfully");
        }
        _ => {
            tracing::warn!(path = %mountpoint.display(), "fusermount3 -u failed, mount may already be gone");
        }
    }

    Ok(())
}

#[cfg(not(target_os = "linux"))]
fn run_mount(_mountpoint: &std::path::Path) -> Result<()> {
    Err(error::Error::Config(
        "FUSE mount is only supported on Linux".into(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use db::models::{NewFileEntry, SyncState};

    fn test_db() -> db::Database {
        db::Database::open_in_memory().unwrap()
    }

    fn insert(db: &db::Database, parent: u64, name: &str, is_dir: bool) -> u64 {
        db.insert(&NewFileEntry {
            parent_inode: parent,
            name: name.to_owned(),
            is_dir,
            size: 0,
            permissions: 0o755,
            mtime: 0,
            etag: None,
            content_hash: None,
            is_pinned: false,
            is_cached: false,
            sync_state: SyncState::Synced,
        })
        .unwrap()
    }

    #[test]
    fn resolve_root_path() {
        let db = test_db();
        let mount = Path::new("/mnt");
        let inode = resolve_path_to_inode(&db, mount, mount).unwrap();
        assert_eq!(inode, 1);
    }

    #[test]
    fn resolve_one_level() {
        let db = test_db();
        let mount = Path::new("/mnt");
        let file_inode = insert(&db, 1, "docs", true);
        let inode = resolve_path_to_inode(&db, mount, Path::new("/mnt/docs")).unwrap();
        assert_eq!(inode, file_inode);
    }

    #[test]
    fn resolve_not_found() {
        let db = test_db();
        let mount = Path::new("/mnt");
        let err = resolve_path_to_inode(&db, mount, Path::new("/mnt/nonexistent")).unwrap_err();
        assert!(matches!(err, Error::NotFound(_)));
    }

    #[test]
    fn resolve_outside_mount() {
        let db = test_db();
        let mount = Path::new("/mnt");
        let err = resolve_path_to_inode(&db, mount, Path::new("/other/path")).unwrap_err();
        assert!(matches!(err, Error::NotFound(_)));
    }
}
