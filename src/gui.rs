//! 主界面：键盘热力图 + 统计列表 + 历史翻页 + 趋势图表 + 设置窗口。
//! 配色复刻原版：浅色统计窗（背景 #EEEEEE、文字 #575757），
//! 热力渐变 #EEEEEE → #B26C65（莫兰迪红），深色设置窗（#444444）。

use crate::chart;
use crate::config::Config;
use crate::keys::{KEYS, MAIN_UNITS, NAV_UNITS, NUM_UNITS, MAIN_ROWS};
use crate::lang::{strings, Lang, Strings};
use crate::stats::{BucketStats, DayEntry, Shared, Store};
use crate::tray::Tray;
use crate::trend::{self, Granularity};
use chrono::{Datelike, NaiveDate, Timelike};
use eframe::egui;
use egui::{Align2, Color32, FontId, Pos2, Rect, Sense, Shape, Vec2};
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};

pub(crate) const BG: Color32 = Color32::from_rgb(0xEE, 0xEE, 0xEE);
const TEXT: Color32 = Color32::from_rgb(0x57, 0x57, 0x57);
const HEAT_FROM: (u8, u8, u8) = (0xEE, 0xEE, 0xEE);
const HEAT_TO: (u8, u8, u8) = (0xB2, 0x6C, 0x65);
/// 趋势曲线配色：键盘（蓝灰）、鼠标移动距离（莫兰迪红）
const KEY_COLOR: Color32 = Color32::from_rgb(0x6B, 0x8B, 0xA4);
const MOUSE_COLOR: Color32 = Color32::from_rgb(0xB2, 0x6C, 0x65);
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

pub struct App {
    shared: Arc<Shared>,
    cfg_path: PathBuf,
    cfg: Config,
    lang: Lang,
    /// 底部“展开更多”面板是否展开（展开后同时显示统计与设置）
    more_open: bool,
    /// 调试：启动后最小化一次
    start_minimized: bool,
    last_applied_h: f32,
    /// 待达成的窗口高度（早期 resize 请求可能被 WM 丢弃，需按实际尺寸校验重发）
    resize_target: Option<f32>,
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
    /// 用户手动调整过窗口大小后不再自动贴合内容
    user_sized: bool,
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
    /// 分时视图：None = 整天/总计，Some(h) = 只看 0-23 时
    hour_sel: Option<u8>,
    /// 上次刷新快照的时间（带小时明细后 store 更大，按需节流克隆）
    last_snapshot: Instant,
    /// 分时分布条在屏幕上的矩形（本帧绘制时记录，供离屏交互测试使用）
    strip_rect: Rect,
    /// 趋势图区域（点击跳转日期时用来换算桶位置，测试也会读）
    chart_rect: Rect,
    /// 视图日期列表缓存：[None=总计, 今日, 有数据的天…]（快照刷新时重建）
    view_days: Vec<Option<String>>,
    /// 日历弹窗是否展开
    cal_open: bool,
    /// 日历当前显示的月份 (年, 月)
    cal_month: (i32, u32),
    /// 日期标题的矩形（日历定位用）
    nav_title_rect: Rect,
}

