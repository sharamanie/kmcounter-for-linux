//! **常驻** KWin 监听脚本 + 它上报用的 DBus 服务（`org.kmcounter.HudPos`）。
//!
//! 这个脚本要一直活着（订阅 `windowAdded` / `frameGeometryChanged` / `minimizedChanged`），
//! 所以**不能**走 `script::run_script`（那个会在下次调用时卸载上一个），自己管加载/卸载。
//!
//! 脚本干三件事：
//! 1. **位置上报** —— HUD 几何变化时 `callDBus` 把位置报给本程序（Wayland 拿不到全局坐标，
//!    这是唯一的回读通道）；
//! 2. **恢复 / 夹回** —— 认领 HUD 后在时间窗内反复把它摆回记忆位置，越界时夹回屏幕内
//!    （用户一拖动就永久放行）；
//! 3. **主窗口最小化拦截**（`kmcHookMain`）—— 最小化 = unmap → eframe 事件循环停摆 →
//!    两个 HUD 一起冻结。所以一发现最小化就撤销它并改成「停放」，同时上报本程序。
// ============================================================================
// HUD 位置记忆 / 限制在屏幕内（Wayland 专用通道）
//
// Wayland 协议没有全局坐标系：winit 既拿不到也设不了窗口位置（X11/Windows 都可以），
// 所以 KDE 上的「记忆位置 + 夹回屏幕」只能走 KWin 脚本：
// - 回读：常驻监听脚本在窗口几何变化时 callDBus 上报给本程序的 DBus 服务；
// - 恢复 / 夹回：同一个脚本在窗口出现时把 HUD 放回记忆位置、越界时夹回屏幕内。
// 非 KDE 的 Wayland（GNOME 等）没有等价通道，只能优雅降级（KDE/X11、Windows 走 winit）。
// ============================================================================

use std::path::PathBuf;

use super::hud_pos_supported;

/// KWin 脚本上报的最新 HUD 位置：[APM, KPS]（None = 还没上报过）
/// KWin 脚本上报的 HUD 位置。
///
/// `user_moved` 是这个位置**是否来自用户拖动**（脚本侧的 `st.userMoved`）——
/// 程序只认它的位置来更新「记忆」，见 `gui::hud::tick_hud_pos` 里的说明。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HudPosReport {
    pub pos: (f32, f32),
    pub user_moved: bool,
}

static HUD_POS_REPORTS: std::sync::Mutex<[Option<HudPosReport>; 2]> = std::sync::Mutex::new([None, None]);

fn put_report(tag: &str, x: i32, y: i32, user_moved: bool) {
    let idx = match tag {
        "apm" => 0,
        "kps" => 1,
        _ => return,
    };
    if let Ok(mut g) = HUD_POS_REPORTS.lock() {
        g[idx] = Some(HudPosReport { pos: (x as f32, y as f32), user_moved });
    }
}

/// KWin 脚本上报的最新 HUD 位置（[APM, KPS]；没上过报就是 [None, None]）
pub fn hud_pos_reports() -> [Option<HudPosReport>; 2] {
    HUD_POS_REPORTS.lock().map(|g| *g).unwrap_or([None, None])
}

/// 主窗口被最小化的上报（KWin 脚本在拦截到最小化时置位；程序下一帧消费）
static MAIN_HIDDEN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);


/// 取走「主窗口被最小化过」的标记（消费式，取一次清一次）
pub fn take_main_hidden() -> bool {
    MAIN_HIDDEN.swap(false, std::sync::atomic::Ordering::Relaxed)
}

struct HudPosIface;

/// 接收 KWin 监听脚本的位置上报（tag = "apm" | "kps"）
#[zbus::interface(name = "org.kmcounter.HudPos")]
impl HudPosIface {
    /// `user_moved`：这个位置是不是用户拖出来的（1/0）—— 只有它对，程序才更新位置记忆
    fn report(&self, tag: &str, x: i32, y: i32, user_moved: i32) {
        put_report(tag, x, y, user_moved != 0);
    }

