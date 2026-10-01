//! KMCounter-rs：用 Rust 重建的 KMCounter（键鼠使用统计 + 键盘热力图）
//! 支持 Windows（低级钩子）与 Linux/Arch（evdev，X11/Wayland 皆可）。

mod autostart;
mod chart;
mod config;
mod controller;
mod fonts;
mod gui;
mod import;
mod input;
mod keys;
mod kwin;
mod lang;
#[cfg(feature = "softshot")]
mod softshot;
mod session;
mod stats;
mod tray;
mod trend;

use config::Config;
use lang::{strings, Lang};
use stats::{Shared, Store};
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::Arc;

const APP: &str = "KMCounter-rs";

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn,kmcounter=info")).init();

    let mut config_path: Option<PathBuf> = None;
    let mut export_path: Option<Option<PathBuf>> = None;
    let mut import_path: Option<PathBuf> = None;
    let mut rest: Vec<String> = Vec::new();
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--config" | "-c" => {
                config_path = args.next().map(PathBuf::from);
            }
            "--export" => {
                // 可跟路径；不带路径则导出到当前目录 kmcounter_export_日期.csv
                let p = args.next().filter(|s| !s.starts_with('-')).map(PathBuf::from);
                export_path = Some(p);
            }
            "--import" => {
                // 导入原版 KMCounter.ini
                import_path = args.next().map(PathBuf::from);
            }
            _ => rest.push(a),
        }
    }

    let dir = config::data_dir(config_path.clone());
    let cfg_path = if config_path.is_some() {
        config_path.unwrap()
    } else {
        dir.join("config.toml")
    };
    let stats_path = cfg_path.with_file_name("stats.json");

    for a in &rest {
        match a.as_str() {
            "--version" | "-V" => {
                println!("{APP} v{}", env!("CARGO_PKG_VERSION"));
                return;
            }
            "--help" | "-h" => {
                print_help();
                return;
            }
            "--stats" => {
                cmd_stats(&stats_path);
                return;
            }
            "--reset" => {
                cmd_reset(&stats_path);
                return;
            }
            other => {
                eprintln!("未知参数: {other}\n");
                print_help();
                std::process::exit(2);
            }
        }
    }

    if let Some(path) = import_path {
        cmd_import(&stats_path, &cfg_path, path);
        return;
    }

    if let Some(path) = export_path {
        cmd_export(&stats_path, &cfg_path, path);
        return;
    }

    run_app(cfg_path, stats_path);
}

fn print_help() {
    println!(
        "{APP} v{} — 键鼠使用统计 + 键盘热力图（Rust 重建版 KMCounter）

用法:
  kmcounter               常驻运行（托盘 + 统计窗口）
  kmcounter --stats       在终端查看今日/总计统计
  kmcounter --export [路径]  导出每日数据 CSV（同目录另存 *_hourly.csv 分时明细）
                             默认 ./kmcounter_export_日期.csv
  kmcounter --reset       重置数据（询问 today/all）
  kmcounter --import INI   导入原版 KMCounter 的 KMCounter.ini 数据
  kmcounter --config PATH 指定配置文件路径（stats.json 存放在同目录）
  kmcounter --version     显示版本

参数:
  -c, --config <PATH>     配置文件路径
  -h, --help              显示本帮助",
        env!("CARGO_PKG_VERSION")
    );
}

