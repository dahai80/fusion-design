//! OPS-13：fd-cli 文件日志落地。
//!
//! WKWebView 内嵌主渠道下 fd-cli 子进程 stdout 被壳吞，现场故障零持久诊断件。
//! 本模块建日轮转文件 appender + stdout 双写 subscriber，返 WorkerGuard 保活。
//!
//! env 网关：
//! - `FUSION_LOG_DISABLE_FILE=1|true` → stdout-only，无 guard。
//! - `FUSION_LOG_DIR` → 覆盖日志目录（默认 macOS `~/Library/Logs/fusion-design`）。
//!
//! 任何环节失败（目录解析/创建）安全回退 stdout-only，不阻断 CLI 主流程。

use std::path::PathBuf;

use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::prelude::*;

/// 初始化日志：文件日轮转 + stdout 双写。返 `Some(guard)` 表示文件 appender 已建，
/// 调用方须持有 guard 至进程结束（drop 即 flush 关闭 file writer）。`None` = stdout-only。
///
/// 全局 subscriber 只能 init 一次/进程，由 main() 一次性调用。
pub fn init_logging() -> Option<WorkerGuard> {
    let env_filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn"));

    // FUSION_LOG_DISABLE_FILE=1 → stdout-only，无 guard。
    if should_disable_file() {
        eprintln!("[fusion-design] 文件日志禁用 (FUSION_LOG_DISABLE_FILE)，stdout-only");
        tracing_subscriber::fmt().with_env_filter(env_filter).init();
        return None;
    }

    // 解析日志目录：FUSION_LOG_DIR 覆盖 > 平台默认 > 回退 stdout。
    let log_dir = match resolve_log_dir() {
        Ok(d) => d,
        Err(e) => {
            eprintln!("[fusion-design] 日志目录解析失败，回退 stdout: {e}");
            tracing_subscriber::fmt().with_env_filter(env_filter).init();
            return None;
        }
    };

    // 确保目录存在（mkdir -p）。失败回退 stdout。
    if let Err(e) = std::fs::create_dir_all(&log_dir) {
        eprintln!(
            "[fusion-design] 日志目录创建失败 {}: {e}，回退 stdout",
            log_dir.display()
        );
        tracing_subscriber::fmt().with_env_filter(env_filter).init();
        return None;
    }

    let file_appender = tracing_appender::rolling::daily(&log_dir, "fusion-design.log");
    let (non_blocking, guard) = tracing_appender::non_blocking(file_appender);

    // layered：file NonBlocking writer（无 ANSI 色码）+ stdout 并发。
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
        "[fusion-design] 文件日志启用: {}/fusion-design.log",
        log_dir.display()
    );
    Some(guard)
}

/// 判定是否禁用文件日志。`FUSION_LOG_DISABLE_FILE=1` 或 `=true`（大小写不敏感）→ true。
fn should_disable_file() -> bool {
    match std::env::var("FUSION_LOG_DISABLE_FILE") {
        Ok(v) => v == "1" || v.eq_ignore_ascii_case("true"),
        Err(_) => false,
    }
}

/// 解析日志目录。`FUSION_LOG_DIR` 非空显式覆盖优先；否则按平台默认：
/// - macOS：`~/Library/Logs/fusion-design`（经 home_dir 拼装，dirs 5 已移除 log_dir）。
/// - Linux：`~/.local/share/fusion-design/logs`（XDG data_local）。
fn resolve_log_dir() -> Result<PathBuf, String> {
    if let Ok(d) = std::env::var("FUSION_LOG_DIR") {
        if !d.is_empty() {
            return Ok(PathBuf::from(d));
        }
    }
    #[cfg(target_os = "macos")]
    {
        let home = dirs::home_dir().ok_or_else(|| "dirs::home_dir() 无法解析 HOME".to_string())?;
        Ok(home.join("Library/Logs/fusion-design"))
    }
    #[cfg(not(target_os = "macos"))]
    {
        let base = dirs::data_local_dir()
            .ok_or_else(|| "dirs::data_local_dir() 无法解析本地数据目录".to_string())?;
        Ok(base.join("fusion-design").join("logs"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // env-var 测试并发 mutate 共享进程 env → race。用 static mutex 串行化。
    use std::sync::{Mutex, OnceLock};
    static ENV_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    fn lock() -> &'static Mutex<()> {
        ENV_LOCK.get_or_init(|| Mutex::new(()))
    }
    // 测纯函数，不真跑 init_logging 全局 init（set_global_default 单次限制，多测同进程 panic）。

    #[test]
    fn disable_file_unset_is_false() {
        let _g = lock().lock().unwrap();
        // 显式清空隔离父进程可能残留的 env。
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
        // 空字符串视为未设，回退平台默认。
        // macOS 末端 fusion-design；Linux 末端 logs。不假设平台，断言含 fusion-design 段。
        let result = resolve_log_dir();
        std::env::remove_var("FUSION_LOG_DIR");
        match result {
            Ok(d) => {
                let has_seg = d.components().any(|c| c.as_os_str() == "fusion-design");
                assert!(has_seg, "平台默认路径应含 fusion-design 段: {d:?}");
            }
            Err(_) => { /* 平台无 HOME（CI 无 HOME 场景），Err 合理 */ }
        }
    }
}
