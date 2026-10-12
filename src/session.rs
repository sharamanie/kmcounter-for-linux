//! 会话结束（关机 / 重启 / 休眠）时的处理。
//!
//! 为什么需要：`close_to_tray = true` 时，我们会**取消**窗口关闭请求（点 × = 收起托盘），
//! 但 KDE 关机/重启时给窗口发的也是同样的关闭请求 —— 一律取消的话，桌面会停在
//! “正在等待 KMCounter-rs 关闭”，把关机/重启挡住。窗口关闭事件本身在 Wayland 下
//! 不携带原因，因此改用 logind 的信号来识别“会话真的要结束了”：
//!   - `PrepareForShutdown(true)`：即将关机/重启 → 保存并立即退出（不再取消任何东西）
//!   - `PrepareForSleep(true)`：即将休眠 → 只保存，不退出
//!   - **注销**：Wayland 下桌面同样只是给窗口发关闭请求，没有原因字段。于是再加两路：
//!     · 监听用户总线上的 `org.kde.LogoutPrompt`（Plasma 注销/确认注销时会拉起它）
//!       —— 见到它就记下"会话要结束了"，若此刻我们刚取消过关窗（说明桌面正等我们），直接保存退出；
//!     · 轮询 logind 会话属性 `State`，变成 `closing` 说明会话正在终止 → 保存退出。
//!     窗口关闭回调里也会读"会话要结束了"这个标记：确实要注销就直接退出，不再收起托盘。
//!
//! **窗口根本收不到关闭请求的情况**（2026-10-12 补）：窗口处于"仅托盘"（最小化）状态时，
//! 桌面在注销/关机流程里发出的关闭请求到不了窗口 —— 关窗回调不会被触发，上面"取消关窗"
//! 与"关窗时按会话结束处理"这两条路都不会走到，桌面于是停在"正在等待 KMCounter-rs 关闭"
//! 一直等到超时（实测约 20 秒）再强杀（`kwin_killer_helper`）。日志特征：注销提示之后
//! 一大段空白，最后只有 SIGTERM 处理器收场。因此再加一路**不依赖窗口状态**的信号：
//!   - 轮询 `org.kde.ksmserver` 的 `org.kde.KSMServerInterface.isShuttingDown()`：这是会话
//!     管理器自己维护的"正在关闭会话"状态，用户取消注销时桌面会调 `resetLogout` 把它复位，
//!     所以连续读到它就可以放心保存退出 —— 与窗口是显示还是最小化无关。
//!   - 辅助：`org.kde.Shutdown`（只有 logout/logoutAndShutdown/logoutAndReboot/saveSession
//!     四个方法，用户确认之后才会出现的服务）出现时也记下"会话要结束了"，
//!     让关窗回调那条路更早生效（不直接退出：saveSession 同样会拉起它，有误判风险）。

use crate::config::Config;
use crate::stats::Shared;
use parking_lot::Mutex;
#[cfg(target_os = "linux")]
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

/// 会话是否正在结束（注销/关机/重启），由下面的监听线程置位
static SESSION_ENDING: AtomicBool = AtomicBool::new(false);
static SESSION_ENDING_AT: Mutex<Option<Instant>> = Mutex::new(None);
/// 最近一次"因点 × 而取消关窗"的时刻，用于判断桌面是否正在等我们退出
static CLOSE_CANCELLED_AT: Mutex<Option<Instant>> = Mutex::new(None);
/// 见到桌面拉起注销提示时的处理：记下"会话要结束了"；若我们刚取消过关窗，
/// 说明桌面正卡在等我们退出，直接保存退出。
#[cfg(target_os = "linux")]
fn handle_logout_prompt(shared: &Arc<Shared>, cfg: &Arc<Mutex<Config>>, stats_path: &Path, cfg_path: &Path) {
    mark_session_ending("桌面拉起了注销提示");
    if close_cancelled_recently() {
        save_and_exit(shared, cfg, stats_path, cfg_path, "注销流程正在等待本程序退出");
    }
}