/// 导出每日数据 CSV（按日期升序；只含有数据的天）
fn cmd_export(stats_path: &Path, cfg_path: &Path, out: Option<PathBuf>) {
    let store = Store::load(stats_path);
    if store.days.is_empty() {
        println!("没有可导出的数据。");
        return;
    }

    // 与 --stats 相同的换算参数
    let cfg = Config::load(&cfg_path.with_file_name("config.toml")).0;
    let mm_w = resolve_screen_mm(&cfg).0 as f64;
    let px_w = resolve_screen_px_w(&cfg) as f64;
    let px_to_m = |px: f64| px * mm_w / px_w / 1000.0;

    let out = out.unwrap_or_else(|| {
        PathBuf::from(format!("kmcounter_export_{}.csv", stats::today_string()))
    });

    let header = "date,keystrokes,mouse_move_px,mouse_move_meters,mouse_lb,mouse_rb,mouse_mb,mouse_xb,mouse_wheel,mouse_hwheel";
    let mut lines: Vec<String> = vec![header.to_string()];
    for (date, d) in &store.days {
        lines.push(format!(
            "{},{},{},{:.4},{},{},{},{},{},{}",
            date,
            d.keystrokes,
            d.mouse.move_px,
            px_to_m(d.mouse.move_px),
            d.mouse.lb,
            d.mouse.rb,
            d.mouse.mb,
            d.mouse.xb,
            d.mouse.wheel,
            d.mouse.hwheel
        ));
    }
    match std::fs::write(&out, lines.join("\n") + "\n") {
        Ok(_) => println!("已导出 {} 天的数据 → {}", lines.len() - 1, out.display()),
        Err(e) => {
            eprintln!("导出失败: {e}");
            std::process::exit(1);
        }
    }

    // 分时明细：另存一份 xxx_hourly.csv（有分时数据时才写）
    let stem = out.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| "kmcounter_export".into());
    let ext = out.extension().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| "csv".into());
    let hourly_out = out.with_file_name(format!("{stem}_hourly.{ext}"));
    let mut hlines: Vec<String> = vec![
        "date,hour,keystrokes,mouse_move_px,mouse_move_meters,mouse_lb,mouse_rb,mouse_mb,mouse_xb,mouse_wheel,mouse_hwheel"
            .to_string(),
    ];
    for (date, d) in &store.days {
        let mut hrs: Vec<(&u8, &stats::BucketStats)> = d.hours.iter().collect();
        hrs.sort_by_key(|(h, _)| **h);
        for (h, u) in hrs {
            // 作废的小时按 0 导出（与界面一致；原始值仍保留在 stats.json 里）
            let (ks, px) = if u.invalid { (0, 0.0) } else { (u.keystrokes, u.mouse.move_px) };
            hlines.push(format!(
                "{},{},{},{},{:.4},{},{},{},{},{},{}",
                date,
                h,
                ks,
                px,
                px_to_m(px),
                u.mouse.lb,
                u.mouse.rb,
                u.mouse.mb,
                u.mouse.xb,
                u.mouse.wheel,
                u.mouse.hwheel
            ));
        }
    }
    if hlines.len() > 1 {
        match std::fs::write(&hourly_out, hlines.join("\n") + "\n") {
            Ok(_) => println!("已导出 {} 个小时的明细 → {}", hlines.len() - 1, hourly_out.display()),
            Err(e) => eprintln!("分时明细导出失败: {e}"),
        }
    }
}

