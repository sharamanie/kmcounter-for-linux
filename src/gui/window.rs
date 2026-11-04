//! 主窗口的显示 / 隐藏 / 停放 —— 从 `gui.rs` 拆出（用户第二十九轮）。
//!
//! **为什么单独成模块**：三个平台对「隐藏窗口」的语义完全不同，而 Wayland 下还有个
//! 反直觉的硬约束 —— **不能 unmap**（最小化 / `Visible(false)` 都不行）：主窗口一 unmap，
//! winit 就不再唤醒 eframe 的事件循环，连 `request_redraw` 都不再发出，**所有视口一起停摆**
//! （实测：两个 HUD 全部冻结，HUD 自己还 mapped 也没用）。
//! 于是 Wayland 走「停放」：缩到 32px + 全透明 + 压到最底层，但保持 mapped。
//!
//! 这套判断如果散在 `App::update` 的尺寸自适应逻辑里（"停放期间不下发尺寸 / 不记
//! `last_seen` / 不判定用户手动缩放"）非常容易漏一处就出 bug —— 所以集中到这里，
//! `App::update` 只需要问一句 [`is_parked`]。
//!
//! 平台语义速查：
//!
//! | 平台 | 「收起」怎么做 | 「呼出」怎么做 |
//! |---|---|---|
//! | X11 | `ViewportCommand::Visible(false)` | 反向命令 |
//! | Windows | 直接 Win32 `ShowWindow(SW_HIDE)`（窗口隐藏后渲染循环可能停摆，egui 命令等不到） | `ShowWindow(SW_SHOW/SW_RESTORE)` + 置顶 |
//! | KDE/Wayland | **停放**：KWin 脚本全透明 + 压底层，尺寸由本模块压到 [`PARK_SIZE`] | KWin 脚本取消最小化 + 激活 |
//!
//! 非 KDE 的 Wayland（GNOME 等）没有 KWin 脚本通道：`kwin::*` 全是空操作，
//! 停放只剩「缩到 32px + 但我们没法让它透明」—— 视觉上会留一个 32px 的小方块。
//! 属于已知降级，等有等价通道再补。

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use eframe::egui;
use egui::Vec2;

use crate::stats::Shared;

/// 主窗口的下限尺寸：`ViewportBuilder::with_min_inner_size` 建窗口时用一次，
/// 停放解除后还要再发一次（停放期间被临时降到了 [`PARK_SIZE`]）——
/// 所以必须是常量，不能只在 `main.rs` 里写死数值（两处不一致会让窗口卡在错误的下限上）。
pub const MAIN_MIN_W: f32 = 460.0;
pub const MAIN_MIN_H: f32 = 360.0;

/// 收起到托盘时主窗口被压到多大（够小到不挡点击，又不会被合成器当成非法尺寸）。
/// **不动位置**：窗口留在原地（左上角不变），恢复时也就不用记原位 ——
/// Wayland 下 egui 拿不到窗口全局坐标，记下来的原位根本不可用。
pub const PARK_SIZE: f32 = 32.0;

/// 是否运行在 Wayland 会话（winit 下 `set_visible` / `set_outer_position` 都是空实现）
pub(crate) fn is_wayland() -> bool {
    std::env::var_os("WAYLAND_DISPLAY").is_some()
}

/// 主窗口是否已收起到托盘（停放中）。
///
/// 注意这是**跨线程**状态（托盘线程 / HUD 视口回调都会改），唯一真源是
/// [`Shared::ui_hidden`]；本模块只负责在 Wayland 下把它翻译成「停放」。
pub(crate) fn is_parked(shared: &Arc<Shared>) -> bool {
    shared.ui_hidden.load(Ordering::Relaxed)
}

/// 收起窗口到托盘（三个平台三条路，见模块头部的表）。
pub fn hide_window(shared: &Arc<Shared>, ctx: &egui::Context) {
    shared.ui_hidden.store(true, Ordering::Relaxed);
    if is_wayland() {
        // Wayland 下 winit 的 set_visible 是空实现、最小化又会让事件循环停摆，
        // 所以只做两件事：标记状态 + 让 KWin 把窗口透明化并压到最底层。
        // 缩尺寸不在这里发：一次性命令会被丢（第 1 帧窗口还没定型），
        // 交给 [`parked_tick`] 每帧重试。
        #[cfg(target_os = "linux")]
        std::thread::spawn(|| {
            crate::kwin::park_main_window();
            crate::kwin::clear_keep_above();
        });
        log::info!("窗口已收起到托盘（停放：保持 mapped，HUD 继续更新）");
    } else {
        #[cfg(windows)]
        win32_window::hide();
        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
        log::info!("窗口已隐藏到托盘");
    }
}