/// 窗口尺寸管理：底部展开 设置/统计 面板时向下加高窗口（按钮位置保持不动）
const BASE_W: f32 = 1260.0;
const BASE_H: f32 = 500.0;
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
        // 字体
        let mut defs = egui::FontDefinitions::default();
        let has_cjk = crate::fonts::install_cjk(&mut defs, &cfg.font_path);
        egui_ctx.set_fonts(defs);

        let lang = Lang::resolve(&cfg.language).downgrade_if_no_cjk(has_cjk);
        egui_ctx.set_visuals(egui::Visuals::light());
        egui_ctx.style_mut(|s| {
            s.spacing.item_spacing = Vec2::new(8.0, 6.0);
        });

        // 托盘命令交给独立控制线程处理（最小化时渲染帧会停止，不能在渲染循环里读命令）
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
                    autostart: autostart_state.clone(),
                    flags: flags.clone(),
                    tray_ctl: Some(t.tx),
                },
                t.rx,
                egui_ctx.clone(),
                show_request_path.clone(),
            );
        }
        // 调试：KMCOUNTER_SWITCH_PAGE_AFTER_SECS=N → N 秒后自动切换到另一个页面（验证两页布局一致性）
        if let Ok(secs) = std::env::var("KMCOUNTER_SWITCH_PAGE_AFTER_SECS").map(|v| v.parse::<u64>()) {
            if let Ok(secs) = secs {
                let ctx = egui_ctx.clone();
                let flags = flags.clone();
                flags.switch_page.store(true, Ordering::Relaxed); // 由渲染循环执行切换
                std::thread::spawn(move || {
                    std::thread::sleep(std::time::Duration::from_secs(secs));
                    flags.switch_page_after.store(true, Ordering::Relaxed);
                    ctx.request_repaint();
                });
            }
        }

        // 调试：KMCOUNTER_FAKE_CLOSE_AFTER_SECS=N → N 秒后模拟“点窗口关闭按钮”
        //（发送 ViewportCommand::Close，egui-winit 会转成 close_requested 事件，与真实点 × 同路径）
        if let Ok(list) = std::env::var("KMCOUNTER_FAKE_CLOSE_AFTER_SECS") {
            let secs: Vec<u64> = list.split(',').filter_map(|s| s.trim().parse().ok()).collect();
            if !secs.is_empty() {
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
        }

        // 调试用：启动时预展开底部面板 / 指定页（KMCOUNTER_START_PANELS=stats|settings|both|trend）
        let start_panels = std::env::var("KMCOUNTER_START_PANELS").unwrap_or_default().to_lowercase();
        let more_open = start_panels.contains("stats") || start_panels.contains("settings") || start_panels.contains("both");
        let start_page = if start_panels.contains("trend") { Page::Trends } else { Page::Heatmap };
        let start_minimized = start_panels.contains("min");
        let snap = (*shared.store.lock()).clone();
        let snap_today = shared.today.lock().clone();
        let welcome = first_run && !cfg.welcome_shown;
        // 视图日期列表（初始）
        let mut view_days: Vec<Option<String>> = vec![None, Some(snap_today.clone())];
        for d in snap.days.keys().rev() {
            if d != &snap_today {
                view_days.push(Some(d.clone()));
            }
        }

        App {
            shared,
            cfg_path,
            welcome,
            has_cjk,
            tray_tx,
            lang,
            more_open,
            edit: cfg.clone(),
            tray_available,
            cfg_shared,
            flags,
            exiting: false,
            last_save: Instant::now(),
            snap,
            snap_today,
            // 调试：KMCOUNTER_VIEW_IDX=1 表示打开即看“今日”（截图/验证用）
            view_idx: std::env::var("KMCOUNTER_VIEW_IDX").ok().and_then(|v| v.trim().parse::<usize>().ok()).unwrap_or(0),
            title_dirty: true,
            page: start_page,
            // 调试：KMCOUNTER_GRAN=hour|day|week|month|year 指定趋势页起始粒度（截图用）
            granularity: match std::env::var("KMCOUNTER_GRAN").unwrap_or_default().to_lowercase().as_str() {
                "hour" | "hourly" => Granularity::Hourly,
                "week" | "weekly" => Granularity::Weekly,
                "month" | "monthly" => Granularity::Monthly,
                "year" | "yearly" => Granularity::Yearly,
                _ => Granularity::Daily,
            },
            // 调试：KMCOUNTER_HOUR=14 启动即选中 14 时（截图/验证用）
            hour_sel: std::env::var("KMCOUNTER_HOUR").ok().and_then(|v| v.trim().parse::<u8>().ok()).filter(|h| *h < 24),
            last_snapshot: Instant::now(),
            strip_rect: Rect::NOTHING,
            chart_rect: Rect::NOTHING,
            view_days,
            cal_open: std::env::var("KMCOUNTER_CAL").map(|v| v == "1").unwrap_or(false),
            cal_month: (chrono::Local::now().year(), chrono::Local::now().month()),
            nav_title_rect: Rect::NOTHING,
            start_minimized,
            last_applied_h: BASE_H,
            resize_target: None,
            resize_deadline: Instant::now(),
            measured_panel_h: 0.0,
            panel_bottom: 0.0,
            base_h: BASE_H,
            bottom_region_top: f32::MAX,
            content_bottom: 0.0,
            last_seen_h: 0.0,
            user_sized: false,
            screenshot_prefix: std::env::var("KMCOUNTER_SCREENSHOT").ok().filter(|s| !s.is_empty()),
            frame_count: 0,
            shots_taken: 0,
            selected: std::env::var("KMCOUNTER_SELECT")
                .ok()
                .map(|v| {
                    v.split(',')
                        .filter_map(|s| s.trim().parse::<usize>().ok())
                        .filter(|i| *i < KEYS.len())
                        .collect()
                })
                .unwrap_or_default(),
            cfg,
        }
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

    fn view_label(&self) -> String {
        if self.view_idx == 0 {
            self.s().total_label.to_string()
        } else {
            self.view_days
                .get(self.view_idx)
                .and_then(|o| o.clone())
                .unwrap_or_else(|| self.snap_today.clone())
        }
    }

    /// 跳转到某个日期（"YYYYMMDD"）；不在列表里（没数据）则不动
    fn goto_day(&mut self, date: &str) -> bool {
        match self.view_days.iter().position(|o| o.as_deref() == Some(date)) {
            Some(i) => {
                self.view_idx = i;
                self.title_dirty = true;
                true
            }
            None => false,
        }
    }

    fn nav_history(&mut self, newer: bool) {
        let len = self.view_days.len();
        if len <= 1 {
            return;
        }
        self.view_idx = if newer {
            if self.view_idx == 0 { len - 1 } else { self.view_idx - 1 }
        } else {
            (self.view_idx + 1) % len
        };
        self.title_dirty = true;
    }

    /// 翻页（◀ ▶ / 上下键 / 滚轮共用）：
    /// 趋势页在周/月/年粒度下按对应单位滚动（滚一格 = 一周/一月/一年），其余情况仍按天翻。
    fn nav_step(&mut self, newer: bool) {
        let target = match (self.page, self.granularity) {
            (Page::Trends, Granularity::Weekly) => Some(shift_days(self.anchor_date(), if newer { 7 } else { -7 })),
            (Page::Trends, Granularity::Monthly) => Some(shift_months(self.anchor_date(), 1, newer)),
            (Page::Trends, Granularity::Yearly) => Some(shift_months(self.anchor_date(), 12, newer)),
            _ => None,
        };
        match target {
            Some(t) => self.jump_anchor(t, newer),
            None => self.nav_history(newer),
        }
    }

    /// 把显示日期挪到 `target` 附近有数据的那天（该天没有数据就取最接近的一天）
    fn jump_anchor(&mut self, target: NaiveDate, newer: bool) {
        if self.view_days.len() <= 1 {
            return;
        }
        let today = NaiveDate::parse_from_str(&self.snap_today, "%Y%m%d")
            .unwrap_or_else(|_| chrono::Local::now().date_naive());
        let mut idx = if target > today {
            // 已经滚到未来：停在「总计」（它就是“从今天往前看”）
            if newer { 0 } else { 1 }
        } else {
            self.nearest_view_idx(target).unwrap_or(1)
        };
        if idx == self.view_idx {
            // 目标附近没有别的数据日（中间有缺口）：朝滚动方向再挪一格，避免“滚了没反应”
            idx = if newer {
                self.view_idx.saturating_sub(1)
            } else {
                (self.view_idx + 1) % self.view_days.len()
            };
            if idx == self.view_idx {
                return;
            }
        }
        self.view_idx = idx;
        self.title_dirty = true;
    }

    /// `view_days`（不含「总计」）里最接近 `target` 的那一项
    fn nearest_view_idx(&self, target: NaiveDate) -> Option<usize> {
        self.view_days
            .iter()
            .enumerate()
            .filter_map(|(i, d)| {
                let date = NaiveDate::parse_from_str(d.as_deref()?, "%Y%m%d").ok()?;
                Some((i, (date - target).num_days().abs()))
            })
            .min_by_key(|(_, dist)| *dist)
            .map(|(i, _)| i)
    }

    /// 点击趋势图上的点：跳到那一点所在的那天（小时粒度时同时选中那个小时）
    fn goto_trend_point(&mut self, date: NaiveDate, hour: Option<u8>) -> bool {
        let key = date.format("%Y%m%d").to_string();
        let on_this_day = self.view_idx != 0 && self.view_label() == key;
        if !on_this_day && !self.goto_day(&key) {
            return false; // 那天没有数据：不跳
        }
        self.hour_sel = hour;
        true
    }

    /// 点击趋势图上某个桶 → 跳到该桶里（或最接近的）有数据的那天
    fn goto_trend_bucket(&mut self, start: NaiveDate, g: Granularity) -> bool {
        let (lo, hi) = bucket_range(start, g);
        let idx = self
            .view_days
            .iter()
            .enumerate()
            .filter_map(|(i, d)| {
                let date = NaiveDate::parse_from_str(d.as_deref()?, "%Y%m%d").ok()?;
                // 桶内有数据 → 距离 0（优先）；桶外按到桶边缘的天数算
                let dist = if date < lo {
                    (lo - date).num_days()
                } else if date > hi {
                    (date - hi).num_days()
                } else {
                    0
                };
                Some((i, dist))
            })
            .min_by_key(|(_, dist)| *dist)
            .map(|(i, _)| i);
        match idx {
            Some(i) => {
                self.view_idx = i;
                self.hour_sel = None;
                self.title_dirty = true;
                true
            }
            None => false,
        }
    }

    fn refresh_snapshot(&mut self) {
        self.snap = (*self.shared.store.lock()).clone();
        self.snap_today = self.shared.today.lock().clone();
        self.rebuild_view_days();
        self.title_dirty = true;
    }

    fn save_all(&mut self) {
        let store = (*self.shared.store.lock()).clone();
        let stats_path = self.cfg_path.with_file_name("stats.json");
        if let Err(e) = store.save(&stats_path) {
            log::error!("保存统计数据失败: {e}");
        }
        let cfg = self.cfg_shared.lock().clone();
        if let Err(e) = cfg.save(&self.cfg_path) {
            log::error!("保存配置失败: {e}");
        }
        self.shared.dirty.store(false, Ordering::Relaxed);
        self.last_save = Instant::now();
    }

    fn current_metrics(&self) -> (f32, f32, f32, Vec2) {
        let l = &self.cfg.layout;
        let u = (l.key_w + l.key_spacing) as f32;
        let pitch = (l.key_h + l.key_spacing) as f32;
        let gap = l.h_spacing as f32;
        let nav_x = MAIN_UNITS * u + gap;
        let num_x = nav_x + NAV_UNITS * u + gap;
        let size = Vec2::new(
            num_x + NUM_UNITS * u - l.key_spacing as f32,
            MAIN_ROWS * pitch - l.key_spacing as f32,
        );
        (u, pitch, gap, size)
    }

    /// 按键画在哪一块：直接按分区决定（控制键区那两段都画在导航块，
    /// PrtScr / ScrollLock / Pause 就在它顶部一行）
    fn block_offset(&self, i: usize, u: f32, gap: f32) -> f32 {
        let nav_x = MAIN_UNITS * u + gap;
        match crate::keys::block_of(i) {
            crate::keys::Block::Main => 0.0,
            crate::keys::Block::Control => nav_x,
            crate::keys::Block::Numpad => nav_x + NAV_UNITS * u + gap,
        }
    }

    fn key_rect(&self, i: usize, rect: Rect, u: f32, pitch: f32, gap: f32) -> Rect {
        let k = &KEYS[i];
        let ox = self.block_offset(i, u, gap);
        Rect::from_min_size(
            Pos2::new(rect.left() + ox + k.x * u, rect.top() + k.y * pitch),
            Vec2::new(k.w * u - self.cfg.layout.key_spacing as f32, k.h * pitch - self.cfg.layout.key_spacing as f32),
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

    /// 小时选择：全天 ↔ 0-23 时循环
    fn nav_hour(&mut self, prev: bool) {
        self.hour_sel = match (self.hour_sel, prev) {
            (None, true) => Some(23),
            (None, false) => Some(0),
            (Some(0), true) => None,
            (Some(h), true) => Some(h - 1),
            (Some(23), false) => None,
            (Some(h), false) => Some(h + 1),
        };
    }

    /// 小时选择标题（“全天” / “14 时”）
    fn hour_title(&self, s: &'static Strings) -> String {
        match self.hour_sel {
            None => s.all_day.to_string(),
            Some(h) => hour_label(h, s),
        }
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

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.save_all();
    }
}

impl App {
    /// 一帧的界面与窗口逻辑。与 eframe 解耦，便于离屏软渲染出图（见 softshot 模块）。
    pub(crate) fn draw(&mut self, ctx: &egui::Context) {
        let s = self.s();

        self.frame_count += 1;

        // ---- 同步共享配置（托盘控制线程可能已修改，如开机启动）----
        self.cfg = self.cfg_shared.lock().clone();

        // 调试：启动后收起窗口（KMCOUNTER_START_PANELS 含 "min"；首帧执行，保证命令能被处理）
        if self.start_minimized && self.frame_count == 1 {
            self.start_minimized = false;
            log::info!("调试：启动后收起窗口");
            hide_window(&self.shared, ctx);
        }

        // 调试：定时切页
        if self.flags.switch_page_after.swap(false, Ordering::Relaxed) {
            self.page = if self.page == Page::Heatmap { Page::Trends } else { Page::Heatmap };
            log::info!("调试：切换到 {:?}", if self.page == Page::Heatmap { "热力图" } else { "趋势" });
        }

        // ---- 托盘请求：打开设置（控制线程只置标志，UI 状态在渲染线程改）----
        if self.flags.want_settings.swap(false, Ordering::Relaxed) {
            self.edit = self.cfg.clone();
            self.more_open = true;
        }
        // ---- 托盘呼出窗口：视图回到「今日」（带小时也一起回到全天）----
        if self.flags.show_today.swap(false, Ordering::Relaxed) {
            self.view_idx = 1; // [0]=总计 [1]=今日
            self.hour_sel = None;
            self.title_dirty = true;
        }

        // ---- 定时任务：跨夜滚动 / 清理 / 保存 ----
        if self.shared.rollover_if_needed() {
            let days = self.cfg.history_days_clamped();
            self.shared.store.lock().purge(days);
            self.snap_today = self.shared.today.lock().clone();
            self.title_dirty = true;
        }
        if self.shared.dirty.load(Ordering::Relaxed) && self.last_save.elapsed() > Duration::from_secs(30) {
            self.save_all();
        }

        // ---- 快照刷新（节流：带分时明细后 store 明显变大，避免每帧整份克隆）----
        let today_changed = self.snap_today != *self.shared.today.lock();
        if (self.shared.dirty.load(Ordering::Relaxed) && self.last_snapshot.elapsed() > Duration::from_millis(400))
            || today_changed
        {
            self.refresh_snapshot();
            self.last_snapshot = Instant::now();
        }

        // ---- 窗口关闭：按配置收起窗口（继续后台统计）或直接退出 ----
        let close_requested = ctx.input(|i| i.viewport().close_requested());
        if close_requested && !self.exiting && self.tray_available && self.cfg.close_to_tray {
            if crate::session::session_ending_fresh() {
                // 注销/关机流程里的关闭请求：直接退出，别收起窗口把它卡住
                log::info!("会话正在结束，本次关闭按退出处理");
                self.exiting = true;
            } else {
                hide_window(&self.shared, ctx);
                crate::session::mark_close_cancelled();
                ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            }
        }
        if self.exiting {
            self.save_all();
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            return;
        }



        // ---- 标题 ----
        if self.title_dirty {
            self.title_dirty = false;
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(format!(
                "{} v{} | {} - {}",
                s.app_title,
                env!("CARGO_PKG_VERSION"),
                s.view_date_prefix,
                self.view_label()
            )));
        }

        // ---- 窗口高度：底部面板展开时向下加高，按钮与内容位置不变 ----
        // 识别用户手动缩放：无待处理 resize 时窗口高度自己变了 → 尊重用户，不再自动贴合
        let actual_h = ctx.input(|i| i.viewport().inner_rect.map(|r| r.height()));
        if self.resize_target.is_none() && !self.user_sized {
            if let (Some(h), true) = (actual_h, self.last_seen_h > 100.0) {
                if (h - self.last_seen_h).abs() > 3.0 {
                    self.user_sized = true;
                    log::debug!("检测到用户手动调整窗口大小，停止自动贴合内容");
                }
            }
        }
        if let Some(h) = actual_h {
            self.last_seen_h = h;
        }
        if self.user_sized {
            // 用户手动定过尺寸：收起时以实际高度为准
            if self.panel_extra_h() <= 0.0 && self.resize_target.is_none() && !self.shared.ui_hidden.load(Ordering::Relaxed) {
                if let Some(h) = actual_h {
                    if h > 100.0 {
                        self.base_h = h;
                    }
                }
            }
        } else if self.page == Page::Heatmap && self.content_bottom > 100.0 {
            // 热力图页内容尺寸固定：锚点直接由内容算出 → 按钮始终紧贴内容，无留白
            self.base_h = self.content_bottom + CONTROLS_H + 8.0;
        }
        let desired_h = if self.more_open {
            // 实测底部优先：字体/DPI/行高变化时也保证面板底边完整可见
            (self.base_h + self.panel_extra_h()).max(self.panel_bottom + PANEL_BOTTOM_PAD)
        } else {
            self.base_h
        };
        if (self.last_applied_h - desired_h).abs() > 0.5 {
            // 展开状态或实测高度变化：设定目标，按实际尺寸校验并重发
            self.last_applied_h = desired_h;
            self.resize_target = Some(desired_h);
            self.resize_deadline = Instant::now() + Duration::from_secs(3);
        }
        if let Some(target) = self.resize_target {
            let (actual_h, cur_w) = ctx.input(|i| {
                let r = i.viewport().inner_rect;
                (r.map(|r| r.height()), r.map(|r| r.width()).unwrap_or(BASE_W))
            });
            if actual_h.map_or(false, |h| (h - target).abs() <= 2.0) || Instant::now() > self.resize_deadline {
                self.resize_target = None; // 已达成，或 WM 拒绝（放弃，避免与用户手动缩放打架）
            } else {
                ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(Vec2::new(cur_w.max(960.0), target)));
            }
        }

        // ---- 历史翻页：翻页键/上下键/滚轮（仅统计窗口激活时）----
        let focused = ctx.input(|i| i.focused);
        let (up, down, wheel_y) = ctx.input(|i| {
            (
                i.key_pressed(egui::Key::ArrowUp) || i.key_pressed(egui::Key::PageUp),
                i.key_pressed(egui::Key::ArrowDown) || i.key_pressed(egui::Key::PageDown),
                i.raw_scroll_delta.y,
            )
        });
        // 指针悬停在底部区域（按钮行/展开面板）时不翻日期，避免与设置控件冲突
        let pointer_in_bottom = ctx
            .input(|i| i.pointer.hover_pos())
            .map_or(false, |p| p.y >= self.bottom_region_top);
        if focused && !pointer_in_bottom && !self.cal_open {
            if up {
                self.nav_step(true);
            }
            if down {
                self.nav_step(false);
            }
            if wheel_y > 0.0 {
                self.nav_step(true);
            } else if wheel_y < 0.0 {
                self.nav_step(false);
            }
        }

        // ---- 界面 ----
        egui::CentralPanel::default()
            .frame(egui::Frame::central_panel(&ctx.style()).fill(BG))
            .show(ctx, |ui| {
                // 面板内容区起点（用于把底部区域锚定到 base_h，与当前窗口实际高度无关）
                let content_top = ui.cursor().min.y;
                // 本帧开始时的可用宽度：用于展开面板的左右分栏。
                // 注意不能用分栏处的 available_width——热力图页的固定尺寸内容会把它撑大，
                // 与趋势页不一致，导致设置栏左右移动
                let panel_area_w = ui.available_width();

                // 页签：热力图 / 趋势（开关按钮固定在窗口底部，不在此处）
                ui.horizontal(|ui| {
                    ui.selectable_value(&mut self.page, Page::Heatmap, s.tab_heatmap);
                    ui.selectable_value(&mut self.page, Page::Trends, s.tab_trend);
                });

                // 日期/分时导航（热力图页与趋势页共用）
                self.nav_row(ui, s);

                // 提示行只在热力图页显示（涉及按键选择，趋势页用不到）
                if self.page == Page::Heatmap {
                    // 分时摘要：选中小时时先说明该小时的键盘/鼠标量
                    let unit = self.unit_for_view();
                    let unit_t = s.unit_times.trim();
                    let mut head = String::new();
                    if unit.invalid {
                        head = s.hour_invalid.to_string();
                    } else if let Some(h) = self.hour_sel {
                        head = format!(
                            "{}{} · {} {} {} · {} {:.2} {}",
                            h,
                            s.hour_suffix,
                            s.keystrokes,
                            unit.keystrokes,
                            unit_t,
                            s.mouse_move,
                            self.shared.px_to_meters(unit.mouse.move_px),
                            s.unit_m
                        );
                    }
                    // 提示行：有选中按键时显示各键按压次数与合计，否则显示操作提示
                    if !self.selected.is_empty() {
                        let mut parts: Vec<String> = Vec::new();
                        for &i in &self.selected {
                            let k = &KEYS[i];
                            let label = if k.label.is_empty() { k.name } else { k.label };
                            let n = unit.key_count(i);
                            parts.push(if unit_t.is_empty() { format!("{label} {n}") } else { format!("{label} {n} {unit_t}") });
                        }
                        let sum = sum_selected(&unit, &self.selected);
                        let sum_text = if unit_t.is_empty() {
                            format!("{} {}", s.sel_sum, sum)
                        } else {
                            format!("{} {} {}", s.sel_sum, sum, unit_t)
                        };
                        let body = format!("{}：{} · {}", s.sel_prefix, parts.join(" · "), sum_text);
                        let text = if head.is_empty() { body } else { format!("{head} · {body}") };
                        ui.label(egui::RichText::new(text).small().color(KEY_COLOR));
                    } else {
                        // 数据不足时合并提示
                        let enough = unit.keystrokes >= if self.hour_sel.is_some() { HOUR_ENOUGH_KEYS } else { 100 };
                        let body = if enough {
                            format!("{} · {}", s.nav_hint, s.sel_hint)
                        } else {
                            format!("{} · {} · {}", s.not_enough_hint, s.nav_hint, s.sel_hint)
                        };
                        let text = if head.is_empty() { body } else { format!("{head} · {body}") };
                        ui.label(egui::RichText::new(text).small().color(TEXT));
                    }
                }

                // 欢迎横幅（仅首次运行）
                if self.welcome {
                    ui.add_space(2.0);
                    egui::Frame::group(ui.style())
                        .fill(Color32::from_rgb(0xFF, 0xF6, 0xD9))
                        .stroke(egui::Stroke::new(1.0_f32, Color32::from_rgb(0xE6, 0xC9, 0x7A)))
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                ui.strong(egui::RichText::new(s.welcome_main).color(TEXT));
                                if ui.button("×").clicked() {
                                    self.welcome = false;
                                    let mut cfg = self.cfg_shared.lock();
                                    cfg.welcome_shown = true;
                                    let _ = cfg.save(&self.cfg_path);
                                }
                            });
                            ui.label(egui::RichText::new(s.welcome_sub).color(TEXT));
                        });
                }

                // 输入捕获警告
                if !self.shared.input_ok.load(Ordering::Relaxed) {
                    ui.add_space(2.0);
                    egui::Frame::group(ui.style())
                        .fill(Color32::from_rgb(0xFB, 0xE3, 0xE1))
                        .stroke(egui::Stroke::new(1.0_f32, Color32::from_rgb(0xB2, 0x6C, 0x65)))
                        .show(ui, |ui| {
                            let msg = format!("{}{}", s.input_error, self.shared.input_msg.lock());
                            ui.label(egui::RichText::new(msg).color(TEXT));
                            #[cfg(target_os = "linux")]
                            ui.label(egui::RichText::new(s.input_error_linux_hint).small().color(TEXT));
                        });
                }

                ui.add_space(4.0);

                match self.page {
                    Page::Heatmap => {
                        // ---- 热力图（位置固定，不会被展开面板推走；点击按键可选中查看计数）----
                        let (u, pitch, gap, size) = self.current_metrics();
                        let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
                        self.paint_heatmap(ui, rect, resp, u, pitch, gap);
                    }
                    Page::Trends => {
                        self.show_trend_page(ui, s, content_top);
                    }
                }

                // 把底部按钮行推到「收起高度」的底部：布局与当前窗口实际高度无关，
                // 因此 resize 尚未落地时也不会跳动（消除展开瞬间的一帧闪屏）
                self.content_bottom = ui.cursor().min.y;
                let remaining = (content_top + self.base_h - CONTROLS_H - self.content_bottom).max(0.0);
                ui.add_space(remaining);
                self.bottom_region_top = ui.cursor().min.y;

                // ---- 底部固定按钮行 ----
                ui.horizontal(|ui| {
                    let label = if self.more_open { s.more_collapse } else { s.more_label };
                    // 固定宽度：避免“展开更多/收起”文字长度不同导致退出按钮左右移动
                    if ui
                        .add_sized(
                            [MORE_BTN_W, 20.0],
                            egui::SelectableLabel::new(self.more_open, egui::RichText::new(label)),
                        )
                        .clicked()
                    {
                        self.more_open = !self.more_open;
                        if self.more_open {
                            self.edit = self.cfg.clone();
                        }
                    }
                    if ui.button(s.menu_exit).clicked() {
                        self.exiting = true;
                    }
                });

                // ---- 按钮下方的展开面板（窗口向下生长，按钮位置不变）----
                if self.more_open {
                    let before = ui.cursor().min.y;
                    // 左右两栏：统计在左、设置在右，宽度按窗口宽度固定分配（与页面无关，位置稳定）
                    // 两列宽度之和 + 列间距 = 面板可用宽度：右缘与上方图表/卡片严格对齐
                    // 统计表 3~4 列够用即可，其余留给“设置”（设置内部改成三栏，不再又窄又长）
                    let left_w = (panel_area_w * 0.38 - 4.0).max(330.0);
                    let right_w = (panel_area_w - left_w - 8.0).max(340.0);
                    geo_log("panel area", Rect::from_min_size(Pos2::ZERO, Vec2::new(panel_area_w, 0.0)));
                    geo_log("left col alloc", Rect::from_min_size(Pos2::new(0.0, 0.0), Vec2::new(left_w, 0.0)));
                    geo_log("right col alloc", Rect::from_min_size(Pos2::new(left_w + 8.0, 0.0), Vec2::new(right_w, 0.0)));
                    ui.horizontal_top(|ui| {
                        ui.allocate_ui(Vec2::new(left_w, 0.0), |ui| {
                            self.show_stats_list(ui, s);
                        });
                        // 列间距用 item_spacing，不再额外 add_space（否则两列会整体右移）
                        ui.allocate_ui(Vec2::new(right_w, 0.0), |ui| {
                            self.show_settings_section(ui, s);
                        });
                    });
                    let measured = ui.cursor().min.y - before;
                    self.panel_bottom = ui.cursor().min.y;
                    // 实测高度回填，下一帧按实际内容调整窗口高度
                    if (self.measured_panel_h - measured).abs() > 1.0 {
                        self.measured_panel_h = measured;
                        self.last_applied_h = 0.0; // 强制下一帧重算窗口高度
                    }
                } else {
                    self.panel_bottom = 0.0;
                }
            });

        // ---- 调试自截图：不依赖合成器，直接取 egui 帧缓冲 ----
        if let Some(prefix) = self.screenshot_prefix.clone() {
            // 等窗口尺寸稳定后连拍若干张
            if matches!(self.frame_count, 6 | 12 | 20) && self.shots_taken < 3 {
                ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot);
            }
            let shot = ctx.input(|i| {
                i.events.iter().find_map(|e| match e {
                    egui::Event::Screenshot { image, .. } => Some(image.clone()),
                    _ => None,
                })
            });
            if let Some(img) = shot {
                self.shots_taken += 1;
                let path = format!("{prefix}-{}.ppm", self.shots_taken);
                match save_ppm(&path, &img) {
                    Ok(_) => log::info!("已保存截图 {path} ({}x{})", img.size[0], img.size[1]),
                    Err(e) => log::warn!("截图保存失败: {e}"),
                }
            }
        }



        // ---- 日历弹窗（浮在最上层；两个页面都能开）----
        if self.cal_open {
            self.show_calendar(ctx, s);
        }

        if std::env::var("KMCOUNTER_LAYOUT_LOG").is_ok() && self.frame_count % 6 == 0 {
            log::info!(
                "LAYOUT|page={:?}|base_h={:.1}|content_bottom={:.1}|bottom_top={:.1}|desired_h={:.1}|actual_h={:?}|panel_extra={:.1}",
                if self.page == Page::Heatmap { "heat" } else { "trend" },
                self.base_h,
                self.content_bottom,
                self.bottom_region_top,
                self.base_h + self.panel_extra_h(),
                ctx.input(|i| i.viewport().inner_rect.map(|r| r.height())),
                self.panel_extra_h()
            );
        }
        ctx.request_repaint_after(Duration::from_millis(500));
    }

    /// 日期 + 分时导航行（热力图页与趋势页共用，选择状态也是共用的）
    fn nav_row(&mut self, ui: &mut egui::Ui, s: &'static Strings) {
        ui.horizontal(|ui| {
            if ui.button("◀").clicked() {
                self.nav_step(true);
            }
            ui.add_space(4.0);
            // 固定宽度的日期标题：右箭头位置不随文字长度变化；点击打开日历
            let title = format!("{} - {}", s.view_date_prefix, self.view_label());
            let title_resp = ui
                .add_sized(
                    [NAV_TITLE_W, 20.0],
                    egui::Label::new(egui::RichText::new(title).strong())
                        .truncate()
                        .sense(Sense::click()),
                )
                .on_hover_cursor(egui::CursorIcon::PointingHand)
                .on_hover_text(s.cal_today);
            self.nav_title_rect = title_resp.rect;
            if title_resp.clicked() {
                self.cal_open = !self.cal_open;
                if self.cal_open {
                    let d = self.anchor_date();
                    self.cal_month = (d.year(), d.month());
                }
            }
            if ui.button("▶").clicked() {
                self.nav_step(false);
            }
            // 分时选择：全天 / 0-23 时（循环）
            ui.separator();
            if ui.button("◀").clicked() {
                self.nav_hour(true);
            }
            ui.add_space(2.0);
            let htitle = self.hour_title(s);
            ui.add_sized(
                [HOUR_TITLE_W, 20.0],
                egui::Label::new(egui::RichText::new(htitle).strong()).truncate(),
            );
            if ui.button("▶").clicked() {
                self.nav_hour(false);
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if self.page == Page::Heatmap && !self.selected.is_empty() && ui.button(s.sel_clear).clicked() {
                    self.selected.clear();
                }
            });
        });
    }

    /// 当前视图对应的“那天”：总计视图下取今天（趋势窗口以今天为末端）
    fn anchor_date(&self) -> NaiveDate {
        let today = NaiveDate::parse_from_str(&self.snap_today, "%Y%m%d").unwrap_or_else(|_| chrono::Local::now().date_naive());
        if self.view_idx == 0 {
            today
        } else {
            NaiveDate::parse_from_str(&self.view_label(), "%Y%m%d").unwrap_or(today)
        }
    }

    /// 趋势页“本小时”取哪个小时：选中了小时就用它，否则用现在的钟点
    fn anchor_hour(&self) -> u8 {
        self.hour_sel.unwrap_or_else(|| chrono::Local::now().hour().min(23) as u8)
    }

    /// 日历弹窗：点日期标题打开，直接点某天即切换（没有数据的日子不可点）
    fn show_calendar(&mut self, ctx: &egui::Context, s: &'static Strings) {
        let dim = Color32::from_rgb(0x99, 0x99, 0x99);
        let faint = Color32::from_rgb(0xC8, 0xC8, 0xC8);
        let today = NaiveDate::parse_from_str(&self.snap_today, "%Y%m%d").ok();
        let selected = if self.view_idx == 0 {
            None
        } else {
            NaiveDate::parse_from_str(&self.view_label(), "%Y%m%d").ok()
        };
        let (mut y, mut m) = self.cal_month;
        let first = NaiveDate::from_ymd_opt(y, m, 1).unwrap_or_else(|| chrono::Local::now().date_naive());
        let days_in_month = {
            let next = if m == 12 {
                NaiveDate::from_ymd_opt(y + 1, 1, 1)
            } else {
                NaiveDate::from_ymd_opt(y, m + 1, 1)
            }
            .unwrap_or(first);
            (next - first).num_days()
        };
        let lead = first.weekday().num_days_from_monday() as usize;
        let pos = self.nav_title_rect.left_bottom() + Vec2::new(0.0, 4.0);
        let mut close = false;

        let win = egui::Window::new("kmc_calendar")
            .title_bar(false)
            .resizable(false)
            .collapsible(false)
            .fixed_pos(pos)
            .show(ctx, |ui| {
                ui.set_min_width(258.0);
                ui.horizontal(|ui| {
                    if ui.button("◀").clicked() {
                        let p = first - chrono::Duration::days(1);
                        y = p.year();
                        m = p.month();
                    }
                    ui.add_sized(
                        [110.0, 20.0],
                        egui::Label::new(
                            egui::RichText::new(format!("{} / {:02}", y, m)).strong().color(TEXT),
                        ),
                    );
                    if ui.button("▶").clicked() {
                        let n = first + chrono::Duration::days(days_in_month);
                        y = n.year();
                        m = n.month();
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button(s.cal_today).clicked() {
                            self.view_idx = 1;
                            self.hour_sel = None;
                            self.title_dirty = true;
                            close = true;
                        }
                        if ui.button(s.total_label).clicked() {
                            self.view_idx = 0;
                            self.title_dirty = true;
                            close = true;
                        }
                    });
                });
                ui.add_space(2.0);
                egui::Grid::new("kmc_cal_grid").num_columns(7).spacing([2.0, 2.0]).show(ui, |ui| {
                    for wd in s.weekdays.split_whitespace() {
                        ui.add_sized([32.0, 14.0], egui::Label::new(egui::RichText::new(wd).small().color(dim)));
                    }
                    ui.end_row();
                    for cell in 0..42usize {
                        let day = cell as i64 - lead as i64 + 1;
                        if day < 1 || day > days_in_month {
                            ui.add_sized([32.0, 22.0], egui::Label::new(""));
                            if cell % 7 == 6 {
                                ui.end_row();
                            }
                            continue;
                        }
                        let date = first + chrono::Duration::days(day - 1);
                        let key = date.format("%Y%m%d").to_string();
                        let has_data = self.snap.days.contains_key(&key) || Some(date) == today;
                        let is_today = Some(date) == today;
                        let is_sel = Some(date) == selected;
                        let mut text = egui::RichText::new(format!("{day:2}")).color(if has_data { TEXT } else { faint });
                        if has_data && self.snap.days.get(&key).map(|d| d.keystrokes).unwrap_or(0) >= 100 {
                            text = text.strong();
                        }
                        if is_sel {
                            text = text.color(Color32::WHITE);
                        }
                        let mut btn = egui::Button::new(text).min_size(Vec2::new(32.0, 22.0));
                        if is_sel {
                            btn = btn.fill(SEL_COLOR);
                        } else if has_data {
                            btn = btn.fill(Color32::from_rgb(0xF7, 0xF7, 0xF7));
                        }
                        if is_today {
                            btn = btn.stroke(egui::Stroke::new(1.0_f32, SEL_COLOR));
                        }
                        if ui.add_enabled(has_data, btn).clicked() {
                            self.goto_day(&key);
                            close = true;
                        }
                        if cell % 7 == 6 {
                            ui.end_row();
                        }
                    }
                });
            });

        self.cal_month = (y, m);
        if close {
            self.cal_open = false;
        }
        // Esc 或点击弹窗外关闭
        let esc = ctx.input(|i| i.key_pressed(egui::Key::Escape));
        let clicked_out = ctx.input(|i| i.pointer.any_click())
            && ctx
                .input(|i| i.pointer.interact_pos())
                .map_or(false, |p| {
                    !win.as_ref().map(|w| w.response.rect.contains(p)).unwrap_or(false)
                        && !self.nav_title_rect.contains(p)
                });
        if esc || clicked_out {
            self.cal_open = false;
        }
    }

    /// 布局锚点（软渲染出图时用来确定画布高度）
    #[cfg(feature = "softshot")]
    pub(crate) fn debug_layout(&self) -> (f32, f32, f32, f32) {
        (self.base_h, self.panel_extra_h(), self.content_bottom, self.bottom_region_top)
    }

    /// 测试用 setter：不通过环境变量（环境变量是进程级的，并行测试会互相串）
    #[cfg(feature = "softshot")]
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn debug_set_hour(&mut self, hour: Option<u8>) {
        self.hour_sel = hour;
    }

    #[cfg(feature = "softshot")]
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn debug_open_panel(&mut self) {
        self.more_open = true;
    }

    #[cfg(feature = "softshot")]
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn debug_set_trend_page(&mut self) {
        self.page = Page::Trends;
    }

    #[cfg(feature = "softshot")]
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn debug_nav_title_rect(&self) -> Rect {
        self.nav_title_rect
    }

    /// 当前语言的文案表（测试里按文案定位控件）
    #[cfg(feature = "softshot")]
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn debug_strings(&self) -> &'static crate::lang::Strings {
        self.s()
    }

    /// 系统里有没有中文字体（测试据此决定是否检查中文界面）
    #[cfg(feature = "softshot")]
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn debug_has_cjk(&self) -> bool {
        self.has_cjk
    }

    /// 指定屏幕物理尺寸（测试里让「屏幕尺寸」有一个确定的数值）
    #[cfg(feature = "softshot")]
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn debug_set_screen_mm(&self, w: u32, h: u32) {
        self.shared.screen_w_mm.store(w, Ordering::Relaxed);
        self.shared.screen_h_mm.store(h, Ordering::Relaxed);
    }

    #[cfg(feature = "softshot")]
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn debug_open_calendar(&mut self) {
        self.cal_open = true;
        let d = chrono::Local::now().date_naive();
        self.cal_month = (d.year(), d.month());
    }

    /// 视图标签 / 跳转某天 / 托盘标志（离屏测试用）
    #[cfg(feature = "softshot")]
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn debug_view_label(&self) -> String {
        self.view_label()
    }

    /// 供测试直接走「日历点某天」的同一条路径
    #[cfg(feature = "softshot")]
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn debug_goto_day(&mut self, date: &str) -> bool {
        self.goto_day(date)
    }

    #[cfg(feature = "softshot")]
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn debug_calendar_open(&self) -> bool {
        self.cal_open
    }

    #[cfg(feature = "softshot")]
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn debug_flags(&self) -> Arc<crate::controller::UiFlags> {
        self.flags.clone()
    }

    /// 分布条矩形 / 当前选中小时 / 已选按键（离屏交互测试用）
    #[cfg(feature = "softshot")]
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn debug_strip_rect(&self) -> Rect {
        self.strip_rect
    }

    /// 趋势图矩形（测试按桶中心点击）
    #[cfg(feature = "softshot")]
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn debug_chart_rect(&self) -> Rect {
        self.chart_rect
    }

    /// 切换趋势粒度（测试里不通过环境变量）
    #[cfg(feature = "softshot")]
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn debug_set_granularity(&mut self, g: Granularity) {
        self.granularity = g;
    }

    #[cfg(feature = "softshot")]
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn debug_hour_sel(&self) -> Option<u8> {
        self.hour_sel
    }

    /// 走“保存”同一条路径切换语言，返回生效后的语言代码（离屏测试用）
    #[cfg(feature = "softshot")]
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn debug_set_language(&mut self, code: &str) -> &'static str {
        self.edit.language = code.to_string();
        self.apply_language();
        match self.lang {
            Lang::Zh => "zh",
            Lang::En => "en",
        }
    }

    #[cfg(feature = "softshot")]
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn debug_selected(&self) -> Vec<usize> {
        self.selected.iter().copied().collect()
    }

    /// 趋势页顶部：本小时/本周/本月 相对 昨日同小时/上周同期/上月同期 的环比卡片（每行一个指标）
    fn comparison_strip(&mut self, ui: &mut egui::Ui, s: &'static Strings, anchor: NaiveDate, hour: u8, is_today: bool) {
        use crate::trend::{hour_compare, month_compare, week_compare};
        let keys_metric = |d: &BucketStats| d.keystrokes as f64;
        let px_metric = |d: &BucketStats| d.mouse.move_px;

        let hr_keys = hour_compare(&self.snap, anchor, hour, keys_metric);
        let hr_px = hour_compare(&self.snap, anchor, hour, px_metric);
        let wk_keys = week_compare(&self.snap, anchor, keys_metric);
        let mo_keys = month_compare(&self.snap, anchor, keys_metric);
        let wk_px = week_compare(&self.snap, anchor, px_metric);
        let mo_px = month_compare(&self.snap, anchor, px_metric);
        // 看今天就说“本”，看历史某天就说“该”
        let (h_lab, w_lab, m_lab) = if is_today {
            (s.stat_this_hour, s.stat_this_week, s.stat_this_month)
        } else {
            (s.stat_that_hour, s.stat_that_week, s.stat_that_month)
        };

        let px_to_m = |px: f64| self.shared.px_to_meters(px);
        let fmt_keys = |v: f64| format!("{} {}", fmt_thousands(v), s.unit_times.trim()).trim_end().to_string();
        let fmt_meters = |px: f64| format!("{:.2} {}", px_to_m(px), s.unit_m);

        let rows: [[(String, String, crate::trend::PeriodDelta, &'static str); 3]; 2] = [
            [
                (format!("{} · {}", s.keystrokes, h_lab), fmt_keys(hr_keys.current), hr_keys, s.stat_vs_hour),
                (format!("{} · {}", s.keystrokes, w_lab), fmt_keys(wk_keys.current), wk_keys, s.stat_vs_week),
                (format!("{} · {}", s.keystrokes, m_lab), fmt_keys(mo_keys.current), mo_keys, s.stat_vs_month),
            ],
            [
                (format!("{} · {}", s.mouse_move, h_lab), fmt_meters(hr_px.current), hr_px, s.stat_vs_hour),
                (format!("{} · {}", s.mouse_move, w_lab), fmt_meters(wk_px.current), wk_px, s.stat_vs_week),
                (format!("{} · {}", s.mouse_move, m_lab), fmt_meters(mo_px.current), mo_px, s.stat_vs_month),
            ],
        ];
        for (i, cards) in rows.into_iter().enumerate() {
            let row = ui.horizontal(|ui| {
                let n = cards.len() as f32;
                let spacing = ui.spacing().item_spacing.x;
                let card_w = ((ui.available_width() - spacing * (n - 1.0)) / n).max(120.0);
                for (title, value, d, vs) in cards {
                    delta_card(ui, s, &title, &value, d, vs, card_w);
                }
            });
            geo_log(&format!("cards row {i}"), row.response.rect);
        }
    }

    /// 趋势页：时/日/周/月/年粒度切换 + 双轴平滑曲线（键盘次数 / 鼠标移动距离）
    fn show_trend_page(&mut self, ui: &mut egui::Ui, s: &'static Strings, content_top: f32) {
        let now = chrono::Local::now();
        let today = NaiveDate::parse_from_str(&self.snap_today, "%Y%m%d").unwrap_or_else(|_| now.date_naive());
        // 趋势以“当前视图那天”为锚：总计视图 = 今天；选中历史某天 = 那天
        let anchor = self.anchor_date();
        let is_today = anchor == today;
        let hour = self.anchor_hour();

        // 日期/分时导航由外层统一绘制（与热力图页同一套，选择状态共用）
        // 环比卡片（本/该小时、本周/该周、本月/该月）
        self.comparison_strip(ui, s, anchor, hour, is_today);
        ui.add_space(2.0);

        // 粒度选择
        let gran_row = ui.horizontal(|ui| {
            ui.selectable_value(&mut self.granularity, Granularity::Hourly, s.gran_hourly);
            ui.selectable_value(&mut self.granularity, Granularity::Daily, s.gran_daily);
            ui.selectable_value(&mut self.granularity, Granularity::Weekly, s.gran_weekly);
            ui.selectable_value(&mut self.granularity, Granularity::Monthly, s.gran_monthly);
            ui.selectable_value(&mut self.granularity, Granularity::Yearly, s.gran_yearly);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(egui::RichText::new(s.trend_hint).small().color(Color32::from_rgb(0x99, 0x99, 0x99)));
            });
        });
        geo_log("granularity row", gran_row.response.rect);
        ui.add_space(2.0);

        // 聚合数据：
        //  · 小时粒度：总计视图看“最近 72 小时”；选中某天则看那天的 24 小时（今天只画到当前钟点）
        //  · 其余粒度：窗口末端对齐锚定日期
        let px_to_m = |px: f64| self.shared.px_to_meters(px);
        let pts = if self.granularity == Granularity::Hourly {
            if is_today {
                trend::build_hourly_trends(
                    &self.snap,
                    chrono::NaiveDateTime::new(today, now.time()),
                    trend::HOURLY_WINDOW,
                    px_to_m,
                )
            } else {
                let end_hour = if anchor == today { now.hour().min(23) as u32 } else { 23 };
                let end = anchor
                    .and_hms_opt(end_hour, 0, 0)
                    .unwrap_or_else(|| anchor.and_hms_opt(23, 0, 0).unwrap());
                trend::build_hourly_trends(&self.snap, end, end_hour + 1, px_to_m)
            }
        } else {
            trend::build_trends(&self.snap, anchor, self.granularity, px_to_m)
        };

        let key_name = s.keystrokes.to_string();
        let mouse_name = format!("{} ({})", s.trend_mouse_distance, s.unit_m);
        let data = chart::ChartData {
            labels: pts.iter().map(|p| p.label.clone()).collect(),
            series: vec![
                chart::Series {
                    name: key_name.clone(),
                    color: KEY_COLOR,
                    values: pts.iter().map(|p| p.keystrokes).collect(),
                },
                chart::Series {
                    name: mouse_name.clone(),
                    color: MOUSE_COLOR,
                    values: pts.iter().map(|p| p.move_meters).collect(),
                },
            ],
        };

        // 图表高度由 base_h 推出，与窗口当前实际高度无关：
        // 否则窗口变高 → 图表变高 → 面板下移 → 又要更高，会互相追着长
        let used_above = ui.cursor().min.y - content_top;
        let height = (self.base_h - CONTROLS_H - used_above - 8.0).max(200.0);
        // Sense::click：点某一点可以直接跳到那一天（悬停提示仍在 draw 里处理）
        let (rect, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), height), Sense::click());
        geo_log("chart", rect);
        self.chart_rect = rect;
        let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
        let painter = ui.painter().clone();
        let hover = chart::draw(ui, rect, &resp, &data);

        // 数据不足提示
        if pts.len() < 2 {
            painter.text(
                rect.center(),
                Align2::CENTER_CENTER,
                s.trend_no_data,
                FontId::proportional(14.0),
                Color32::from_rgb(0x99, 0x99, 0x99),
            );
        }

        // 左键点某个点 → 跳到那天（小时粒度连该小时一起选中；周/月/年粒度跳到该桶里有数据的那天）
        if resp.clicked() {
            if let Some(h) = hover {
                let p = &pts[h.index];
                match p.hour {
                    Some(hour) => {
                        self.goto_trend_point(p.date, Some(hour));
                    }
                    None => {
                        self.goto_trend_bucket(p.date, self.granularity);
                    }
                }
            }
        }

        // 悬停提示框
        if let Some(h) = hover {
            let full = pts[h.index].full_label.clone();
            let unit = s.unit_times.trim();
            let line1 = if unit.is_empty() {
                format!("{}: {:.0}", key_name, pts[h.index].keystrokes)
            } else {
                format!("{}: {:.0} {}", key_name, pts[h.index].keystrokes, unit)
            };
            let line2 = format!("{}: {:.2} {}", mouse_name, pts[h.index].move_meters, s.unit_m);
            let lines: Vec<(String, Color32)> = vec![
                (full, Color32::from_rgb(0x88, 0x88, 0x88)),
                (line1, KEY_COLOR),
                (line2, MOUSE_COLOR),
            ];
            let font = FontId::proportional(12.0);
            let galleys: Vec<_> = lines
                .iter()
                .map(|(t, c)| painter.layout_no_wrap(t.clone(), font.clone(), *c))
                .collect();
            let w = galleys.iter().map(|g| g.size().x).fold(0.0f32, f32::max);
            let hgt: f32 = galleys.iter().map(|g| g.size().y).sum::<f32>() + 4.0 * (lines.len() as f32 - 1.0);
            let box_size = Vec2::new(w + 12.0, hgt + 12.0);
            let mut origin = h.pos + Vec2::new(14.0, 14.0);
            let screen = ui.input(|i| i.screen_rect);
            if origin.x + box_size.x > screen.right() {
                origin.x = h.pos.x - box_size.x - 14.0;
            }
            if origin.y + box_size.y > screen.bottom() {
                origin.y = h.pos.y - box_size.y - 14.0;
            }
            let fg = painter.with_layer_id(egui::LayerId::new(egui::Order::Tooltip, egui::Id::new("trend_tip")));
            fg.rect_filled(Rect::from_min_size(origin, box_size), 4.0, Color32::from_rgba_unmultiplied(40, 40, 40, 240));
            let mut y = origin.y + 6.0;
            for g in &galleys {
                fg.galley(Pos2::new(origin.x + 6.0, y), g.clone(), Color32::WHITE);
                y += g.size().y + 4.0;
            }
        }
    }

    fn paint_heatmap(&mut self, ui: &mut egui::Ui, rect: Rect, resp: egui::Response, u: f32, pitch: f32, gap: f32) {
        let painter = ui.painter().clone();
        let entry = self.day_for_view();
        let day = entry.view(self.hour_sel);
        let maxcount = (day.keystrokes as f64) / 10.0;
        // 数据被判定作废的小时不画热力（原始数值仍在数据里）
        let enough = !day.invalid && day.keystrokes >= if self.hour_sel.is_some() { HOUR_ENOUGH_KEYS } else { 100 };
        let key_font = FontId::proportional((self.cfg.layout.key_w as f32 * 0.21).clamp(8.0, 12.0));

        for (i, k) in KEYS.iter().enumerate() {
            let r = self.key_rect(i, rect, u, pitch, gap);
            let count = day.key_count(i) as f64;
            let fill = if enough && count > 0.0 { heat_color(count, maxcount) } else { BG };
            painter.rect_filled(r, 2.0, fill);
            // 被点选的按键用强调色描边
            let stroke = if self.selected.contains(&i) {
                egui::Stroke::new(2.5_f32, SEL_COLOR)
            } else {
                egui::Stroke::new(1.0_f32, Color32::WHITE)
            };
            painter.rect_stroke(r, 2.0, stroke);

            if let Some(dir) = k.arrow {
                let (a, b, c) = arrow_points(dir, r);
                painter.add(Shape::convex_polygon(vec![a, b, c], TEXT, egui::Stroke::NONE));
            } else if !k.label.is_empty() {
                painter.text(r.center(), Align2::CENTER_CENTER, k.label, key_font.clone(), TEXT);
            }
        }

        // ---- 键盘右上角空白处：24 小时分布（点击柱子只看该小时）----
        let strip = self.hour_strip_rect(rect, u, pitch, gap);
        self.strip_rect = strip;
        let prof = entry.hour_profile();
        let hover_pos = if resp.hovered() { ui.input(|i| i.pointer.hover_pos()) } else { None };

        let hstrip_sel = self.hour_sel;
        let now_hour = (self.view_idx > 0 && self.view_label() == self.snap_today)
            .then(|| chrono::Local::now().hour().min(23) as u8);
        let invalid_hours: Vec<bool> = (0..HOURS).map(|h| entry.hour_invalid(h as u8)).collect();
        paint_hour_strip(&painter, strip, &prof, hstrip_sel, now_hour, &invalid_hours);

        // ---- 点击：分布条优先，其次是按键（选中/取消，查看按压次数）----
        if resp.clicked() {
            if let Some(pos) = resp.interact_pointer_pos() {
                if strip.contains(pos) {
                    if let Some(idx) = hour_bar_index(strip, pos.x) {
                        let h = idx as u8;
                        self.hour_sel = if self.hour_sel == Some(h) { None } else { Some(h) };
                    }
                } else {
                    for i in 0..KEYS.len() {
                        if self.key_rect(i, rect, u, pitch, gap).contains(pos) {
                            if !self.selected.remove(&i) {
                                self.selected.insert(i);
                            }
                            break;
                        }
                    }
                }
            }
        }

        // ---- 悬停提示：分布条显示该小时数值，按键显示该键敲击次数 ----
        if let Some(pos) = hover_pos {
            if strip.contains(pos) {
                if let Some(idx) = hour_bar_index(strip, pos.x) {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                    let (ks, px) = prof[idx];
                    let unit = self.s().unit_times.trim();
                    let mark = if invalid_hours.get(idx).copied().unwrap_or(false) { self.s().invalid_mark } else { "" };
                    let text = format!(
                        "{}{} · {} {}{} · {} {:.2} {}{}{}",
                        idx,
                        self.s().hour_suffix,
                        self.s().keystrokes,
                        ks,
                        unit,
                        self.s().mouse_move,
                        self.shared.px_to_meters(px),
                        self.s().unit_m,
                        mark,
                        self.s().strip_hint
                    );
                    paint_tooltip(&painter, ui, pos, &text);
                }
            } else {
                for (i, k) in KEYS.iter().enumerate() {
                    let r = self.key_rect(i, rect, u, pitch, gap);
                    if r.contains(pos) {
                        let count = day.key_count(i);
                        if count > 0 {
                            let label = if k.label.is_empty() { k.name } else { k.label };
                            let unit = self.s().unit_times.trim();
                            let text = if unit.is_empty() {
                                format!("{label}: {count}")
                            } else {
                                format!("{label}: {count} {unit}")
                            };
                            paint_tooltip(&painter, ui, pos, &text);
                        }
                        break;
                    }
                }
            }
        }
    }

    /// 分时分布条的位置：F 行右侧、数字键盘区上方那一块空白（正好一个键位高）。
    /// 导航区顶部现在放了 PrtScr / ScrollLock / Pause，所以这块只剩数字键盘上方 4 个单位宽。
    fn hour_strip_rect(&self, rect: Rect, u: f32, pitch: f32, gap: f32) -> Rect {
        let spacing = self.cfg.layout.key_spacing as f32;
        let nav_x = MAIN_UNITS * u + gap;
        let num_x = nav_x + NAV_UNITS * u + gap;
        let right = num_x + NUM_UNITS * u + gap - spacing;
        Rect::from_min_max(
            Pos2::new(rect.left() + num_x, rect.top()),
            Pos2::new(rect.left() + right, rect.top() + pitch - spacing),
        )
    }

    /// 统计表：列随视图变化 ——
    /// 选中小时时插入「该小时」列；查看「总计」时不再重复两列相同的数字。
    /// 每行是 `(标签, 各列文本, 是否细分项)`；细分项（键盘分区）渲染时缩进。
    fn stats_table(&self, s: &'static Strings) -> (Vec<String>, Vec<(String, Vec<String>, bool)>) {
        let entry = self.day_for_view();
        let view_is_total = self.view_idx == 0;
        let view_is_today = self.view_label() == self.snap_today;
        let with_hour = self.hour_sel.is_some();
        let hour_unit = entry.view(self.hour_sel);
        let day_unit = entry.view(None);
        let total_unit = self.snap.total.view(None);

        let mut header: Vec<String> = vec![s.col_item.to_string()];
        if let Some(h) = self.hour_sel {
            header.push(hour_label(h, s));
        }
        if !view_is_total {
            header.push(if view_is_today { s.col_today.to_string() } else { self.view_label() });
        }
        header.push(s.col_total.to_string());
        let ncols = header.len();

        let fmt_move = |px: f64| format!("{:.2} {}", self.shared.px_to_meters(px), s.unit_m);
        let mk_row = |label: String, h: String, v: String, t: String, sub: bool| -> (String, Vec<String>, bool) {
            let mut cells = Vec::with_capacity(ncols);
            if with_hour {
                cells.push(h);
            }
            if !view_is_total {
                cells.push(v);
            }
            cells.push(t);
            (label, cells, sub)
        };
        let mk = |label: String, h: String, v: String, t: String| mk_row(label, h, v, t, false);

        let mut rows: Vec<(String, Vec<String>, bool)> = Vec::new();
        rows.push(mk(
            s.mouse_move.into(),
            fmt_move(hour_unit.mouse.move_px),
            fmt_move(day_unit.mouse.move_px),
            fmt_move(total_unit.mouse.move_px),
        ));
        rows.push(mk(
            s.keystrokes.into(),
            hour_unit.keystrokes.to_string(),
            day_unit.keystrokes.to_string(),
            total_unit.keystrokes.to_string(),
        ));
        // 键盘敲击的细分：四个键盘分区（只统计布局内的按键，布局外的不计）
        for z in crate::keys::ZONES {
            let sum = |b: &BucketStats| crate::keys::zone_sum(z, |i| b.key_count(i));
            rows.push(mk_row(
                zone_label(z, s).to_string(),
                sum(&hour_unit).to_string(),
                sum(&day_unit).to_string(),
                sum(&total_unit).to_string(),
                true,
            ));
        }
        {
            let mut push_num = |label: &str, pick: &dyn Fn(&BucketStats) -> u64| {
                rows.push(mk(
                    label.to_string(),
                    pick(&hour_unit).to_string(),
                    pick(&day_unit).to_string(),
                    pick(&total_unit).to_string(),
                ));
            };
            push_num(s.lbutton, &|u| u.mouse.lb);
            push_num(s.rbutton, &|u| u.mouse.rb);
            push_num(s.mbutton, &|u| u.mouse.mb);
            push_num(s.wheel, &|u| u.mouse.wheel);
            push_num(s.hwheel, &|u| u.mouse.hwheel);
            push_num(s.xbutton, &|u| u.mouse.xb);
        }
        // 屏幕尺寸与时段无关，只占一格：固定放在「总计」列（最后一列），
        // 否则列数随视图变化时它会从总计列跳到「今日」列
        let mut cells = vec![String::new(); ncols - 1];
        cells[ncols - 2] = monitor_size_str(&self.shared, s);
        rows.push((s.monitor.into(), cells, false));

        (header, rows)
    }

    /// 统计列表：内嵌展开，走正常布局流，不遮挡任何按键
    fn show_stats_list(&mut self, ui: &mut egui::Ui, s: &'static Strings) {
        let (header, rows) = self.stats_table(s);
        let dim = Color32::from_rgb(0x99, 0x99, 0x99);
        let avail_before = ui.available_width();
        let inner_w = (avail_before - group_pad(ui).x).max(240.0);
        let frame = egui::Frame::group(ui.style())
            .fill(Color32::from_rgb(0xF7, 0xF7, 0xF7))
            .stroke(egui::Stroke::new(1.0_f32, Color32::from_rgb(0xDD, 0xDD, 0xDD)))
            .show(ui, |ui| {
                // 展开面板的父布局是横向的；Frame 会继承父布局，这里显式竖排
                ui.vertical(|ui| {
                // 宽度写死（min=max）：外框宽度精确等于列宽
                ui.set_width(inner_w);
                egui::Grid::new("stats_grid")
                    .num_columns(header.len())
                    .spacing([20.0, 3.0])
                    .show(ui, |ui| {
                        // 表头：与数据行同字号并加粗
                        for (i, h) in header.iter().enumerate() {
                            let t = egui::RichText::new(h.clone()).strong().color(dim);
                            if i == 0 {
                                ui.label(t);
                            } else {
                                right_label(ui, t);
                            }
                        }
                        ui.end_row();

                        for (label, cells, sub) in &rows {
                            if *sub {
                                // 细分项（键盘分区）：缩进一级，表示从属于上面的「键盘敲击」
                                ui.horizontal(|ui| {
                                    ui.add_space(SUB_ROW_INDENT);
                                    ui.label(egui::RichText::new(label.clone()).color(TEXT));
                                });
                            } else {
                                ui.label(egui::RichText::new(label.clone()).color(TEXT));
                            }
                            for c in cells {
                                right_label(ui, egui::RichText::new(c.clone()).color(TEXT));
                            }
                            ui.end_row();
                        }
                    });
                });
            });
        geo_log("stats frame", frame.response.rect);
        geo_log("stats col avail", Rect::from_min_size(Pos2::ZERO, Vec2::new(avail_before, 0.0)));
    }

    /// 设置：底部内嵌展开（与统计列表同一套浅色风格）
    fn show_settings_section(&mut self, ui: &mut egui::Ui, s: &'static Strings) {
        let avail_before = ui.available_width();
        let inner_w = (avail_before - group_pad(ui).x).max(240.0);
        let frame = egui::Frame::group(ui.style())
            .fill(Color32::from_rgb(0xF7, 0xF7, 0xF7))
            .stroke(egui::Stroke::new(1.0_f32, Color32::from_rgb(0xDD, 0xDD, 0xDD)))
            .show(ui, |ui| {
                ui.vertical(|ui| {
                ui.set_width(inner_w);
                let label = |ui: &mut egui::Ui, t: &str| {
                    ui.label(egui::RichText::new(t).color(TEXT));
                };

                ui.strong(egui::RichText::new(s.settings_title).size(16.0).color(TEXT));
                ui.add_space(2.0);
                ui.columns(3, |cols| {
                    // 左栏：历史数据 + 屏幕尺寸
                    let ui = &mut cols[0];
                    ui.strong(egui::RichText::new(s.settings_history).size(14.0).color(TEXT));
                    ui.label(egui::RichText::new(s.settings_history_sub).small().color(TEXT));
                    ui.horizontal(|ui| {
                        label(ui, s.settings_storage);
                        ui.add(egui::DragValue::new(&mut self.edit.history_days).range(0..=10000).speed(1.0));
                        label(ui, s.settings_days);
                    });
                    ui.add_space(6.0);
                    ui.strong(egui::RichText::new(s.settings_screen).size(14.0).color(TEXT));
                    ui.label(egui::RichText::new(s.settings_screen_sub).small().color(TEXT));
                    egui::Grid::new("screen_grid").num_columns(3).spacing([8.0, 4.0]).show(ui, |ui| {
                        label(ui, s.settings_width);
                        ui.add(egui::DragValue::new(&mut self.edit.screen_w_mm).range(0..=20000).speed(1.0));
                        label(ui, s.settings_mm);
                        ui.end_row();
                        label(ui, s.settings_height);
                        ui.add(egui::DragValue::new(&mut self.edit.screen_h_mm).range(0..=20000).speed(1.0));
                        label(ui, s.settings_mm);
                        ui.end_row();
                    });


                    // 左栏之二：界面语言（三栏重新分配：语言挪到最矮的第一栏，第三栏不再拖得很长）
                    ui.add_space(6.0);
                    ui.strong(egui::RichText::new(s.settings_language).size(14.0).color(TEXT));
                    ui.label(egui::RichText::new(s.settings_language_sub).small().color(TEXT));
                    ui.horizontal(|ui| {
                        ui.radio_value(&mut self.edit.language, "auto".to_string(), s.lang_auto);
                        for (code, name) in crate::lang::LANG_CHOICES {
                            // 没有中文字体时不提供中文选项（选了也会回退英文）
                            if code == "zh" && !self.has_cjk {
                                continue;
                            }
                            ui.radio_value(&mut self.edit.language, code.to_string(), name);
                        }
                    });
                    if !self.has_cjk {
                        ui.label(egui::RichText::new(s.lang_no_cjk).small().color(DELTA_DOWN));
                    }

                    // 中栏：键盘布局
                    let ui = &mut cols[1];
                    ui.strong(egui::RichText::new(s.settings_layout).size(14.0).color(TEXT));
                    ui.label(egui::RichText::new(s.settings_layout_sub).small().color(TEXT));
                    egui::Grid::new("layout_grid").num_columns(3).spacing([8.0, 4.0]).show(ui, |ui| {
                        label(ui, s.settings_key_w);
                        ui.add(egui::DragValue::new(&mut self.edit.layout.key_w).range(30..=200).speed(1.0));
                        label(ui, s.settings_px);
                        ui.end_row();
                        label(ui, s.settings_key_h);
                        ui.add(egui::DragValue::new(&mut self.edit.layout.key_h).range(25..=200).speed(1.0));
                        label(ui, s.settings_px);
                        ui.end_row();
                        label(ui, s.settings_key_spacing);
                        ui.add(egui::DragValue::new(&mut self.edit.layout.key_spacing).range(0..=20).speed(1.0));
                        label(ui, s.settings_px);
                        ui.end_row();
                        label(ui, s.settings_h_spacing);
                        ui.add(egui::DragValue::new(&mut self.edit.layout.h_spacing).range(0..=60).speed(1.0));
                        label(ui, s.settings_px);
                        ui.end_row();
                        label(ui, s.settings_v_spacing);
                        ui.add(egui::DragValue::new(&mut self.edit.layout.v_spacing).range(0..=60).speed(1.0));
                        label(ui, s.settings_px);
                        ui.end_row();
                    });

                    // 右栏：窗口行为 + 数据校验
                    let ui = &mut cols[2];
                    ui.strong(egui::RichText::new(s.settings_window).size(14.0).color(TEXT));
                    ui.label(egui::RichText::new(s.settings_window_sub).small().color(TEXT));
                    ui.checkbox(&mut self.edit.keep_on_top, s.settings_keep_on_top);

                    ui.add_space(6.0);
                    ui.strong(egui::RichText::new(s.settings_limits).size(14.0).color(TEXT));
                    ui.label(egui::RichText::new(s.settings_limits_sub).small().color(TEXT));
                    egui::Grid::new("limits_grid").num_columns(3).spacing([8.0, 4.0]).show(ui, |ui| {
                        label(ui, s.settings_limit_move);
                        ui.add(egui::DragValue::new(&mut self.edit.limit_move_km).range(0.0..=100000.0).speed(1.0));
                        label(ui, s.unit_km_per_hour);
                        ui.end_row();
                        label(ui, s.settings_limit_click);
                        ui.add(egui::DragValue::new(&mut self.edit.limit_clicks).range(0..=10_000_000).speed(10.0));
                        label(ui, s.unit_per_hour);
                        ui.end_row();
                        label(ui, s.settings_limit_keys);
                        ui.add(egui::DragValue::new(&mut self.edit.limit_keystrokes).range(0..=10_000_000).speed(10.0));
                        label(ui, s.unit_per_hour);
                        ui.end_row();
                    });
                    }); // ui.columns(3)

                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        // 按钮靠右：宽窗口下也在容器右下角
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.button(s.settings_save).clicked() {
                                {
                                    let mut cfg = self.cfg_shared.lock();
                                    *cfg = self.edit.clone();
                                    let _ = cfg.save(&self.cfg_path);
                                }
                                self.cfg = self.edit.clone();
                                self.apply_language();
                                // 阈值同步给输入线程（它按这个判定小时是否作废）
                                {
                                    let mm = self.shared.screen_w_mm.load(Ordering::Relaxed) as f64;
                                    let px = self.shared.screen_px_w.load(Ordering::Relaxed) as f64;
                                    let ppm = if mm > 0.0 && px > 0.0 { 1000.0 * px / mm } else { 0.0 };
                                    self.shared.store.lock().limits = crate::stats::HourLimits::from_config(&self.cfg, ppm);
                                }
                                if self.cfg.screen_w_mm > 0 {
                                    self.shared.screen_w_mm.store(self.cfg.screen_w_mm, Ordering::Relaxed);
                                    self.shared.screen_h_mm.store(self.cfg.screen_h_mm, Ordering::Relaxed);
                                }
                                self.more_open = false;
                            }
                            if ui.button(s.settings_cancel).clicked() {
                                self.more_open = false;
                            }
                        });
                    });
                });
            });
        geo_log("settings frame", frame.response.rect);
        geo_log("settings col avail", Rect::from_min_size(Pos2::ZERO, Vec2::new(avail_before, 0.0)));
    }
}

