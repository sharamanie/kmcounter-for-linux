//! 主界面：键盘热力图 + 统计列表 + 历史翻页 + 趋势图表 + 设置窗口。
//! 配色复刻原版：浅色统计窗（背景 #EEEEEE、文字 #575757），
//! 热力渐变 #EEEEEE → #B26C65（莫兰迪红），深色设置窗（#444444）。

use crate::chart;
use crate::config::Config;
use crate::keys::{
    layout_profile, HourStripPlacement, LayoutKey, LayoutProfile, KEYS,
};
use crate::lang::{strings, Lang, Strings};
use crate::stats::{BucketStats, DayEntry, HourPoint, MouseStats, Shared, Store};
use crate::tray::Tray;
use crate::trend::{self, Granularity};
use chrono::{Datelike, NaiveDate, Timelike};
use eframe::egui;
use egui::{Align2, Color32, FontId, Pos2, Rect, Sense, Shape, Vec2};
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};

pub(crate) mod hud;
pub(crate) use hud::*;
/// 主窗口的显示 / 隐藏 / 停放（三平台语义 + Wayland 的「不能 unmap」约束都收在这里）
pub(crate) mod window;
pub(crate) use window::*;

pub(crate) mod debug;
pub(crate) mod frame;
pub(crate) mod heatmap;
pub(crate) mod nav;
pub(crate) mod rain;
pub(crate) mod rank_page;
pub(crate) mod settings;
pub(crate) mod trend_page;

pub(crate) const BG: Color32 = Color32::from_rgb(0xEE, 0xEE, 0xEE);
const TEXT: Color32 = Color32::from_rgb(0x57, 0x57, 0x57);
const HEAT_FROM: (u8, u8, u8) = (0xEE, 0xEE, 0xEE);
const HEAT_TO: (u8, u8, u8) = (0xB2, 0x6C, 0x65);
/// 趋势曲线配色：键盘（蓝灰）、鼠标移动距离（莫兰迪红）
/// 三条曲线的配色：键盘敲击=红系，鼠标按键=绿系，鼠标移动=蓝系
/// （键盘热力图与「键盘敲击」曲线同属红系，鼠标五键与「鼠标按键」曲线同属绿系）
pub(crate) const KEY_COLOR: Color32 = Color32::from_rgb(0xB2, 0x6C, 0x65); // 键盘：红系
pub(crate) const CLICK_COLOR: Color32 = Color32::from_rgb(0x6F, 0x8F, 0x62); // 鼠标按键：绿系
pub(crate) const MOUSE_COLOR: Color32 = Color32::from_rgb(0x6B, 0x8B, 0xA4); // 鼠标移动：蓝系
/// 环比涨跌配色（莫兰迪绿/红）
const DELTA_UP: Color32 = Color32::from_rgb(0x7F, 0xA3, 0x7A);
const DELTA_DOWN: Color32 = Color32::from_rgb(0xC0, 0x77, 0x6E);
const DELTA_FLAT: Color32 = Color32::from_rgb(0x99, 0x99, 0x99);
/// 被点选按键的强调色
const SEL_COLOR: Color32 = Color32::from_rgb(0x3F, 0x6B, 0xA8);

#[derive(Clone, Copy, PartialEq, Eq)]
enum Page {
    Heatmap,
    Trends,
    /// 排行：某区间内各按键的排行榜
    Ranking,
}

/// 排行页的统计区间（锚定日期 = 导航行选中的那天；全部 = 从最早的数据日到锚定日期）
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum RankRange {
    Day,
    Week,
    Month,
    Year,
    All,
}

/// 排行页的键位范围筛选（可多选：例如同时看主键区 + 控制键区）
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub(crate) enum RankScope {
    Main,
    Function,
    Control,
    Numpad,
    /// 鼠标五键（左/中/右/滚轮/侧键）
    Mouse,
    /// 布局之外的按键（多媒体键等）
    Other,
}

/// 全部键位范围（默认状态）
pub(crate) const ALL_SCOPES: [RankScope; 6] = [
    RankScope::Main,
    RankScope::Function,
    RankScope::Control,
    RankScope::Numpad,
    RankScope::Mouse,
    RankScope::Other,
];

/// 布局键的分区 → 排行页的键位范围
fn scope_of_zone(z: crate::keys::Zone) -> RankScope {
    use crate::keys::Zone;
    match z {
        Zone::Main => RankScope::Main,
        Zone::Function => RankScope::Function,
        Zone::Control => RankScope::Control,
        Zone::Numpad => RankScope::Numpad,
    }
}

/// 排行页的排序/展示指标
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum RankMetric {
    Count,
    Share,
    PerDay,
}

/// 排行里的一行
#[derive(Clone, Debug, PartialEq)]
struct RankRow {
    /// 显示名（布局键用 label，布局外按键用 other_key_label）
    pub label: String,
    /// 区间内计数
    pub count: u64,
    /// 占该区间全部计数的比例（0..1）
    pub share: f64,
    /// 日均次数（区间天数 > 0）
    pub per_day: f64,
    /// 布局键的下标（鼠标行 / 布局外为 None）——点击行时用来跳到热力图并选中
    pub key_idx: Option<usize>,
    /// 是否鼠标行（配色用绿色系，与键盘的红系区分）
    pub mouse: bool,
}

fn rank_rows(
    store: &Store,
    from: NaiveDate,
    to: NaiveDate,
    hour: Option<u8>,
    scopes: &std::collections::BTreeSet<RankScope>,
    only_selected: Option<&std::collections::BTreeSet<usize>>,
    s: &Strings,
) -> Vec<RankRow> {
    let (counts, mouse, other) = rank_collect(store, from, to, hour);
    let mut rows: Vec<RankRow> = Vec::new();

    // 布局键：所属分区在选中的范围里（可同时勾多个分区）
    for (i, k) in KEYS.iter().enumerate() {
        if !scopes.contains(&scope_of_zone(crate::keys::zone_of(i))) {
            continue;
        }
        if let Some(sel) = only_selected {
            if !sel.contains(&i) {
                continue;
            }
        }
        let c = counts.get(i).copied().unwrap_or(0);
        rows.push(RankRow {
            label: rank_key_label(k).to_string(),
            count: c,
            share: 0.0,
            per_day: 0.0,
            key_idx: Some(i),
            mouse: false,
        });
    }
    // 鼠标 / 布局外键只在「看全部」时列出（与只选按键互斥）
    if only_selected.is_none() {
        if scopes.contains(&RankScope::Mouse) {
            rank_push_mouse_rows(&mut rows, &mouse, s);
        }
        if scopes.contains(&RankScope::Other) {
            rank_push_other_rows(&mut rows, other);
        }
    }

    // 占比与日均（合计含所有被选中的行）
    let total: u64 = rows.iter().map(|r| r.count).sum();
    // 区间天数（含没有数据的天）：与页面汇总行显示的天数一致，
    // 否则「1000 天 · 合计 X」配「X/活跃天数」的日均，用户没法从界面反推
    let days = (to - from).num_days() + 1;
    for r in rows.iter_mut() {
        r.share = if total > 0 { r.count as f64 / total as f64 } else { 0.0 };
        r.per_day = if days > 0 { r.count as f64 / days as f64 } else { 0.0 };
    }
    rows
}

/// 逐天累加区间内的按键 / 鼠标 / 布局外计数（作废的小时按 0 计）。
/// 返回（布局键计数, [左/中/右/滚轮/侧], 布局外键计数）。
fn rank_collect(
    store: &Store,
    from: NaiveDate,
    to: NaiveDate,
    hour: Option<u8>,
) -> (Vec<u64>, [u64; 5], std::collections::BTreeMap<String, u64>) {
    let mut counts: Vec<u64> = vec![0; crate::keys::N_KEYS];
    let mut mouse = [0u64; 5]; // 左/中/右/滚轮/侧键
    let mut other: std::collections::BTreeMap<String, u64> = std::collections::BTreeMap::new();

    let mut d = from;
    while d <= to {
        if let Some(day) = store.days.get(&d.format("%Y%m%d").to_string()) {
            // 取整天的桶，或只看某个钟点（作废的小时算 0）
            let unit: Option<&BucketStats> = match hour {
                None => Some(&day.bucket),
                Some(h) => day.hours.get(&h).filter(|u| !u.invalid),
            };
            if let Some(u) = unit {
                rank_merge_unit(u, &mut counts, &mut mouse, &mut other);
            }
        }
        d += chrono::Duration::days(1);
    }
    (counts, mouse, other)
}

/// 把一个统计桶累加进累计器。
fn rank_merge_unit(
    u: &BucketStats,
    counts: &mut [u64],
    mouse: &mut [u64; 5],
    other: &mut std::collections::BTreeMap<String, u64>,
) {
    for (i, c) in u.keys.iter().enumerate() {
        if let Some(slot) = counts.get_mut(i) {
            *slot += *c;
        }
    }
    mouse[0] += u.mouse.lb;
    mouse[1] += u.mouse.mb;
    mouse[2] += u.mouse.rb;
    mouse[3] += u.mouse.wheel;
    mouse[4] += u.mouse.xb;
    for (id, n) in &u.other {
        *other.entry(id.clone()).or_insert(0) += *n;
    }
}

/// 与键盘上那 5 个鼠标键一致：左 / 中 / 右 / 滚轮 / 侧键。
fn rank_push_mouse_rows(rows: &mut Vec<RankRow>, mouse: &[u64; 5], s: &Strings) {
    let names = [s.mkey_l, s.mkey_m, s.mkey_r, s.mkey_wheel, s.mkey_side];
    for (i, label) in names.iter().enumerate() {
        rows.push(RankRow {
            label: label.to_string(),
            count: mouse[i],
            share: 0.0,
            per_day: 0.0,
            key_idx: None,
            mouse: true,
        });
    }
}

/// 布局外的键（other 里记的内部 id）——名字走 stats 的标签映射。
fn rank_push_other_rows(rows: &mut Vec<RankRow>, other: std::collections::BTreeMap<String, u64>) {
    for (id, n) in other {
        rows.push(RankRow {
            label: crate::stats::other_key_label(&id),
            count: n,
            share: 0.0,
            per_day: 0.0,
            key_idx: None,
            mouse: false,
        });
    }
}