/// 呼出窗口（取消收起 / 解除停放 / 取消最小化 + 激活）。
pub fn show_window_now(shared: &Arc<Shared>, ctx: &egui::Context, keep_on_top: bool) {
    show_window_impl(shared, ctx, None, keep_on_top);
}

/// 从 **HUD 视口回调**里叫回主窗口（双击悬浮窗）。
///
/// 与 [`show_window_now`] 的唯一区别：egui 命令显式发给 ROOT 视口 —— 回调里的
/// "当前视口"是 HUD 自己，直接发命令会打到 HUD 上（尺寸/可见性全落到悬浮窗身上）。
pub(crate) fn show_window_from_hud(shared: &Arc<Shared>, ctx: &egui::Context, keep_on_top: bool) {
    show_window_impl(shared, ctx, Some(egui::ViewportId::ROOT), keep_on_top);
}

fn show_window_impl(
    shared: &Arc<Shared>,
    ctx: &egui::Context,
    target: Option<egui::ViewportId>,
    keep_on_top: bool,
) {
    shared.ui_hidden.store(false, Ordering::Relaxed);
    let send = |c: egui::ViewportCommand| match target {
        Some(vp) => ctx.send_viewport_cmd_to(vp, c),
        None => ctx.send_viewport_cmd(c),
    };
    // 停放时把最小尺寸降到了 PARK_SIZE，这里恢复常态值 —— 否则窗口会被永久限制在 32px 的下限上
    //（实际尺寸由自适应逻辑下发，见 `App::update` 里「停放解除后强制下发尺寸」）
    send(egui::ViewportCommand::MinInnerSize(egui::vec2(
        MAIN_MIN_W,
        MAIN_MIN_H,
    )));
    if is_wayland() {
        // Wayland 下 winit 的 Visible/Focus/Minimized(false) 全是空实现：
        // 无论窗口是最小化、被停放还是被其他窗口挡住，都交给 KWin 脚本（异步执行）。
        // **必须解除停放**（不透明 + 回正常层级 + 撤销最小化）：只取消最小化的话，窗口
        // 虽然"恢复了"却还是全透明 + 32px 高 + 压在最底层 —— 看起来就是"点了没反应"（踩过）。
        #[cfg(target_os = "linux")]
        std::thread::spawn(move || {
            crate::kwin::unpark_main_window(keep_on_top);
            crate::kwin::unminimize_activate(keep_on_top);
        });
        #[cfg(not(target_os = "linux"))]
        let _ = keep_on_top; // 非 Linux 没有 KWin 置顶逻辑
    }
    // Windows：直接调 Win32 显示/激活（隐藏状态下渲染循环可能停摆，等不到 egui 命令）
    #[cfg(windows)]
    win32_window::show(keep_on_top);
    #[cfg(not(windows))]
    let _ = keep_on_top;
    send(egui::ViewportCommand::Minimized(false));
    send(egui::ViewportCommand::Visible(true));
    send(egui::ViewportCommand::Focus);
    ctx.request_repaint();
}

/// 停放期间每帧调用：把主窗口压到 [`PARK_SIZE`]，直到实测尺寸到位为止。
///
/// **必须重试而不是发一次**：`InnerSize` 在第 1 帧下发时窗口还没定型，命令会被整个丢掉
/// （实测尺寸一直停在 1270x540）—— 所以限流 1 秒反复下发，到位即停。
///
/// 还要先发 `MinInnerSize`：窗口建了 `min_inner_size = 460x360`，不先降下来 egui 会把
/// `InnerSize` 夹到最小值，压根缩不动。
pub(crate) fn parked_tick(shared: &Arc<Shared>, ctx: &egui::Context, actual: Vec2) {
    if !is_parked(shared) || !is_wayland() {
        return; // 非 Wayland 是真的隐藏了，没有「压尺寸」这回事
    }
    if actual.x <= PARK_SIZE + 4.0 && actual.y <= PARK_SIZE + 4.0 {
        return; // 已经到位
    }
    static PARK_AT: AtomicU64 = AtomicU64::new(0);
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    if now_ms.saturating_sub(PARK_AT.load(Ordering::Relaxed)) < 1000 {
        return; // 限流：别每帧都发（白烧 CPU）
    }
    PARK_AT.store(now_ms, Ordering::Relaxed);
    let park = egui::vec2(PARK_SIZE, PARK_SIZE);
    ctx.send_viewport_cmd(egui::ViewportCommand::MinInnerSize(park));
    ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(park));
}

