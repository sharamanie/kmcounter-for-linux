//! 一次性 **KWin 脚本**：把一段 JS 写进临时文件，交给 KWin 的 `Scripting` 接口执行。
//!
//! 全部是「改窗口状态」这类 winit 在 Wayland 下做不到的事：
//! 停放主窗口（透明 + 压底层 + 撤销最小化）、解除停放、取消最小化并激活、清掉置顶。
//!
//! 注意：**改不了几何**（见 `kwin` 模块头部），所以这些脚本只碰
//! `minimized / opacity / keepAbove / keepBelow / activeWindow`。

use super::is_kde;
/// KWin 脚本里的「主窗口」判定：class 是本程序，且标题不是 HUD（我们的 HUD 标题以 "KMC - " 开头）。
///
/// **必须排除 HUD**：否则呼出主窗口时会把悬浮窗一起取消最小化 / 激活 / 改 keepAbove ——
/// 表现就是"焦点被 HUD 抢走"、"悬浮窗的置顶莫名消失"（踩过）。
const JS_MAIN_WINDOW: &str =
    r#"String(w.resourceClass).indexOf("kmcounter") >= 0 && String(w.caption).indexOf("KMC -") !== 0"#;

// 停放时缩到多大的方块是渲染循环的事（`gui::PARK_SIZE` + `ViewportCommand::InnerSize`），
// 不放这里：`kwin` 模块带 `#![cfg(target_os = "linux")]`，放进来 Windows 就编译不过。
// 本脚本只负责透明度和窗口层级 —— 脚本里改几何要靠 `Qt.rect` 构造 QRectF，
// 而 KWin 6 的脚本环境里 `Qt` 根本不存在（实测 `ReferenceError: Qt is not defined`），
// `moveResize` 也已被移除，普通对象赋值则是静默失败。

/// 主窗口「停放」：收起到托盘时**不能** unmap（最小化 / `Visible(false)`）。
///
/// 实测：Wayland 下主窗口一 unmap，eframe 的事件循环就再也不被唤醒（连
/// `request_redraw` 都不再发出），所有视口一起停摆 —— 两个 HUD 全部冻结
///（HUD 自己是可见的、winit 的 request_redraw 也不依赖帧回调，照样没用）。
///
/// 所以收起时把主窗口缩成屏幕角落的一小块并保持 mapped：全透明 + 极小尺寸，
/// 视觉上等同于消失，事件循环照常跑，HUD 继续更新。
pub fn park_main_window() -> bool {
    if !is_kde() {
        return false;
    }
    let js = format!(
        r#"
function run() {{
    var wins = (typeof workspace.windowList === "function") ? workspace.windowList() : workspace.clientList();
    for (var i = 0; i < wins.length; i++) {{
        var w = wins[i];
        if ({cond}) {{
            w.minimized = false;
            w.keepAbove = false;
            w.keepBelow = true;
            // 只管透明度和层级：尺寸由渲染循环下发 InnerSize（egui 命令可靠，且位置天然不变）
            try {{ w.opacity = 0.0; }} catch (e) {{}}
            print("KMC-PARK|opacity=" + w.opacity);
        }}
    }}
}}
run();
"#,
        cond = JS_MAIN_WINDOW
    );
    let ok = run_script("kmc-park", &js);
    if ok {
        log::info!("主窗口已停放（保持 mapped + 全透明并压到最底层，尺寸由渲染循环压到最小 —— HUD 继续更新）");
    }
    ok
}

/// 从停放恢复：不透明 + 回到正常层级 + 激活
///（尺寸由渲染循环的自适应逻辑下发，见 `App::update` 里的「停放恢复后强制下发尺寸」）。
pub fn unpark_main_window(keep_on_top: bool) -> bool {
    if !is_kde() {
        return false;
    }
    let js = format!(
        r#"
function run() {{
    var wins = (typeof workspace.windowList === "function") ? workspace.windowList() : workspace.clientList();
    for (var i = 0; i < wins.length; i++) {{
        var w = wins[i];
        if ({cond}) {{
            w.minimized = false;
            w.keepBelow = false;
            try {{ w.opacity = 1.0; }} catch (e) {{}}
            w.keepAbove = {keep};
            workspace.activeWindow = w;
        }}
    }}
}}
run();
"#,
        cond = JS_MAIN_WINDOW,
        keep = if keep_on_top { "true" } else { "false" }
    );
    let ok = run_script("kmc-unpark", &js);
    if ok {
        log::info!("主窗口已从停放恢复（不透明 + 回到正常层级）");
    }
    ok
}

