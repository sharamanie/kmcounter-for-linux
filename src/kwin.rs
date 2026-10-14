//! KDE (KWin) 集成：Wayland 下 winit 无法隐藏窗口、也没有“跳过任务栏”协议，
//! 因此通过 KWin 窗口规则实现“只在托盘、不占任务栏/Alt-Tab”，
//! 并用 1x1 收缩代替隐藏（winit 在 Wayland 下 set_visible 是空实现）。
//!
//! 规则写入 `~/.config/kwinrulesrc`（只增删带标记的本程序规则，保留用户其他规则），
//! 改完通过 DBus 通知 KWin 重新加载。写文件前会备份为 kwinrulesrc.bak。

#![cfg(target_os = "linux")]

use std::path::PathBuf;

const MARKER: &str = "KMCounter-rs (auto)";
/// HUD 悬浮窗规则（置顶）单独一个标记，便于与主窗口规则分开维护
const HUD_MARKER: &str = "KMCounter-rs HUD (auto)";
const WM_CLASS: &str = "kmcounter";

/// 主窗口规则体：跳过任务栏 + 跳过 Alt-Tab
fn main_rule_body() -> String {
    format!(
        "skiptaskbar=true\nskiptaskbarrule=2\nskipswitcher=true\nskipswitcherrule=2\nwmclass={WM_CLASS}\nwmclassmatch=1\nwmclasscomplete=false\ntypes=1\n"
    )
}

/// HUD 悬浮窗规则体：**强制置顶** + 跳过任务栏/Alt-Tab。
///
/// 为什么必须靠 KWin 规则置顶：winit 的 Wayland 后端 `set_window_level` 是**空实现**
/// （`ViewportBuilder::with_always_on_top()` 在 Wayland 下完全无效），所以 HUD 的"置顶"
/// 只能由合成器侧的 KWin 规则（`above=true` 强制）来保证。
/// 用标题子串（`titlematch=2`）把 HUD（"KMC - APM" / "KMC - KPS"）与主窗口（"KMCounter-rs"）区分开。
fn hud_rule_body() -> String {
    format!(
        "above=true\naboverule=2\nskiptaskbar=true\nskiptaskbarrule=2\nskipswitcher=true\nskipswitcherrule=2\nwmclass={WM_CLASS}\nwmclassmatch=1\nwmclasscomplete=false\ntitle=KMC -\ntitlematch=2\ntypes=1\n"
    )
}

/// 是否 KDE 会话（只有 KWin 支持这套窗口规则）
pub fn is_kde() -> bool {
    std::env::var("XDG_CURRENT_DESKTOP")
        .map(|v| v.to_uppercase().contains("KDE"))
        .unwrap_or(false)
}

fn rules_path() -> PathBuf {
    dirs::config_dir().unwrap_or_else(|| PathBuf::from(".")).join("kwinrulesrc")
}

/// 启用/停用「跳过任务栏 + 跳过 Alt-Tab」规则（幂等）。
///
/// 同时维护两条规则：主窗口一条、HUD 悬浮窗一条（后者额外 `above=true` 强制置顶，
/// 因为 Wayland 下 winit 的 always_on_top 是空实现）。关掉时两条一起移除。
pub fn set_skip_taskbar(enabled: bool) {
    if !is_kde() {
        if enabled {
            log::info!("非 KDE 会话，跳过任务栏规则不适用（Windows/X11 可直接隐藏窗口）");
        }
        return;
    }
    let path = rules_path();
    let text = std::fs::read_to_string(&path).unwrap_or_default();
    let has_main = text.contains(MARKER);
    let has_hud = text.contains(HUD_MARKER);
    if enabled == (has_main && has_hud) {
        return; // 已是目标状态
    }
    let new_text = if enabled {
        // 逐条补齐：从旧版本升级过来时可能只有主窗口那条（HUD 置顶则缺失）
        let mut t = if has_main { text.clone() } else { add_rule(&text) };
        if !has_hud {
            t = add_hud_rule(&t);
        }
        t
    } else {
        remove_rule(&text)
    };
    if new_text == text {
        log::warn!("KWin 规则未发生变化，跳过写入");
        return;
    }
    // 首次修改前备份
    let bak = path.with_extension("bak");
    if path.exists() && !bak.exists() {
        let _ = std::fs::copy(&path, &bak);
    }
    if let Err(e) = std::fs::write(&path, &new_text) {
        log::error!("写入 KWin 规则失败: {e}");
        return;
    }
    match reconfigure() {
        Ok(_) => log::info!(
            "已{} KWin 规则（跳过任务栏/Alt-Tab{}）",
            if enabled { "启用" } else { "移除" },
            if enabled { " + HUD 置顶" } else { "" }
        ),
        Err(e) => log::warn!("已写入规则但通知 KWin 重载失败（重启或重新登录后生效）: {e}"),
    }
}

