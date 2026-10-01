//! 开机启动：Windows 写注册表 Run 键，Linux 写 XDG autostart .desktop 文件。

/// 当前是否已启用开机启动
pub fn is_enabled() -> bool {
    #[cfg(windows)]
    {
        use winreg::enums::HKEY_CURRENT_USER;
        use winreg::RegKey;
        RegKey::predef(HKEY_CURRENT_USER)
            .open_subkey("Software\\Microsoft\\Windows\\CurrentVersion\\Run")
            .and_then(|k| k.get_value::<String, _>("KMCounter-rs"))
            .is_ok()
    }
    #[cfg(target_os = "linux")]
    {
        autostart_path().exists()
    }
}

/// 设置开机启动状态
pub fn set(enable: bool) {
    let result = if enable { enable_it() } else { disable_it() };
    if let Err(e) = result {
        log::error!("设置开机启动失败: {e}");
    }
}

#[cfg(windows)]
fn enable_it() -> std::io::Result<()> {
    use winreg::enums::HKEY_CURRENT_USER;
    use winreg::RegKey;
    let exe = std::env::current_exe().map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let key = hkcu.open_subkey_with_flags("Software\\Microsoft\\Windows\\CurrentVersion\\Run", winreg::enums::KEY_SET_VALUE)?;
    key.set_value("KMCounter-rs", &format!("\"{}\"", exe.display()))?;
    Ok(())
}

#[cfg(windows)]
fn disable_it() -> std::io::Result<()> {
    use winreg::enums::HKEY_CURRENT_USER;
    use winreg::RegKey;
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let _ = match hkcu.open_subkey_with_flags("Software\\Microsoft\\Windows\\CurrentVersion\\Run", winreg::enums::KEY_SET_VALUE) {
        Ok(key) => key.delete_value("KMCounter-rs").ok(),
        Err(_) => None,
    };
    Ok(())
}

#[cfg(target_os = "linux")]
fn autostart_path() -> std::path::PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join("autostart/kmcounter.desktop")
}

#[cfg(target_os = "linux")]
fn enable_it() -> std::io::Result<()> {
    use std::io::Write;
    let exe = std::env::current_exe().map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
    let path = autostart_path();
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let content = format!(
        "[Desktop Entry]\nType=Application\nName=KMCounter-rs\nComment=Keyboard & mouse counter\nExec=\"{}\"\nTerminal=false\nX-GNOME-Autostart-enabled=true\nCategories=Utility;\n",
        exe.display()
    );
    let mut f = std::fs::File::create(&path)?;
    f.write_all(content.as_bytes())?;
    Ok(())
}

#[cfg(target_os = "linux")]
fn disable_it() -> std::io::Result<()> {
    match std::fs::remove_file(autostart_path()) {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }
    Ok(())
}