/// 排行列表里的按键名：标签唯一时用标签，重名时退回内部名（D1 / Num1 都显示 "1"，LShift / RShift 都显示 "Shift"）
fn rank_key_label(k: &crate::keys::KeyDef) -> &'static str {
    let label = if k.label.is_empty() { k.name } else { k.label };
    if KEYS.iter().filter(|o| o.label == k.label && !k.label.is_empty()).count() > 1 {
        k.name
    } else {
        label
    }
}

/// 按指标取值（排序与显示共用）
fn rank_value(r: &RankRow, m: RankMetric) -> f64 {
    match m {
        RankMetric::Count => r.count as f64,
        RankMetric::Share => r.share,
        RankMetric::PerDay => r.per_day,
    }
}

/// 渐变色：与原版 getcolors(0xEEEEEE, 0xB26C65, 100) 等价
fn heat_color(count: f64, maxcount: f64) -> Color32 {
    let t = if maxcount <= 0.0 {
        0.0
    } else if count >= maxcount {
        1.0
    } else if count < maxcount / 100.0 {
        0.0
    } else {
        (count / maxcount) as f32
    };
    let l = |a: u8, b: u8| (a as f32 + (b as f32 - a as f32) * t) as u8;
    Color32::from_rgb(l(HEAT_FROM.0, HEAT_TO.0), l(HEAT_FROM.1, HEAT_TO.1), l(HEAT_FROM.2, HEAT_TO.2))
}

/// 鼠标「五键」热力的渐变端点（绿色系，与键盘的 #EEEEEE→#B26C65 同亮度基准）
const MHEAT_FROM: (u8, u8, u8) = (0xEE, 0xEE, 0xEE);
const MHEAT_TO: (u8, u8, u8) = (0x6F, 0x8F, 0x62);

/// 鼠标五键的独立热力色：量程是「这五个值里的最大值」，与键盘的计数池互不影响
fn mouse_heat_color(count: f64, maxcount: f64) -> Color32 {
    let t = if maxcount <= 0.0 {
        0.0
    } else if count >= maxcount {
        1.0
    } else if count < maxcount / 100.0 {
        0.0
    } else {
        (count / maxcount) as f32
    };
    let l = |a: u8, b: u8| (a as f32 + (b as f32 - a as f32) * t) as u8;
    Color32::from_rgb(l(MHEAT_FROM.0, MHEAT_TO.0), l(MHEAT_FROM.1, MHEAT_TO.1), l(MHEAT_FROM.2, MHEAT_TO.2))
}

/// 方向键上方的 5 个格子：鼠标左/中/右键（上一行）与滚轮/侧键（下一行，中间留出 ↑ 键）。
/// 它们**不参与键盘热力**：颜色取自「鼠标五键」这个独立计数池（绿色系）。
struct MouseKeyDef {
    label: fn(&Strings) -> &'static str,
    value: fn(&MouseStats) -> f64,
}

const MOUSE_KEYS: [MouseKeyDef; 5] = [
    MouseKeyDef { label: |s| s.mkey_l, value: |m| m.lb as f64 },
    MouseKeyDef { label: |s| s.mkey_m, value: |m| m.mb as f64 },
    MouseKeyDef { label: |s| s.mkey_r, value: |m| m.rb as f64 },
    MouseKeyDef { label: |s| s.mkey_wheel, value: |m| m.wheel as f64 },
    MouseKeyDef { label: |s| s.mkey_side, value: |m| m.xb as f64 },
];

/// 鼠标五键的计数池量程（五个值里的最大值）
fn mouse_key_pool(m: &MouseStats) -> f64 {
    MOUSE_KEYS.iter().map(|k| (k.value)(m)).fold(0.0_f64, f64::max)
}

pub struct App {
    shared: Arc<Shared>,
    cfg_path: PathBuf,
    cfg: Config,
    lang: Lang,
    /// 底部“展开更多”面板是否展开（展开后同时显示统计与设置）
    more_open: bool,
    /// 右上角 "?" 的悬浮窗设置说明是否展开（展开时盖住内容区，见 `show_hud_help`）
    help_open: bool,
    /// 上一次「导出数据」的结果（文案, 时间）：显示十来秒后自动消失
    export_msg: Option<(String, std::time::Instant)>,
    /// 调试：启动后最小化一次
    start_minimized: bool,
    last_applied_h: f32,
    /// 待达成的窗口高度（早期 resize 请求可能被 WM 丢弃，需按实际尺寸校验重发）
    resize_target: Option<f32>,
    /// 待达成的窗口宽度（切换配列时设定，由每帧轮询校验达成）
    resize_target_w: Option<f32>,
    resize_deadline: Instant,
    /// 底部展开面板的实测高度（首帧估算窗口高度用）
    measured_panel_h: f32,
    /// 展开面板内容底部的绝对 y（窗口高度直接贴着它算，避免“底部边框被裁掉”）
    panel_bottom: f32,
    /// “收起状态”下的窗口高度（布局锚点：展开时底部区域固定在此高度处，避免 resize 落地前的跳动）
    base_h: f32,
    /// 底部区域（按钮行及展开面板）的起始 y，用于滚轮翻日期时避开该区域
    bottom_region_top: f32,
    /// 页面内容的底部 y（上一帧），用于把窗口高度自动贴合内容（消除留白）
    content_bottom: f32,
    /// 上一帧实际窗口高度（用于识别用户手动缩放）
    last_seen_h: f32,
    /// 上一帧实际窗口宽度（用于识别用户手动缩放）
    last_seen_w: f32,
    /// 用户手动调整过窗口大小后不再自动贴合内容
    user_sized: bool,
    /// 上一帧是否处于「停放」（收起）状态：用来捕捉「刚解除停放」那一帧
    was_parked: bool,
    /// 解除停放后的静默期：这期间窗口尺寸还是停放时的 32px（WM/KWin 脚本还没恢复），
    /// 几何变化一律不能当成「用户手动缩放」，也不记 last_seen（否则自适应会永久停摆）
    park_release_until: Option<Instant>,
    /// 调试：自截图输出前缀（KMCOUNTER_SCREENSHOT=/path/prefix）
    screenshot_prefix: Option<String>,
    frame_count: u64,
    shots_taken: u32,
    /// 热力图上被点选的按键（索引）
    selected: std::collections::BTreeSet<usize>,
    edit: Config,
    welcome: bool,
    tray_available: bool,
    /// 共享配置（托盘控制线程也会改，如开机启动开关）
    cfg_shared: Arc<parking_lot::Mutex<Config>>,
    /// 是否有可用中文字体（没有时中文会回退英文）
    has_cjk: bool,
    /// 托盘控制通道：界面语言变化时同步给托盘菜单
    tray_tx: Option<std::sync::mpsc::Sender<crate::tray::TrayCmd>>,
    /// 托盘 → GUI 的 UI 请求标志（控制线程写入，渲染循环读取）
    flags: Arc<crate::controller::UiFlags>,
    exiting: bool,
    last_save: Instant,
    // 快照（避免每帧加锁）
    snap: Store,
    snap_today: String,
    view_idx: usize,
    title_dirty: bool,
    page: Page,
    granularity: Granularity,
    /// 排行页：区间 / 键位范围 / 指标 / 排序方向 / 只看已选
    rank_range: RankRange,
    rank_scopes: std::collections::BTreeSet<RankScope>,
    rank_metric: RankMetric,
    /// true = 多到少（默认），false = 少到多
    rank_desc: bool,
    rank_only_selected: bool,
    /// 分时视图：None = 整天/总计，Some(h) = 只看 0-23 时
    hour_sel: Option<u8>,
    /// 上次刷新快照的时间（带小时明细后 store 更大，按需节流克隆）
    last_snapshot: Instant,
    /// 分时分布条在屏幕上的矩形（本帧绘制时记录，供离屏交互测试使用）
    strip_rect: Rect,
    /// 趋势图区域（点击跳转日期时用来换算桶位置，测试也会读）
    chart_rect: Rect,
    /// 键盘区域（测试按坐标定位鼠标五键）
    kbd_rect: Rect,
    /// 排行列表区域（指针在里面时滚轮用来滚动列表，不翻日期）
    rank_rect: Rect,
    /// 上一帧的视图下标：用来捕捉「刚切到总计」这个动作（此时排行页区间跟着变成全部）
    last_view_idx: usize,
    /// 排行页右上角分时折线图的矩形（测试用）
    rank_chart_rect: Rect,
    /// 视图日期列表缓存：[None=总计, 今日, 有数据的天…]（快照刷新时重建）
    view_days: Vec<Option<String>>,
    /// 日历弹窗是否展开
    cal_open: bool,
    /// 日历当前显示的月份 (年, 月)
    cal_month: (i32, u32),
    /// 日期标题的矩形（日历定位用）
    nav_title_rect: Rect,
    /// 鼠标滚轮翻页增量累加器（防微增量连发与抖动）
    wheel_delta_accum: f32,
    /// 独立 APM 悬浮窗是否显示
    pub hud_apm_show: bool,
    /// 独立 KPS 悬浮窗是否显示
    pub hud_kps_show: bool,
    /// APM 悬浮窗自己的设置窗口是否打开（右键悬浮窗打开；会话级，不写配置）
    apm_cfg_open: bool,
    /// KPS 悬浮窗自己的设置窗口是否打开
    kps_cfg_open: bool,
    /// KPS「设置键位」捕获中（此时输入线程会把按下的键记进 shared.kps_capture_buf）
    kps_capturing: bool,
    /// KPS 悬浮窗最近一次下发的窗口尺寸（变了才发命令；由 HUD 视口回调更新，
    /// 主窗口最小化时 HUD 自己也在用）
    kps_hud_size: Arc<parking_lot::Mutex<(f32, f32)>>,
    /// APM 悬浮窗最近一次下发的窗口尺寸（宽度贴合内容；变了才发命令）
    apm_hud_size: Arc<parking_lot::Mutex<(f32, f32)>>,
    /// HUD 独立视口 → 主窗口的事件（关窗 / 打开设置；主窗口最小化期间也可能堆积）
    hud_apm_events: Arc<parking_lot::Mutex<Vec<hud::HudEvent>>>,
    hud_kps_events: Arc<parking_lot::Mutex<Vec<hud::HudEvent>>>,
    /// HUD 视口本帧看到的位置 / 显示器尺寸（位置记忆用，由视口回调回写）
    hud_apm_seen: Arc<parking_lot::Mutex<hud::HudSeen>>,
    hud_kps_seen: Arc<parking_lot::Mutex<hud::HudSeen>>,
    /// APM / KPS 悬浮窗的位置记忆状态（恢复计数 + 待保存的稳定位置）
    apm_pos: HudPosState,
    kps_pos: HudPosState,
    /// 启动后的「位置安静期」：这段时间内不保存位置（避免和恢复动作打架）
    hud_pos_grace_until: Instant,
}

