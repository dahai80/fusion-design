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
    String(String),       // 字体族等 / token:xxx 引用
}

impl TokenValue {
    /// 转换为 CSS 属性值字符串。
    /// - Color → 直接输出 hex
    /// - Number → 输出 `Npx`（字号/间距/圆角默认 px）
    /// - Shadow → 直接输出 CSS box-shadow
    /// - String → 直接输出（字体族等）；若为 `token:xxx` 引用则输出 `var(--xxx)`
    pub fn to_css_value(&self) -> String {
        match self {
            TokenValue::Color(c) => c.clone(),
            TokenValue::Number(n) => format!("{}px", n),
            TokenValue::Shadow(s) => s.clone(),
            TokenValue::String(s) => {
                if let Some(ref_name) = s.strip_prefix("token:") {
                    format!("var(--{})", ref_name)
                } else {
                    s.clone()
                }
            }
        }
    }

    /// 检查是否为 token 引用（`token:xxx` 格式）。
    pub fn is_reference(&self) -> bool {
        match self {
            TokenValue::String(s) => s.starts_with("token:"),
            _ => false,
        }
    }

    /// 提取引用目标名（`token:xxx` → `xxx`）。
    pub fn reference_target(&self) -> Option<&str> {
        match self {
            TokenValue::String(s) => s.strip_prefix("token:"),
            _ => None,
        }
        .filter(|s| !s.is_empty())
    }
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dark_tokens: Option<Vec<Token>>,
}

/// 主题模式。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum Theme {
    #[default]
    Light,
    Dark,
}

impl DesignSystem {
    /// 生成 CSS Custom Properties 输出：`:root { --token-name: value; ... }`
    /// Token name 中的 `.` 替换为 `-` 以符合 CSS 自定义属性命名规范。
    /// 引用类型 token（`token:xxx`）会递归解析为最终值。
    pub fn to_css_custom_properties(&self) -> String {
        let mut lines = vec![":root {".to_string()];
        let mut visited = std::collections::HashSet::new();
        for token in &self.tokens {
            let css_name = token.name.replace('.', "-");
            let resolved = self.resolve_reference(&token.value, &mut visited);
            lines.push(format!("  --{}: {};", css_name, resolved));
            visited.clear();
        }
        lines.push("}".to_string());
        lines.join("\n")
    }

    /// 解析 token 引用：若值为 `token:xxx` 则查找目标 token 的值并递归解析。
    /// 防止循环引用（最多解析 8 层）。
    pub fn resolve_reference(
        &self,
        value: &TokenValue,
        visited: &mut std::collections::HashSet<String>,
    ) -> String {
        if let Some(target) = value.reference_target() {
            if visited.contains(target) {
                tracing::warn!(
                    "检测到循环 token 引用: {:?}, 已访问: {:?}",
                    target,
                    visited
                );
                return value.to_css_value();
            }
            visited.insert(target.to_string());
            if let Some(resolved) = self.find_token_value(target) {
                self.resolve_reference(resolved, visited)
            } else {
                tracing::warn!("Token 引用目标未找到: {}", target);
                value.to_css_value()
            }
        } else {
            value.to_css_value()
        }
    }

    /// 按 name 查找 token 值。
    fn find_token_value(&self, name: &str) -> Option<&TokenValue> {
        self.tokens
            .iter()
            .find(|t| t.name == name)
            .map(|t| &t.value)
    }

    /// 按主题模式生成 CSS Custom Properties。
    /// Light 使用 self.tokens，Dark 使用 self.dark_tokens（如无则回退到 tokens）。
    pub fn to_css_custom_properties_for_theme(&self, theme: Theme) -> String {
        let tokens = match theme {
            Theme::Light => &self.tokens,
            Theme::Dark => self.dark_tokens.as_ref().unwrap_or(&self.tokens),
        };
        let mut lines = vec![":root {".to_string()];
        let mut visited = std::collections::HashSet::new();
        for token in tokens {
            let css_name = token.name.replace('.', "-");
            let resolved = self.resolve_reference(&token.value, &mut visited);
            lines.push(format!("  --{}: {};", css_name, resolved));
            visited.clear();
        }
        lines.push("}".to_string());
        lines.join("\n")
    }
}

/// 设计系统注册中心（管理多套规范，支持一键切换）。
#[derive(Debug, Default)]
pub struct DesignSystemRegistry {
    systems: HashMap<String, DesignSystem>,
    active_id: Option<String>,
    active_theme: Theme,
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

    /// 切换主题模式（Light/Dark）。
    pub fn activate_theme(&mut self, theme: Theme) {
        self.active_theme = theme;
        tracing::info!(theme = ?theme, "主题已切换");
    }

