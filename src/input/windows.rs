//! Windows 输入捕获：WH_KEYBOARD_LL + WH_MOUSE_LL 低级钩子，与原版逻辑一致：
//! - 键盘只在 KEYUP / SYSKEYUP 计次，忽略注入输入（LLKHF_INJECTED）
//! - 鼠标只在按钮抬起计次；滚轮每条消息计 1；移动按像素累计距离
//! - 钩子线程需要消息泵

#![cfg(windows)]

use super::{InEvent};
use crate::keys::idx_by_win_sc;
use crate::stats::{other_key_id_win, MouseEv, Shared};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex, OnceLock};

use windows_sys::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, DispatchMessageW, GetMessageW, GetSystemMetrics, KBDLLHOOKSTRUCT, LLMHF_INJECTED,
    LLKHF_EXTENDED, LLKHF_INJECTED, MSG, MSLLHOOKSTRUCT, SetWindowsHookExW, SM_CXSCREEN,
    UnhookWindowsHookEx, WH_KEYBOARD_LL, WH_MOUSE_LL, HHOOK,
};

static SINK: OnceLock<Sender<InEvent>> = OnceLock::new();

// 距离计算需要上一次位置
static LAST_POS: Mutex<Option<(i32, i32)>> = Mutex::new(None);

unsafe extern "system" fn keyboard_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code >= 0 {
        // WM_KEYUP = 0x0101, WM_SYSKEYUP = 0x0105
        let is_up = wparam == 0x0101 || wparam == 0x0105;
        if is_up {
            let info = &*(lparam as *const KBDLLHOOKSTRUCT);
            if (info.flags & LLKHF_INJECTED) == 0 {
                let extended = (info.flags & LLKHF_EXTENDED) as u16;
                let sc = (extended << 8) | (info.scanCode as u16 & 0xff);
                if let Some(sink) = SINK.get() {
                    match idx_by_win_sc(sc) {
                        Some(idx) => {
                            let _ = sink.send(InEvent::Key(idx));
                        }
                        None => {
                            let _ = sink.send(InEvent::KeyOther(other_key_id_win(sc)));
                        }
                    }
                }
            }
        }
    }
    CallNextHookEx(std::ptr::null_mut(), code, wparam, lparam)
}

unsafe extern "system" fn mouse_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code >= 0 {
        let info = &*(lparam as *const MSLLHOOKSTRUCT);
        if (info.flags & LLMHF_INJECTED) == 0 {
            if let Some(sink) = SINK.get() {
                match wparam {
                    0x0200 => {
                        // WM_MOUSEMOVE：勾股定理累计移动距离
                        let (x, y) = (info.pt.x, info.pt.y);
                        let mut last = LAST_POS.lock().unwrap();
                        let d = match *last {
                            Some((ox, oy)) => (((x - ox) as f64).powi(2) + ((y - oy) as f64).powi(2)).sqrt(),
                            None => 0.0,
                        };
                        *last = Some((x, y));
                        drop(last);
                        if d > 0.0 {
                            let _ = sink.send(InEvent::Mouse(MouseEv::MovePx(d)));
                        }
                    }
                    0x0202 => { let _ = sink.send(InEvent::Mouse(MouseEv::LeftUp)); }   // WM_LBUTTONUP
                    0x0205 => { let _ = sink.send(InEvent::Mouse(MouseEv::RightUp)); }  // WM_RBUTTONUP
                    0x0208 => { let _ = sink.send(InEvent::Mouse(MouseEv::MiddleUp)); } // WM_MBUTTONUP
                    0x020C => { let _ = sink.send(InEvent::Mouse(MouseEv::XUp)); }      // WM_XBUTTONUP
                    0x020A => { let _ = sink.send(InEvent::Mouse(MouseEv::Wheel)); }    // WM_MOUSEWHEEL
                    0x020E => { let _ = sink.send(InEvent::Mouse(MouseEv::HWheel)); }   // WM_MOUSEHWHEEL
                    _ => {}
                }
            }
        }
    }
    CallNextHookEx(std::ptr::null_mut(), code, wparam, lparam)
}

/// 启动钩子线程（非阻塞）。失败时把警告写入 shared。
pub fn start(shared: Arc<Shared>) {
    std::thread::Builder::new()
        .name("kmcounter-hooks".into())
        .spawn(move || unsafe {
            let sink = super::spawn_applier(shared.clone());
            let _ = SINK.set(sink);

            let hmod = GetModuleHandleW(std::ptr::null());
            let hkb: HHOOK = SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_proc), hmod, 0);
            let hms: HHOOK = SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_proc), hmod, 0);

            if hkb.is_null() || hms.is_null() {
                shared.set_input_status(
                    false,
                    format!("SetWindowsHookEx 失败 (keyboard={}, mouse={})", hkb.is_null(), hms.is_null()),
                );
            } else {
                // 主显示器像素宽度（用于移动距离换算）
                let px = GetSystemMetrics(SM_CXSCREEN);
                if px > 0 && shared.screen_px_w.load(std::sync::atomic::Ordering::Relaxed) == 0 {
                    shared.screen_px_w.store(px as u32, std::sync::atomic::Ordering::Relaxed);
                }
                shared.set_input_status(true, String::new());
            }

            // 消息泵：低级钩子依赖它
            let mut msg: MSG = std::mem::zeroed();
            while GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {
                DispatchMessageW(&msg);
            }

            if !hkb.is_null() {
                UnhookWindowsHookEx(hkb);
            }
            if !hms.is_null() {
                UnhookWindowsHookEx(hms);
            }
        })
        .expect("无法启动钩子线程");
}