/// 极简悬浮窗 HUD 模式默认尺寸
#[allow(dead_code)]
pub const HUD_W: f32 = 390.0;
#[allow(dead_code)]
pub const HUD_H: f32 = 36.0;

/// 窗口尺寸管理：底部展开 设置/统计 面板时向下加高窗口（按钮位置保持不动）
const BASE_W: f32 = 1260.0;
const BASE_H: f32 = 500.0;

// 主窗口的下限尺寸 / 停放尺寸在 `gui::window` 里（MAIN_MIN_W/H、PARK_SIZE），
// 由上面的 `pub(crate) use window::*` 转出，`main.rs` 照旧用 `gui::MAIN_MIN_W`。

/// 根据键盘配列与几何配置计算窗口默认宽度
pub fn default_width_for_layout(layout_id: &str, cfg: &crate::config::LayoutCfg) -> f32 {
    let prof = layout_profile(layout_id);
    match prof.id {
        "104" => BASE_W,
        "98" => 1140.0,
        "87" => 990.0,
        "75" | "68" => 940.0,
        "60" | "64" => 860.0,
        _ => {
            let u = (cfg.key_w + cfg.key_spacing) as f32;
            let gap = cfg.h_spacing as f32;
            let kbd_w = prof.units_w * u + prof.gaps * gap - cfg.key_spacing as f32;
            (kbd_w + 24.0).max(840.0)
        }
    }
}
const SETTINGS_EXTRA_H: f32 = 285.0;
const STATS_EXTRA_H: f32 = 205.0;
const CONTROLS_H: f32 = 36.0;
/// 日期导航标题的固定宽度（保证 ▶ 位置稳定）
const NAV_TITLE_W: f32 = 235.0;
/// 底部「展开更多/收起」按钮的固定宽度（保证退出按钮位置稳定）
const MORE_BTN_W: f32 = 78.0;
/// 小时选择标题的固定宽度（保证右侧 ▶ 位置稳定）
const HOUR_TITLE_W: f32 = 76.0;
/// 展开面板底部预留：CentralPanel 下边距 8 + 4px 余量，保证容器底边框不被窗口边缘裁掉
const PANEL_BOTTOM_PAD: f32 = 12.0;
/// 小时视图下“按键较少”的判定阈值（一小时天然比一天少）
const HOUR_ENOUGH_KEYS: u64 = 20;
/// 排行页每一行的高度
pub(crate) const RANK_ROW_H: f32 = 22.0;
/// 排行表三列（右对齐）距右边的偏移：占比 / 日均 / 次数
const RANK_COL_SHARE: f32 = 210.0;
const RANK_COL_DAY: f32 = 110.0;
const RANK_COL_COUNT: f32 = 8.0;
/// 统计表里细分项（键盘分区）的缩进
const SUB_ROW_INDENT: f32 = 10.0;
/// 一天的小时数（与 stats::HOURS_PER_DAY 一致，供分布条使用）
const HOURS: usize = crate::stats::HOURS_PER_DAY;