/// 让 KWin 重新加载配置
fn reconfigure() -> Result<(), String> {
    let conn = zbus::blocking::Connection::session().map_err(|e| e.to_string())?;
    conn.call_method(Some("org.kde.KWin"), "/KWin", Some("org.kde.KWin"), "reconfigure", &())
        .map_err(|e| e.to_string())?;
    Ok(())
}


fn kv_get(kv: &[(String, String)], key: &str) -> Option<String> {
    kv.iter().find(|(k, _)| k == key).map(|(_, v)| v.clone())
}

fn kv_set(kv: &mut Vec<(String, String)>, key: &str, val: String) {
    if let Some((_, v)) = kv.iter_mut().find(|(k, _)| k == key) {
        *v = val;
    } else {
        kv.push((key.to_string(), val));
    }
}

/// 在 INI 文本中追加本程序规则（若不存在）：主窗口（跳过任务栏 / Alt-Tab）
fn add_rule(text: &str) -> String {
    add_rule_with(text, MARKER, &main_rule_body())
}

/// 在 INI 文本中追加 HUD 悬浮窗规则（置顶 + 跳过任务栏 / Alt-Tab）
fn add_hud_rule(text: &str) -> String {
    add_rule_with(text, HUD_MARKER, &hud_rule_body())
}

/// 在 INI 文本中追加一条带标记的规则（自动分配新 id 并登记到 [General] rules）
fn add_rule_with(text: &str, marker: &str, body: &str) -> String {
    let (mut general, groups) = split_ini(text);
    let next_id = groups
        .iter()
        .filter_map(|(name, _)| name.parse::<u32>().ok())
        .max()
        .unwrap_or(0)
        + 1;
    let id = next_id.to_string();
    let mut rules: Vec<String> = kv_get(&general, "rules")
        .map(|v| v.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect())
        .unwrap_or_default();
    if !rules.iter().any(|r| r == &id) {
        rules.push(id.clone());
    }
    kv_set(&mut general, "count", rules.len().to_string());
    kv_set(&mut general, "rules", rules.join(","));

    let mut out = render_general(&general);
    for (name, group_body) in &groups {
        out.push_str(&format!("\n[{name}]\n{group_body}"));
    }
    out.push_str(&format!("\n[{id}]\nDescription={marker}\n{body}"));
    out
}

/// 从 INI 文本中删除本程序的两条规则（主窗口 + HUD，若存在），保留其他规则
fn remove_rule(text: &str) -> String {
    let (mut general, groups) = split_ini(text);
    let kept: Vec<(String, String)> = groups
        .iter()
        .filter(|(_, body)| !body.contains(MARKER) && !body.contains(HUD_MARKER))
        .cloned()
        .collect();
    if kept.len() == groups.len() {
        return text.to_string();
    }
    let ids: Vec<String> = kept.iter().map(|(n, _)| n.clone()).collect();
    let filtered: Vec<String> = kv_get(&general, "rules")
        .map(|v| v.split(',').map(|s| s.trim().to_string()).filter(|s| ids.contains(s)).collect())
        .unwrap_or_default();
    kv_set(&mut general, "count", filtered.len().to_string());
    kv_set(&mut general, "rules", filtered.join(","));

    let mut out = render_general(&general);
    for (name, body) in &kept {
        out.push_str(&format!("\n[{name}]\n{body}"));
    }
    out
}

/// 解析 INI：返回 ([General] 键值对, 其余分组(名, 原文体))
fn split_ini(text: &str) -> (Vec<(String, String)>, Vec<(String, String)>) {
    let mut general: Vec<(String, String)> = Vec::new();
    let mut groups: Vec<(String, String)> = Vec::new();
    let mut cur: Option<(String, String)> = None; // (name, body)
    for line in text.lines() {
        let t = line.trim();
        if t.starts_with('[') && t.ends_with(']') {
            if let Some((name, body)) = cur.take() {
                if name == "General" {
                    for l in body.lines() {
                        if let Some((k, v)) = l.split_once('=') {
                            general.push((k.trim().to_string(), v.trim().to_string()));
                        }
                    }
                } else {
                    groups.push((name, body));
                }
            }
            cur = Some((t[1..t.len() - 1].to_string(), String::new()));
        } else if let Some((_, body)) = cur.as_mut() {
            if !t.is_empty() {
                body.push_str(t);
                body.push('\n');
            }
        }
    }
    if let Some((name, body)) = cur.take() {
        if name == "General" {
            for l in body.lines() {
                if let Some((k, v)) = l.split_once('=') {
                    general.push((k.trim().to_string(), v.trim().to_string()));
                }
            }
        } else {
            groups.push((name, body));
        }
    }
    (general, groups)
}