/// 在键盘右上角空白处绘制 24 小时分布条（当前小时浅底、选中小时强调描边、每 3 小时标刻度）
fn paint_hour_strip(
    painter: &egui::Painter,
    area: Rect,
    prof: &[(u64, f64)],
    selected: Option<u8>,
    now_hour: Option<u8>,
    invalid: &[bool],
) {
    let label_h = 10.0;
    let bars = Rect::from_min_max(area.min, Pos2::new(area.right(), area.bottom() - label_h));
    let slot = bars.width() / HOURS as f32;
    // 量程只按“有效”小时算，否则一个作废的巨大值会把其他柱子压成一条线
    let max = prof
        .iter()
        .enumerate()
        .filter(|(h, _)| !invalid.get(*h).copied().unwrap_or(false))
        .map(|(_, (k, _))| *k)
        .max()
        .unwrap_or(0);
    let dim = Color32::from_rgb(0x99, 0x99, 0x99);

    for h in 0..HOURS {
        let x0 = bars.left() + h as f32 * slot;
        if now_hour == Some(h as u8) {
            painter.rect_filled(
                Rect::from_min_size(Pos2::new(x0, area.top()), Vec2::new(slot, area.height())),
                2.0,
                Color32::from_rgb(0xE4, 0xE4, 0xE4),
            );
        }
        let ks = prof.get(h).map(|(k, _)| *k).unwrap_or(0);
        let bad = invalid.get(h).copied().unwrap_or(false);
        // 作废的小时画满高度，表示“超出量程、已作废”
        let frac = if bad {
            1.0
        } else if max == 0 {
            0.0
        } else {
            ks as f32 / max as f32
        };
        let bh = (2.0 + frac * (bars.height() - 3.0)).min(bars.height());
        let bw = (slot * 0.64).max(2.0);
        let bar = Rect::from_min_size(
            Pos2::new(x0 + (slot - bw) / 2.0, bars.bottom() - bh),
            Vec2::new(bw, bh),
        );
        let fill = if bad {
            Color32::from_rgb(0xCF, 0xCF, 0xCF) // 已作废的小时：灰
        } else if ks == 0 {
            Color32::from_rgb(0xE8, 0xE8, 0xE8)
        } else {
            heat_color(ks as f64, max as f64)
        };
        painter.rect_filled(bar, 1.5, fill);
        if selected == Some(h as u8) {
            painter.rect_stroke(bar.expand(1.5), 2.0, egui::Stroke::new(2.0_f32, SEL_COLOR));
        }
        if h % 3 == 0 {
            painter.text(
                Pos2::new(x0 + slot / 2.0, area.bottom() - label_h / 2.0),
                Align2::CENTER_CENTER,
                h.to_string(),
                FontId::proportional(9.0),
                dim,
            );
        }
    }
}

