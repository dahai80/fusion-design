//! A-3：fd-cli 子命令 handler 模块。
//!
//! 从原 main.rs 的 run() 巨型 match 拆出的各子命令实现。仅拆出体量最大的
//! 6 个 arm（export/generate/chat/lint/check_mlx/health），其余 arm 仍内联
//! 于 main.rs run() 并标注 TODO 待后续拆分，避免一次大改风险失控。

pub mod chat;
pub mod check_mlx;
pub mod design_system;
pub mod export;
pub mod generate;
pub mod health;
pub mod lint;