/// 导入原版 KMCounter（AHK）的 KMCounter.ini
fn cmd_import(stats_path: &Path, cfg_path: &Path, ini_path: PathBuf) {
    let ini = match import::read_ini(&ini_path) {
        Ok(i) => i,
        Err(e) => {
            eprintln!("无法解析 {}：{e}", ini_path.display());
            std::process::exit(1);
        }
    };

    let mut store = Store::load(stats_path);
    let before_days = store.days.len();

    // 鼠标“米 → 像素”换算：用本机屏幕的比例，这样界面显示的距离与原版一致
    let cfg_file = &cfg_path.with_file_name("config.toml");
    let (cfg, _) = Config::load(cfg_file);
    let mm_w = resolve_screen_mm(&cfg).0;
    let px_w = resolve_screen_px_w(&cfg);
    let px_per_meter = if mm_w > 0 { 1000.0 * px_w as f64 / mm_w as f64 } else { 0.0 };

    let rep = import::import(&ini, &mut store, px_per_meter);

    println!("── 导入结果 ──");
    println!("数据区间    {} → {}", rep.first_day, rep.last_day);
    println!("导入天数    {}（原有 {} 天，现有 {} 天）", rep.days_imported, before_days, store.days.len());
    if !rep.days_skipped.is_empty() {
        println!("跳过天数    {}（新版本已有当天数据，保持不动）：{}", rep.days_skipped.len(), rep.days_skipped.join(", "));
    }
    println!("键盘敲击    {} 次（布局内 {} / 布局外 {}）", rep.keystrokes, rep.keys_in_layout, rep.keys_outside);
    println!(
        "鼠标移动    {:.2} 米 → 存储为 {:.0} 像素（按 {}毫米 / {}像素 换算）",
        rep.move_meters, rep.move_px, mm_w.max(1), px_w
    );
    if !rep.other_keys.is_empty() {
        let list: Vec<String> = rep.other_keys.iter().take(8).map(|(sc, n)| format!("sc{sc}×{n}")).collect();
        println!("布局外按键  {}（{}）", if rep.other_keys.len() > 8 { "部分" } else { "全部" }, list.join(" "));
    }
    if rep.days_adjusted > 0 {
        let (d, declared, actual) = rep.max_deviation.clone().unwrap_or_default();
        println!(
            "数据修正    {} 天的 keystrokes 与逐键合计不符，已按逐键合计修正（最多 {d}: 原版 {declared} → {actual}）",
            rep.days_adjusted
        );
    }
    println!("全历史总计  已按导入的天数汇总（原版 [total] 节声明 {} 次，与逐日数据本身不一致，未直接采用）", rep.ini_total_keystrokes);
    if let Some((w, h)) = rep.ini_screen_mm {
        println!("原版屏幕    物理 {w}×{h} 毫米（可在设置里填入“屏幕尺寸”）");
    }
    if let Some(l) = rep.ini_layout {
        println!("原版布局    键宽 {} 键高 {} 键间距 {} 水平间距 {} 垂直间距 {}", l[0], l[1], l[2], l[3], l[4]);
    }

    if let Err(e) = store.save(stats_path) {
        eprintln!("保存失败: {e}");
        std::process::exit(1);
    }
    println!("已写入 {}", stats_path.display());

    // 导入的历史可能早于 history_days：不改大一点，程序启动时会把它们清理掉
    if let Some(storage) = rep.ini_history_days {
        let (mut cfg, _) = Config::load(cfg_file);
        if storage > cfg.history_days {
            let old = cfg.history_days;
            cfg.history_days = storage;
            if let Err(e) = cfg.save(cfg_file) {
                eprintln!("更新 history_days 失败: {e}");
            } else {
                println!("历史保留天数 {} → {}（原版设置为 {storage}，否则旧数据会被自动清理）", old, storage);
            }
        }
    }
    println!("提示：程序正在运行时导入无效（它会在保存时覆盖），请先退出再导入。");
}

fn cmd_reset(stats_path: &Path) {
    println!("1) 仅重置今日数据  2) 清空全部历史数据");
    print!("请选择 [1/2]: ");
    use std::io::Write;
    std::io::stdout().flush().ok();
    let mut line = String::new();
    std::io::stdin().read_line(&mut line).ok();
    match line.trim() {
        "1" => {
            let mut store = Store::load(stats_path);
            let today = stats::today_string();
            store.reset_day(&today);
            store.save(stats_path).ok();
            println!("已重置今日数据（总计不受影响）。");
        }
        "2" => {
            let _ = std::fs::remove_file(stats_path);
            println!("已清空全部数据。");
        }
        _ => println!("已取消。"),
    }
}