impl App {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        egui_ctx: &egui::Context,
        shared: Arc<Shared>,
        cfg_shared: Arc<parking_lot::Mutex<Config>>,
        cfg_path: PathBuf,
        stats_path: PathBuf,
        show_request_path: PathBuf,
        first_run: bool,
        tray: Option<Tray>,
        autostart_state: Arc<std::sync::atomic::AtomicBool>,
    ) -> Self {
        let cfg = cfg_shared.lock().clone();
        let (has_cjk, lang) = Self::install_theme(egui_ctx, &cfg);
        let (flags, tray_available, tray_tx) = Self::start_tray_controller(
            egui_ctx,
            &shared,
            &cfg_shared,
            &cfg_path,
            stats_path,
            show_request_path,
            tray,
            autostart_state,
        );
        let hud_kps_events_dbg = Self::spawn_debug_schedulers(egui_ctx, &shared, &flags, cfg.keep_on_top);
        let (start_page, more_open, start_minimized, help_open) = Self::startup_panels();
        let snap = (*shared.store.lock()).clone();
        let snap_today = shared.today.lock().clone();
        let welcome = first_run && !cfg.welcome_shown;
        let view_days = Self::initial_view_days(&snap, &snap_today);

        let app = App {
            shared,
            cfg_path,
            welcome,
            has_cjk,
            tray_tx,
            lang,
            more_open,
            help_open,
            export_msg: None,
            edit: cfg.clone(),
            tray_available,
            cfg_shared,
            flags,
            exiting: false,
            last_save: Instant::now(),
            snap,
            snap_today,
            view_idx: Self::env_usize("KMCOUNTER_VIEW_IDX").unwrap_or(0),
            title_dirty: true,
            page: start_page,
            // 调试：KMCOUNTER_GRAN=hour|day|week|month|year 指定趋势页起始粒度（截图用）
            rank_range: RankRange::Week,
            rank_scopes: ALL_SCOPES.iter().copied().collect(),
            rank_metric: RankMetric::Count,
            rank_desc: true,
            rank_only_selected: false,
            granularity: Self::initial_granularity(),
            hour_sel: Self::env_usize("KMCOUNTER_HOUR").filter(|h| *h < 24).map(|h| h as u8),
            last_snapshot: Instant::now(),
            strip_rect: Rect::NOTHING,
            last_view_idx: 1,
            rank_chart_rect: Rect::NOTHING,
            chart_rect: Rect::NOTHING,
            kbd_rect: Rect::NOTHING,
            rank_rect: Rect::NOTHING,
            view_days,
            cal_open: std::env::var("KMCOUNTER_CAL").map(|v| v == "1").unwrap_or(false),
            cal_month: (chrono::Local::now().year(), chrono::Local::now().month()),
            nav_title_rect: Rect::NOTHING,
            wheel_delta_accum: 0.0,
            hud_apm_show: cfg.hud_apm_show,
            hud_kps_show: cfg.hud_kps_show,
            apm_cfg_open: false,
            kps_cfg_open: false,
            kps_capturing: false,
            kps_hud_size: Arc::new(parking_lot::Mutex::new((0.0, 0.0))),
            apm_hud_size: Arc::new(parking_lot::Mutex::new((0.0, 0.0))),
            hud_apm_events: Arc::new(parking_lot::Mutex::new(Vec::new())),
            hud_kps_events: hud_kps_events_dbg,
            hud_apm_seen: Arc::new(parking_lot::Mutex::new(hud::HudSeen::default())),
            hud_kps_seen: Arc::new(parking_lot::Mutex::new(hud::HudSeen::default())),
            apm_pos: HudPosState::default(),
            kps_pos: HudPosState::default(),
            // 覆盖 KWin 脚本侧恢复窗的前半段：恢复没成功时上报的是合成器的默认摆放位置，
            // 这段时间内一律不落盘 —— 记忆永不损坏（最坏情况是这次没恢复上，下次启动再试）
            hud_pos_grace_until: Instant::now() + Duration::from_secs(15),
            start_minimized,
            last_applied_h: BASE_H,
            resize_target: None,
            resize_target_w: None,
            resize_deadline: Instant::now(),
            measured_panel_h: 0.0,
            panel_bottom: 0.0,
            base_h: BASE_H,
            bottom_region_top: f32::MAX,
            content_bottom: 0.0,
            last_seen_h: 0.0,
            last_seen_w: 0.0,
            user_sized: false,
            was_parked: false,
            park_release_until: None,
            screenshot_prefix: std::env::var("KMCOUNTER_SCREENSHOT").ok().filter(|s| !s.is_empty()),
            frame_count: 0,
            shots_taken: 0,
            selected: Self::initial_selected(),
            cfg,
        };
        // 启动即把 HUD 状态推给托盘：菜单首次弹出时 ✔ 就是对的
        app.push_hud_state();
        app
    }

    /// 视图日期列表（初始）：[0]=总计、[1]=今日，其后按日期倒序。
    fn initial_view_days(snap: &Store, snap_today: &str) -> Vec<Option<String>> {
        let mut days: Vec<Option<String>> = vec![None, Some(snap_today.to_string())];
        for d in snap.days.keys().rev() {
            if d != snap_today {
                days.push(Some(d.clone()));
            }
        }
        days
    }


    /// 读一个整数环境变量（调试用），空值或解析失败返回 None。
    fn env_usize(key: &str) -> Option<usize> {
        std::env::var(key)
            .ok()
            .and_then(|v| v.trim().parse::<usize>().ok())
    }

    /// 调试：KMCOUNTER_GRAN=hour|day|week|month|year 指定趋势页起始粒度（截图用）。
    fn initial_granularity() -> Granularity {
        match std::env::var("KMCOUNTER_GRAN")
            .unwrap_or_default()
            .to_lowercase()
            .as_str()
        {
            "hour" | "hourly" => Granularity::Hourly,
            "week" | "weekly" => Granularity::Weekly,
            "month" | "monthly" => Granularity::Monthly,
            "year" | "yearly" => Granularity::Yearly,
            _ => Granularity::Daily,
        }
    }

    /// 调试：KMCOUNTER_SELECT=3,5,7 启动即选中这些键（截图/验证用）。
    fn initial_selected() -> std::collections::BTreeSet<usize> {
        std::env::var("KMCOUNTER_SELECT")
            .ok()
            .map(|v| {
                v.split(',')
                    .filter_map(|s| s.trim().parse::<usize>().ok())
                    .filter(|i| *i < KEYS.len())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// 装字体、定界面语言、设浅色视觉与紧凑间距；返回（是否有中文字体, 解析后的语言）。
    fn install_theme(egui_ctx: &egui::Context, cfg: &Config) -> (bool, Lang) {
        // 字体
        let mut defs = egui::FontDefinitions::default();
        let has_cjk = crate::fonts::install_cjk(&mut defs, &cfg.font_path);
        egui_ctx.set_fonts(defs);

        let lang = Lang::resolve(&cfg.language).downgrade_if_no_cjk(has_cjk);
        egui_ctx.set_visuals(egui::Visuals::light());
        egui_ctx.style_mut(|s| {
            s.spacing.item_spacing = Vec2::new(8.0, 6.0);
        });
        (has_cjk, lang)
    }

    /// 托盘命令交给独立控制线程处理（最小化时渲染帧会停止，不能在渲染循环里读命令）。
    /// 返回（UI 标志位, 是否有托盘, 留给自己的那份托盘发送端——切语言时要推新菜单）。
    #[allow(clippy::too_many_arguments)]
    fn start_tray_controller(
        egui_ctx: &egui::Context,
        shared: &Arc<Shared>,
        cfg_shared: &Arc<parking_lot::Mutex<Config>>,
        cfg_path: &PathBuf,
        stats_path: PathBuf,
        show_request_path: PathBuf,
        tray: Option<Tray>,
        autostart_state: Arc<std::sync::atomic::AtomicBool>,
    ) -> (
        Arc<crate::controller::UiFlags>,
        bool,
        Option<std::sync::mpsc::Sender<crate::tray::TrayCmd>>,
    ) {
        let flags = Arc::new(crate::controller::UiFlags::default());
        let tray_available = tray.is_some();
        // 留一份托盘控制通道给自己：切换语言时把新语言推给托盘菜单
        let tray_tx = tray.as_ref().map(|t| t.tx.clone());
        if let Some(t) = tray {
            crate::controller::spawn(
                crate::controller::Controller {
                    shared: shared.clone(),
                    cfg: cfg_shared.clone(),
                    cfg_path: cfg_path.clone(),
                    stats_path,
                    autostart: autostart_state,
                    flags: flags.clone(),
                    tray_ctl: Some(t.tx),
                },
                t.rx,
                egui_ctx.clone(),
                show_request_path,
            );
        }
        (flags, tray_available, tray_tx)
    }

    /// 三个调试用的定时器（N 秒后自动切页 / 模拟点关闭按钮 / 模拟右键或双击悬浮窗）。
    /// 只在设置了对应环境变量时生效；返回 HUD 事件队列。
    fn spawn_debug_schedulers(
        egui_ctx: &egui::Context,
        shared: &Arc<Shared>,
        flags: &Arc<crate::controller::UiFlags>,
        keep_on_top: bool,
    ) -> Arc<parking_lot::Mutex<Vec<hud::HudEvent>>> {
        Self::spawn_switch_page_timer(egui_ctx, flags);
        Self::spawn_fake_close_timer(egui_ctx);
        Self::spawn_fake_hud_click(egui_ctx, shared, keep_on_top)
    }

    /// KMCOUNTER_SWITCH_PAGE_AFTER_SECS=N → N 秒后自动切换到另一个页面（验证两页布局一致性）
    fn spawn_switch_page_timer(egui_ctx: &egui::Context, flags: &Arc<crate::controller::UiFlags>) {
        let Ok(secs) = std::env::var("KMCOUNTER_SWITCH_PAGE_AFTER_SECS").map(|v| v.parse::<u64>()) else {
            return;
        };
        let Ok(secs) = secs else { return };
        let ctx = egui_ctx.clone();
        let flags = flags.clone();
        flags.switch_page.store(true, Ordering::Relaxed); // 由渲染循环执行切换
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_secs(secs));
            flags.switch_page_after.store(true, Ordering::Relaxed);
            ctx.request_repaint();
        });
    }

    /// KMCOUNTER_FAKE_CLOSE_AFTER_SECS=N → N 秒后模拟“点窗口关闭按钮”
    ///（发送 ViewportCommand::Close，egui-winit 会转成 close_requested 事件，与真实点 × 同路径）
    fn spawn_fake_close_timer(egui_ctx: &egui::Context) {
        let Ok(list) = std::env::var("KMCOUNTER_FAKE_CLOSE_AFTER_SECS") else {
            return;
        };
        let secs: Vec<u64> = list.split(',').filter_map(|s| s.trim().parse().ok()).collect();
        if secs.is_empty() {
            return;
        }
        let ctx = egui_ctx.clone();
        std::thread::spawn(move || {
            let start = std::time::Instant::now();
            for s in secs {
                let target = std::time::Duration::from_secs(s);
                if target > start.elapsed() {
                    std::thread::sleep(target - start.elapsed());
                }
                log::info!("调试：模拟点击窗口关闭按钮 (@{s}s)");
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
        });
    }

    fn spawn_fake_hud_click(
        egui_ctx: &egui::Context,
        shared: &Arc<Shared>,
        keep_on_top: bool,
    ) -> Arc<parking_lot::Mutex<Vec<hud::HudEvent>>> {
        let queue = Arc::new(parking_lot::Mutex::new(Vec::new()));
        let Ok(spec) = std::env::var("KMCOUNTER_FAKE_HUD_CLICK") else {
            return queue;
        };
        let jobs = parse_fake_hud_jobs(&spec);
        if jobs.is_empty() {
            return queue;
        }
        let ctx = egui_ctx.clone();
        let shared = shared.clone();
        let inner = queue.clone();
        std::thread::spawn(move || {
            run_fake_hud_jobs(jobs, &shared, &ctx, keep_on_top, &inner);
        });
        queue
    }

    /// 读 KMCOUNTER_START_PANELS：返回（起始页, 是否展开底部面板, 是否启动即隐藏, 是否直接进帮助页）。
    fn startup_panels() -> (Page, bool, bool, bool) {
        let start_panels = std::env::var("KMCOUNTER_START_PANELS").unwrap_or_default().to_lowercase();
        let more_open = start_panels.contains("stats") || start_panels.contains("settings") || start_panels.contains("both");
        let start_page = if start_panels.contains("trend") {
            Page::Trends
        } else if start_panels.contains("rank") {
            Page::Ranking
        } else {
            Page::Heatmap
        };
        let start_minimized = start_panels.contains("min");
        let help_open = start_panels.contains("help");
        // 启动即进帮助页时同样收起「展开更多」——与点 "?" 的行为保持一致（用于真机核对窗口尺寸）
        let more_open = more_open && !help_open;
        (start_page, more_open, start_minimized, help_open)
    }

    fn s(&self) -> &'static Strings {
        strings(self.lang)
    }

    /// 按 self.edit.language 切换界面语言（无中文字体时自动回退英文），并同步托盘菜单
    fn apply_language(&mut self) {
        let lang = Lang::resolve(&self.edit.language).downgrade_if_no_cjk(self.has_cjk);
        if lang == self.lang {
            return;
        }
        self.lang = lang;
        self.title_dirty = true; // 窗口标题里也带界面文案
        if let Some(tx) = &self.tray_tx {
            let _ = tx.send(crate::tray::TrayCmd::Language(lang));
        }
        log::info!("界面语言切换为 {:?}", lang);
    }

    /// 重建视图日期列表：[None=总计, 今日, 其余有数据的天（新→旧）]
    fn rebuild_view_days(&mut self) {
        let today = self.snap_today.clone();
        let mut v: Vec<Option<String>> = vec![None, Some(today.clone())];
        for d in self.snap.days.keys().rev() {
            if d != &today {
                v.push(Some(d.clone()));
            }
        }
        self.view_days = v;
        if self.view_idx >= self.view_days.len() {
            self.view_idx = 0;
        }
    }

    fn refresh_snapshot(&mut self) {
        self.snap = (*self.shared.store.lock()).clone();
        self.snap_today = self.shared.today.lock().clone();
        self.rebuild_view_days();
        self.title_dirty = true;
    }

    fn save_all(&mut self) {
        // 先清 dirty 再取快照：清标记与取快照之间到达的输入会包含在快照里，
        // 快照之后到达的输入会重新置 dirty（等下一轮保存），不会像“先快照后清标记”那样被漏掉
        self.shared.dirty.store(false, Ordering::Relaxed);
        let store = (*self.shared.store.lock()).clone();
        let stats_path = self.cfg_path.with_file_name("stats.json");
        let mut ok = true;
        if let Err(e) = store.save(&stats_path) {
            log::error!("保存统计数据失败: {e}");
            ok = false;
        }
        let cfg = self.cfg_shared.lock().clone();
        if let Err(e) = cfg.save(&self.cfg_path) {
            log::error!("保存配置失败: {e}");
        }
        if !ok {
            self.shared.dirty.store(true, Ordering::Relaxed); // 保存失败：下次重试
        }
        self.last_save = Instant::now();
    }

    pub(crate) fn active_layout(&self) -> &'static LayoutProfile {
        layout_profile(&self.cfg.layout.keyboard_layout)
    }

    fn current_metrics(&self) -> (f32, f32, f32, Vec2) {
        let l = &self.cfg.layout;
        let u = (l.key_w + l.key_spacing) as f32;
        let pitch = (l.key_h + l.key_spacing) as f32;
        let gap = l.h_spacing as f32;
        let prof = self.active_layout();
        let size = Vec2::new(
            prof.units_w * u + prof.gaps * gap - l.key_spacing as f32,
            prof.rows_h * pitch - l.key_spacing as f32,
        );
        (u, pitch, gap, size)
    }

    fn key_rect_for(&self, lk: &LayoutKey, rect: Rect, u: f32, pitch: f32, gap: f32) -> Rect {
        let spacing = self.cfg.layout.key_spacing as f32;
        let ox = lk.x * u + lk.gap_count as f32 * gap;
        let oy = lk.y * pitch;
        Rect::from_min_size(
            Pos2::new(rect.left() + ox, rect.top() + oy),
            Vec2::new(lk.w * u - spacing, lk.h * pitch - spacing),
        )
    }

    #[allow(dead_code)]
    pub(crate) fn key_rect(&self, i: usize, rect: Rect, u: f32, pitch: f32, gap: f32) -> Rect {
        let prof = self.active_layout();
        if let Some(lk) = prof.keys.iter().find(|k| k.idx == i) {
            self.key_rect_for(lk, rect, u, pitch, gap)
        } else {
            Rect::NOTHING
        }
    }

    /// 鼠标五键的矩形（根据当前配列自适应摆放）
    fn mouse_key_rect(&self, rect: Rect, u: f32, pitch: f32, gap: f32, mk_idx: usize) -> Rect {
        let spacing = self.cfg.layout.key_spacing as f32;
        let p = &self.active_layout().mouse_keys[mk_idx];
        let ox = p.x * u + p.gap_count as f32 * gap;
        let oy = p.y * pitch;
        Rect::from_min_size(
            Pos2::new(rect.left() + ox, rect.top() + oy),
            Vec2::new(u - spacing, pitch - spacing),
        )
    }

    /// 当前视图的“那一天”（总计视图返回 total）
    fn day_for_view(&self) -> DayEntry {
        if self.view_idx == 0 {
            self.snap.total.clone()
        } else {
            let d = self.view_label();
            self.snap.days.get(&d).cloned().unwrap_or_default()
        }
    }

    /// 当前视图真正参与显示的桶：选中小时时为该小时（总计视图下即“各小时累计”）
    fn unit_for_view(&self) -> BucketStats {
        self.day_for_view().view(self.hour_sel)
    }

    /// 底部展开面板需要额外的高度（首帧用估算值，之后用实测值自适应）
    fn panel_extra_h(&self) -> f32 {
        if !self.more_open {
            return 0.0;
        }
        if self.measured_panel_h > 1.0 {
            self.measured_panel_h + PANEL_BOTTOM_PAD
        } else {
            // 首帧估算：统计 + 设置两块内容之和
            STATS_EXTRA_H + SETTINGS_EXTRA_H
        }
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.draw(ctx);
    }

    /// 清屏色：**必须完全透明**，否则悬浮窗不透明度调到 0 也还是有底色。
    ///
    /// eframe 的清屏色是**全视口共用**的（glow 后端在 paint 前对每个 viewport 都调
    /// painter.clear(.., clear_color)），默认值 Color32::from_rgba_unmultiplied(12,12,12,180)
    /// 会先给**每个**视口铺一层半透明深灰。悬浮窗的 CentralPanel 在 fill 的 alpha 为 0 时
    /// 不产生任何图元（epaint 对全透明填充直接跳过），于是露出来的就是那层清屏灰 ——
    /// 现象就是「不透明度调到 0 了，底板还在」。
    /// **注意：软件渲染（softshot）根本测不出这个 bug**，因为它不走 eframe 的清屏路径。
    ///
    /// 主窗口不透明（没有 with_transparent），glClearColor 的 alpha 对它无效，
    /// 且它自己的 CentralPanel 铺满整窗，所以改成全透明不影响主界面。
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        [0.0, 0.0, 0.0, 0.0]
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.save_all();
        // 卸载 HUD 监听脚本（KDE/Wayland），避免 KWin 里留下僵尸脚本
        #[cfg(target_os = "linux")]
        crate::kwin::stop_hud_listener();
    }
}