/// 分组框（统计/设置/卡片）的左右内边距之和：用于把外框宽度算准
fn group_pad(ui: &egui::Ui) -> Vec2 {
    egui::Frame::group(ui.style()).inner_margin.sum()
}

fn geo_log(tag: &str, r: Rect) {
    if std::env::var("KMCOUNTER_GEO_LOG").is_ok() {
        eprintln!("[geo] {tag:<16} x {:.1}..{:.1} (w {:.1})  y {:.1}..{:.1} (h {:.1})", r.left(), r.right(), r.width(), r.top(), r.bottom(), r.height());
    }
}

/// 环比卡片：标题（指标 · 时间窗）+ 当前值 + 涨跌幅（悬停显示对比口径）
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

/// 小时列标题（“14 时” / “14:00”）
fn hour_label(h: u8, s: &Strings) -> String {
    format!("{h}{}", s.hour_suffix)
}

/// 日期 ± n 天
fn shift_days(d: NaiveDate, days: i64) -> NaiveDate {
    d + chrono::Duration::days(days)
}

/// 日期 ± n 个月（n = 12 即一年）。月末按当月天数收敛：3-31 减一月 = 2-28。
fn shift_months(d: NaiveDate, months: u32, newer: bool) -> NaiveDate {
    let m = chrono::Months::new(months);
    let r = if newer { d.checked_add_months(m) } else { d.checked_sub_months(m) };
    r.unwrap_or(d)
}