    /// KWin 脚本拦下主窗口的最小化（最小化 = unmap，会把事件循环和两个 HUD 一起冻住，
    /// 见 `hud_listener_script` 里的 `kmcHookMain`）—— 程序据此进入「已收起到托盘」状态
    fn main_hidden(&self) {
        MAIN_HIDDEN.store(true, std::sync::atomic::Ordering::Relaxed);
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
function kmcTag(w) {{
    try {{
        if (String(w.resourceClass).indexOf("kmcounter") < 0) return null;
        // **必须全等**：设置窗口的标题是 "KMC - KPS 设置"/"KMC - KPS Settings"（更长），
        // 之前用前缀匹配把它们也认成了 HUD —— 两个窗口被同一套 place/上报互相拉扯，
        // 设置窗口每次被合成器随机摆放的位置不断上报落盘，位置记忆全被污染（实测抓包定位）
        var c = String(w.caption);
        if (c === "KMC - APM") return "apm";
        if (c === "KMC - KPS") return "kps";
    }} catch (e) {{}}
    return null;
}}
function areaOfScreen(sc) {{
    // Plasma 6.7 的 clientArea 不收 Output/Screen 对象（实测报 "Could not convert argument 1
    // from KWin::LogicalOutput to const KWin::Window*"，而且每次调用都刷警告栈）——
    // 直接用屏幕的 raw geometry（含面板区域，对「防窗口丢失」的夹取足够）
    try {{ return sc.geometry; }} catch (e) {{}}
    return null;
}}
// 夹取范围：给了目标点 (tx,ty) 就找**包含它的屏幕**（恢复目标可能在另一块屏上 ——
// 双屏实测：记忆位在 DP-4 (915,0,2560x1440)，窗口初始落在 eDP-1 (0,0,914x1463)，
// 按窗口当前所在屏夹取会把窗口永远锁死在 eDP-1，永远恢复不到 DP-4 的记忆位置）；
// 不给目标点就用窗口当前所在的屏（拖动越界的即时夹取）。
function kmcArea(w, tx, ty) {{
    if (tx !== undefined) {{
        try {{
            var ss = workspace.screens;
            for (var i = 0; i < ss.length; i++) {{
                var g = ss[i].geometry;
                if (tx >= g.x && tx < g.x + g.width && ty >= g.y && ty < g.y + g.height) {{
                    var a = areaOfScreen(ss[i]);
                    if (a) return a;
                }}
            }}
        }} catch (e) {{}}
    }}
    try {{ return workspace.clientArea(KWin.PlacementArea, w); }} catch (e) {{}}
    try {{ if (w.output) return w.output.geometry; }} catch (e) {{}}
    return null;
}}
function kmcMoveTo(w, x, y) {{
    var g = w.frameGeometry;
    var r = null;
    try {{ r = Qt.rect(x, y, g.width, g.height); }} catch (e) {{}}
    if (!r) r = {{ x: x, y: y, width: g.width, height: g.height }};
    try {{ w.frameGeometry = r; }} catch (e) {{}}
    // QRectF 属性赋普通 JS 对象时 QJSEngine **静默失败**（不抛异常）——这正是旧版"位置记忆无效"
    // 的根源：赋值看着成功、实际没写入。所以赋完必须读回验证，没生效再走 moveResize 兜底。
    var h = w.frameGeometry;
    if (Math.abs(h.x - x) > 1 || Math.abs(h.y - y) > 1) {{
        try {{ w.moveResize(Qt.rect(x, y, g.width, g.height)); }} catch (e) {{}}
    }}
}}
function kmcAt(w, t) {{
    var g = w.frameGeometry;
    return Math.abs(g.x - t[0]) <= 1 && Math.abs(g.y - t[1]) <= 1;
}}
function kmcClamp(w, tag) {{
    if (!KMC_CLAMP[tag]) return;
    var a = kmcArea(w); // 拖动越界：按窗口当前所在的屏夹回
    if (!a) return;
    var g = w.frameGeometry;
    var x = Math.max(a.x, Math.min(g.x, a.x + a.width - g.width));
    var y = Math.max(a.y, Math.min(g.y, a.y + a.height - g.height));
    if (Math.abs(x - g.x) > 1 || Math.abs(y - g.y) > 1) kmcMoveTo(w, x, y);
}}
function kmcReport(w, tag, moved) {{
    try {{
        var g = w.frameGeometry;
        // 末位 = 这个位置是否来自用户拖动：**只有它=1 程序才会更新位置记忆**。
        // 合成器在窗口刚出现时的自动摆放也会走到这里，若不区分，一次没恢复上就会把
        // 错位当成新记忆（实测：反复重启会让 HUD 位置越走越远）。
        callDBus("org.kmcounter.HudPos", "/", "org.kmcounter.HudPos", "Report",
                 tag, Math.round(g.x), Math.round(g.y), moved ? 1 : 0);
    }} catch (e) {{}}
}}
function kmcIsOurs(w) {{
    try {{ return String(w.resourceClass).indexOf("kmcounter") >= 0; }} catch (e) {{}}
    return false;
}}
// Wayland 窗口刚创建（windowAdded）时标题常常还是空的 → 认不出是 HUD；而且合成器会在首次显示时再按自己的
// 策略摆一次位置，覆盖掉过早写入的位置（这正是“位置记忆无效”的原因）。
// 所以：标题变化时重新认领；认领后在前几次「非用户拖动」的几何变化里反复把窗口摆回记忆位置，直到摆准或用户动手拖动。
function kmcHook(w) {{
    if (!kmcIsOurs(w)) return;
    // 恢复策略（实测教训）：窗口初始化期的几何事件非常密集，合成器/程序自己会反复改尺寸和位置，
    // 按「次数」重试会在窗口定型前耗尽（200% 缩放的大窗口实测就是没恢复成功）——所以改成**时间窗**：
    // 认领后 8 秒内只要窗口没在记忆位置就反复拉回；期间**不上报**（否则会把合成器的默认摆放
    // 写进位置记忆）。用户一旦拖动（started 信号）立即永久放行。
    var st = {{ tag: null, dragging: false, userMoved: false, born: 0, lastWarn: 0, placing: false, lastPlace: 0, placeCount: 0 }};
    function target() {{ return st.tag ? KMC_TARGET[st.tag] : null; }}
    // 恢复窗 30 秒：窗口 map 初始化期间 moveTo 可能被 KWin 的摆放流程吞掉，而脚本没有
    // setTimeout（实测 undefined），重试只能靠几何事件驱动 —— 窗后 KPS/APM 的数字宽度
    // 变化会持续产生几何事件，拉长窗口让它们都有机会再试一次
    function restoring() {{
        var t = target();
        return !!(t && !st.userMoved && Date.now() - st.born < 30000);
    }}
    // 恢复目标先夹进屏幕：否则「限制在屏幕内」开着而记忆位置越界时，place（往记忆位拉）
    // 和 clamp（往屏内拉）会互相拉锯永不收敛（实测窗口反复横跳）
    function targetClamped() {{
        var t = target();
        if (!t) return null;
        var a = kmcArea(w, t[0], t[1]);
        if (!a) return t;
        var g = w.frameGeometry;
        return [Math.max(a.x, Math.min(t[0], a.x + a.width - g.width)),
                Math.max(a.y, Math.min(t[1], a.y + a.height - g.height))];
    }}
    // 防回路：place() 自己移窗口会触发 frameGeometryChanged → onChanged → place()。
    // 加上 KWin 在窗口 map 期间可能连续回调，曾在日志里看到
    // "RangeError: Maximum call stack size exceeded"。这里用 st.placing 防同步重入 + 200ms 窗口内最多 8 次防死循环，
    // 避免单次 80ms 节流把紧跟在 claim() 后的首次尺寸调整（InnerSize）事件吞掉。
    function place() {{
        if (!st.tag || st.dragging || st.userMoved) return;
        if (st.placing) return;
        try {{ if (w.move || w.resize) return; }} catch (e) {{}}
        var now = Date.now();
        if (now - st.lastPlace < 200) {{
            if (st.placeCount >= 8) return;
        }} else {{
            st.placeCount = 0;
        }}
        var t = targetClamped();
        if (!t || now - st.born > 30000 || kmcAt(w, t)) return;
        st.placing = true;
        st.lastPlace = now;
        st.placeCount = (st.placeCount || 0) + 1;
        try {{
            kmcMoveTo(w, t[0], t[1]);
        }} finally {{
            st.placing = false;
        }}
        // 诊断：moveTo 后仍未到位（节流 5s 一条）；配合程序侧 15s 落盘静默期，记忆不会损坏
        if (!kmcAt(w, t) && Date.now() - (st.lastWarn || 0) > 5000) {{
            st.lastWarn = Date.now();
            var g = w.frameGeometry;
            print("KMCPLACE miss " + st.tag + " want=" + Math.round(t[0]) + "," + Math.round(t[1])
                  + " got=" + Math.round(g.x) + "," + Math.round(g.y)
                  + " size=" + Math.round(g.width) + "x" + Math.round(g.height));
        }}
    }}
    function report() {{
        if (!st.tag) return;
        // 恢复期且窗口还没到恢复位置：不上报（这个位置不是用户的选择）
        var t = targetClamped();
        if (restoring() && (!t || !kmcAt(w, t))) return;
        kmcReport(w, st.tag, st.userMoved);
    }}
    function onChanged() {{
        if (!st.tag) return;
        var moving = st.dragging;
        try {{ moving = moving || !!(w.move || w.resize); }} catch (e) {{}}
        // 用户拖动期间合成器自己在逐帧改位置，此时写 frameGeometry 会被覆盖；
        // 所以拖动中只等「拖动结束」信号再夹取。非拖动的几何变化（程序自己改尺寸等）仍即时夹取。
        // **绝对不能在这里写 `st.dragging = true`**：`w.move || w.resize` 在程序化改尺寸或合成器初始化时
        // 不会触发 `interactiveMoveResizeFinished`，一旦置位 `st.dragging` 就会永久卡死为 true，
        // 导致后续 `place()` 与 `kmcClamp()` 永远失效。`st.dragging` 只由下方明确的交互开始/结束信号维护。
        if (!moving) {{ place(); kmcClamp(w, st.tag); }}
        report();
    }}
    function onFinished() {{ if (st.tag) {{ st.dragging = false; kmcClamp(w, st.tag); report(); }} }}
    function claim() {{
        if (st.tag) return;
        var tag = kmcTag(w);
        if (!tag) return;
        st.tag = tag;
        st.born = Date.now();
        KMC_RETRY.push(function () {{ place(); }});
        place();
        kmcClamp(w, tag);
        report();
    }}
    try {{ w.frameGeometryChanged.connect(onChanged); }} catch (e) {{}}
    try {{ if (w.bufferGeometryChanged) w.bufferGeometryChanged.connect(onChanged); }} catch (e) {{}}
    try {{ if (w.clientGeometryChanged) w.clientGeometryChanged.connect(onChanged); }} catch (e) {{}}
    // 拖动开始置位：恢复重试立即停手、此后的位置都是用户的选择（正常上报与记忆）
    // 这两个才是"用户拖动开始"的可靠信号（`w.move` 在合成器自己搬窗口时也为真，不能用）。
    // 打一行日志：位置记忆只被这里置过标记的位置更新，出问题时 journalctl 里一搜就知道
    // `if (!st.tag) return`：认领（标题匹配上）之前也可能收到这个信号（journal 里见过
    // KMCDRAG|null|...），此时还没确认是本程序的 HUD —— 置了标记会一直留到认领之后，
    // 让"合成器摆的位置"被当成用户拖动写进记忆。只有认领之后的拖动才算数。
    try {{ if (w.interactiveMoveResizeStarted) w.interactiveMoveResizeStarted.connect(function () {{ if (!st.tag) return; st.dragging = true; st.userMoved = true; print("KMCDRAG|" + st.tag + "|interactiveMoveResizeStarted"); }}); }} catch (e) {{}}
    try {{ if (w.clientStartUserMovedResized) w.clientStartUserMovedResized.connect(function () {{ if (!st.tag) return; st.dragging = true; st.userMoved = true; print("KMCDRAG|" + st.tag + "|clientStartUserMovedResized"); }}); }} catch (e) {{}}
    try {{ if (w.interactiveMoveResizeFinished) w.interactiveMoveResizeFinished.connect(onFinished); }} catch (e) {{}}
    try {{ if (w.clientFinishUserMovedResized) w.clientFinishUserMovedResized.connect(onFinished); }} catch (e) {{}}
    try {{ if (w.captionChanged) w.captionChanged.connect(claim); }} catch (e) {{}}
    try {{ if (w.windowShown) w.windowShown.connect(function () {{ claim(); place(); }}); }} catch (e) {{}}
    claim();
}}
// 主窗口「最小化」拦截：最小化 = unmap → eframe 的事件循环被 Wayland 掐断，两个 HUD 一起冻结
//（实测：最小化后心跳归零，取消最小化立刻恢复）。所以一发现主窗口被最小化，立刻撤销并改成
//「停放」：全透明 + 压到最底层、保持 mapped —— 视觉上同样是消失，但事件循环照常跑。
// 同时上报程序（程序会把窗口缩到 32px，之后托盘「统计/设置」或双击悬浮窗都能再叫回来）。
function kmcHookMain(w) {{
    try {{
        if (String(w.resourceClass).indexOf("kmcounter") < 0) return;
        if (String(w.caption).indexOf("KMC -") === 0) return; // HUD 不算主窗口
        if (!w.minimizedChanged) return;
        w.minimizedChanged.connect(function () {{
            if (!w.minimized) return;
            print("KMCMIN|主窗口被最小化 -> 转为停放");
            w.minimized = false;
            w.keepBelow = true;
            try {{ w.opacity = 0.0; }} catch (e) {{}}
            try {{ callDBus("org.kmcounter.HudPos", "/", "org.kmcounter.HudPos", "MainHidden"); }} catch (e) {{}}
        }});
    }} catch (e) {{}}
}}
// 工作区级事件兜底：窗口内容不变时自己的几何事件会断流（KPS 数字宽度恰好不变时
// 恢复会静默失败，实测多轮复现），用户切窗口 / 换屏 / 改显示器时驱动所有已认领窗口重试
var KMC_RETRY = [];
try {{ workspace.windowActivated.connect(function () {{ KMC_RETRY.forEach(function (f) {{ f(); }}); }}); }} catch (e) {{}}
try {{ workspace.screensChanged.connect(function () {{ KMC_RETRY.forEach(function (f) {{ f(); }}); }}); }} catch (e) {{}}
try {{ workspace.currentDesktopChanged.connect(function () {{ KMC_RETRY.forEach(function (f) {{ f(); }}); }}); }} catch (e) {{}}
function kmcHookAll(w) {{ kmcHook(w); kmcHookMain(w); }}
try {{
    workspace.windowList().forEach(kmcHookAll);
    if (workspace.windowAdded && workspace.windowAdded.connect) workspace.windowAdded.connect(kmcHookAll);
}} catch (e) {{}}
"#,
        apm_clamp = apm_clamp,
        kps_clamp = kps_clamp,
        apm_target = target(apm_target),
        kps_target = target(kps_target),
    )
}

