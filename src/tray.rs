//! 托盘图标与菜单。
//! - Windows：Shell_NotifyIconW + 弹出菜单（自绘，不引入额外依赖）
//! - Linux：ksni（StatusNotifierItem / DBus，纯 Rust，无 GTK 依赖）

use crate::lang::Lang;

#[derive(Debug, Clone)]
pub enum TrayCmd {
    Show,
    Settings,
    ToggleAutostart,
    Exit,
    /// GUI → 托盘：开机启动勾选状态变化
    AutostartState(bool),
    /// GUI → 托盘：界面语言变化（托盘菜单跟着切）
    Language(Lang),
}

pub struct Tray {
    /// GUI 消费托盘命令
    pub rx: std::sync::mpsc::Receiver<TrayCmd>,
    /// GUI → 托盘控制通道
    pub tx: std::sync::mpsc::Sender<TrayCmd>,
    /// 命令注入端（与托盘菜单发出的命令同队列，便于测试/程序化触发）
    pub cmd_tx: std::sync::mpsc::Sender<TrayCmd>,
}

/// 生成托盘图标（32×32 RGBA）：莫兰迪红底 + 白色按键点阵
pub fn icon_rgba() -> (Vec<u8>, u32) {
    const S: u32 = 32;
    let mut px = vec![0u8; (S * S * 4) as usize];
    let mut set = |x: u32, y: u32, c: [u8; 4]| {
        if x < S && y < S {
            let i = ((y * S + x) * 4) as usize;
            px[i..i + 4].copy_from_slice(&c);
        }
    };
    let bg = [0xB2u8, 0x6C, 0x65, 0xFF];
    let white = [0xEEu8, 0xEE, 0xEE, 0xFF];
    // 圆角矩形底
    let r: i32 = 6;
    for y in 0..S {
        for x in 0..S {
            let dx = (x as i32 - (S as i32 - 1)).abs().min(x as i32);
            let dy = (y as i32 - (S as i32 - 1)).abs().min(y as i32);
            let corner = (x < r as u32 && y < r as u32)
                || (x >= S - r as u32 && y < r as u32)
                || (x < r as u32 && y >= S - r as u32)
                || (x >= S - r as u32 && y >= S - r as u32);
            if !corner || (dx <= r && dy <= r) {
                set(x, y, bg);
            }
        }
    }
    // 按键点阵：两行 3×3 + 底部一条空格键
    let mut key = |gx: u32, gy: u32| {
        for dy in 0..4u32 {
            for dx in 0..4u32 {
                set(5 + gx * 7 + dx, 6 + gy * 7 + dy, white);
            }
        }
    };
    for gy in 0..2u32 {
        for gx in 0..3u32 {
            key(gx, gy);
        }
    }
    for dx in 0..10u32 {
        for dy in 0..4u32 {
            set(12 + dx, 20 + dy, white);
        }
    }
    (px, S)
}

#[cfg(target_os = "linux")]
mod imp {
    use super::{Tray, TrayCmd};
    use crate::lang::{strings, Lang};
    use ksni::menu::MenuItem;

    struct KTray {
        tx: std::sync::mpsc::Sender<TrayCmd>,
        autostart: bool,
        tip: String,
        lang: Lang,
    }

    impl ksni::Tray for KTray {
        fn id(&self) -> String {
            "kmcounter".into()
        }

        fn icon_pixmap(&self) -> Vec<ksni::Icon> {
            let (rgba, w) = super::icon_rgba();
            // ksni 要求 ARGB32 大端序：每像素 [A, R, G, B]
            let mut argb = Vec::with_capacity(rgba.len());
            for px in rgba.chunks_exact(4) {
                argb.push(px[3]);
                argb.push(px[0]);
                argb.push(px[1]);
                argb.push(px[2]);
            }
            vec![ksni::Icon {
                width: w as i32,
                height: w as i32,
                data: argb,
            }]
        }

        fn title(&self) -> String {
            "KMCounter-rs".into()
        }

        fn tool_tip(&self) -> ksni::ToolTip {
            ksni::ToolTip {
                icon_name: String::new(),
                icon_pixmap: Vec::new(),
                title: "KMCounter-rs".into(),
                description: self.tip.clone(),
            }
        }