/// Windows：直接调 Win32 显示/隐藏主窗口。
/// 原因：窗口被 `Visible(false)` 隐藏后，渲染循环可能收不到重绘事件，egui 的
/// ViewportCommand 就一直没人处理 —— 托盘点击也就“没反应”。这里绕开渲染循环，
/// 直接找到本进程的主窗口做 ShowWindow/SetForegroundWindow。
///
/// 注意：不能拿“本进程第一个顶层窗口”当主窗口。进程里还住着 OpenGL 驱动选像素格式时
/// 建的假窗口（NVIDIA 的 `NVOpenGLPbuffer`）、winit 的线程消息窗口、输入法注入的窗口，
/// 谁先谁后由 Z 序决定；一旦选中它们，之后每次显示/隐藏都落空——日志照样写“已显示窗口”，
/// 真正的主窗口却再也回不来（托盘点了没反应）。所以这里按确定的特征认窗口：
///   1. 窗口类名是 winit 主窗口类（`Window Class`）；
///   2. 或者标题以程序标题开头（egui 每帧设置，形如 `KMCounter-rs v0.1.0 | …`）；
/// 两者都没有时才退用“第一个非辅助窗口”，且这种猜测不缓存、每次重新找。
#[cfg(windows)]
pub(crate) mod win32_window {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use windows_sys::Win32::Foundation::{HWND, LPARAM, TRUE};
    use windows_sys::Win32::UI::WindowsAndMessaging::*;

    /// winit 主窗口的窗口类名
    const WINIT_CLASS: &str = "Window Class";
    /// 窗口标题前缀（与 gui::draw 里 ViewportCommand::Title 的前缀一致）
    const TITLE_PREFIX: &str = "KMCounter-rs";

    static MAIN_HWND: AtomicUsize = AtomicUsize::new(0);

    fn class_of(hwnd: HWND) -> String {
        let mut buf = [0u16; 128];
        let n = unsafe { GetClassNameW(hwnd, buf.as_mut_ptr(), buf.len() as i32) };
        String::from_utf16_lossy(&buf[..n.max(0) as usize])
    }

    fn title_of(hwnd: HWND) -> String {
        let mut buf = [0u16; 256];
        let n = unsafe { GetWindowTextW(hwnd, buf.as_mut_ptr(), buf.len() as i32) };
        String::from_utf16_lossy(&buf[..n.max(0) as usize])
    }

    /// 进程内的辅助窗口：托盘自己的隐藏窗口、OpenGL 驱动的像素格式窗口、
    /// winit 的消息窗口、输入法（TSF/搜狗）注入的窗口——显示/隐藏它们都看不见任何效果
    fn is_helper(class: &str) -> bool {
        class.contains("KMCounterTrayWnd")
            || class.contains("Pbuffer")
            || class.contains("Winit Thread Event Target")
            || class.contains("IME")
            || class.contains("Sogou")
            || class.starts_with("SoPY")
    }

    /// 是否确定是本程序的主窗口（winit 的窗口类，或 egui 设置的标题）
    fn is_main_window(hwnd: HWND) -> bool {
        class_of(hwnd) == WINIT_CLASS || title_of(hwnd).starts_with(TITLE_PREFIX)
    }

    /// 枚举候选：按优先级各记一个
    struct Candidates {
        pid: u32,
        by_class: HWND,
        by_title: HWND,
        fallback: HWND,
    }

    unsafe extern "system" fn enum_proc(hwnd: HWND, lparam: LPARAM) -> windows_sys::Win32::Foundation::BOOL {
        let st = &mut *(lparam as *mut Candidates);
        let mut owner = 0u32;
        GetWindowThreadProcessId(hwnd, &mut owner);
        if owner != st.pid {
            return TRUE;
        }
        let class = class_of(hwnd);
        if is_helper(&class) {
            return TRUE;
        }
        if class == WINIT_CLASS {
            if st.by_class.is_null() {
                st.by_class = hwnd;
            }
            return TRUE;
        }
        if title_of(hwnd).starts_with(TITLE_PREFIX) {
            if st.by_title.is_null() {
                st.by_title = hwnd;
            }
            return TRUE;
        }
        if st.fallback.is_null() {
            st.fallback = hwnd;
        }
        TRUE // 进程里窗口很少，全部枚举完再按优先级取
    }