fn cmd_stats(stats_path: &Path) {
    let lang = strings(Lang::resolve("auto"));
    let store = Store::load(stats_path);
    let today = stats::today_string();
    let cfg = Config::load(&stats_path.with_file_name("config.toml")).0;
    let mm_w = crate::resolve_screen_mm(&cfg).0 as f64;
    let px_w = resolve_screen_px_w(&cfg) as f64;

    let show = |title: &str, d: Option<&stats::BucketStats>| {
        let d = d.cloned().unwrap_or_default();
        println!("── {title} ──");
        println!(
            "{:<10} {:.2} {}",
            lang.mouse_move,
            d.mouse.move_px * mm_w / px_w / 1000.0,
            lang.unit_m
        );
        println!("{:<10} {} {}", lang.keystrokes, d.keystrokes, lang.unit_times.trim());
        // 键盘分区：键盘敲击的细分（只算布局内的按键）
        for z in keys::ZONES {
            let name = match z {
                keys::Zone::Main => lang.zone_main,
                keys::Zone::Function => lang.zone_function,
                keys::Zone::Control => lang.zone_control,
                keys::Zone::Numpad => lang.zone_numpad,
            };
            println!("  {:<8} {}", name, keys::zone_sum(z, |i| d.key_count(i)));
        }
        println!(
            "{:<10} L={} R={} M={} X={} W={} H={}",
            lang.lbutton, d.mouse.lb, d.mouse.rb, d.mouse.mb, d.mouse.xb, d.mouse.wheel, d.mouse.hwheel
        );
    };
    show(&format!("{} ({today})", lang.col_today), store.day(&today).map(|d| &d.bucket));
    show(lang.total_label, Some(&store.total.bucket));

    // 分时：今日各小时（只列有数据的小时）+ 最活跃时段
    if let Some(entry) = store.day(&today) {
        if !entry.hours.is_empty() {
            println!("── {} ──", lang.hour_dist);
            let mut hours: Vec<(u8, u64, f64)> = entry
                .hours
                .iter()
                .map(|(h, u)| (*h, u.keystrokes, u.mouse.move_px * mm_w / px_w / 1000.0))
                .collect();
            hours.sort_by_key(|(h, _, _)| *h);
            for (h, ks, mv) in &hours {
                let bad = entry.hour_invalid(*h);
                println!(
                    "{}{}   {} {} {}   {} {:.2} {}{}",
                    h,
                    lang.hour_suffix,
                    lang.keystrokes,
                    ks,
                    lang.unit_times.trim(),
                    lang.mouse_move,
                    mv,
                    lang.unit_m,
                    if bad { "   [已作废，未计入当日/总计]" } else { "" }
                );
            }
            if let Some((h, ks, _)) = hours.iter().max_by_key(|(_, k, _)| *k) {
                println!("{}: {}{} ({ks} {})", lang.hour_peak, h, lang.hour_suffix, lang.unit_times.trim());
            }
        }
    }

    // 最常用按键
    let day = store.day(&today).cloned().unwrap_or_default();
    let mut kv: Vec<(String, u64)> = keys::KEYS
        .iter()
        .enumerate()
        .map(|(i, k)| (k.name.to_string(), day.keys.get(i).copied().unwrap_or(0)))
        .filter(|(_, c)| *c > 0)
        .collect();
    for (id, c) in &day.other {
        kv.push((stats::other_key_label(id), *c));
    }
    kv.sort_by(|a, b| b.1.cmp(&a.1));
    if !kv.is_empty() {
        println!("── {} ──", lang.top_keys);
        for (name, c) in kv.iter().take(15) {
            println!("{:<12} {}", name, c);
        }
    }
}