        fn menu(&self) -> Vec<MenuItem<Self>> {
            let s = strings(self.lang);
            use ksni::menu::*;
            vec![
                StandardItem {
                    label: s.menu_stats.into(),
                    activate: Box::new(|t: &mut Self| {
                        let _ = t.tx.send(TrayCmd::Show);
                    }),
                    ..Default::default()
                }
                .into(),
                StandardItem {
                    label: s.menu_settings.into(),
                    activate: Box::new(|t: &mut Self| {
                        let _ = t.tx.send(TrayCmd::Settings);
                    }),
                    ..Default::default()
                }
                .into(),
                MenuItem::Separator,
                CheckmarkItem {
                    label: s.menu_autostart.into(),
                    checked: self.autostart,
                    activate: Box::new(|t: &mut Self| {
                        let _ = t.tx.send(TrayCmd::ToggleAutostart);
                    }),
                    ..Default::default()
                }
                .into(),
                MenuItem::Separator,
                StandardItem {
                    label: s.menu_exit.into(),
                    activate: Box::new(|t: &mut Self| {
                        let _ = t.tx.send(TrayCmd::Exit);
                    }),
                    ..Default::default()
                }
                .into(),
            ]
        }

        fn activate(&mut self, _x: i32, _y: i32) {
            let _ = self.tx.send(TrayCmd::Show);
        }
    }

    pub fn start(lang: Lang) -> Option<Tray> {
        let (cmd_tx, cmd_rx) = std::sync::mpsc::channel::<TrayCmd>();
        let (ctl_tx, ctl_rx) = std::sync::mpsc::channel::<TrayCmd>();

        // ksni::blocking 的 spawn 自带线程与事件循环
        let autostart = crate::autostart::is_enabled();
        let tip = strings(lang).welcome_main.to_string();
        let tray = KTray { tx: cmd_tx.clone(), autostart, tip, lang };
        let handle = match ksni::blocking::TrayMethods::spawn(tray) {
            Ok(h) => h,
            Err(e) => {
                log::warn!("托盘初始化失败（可能没有运行 StatusNotifier 支持）：{e}");
                return None;
            }
        };

        // 控制通道处理：更新勾选状态
        std::thread::Builder::new()
            .name("kmcounter-tray-ctl".into())
            .spawn(move || {
                while let Ok(cmd) = ctl_rx.recv() {
                    match cmd {
                        TrayCmd::AutostartState(b) => {
                            handle.update(|t: &mut KTray| {
                                t.autostart = b;
                            });
                        }
                        TrayCmd::Language(l) => {
                            // 菜单是懒构建的，改完语言下次弹菜单即为新语言
                            handle.update(|t: &mut KTray| {
                                t.lang = l;
                                t.tip = strings(l).welcome_main.to_string();
                            });
                        }
                        _ => {}
                    }
                }
            })
            .ok();

        Some(Tray { rx: cmd_rx, tx: ctl_tx, cmd_tx })
    }
}

#[cfg(windows)]
mod imp {
    use super::{Tray, TrayCmd};
    use crate::lang::{strings, Lang};
    use std::sync::mpsc::{Receiver, Sender};
    use std::sync::{Mutex, OnceLock};

    static TX_GUI: OnceLock<Sender<TrayCmd>> = OnceLock::new();
    static CTL_RX: OnceLock<Mutex<Receiver<TrayCmd>>> = OnceLock::new();
    /// 界面语言（0=中文 1=英文）：随设置变化，菜单构建时读取
    static LANG_CODE: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(1);

    fn lang_now() -> Lang {
        if LANG_CODE.load(std::sync::atomic::Ordering::Relaxed) == 0 {
            Lang::Zh
        } else {
            Lang::En
        }
    }
    static HMENU: Mutex<usize> = Mutex::new(0);
    /// 托盘窗口句柄与图标：Explorer 重启后要重新添加图标
    static TRAY_HWND: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    static TRAY_ICON: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    /// Explorer 重启时广播的消息（GetMessage 之前用 RegisterWindowMessageW 取得）
    static WM_TASKBARCREATED: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

