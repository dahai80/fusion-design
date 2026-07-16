//! Fusion-Design 设计系统 — 三套内置规范 + Token 管理。
//!
//! 对应 PRD 模块 3「本地设计系统与组件库」。
//! 全局 Token（颜色/字号/间距/圆角/阴影）统一定义，一键同步所有页面。

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// 设计 Token 值类型。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum TokenValue {
    Color(String),        // hex, e.g. "#FFFFFF"
    Number(f32),          // 字号/间距/圆角
    Shadow(String),       // CSS box-shadow
    String(String),       // 字体族等
}

/// 单个 Token 定义。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Token {
    pub name: String,
    pub value: TokenValue,
    pub description: String,
}

/// 设计规范（一组 Token 集合）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DesignSystem {
    pub id: String,
    pub name: String,
    pub tokens: Vec<Token>,
}

/// 设计系统注册中心（管理多套规范，支持一键切换）。
#[derive(Debug, Default)]
pub struct DesignSystemRegistry {
    systems: HashMap<String, DesignSystem>,
    active_id: Option<String>,
}

impl DesignSystemRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// 注册一套设计规范。
    pub fn register(&mut self, system: DesignSystem) -> Result<(), DuplicateError> {
        if self.systems.contains_key(&system.id) {
            return Err(DuplicateError(system.id));
        }
        self.systems.insert(system.id.clone(), system);
        Ok(())
    }

    /// 激活某套规范（切换全局 Token）。
    pub fn activate(&mut self, id: &str) -> Result<(), NotFoundError> {
        if !self.systems.contains_key(id) {
            return Err(NotFoundError(id.to_string()));
        }
        self.active_id = Some(id.to_string());
        Ok(())
    }

    /// 获取当前激活的规范。
    pub fn active(&self) -> Option<&DesignSystem> {
        self.active_id
            .as_ref()
            .and_then(|id| self.systems.get(id))
    }

    /// 列出全部已注册规范 ID。
    pub fn list(&self) -> Vec<&str> {
        self.systems.keys().map(|s| s.as_str()).collect()
    }

    /// 注册三套内置规范（Apple HIG / 极简后台 / 机器人仿真）。
    pub fn register_builtin(&mut self) {
        self.systems.extend([
            ("apple-hig".to_string(), builtin_apple_hig()),
            ("minimal-dashboard".to_string(), builtin_minimal_dashboard()),
            ("robot-sim".to_string(), builtin_robot_sim()),
        ]);
    }

    /// 按名称查找 Token 值（当前激活规范内）。
    pub fn lookup(&self, token_name: &str) -> Option<&TokenValue> {
        self.active()?
            .tokens
            .iter()
            .find(|t| t.name == token_name)
            .map(|t| &t.value)
    }

    /// 导出为 JSON（团队本地共享设计规范）。
    pub fn export_json(&self) -> serde_json::Result<String> {
        let active = self.active();
        serde_json::to_string_pretty(&active)
    }

    /// 从 JSON 导入一套规范。
    pub fn import_json(&mut self, json: &str) -> Result<(), ImportError> {
        let system: DesignSystem =
            serde_json::from_str(json).map_err(ImportError)?;
        self.systems.insert(system.id.clone(), system);
        Ok(())
    }
}

#[derive(Debug, thiserror::Error)]
#[error("设计规范 {0} 已注册")]
pub struct DuplicateError(pub String);

#[derive(Debug, thiserror::Error)]
#[error("设计规范 {0} 未找到")]
pub struct NotFoundError(pub String);

