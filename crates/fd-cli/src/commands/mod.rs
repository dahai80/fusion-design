//! A-3: fd-cli subcommand handler modules.
//!
//! All subcommand implementations split out of the original giant run() match in main.rs.
//! ARCH-3 completed the full split: 10 handler modules (ai_skill/chat/check_mlx/
//! design_system/export/generate/health/history/io_cmd/lint); main.rs run() keeps only
//! the `Command::* => commands::<group>::<fn>(...).await` call entries, zero inline arms.

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
pub mod search;