    /// 添加托盘图标（启动时与 Explorer 重启后各调一次）
    unsafe fn add_icon(hwnd: windows_sys::Win32::Foundation::HWND) -> bool {
        use windows_sys::Win32::UI::Shell::*;
        let icon = TRAY_ICON.load(std::sync::atomic::Ordering::Relaxed) as *mut core::ffi::c_void;
        let mut nid: NOTIFYICONDATAW = std::mem::zeroed();
        nid.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
        nid.hWnd = hwnd;
        nid.uID = 1;
        nid.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP;
        nid.uCallbackMessage = WM_APP_TRAY;
        nid.hIcon = icon;
        let tip = wide("KMCounter-rs");
        let n = tip.len().min(128);
        nid.szTip[..n].copy_from_slice(&tip[..n]);
        if Shell_NotifyIconW(NIM_ADD, &nid) == 0 {
            log::warn!("托盘图标添加失败");
            return false;
        }
        // NOTIFYICON_VERSION_3：v3 起右键回调是 WM_CONTEXTMENU，左键仍是 WM_LBUTTONUP
        nid.Anonymous.uVersion = 3;
        Shell_NotifyIconW(NIM_SETVERSION, &nid);
        log::info!("托盘图标已就绪（NOTIFYICON_VERSION_3）");
        true
    }

    // 菜单 ID
    const ID_STATS: u32 = 1;
    const ID_SETTINGS: u32 = 2;
    const ID_AUTOSTART: u32 = 3;
    const ID_EXIT: u32 = 4;
    const WM_APP_TRAY: u32 = 0x8000; // WM_APP

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    unsafe fn make_icon() -> windows_sys::Win32::UI::WindowsAndMessaging::HICON {
        use windows_sys::Win32::Graphics::Gdi::*;
        use windows_sys::Win32::UI::WindowsAndMessaging::*;
        let (rgba, size) = super::icon_rgba();
        let w = size as i32;
        let hdc = GetDC(std::ptr::null_mut());
        let mut bmi: BITMAPINFO = std::mem::zeroed();
        bmi.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
        bmi.bmiHeader.biWidth = w;
        bmi.bmiHeader.biHeight = -w; // top-down
        bmi.bmiHeader.biPlanes = 1;
        bmi.bmiHeader.biBitCount = 32;
        bmi.bmiHeader.biCompression = BI_RGB;
        let mut bits: *mut std::ffi::c_void = std::ptr::null_mut();
        let hbm_color = CreateDIBSection(hdc, &bmi, DIB_RGB_COLORS, &mut bits, std::ptr::null_mut(), 0);
        ReleaseDC(std::ptr::null_mut(), hdc);
        if !hbm_color.is_null() && !bits.is_null() {
            let dst = std::slice::from_raw_parts_mut(bits as *mut u8, (size * size * 4) as usize);
            // RGBA → BGRA（图标颜色位图使用非预乘 BGRA）
            for (i, px) in rgba.chunks_exact(4).enumerate() {
                dst[i * 4] = px[2];
                dst[i * 4 + 1] = px[1];
                dst[i * 4 + 2] = px[0];
                dst[i * 4 + 3] = px[3];
            }
        }
        let hbm_mask = CreateBitmap(w, w, 1, 1, std::ptr::null());
        let ii = ICONINFO {
            fIcon: 1,
            xHotspot: 0,
            yHotspot: 0,
            hbmMask: hbm_mask,
            hbmColor: hbm_color,
        };
        let icon = CreateIconIndirect(&ii);
        if !hbm_color.is_null() {
            DeleteObject(hbm_color);
        }
        if !hbm_mask.is_null() {
            DeleteObject(hbm_mask);
        }
        icon
    }

    unsafe fn build_menu() -> windows_sys::Win32::UI::WindowsAndMessaging::HMENU {
        use windows_sys::Win32::UI::WindowsAndMessaging::*;
        let s = strings(lang_now());
        let menu = CreatePopupMenu();
        AppendMenuW(menu, 0, ID_STATS as usize, wide(s.menu_stats).as_ptr());
        AppendMenuW(menu, 0, ID_SETTINGS as usize, wide(s.menu_settings).as_ptr());
        AppendMenuW(menu, MF_SEPARATOR, 0, std::ptr::null());
        let checked = if crate::autostart::is_enabled() { MF_CHECKED } else { MF_UNCHECKED };
        AppendMenuW(menu, checked, ID_AUTOSTART as usize, wide(s.menu_autostart).as_ptr());
        AppendMenuW(menu, MF_SEPARATOR, 0, std::ptr::null());
        AppendMenuW(menu, 0, ID_EXIT as usize, wide(s.menu_exit).as_ptr());
        *HMENU.lock().unwrap() = menu as usize;
        menu
    }