impl App {
    /// 一帧的界面与窗口逻辑。与 eframe 解耦，便于离屏软渲染出图（见 softshot 模块）。
    ///
    /// 这里只剩编排，每一步都在 gui::frame 里（顺序不能随意调整，理由见那个文件的开头）。
    pub(crate) fn draw(&mut self, ctx: &egui::Context) {
        let s = self.s();

        self.frame_count += 1;

        self.sync_config_and_flags(ctx);
        if self.handle_close_request(ctx) {
            // 真正退出：eframe 会随即销毁窗口，这帧之后的步骤都没有意义
            return;
        }
        self.update_title(ctx, s);
        self.poll_hud_flags(ctx, s);
        self.adapt_window_size(ctx);
        self.reset_rank_range_on_total();
        self.handle_navigation_keys(ctx);
        self.draw_ui(ctx, s);
        self.debug_take_screenshots(ctx);
        // 日历是浮层：必须最后画，才能盖在内容区之上
        if self.cal_open {
            self.show_calendar(ctx, s);
        }
        self.debug_layout_log(ctx);
        // 500ms 一帧足够（统计数据由采集线程更新，界面只需要刷新节拍）
        ctx.request_repaint_after(Duration::from_millis(500));
    }

    /// 右上角 "?" 打开的「悬浮窗设置说明」：把 APM / KPS 两个设置窗里的说明集中到一处。
    ///
    /// 三条列表分别对应「通用 / APM / KPS」，**渲染时自动编号**（文案里不要写序号，
    /// 增删条目不用手工重排）。正文比界面小号字大 50%：设置窗里字号受窗口尺寸限制，
    /// 这里空间宽裕，看得清更重要。
    /// 高度受内容区限制：说明再长也不撑高窗口（自己滚），免得把底部按钮行挤出去。
    fn show_hud_help(&mut self, ui: &mut egui::Ui, s: &'static Strings, content_top: f32) {
        let size = help_font_size(ui);
        ui.label(egui::RichText::new(s.help_title).size(size * 1.2).strong().color(TEXT));
        ui.add_space(6.0);
        // 帮助页独占内容区（底栏不画），所以能用到底：base_h 的下边缘就是窗口内容底。
        // 窗口高度不受影响（base_h 仍由普通页面决定），说明再长也只在里面滚。
        let avail = (content_top + self.base_h - ui.cursor().min.y - 4.0).max(120.0);
        egui::ScrollArea::vertical()
            .max_height(avail)
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.columns(2, |cols| {
                    // 左栏：通用 + APM
                    help_section(&mut cols[0], s.help_common, size);
                    for (i, item) in s.help_common_items.iter().enumerate() {
                        help_item(&mut cols[0], i + 1, item, size, s.help_colon);
                    }
                    help_section(&mut cols[0], s.help_apm, size);
                    for (i, item) in s.help_apm_items.iter().enumerate() {
                        help_item(&mut cols[0], i + 1, item, size, s.help_colon);
                    }
                    // 右栏：KPS
                    help_section(&mut cols[1], s.help_kps, size);
                    for (i, item) in s.help_kps_items.iter().enumerate() {
                        help_item(&mut cols[1], i + 1, item, size, s.help_colon);
                    }
                });
            });
    }

    /// 开关右上角 "?" 的「悬浮窗设置说明」页。
    ///
    /// 帮助页是**独立页面**：进来时顺手把底部「展开更多」收起 —— 否则窗口会停在"展开后"的
    /// 高度，看着像帮助页把面板顶下去了（`more_open=false` → `desired_h` 回 `base_h`
    /// → 自适应逻辑下一帧就把窗口缩回去）。标题也要跟着在「帮助」和日期之间切换。
    /// 抽成方法是为了 UI 按钮、调试入口、单元测试共用同一份逻辑。
    fn toggle_help(&mut self) {
        self.help_open = !self.help_open;
        if self.help_open {
            self.more_open = false;
        }
        self.title_dirty = true;
    }

    /// 当前视图这一天的小时曲线数据（热力图导航行与排行页共用）：
    /// 返回 (24 小时点, 作废标记, 当前钟点)。
    pub(crate) fn hour_strip_data(&self) -> (Vec<HourPoint>, Vec<bool>, Option<u8>) {
        let entry = self.day_for_view();
        let prof = entry.hour_profile();
        let invalid: Vec<bool> = (0..HOURS).map(|h| entry.hour_invalid(h as u8)).collect();
        let now_hour = (self.view_idx > 0 && self.view_label() == self.snap_today)
            .then(|| chrono::Local::now().hour().min(23) as u8);
        (prof, invalid, now_hour)
    }

    /// 小时曲线的悬停提示（两处共用）。
    pub(crate) fn hour_strip_hover(
        &self,
        ui: &egui::Ui,
        painter: &egui::Painter,
        area: Rect,
        prof: &[HourPoint],
        invalid: &[bool],
        s: &'static Strings,
        pos: Pos2,
    ) {
        let Some(idx) = hour_bar_index(area, pos.x) else { return };
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        let p = prof.get(idx).copied().unwrap_or_default();
        let unit = s.unit_times.trim();
        let mark = if invalid.get(idx).copied().unwrap_or(false) { s.invalid_mark } else { "" };
        let text = format!(
            "{}{} · {} {}{} · {} {}{} · {} {:.2} {}{}{}",
            idx,
            s.hour_suffix,
            s.keystrokes,
            p.keystrokes,
            unit,
            s.mouse_clicks,
            p.clicks,
            unit,
            s.mouse_move,
            self.shared.px_to_meters(p.move_px),
            s.unit_m,
            mark,
            s.strip_hint
        );
        paint_tooltip(painter, ui, pos, &text);
    }

    /// 小时曲线的点击：切换选中小时（再点一次取消）。
    pub(crate) fn hour_strip_click(&mut self, resp: &egui::Response, area: Rect) {
        if !resp.clicked() {
            return;
        }
        let Some(pos) = resp.interact_pointer_pos() else { return };
        let Some(idx) = hour_bar_index(area, pos.x) else { return };
        let h = idx as u8;
        self.hour_sel = if self.hour_sel == Some(h) { None } else { Some(h) };
    }

}

/// 在键盘右上角空白处绘制 24 小时分时折线图：三条曲线（键盘敲击=红 / 鼠标按键=绿 / 鼠标移动=蓝），
/// 每条各自按自己的量程归一化（三者的量级差很多，共用一根轴会互相压平），共用下面的小时刻度。
/// 当前小时有浅色底、选中小时有强调描边、作废的小时压一条浅灰并按 0 画。
fn paint_hour_lines(
    painter: &egui::Painter,
    area: Rect,
    prof: &[HourPoint],
    selected: Option<u8>,
    now_hour: Option<u8>,
    invalid: &[bool],
) {
    // 浅色卡片底板与细边框
    painter.rect_filled(area, 4.0, Color32::from_rgb(0xF5, 0xF6, 0xF8));
    painter.rect_stroke(area, 4.0, egui::Stroke::new(1.0_f32, Color32::from_rgb(0xDC, 0xDF, 0xE5)));

    let (label_h, plot, slot) = hour_plot_geometry(area);
    let maxes = hour_maxes(prof, invalid);
    paint_hour_bands(painter, area, plot, slot, selected, now_hour, invalid);
    paint_hour_curves(painter, prof, invalid, plot, slot, &maxes);
    paint_hour_labels(painter, area, slot, label_h);
}