/// 渲染 [General] 段（count/rules 在前，其余保留）
fn render_general(kv: &[(String, String)]) -> String {
    let mut out = String::from("[General]\n");
    for key in ["count", "rules"] {
        if let Some((_, v)) = kv.iter().find(|(k, _)| k == key) {
            out.push_str(&format!("{key}={v}\n"));
        }
    }
    for (k, v) in kv.iter().filter(|(k, _)| k != "count" && k != "rules") {
        out.push_str(&format!("{k}={v}\n"));
    }
    out
}


/// KWin 脚本里的「主窗口」判定：class 是本程序，且标题不是 HUD（我们的 HUD 标题以 "KMC - " 开头）。
///
/// **必须排除 HUD**：否则呼出主窗口时会把悬浮窗一起取消最小化 / 激活 / 改 keepAbove ——
/// 表现就是"焦点被 HUD 抢走"、"悬浮窗的置顶莫名消失"（踩过）。
const JS_MAIN_WINDOW: &str =
    r#"String(w.resourceClass).indexOf("kmcounter") >= 0 && String(w.caption).indexOf("KMC -") !== 0"#;

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

// ============================================================================
// HUD 位置记忆 / 限制在屏幕内（Wayland 专用通道）
//
// Wayland 协议没有全局坐标系：winit 既拿不到也设不了窗口位置（X11/Windows 都可以），
// 所以 KDE 上的「记忆位置 + 夹回屏幕」只能走 KWin 脚本：
// - 回读：常驻监听脚本在窗口几何变化时 callDBus 上报给本程序的 DBus 服务；
// - 恢复 / 夹回：同一个脚本在窗口出现时把 HUD 放回记忆位置、越界时夹回屏幕内。
// 非 KDE 的 Wayland（GNOME 等）没有等价通道，只能优雅降级（KDE/X11、Windows 走 winit）。
// ============================================================================

fn is_wayland() -> bool {
    std::env::var_os("WAYLAND_DISPLAY").is_some()
}

/// HUD 位置通道是否可用（仅 KDE + Wayland）
pub fn hud_pos_supported() -> bool {
    is_kde() && is_wayland()
}

/// KWin 脚本上报的最新 HUD 位置：[APM, KPS]（None = 还没上报过）
static HUD_POS_REPORTS: std::sync::Mutex<[Option<(f32, f32)>; 2]> = std::sync::Mutex::new([None, None]);

fn put_report(tag: &str, x: i32, y: i32) {
    let idx = match tag {
        "apm" => 0,
        "kps" => 1,
        _ => return,
    };
    if let Ok(mut g) = HUD_POS_REPORTS.lock() {
        g[idx] = Some((x as f32, y as f32));
    }
}

/// KWin 脚本上报的最新 HUD 位置（[APM, KPS]；没上过报就是 [None, None]）
pub fn hud_pos_reports() -> [Option<(f32, f32)>; 2] {
    HUD_POS_REPORTS.lock().map(|g| *g).unwrap_or([None, None])
}

struct HudPosIface;

/// 接收 KWin 监听脚本的位置上报（tag = "apm" | "kps"）
#[zbus::interface(name = "org.kmcounter.HudPos")]
impl HudPosIface {
    fn report(&self, tag: &str, x: i32, y: i32) {
        put_report(tag, x, y);
    }
}

/// 启动 HUD 位置上报的 DBus 服务（幂等；仅 KDE/Wayland 有意义）。
/// 连接存进 static 常驻进程；名字被占用（程序多开）时静默跳过。
pub fn start_hud_pos_service() {
    if !hud_pos_supported() {
        return;
    }
    static CONN: std::sync::OnceLock<Option<zbus::blocking::Connection>> = std::sync::OnceLock::new();
    CONN.get_or_init(|| {
        match zbus::blocking::connection::Builder::session()
            .and_then(|b| b.name("org.kmcounter.HudPos"))
            .and_then(|b| b.serve_at("/", HudPosIface))
            .and_then(|b| b.build())
        {
            Ok(conn) => {
                log::info!("HUD 位置上报服务已就绪（org.kmcounter.HudPos）");
                Some(conn)
            }
            Err(e) => {
                log::warn!("HUD 位置上报服务启动失败（KDE/Wayland 下位置记忆不可用）: {e}");
                None
            }
        }
    });
}

