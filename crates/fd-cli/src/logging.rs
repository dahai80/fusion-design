//! OPS-13: fd-cli file logging.
//!
//! When embedded in the WKWebView host channel, fd-cli subprocess stdout is swallowed by
//! the shell, leaving no persisted diagnostics for field failures. This module builds a
//! daily-rotating file appender + stdout dual-write subscriber and returns a WorkerGuard
//! to keep the writer alive.
//!
//! env gates:
//! - `FUSION_LOG_DISABLE_FILE=1|true` → stdout-only, no guard.
//! - `FUSION_LOG_DIR` → override log directory (default macOS `~/Library/Logs/fusion-design`).
//!
//! Any failure (directory resolution/creation) safely falls back to stdout-only without
//! blocking the CLI main flow.

use std::path::PathBuf;

use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::prelude::*;

/// Initialize logging: daily-rotating file + stdout dual write. Returns `Some(guard)` when
/// the file appender was created; the caller must hold the guard until process exit
/// (dropping it flushes and closes the file writer). `None` = stdout-only.
///
/// The global subscriber can only be initialized once per process; called once from main().
pub fn init_logging() -> Option<WorkerGuard> {
    let env_filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn"));

    // FUSION_LOG_DISABLE_FILE=1 → stdout-only, no guard.
    if should_disable_file() {
        eprintln!("[fusion-design] file logging disabled (FUSION_LOG_DISABLE_FILE), stdout-only");
        tracing_subscriber::fmt().with_env_filter(env_filter).init();
        return None;
    }

    // Resolve log directory: FUSION_LOG_DIR override > platform default > fall back to stdout.
    let log_dir = match resolve_log_dir() {
        Ok(d) => d,
        Err(e) => {
            eprintln!("[fusion-design] failed to resolve log directory, falling back to stdout: {e}");
            tracing_subscriber::fmt().with_env_filter(env_filter).init();
            return None;
        }
    };

    // Ensure the directory exists (mkdir -p). On failure fall back to stdout.
    if let Err(e) = std::fs::create_dir_all(&log_dir) {
        eprintln!(
            "[fusion-design] failed to create log directory {}: {e}, falling back to stdout",
            log_dir.display()
        );
        tracing_subscriber::fmt().with_env_filter(env_filter).init();
        return None;
    }

    let file_appender = tracing_appender::rolling::daily(&log_dir, "fusion-design.log");
    let (non_blocking, guard) = tracing_appender::non_blocking(file_appender);

    // layered: file NonBlocking writer (no ANSI color codes) + stdout concurrent.
    let file_layer = tracing_subscriber::fmt::layer()
        .with_writer(non_blocking)
        .with_ansi(false);
    let stdout_layer = tracing_subscriber::fmt::layer().with_writer(std::io::stdout);

    tracing_subscriber::registry()
        .with(env_filter)
        .with(file_layer)
        .with(stdout_layer)
        .init();

    eprintln!(
        "[fusion-design] file logging enabled: {}/fusion-design.log",
        log_dir.display()
    );
    Some(guard)
}

/// Whether file logging is disabled. `FUSION_LOG_DISABLE_FILE=1` or `=true`
/// (case-insensitive) → true.
fn should_disable_file() -> bool {
    match std::env::var("FUSION_LOG_DISABLE_FILE") {
        Ok(v) => v == "1" || v.eq_ignore_ascii_case("true"),
        Err(_) => false,
    }
}

/// Resolve the log directory. A non-empty `FUSION_LOG_DIR` takes precedence; otherwise the
/// platform default applies:
/// - macOS: `~/Library/Logs/fusion-design` (assembled via home_dir; dirs 5 removed log_dir).
/// - Linux：`~/.local/share/fusion-design/logs`（XDG data_local）。
fn resolve_log_dir() -> Result<PathBuf, String> {
    if let Ok(d) = std::env::var("FUSION_LOG_DIR") {
        if !d.is_empty() {
            return Ok(PathBuf::from(d));
        }
    }
    #[cfg(target_os = "macos")]
    {
        let home = dirs::home_dir()
            .ok_or_else(|| "dirs::home_dir() failed to resolve HOME".to_string())?;
        Ok(home.join("Library/Logs/fusion-design"))
    }
    #[cfg(not(target_os = "macos"))]
    {
        let base = dirs::data_local_dir()
            .ok_or_else(|| "dirs::data_local_dir() failed to resolve local data dir".to_string())?;
        Ok(base.join("fusion-design").join("logs"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Tests concurrently mutate the shared process env via env vars → race. Serialize with
    // a static mutex.
    use std::sync::{Mutex, OnceLock};
    static ENV_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    fn lock() -> &'static Mutex<()> {
        ENV_LOCK.get_or_init(|| Mutex::new(()))
    }
    // Test pure functions only; never run init_logging's global init here
    // (set_global_default is once-per-process; multiple tests would panic).

    #[test]
    fn disable_file_unset_is_false() {
        let _g = lock().lock().unwrap();
        // Explicitly clear env vars possibly inherited from the parent process.
        std::env::remove_var("FUSION_LOG_DISABLE_FILE");
        assert!(!should_disable_file());
    }

    #[test]
    fn disable_file_one_is_true() {
        let _g = lock().lock().unwrap();
        std::env::set_var("FUSION_LOG_DISABLE_FILE", "1");
        assert!(should_disable_file());
        std::env::remove_var("FUSION_LOG_DISABLE_FILE");
    }

    #[test]
    fn disable_file_true_case_insensitive() {
        let _g = lock().lock().unwrap();
        std::env::set_var("FUSION_LOG_DISABLE_FILE", "TRUE");
        assert!(should_disable_file());
        std::env::remove_var("FUSION_LOG_DISABLE_FILE");
    }

    #[test]
    fn disable_file_other_value_is_false() {
        let _g = lock().lock().unwrap();
        std::env::set_var("FUSION_LOG_DISABLE_FILE", "0");
        assert!(!should_disable_file());
        std::env::remove_var("FUSION_LOG_DISABLE_FILE");
    }

    #[test]
    fn resolve_log_dir_env_override() {
        let _g = lock().lock().unwrap();
        let tmp = tempfile::tempdir().expect("tempdir");
        let dir = tmp.path().to_path_buf();
        std::env::set_var("FUSION_LOG_DIR", &dir);
        let resolved = resolve_log_dir().expect("resolve");
        assert_eq!(resolved, dir);
        std::env::remove_var("FUSION_LOG_DIR");
    }

    #[test]
    fn resolve_log_dir_empty_env_falls_back() {
        let _g = lock().lock().unwrap();
        std::env::set_var("FUSION_LOG_DIR", "");
        // Empty string counts as unset; fall back to the platform default.
        // macOS ends with fusion-design; Linux ends with logs. Do not assume the platform;
        // assert the path contains a fusion-design segment.
        let result = resolve_log_dir();
        std::env::remove_var("FUSION_LOG_DIR");
        match result {
            Ok(d) => {
                let has_seg = d.components().any(|c| c.as_os_str() == "fusion-design");
                assert!(has_seg, "platform default path should contain fusion-design segment: {d:?}");
            }
            Err(_) => { /* platform has no HOME (CI without HOME); Err is acceptable */ }
        }
    }
}