/// 分时图的几何：底部刻度高、曲线区、每小时占的宽度。
fn hour_plot_geometry(area: Rect) -> (f32, Rect, f32) {
    let label_h = 11.0;
    let plot = Rect::from_min_max(area.min, Pos2::new(area.right(), area.bottom() - label_h));
    let slot = area.width() / HOURS as f32;
    (label_h, plot, slot)
}

/// 某小时的三条曲线取值；作废的小时按 0 画（与趋势图一致）。
fn hour_values(prof: &[HourPoint], invalid: &[bool], h: usize) -> [f64; 3] {
    let p = prof.get(h).copied().unwrap_or_default();
    if invalid.get(h).copied().unwrap_or(false) {
        [0.0, 0.0, 0.0]
    } else {
        [p.keystrokes as f64, p.clicks as f64, p.move_px]
    }
}

/// 三条曲线各自的最大值（作废小时不参与量程计算）。
fn hour_maxes(prof: &[HourPoint], invalid: &[bool]) -> [f64; 3] {
    let mut maxes = [0.0f64; 3];
    for h in 0..HOURS {
        let v = hour_values(prof, invalid, h);
        for i in 0..3 {
            maxes[i] = maxes[i].max(v[i]);
        }
    }
    maxes
}

/// 背景：当前小时浅底 / 选中小时强调描边 / 作废小时浅灰条。
fn paint_hour_bands(
    painter: &egui::Painter,
    area: Rect,
    plot: Rect,
    slot: f32,
    selected: Option<u8>,
    now_hour: Option<u8>,
    invalid: &[bool],
) {
    for h in 0..HOURS {
        let x0 = area.left() + h as f32 * slot;
        let band = Rect::from_min_size(
            Pos2::new(x0, area.top() + 1.0),
            Vec2::new(slot, area.height() - 2.0),
        );
        if now_hour == Some(h as u8) {
            painter.rect_filled(band, 2.0, Color32::from_rgb(0xE2, 0xE5, 0xED));
        }
        if invalid.get(h).copied().unwrap_or(false) {
            painter.rect_filled(
                Rect::from_min_size(
                    Pos2::new(x0 + slot * 0.3, plot.top()),
                    Vec2::new(slot * 0.4, plot.height()),
                ),
                1.0,
                Color32::from_rgb(0xDA, 0xDA, 0xDA),
            );
        }
        if selected == Some(h as u8) {
            painter.rect_stroke(band.shrink(0.5), 2.0, egui::Stroke::new(2.0_f32, SEL_COLOR));
        }
    }
}

/// 三条折线 + 每个小时的点（上下各留 3px，避免贴边）。
fn paint_hour_curves(
    painter: &egui::Painter,
    prof: &[HourPoint],
    invalid: &[bool],
    plot: Rect,
    slot: f32,
    maxes: &[f64; 3],
) {
    let colors = [KEY_COLOR, CLICK_COLOR, MOUSE_COLOR];
    let x_at = |h: usize| plot.left() + (h as f32 + 0.5) * slot;
    let y_at = |v: f64, max: f64| {
        if max <= 0.0 {
            plot.bottom() - 3.0
        } else {
            plot.bottom() - 3.0 - (v / max).min(1.0) as f32 * (plot.height() - 6.0)
        }
    };
    for si in 0..3 {
        let max = maxes[si];
        let pts: Vec<Pos2> = (0..HOURS)
            .map(|h| Pos2::new(x_at(h), y_at(hour_values(prof, invalid, h)[si], max)))
            .collect();
        painter.add(Shape::line(pts.clone(), egui::Stroke::new(1.6_f32, colors[si])));
        for p in &pts {
            painter.circle_filled(*p, 1.8, colors[si]);
        }
    }
}

/// 底部小时刻度（每 3 小时一个）。
fn paint_hour_labels(painter: &egui::Painter, area: Rect, slot: f32, label_h: f32) {
    let dim = Color32::from_rgb(0x99, 0x99, 0x99);
    let x_at = |h: usize| area.left() + (h as f32 + 0.5) * slot;
    for h in (0..HOURS).step_by(3) {
        painter.text(
            Pos2::new(x_at(h), area.bottom() - label_h / 2.0),
            Align2::CENTER_CENTER,
            h.to_string(),
            FontId::proportional(9.0),
            dim,
        );
    }
}

fn group_pad(ui: &egui::Ui) -> Vec2 {
    egui::Frame::group(ui.style()).inner_margin.sum()
}

fn geo_log(tag: &str, r: Rect) {
    if std::env::var("KMCOUNTER_GEO_LOG").is_ok() {
        eprintln!("[geo] {tag:<16} x {:.1}..{:.1} (w {:.1})  y {:.1}..{:.1} (h {:.1})", r.left(), r.right(), r.width(), r.top(), r.bottom(), r.height());
    }
}

fn delta_card(ui: &mut egui::Ui, s: &Strings, title: &str, value: &str, d: crate::trend::PeriodDelta, vs: &'static str, card_w: f32) {
    // 单行紧凑排版（标题 · 数值 · 涨跌同一行），把纵向空间让给“切换日期”
    let pad = 12.0; // inner_margin 左右各 6
    let (text, color) = match d.pct {
        None => (s.stat_flat.to_string(), DELTA_FLAT),
        Some(p) if p.abs() < 0.5 => (s.stat_flat.to_string(), DELTA_FLAT),
        Some(p) if p > 0.0 => (format!("+{:.0}%", p), DELTA_UP),
        Some(p) => (format!("-{:.0}%", p.abs()), DELTA_DOWN),
    };
    egui::Frame::none()
        .fill(Color32::from_rgb(0xF7, 0xF7, 0xF7))
        .stroke(egui::Stroke::new(1.0_f32, Color32::from_rgb(0xDD, 0xDD, 0xDD)))
        .rounding(3.0)
        .inner_margin(egui::Margin::symmetric(6.0, 3.0))
        .show(ui, |ui| {
            // 注意：横向行里直接用 with_layout(right_to_left) 会吞掉“整行剩余宽度”，
            // 整行会被撑大并连锁把后面的行（含图表）一起撑宽，所以这里给右侧固定宽度。
            let inner_w = (card_w - pad).max(60.0);
            let right_w = (inner_w * 0.5).max(96.0);
            ui.allocate_ui_with_layout(
                Vec2::new(inner_w, 17.0),
                egui::Layout::right_to_left(egui::Align::Center),
                |ui| {
                    ui.label(egui::RichText::new(text).size(11.0).color(color));
                    ui.label(egui::RichText::new(value).strong().size(12.5).color(TEXT));
                    ui.allocate_ui_with_layout(
                        Vec2::new((inner_w - right_w - 6.0).max(40.0), 17.0),
                        egui::Layout::left_to_right(egui::Align::Center),
                        |ui| {
                            ui.label(egui::RichText::new(title).small().color(Color32::from_rgb(0x99, 0x99, 0x99)));
                        },
                    );
                },
            );
        })
        .response
        .on_hover_text(vs);
}

fn hour_label(h: u8, s: &Strings) -> String {
    format!("{h}{}", s.hour_suffix)
}

fn shift_days(d: NaiveDate, days: i64) -> NaiveDate {
    d + chrono::Duration::days(days)
}

fn shift_months(d: NaiveDate, months: u32, newer: bool) -> NaiveDate {
    let m = chrono::Months::new(months);
    let r = if newer { d.checked_add_months(m) } else { d.checked_sub_months(m) };
    r.unwrap_or(d)
}

fn bucket_range(start: NaiveDate, g: Granularity) -> (NaiveDate, NaiveDate) {
    let next = match g {
        Granularity::Hourly | Granularity::Daily => shift_days(start, 1),
        Granularity::Weekly => shift_days(start, 7),
        Granularity::Monthly => shift_months(start, 1, true),
        Granularity::Yearly => shift_months(start, 12, true),
    };
    (start, next - chrono::Duration::days(1))
}

fn zone_label(z: crate::keys::Zone, s: &Strings) -> &'static str {
    use crate::keys::Zone;
    match z {
        Zone::Main => s.zone_main,
        Zone::Function => s.zone_function,
        Zone::Control => s.zone_control,
        Zone::Numpad => s.zone_numpad,
    }
}

/// 分布条的 x 坐标 → 小时下标（超出条外返回 None）
fn hour_bar_index(area: Rect, x: f32) -> Option<usize> {
    if area.width() <= 0.0 || x < area.left() || x > area.right() {
        return None;
    }
    let idx = ((x - area.left()) / (area.width() / HOURS as f32)).floor();
    if (0.0..HOURS as f32).contains(&idx) {
        Some(idx as usize)
    } else {
        None
    }
}

/// 右对齐单元格（数字列）
fn right_label(ui: &mut egui::Ui, t: egui::RichText) {
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        ui.label(t);
    });
}

/// 深色小提示框：贴近鼠标，自动避让屏幕边缘
fn paint_tooltip(painter: &egui::Painter, ui: &egui::Ui, pos: Pos2, text: &str) {
    let galley = painter.layout_no_wrap(text.to_string(), FontId::proportional(12.0), Color32::WHITE);
    let pad = 6.0;
    let box_size = galley.size() + Vec2::new(pad * 2.0, pad * 2.0);
    let mut origin = pos + Vec2::new(14.0, 14.0);
    let screen = ui.input(|i| i.screen_rect);
    if origin.x + box_size.x > screen.right() {
        origin.x = pos.x - box_size.x - 14.0;
    }
    if origin.y + box_size.y > screen.bottom() {
        origin.y = pos.y - box_size.y - 14.0;
    }
    painter.rect_filled(Rect::from_min_size(origin, box_size), 4.0, Color32::from_rgba_unmultiplied(40, 40, 40, 240));
    painter.galley(origin + Vec2::splat(pad), galley, Color32::WHITE);
}

