//! Linux 输入捕获：直接读取 /dev/input/event*（evdev 层），
//! 在 X11 与 Wayland 下都可用。需要用户在 input 组中。
//! - 虚拟设备（uinput 注入，BUS_VIRTUAL + 名字像 uinput/ydotool）被跳过，实现“区分真实模拟”
//!   （不能只看总线类型：虚拟机里的 virtio 键鼠也是 BUS_VIRTUAL）
//! - inotify 监视 /dev/input 实现热插拔
//! - 键盘计“按下”，鼠标按键计“按下”，滚轮按刻度计 1，移动按像素累计距离

#![cfg(target_os = "linux")]

use super::InEvent;
use crate::keys::idx_by_lin_code;
use crate::stats::{other_key_id_lin, MouseEv, Shared};
use std::io::ErrorKind;
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::Arc;
use std::time::Duration;

use evdev::{BusType, Device, RelativeAxisCode};

const EV_SYN: u16 = 0x00;
const SYN_REPORT: u16 = 0x00;
const EV_KEY: u16 = 0x01;
const EV_REL: u16 = 0x02;

const REL_X: u16 = 0x00;
const REL_Y: u16 = 0x01;
const REL_WHEEL: u16 = 0x08;
const REL_HWHEEL: u16 = 0x06;
const REL_WHEEL_HI_RES: u16 = 0x0b;
const REL_HWHEEL_HI_RES: u16 = 0x0c;

const BTN_LEFT: u16 = 0x110;
const BTN_RIGHT: u16 = 0x111;
const BTN_MIDDLE: u16 = 0x112;
const BTN_SIDE: u16 = 0x113;
const BTN_EXTRA: u16 = 0x114;

struct Dev {
    device: Device,
    is_keyboard: bool,
    wheel_via_hires: bool,
    wheel_frac: f64,
    hwheel_frac: f64,
    /// 当前帧（上一个 SYN_REPORT 之后）累计的相对位移
    batch_dx: f64,
    batch_dy: f64,
}

impl Dev {
    fn open(path: &Path) -> std::io::Result<Dev> {
        let device = Device::open(path)?;
        // 注入设备（uinput，如 ydotool/dotool）不计入。
        // 但**不能只按 BUS_VIRTUAL 过滤**：虚拟机里的 virtio 键鼠总线类型也是 BUS_VIRTUAL，
        // 那样会让 VM 里显示“未找到任何输入设备”、统计恒为 0，所以结合设备名判断。
        if device.input_id().bus_type() == BusType::BUS_VIRTUAL {
            let name = device.name().unwrap_or_default().to_lowercase();
            if ["uinput", "ydotool", "dotool", "wtype"].iter().any(|s| name.contains(s)) {
                return Err(ErrorKind::Unsupported.into());
            }
            log::debug!("虚拟总线设备（{name}）不像注入设备，仍计入统计");
        }
        let is_keyboard = device
            .supported_keys()
            .map(|k| k.iter().count() >= 20)
            .unwrap_or(false);
        let axes = device.supported_relative_axes();
        let has_wheel = axes.map(|a| a.contains(RelativeAxisCode::REL_WHEEL)).unwrap_or(false);
        let has_hires = axes
            .map(|a| a.contains(RelativeAxisCode::REL_WHEEL_HI_RES))
            .unwrap_or(false);
        device.set_nonblocking(true)?;
        Ok(Dev {
            device,
            is_keyboard,
            wheel_via_hires: !has_wheel && has_hires,
            wheel_frac: 0.0,
            hwheel_frac: 0.0,
            batch_dx: 0.0,
            batch_dy: 0.0,
        })
    }

    fn flush_motion(&mut self, sink: &Sender<InEvent>) {
        let (dx, dy) = (self.batch_dx, self.batch_dy);
        self.batch_dx = 0.0;
        self.batch_dy = 0.0;
        if dx != 0.0 || dy != 0.0 {
            let d = (dx * dx + dy * dy).sqrt();
            let _ = sink.send(InEvent::Mouse(MouseEv::MovePx(d)));
        }
    }
}

/// 枚举并打开 /dev/input/event*。返回 (打开的设备, 被权限拒绝的数量)
fn scan_devices() -> (Vec<(PathBuf, Dev)>, usize) {
    let mut out = Vec::new();
    let mut denied = 0;
    if let Ok(entries) = std::fs::read_dir("/dev/input") {
        for e in entries.flatten() {
            let name = e.file_name();
            let name = name.to_string_lossy();
            if name.starts_with("event") {
                match Dev::open(&e.path()) {
                    Ok(d) => out.push((e.path(), d)),
                    Err(err) if err.kind() == ErrorKind::PermissionDenied => denied += 1,
                    Err(err) if err.kind() == ErrorKind::Unsupported => {}
                    Err(err) => log::debug!("跳过 {}: {err}", e.path().display()),
                }
            }
        }
    }
    (out, denied)
}