/// 生成 HUD 监听脚本（KWin JS）：恢复位置 / 夹回屏幕 / 上报位置三件事。
/// 开关与记忆位置直接编进脚本文本（改配置 → 整个脚本重载）。
fn hud_listener_script(
    apm_clamp: bool,
    kps_clamp: bool,
    apm_target: Option<(f32, f32)>,
    kps_target: Option<(f32, f32)>,
) -> String {
    let target = |t: Option<(f32, f32)>| match t {
        Some((x, y)) => format!("[{}, {}]", x, y),
        None => "null".to_string(),
    };
    format!(
        r#"var KMC_CLAMP = {{ "apm": {apm_clamp}, "kps": {kps_clamp} }};
var KMC_TARGET = {{ "apm": {apm_target}, "kps": {kps_target} }};
var KMC_PLACED = {{}};
function kmcTag(w) {{
    try {{
        if (String(w.resourceClass).indexOf("kmcounter") < 0) return null;
        var c = String(w.caption);
        if (c.indexOf("KMC - APM") === 0) return "apm";
        if (c.indexOf("KMC - KPS") === 0) return "kps";
    }} catch (e) {{}}
    return null;
}}
function kmcArea(w) {{
    try {{ if (w.output) return workspace.clientArea(KWin.PlacementArea, w.output, w.desktop); }} catch (e) {{}}
    try {{ return workspace.clientArea(KWin.PlacementArea, workspace.activeScreen, workspace.currentDesktop); }} catch (e) {{}}
    return null;
}}
function kmcMoveTo(w, x, y) {{
    var g = w.frameGeometry;
    try {{ w.frameGeometry = {{ x: x, y: y, width: g.width, height: g.height }}; return; }} catch (e) {{}}
    try {{ w.moveResize({{ x: x, y: y, width: g.width, height: g.height }}); }} catch (e) {{}}
}}
function kmcPlace(w, tag) {{
    if (KMC_PLACED[tag]) return;
    KMC_PLACED[tag] = true;
    var t = KMC_TARGET[tag];
    if (t) kmcMoveTo(w, t[0], t[1]);
}}
function kmcClamp(w, tag) {{
    if (!KMC_CLAMP[tag]) return;
    var a = kmcArea(w);
    if (!a) return;
    var g = w.frameGeometry;
    var x = Math.max(a.x, Math.min(g.x, a.x + a.width - g.width));
    var y = Math.max(a.y, Math.min(g.y, a.y + a.height - g.height));
    if (Math.abs(x - g.x) > 1 || Math.abs(y - g.y) > 1) kmcMoveTo(w, x, y);
}}
function kmcReport(w, tag) {{
    try {{
        var g = w.frameGeometry;
        callDBus("org.kmcounter.HudPos", "/", "org.kmcounter.HudPos", "Report", tag, Math.round(g.x), Math.round(g.y));
    }} catch (e) {{}}
}}
function kmcHook(w) {{
    var tag = kmcTag(w);
    if (!tag) return;
    kmcPlace(w, tag);
    function onChanged() {{ kmcClamp(w, tag); kmcReport(w, tag); }}
    try {{ w.frameGeometryChanged.connect(onChanged); }} catch (e) {{}}
    kmcClamp(w, tag);
    kmcReport(w, tag);
}}
try {{
    workspace.windowList().forEach(kmcHook);
    if (workspace.windowAdded && workspace.windowAdded.connect) workspace.windowAdded.connect(kmcHook);
}} catch (e) {{}}
"#,
        apm_clamp = apm_clamp,
        kps_clamp = kps_clamp,
        apm_target = target(apm_target),
        kps_target = target(kps_target),
    )
}

/// HUD 监听脚本的上一次加载状态（(脚本名, 临时文件), 参数签名）。
/// 签名没变就不重载（幂等）；变了就卸载旧的再加载新的。
static HUD_LISTENER: std::sync::Mutex<Option<((String, PathBuf), String)>> = std::sync::Mutex::new(None);