    /// 获取当前主题模式。
    pub fn current_theme(&self) -> Theme {
        self.active_theme
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

    /// 按 ID 获取规范。
    pub fn get(&self, id: &str) -> Option<&DesignSystem> {
        self.systems.get(id)
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

pub fn builtin_apple_hig() -> DesignSystem {
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
        dark_tokens: Some(vec![
            Token { name: "color.bg".into(), value: TokenValue::Color("#1C1C1E".into()), description: "暗色背景".into() },
            Token { name: "color.fg".into(), value: TokenValue::Color("#F2F2F7".into()), description: "暗色前景".into() },
            Token { name: "color.accent".into(), value: TokenValue::Color("#0A84FF".into()), description: "暗色强调".into() },
            Token { name: "font.size.body".into(), value: TokenValue::Number(17.0), description: "正文字号".into() },
            Token { name: "radius.card".into(), value: TokenValue::Number(10.0), description: "卡片圆角".into() },
        ]),
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
        dark_tokens: None,
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
        dark_tokens: None,
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

    // ── TokenValue::to_css_value 测试 ──

    #[test]
    fn token_value_color_to_css() {
        let v = TokenValue::Color("#007AFF".into());
        assert_eq!(v.to_css_value(), "#007AFF");
    }

    #[test]
    fn token_value_number_to_css() {
        let v = TokenValue::Number(17.0);
        assert_eq!(v.to_css_value(), "17px");
    }

    #[test]
    fn token_value_shadow_to_css() {
        let v = TokenValue::Shadow("0 2px 4px rgba(0,0,0,0.1)".into());
        assert_eq!(v.to_css_value(), "0 2px 4px rgba(0,0,0,0.1)");
    }

    #[test]
    fn token_value_string_to_css() {
        let v = TokenValue::String("SF Pro, sans-serif".into());
        assert_eq!(v.to_css_value(), "SF Pro, sans-serif");
    }

    #[test]
    fn token_value_reference_to_css_var() {
        let v = TokenValue::String("token:color.accent".into());
        assert_eq!(v.to_css_value(), "var(--color.accent)");
    }

    #[test]
    fn token_value_is_reference() {
        assert!(TokenValue::String("token:color.accent".into()).is_reference());
        assert!(!TokenValue::String("SF Pro".into()).is_reference());
        assert!(!TokenValue::Color("#FFF".into()).is_reference());
    }

    #[test]
    fn token_value_reference_target() {
        let v = TokenValue::String("token:color.accent".into());
        assert_eq!(v.reference_target(), Some("color.accent"));
        let v2 = TokenValue::String("no-ref".into());
        assert_eq!(v2.reference_target(), None);
    }

    // ── DesignSystem::to_css_custom_properties 测试 ──

    #[test]
    fn design_system_css_custom_properties() {
        let ds = builtin_apple_hig();
        let css = ds.to_css_custom_properties();
        assert!(css.starts_with(":root {"));
        assert!(css.contains("--color-bg: #F2F2F7;"));
        assert!(css.contains("--color-fg: #1C1C1E;"));
        assert!(css.contains("--color-accent: #007AFF;"));
        assert!(css.contains("--font-size-body: 17px;"));
        assert!(css.contains("--radius-card: 10px;"));
        assert!(css.ends_with("}"));
    }

    #[test]
    fn design_system_css_with_references() {
        let ds = DesignSystem {
            id: "ref-test".into(),
            name: "Ref Test".into(),
            tokens: vec![
                Token { name: "color.primary".into(), value: TokenValue::Color("#007AFF".into()), description: "主色".into() },
                Token { name: "color.button".into(), value: TokenValue::String("token:color.primary".into()), description: "按钮色引用主色".into() },
            ],
            dark_tokens: None,
        };
        let css = ds.to_css_custom_properties();
        // 引用 token 解析为实际值
        assert!(css.contains("--color-button: #007AFF;"));
    }

    #[test]
    fn design_system_circular_reference_safe() {
        let ds = DesignSystem {
            id: "circular".into(),
            name: "Circular".into(),
            tokens: vec![
                Token { name: "a".into(), value: TokenValue::String("token:b".into()), description: "循环A".into() },
                Token { name: "b".into(), value: TokenValue::String("token:a".into()), description: "循环B".into() },
            ],
            dark_tokens: None,
        };
        let css = ds.to_css_custom_properties();
        // 循环引用不会 panic，回退到原始 CSS var 输出
        assert!(css.contains("--a:") || css.contains("--b:"));
    }

    // ── DesignSystem::resolve_reference 测试 ──

    #[test]
    fn resolve_direct_value() {
        let ds = builtin_apple_hig();
        let mut visited = std::collections::HashSet::new();
        let val = ds.resolve_reference(&TokenValue::Color("#FFF".into()), &mut visited);
        assert_eq!(val, "#FFF");
    }

    #[test]
    fn resolve_single_reference() {
        let ds = builtin_apple_hig();
        let mut visited = std::collections::HashSet::new();
        let val = ds.resolve_reference(
            &TokenValue::String("token:color.accent".into()),
            &mut visited,
        );
        assert_eq!(val, "#007AFF");
    }

    #[test]
    fn resolve_chained_reference() {
        let ds = DesignSystem {
            id: "chain".into(),
            name: "Chain".into(),
            tokens: vec![
                Token { name: "color.base".into(), value: TokenValue::Color("#007AFF".into()), description: "基础色".into() },
                Token { name: "color.mid".into(), value: TokenValue::String("token:color.base".into()), description: "中间引用".into() },
                Token { name: "color.top".into(), value: TokenValue::String("token:color.mid".into()), description: "顶层引用".into() },
            ],
            dark_tokens: None,
        };
        let mut visited = std::collections::HashSet::new();
        let val = ds.resolve_reference(
            &TokenValue::String("token:color.top".into()),
            &mut visited,
        );
        assert_eq!(val, "#007AFF");
    }

    #[test]
    fn resolve_missing_reference_falls_back() {
        let ds = builtin_apple_hig();
        let mut visited = std::collections::HashSet::new();
        let val = ds.resolve_reference(
            &TokenValue::String("token:nonexistent".into()),
            &mut visited,
        );
        // 回退到原始 CSS var 输出
        assert_eq!(val, "var(--nonexistent)");
    }
}