/// 热插拔监视线程：发现新设备节点就报告
fn start_hotplug_watcher(tx: Sender<PathBuf>) {
    std::thread::Builder::new()
        .name("kmcounter-hotplug".into())
        .spawn(move || {
            use inotify::{Inotify, WatchMask};
            loop {
                let mut ino = match Inotify::init() {
                    Ok(i) => i,
                    Err(e) => {
                        log::warn!("inotify 初始化失败：{e}（热插拔不可用）");
                        return;
                    }
                };
                if ino.watches().add("/dev/input", WatchMask::CREATE | WatchMask::MOVED_TO).is_err() {
                    log::warn!("无法监视 /dev/input（可能缺少权限），热插拔不可用");
                    return;
                }
                let mut buf = [0u8; 4096];
                loop {
                    match ino.read_events_blocking(&mut buf) {
                        Ok(events) => {
                            for ev in events {
                                if let Some(name) = ev.name {
                                    let name = name.to_string_lossy();
                                    if name.starts_with("event") {
                                        let p = PathBuf::from("/dev/input").join(name.to_string());
                                        if tx.send(p).is_err() {
                                            return;
                                        }
                                    }
                                }
                            }
                        }
                        Err(e) => {
                            log::warn!("inotify 读取失败：{e}，1 秒后重建监视");
                            break;
                        }
                    }
                }
                std::thread::sleep(Duration::from_secs(1));
            }
        })
        .ok();
}

pub fn start(shared: Arc<Shared>) {
    std::thread::Builder::new()
        .name("kmcounter-evdev".into())
        .spawn(move || {
            let sink = super::spawn_applier(shared.clone());
            let (hotplug_tx, hotplug_rx): (Sender<PathBuf>, Receiver<PathBuf>) = std::sync::mpsc::channel();
            start_hotplug_watcher(hotplug_tx);

            let (mut devs, mut denied) = scan_devices();

            if devs.is_empty() {
                let msg = if denied > 0 {
                    "无法读取 /dev/input（权限不足）。".to_string()
                } else {
                    "未找到任何输入设备。".to_string()
                };
                shared.set_input_status(false, msg);
            } else {
                shared.set_input_status(true, String::new());
            }

            // Linux 下无法可靠自动获取显示器像素宽，默认 1920，可在配置中覆盖
            if shared.screen_px_w.load(std::sync::atomic::Ordering::Relaxed) == 0 {
                shared.screen_px_w.store(1920, std::sync::atomic::Ordering::Relaxed);
            }

            loop {
                // 处理热插拔
                while let Ok(path) = hotplug_rx.try_recv() {
                    if devs.iter().any(|(p, _)| p == &path) {
                        continue;
                    }
                    match Dev::open(&path) {
                        Ok(d) => {
                            log::info!("已接入输入设备: {}", path.display());
                            devs.push((path, d));
                            if !shared.input_ok.load(std::sync::atomic::Ordering::Relaxed) {
                                shared.set_input_status(true, String::new());
                            }
                        }
                        Err(_) => {}
                    }
                }

                // 全部设备丢失时轮询重试（等待权限修复或设备接入）
                if devs.is_empty() {
                    std::thread::sleep(Duration::from_millis(2000));
                    let (nd, ndenied) = scan_devices();
                    if devs.is_empty() && (!nd.is_empty() || (ndenied == 0 && denied > 0)) {
                        shared.set_input_status(!nd.is_empty(), String::new());
                    }
                    denied = ndenied;
                    devs = nd;
                    continue;
                }

                // fds 与 devs 一一对应
                let mut fds: Vec<nix::poll::PollFd> = Vec::with_capacity(devs.len());
                for (_, d) in devs.iter() {
                    let fd = d.device.as_raw_fd();
                    fds.push(nix::poll::PollFd::new(
                        // SAFETY: fd 由打开的设备持有，poll 期间有效
                        unsafe { std::os::fd::BorrowedFd::borrow_raw(fd) },
                        nix::poll::PollFlags::POLLIN,
                    ));
                }

                match nix::poll::poll(&mut fds, 250u16) {
                    Ok(0) => continue,
                    Ok(_) => {}
                    Err(e) => {
                        log::warn!("poll 失败：{e}");
                        std::thread::sleep(Duration::from_millis(500));
                        continue;
                    }
                }

                // 先收集结果，再可变处理（借用规则）
                use nix::poll::PollFlags;
                let mut dead: Vec<usize> = Vec::new();
                let mut ready: Vec<usize> = Vec::new();
                for (i, p) in fds.iter().enumerate() {
                    match p.revents() {
                        Some(r) if r.contains(PollFlags::POLLIN) => ready.push(i),
                        Some(r) if r.intersects(PollFlags::POLLERR | PollFlags::POLLHUP | PollFlags::POLLNVAL) => dead.push(i),
                        _ => {}
                    }
                }

                for i in ready {
                    let (_, dev) = &mut devs[i];
                    // 先把事件收下来再处理：fetch_events 借用了设备，
                    // 借用要在后面的 flush_motion（&mut dev）之前结束
                    let mut events: Vec<evdev::InputEvent> = Vec::new();
                    if let Err(e) = dev.device.fetch_events().map(|it| events.extend(it)) {
                        if e.kind() == ErrorKind::WouldBlock {
                            events = Vec::new();
                        } else {
                            dev.flush_motion(&sink); // 掉线前最后一段位移别丢
                            dead.push(i);
                            continue;
                        }
                    }
                    for ev in events {
                        handle_event(dev, ev, &sink);
                    }
                    // 兜底：批次的最后一帧可能没有 SYN_REPORT 收尾
                    dev.flush_motion(&sink);
                }

                if !dead.is_empty() {
                    dead.sort_unstable();
                    dead.dedup();
                    log::info!("输入设备移除: {} 个", dead.len());
                    let dead_set: Vec<usize> = dead;
                    devs = devs
                        .into_iter()
                        .enumerate()
                        .filter(|(i, _)| !dead_set.contains(i))
                        .map(|(_, v)| v)
                        .collect();
                    if devs.is_empty() {
                        shared.set_input_status(false, "所有输入设备已断开。".to_string());
                    }
                }
            }
        })
        .expect("无法启动 evdev 线程");
}