/// 一个趋势桶覆盖的日期闭区间（点击图表时用来找该桶里有数据的那天）
fn bucket_range(start: NaiveDate, g: Granularity) -> (NaiveDate, NaiveDate) {
    let next = match g {
        Granularity::Hourly | Granularity::Daily => shift_days(start, 1),
        Granularity::Weekly => shift_days(start, 7),
        Granularity::Monthly => shift_months(start, 1, true),
        Granularity::Yearly => shift_months(start, 12, true),
    };
    (start, next - chrono::Duration::days(1))
}

/// 键盘分区的显示名
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

/// 收起窗口：Wayland 下 winit 无法隐藏窗口，用 1x1 收缩（最小化无法程序化恢复且会停止渲染帧）；
/// X11/Windows 直接隐藏（任务栏条目随之消失）。
pub fn hide_window(shared: &Arc<Shared>, ctx: &egui::Context) {
    if is_wayland() {
        // Wayland 无法隐藏窗口（winit set_visible 是空实现），且窗口被遮挡时尺寸变更无法生效
        //（需提交缓冲）。最小化是合成器侧操作，任何情况下都可靠；配合 KWin 规则不占任务栏，
        //恢复由 KWin 脚本完成（见 kwin::unminimize_activate）。
        shared.ui_hidden.store(true, Ordering::Relaxed);
        ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true));
        // 清除置顶，避免窗口状态残留（未置顶时是无害的空操作）；KWin 集成仅 Linux 有
        #[cfg(target_os = "linux")]
        std::thread::spawn(crate::kwin::clear_keep_above);
        log::info!("窗口已最小化（配合跳过任务栏规则 = 仅托盘）");
    } else {
        #[cfg(windows)]
        win32_window::hide();
        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
        log::info!("窗口已隐藏到托盘");
    }
}