fn run_app(cfg_path: PathBuf, stats_path: PathBuf) {
    // Windows 双击/开机启动会给控制台程序分配一个黑窗口：GUI 模式下摘掉它
    //（--stats 等命令行模式在这之前就已返回，输出不受影响）
    #[cfg(windows)]
    detach_console();

    let show_request = config::data_dir(None).join("show.request");

    // 单实例：已在运行则请求它显示窗口（而不是干巴巴报错）
    if let Err(msg) = acquire_single_instance() {
        match std::fs::write(&show_request, b"show\n") {
            Ok(_) => {
                eprintln!("{APP}: {msg}\n已请求正在运行的实例显示窗口。");
                #[cfg(target_os = "linux")]
                if !kwin::is_kde() && std::env::var_os("WAYLAND_DISPLAY").is_some() {
                    eprintln!("提示：当前 Wayland 会话不支持程序自行恢复窗口，若窗口已最小化请点击任务栏条目。");
                }
            }
            Err(_) => eprintln!("{APP}: {msg}"),
        }
        std::process::exit(0);
    }

    let (cfg, first_run) = Config::load(&cfg_path);
    let mut store = Store::load(&stats_path);

    // 应用历史保留天数
    let purged = store.purge(cfg.history_days_clamped());
    if purged > 0 {
        log::info!("已清理 {purged} 天过期历史数据");
    }
    if let Err(e) = store.save(&stats_path) {
        log::error!("统计数据保存失败: {e}");
    }

    // 屏幕物理尺寸（用于鼠标移动距离换算）
    let (mm_w, mm_h) = resolve_screen_mm(&cfg);
    let px_w = resolve_screen_px_w(&cfg);
    // 分时校验阈值：公里换算成像素要用与界面相同的比例
    let px_per_meter = if mm_w > 0 { 1000.0 * px_w as f64 / mm_w as f64 } else { 0.0 };
    store.limits = stats::HourLimits::from_config(&cfg, px_per_meter);
    log::info!(
        "分时校验：鼠标移动超过 {} 公里/小时 即作废该小时；鼠标点击 {} 次/小时；键盘 {} 次/小时（0 = 关闭）",
        if cfg.limit_move_km > 0.0 { format!("{}", cfg.limit_move_km) } else { "∞".into() },
        if cfg.limit_clicks > 0 { cfg.limit_clicks.to_string() } else { "∞".into() },
        if cfg.limit_keystrokes > 0 { cfg.limit_keystrokes.to_string() } else { "∞".into() },
    );
    if cfg.limit_move_km > 0.0 && px_per_meter <= 0.0 {
        log::warn!("未知显示器物理尺寸，鼠标移动阈值（{} 公里/小时）暂时无法换算，已忽略", cfg.limit_move_km);
    }
    let shared = Shared::new(store, mm_w, mm_h);
    shared.screen_px_w.store(px_w, Ordering::Relaxed);

    // 输入捕获
    input::start(shared.clone());

    // 托盘（失败则窗口关闭即退出）
    let tray_lang = Lang::resolve(&cfg.language);
    let tray = tray::start(tray_lang);
    let autostart_state = Arc::new(std::sync::atomic::AtomicBool::new(autostart::is_enabled()));

    // KDE：按配置维护“跳过任务栏/Alt-Tab”的 KWin 窗口规则（后台执行，避免阻塞启动；仅 Linux）
    #[cfg(target_os = "linux")]
    {
        let skip = cfg.skip_taskbar;
        std::thread::Builder::new()
            .name("kmcounter-kwin".into())
            .spawn(move || kwin::set_skip_taskbar(skip))
            .ok();
    }

    // 共享配置（托盘控制线程与 GUI 线程共同持有）
    let cfg_shared = Arc::new(parking_lot::Mutex::new(cfg.clone()));

    // 后台定时保存：不依赖渲染帧（窗口最小化时帧会停止）
    {
        let shared = shared.clone();
        let cfg_shared = cfg_shared.clone();
        let stats_path = stats_path.clone();
        let cfg_path = cfg_path.clone();
        std::thread::Builder::new()
            .name("kmcounter-saver".into())
            .spawn(move || loop {
                std::thread::sleep(std::time::Duration::from_secs(30));
                if shared.dirty.load(Ordering::Relaxed) {
                    let store = (*shared.store.lock()).clone();
                    match store.save(&stats_path) {
                        Ok(_) => {
                            shared.dirty.store(false, Ordering::Relaxed);
                            log::debug!("后台定时保存完成");
                        }
                        Err(e) => log::error!("后台保存失败: {e}"),
                    }
                    let _ = cfg_shared.lock().save(&cfg_path);
                }
            })
            .ok();
    }

    // 关机/重启/休眠：保存并（关机时）主动退出，避免挡住桌面关机流程
    session::spawn_guard(shared.clone(), cfg_shared.clone(), stats_path.clone(), cfg_path.clone());

    // Ctrl+C / SIGTERM 时保存
    {
        let shared = shared.clone();
        let stats_path = stats_path.clone();
        std::thread::spawn(move || {
            if ctrlc::set_handler(move || {
                log::info!("收到退出信号，保存数据…");
                let store = (*shared.store.lock()).clone();
                let _ = store.save(&stats_path);
                force_exit();
            })
            .is_err()
            {
                log::warn!("无法注册 Ctrl+C 处理器");
            }
        });
    }

    // GUI
    // 调试/截图模式（KMCOUNTER_START_PANELS=stats|settings|both）：预展开面板并置顶显示
    let debug_panels = std::env::var("KMCOUNTER_START_PANELS").map(|v| !v.is_empty()).unwrap_or(false);
    let mut viewport = eframe::egui::ViewportBuilder::default()
        .with_inner_size([1260.0, 500.0])
        .with_min_inner_size([960.0, 380.0]);
    if debug_panels {
        viewport = viewport.with_always_on_top();
    }
    let options = eframe::NativeOptions { viewport, ..Default::default() };
    // 调试：KMCOUNTER_AUTO_EXIT_SECS=N → N 秒后模拟托盘“退出”命令（验证退出路径）
    if let Ok(secs) = std::env::var("KMCOUNTER_AUTO_EXIT_SECS").map(|v| v.parse::<u64>()) {
        if let (Ok(secs), Some(tx)) = (secs, tray.as_ref().map(|t| t.cmd_tx.clone())) {
            log::info!("调试：{secs} 秒后自动发送退出命令");
            std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_secs(secs));
                let _ = tx.send(tray::TrayCmd::Exit);
            });
        }
    }

    let shared_for_gui = shared.clone();
    let cfg_shared_for_gui = cfg_shared.clone();
    let cfg_path_clone = cfg_path.clone();
    let stats_path_clone = stats_path.clone();
    let autostart_for_gui = autostart_state.clone();
    let show_request_for_gui = show_request.clone();
    let result = eframe::run_native(
        APP,
        options,
        Box::new(move |cc| {
            Ok(Box::new(gui::App::new(
                &cc.egui_ctx,
                shared_for_gui,
                cfg_shared_for_gui,
                cfg_path_clone,
                stats_path_clone,
                show_request_for_gui,
                first_run,
                tray,
                autostart_for_gui,
            )))
        }),
    );
    if let Err(e) = result {
        log::error!("GUI 运行失败: {e}");
        // 至少把数据保住
        let store = (*shared.store.lock()).clone();
        let _ = store.save(&stats_path);
        std::process::exit(1);
    }

    // 正常退出后再保存一次（幂等）
    let store = (*shared.store.lock()).clone();
    let _ = store.save(&stats_path);
    let _ = cfg_shared.lock().save(&cfg_path);
}