/// 标记有效期：会话结束流程一般在几十秒内完成
const ENDING_TTL: Duration = Duration::from_secs(20);
#[cfg(target_os = "linux")]
const CANCELLED_TTL: Duration = Duration::from_secs(30);

/// 记录"会话正在结束"（注销/关机/重启）。
/// Linux 侧由 logind/注销提示监听线程调用；Windows 侧目前没有调用方，
/// 但保留了这条接口：给主窗口做子类化拦到 `WM_QUERYENDSESSION` 时置位它，
/// 随后的关闭请求就会走"会话结束 → 保存退出"这条路径（见 HANDOFF-Windows.md）。
#[allow(dead_code)]
pub fn mark_session_ending(why: &str) {
    log::info!("检测到会话即将结束（{why}）");
    SESSION_ENDING.store(true, Ordering::Relaxed);
    *SESSION_ENDING_AT.lock() = Some(Instant::now());
}

/// 会话是否在最近（TTL 内）进入结束流程
pub fn session_ending_fresh() -> bool {
    SESSION_ENDING.load(Ordering::Relaxed)
        && SESSION_ENDING_AT.lock().map_or(false, |t| t.elapsed() < ENDING_TTL)
}

/// GUI 因点 × 收起窗口时调用（取消了一次关窗）
pub fn mark_close_cancelled() {
    *CLOSE_CANCELLED_AT.lock() = Some(Instant::now());
}

/// 最近是否取消过窗口关闭（说明我们可能正卡着桌面的注销流程）
#[cfg(target_os = "linux")]
fn close_cancelled_recently() -> bool {
    CLOSE_CANCELLED_AT.lock().map_or(false, |t| t.elapsed() < CANCELLED_TTL)
}

/// 保存统计数据与配置后立即退出（跳过析构，避免与渲染线程的 GL 清理竞态）
#[cfg(target_os = "linux")]
fn save_and_exit(shared: &Arc<Shared>, cfg: &Arc<Mutex<Config>>, stats_path: &Path, cfg_path: &Path, why: &str) -> ! {
    log::info!("{why}：保存数据并退出");
    let store = (*shared.store.lock()).clone();
    if let Err(e) = store.save(stats_path) {
        log::error!("关机前保存统计数据失败: {e}");
    }
    let _ = cfg.lock().save(cfg_path);
    crate::force_exit()
}

#[cfg(target_os = "linux")]
fn save_only(shared: &Arc<Shared>, cfg: &Arc<Mutex<Config>>, stats_path: &Path, cfg_path: &Path, why: &str) {
    log::info!("{why}：保存数据");
    let store = (*shared.store.lock()).clone();
    if let Err(e) = store.save(stats_path) {
        log::error!("休眠前保存统计数据失败: {e}");
    }
    let _ = cfg.lock().save(cfg_path);
}

/// ksmserver 轮询间隔：桌面等待窗口关闭的超时是数十秒，一秒内发现足够快
#[cfg(target_os = "linux")]
const KSMSERVER_POLL_INTERVAL: Duration = Duration::from_millis(500);
/// 连续读到多少次"正在关闭"才退出（防单次读数抖动）
#[cfg(target_os = "linux")]
const SHUTTING_DOWN_STREAK: u8 = 2;

/// 连到会话管理器（ksmserver）的会话管理接口；非 KDE 会话没有这个服务 → Err
#[cfg(target_os = "linux")]
fn ksmserver_proxy(conn: &zbus::blocking::Connection) -> Result<zbus::blocking::Proxy<'_>, zbus::Error> {
    zbus::blocking::Proxy::new(conn, "org.kde.ksmserver", "/KSMServer", "org.kde.KSMServerInterface")
}

/// 读一次会话管理器的"正在关闭会话"状态（注销/关机/重启进行中）
#[cfg(target_os = "linux")]
fn ksmserver_is_shutting_down(proxy: &zbus::blocking::Proxy<'_>) -> Result<bool, String> {
    proxy.call::<_, (), bool>("isShuttingDown", &()).map_err(|e| e.to_string())
}