fn arrow_points(dir: crate::keys::ArrowDir, r: Rect) -> (Pos2, Pos2, Pos2) {
    let (w, h) = (r.width(), r.height());
    match dir {
        crate::keys::ArrowDir::Up => (
            Pos2::new(r.center().x, r.top() + h * 0.3),
            Pos2::new(r.left() + w * 0.3, r.bottom() - h * 0.25),
            Pos2::new(r.right() - w * 0.3, r.bottom() - h * 0.25),
        ),
        crate::keys::ArrowDir::Down => (
            Pos2::new(r.center().x, r.bottom() - h * 0.3),
            Pos2::new(r.left() + w * 0.3, r.top() + h * 0.25),
            Pos2::new(r.right() - w * 0.3, r.top() + h * 0.25),
        ),
        crate::keys::ArrowDir::Left => (
            Pos2::new(r.left() + w * 0.3, r.center().y),
            Pos2::new(r.right() - w * 0.25, r.top() + h * 0.3),
            Pos2::new(r.right() - w * 0.25, r.bottom() - h * 0.3),
        ),
        crate::keys::ArrowDir::Right => (
            Pos2::new(r.right() - w * 0.3, r.center().y),
            Pos2::new(r.left() + w * 0.25, r.top() + h * 0.3),
            Pos2::new(r.left() + w * 0.25, r.bottom() - h * 0.3),
        ),
    }
}

// 主窗口的显示 / 隐藏 / 停放（hide_window / show_window_now / is_wayland /
// win32_window）在 `gui::window` 里，见文件顶部的 `pub(crate) use window::*;`。

fn monitor_size_str(shared: &Arc<Shared>, s: &Strings) -> String {
    let w = shared.screen_w_mm.load(Ordering::Relaxed) as f64;
    let h = shared.screen_h_mm.load(Ordering::Relaxed) as f64;
    if w > 0.0 && h > 0.0 {
        format!("{:.1} {}", (w * w + h * h).sqrt() / 25.4, s.unit_inch)
    } else {
        "-".to_string()
    }
}

/// 千分位整数格式化：12345 → "12,345"
fn fmt_thousands(v: f64) -> String {
    let n = v.round() as i128;
    let neg = n < 0;
    let digits = n.abs().to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    if neg {
        out.insert(0, '-');
    }
    out
}

fn format_compact_num(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    } else if n >= 10_000 {
        format!("{:.1}k", n as f64 / 1_000.0)
    } else {
        n.to_string()
    }
}

fn format_dist_m(d: f64) -> String {
    if d >= 1000.0 {
        format!("{:.1}km", d / 1000.0)
    } else {
        format!("{:.1}m", d)
    }
}

/// 选中按键的按压次数合计
/// 说明页正文大小 = 界面「小号字」的 1.5 倍
fn help_font_size(ui: &egui::Ui) -> f32 {
    let small = ui
        .style()
        .text_styles
        .get(&egui::TextStyle::Small)
        .map(|f| f.size)
        .unwrap_or(9.0);
    (small * 1.5).round()
}

/// 说明页的小节标题
fn help_section(ui: &mut egui::Ui, title: &str, size: f32) {
    ui.add_space(3.0);
    ui.label(egui::RichText::new(title).size(size * 1.15).strong().color(SEL_COLOR));
    ui.add_space(4.0);
}

/// 说明页的一条：`1. 名称：说明`（名称部分加粗；没有冒号就把整条当说明）。
/// 用 `horizontal_wrapped` 让它在窄窗口下也能折行，不横向裁掉文字。
fn help_item(ui: &mut egui::Ui, no: usize, text: &str, size: f32, colon: &str) {
    // 名称 = 第一个冒号之前的部分（中文全角 / 英文半角+空格两种写法都认）
    let (head, body) = match text.split_once('：').or_else(|| text.split_once(": ")) {
        Some((h, b)) => (Some(h), b),
        None => (None, text),
    };
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 5.0;
        ui.label(egui::RichText::new(format!("{no}.")).size(size).strong().color(TEXT));
        if let Some(h) = head {
            let head = format!("{h}{colon}");
            ui.label(egui::RichText::new(head).size(size).strong().color(TEXT));
        }
        ui.label(egui::RichText::new(body).size(size).color(TEXT));
    });
    ui.add_space(4.0);
}

fn sum_selected(unit: &BucketStats, selected: &std::collections::BTreeSet<usize>) -> u64 {
    selected.iter().map(|&i| unit.key_count(i)).sum()
}

/// 解析 KMCOUNTER_FAKE_HUD_CLICK：形如 "double@8,right@20" 的逗号分隔列表，
/// 每项是「动作@秒」，省略秒数默认 10 秒。
fn parse_fake_hud_jobs(spec: &str) -> Vec<(String, u64)> {
    let mut jobs: Vec<(String, u64)> = Vec::new();
    for part in spec.split(',') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        let (kind, secs) = part.split_once('@').unwrap_or((part, "10"));
        jobs.push((kind.to_lowercase(), secs.parse().unwrap_or(10)));
    }
    jobs
}