/// 显示本程序窗口：取消最小化并激活（KDE/Wayland 下 winit 无法做到，交给 KWin 脚本）。
///
/// winit 在 Wayland 下无法程序化恢复最小化的窗口（focus_window / set_minimized(false) 均为空实现），
/// 而 KWin 脚本可以做到（同一个脚本里先取消最小化再赋值 activeWindow 即可，实测可靠）。
/// 非 KDE 会话返回 false。
pub fn unminimize_activate(keep_on_top: bool) -> bool {
    if !is_kde() {
        return false;
    }
    // 取消最小化 + 激活；keep_on_top 时额外置顶（激活是否被接受由 KWin 决定，不稳定，置顶可靠）
    let show = format!(
        r#"
function run() {{
    var wins = (typeof workspace.windowList === "function") ? workspace.windowList() : workspace.clientList();
    for (var i = 0; i < wins.length; i++) {{
        var w = wins[i];
        if ({cond}) {{
            w.minimized = false;
            w.keepAbove = {keep};
            workspace.activeWindow = w;
        }}
    }}
}}
run();
"#,
        cond = JS_MAIN_WINDOW,
        keep = if keep_on_top { "true" } else { "false" }
    );
    let act = format!(
        r#"
function run() {{
    var wins = (typeof workspace.windowList === "function") ? workspace.windowList() : workspace.clientList();
    for (var i = 0; i < wins.length; i++) {{
        var w = wins[i];
        if ({cond}) {{ workspace.activeWindow = w; }}
    }}
}}
run();
"#,
        cond = JS_MAIN_WINDOW
    );
    // KWin 是否接受脚本激活并不稳定（实测同一脚本时好时坏），多试几次提高成功率；
    // 窗口在第一个脚本执行时就已经取消最小化，后面的重试只影响"激活"这一步，所以间隔取小一点
    let ok = run_script("kmc-show", &show);
    for _ in 0..3 {
        std::thread::sleep(std::time::Duration::from_millis(150));
        run_script("kmc-act", &act);
    }
    if ok {
        log::info!("已通过 KWin 脚本显示窗口（取消最小化 + 激活{}）", if keep_on_top { " + 置顶" } else { "" });
    }
    ok
}

/// 收起窗口时清除置顶（避免窗口一直压在其他窗口上）。
/// **只清主窗口**：HUD 的置顶由 KWin 规则强制，跟着清掉会让悬浮窗失去置顶（踩过）。
pub fn clear_keep_above() {
    if !is_kde() {
        return;
    }
    let script = format!(
        r#"
function run() {{
    var wins = (typeof workspace.windowList === "function") ? workspace.windowList() : workspace.clientList();
    for (var i = 0; i < wins.length; i++) {{
        var w = wins[i];
        if ({cond}) {{ w.keepAbove = false; }}
    }}
}}
run();
"#,
        cond = JS_MAIN_WINDOW
    );
    run_script("kmc-unkeep", &script);
}

/// 启动补刀：把「跳过任务栏 / Alt-Tab」直接设到本程序的所有窗口上（HUD 按配置钉置顶）。
///
/// 为什么规则之外还需要它：KWin 规则在窗口 map 时套用，但启动时「写规则 + reconfigure」
/// 与「eframe 创建窗口」并发 —— 窗口若赶在 KWin 重载完成前 map，这条规则就漏套了，
/// 表现是任务栏从此常驻本程序条目（还有个副作用：任务栏条目找不到对应 .desktop 时，
/// 会回退显示 exe 完整路径，中文目录被按 Latin-1 渲染成乱码）。规则照留（跨会话兜底），
/// 这里在窗口起来后把属性直接钉上去，不依赖 reconfigure 时序。幂等，重复执行无害。
pub fn enforce_skip_taskbar(skip: bool, hud_above: bool) {
    if !is_kde() {
        return;
    }
    let ok = run_script("kmc-enforce", &enforce_js(skip, hud_above));
    if ok {
        log::debug!(
            "已直接{}本程序窗口的「跳过任务栏/Alt-Tab」（HUD 置顶: {}）",
            if skip { "钉上" } else { "撤下" },
            hud_above
        );
    }
}

/// 运行时直接向正在运行的 HUD 窗口应用置顶属性（keepAbove）。
pub fn apply_hud_above(above: bool) {
    if !is_kde() {
        return;
    }
    let js = format!(
        r#"
function run() {{
    var wins = (typeof workspace.windowList === "function") ? workspace.windowList() : workspace.clientList();
    for (var i = 0; i < wins.length; i++) {{
        var w = wins[i];
        if (String(w.resourceClass).indexOf("kmcounter") >= 0) {{
            if (String(w.caption).indexOf("KMC -") === 0) {{
                w.keepAbove = {above};
            }}
        }}
    }}
}}
run();
"#
    );
    run_script("kmc-hud-above", &js);
}

