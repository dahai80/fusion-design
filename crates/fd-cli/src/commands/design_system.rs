//! A-3：设计规范相关子命令 handler（ListDesignSystems/Activate/TokenCSS/Theme）。

use crate::ThemeModeArg; // 若 Step 0 发现非 pub，先补 pub

pub async fn list_design_systems() -> anyhow::Result<()> {
    let mut reg = fd_design_system::DesignSystemRegistry::new();
    reg.register_builtin();
    for id in reg.list() {
        println!("{id}");
    }
    Ok(())
}

pub async fn activate(id: String) -> anyhow::Result<()> {
    let mut reg = fd_design_system::DesignSystemRegistry::new();
    reg.register_builtin();
    reg.activate(&id)?;
    println!("已激活: {id}");
    Ok(())
}

pub async fn token_css(design_system: String) -> anyhow::Result<()> {
    let mut reg = fd_design_system::DesignSystemRegistry::new();
    reg.register_builtin();
    let system = reg.get(&design_system).ok_or_else(|| {
        anyhow::anyhow!("设计规范 '{design_system}' 未找到，可用: {:?}", reg.list())
    })?;
    println!("{}", system.to_css_custom_properties());
    Ok(())
}

pub async fn theme(design_system: String, mode: ThemeModeArg) -> anyhow::Result<()> {
    let mut reg = fd_design_system::DesignSystemRegistry::new();
    reg.register_builtin();
    let system = reg.get(&design_system).ok_or_else(|| {
        anyhow::anyhow!("设计规范 '{design_system}' 未找到，可用: {:?}", reg.list())
    })?;
    let css = system.to_css_custom_properties_for_theme(mode.into());
    println!("{css}");
    Ok(())
}