#[derive(Debug, thiserror::Error)]
#[error("导入 JSON 失败: {0}")]
pub struct ImportError(#[from] serde_json::Error);

// ── 三套内置规范 ──

fn builtin_apple_hig() -> DesignSystem {
    DesignSystem {
        id: "apple-hig".into(),
        name: "Apple HIG".into(),
        tokens: vec![
            Token { name: "color.bg".into(), value: TokenValue::Color("#F2F2F7".into()), description: "系统背景".into() },
            Token { name: "color.fg".into(), value: TokenValue::Color("#1C1C1E".into()), description: "主前景".into() },
            Token { name: "color.accent".into(), value: TokenValue::Color("#007AFF".into()), description: "系统强调色".into() },
            Token { name: "font.size.body".into(), value: TokenValue::Number(17.0), description: "正文字号".into() },
            Token { name: "radius.card".into(), value: TokenValue::Number(10.0), description: "卡片圆角".into() },
        ],
    }
}

fn builtin_minimal_dashboard() -> DesignSystem {
    DesignSystem {
        id: "minimal-dashboard".into(),
        name: "极简后台".into(),
        tokens: vec![
            Token { name: "color.bg".into(), value: TokenValue::Color("#FFFFFF".into()), description: "纯白背景".into() },
            Token { name: "color.fg".into(), value: TokenValue::Color("#374151".into()), description: "深灰前景".into() },
            Token { name: "color.accent".into(), value: TokenValue::Color("#10B981".into()), description: "绿色强调".into() },
            Token { name: "font.size.body".into(), value: TokenValue::Number(14.0), description: "正文字号".into() },
            Token { name: "radius.card".into(), value: TokenValue::Number(6.0), description: "卡片圆角".into() },
        ],
    }
}

fn builtin_robot_sim() -> DesignSystem {
    DesignSystem {
        id: "robot-sim".into(),
        name: "机器人仿真控制台".into(),
        tokens: vec![
            Token { name: "color.bg".into(), value: TokenValue::Color("#0F172A".into()), description: "深色背景".into() },
            Token { name: "color.fg".into(), value: TokenValue::Color("#E2E8F0".into()), description: "浅色前景".into() },
            Token { name: "color.warn".into(), value: TokenValue::Color("#F59E0B".into()), description: "警告色".into() },
            Token { name: "color.danger".into(), value: TokenValue::Color("#EF4444".into()), description: "危险/急停色".into() },
            Token { name: "font.size.body".into(), value: TokenValue::Number(13.0), description: "等宽字号".into() },
            Token { name: "radius.panel".into(), value: TokenValue::Number(4.0), description: "面板圆角".into() },
        ],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_register_and_activate() {
        let mut reg = DesignSystemRegistry::new();
        reg.register(builtin_apple_hig()).unwrap();
        reg.activate("apple-hig").unwrap();
        assert!(reg.active().is_some());
        assert_eq!(reg.active().unwrap().name, "Apple HIG");
    }

    #[test]
    fn registry_duplicate_rejected() {
        let mut reg = DesignSystemRegistry::new();
        reg.register(builtin_apple_hig()).unwrap();
        assert!(reg.register(builtin_apple_hig()).is_err());
    }

    #[test]
    fn registry_activate_unknown_rejected() {
        let mut reg = DesignSystemRegistry::new();
        assert!(reg.activate("nope").is_err());
    }

    #[test]
    fn register_builtin_loads_three() {
        let mut reg = DesignSystemRegistry::new();
        reg.register_builtin();
        assert_eq!(reg.list().len(), 3);
        reg.activate("robot-sim").unwrap();
        assert!(reg.lookup("color.danger").is_some());
    }

    #[test]
    fn lookup_returns_token_value() {
        let mut reg = DesignSystemRegistry::new();
        reg.register_builtin();
        reg.activate("apple-hig").unwrap();
        match reg.lookup("color.accent").unwrap() {
            TokenValue::Color(c) => assert_eq!(c, "#007AFF"),
            _ => panic!("期望 Color"),
        }
    }

    #[test]
    fn lookup_missing_returns_none() {
        let mut reg = DesignSystemRegistry::new();
        reg.register_builtin();
        reg.activate("apple-hig").unwrap();
        assert!(reg.lookup("no.such.token").is_none());
    }

    #[test]
    fn lookup_without_active_returns_none() {
        let reg = DesignSystemRegistry::new();
        assert!(reg.lookup("color.bg").is_none());
    }

    #[test]
    fn export_import_roundtrip() {
        let mut reg = DesignSystemRegistry::new();
        reg.register_builtin();
        reg.activate("minimal-dashboard").unwrap();
        let json = reg.export_json().unwrap();
        let mut reg2 = DesignSystemRegistry::new();
        reg2.import_json(&json).unwrap();
        assert_eq!(reg2.list().len(), 1);
    }

    #[test]
    fn import_invalid_json_rejected() {
        let mut reg = DesignSystemRegistry::new();
        assert!(reg.import_json("not json").is_err());
    }

    #[test]
    fn token_value_serde_roundtrip() {
        let v = TokenValue::Color("#FFF".into());
        let s = serde_json::to_string(&v).unwrap();
        let v2: TokenValue = serde_json::from_str(&s).unwrap();
        assert_eq!(v, v2);
    }
}