/// Windows：双击/开机启动时隐藏并释放控制台黑窗口；
/// 但如果是从终端里启动的（控制台里还有别的进程），就保留控制台以便看到 RUST_LOG 日志。
#[cfg(windows)]
fn detach_console() {
    use windows_sys::Win32::System::Console::{FreeConsole, GetConsoleProcessList, GetConsoleWindow};
    use windows_sys::Win32::UI::WindowsAndMessaging::{ShowWindow, SW_HIDE};
    unsafe {
        let hwnd = GetConsoleWindow();
        if hwnd.is_null() {
            return; // 没有控制台（例如已是 GUI 子系统启动）
        }
        // 控制台里只有自己 → 是双击/自启拉起的，藏起来；还有别的进程 → 从终端启动，保留输出
        let mut list = [0u32; 4];
        let n = GetConsoleProcessList(list.as_mut_ptr(), list.len() as u32);
        if n > 1 {
            log::debug!("从终端启动，保留控制台输出");
            return;
        }
        ShowWindow(hwnd, SW_HIDE);
        FreeConsole();
    }
}

/// 屏幕物理尺寸：配置覆盖 > 平台自动检测 > 兜底估计
pub fn resolve_screen_mm(cfg: &Config) -> (u32, u32) {
    let (auto_w, auto_h) = auto_screen_mm();
    let w = if cfg.screen_w_mm > 0 { cfg.screen_w_mm } else { auto_w };
    let h = if cfg.screen_h_mm > 0 { cfg.screen_h_mm } else { auto_h };
    (w, h)
}

/// 信号处理器 / 控制线程中的立即退出：跳过 atexit/析构，避免与 GUI 线程的 GL 清理竞态
#[cfg(target_os = "linux")]
pub(crate) fn force_exit() -> ! {
    // SAFETY: _exit 是异步信号安全的立即终止
    unsafe { libc::_exit(0) }
}

#[cfg(not(target_os = "linux"))]
pub(crate) fn force_exit() -> ! {
    std::process::exit(0)
}

#[cfg(windows)]
fn auto_screen_mm() -> (u32, u32) {
    unsafe {
        let hdc = windows_sys::Win32::Graphics::Gdi::GetDC(std::ptr::null_mut());
        if !hdc.is_null() {
            let w = windows_sys::Win32::Graphics::Gdi::GetDeviceCaps(hdc, 4) as u32; // HORZSIZE
            let h = windows_sys::Win32::Graphics::Gdi::GetDeviceCaps(hdc, 6) as u32; // VERTSIZE
            windows_sys::Win32::Graphics::Gdi::ReleaseDC(std::ptr::null_mut(), hdc);
            if w > 0 && h > 0 {
                return (w, h);
            }
        }
    }
    (527, 296)
}

