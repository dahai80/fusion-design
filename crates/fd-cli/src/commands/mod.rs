//! A-3：fd-cli 子命令 handler 模块。
//!
//! 从原 main.rs 的 run() 巨型 match 拆出的全部子命令实现。ARCH-3 已完成
//! 全量拆分：10 个 handler 模块（ai_skill/chat/check_mlx/design_system/
//! export/generate/health/history/io_cmd/lint），main.rs run() 仅保留
//! `Command::* => commands::<group>::<fn>(...).await` 调用入口，零内联 arm。

pub mod ai_skill;
pub mod chat;
pub mod check_mlx;
pub mod design_system;
pub mod export;
pub mod generate;
pub mod health;
pub mod history;
pub mod io_cmd;
pub mod lint;