    fn find_main_window() -> HWND {
        let mut st = Candidates {
            pid: std::process::id(),
            by_class: std::ptr::null_mut(),
            by_title: std::ptr::null_mut(),
            fallback: std::ptr::null_mut(),
        };
        unsafe { EnumWindows(Some(enum_proc), &mut st as *mut Candidates as LPARAM) };
        if !st.by_class.is_null() {
            return st.by_class;
        }
        if !st.by_title.is_null() {
            return st.by_title;
        }
        if !st.fallback.is_null() {
            log::warn!("Win32：没找到 winit 主窗口（类名 {} 或标题 {}…），暂以 0x{:X} 代用", WINIT_CLASS, TITLE_PREFIX, st.fallback as usize);
        }
        st.fallback
    }

    fn main_hwnd() -> HWND {
        let cached = MAIN_HWND.load(Ordering::Relaxed) as HWND;
        // 缓存也要复核：只有“看着就是主窗口”的缓存才敢用（选错一次就会永久失效）
        if !cached.is_null() && unsafe { IsWindow(cached) } != 0 && is_main_window(cached) {
            return cached;
        }
        MAIN_HWND.store(0, Ordering::Relaxed);
        let hwnd = find_main_window();
        if hwnd.is_null() {
            log::warn!("Win32：没找到主窗口句柄，显示/隐藏退回 egui 命令");
            return hwnd;
        }
        // 按特征认出来的才缓存；退化猜测不缓存，下次重新找
        if is_main_window(hwnd) {
            MAIN_HWND.store(hwnd as usize, Ordering::Relaxed);
        }
        hwnd
    }

    /// 显示并激活主窗口
    pub fn show(keep_on_top: bool) {
        let hwnd = main_hwnd();
        if hwnd.is_null() {
            log::warn!("没找到主窗口句柄，显示窗口退回 egui 命令");
            return;
        }
        unsafe {
            ShowWindow(hwnd, SW_SHOW);
            ShowWindow(hwnd, SW_RESTORE);
            BringWindowToTop(hwnd);
            SetForegroundWindow(hwnd);
            if keep_on_top {
                SetWindowPos(hwnd, HWND_TOPMOST, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE);
                SetWindowPos(hwnd, HWND_NOTOPMOST, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE);
            }
        }
        if unsafe { IsWindowVisible(hwnd) } == 0 {
            // 找错了窗口：丢掉缓存，下次重新定位（否则会一直“有日志、没反应”）
            log::warn!("Win32：显示后窗口 0x{:X} 仍不可见，丢弃该句柄，下次重新定位主窗口", hwnd as usize);
            MAIN_HWND.store(0, Ordering::Relaxed);
        } else {
            log::info!("已通过 Win32 直接显示窗口（0x{:X}）", hwnd as usize);
        }
    }

    /// 隐藏主窗口（收起托盘）
    pub fn hide() {
        let hwnd = main_hwnd();
        if hwnd.is_null() {
            return;
        }
        unsafe {
            ShowWindow(hwnd, SW_HIDE);
        }
        if unsafe { IsWindowVisible(hwnd) } != 0 {
            log::warn!("Win32：隐藏后窗口 0x{:X} 仍可见，丢弃该句柄，下次重新定位主窗口", hwnd as usize);
            MAIN_HWND.store(0, Ordering::Relaxed);
        } else {
            log::info!("已通过 Win32 直接隐藏窗口（0x{:X}）", hwnd as usize);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 停放的判定必须跟着 `Shared::ui_hidden`（跨线程真源），不能在模块里另存一份状态
    #[test]
    fn parked_follows_shared_flag() {
        let shared = Shared::new(crate::stats::Store::default(), 0, 0);
        assert!(!is_parked(&shared));
        shared.ui_hidden.store(true, Ordering::Relaxed);
        assert!(is_parked(&shared));
        shared.ui_hidden.store(false, Ordering::Relaxed);
        assert!(!is_parked(&shared));
    }
}