    unsafe extern "system" fn tray_wndproc(hwnd: windows_sys::Win32::Foundation::HWND, msg: u32, wparam: windows_sys::Win32::Foundation::WPARAM, lparam: windows_sys::Win32::Foundation::LPARAM) -> windows_sys::Win32::Foundation::LRESULT {
        use windows_sys::Win32::UI::WindowsAndMessaging::*;
        match msg {
            WM_APP_TRAY => {
                let ev = lparam as u32;
                log::debug!("托盘回调 lparam={ev:#06x} wparam={wparam}");
                // NOTIFYICON_VERSION_3 下右键回调是 WM_CONTEXTMENU(0x007B)；0x0205 是旧版本的 WM_RBUTTONUP
                match ev {
                    0x0202 | 0x0203 | 0x0400 | 0x0401 => {
                        // 左键单击/双击（WM_LBUTTONUP/DBLCLK）→ 显示统计。
                        // 0x0400/0x0401 是 NIN_SELECT/NIN_KEYSELECT：v3 起用它们表示“选中图标”，
                        // 实测（Win11 25600）鼠标点击会先发 0x0201/0x0202 再补一个 0x0400；
                        // 键盘选中或某些系统/位置只发 NIN_*，所以两种都认
                        log::debug!("托盘：左键点击 → 请求显示窗口");
                        if let Some(tx) = TX_GUI.get() {
                            let _ = tx.send(TrayCmd::Show);
                        }
                    }
                    0x0205 | 0x007B => {
                        // 右键 → 菜单
                        use windows_sys::Win32::Foundation::POINT;
                        let mut pt = POINT { x: 0, y: 0 };
                        GetCursorPos(&mut pt);
                        SetForegroundWindow(hwnd);
                        let menu = *HMENU.lock().unwrap() as HMENU;
                        let id = TrackPopupMenu(
                            menu,
                            TPM_RETURNCMD | TPM_RIGHTBUTTON | TPM_NONOTIFY,
                            pt.x,
                            pt.y,
                            0,
                            hwnd,
                            std::ptr::null(),
                        );
                        PostMessageW(hwnd, 0 /*WM_NULL*/, 0, 0);
                        if let Some(tx) = TX_GUI.get() {
                            let cmd = match id as u32 {
                                ID_STATS => Some(TrayCmd::Show),
                                ID_SETTINGS => Some(TrayCmd::Settings),
                                ID_AUTOSTART => Some(TrayCmd::ToggleAutostart),
                                ID_EXIT => Some(TrayCmd::Exit),
                                _ => None,
                            };
                            if let Some(c) = cmd {
                                log::debug!("托盘菜单选择: {c:?}");
                                let _ = tx.send(c);
                            }
                        }
                    }
                    _ => {}
                }
                0
            }
            _ if msg == WM_TASKBARCREATED.load(std::sync::atomic::Ordering::Relaxed) && msg != 0 => {
                // Explorer 重启：任务栏被重建，必须重新添加图标，否则图标残留但点了没反应
                log::info!("检测到 Explorer 重启，重新添加托盘图标");
                let hwnd = TRAY_HWND.load(std::sync::atomic::Ordering::Relaxed) as windows_sys::Win32::Foundation::HWND;
                if !hwnd.is_null() {
                    add_icon(hwnd);
                }
                0
            }
            0x0113 => {
                // WM_TIMER：处理 GUI → 托盘控制命令（勾选状态）
                if let Some(rx) = CTL_RX.get() {
                    while let Ok(cmd) = rx.lock().unwrap().try_recv() {
                        match cmd {
                            TrayCmd::AutostartState(on) => {
                                CheckMenuItem(
                                    *HMENU.lock().unwrap() as HMENU,
                                    ID_AUTOSTART,
                                    MF_BYCOMMAND | if on { MF_CHECKED } else { MF_UNCHECKED },
                                );
                            }
                            TrayCmd::Language(l) => {
                                LANG_CODE.store(if l == Lang::Zh { 0 } else { 1 }, std::sync::atomic::Ordering::Relaxed);
                                // 菜单文字在构建时确定，改语言后重建一次
                                let old = *HMENU.lock().unwrap() as HMENU;
                                if !old.is_null() {
                                    DestroyMenu(old);
                                }
                                build_menu();
                            }
                            _ => {}
                        }
                    }
                }
                0
            }
            0x0010 => {
                // WM_DESTROY
                use windows_sys::Win32::UI::Shell::{Shell_NotifyIconW, NIM_DELETE, NOTIFYICONDATAW};
                let mut nid: NOTIFYICONDATAW = std::mem::zeroed();
                nid.hWnd = hwnd;
                nid.uID = 1;
                Shell_NotifyIconW(NIM_DELETE, &nid);
                PostQuitMessage(0);
                0
            }
            _ => DefWindowProcW(hwnd, msg, wparam, lparam),
        }
    }