/// HUD 监听脚本的上一次加载状态（(脚本名, 临时文件), 参数签名）。

static HUD_LISTENER: std::sync::Mutex<Option<((String, PathBuf), String)>> = std::sync::Mutex::new(None);

/// 单个 HUD 位置规则的规则体：`positionrule=3`（Apply Initially）= 窗口出现时应用一次，
/// 之后用户可自由拖动 —— 正是"恢复到记忆位置"的语义（2=Force 会锁死位置，1=DontAffect 不生效）。

/// 清理历史遗留的孤儿监听脚本：进程异常死亡（被杀 / 崩溃）时 `stop_hud_listener` 没跑，
/// KWin 里会留下还在运行的旧脚本（实测曾同时挂 3 个版本互相打架、其中一个还在刷错误日志）。
/// KWin 的 Scripting 接口没有"列出已加载脚本"的方法，但我们的脚本文件都在 /tmp 且
/// 名字即文件名（`kmc-hudpos` / 旧版的 `kmc-hudpos-<pid>`），按文件名反查逐个卸载即可。
fn cleanup_orphan_scripts(conn: &zbus::blocking::Connection) {
    let Ok(entries) = std::fs::read_dir(std::env::temp_dir()) else {
        return;
    };
    for e in entries.flatten() {
        let fname = e.file_name();
        let Some(fname) = fname.to_str() else { continue };
        let Some(stem) = fname
            .strip_prefix("kmc-hudpos")
            .and_then(|s| s.strip_suffix(".js"))
        else {
            continue;
        };
        // 只认脚本文件：kmc-hudpos.js（现版固定名）与 kmc-hudpos-<pid>.js（旧版）。
        // 必须限定 .js 结尾 —— /tmp 里同前缀的其它文件（日志等）不能误删（实测教训）
        if !(stem.is_empty() || stem.starts_with('-')) {
            continue;
        }
        let plugin_name = fname.trim_end_matches(".js");
        let _ = conn.call_method(
            Some("org.kde.KWin"),
            "/Scripting",
            Some("org.kde.kwin.Scripting"),
            "unloadScript",
            &(plugin_name,),
        );
        let _ = conn.call_method(
            Some("org.kde.KWin"),
            "/Scripting",
            Some("org.kde.kwin.Scripting"),
            "unloadScript",
            &(fname,),
        );
        let _ = std::fs::remove_file(std::env::temp_dir().join(fname));
        log::info!("已清理孤儿 HUD 监听脚本: {fname}");
    }
}

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
    // 名字固定（不带 pid）：加载前先无条件 unload 同名脚本 —— 进程异常死亡时 stop_hud_listener
    // 没跑，会留下孤儿脚本（实测 KWin 里曾同时挂着 3 个版本互相打架）
    let name = "kmc-hudpos".to_string();
    let path = std::env::temp_dir().join(format!("{name}.js"));
    let body = hud_listener_script(apm_clamp, kps_clamp, apm_target, kps_target);
    let res = (|| -> Result<(), String> {
        let conn = zbus::blocking::Connection::session().map_err(|e| e.to_string())?;
        // 必须**先**清理旧脚本并删旧文件，**再**把新脚本写进 path！
        // 之前先写 path 再走 last.take() 的 remove_file(prev_path)，因为 path 固定为 /tmp/kmc-hudpos.js，
        // 重载时直接把刚写好的新脚本文件删掉了，导致 loadScript 读不到文件、监听脚本彻底阵亡。
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
        cleanup_orphan_scripts(&conn);
        let _ = conn.call_method(
            Some("org.kde.KWin"),
            "/Scripting",
            Some("org.kde.kwin.Scripting"),
            "unloadScript",
            &(name.as_str(),),
        );
        std::fs::write(&path, &body).map_err(|e| format!("写入 HUD 监听脚本失败: {e}"))?;
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
        assert!(
            js.contains("interactiveMoveResizeFinished"),
            "拖动结束要再夹取一次（拖动中合成器会覆盖脚本写入的位置）：{js}"
        );
        assert!(js.contains("windowAdded"), "要订阅新窗口：{js}");
        assert!(
            js.contains("c === \"KMC - APM\"") && js.contains("c === \"KMC - KPS\""),
            "标题必须全等匹配（前缀匹配会把设置窗口也认成 HUD，实测污染位置记忆）：{js}"
        );
        assert!(js.contains("KWin.PlacementArea"), "夹取范围用工作区：{js}");
        assert!(js.contains("kmcounter"), "只认本程序的窗口：{js}");
        assert!(
            js.contains("Qt.rect"),
            "移动窗口必须用 Qt.rect 构造 QRectF（普通对象赋值在 Plasma 6 静默失败）：{js}"
        );
        assert!(js.contains("moveResize"), "frameGeometry 赋值要有 moveResize 兜底：{js}");
        assert!(
            js.contains("interactiveMoveResizeStarted"),
            "拖动开始要置 dragging 标志（防拖动中被拉回记忆位置）：{js}"
        );
        assert!(js.contains("w.output.geometry"), "夹取面积要有 output.geometry 兜底：{js}");
        assert!(
            js.contains("workspace.screens") && js.contains("kmcArea(w, t[0], t[1])"),
            "恢复要按目标点所在屏夹取（双屏实测：按窗口当前屏夹会把窗口锁死在另一块屏）：{js}"
        );
        // 最小化 = unmap → 事件循环和两个 HUD 一起冻结。必须拦下最小化改成「停放」
        assert!(
            js.contains("minimizedChanged") && js.contains("kmcHookMain"),
            "要拦主窗口的最小化（否则一最小化两个 HUD 全冻）：{js}"
        );
        assert!(
            js.contains("\"MainHidden\"") || js.contains("MainHidden"),
            "拦下最小化后要上报程序（程序才知道自己已收起）：{js}"
        );
        assert!(
            js.contains("windowAdded.connect(kmcHookAll)"),
            "新窗口要同时挂 HUD 与主窗口两套钩子：{js}"
        );
        // 位置记忆只能被"用户真的拖过"的位置更新（否则合成器的自动摆位会污染记忆）
        assert!(
            js.contains("kmcReport(w, st.tag, st.userMoved)") && js.contains("moved ? 1 : 0"),
            "上报要带「用户是否拖过」标记：{js}"
        );
        assert!(
            !js.contains("if (moving) { st.dragging = true; }"),
            "onChanged 绝不能锁死 st.dragging = true（否则初始化改尺寸后 place/clamp 永久失效）：{js}"
        );
        assert!(
            js.contains("if (!moving) { place(); kmcClamp(w, st.tag); }"),
            "非拖动状态下应调用 place() 与 kmcClamp()：{js}"
        );
        assert!(
            js.contains("bufferGeometryChanged") && js.contains("clientGeometryChanged"),
            "应同时订阅 bufferGeometryChanged 与 clientGeometryChanged：{js}"
        );
    }

    /// 位置上报的读写：tag 归位到 [APM, KPS]，未知 tag 忽略
    #[test]
    fn hud_pos_reports_put_and_read() {
        put_report("apm", 11, 22, true);
        put_report("kps", 33, 44, false);
        put_report("other", 99, 99, true); // 未知 tag：忽略
        let r = hud_pos_reports();
        assert_eq!(r[0], Some(HudPosReport { pos: (11.0, 22.0), user_moved: true }));
        assert_eq!(r[1], Some(HudPosReport { pos: (33.0, 44.0), user_moved: false }));
    }
}