/// 显示窗口：恢复收起前的尺寸（若处于收起状态）
pub fn show_window_now(shared: &Arc<Shared>, ctx: &egui::Context, keep_on_top: bool) {
    shared.ui_hidden.store(false, Ordering::Relaxed);
    if is_wayland() {
        // Wayland 下 winit 的 Visible/Focus/Minimized(false) 全是空实现：
        // 无论窗口是最小化还是被其他窗口挡住，都交给 KWin 脚本取消最小化并激活（异步执行）
        #[cfg(target_os = "linux")]
        std::thread::spawn(move || crate::kwin::unminimize_activate(keep_on_top));
        #[cfg(not(target_os = "linux"))]
        let _ = keep_on_top; // 非 Linux 没有 KWin 置顶逻辑
    }
    // Windows：直接调 Win32 显示/激活（隐藏状态下渲染循环可能停摆，等不到 egui 命令）
    #[cfg(windows)]
    win32_window::show(keep_on_top);
    #[cfg(not(windows))]
    let _ = keep_on_top;
    ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
    ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
    ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
    ctx.request_repaint();
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
mod win32_window {
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

/// 是否运行在 Wayland 会话（winit 下 set_visible 不可用）
fn is_wayland() -> bool {
    std::env::var_os("WAYLAND_DISPLAY").is_some()
}

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

/// 选中按键的按压次数合计
fn sum_selected(unit: &BucketStats, selected: &std::collections::BTreeSet<usize>) -> u64 {
    selected.iter().map(|&i| unit.key_count(i)).sum()
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