/// 同步 HUD 监听脚本（幂等）。**这个脚本要常驻**（订阅 windowAdded / frameGeometryChanged），
/// 所以不能走 [`run_script`]——那个会在下一次调用时把上一个脚本卸载掉。
pub fn sync_hud_listener(
    apm_clamp: bool,
    kps_clamp: bool,
    apm_target: Option<(f32, f32)>,
    kps_target: Option<(f32, f32)>,
) {
    if !hud_pos_supported() {
        return;
    }
    let sig = format!("{apm_clamp}|{kps_clamp}|{apm_target:?}|{kps_target:?}");
    let mut last = HUD_LISTENER.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((_, s)) = last.as_ref() {
        if *s == sig {
            return; // 参数没变
        }
    }
    let name = format!("kmc-hudpos-{}", std::process::id());
    let path = std::env::temp_dir().join(format!("{name}.js"));
    let body = hud_listener_script(apm_clamp, kps_clamp, apm_target, kps_target);
    if let Err(e) = std::fs::write(&path, body) {
        log::warn!("写入 HUD 监听脚本失败: {e}");
        return;
    }
    let res = (|| -> Result<(), String> {
        let conn = zbus::blocking::Connection::session().map_err(|e| e.to_string())?;
        // 卸载上一个（必然已执行完），避免重载时脚本越积越多
        if let Some(((prev_name, prev_path), _)) = last.take() {
            let _ = conn.call_method(
                Some("org.kde.KWin"),
                "/Scripting",
                Some("org.kde.kwin.Scripting"),
                "unloadScript",
                &(prev_name.as_str(),),
            );
            let _ = std::fs::remove_file(&prev_path);
        }
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
        Ok(())
    })();
    match res {
        Ok(_) => {
            log::info!("HUD 监听脚本已加载（位置记忆 + 限制在屏幕内）");
            *last = Some(((name, path), sig));
        }
        Err(e) => {
            log::warn!("HUD 监听脚本加载失败（KDE/Wayland 的位置功能不可用）: {e}");
            let _ = std::fs::remove_file(&path);
        }
    }
}