fn handle_event(dev: &mut Dev, ev: evdev::InputEvent, sink: &Sender<InEvent>) {
    let (ty, code, value) = (ev.event_type().0, ev.code(), ev.value());
    if ty == EV_SYN {
        // 一个 SYN_REPORT 就是一段完整的相对位移：分帧求模长再累加。
        // 把整批（多个 SYN 帧）的 dx/dy 相加后只取一次模长，会让批次内反向的移动互相抵消（系统性少计）
        if code == SYN_REPORT {
            dev.flush_motion(sink);
        }
        return;
    }
    if ty == EV_KEY {
        if value != 1 {
            return; // 只计按下（value==2 是自动重复）
        }
        match code {
            BTN_LEFT => {
                let _ = sink.send(InEvent::Mouse(MouseEv::LeftUp));
            }
            BTN_RIGHT => {
                let _ = sink.send(InEvent::Mouse(MouseEv::RightUp));
            }
            BTN_MIDDLE => {
                let _ = sink.send(InEvent::Mouse(MouseEv::MiddleUp));
            }
            BTN_SIDE | BTN_EXTRA => {
                let _ = sink.send(InEvent::Mouse(MouseEv::XUp));
            }
            _ if dev.is_keyboard => match idx_by_lin_code(code) {
                Some(idx) => {
                    let _ = sink.send(InEvent::Key(idx));
                }
                None => {
                    let _ = sink.send(InEvent::KeyOther(other_key_id_lin(code)));
                }
            },
            _ => {}
        }
    } else if ty == EV_REL {
        match code {
            REL_X => dev.batch_dx += value as f64,
            REL_Y => dev.batch_dy += value as f64,
            REL_WHEEL => {
                if value != 0 {
                    let _ = sink.send(InEvent::Mouse(MouseEv::Wheel));
                }
            }
            REL_HWHEEL => {
                if value != 0 {
                    let _ = sink.send(InEvent::Mouse(MouseEv::HWheel));
                }
            }
            REL_WHEEL_HI_RES if dev.wheel_via_hires => {
                dev.wheel_frac += value.abs() as f64 / 120.0;
                while dev.wheel_frac >= 1.0 {
                    dev.wheel_frac -= 1.0;
                    let _ = sink.send(InEvent::Mouse(MouseEv::Wheel));
                }
            }
            REL_HWHEEL_HI_RES if dev.wheel_via_hires => {
                dev.hwheel_frac += value.abs() as f64 / 120.0;
                while dev.hwheel_frac >= 1.0 {
                    dev.hwheel_frac -= 1.0;
                    let _ = sink.send(InEvent::Mouse(MouseEv::HWheel));
                }
            }
            _ => {}
        }
    }
}