/// 连续判定：只有连续 `need` 次读到"正在关闭"才认为确定要退出。
/// 中途读到 false（用户取消了注销，桌面会复位状态）就清零重来，避免单次读数抖动误判。
#[cfg(target_os = "linux")]
fn shutting_down_streak(streak: &mut u8, reading: bool, need: u8) -> bool {
    *streak = if reading { streak.saturating_add(1) } else { 0 };
    *streak >= need
}

#[cfg(target_os = "linux")]
pub fn spawn_guard(shared: Arc<Shared>, cfg: Arc<Mutex<Config>>, stats_path: PathBuf, cfg_path: PathBuf) {
    // 注销提示监听（用户总线）：Plasma 要注销时会拉起 org.kde.LogoutPrompt
    {
        let (shared, cfg, stats_path, cfg_path) = (shared.clone(), cfg.clone(), stats_path.clone(), cfg_path.clone());
        std::thread::Builder::new()
            .name("kmcounter-logout".into())
            .spawn(move || {
                let Ok(conn) = zbus::blocking::Connection::session() else {
                    log::warn!("连接用户总线失败，注销时可能被桌面提示等待");
                    return;
                };
                let Ok(proxy) = zbus::blocking::Proxy::new(&conn, "org.freedesktop.DBus", "/org/freedesktop/DBus", "org.freedesktop.DBus") else {
                    return;
                };
                let Ok(signals) = proxy.receive_signal("NameOwnerChanged") else {
                    return;
                };
                for msg in signals {
                    let Ok((name, _old, new)) = msg.body().deserialize::<(String, String, String)>() else {
                        continue;
                    };
                    if new.is_empty() {
                        continue; // 只看"服务出现"
                    }
                    if name == "org.kde.LogoutPrompt" {
                        handle_logout_prompt(&shared, &cfg, &stats_path, &cfg_path);
                    } else if name == "org.kde.Shutdown" {
                        // 该接口只有 logout / logoutAndShutdown / logoutAndReboot / saveSession，
                        // 也就是"用户已经确认"之后才会出现（确认框是另一个服务 org.kde.LogoutPrompt）。
                        // 这里只记标记、不直接退出：saveSession 同样会拉起这个服务，直接退有误判风险；
                        // 真正的退出交给下面的 ksmserver 轮询。
                        mark_session_ending("桌面开始执行注销/关机");
                    }
                }
            })
            .ok();
    }

    // KDE：会话管理器报告"正在关闭会话" → 保存并退出。
    // 窗口处于"仅托盘"（最小化）时，桌面在注销/关机流程里发给我们窗口的关闭请求到不了，
    // 于是桌面停在"正在等待 …关闭"、约 20 秒后强杀进程。这个状态与窗口是否显示无关；
    // 用户取消注销时桌面会复位它（resetLogout），所以连续读到就可以放心退出。
    {
        let (shared, cfg, stats_path, cfg_path) = (shared.clone(), cfg.clone(), stats_path.clone(), cfg_path.clone());
        std::thread::Builder::new()
            .name("kmcounter-ksmserver".into())
            .spawn(move || {
                let Ok(conn) = zbus::blocking::Connection::session() else { return };
                let Ok(proxy) = ksmserver_proxy(&conn) else {
                    log::debug!("没有 ksmserver（非 KDE 会话），跳过会话关闭轮询");
                    return;
                };
                let mut streak = 0u8;
                let mut failures = 0u8;
                loop {
                    std::thread::sleep(KSMSERVER_POLL_INTERVAL);
                    let reading = match ksmserver_is_shutting_down(&proxy) {
                        Ok(v) => {
                            failures = 0;
                            v
                        }
                        Err(e) => {
                            failures += 1;
                            if failures >= 3 {
                                log::debug!("查询 ksmserver 会话状态失败，停止轮询：{e}");
                                return;
                            }
                            continue;
                        }
                    };
                    if shutting_down_streak(&mut streak, reading, SHUTTING_DOWN_STREAK) {
                        save_and_exit(&shared, &cfg, &stats_path, &cfg_path, "KDE 会话正在关闭（注销 / 关机 / 重启）");
                    }
                }
            })
            .ok();
    }

    // 会话状态轮询：State 变成 closing 说明会话正在终止
    {
        let (shared, cfg, stats_path, cfg_path) = (shared.clone(), cfg.clone(), stats_path.clone(), cfg_path.clone());
        std::thread::Builder::new()
            .name("kmcounter-sessionstate".into())
            .spawn(move || {
                let Ok(conn) = zbus::blocking::Connection::system() else { return };
                let Ok(manager) = zbus::blocking::Proxy::new(&conn, "org.freedesktop.login1", "/org/freedesktop/login1", "org.freedesktop.login1.Manager") else {
                    return;
                };
                let path: zbus::zvariant::OwnedObjectPath = match manager.call("GetSession", &("auto",)) {
                    Ok(p) => p,
                    Err(e) => {
                        log::debug!("取当前 logind 会话失败：{e}");
                        return;
                    }
                };
                let Ok(session) = zbus::blocking::Proxy::new(&conn, "org.freedesktop.login1", path.as_str(), "org.freedesktop.login1.Session") else {
                    return;
                };
                let mut last = String::new();
                loop {
                    std::thread::sleep(Duration::from_millis(1000));
                    let Ok(state) = session.get_property::<String>("State") else { continue };
                    if state != last {
                        log::debug!("logind 会话状态: {last} → {state}");
                        last = state.clone();
                    }
                    if state == "closing" {
                        save_and_exit(&shared, &cfg, &stats_path, &cfg_path, "会话正在终止（注销）");
                    }
                }
            })
            .ok();
    }

    // 调试：KMCOUNTER_FAKE_LOGOUT_AFTER_SECS=N → N 秒后模拟"桌面开始注销"（不真的注销，
    // 用于验证：之后的窗口关闭请求应直接退出而不是收起托盘）
    if let Ok(secs) = std::env::var("KMCOUNTER_FAKE_LOGOUT_AFTER_SECS").map(|v| v.parse::<u64>()) {
        if let Ok(secs) = secs {
            let (shared, cfg, stats_path, cfg_path) = (shared.clone(), cfg.clone(), stats_path.clone(), cfg_path.clone());
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_secs(secs));
                log::info!("调试：模拟桌面拉起注销提示");
                handle_logout_prompt(&shared, &cfg, &stats_path, &cfg_path);
            });
        }
    }

    // 调试：KMCOUNTER_FAKE_SHUTTINGDOWN_AFTER_SECS=N → N 秒后走一遍"检测到会话正在关闭"的退出路径。
    // 用来验证"窗口在托盘状态（最小化）下也能及时保存退出"这条新增路径（与真实轮询共用同一个退出处理）。
    if let Ok(Ok(secs)) = std::env::var("KMCOUNTER_FAKE_SHUTTINGDOWN_AFTER_SECS").map(|v| v.parse::<u64>()) {
        let (shared, cfg, stats_path, cfg_path) = (shared.clone(), cfg.clone(), stats_path.clone(), cfg_path.clone());
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_secs(secs));
            log::info!("调试：模拟 ksmserver 报告会话正在关闭");
            save_and_exit(&shared, &cfg, &stats_path, &cfg_path, "调试：模拟 KDE 会话正在关闭");
        });
    }

    // 调试：KMCOUNTER_FAKE_SHUTDOWN_AFTER_SECS=N → N 秒后走一遍关机路径（验证保存与退出）
    if let Ok(secs) = std::env::var("KMCOUNTER_FAKE_SHUTDOWN_AFTER_SECS").map(|v| v.parse::<u64>()) {
        if let Ok(secs) = secs {
            let (shared, cfg, stats_path, cfg_path) = (shared.clone(), cfg.clone(), stats_path.clone(), cfg_path.clone());
            std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_secs(secs));
                save_and_exit(&shared, &cfg, &stats_path, &cfg_path, "调试：模拟系统关机");
            });
        }
    }

    // 休眠监听线程要用同一批句柄，先克隆一份
    let (shared2, cfg2, stats_path2, cfg_path2) = (shared.clone(), cfg.clone(), stats_path.clone(), cfg_path.clone());

    std::thread::Builder::new()
        .name("kmcounter-session".into())
        .spawn(move || {
            let conn = match zbus::blocking::Connection::system() {
                Ok(c) => c,
                Err(e) => {
                    log::warn!("连接系统总线失败，关机时可能被桌面提示等待：{e}");
                    return;
                }
            };
            let proxy = match zbus::blocking::Proxy::new(
                &conn,
                "org.freedesktop.login1",
                "/org/freedesktop/login1",
                "org.freedesktop.login1.Manager",
            ) {
                Ok(p) => p,
                Err(e) => {
                    log::warn!("订阅 logind 失败：{e}");
                    return;
                }
            };
            match proxy.receive_signal("PrepareForShutdown") {
                Ok(signals) => {
                    log::debug!("已订阅 logind 关机/重启信号");
                    for msg in signals {
                        let going_down: bool = msg.body().deserialize().unwrap_or(false);
                        if going_down {
                            save_and_exit(&shared, &cfg, &stats_path, &cfg_path, "系统即将关机/重启");
                        } else {
                            log::info!("关机/重启已取消");
                        }
                    }
                }
                Err(e) => log::warn!("监听 logind 关机信号失败：{e}"),
            }
        })
        .ok();

    // 休眠信号单独一个线程（阻塞式信号迭代器一次只能听一个）
    std::thread::Builder::new()
        .name("kmcounter-sleep".into())
        .spawn(move || {
            let Ok(conn) = zbus::blocking::Connection::system() else { return };
            let Ok(proxy) = zbus::blocking::Proxy::new(
                &conn,
                "org.freedesktop.login1",
                "/org/freedesktop/login1",
                "org.freedesktop.login1.Manager",
            ) else {
                return;
            };
            if let Ok(signals) = proxy.receive_signal("PrepareForSleep") {
                for msg in signals {
                    if msg.body().deserialize().unwrap_or(false) {
                        save_only(&shared2, &cfg2, &stats_path2, &cfg_path2, "系统即将休眠");
                    }
                }
            }
        })
        .ok();
}