/// 生成补刀脚本（抽出来便于测试断言内容）：
/// 主窗口和 HUD（标题 "KMC - " 前缀）都设 skipTaskbar/skipSwitcher；
/// HUD 的置顶根据配置钉上或撤下。
fn enforce_js(skip: bool, hud_above: bool) -> String {
    format!(
        r#"
function run() {{
    var wins = (typeof workspace.windowList === "function") ? workspace.windowList() : workspace.clientList();
    for (var i = 0; i < wins.length; i++) {{
        var w = wins[i];
        if (String(w.resourceClass).indexOf("kmcounter") >= 0) {{
            w.skipTaskbar = {skip};
            w.skipSwitcher = {skip};
            if (String(w.caption).indexOf("KMC -") === 0) {{ w.keepAbove = {hud_above}; }}
            print("KMC-ENFORCE|" + String(w.caption).substring(0, 24) + "|skipTB=" + w.skipTaskbar + "|above=" + w.keepAbove);
        }}
    }}
}}
run();
"#
    )
}

/// 把一段 KWin 脚本写入临时文件并让 KWin 加载执行。
///
/// 两个 KWin 脚本接口的坑（都踩过并已修复）：
/// 1. **脚本名必须每次唯一**：Scripting.start() 只执行「尚未启动过」的脚本，同名第二次调用会静默跳过；
/// 2. **不要卸载刚加载的脚本**：KWin 的执行是异步的，立即卸载有竞态（会把还没执行的脚本取消掉），
///    因此这里只卸载「上一次」的脚本（此时它必然已执行完），避免脚本对象累积。
fn run_script(tag: &str, body: &str) -> bool {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    let name = format!("{tag}-{}-{}", std::process::id(), seq);
    let path = std::env::temp_dir().join(format!("{name}.js"));
    if let Err(e) = std::fs::write(&path, body) {
        log::warn!("写入 KWin 脚本失败: {e}");
        return false;
    }
    let res = (|| -> Result<(), String> {
        let conn = zbus::blocking::Connection::session().map_err(|e| e.to_string())?;
        let p = path.to_string_lossy().to_string();
        conn.call_method(
            Some("org.kde.KWin"),
            "/Scripting",
            Some("org.kde.kwin.Scripting"),
            "loadScript",
            &(p.as_str(), name.as_str()),
        )
        .map_err(|e| e.to_string())?;
        conn.call_method(Some("org.kde.KWin"), "/Scripting", Some("org.kde.kwin.Scripting"), "start", &())
            .map_err(|e| e.to_string())?;
        static PREV: std::sync::Mutex<Option<(String, std::path::PathBuf)>> = std::sync::Mutex::new(None);
        if let Some((prev_name, prev_path)) = PREV.lock().unwrap().replace((name.clone(), path.clone())) {
            let _ = conn.call_method(
                Some("org.kde.KWin"),
                "/Scripting",
                Some("org.kde.kwin.Scripting"),
                "unloadScript",
                &(prev_name.as_str(),),
            );
            // 上一次的脚本已卸载（必然执行完了），临时文件一起清掉，避免反复呼出窗口持续堆积
            let _ = std::fs::remove_file(&prev_path);
        }
        Ok(())
    })();
    match res {
        Ok(_) => true,
        Err(e) => {
            log::warn!("KWin 脚本({tag})执行失败: {e}");
            false
        }
    }
}


#[cfg(test)]
mod tests {
    use super::*;

#[test]
fn main_window_scripts_exclude_huds() {
    assert!(JS_MAIN_WINDOW.contains("kmcounter"), "按窗口类识别本程序");
    assert!(JS_MAIN_WINDOW.contains("caption"), "再按标题排除 HUD");
    assert!(JS_MAIN_WINDOW.contains("KMC -"), "HUD 标题以 KMC - 开头，必须排除");
}

/// 补刀脚本：主窗口和 HUD 都要钉 skipTaskbar/skipSwitcher，HUD 额外钉置顶；
/// 关闭时对称地全部撤下（先开后关时清掉残留）。不能碰 minimized/opacity 等其他状态。
#[test]
fn enforce_script_pins_all_windows_symmetrically() {
    let on = enforce_js(true, true);
    assert!(on.contains(r#"w.skipTaskbar = true"#), "主窗口/HUD 都要钉上: {on}");
    assert!(on.contains(r#"w.skipSwitcher = true"#), "Alt-Tab 也要跳过: {on}");
    assert!(on.contains(r#"w.keepAbove = true"#), "HUD 置顶一起钉（同一条规则，竞态会一起漏）: {on}");
    assert!(on.contains(r#"indexOf("KMC -") === 0"#), "置顶只钉 HUD，主窗口不吃置顶");
    assert!(!on.contains("minimized"), "不能碰最小化状态");
    assert!(!on.contains("opacity"), "不能碰透明度");

    let on_no_above = enforce_js(true, false);
    assert!(on_no_above.contains(r#"w.skipTaskbar = true"#));
    assert!(on_no_above.contains(r#"w.keepAbove = false"#), "HUD 置顶关时即使跳任务栏开，keepAbove 也是 false: {on_no_above}");

    let off = enforce_js(false, false);
    assert!(off.contains(r#"w.skipTaskbar = false"#), "关闭时对称撤下: {off}");
    assert!(off.contains(r#"w.keepAbove = false"#), "HUD 置顶也撤下: {off}");
}
}
