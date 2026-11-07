//! KDE (KWin) 集成 —— 所有「Wayland 下 winit 做不到、只能求合成器」的活都在这。
//!
//! 拆成四个文件（用户第二十九轮，原先 1200 行挤在一个文件里）：
//!
//! | 文件 | 职责 |
//! |---|---|
//! | [`rules`] | KWin **窗口规则**（`~/.config/kwinrulesrc` 文本手术 + DBus 通知重载）：跳过任务栏/Alt-Tab、HUD 强制置顶、HUD 位置记忆 |
//! | [`script`] | 一次性 **KWin 脚本**：停放主窗口 / 解除停放 / 取消最小化并激活 / 清置顶 |
//! | [`listener`] | **常驻监听脚本**（位置上报 + 夹回屏幕 + 主窗口最小化拦截）+ 接收上报的 DBus 服务 |
//!
//! 三条通用约束（都踩过）：
//! 1. **窗口规则定位**：KWin `Rules::SetRule` 枚举中 `3 = Apply`（Apply Initially，初始应用后允许用户拖动），
//!    `2 = Force`（强制锁定），`1 = DontAffect`（不干预）。KWin 6 脚本环境同时支持 `w.frameGeometry = {x,y,width,height}`。
//! 2. **脚本名必须每次唯一**：`Scripting.start()` 只执行「尚未启动过」的脚本，同名会被静默跳过。
//! 3. **别卸载刚加载的脚本**：KWin 执行是异步的，立即卸载有竞态；只卸载「上一次」的。
//!
//! 规则文件写入前会备份为 `kwinrulesrc.bak`，只增删带本程序标记的规则、保留用户其他规则。

#![cfg(target_os = "linux")]

use std::path::PathBuf;

mod listener;
mod rules;
mod script;

pub(crate) use listener::{
    hud_pos_reports, start_hud_pos_service, stop_hud_listener,
    sync_hud_listener, take_main_hidden,
};
pub(crate) use rules::{set_hud_above_public, set_skip_taskbar, sync_hud_rules};
pub(crate) use script::{
    clear_keep_above, enforce_skip_taskbar, park_main_window, unminimize_activate,
    unpark_main_window,
};

/// 是否 KDE 会话（只有 KWin 支持这套窗口规则 + 脚本通道）
/// 单元测试下始终返回 `false`，防止测试污染宿主真实的 `~/.config/kwinrulesrc` 与 `/tmp/kmc-hudpos.js`。
pub fn is_kde() -> bool {
    if cfg!(test) {
        return false;
    }
    std::env::var("XDG_CURRENT_DESKTOP")
        .map(|v| v.to_uppercase().contains("KDE"))
        .unwrap_or(false)
}

/// HUD 位置通道是否可用：仅 KDE + Wayland —— 只有 KWin 有「常驻脚本 + DBus 上报」这套通道。
/// 非 KDE 的 Wayland（GNOME 等）没有等价物，位置记忆只能优雅降级。
pub(crate) fn hud_pos_supported() -> bool {
    is_kde() && crate::gui::window::is_wayland()
}

fn rules_path() -> PathBuf {
    dirs::config_dir().unwrap_or_else(|| PathBuf::from(".")).join("kwinrulesrc")
}