/// 按时间表模拟点击悬浮窗：double = 双击呼出主窗口，其余 = 右键（推设置事件）。
fn run_fake_hud_jobs(
    jobs: Vec<(String, u64)>,
    shared: &Arc<Shared>,
    ctx: &egui::Context,
    keep_on_top: bool,
    events: &Arc<parking_lot::Mutex<Vec<hud::HudEvent>>>,
) {
    let start = std::time::Instant::now();
    for (kind, secs) in jobs {
        let target = std::time::Duration::from_secs(secs);
        if target > start.elapsed() {
            std::thread::sleep(target - start.elapsed());
        }
        match kind.as_str() {
            "double" => {
                log::info!("调试：模拟双击悬浮窗 (@{secs}s)");
                crate::gui::window::show_window_from_hud(shared, ctx, keep_on_top);
            }
            // 右键 = 只推事件（与 HUD 回调里完全同一条路）
            _ => {
                log::info!("调试：模拟右键悬浮窗 (@{secs}s)");
                events.lock().push(hud::HudEvent::OpenSettings);
                shared.ui_hidden.store(true, Ordering::Relaxed);
                ctx.request_repaint();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thousands_formatting() {
        assert_eq!(fmt_thousands(0.0), "0");
        assert_eq!(fmt_thousands(999.0), "999");
        assert_eq!(fmt_thousands(12345.6), "12,346");
        assert_eq!(fmt_thousands(1234567.0), "1,234,567");
        assert_eq!(fmt_thousands(-4200.0), "-4,200");
    }

    #[test]
    fn selected_sum() {
        let mut day = BucketStats::default();
        day.keys[32] = 7; // T
        day.keys[33] = 5; // U
        let mut sel = std::collections::BTreeSet::new();
        sel.insert(32);
        sel.insert(33);
        assert_eq!(sum_selected(&day, &sel), 12);
        sel.remove(&32);
        assert_eq!(sum_selected(&day, &sel), 5);
        // 越界索引不应 panic
        let mut bad = std::collections::BTreeSet::new();
        bad.insert(usize::MAX);
        assert_eq!(sum_selected(&day, &bad), 0);
    }

    /// 排行聚合：区间 / 钟点 / 键位范围 / 只看已选 都要生效
    #[test]
    fn rank_rows_filter_and_sum() {
        use crate::keys::idx_by_name;
        let mut store = Store::default();
        // 三天数据：Space 每天 10 次；F1 第一天 5 次；Num1 第二天 7 次；鼠标左键第三天 3 次
        for (day, name, n) in [
            ("20260601", "Space", 10u64),
            ("20260602", "Space", 10),
            ("20260603", "Space", 10),
            ("20260601", "F1", 5),
            ("20260602", "Num1", 7),
        ] {
            for _ in 0..n {
                store.bump_key(day, 12, idx_by_name(name).unwrap());
            }
        }
        for _ in 0..3 {
            store.bump_mouse("20260603", 12, &crate::stats::MouseEv::LeftUp);
        }

        let s = crate::lang::LANG_CHOICES; // 只是占位，下面用中文表
        let _ = s;
        let strings = crate::lang::strings(crate::lang::Lang::Zh);
        let d = |x: &str| chrono::NaiveDate::parse_from_str(x, "%Y%m%d").unwrap();
        // 键位范围现在是多选集合
        let all: std::collections::BTreeSet<RankScope> = ALL_SCOPES.iter().copied().collect();
        let only = |z: RankScope| -> std::collections::BTreeSet<RankScope> { [z].into_iter().collect() };

        // ① 三天全看：Space 30 排第一，合计 30 + 5 + 7 = 42（+ 鼠标左键 3）
        let rows = rank_rows(&store, d("20260601"), d("20260603"), None, &all, None, strings);
        let space = rows.iter().find(|r| r.label == "Space").unwrap();
        assert_eq!(space.count, 30);
        assert_eq!(space.key_idx, Some(idx_by_name("Space").unwrap()));
        let total: u64 = rows.iter().map(|r| r.count).sum();
        assert_eq!(total, 45, "30 键盘(Space) + 5(F1) + 7(Num1) + 3(鼠标左键)");
        assert!((space.share - 30.0 / 45.0).abs() < 1e-9, "占比按合计算");
        assert!((space.per_day - 10.0).abs() < 1e-9, "均价按区间天数算");
        // 鼠标行在，且标了 mouse 标志（配色用绿系）
        let lb = rows.iter().find(|r| r.mouse && r.count == 3).expect("应有鼠标左键行");
        assert_eq!(lb.key_idx, None, "鼠标行没有布局键下标");

        // ② 只第一天
        let rows = rank_rows(&store, d("20260601"), d("20260601"), None, &all, None, strings);
        assert_eq!(rows.iter().find(|r| r.label == "Space").unwrap().count, 10);
        assert_eq!(rows.iter().find(|r| r.label == "F1").unwrap().count, 5);
        assert_eq!(rows.iter().find(|r| r.label == "Num1").unwrap().count, 0);

        // ③ 只看钟点：12 时有数据、13 时没有
        let rows = rank_rows(&store, d("20260601"), d("20260603"), Some(12), &all, None, strings);
        assert_eq!(rows.iter().find(|r| r.label == "Space").unwrap().count, 30);
        let rows = rank_rows(&store, d("20260601"), d("20260603"), Some(13), &all, None, strings);
        assert_eq!(rows.iter().map(|r| r.count).sum::<u64>(), 0, "13 时没有数据");

        // ④ 键位范围：功能键区只有 F 行；数字键区只有 Num*；鼠标只要鼠标行
        let rows = rank_rows(&store, d("20260601"), d("20260603"), None, &only(RankScope::Function), None, strings);
        assert_eq!(rows.len(), 13, "功能键区 = Esc + F1~F12");
        assert!(rows.iter().any(|r| r.label == "Esc") && rows.iter().any(|r| r.label == "F12"));
        assert_eq!(rows.iter().find(|r| r.label == "F1").unwrap().count, 5);
        let rows = rank_rows(&store, d("20260601"), d("20260603"), None, &only(RankScope::Numpad), None, strings);
        // 数字键区的 1 与主键盘的 1 标签相同，排行里要用内部名区分开
        assert_eq!(rows.iter().find(|r| r.label == "Num1").map(|r| r.count), Some(7));
        assert!(rows.iter().all(|r| r.label != "D1"), "数字键区不含主键盘的 1");
        let rows = rank_rows(&store, d("20260601"), d("20260603"), None, &only(RankScope::Mouse), None, strings);
        assert_eq!(rows.len(), 5, "鼠标范围 = 左/中/右/滚轮/侧键 5 行");
        assert!(rows.iter().all(|r| r.mouse));

        // ⑤ 多选：主键盘区 + 控制键区一起
        let two: std::collections::BTreeSet<RankScope> = [RankScope::Main, RankScope::Control].into_iter().collect();
        let rows = rank_rows(&store, d("20260601"), d("20260603"), None, &two, None, strings);
        let labels: Vec<&str> = rows.iter().map(|r| r.label.as_str()).collect();
        assert!(labels.contains(&"Space"), "主键盘区应包含 Space：{labels:?}");
        assert!(labels.contains(&"Insert") || labels.contains(&"Delete"), "控制键区应包含导航键：{labels:?}");
        assert!(!labels.contains(&"Num1"), "没选数字键区就不该有 Num1");
        assert!(!labels.contains(&"F1"), "没选功能键区就不该有 F1");
        assert!(rows.iter().all(|r| !r.mouse), "没选鼠标就不该有鼠标行");

        // ⑥ 只看已选：只剩选中的那个键
        let sel: std::collections::BTreeSet<usize> = [idx_by_name("F1").unwrap()].into_iter().collect();
        let rows = rank_rows(&store, d("20260601"), d("20260603"), None, &all, Some(&sel), strings);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].label, "F1");
        assert_eq!(rows[0].count, 5);
    }

    /// 区间要对齐到自然周期：当周 = 所在那一周（周一到周日）、当月 = 自然月、当年 = 自然年。
    /// （之前的写法把锚点当起点，窗口落到了「锚点 → 未来」，所以当周/当月/当年看起来没效果）
    #[test]
    fn rank_bucket_range_is_calendar_aligned() {
        let d = |x: &str| chrono::NaiveDate::parse_from_str(x, "%Y%m%d").unwrap();
        // 2026-10-06 是周二
        let anchor = d("20261006");
        let (f, t) = bucket_range(Granularity::Weekly.bucket_start(anchor), Granularity::Weekly);
        assert_eq!((f, t), (d("20261005"), d("20261011")), "当周应是 10-05(周一)~10-11(周日)");
        let (f, t) = bucket_range(Granularity::Monthly.bucket_start(anchor), Granularity::Monthly);
        assert_eq!((f, t), (d("20261001"), d("20261031")), "当月应是整个自然月");
        let (f, t) = bucket_range(Granularity::Yearly.bucket_start(anchor), Granularity::Yearly);
        assert_eq!((f, t), (d("20260101"), d("20261231")), "当年应是整个自然年");
        let (f, t) = bucket_range(Granularity::Daily.bucket_start(anchor), Granularity::Daily);
        assert_eq!((f, t), (anchor, anchor), "当日就是那一天");
    }

    /// 配色规格：键盘=红系、鼠标按键=绿系、鼠标移动=蓝系
    #[test]
    fn series_colors_match_spec() {
        assert!(KEY_COLOR.r() > KEY_COLOR.g() && KEY_COLOR.r() > KEY_COLOR.b(), "键盘应是红系");
        assert!(CLICK_COLOR.g() > CLICK_COLOR.r() && CLICK_COLOR.g() > CLICK_COLOR.b(), "鼠标按键应是绿系");
        assert!(MOUSE_COLOR.b() > MOUSE_COLOR.r() && MOUSE_COLOR.b() > MOUSE_COLOR.g(), "鼠标移动应是蓝系");
    }

    /// 鼠标五键是独立计数池：量程只看这五个键；热力色用绿色系，与键盘的红系区分
    #[test]
    fn mouse_keys_use_own_green_pool() {
        let m = crate::stats::MouseStats { lb: 10, rb: 20, mb: 30, xb: 40, wheel: 50, hwheel: 999, move_px: 0.0 };
        // 取值顺序：左 / 中 / 右 / 滚轮 / 侧键（横滚不参与）
        let vals: Vec<f64> = MOUSE_KEYS.iter().map(|k| (k.value)(&m)).collect();
        assert_eq!(vals, vec![10.0, 30.0, 20.0, 50.0, 40.0]);
        assert_eq!(mouse_key_pool(&m), 50.0, "池子取五个键的最大值（横滚 999 不算）");

        // 绿色系（最深）与键盘的红色系（最深）互不相同
        let deep_mouse = mouse_heat_color(50.0, 50.0);
        let deep_kb = heat_color(100.0, 100.0);
        assert!(deep_mouse.g() > deep_mouse.r(), "鼠标键最深色应是绿色系");
        assert!(deep_kb.r() > deep_kb.g(), "键盘最深色应是红色系");
        // 池子独立：键盘最多 100、滚轮 5000 时，各自都能到自己的最深色
        assert_eq!(mouse_heat_color(5000.0, 5000.0), deep_mouse);
        assert_eq!(heat_color(100.0, 100.0), deep_kb);
        // 0 值 → 最浅底色；颜色随计数变深
        assert_eq!(mouse_heat_color(0.0, 50.0), Color32::from_rgb(0xEE, 0xEE, 0xEE));
        let mid = mouse_heat_color(25.0, 50.0);
        // 渐变是「浅底色 → 深绿」，所以量程一半的颜色应比最深色浅（各通道更亮）
        assert!(mid.g() > deep_mouse.g() && mid.r() > deep_mouse.r(), "半量应比最深色浅");
        assert_ne!(mid, Color32::from_rgb(0xEE, 0xEE, 0xEE), "半量不该还是底色");
    }

    #[test]
    fn hour_bar_index_maps_x_to_hour() {
        let area = Rect::from_min_size(Pos2::new(100.0, 20.0), Vec2::new(240.0, 40.0)); // 每格 10px
        assert_eq!(hour_bar_index(area, 100.0), Some(0));
        assert_eq!(hour_bar_index(area, 109.9), Some(0));
        assert_eq!(hour_bar_index(area, 110.0), Some(1));
        assert_eq!(hour_bar_index(area, 339.9), Some(23));
        assert_eq!(hour_bar_index(area, 340.1), None); // 右边界外
        assert_eq!(hour_bar_index(area, 99.0), None);
        // 零宽度不应 panic / 误判
        let empty = Rect::from_min_size(Pos2::ZERO, Vec2::new(0.0, 10.0));
        assert_eq!(hour_bar_index(empty, 0.0), None);
    }

    #[test]
    fn hour_nav_cycles_all_day_and_hours() {
        // 直接验证循环顺序：全天 → 0 → 1 … → 23 → 全天
        let step = |cur: Option<u8>, prev: bool| match (cur, prev) {
            (None, true) => Some(23),
            (None, false) => Some(0),
            (Some(0), true) => None,
            (Some(h), true) => Some(h - 1),
            (Some(23), false) => None,
            (Some(h), false) => Some(h + 1),
        };
        assert_eq!(step(None, false), Some(0));
        assert_eq!(step(Some(0), true), None);
        assert_eq!(step(Some(23), false), None);
        assert_eq!(step(Some(23), true), Some(22));
        assert_eq!(step(None, true), Some(23));
    }

    #[test]
    fn ppm_encoding() {
        let img = egui::ColorImage {
            size: [2, 1],
            pixels: vec![Color32::from_rgb(255, 0, 0), Color32::from_rgb(0, 255, 0)],
        };
        let dir = std::env::temp_dir().join(format!("kmc_ppm_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("t.ppm");
        save_ppm(path.to_str().unwrap(), &img).unwrap();
        let bytes = std::fs::read(&path).unwrap();
        assert_eq!(&bytes[..3], b"P6\n");
        assert!(bytes.starts_with(b"P6\n2 1\n255\n"), "头部错误");
        assert!(bytes.ends_with(&[255, 0, 0, 0, 255, 0]), "像素序错误");
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn hud_mode_toggle_state() {
        let ctx = egui::Context::default();
        let tmp = std::env::temp_dir().join(format!("kmc_hud_test_{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        let cfg_path = tmp.join("config.toml");
        let stats_path = tmp.join("stats.json");
        let show_request_path = tmp.join("show_request");
        let cfg = Config::default();
        let cfg_shared = Arc::new(parking_lot::Mutex::new(cfg));
        let shared = Shared::new(Store::default(), 0, 0);
        let autostart_state = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let mut app = App::new(
            &ctx,
            shared,
            cfg_shared,
            cfg_path,
            stats_path,
            show_request_path,
            false,
            None,
            autostart_state,
        );
        assert!(!app.hud_apm_show);
        app.debug_toggle_hud();
        assert!(app.hud_apm_show);
        app.debug_toggle_hud();
        assert!(!app.hud_apm_show);
        std::fs::remove_dir_all(tmp).ok();
    }
}

/// 以 PPM(P6) 保存帧缓冲：零依赖的调试截图格式，可用 ImageMagick 转 PNG。
/// 忽略 alpha（窗口内容不透明；Color32 为预乘存储，透明确实会偏暗，调试无碍）。
fn save_ppm(path: &str, img: &egui::ColorImage) -> std::io::Result<()> {
    use std::io::Write;
    let mut f = std::io::BufWriter::new(std::fs::File::create(path)?);
    write!(f, "P6\n{} {}\n255\n", img.size[0], img.size[1])?;
    let mut buf = Vec::with_capacity(img.pixels.len() * 3);
    for px in &img.pixels {
        // 截图不保留 alpha，直接取 RGB
        buf.extend_from_slice(&[px.r(), px.g(), px.b()]);
    }
    f.write_all(&buf)?;
    f.flush()
}
