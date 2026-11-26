//! A-3: design-system subcommand handlers (ListDesignSystems/Activate/TokenCSS/Theme).

use crate::ThemeModeArg; // if Step 0 finds this non-pub, make it pub first

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
    println!("activated: {id}");
    Ok(())
}

pub async fn token_css(design_system: String) -> anyhow::Result<()> {
    let mut reg = fd_design_system::DesignSystemRegistry::new();
    reg.register_builtin();
    let system = reg.get(&design_system).ok_or_else(|| {
        anyhow::anyhow!(
            "design system '{design_system}' not found, available: {:?}",
            reg.list()
        )
    })?;
    println!("{}", system.to_css_custom_properties());
    Ok(())
}

pub async fn theme(design_system: String, mode: ThemeModeArg) -> anyhow::Result<()> {
    let mut reg = fd_design_system::DesignSystemRegistry::new();
    reg.register_builtin();
    let system = reg.get(&design_system).ok_or_else(|| {
        anyhow::anyhow!(
            "design system '{design_system}' not found, available: {:?}",
            reg.list()
        )
    })?;
    let css = system.to_css_custom_properties_for_theme(mode.into());
    println!("{css}");
    Ok(())
}
