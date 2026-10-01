//! 托盘命令控制线程：**不依赖渲染帧**地处理托盘菜单命令。
//!
//! 必要性：Wayland 下窗口最小化后合成器不再发送帧回调，egui 的 update() 不再运行，
//! 若把命令处理放在渲染循环里，「退出」在窗口最小化时就会失效（关不掉的坑）。
//! 因此这里用独立线程消费命令：退出路径直接保存并立即终止进程。

use crate::config::Config;
use crate::stats::Shared;
use crate::tray::TrayCmd;
use eframe::egui;
use parking_lot::Mutex;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::Arc;

/// 跨线程 UI 请求标志（托盘 → GUI 渲染循环）
#[derive(Default)]
pub struct UiFlags {
    /// 请求打开设置面板
    pub want_settings: AtomicBool,
    /// 调试：允许定时切页
    pub switch_page: AtomicBool,
    /// 调试：请求切换到另一个页面
    pub switch_page_after: AtomicBool,
    /// 托盘呼出窗口时：把视图切回「今日」
    pub show_today: AtomicBool,
}

pub struct Controller {
    pub shared: Arc<Shared>,
    pub cfg: Arc<Mutex<Config>>,
    pub cfg_path: PathBuf,
    pub stats_path: PathBuf,
    pub autostart: Arc<AtomicBool>,
    pub flags: Arc<UiFlags>,
    pub tray_ctl: Option<Sender<TrayCmd>>,
}

/// 启动控制线程
pub fn spawn(ctl: Controller, rx: Receiver<TrayCmd>, ctx: egui::Context, show_request_path: std::path::PathBuf) {
    // 二次启动“叫回窗口”：独立线程轮询请求文件（渲染帧在被遮挡/收起时可能停止，不能依赖）
    let ctl_flags = ctl.flags.clone();
    {
        let shared = ctl.shared.clone();
        let ctx2 = ctx.clone();
        let keep_cfg = ctl.cfg.clone();
        std::thread::Builder::new()
            .name("kmcounter-showreq".into())
            .spawn(move || {
                let mut last = std::fs::metadata(&show_request_path).and_then(|m| m.modified()).ok();
                loop {
                    std::thread::sleep(std::time::Duration::from_millis(700));
                    let cur = std::fs::metadata(&show_request_path).and_then(|m| m.modified()).ok();
                    let trigger = match (cur, last) {
                        (Some(a), Some(b)) => a != b,
                        (Some(_), None) => true,
                        _ => false,
                    };
                    if cur.is_some() {
                        last = cur;
                    }
                    if trigger {
                        log::info!("收到显示窗口请求（另一实例启动）");
                        let keep = keep_cfg.lock().keep_on_top;
                        ctl_flags.show_today.store(true, Ordering::Relaxed);
                        show_window(&shared, &ctx2, keep);
                    }
                }
            })
            .ok();
    }

    std::thread::Builder::new()
        .name("kmcounter-ctl".into())
        .spawn(move || {
            while let Ok(cmd) = rx.recv() {
                match cmd {
                    TrayCmd::Show | TrayCmd::Settings => {
                        let settings = matches!(cmd, TrayCmd::Settings);
                        log::info!("托盘：显示窗口{}", if settings { "（打开设置）" } else { "" });
                        if settings {
                            ctl.flags.want_settings.store(true, Ordering::Relaxed);
                        }
                        // 呼出即回到「今日」：多数时候想看的就是今天
                        ctl.flags.show_today.store(true, Ordering::Relaxed);
                        let keep = ctl.cfg.lock().keep_on_top;
                        show_window(&ctl.shared, &ctx, keep);
                    }
                    TrayCmd::ToggleAutostart => {
                        let new_state = {
                            let mut cfg = ctl.cfg.lock();
                            let s = !cfg.autostart;
                            crate::autostart::set(s);
                            cfg.autostart = s;
                            let _ = cfg.save(&ctl.cfg_path);
                            s
                        };
                        ctl.autostart.store(new_state, Ordering::Relaxed);
                        if let Some(tx) = &ctl.tray_ctl {
                            let _ = tx.send(TrayCmd::AutostartState(new_state));
                        }
                    }
                    // GUI → 托盘方向的命令（回执/语言同步），控制线程无需处理
                    TrayCmd::AutostartState(_) | TrayCmd::Language(_) => {}
                    TrayCmd::Exit => {
                        // 立即保存并终止（不经过渲染循环，最小化/隐藏时同样有效）
                        log::info!("托盘退出：保存数据并退出");
                        let store = (*ctl.shared.store.lock()).clone();
                        if let Err(e) = store.save(&ctl.stats_path) {
                            log::error!("退出保存统计数据失败: {e}");
                        }
                        let _ = ctl.cfg.lock().save(&ctl.cfg_path);
                        crate::force_exit();
                    }
                }
            }
            log::warn!("托盘命令通道已关闭");
        })
        .ok();
}

/// 显示并聚焦窗口（若处于收起状态则先恢复尺寸）
fn show_window(shared: &Arc<Shared>, ctx: &egui::Context, keep_on_top: bool) {
    crate::gui::show_window_now(shared, ctx, keep_on_top);
}