#[cfg(not(target_os = "linux"))]
pub fn spawn_guard(_shared: Arc<Shared>, _cfg: Arc<Mutex<Config>>, _stats_path: PathBuf, _cfg_path: PathBuf) {
    // Windows 会在关机时向窗口发 WM_QUERYENDSESSION/WM_ENDSESSION，由系统负责，无需额外处理
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;

    /// 连续两次为真才判定退出；中途读到 false（用户取消注销）必须清零重来
    #[test]
    fn shutting_down_needs_consecutive_readings() {
        let mut streak = 0;
        assert!(!shutting_down_streak(&mut streak, true, 2), "单次 true 不足以判定退出");
        assert!(shutting_down_streak(&mut streak, true, 2), "连续两次 true 应判定为退出");

        let mut streak = 0;
        assert!(!shutting_down_streak(&mut streak, true, 2));
        assert!(!shutting_down_streak(&mut streak, false, 2), "读到 false 应立即复位");
        assert!(!shutting_down_streak(&mut streak, true, 2), "复位后重新计数");
        assert!(shutting_down_streak(&mut streak, true, 2));
    }

    /// 真机核对：本机（KDE）能读到会话管理器的状态，且平时必须是 false。
    /// 非 KDE 环境（CI、其他桌面）没有 ksmserver → 视为通过。
    #[test]
    fn ksmserver_reports_not_shutting_down() {
        let Ok(conn) = zbus::blocking::Connection::session() else {
            eprintln!("跳过：没有会话总线");
            return;
        };
        let Ok(proxy) = ksmserver_proxy(&conn) else {
            eprintln!("跳过：本机没有 ksmserver（非 KDE 会话）");
            return;
        };
        match ksmserver_is_shutting_down(&proxy) {
            Ok(v) => assert!(!v, "正常运行（没有人在注销）时 isShuttingDown 必须是 false"),
            Err(e) => eprintln!("跳过：查询 ksmserver 会话状态失败（{e}）"),
        }
    }
}