#[cfg(target_os = "linux")]
fn auto_screen_mm() -> (u32, u32) {
    // 从 DRM 的 EDID 读取物理尺寸（字节 21/22，单位厘米），显示服务器无关。
    // 有效性看 EDID 头部 00 FF FF FF FF FF FF 00（早先误判成版本字节，导致一直用兜底值）
    let mut best: (u32, u32) = (0, 0);
    if let Ok(entries) = std::fs::read_dir("/sys/class/drm") {
        for e in entries.flatten() {
            let data = match std::fs::read(e.path().join("edid")) {
                Ok(d) => d,
                Err(_) => continue,
            };
            if data.len() < 128 || data[..8] != [0x00, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x00] {
                continue;
            }
            let w = data[21] as u32 * 10;
            let h = data[22] as u32 * 10;
            if w > 0 && h > 0 && w * h > best.0 * best.1 {
                best = (w, h);
            }
        }
    }
    if best.0 > 0 && best.1 > 0 {
        return best;
    }
    // 兜底：24" 16:9
    (527, 296)
}

/// 主显示器像素宽度：配置 > 自动检测 > 1920（GUI 与命令行共用，保证“米”的换算一致）
pub fn resolve_screen_px_w(cfg: &Config) -> u32 {
    if cfg.screen_px_w > 0 {
        return cfg.screen_px_w;
    }
    auto_screen_px_w().unwrap_or(1920)
}

/// 从 DRM 首选分辨率读取物理像素宽（与桌面缩放无关）
#[cfg(target_os = "linux")]
fn auto_screen_px_w() -> Option<u32> {
    let mut best: Option<u32> = None;
    let entries = std::fs::read_dir("/sys/class/drm").ok()?;
    for e in entries.flatten() {
        let dir = e.path();
        // 只看已连接输出的 modes（第一行是首选模式）
        if dir.join("modes").exists() {
            let connected = std::fs::read_to_string(dir.join("status")).map(|s| s.trim() == "connected").unwrap_or(false);
            if !connected {
                continue;
            }
            if let Ok(modes) = std::fs::read_to_string(dir.join("modes")) {
                if let Some(w) = modes.lines().next().and_then(|l| l.split_once('x')).and_then(|(w, _)| w.trim().parse::<u32>().ok()) {
                    best = Some(best.map_or(w, |b: u32| b.max(w)));
                }
            }
        }
    }
    best
}

#[cfg(windows)]
fn auto_screen_px_w() -> Option<u32> {
    unsafe {
        let w = windows_sys::Win32::UI::WindowsAndMessaging::GetSystemMetrics(windows_sys::Win32::UI::WindowsAndMessaging::SM_CXSCREEN);
        (w > 0).then_some(w as u32)
    }
}

#[cfg(all(not(windows), not(target_os = "linux")))]
fn auto_screen_px_w() -> Option<u32> {
    None
}

#[cfg(all(not(windows), not(target_os = "linux")))]
fn auto_screen_mm() -> (u32, u32) {
    (527, 296)
}

/// 单实例守护：Windows 命名互斥体，Linux flock 锁文件
fn acquire_single_instance() -> Result<(), String> {
    #[cfg(windows)]
    unsafe {
        use windows_sys::Win32::Foundation::{GetLastError, ERROR_ALREADY_EXISTS};
        use windows_sys::Win32::System::Threading::CreateMutexW;
        let name: Vec<u16> = "KMCounter-rs-SingleInstance\0".encode_utf16().collect();
        let handle = CreateMutexW(std::ptr::null(), 0, name.as_ptr());
        if handle.is_null() {
            return Err("创建单实例互斥体失败".into());
        }
        if GetLastError() == ERROR_ALREADY_EXISTS {
            return Err("另一个 KMCounter-rs 实例已在运行".into());
        }
        // 句柄故意不关闭（不调用 CloseHandle），随进程退出释放
        let _ = handle;
        Ok(())
    }
    #[cfg(target_os = "linux")]
    {
        use std::os::fd::AsRawFd;
        let dir = config::data_dir(None);
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("kmcounter.lock");
        let file = std::fs::OpenOptions::new().create(true).write(true).open(path).map_err(|e| format!("无法创建锁文件: {e}"))?;
        #[allow(deprecated)] // flock 简单够用
        match nix::fcntl::flock(file.as_raw_fd(), nix::fcntl::FlockArg::LockExclusiveNonblock) {
            Ok(_) => {
                // file 必须保持存活；泄漏之，进程退出自动释放
                std::mem::forget(file);
                Ok(())
            }
            Err(_) => Err("另一个 KMCounter-rs 实例已在运行".into()),
        }
    }
    #[cfg(not(any(windows, target_os = "linux")))]
    {
        Ok(())
    }
}