    pub fn start(lang: Lang) -> Option<Tray> {
        let (cmd_tx, cmd_rx) = std::sync::mpsc::channel::<TrayCmd>();
        let (ctl_tx, ctl_rx) = std::sync::mpsc::channel::<TrayCmd>();
        let _ = TX_GUI.set(cmd_tx.clone());
        LANG_CODE.store(if lang == Lang::Zh { 0 } else { 1 }, std::sync::atomic::Ordering::Relaxed);
        let _ = CTL_RX.set(Mutex::new(ctl_rx));

        std::thread::Builder::new()
            .name("kmcounter-tray".into())
            .spawn(move || unsafe {
                use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
                use windows_sys::Win32::UI::WindowsAndMessaging::*;

                let hmod = GetModuleHandleW(std::ptr::null());
                let class_name = wide("KMCounterTrayWnd");
                let wc = WNDCLASSW {
                    style: 0,
                    lpfnWndProc: Some(tray_wndproc),
                    cbClsExtra: 0,
                    cbWndExtra: 0,
                    hInstance: hmod,
                    hIcon: std::ptr::null_mut(),
                    hCursor: std::ptr::null_mut(),
                    hbrBackground: std::ptr::null_mut(),
                    lpszMenuName: std::ptr::null(),
                    lpszClassName: class_name.as_ptr(),
                };
                if RegisterClassW(&wc) == 0 {
                    log::warn!("托盘窗口类注册失败");
                    return;
                }
                let hwnd = CreateWindowExW(
                    0,
                    class_name.as_ptr(),
                    wide("KMCounter").as_ptr(),
                    0x00000000, // WS_OVERLAPPED，不显示
                    0,
                    0,
                    0,
                    0,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    hmod,
                    std::ptr::null(),
                );
                if hwnd.is_null() {
                    log::warn!("托盘窗口创建失败");
                    return;
                }

                // Explorer 重启广播：注册后才能在 wndproc 里识别
                let taskbar_created = RegisterWindowMessageW(wide("TaskbarCreated").as_ptr());
                WM_TASKBARCREATED.store(taskbar_created, std::sync::atomic::Ordering::Relaxed);

                let icon = make_icon();
                TRAY_ICON.store(icon as usize, std::sync::atomic::Ordering::Relaxed);
                TRAY_HWND.store(hwnd as usize, std::sync::atomic::Ordering::Relaxed);
                if !add_icon(hwnd) {
                    return;
                }

                // 200ms 定时器轮询控制通道
                SetTimer(hwnd, 1, 200, None);

                build_menu();

                let mut msg: MSG = std::mem::zeroed();
                while GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {
                    TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
            })
            .ok();

        Some(Tray { rx: cmd_rx, tx: ctl_tx, cmd_tx })
    }
}

#[cfg(not(any(windows, target_os = "linux")))]
mod imp {
    use super::Tray;
    pub fn start(_lang: Lang) -> Option<Tray> {
        None
    }
}

/// 启动托盘。失败返回 None（GUI 会用“关闭即退出”模式兜底）。
pub fn start(lang: Lang) -> Option<Tray> {
    imp::start(lang)
}