/// 卸载 HUD 监听脚本（程序退出时调用，避免 KWin 里留下僵尸脚本）
pub fn stop_hud_listener() {
    if !hud_pos_supported() {
        return;
    }
    let mut last = HUD_LISTENER.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(((name, path), _)) = last.take() {
        if let Ok(conn) = zbus::blocking::Connection::session() {
            let _ = conn.call_method(
                Some("org.kde.KWin"),
                "/Scripting",
                Some("org.kde.kwin.Scripting"),
                "unloadScript",
                &(name.as_str(),),
            );
        }
        let _ = std::fs::remove_file(&path);
        log::info!("HUD 监听脚本已卸载");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "[General]\ncount=2\nrules=1,7\n\n[1]\nDescription=Konsole 置顶\nabove=true\naboverule=2\nwmclass=konsole\n\n[7]\nDescription=别的规则\nopacityactive=90\n";

    #[test]
    fn add_keeps_others() {
        let out = add_rule(SAMPLE);
        assert!(out.contains(MARKER));
        assert!(out.contains("Description=Konsole 置顶"));
        assert!(out.contains("Description=别的规则"));
        assert!(out.contains("rules=1,7,8"), "rules 列表应追加新 id: {out}");
        assert!(out.contains("count=3"));
        assert!(out.contains("wmclass=kmcounter"));
        assert!(out.contains("skiptaskbarrule=2"));
        assert!(out.contains("skipswitcherrule=2"));
    }

    #[test]
    fn add_then_remove_restores() {
        let added = add_rule(SAMPLE);
        let removed = remove_rule(&added);
        assert!(!removed.contains(MARKER));
        assert!(removed.contains("Description=Konsole 置顶"));
        assert!(removed.contains("Description=别的规则"));
        assert!(removed.contains("rules=1,7"));
        assert!(removed.contains("count=2"));
        assert!(!removed.contains("[8]"));
    }

    #[test]
    fn remove_noop_without_rule() {
        assert_eq!(remove_rule(SAMPLE), SAMPLE);
    }

    #[test]
    fn empty_file_supported() {
        let out = add_rule("");
        assert!(out.contains("[General]"));
        assert!(out.contains("rules=1"));
        assert!(out.contains(MARKER));
    }

    /// HUD 规则：强制置顶（Wayland 下 winit 的 always_on_top 是空实现）+ 标题子串匹配；
    /// 主窗口规则不能被带上置顶；删除时两条规则一起走、别人的规则保留
    #[test]
    fn hud_rule_forces_above_and_is_removed_together() {
        let with_main = add_rule(SAMPLE);
        let both = add_hud_rule(&with_main);
        assert!(both.contains(HUD_MARKER), "应有 HUD 规则标记");
        assert!(both.contains("above=true"), "HUD 规则应置顶");
        assert!(both.contains("aboverule=2"), "置顶必须是强制规则（rule=2）");
        assert!(both.contains("title=KMC -"), "用标题子串把 HUD 与主窗口区分开");
        assert!(both.contains("titlematch=2"), "标题按子串匹配，覆盖 APM/KPS 两个 HUD");
        assert!(both.contains("rules=1,7,8,9"), "两条规则都要登记：{both}");

        let main_section = both.split("[8]").nth(1).unwrap().split("[9]").next().unwrap();
        assert!(main_section.contains("skiptaskbar=true"));
        assert!(!main_section.contains("above=true"), "主窗口规则不该强制置顶：{main_section}");

        let removed = remove_rule(&both);
        assert!(!removed.contains(MARKER) && !removed.contains(HUD_MARKER), "两条规则一起删除");
        assert!(removed.contains("Description=Konsole 置顶"), "别人的规则保留");
        assert!(removed.contains("rules=1,7"));
    }

    /// 呼出/收起脚本必须排除 HUD：否则会把悬浮窗一起激活、并清掉它的置顶
    #[test]
    fn main_window_scripts_exclude_huds() {
        assert!(JS_MAIN_WINDOW.contains("kmcounter"), "按窗口类识别本程序");
        assert!(JS_MAIN_WINDOW.contains("caption"), "再按标题排除 HUD");
        assert!(JS_MAIN_WINDOW.contains("KMC -"), "HUD 标题以 KMC - 开头，必须排除");
    }

    /// 手动运行：把两条 KWin 规则（主窗口跳过任务栏 + HUD 强制置顶）写进**真实**配置并让 KWin 重载，
    /// 同时打印最终文件内容，便于在真机上核对规则是否被 KWin 接受。仅在 KDE 会话下有效。
    ///
    /// `XDG_CURRENT_DESKTOP=KDE cargo test --release -- --ignored write_kde_rules_now --nocapture`
    #[test]
    #[ignore = "会写入真实的 ~/.config/kwinrulesrc"]
    fn write_kde_rules_now() {
        set_skip_taskbar(true);
        let text = std::fs::read_to_string(rules_path()).unwrap_or_default();
        println!("---- {}\n{text}", rules_path().display());
        assert!(text.contains(MARKER), "应写入主窗口规则");
        assert!(text.contains(HUD_MARKER), "应写入 HUD 置顶规则");
        assert!(text.contains("above=true"), "HUD 规则必须置顶");
    }

    /// HUD 监听脚本：恢复位置 / 夹回屏幕 / 上报三件事都要编进去
    #[test]
    fn hud_listener_script_has_place_clamp_report() {
        let js = hud_listener_script(true, false, Some((10.0, 20.0)), None);
        assert!(
            js.contains("\"apm\": true") && js.contains("\"kps\": false"),
            "开关要编进脚本：{js}"
        );
        assert!(js.contains("[10, 20]"), "APM 记忆位置要编进脚本：{js}");
        assert!(js.contains("\"kps\": null"), "没有记忆位置时为 null：{js}");
        assert!(js.contains("org.kmcounter.HudPos"), "要上报到我们的 DBus 服务：{js}");
        assert!(js.contains("frameGeometryChanged"), "要订阅几何变化：{js}");
        assert!(js.contains("windowAdded"), "要订阅新窗口：{js}");
        assert!(js.contains("KMC - APM") && js.contains("KMC - KPS"), "按标题区分两个 HUD：{js}");
        assert!(js.contains("KWin.PlacementArea"), "夹取范围用工作区：{js}");
        assert!(js.contains("kmcounter"), "只认本程序的窗口：{js}");
    }

    /// 位置上报的读写：tag 归位到 [APM, KPS]，未知 tag 忽略
    #[test]
    fn hud_pos_reports_put_and_read() {
        put_report("apm", 11, 22);
        put_report("kps", 33, 44);
        put_report("other", 99, 99); // 未知 tag：忽略
        let r = hud_pos_reports();
        assert_eq!(r[0], Some((11.0, 22.0)));
        assert_eq!(r[1], Some((33.0, 44.0)));
    }
}
