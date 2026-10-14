//! 统计数据的存储模型：总计 / 按天 / 按小时 三层桶 + JSON 持久化 + 超期清理。
//!
//! 与原版一致：鼠标各计数按“抬起”计次，键盘按“抬起”计次（仅物理输入）；
//! 鼠标移动先累计像素，显示时用屏幕物理宽度换算成米。
//!
//! 三层共用同一种桶（[`BucketStats`]）：`total`（全历史总计，含分时累计）、
//! `days[date]`（某天）、`days[date].hours[h]`（某天的某一小时）。
//! 小时明细只为“出现过输入”的小时建立，空闲小时不占空间。

use crate::keys::{N_KEYS, KEYS};
use chrono::{Duration, Local, Timelike};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::io::Write;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Instant;

pub fn today_string() -> String {
    Local::now().format("%Y%m%d").to_string()
}

/// 一天中的小时数（分时统计的桶数）
pub const HOURS_PER_DAY: usize = 24;

#[derive(Default, Clone, Serialize, Deserialize, Debug)]
pub struct MouseStats {
    /// 鼠标移动累计像素
    #[serde(default)]
    pub move_px: f64,
    #[serde(default)]
    pub lb: u64,
    #[serde(default)]
    pub rb: u64,
    #[serde(default)]
    pub mb: u64,
    /// 侧键（X1/X2 合并）
    #[serde(default)]
    pub xb: u64,
    #[serde(default)]
    pub wheel: u64,
    #[serde(default)]
    pub hwheel: u64,
}

/// 一个统计桶：总计、某天、某小时共用同一形状
#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct BucketStats {
    #[serde(default)]
    pub keystrokes: u64,
    /// 每个布局按键的计数，下标 = keys.rs 中的按键索引
    #[serde(default = "default_keys", with = "keys_compact")]
    pub keys: Box<[u64]>,
    /// 布局之外的按键（多媒体键、厂商自定义键等），键名 = "win:822" / "lin:114"
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub other: BTreeMap<String, u64>,
    #[serde(default)]
    pub mouse: MouseStats,
    /// 分时数据校验未通过（超过阈值）→ 该小时整体作废：
    /// 原始数值仍然保留在桶里（便于事后核对），但不再计入当日/总计。
    #[serde(default, skip_serializing_if = "is_false")]
    pub invalid: bool,
}

fn is_false(b: &bool) -> bool {
    !*b
}

fn default_keys() -> Box<[u64]> {
    vec![0u64; N_KEYS].into_boxed_slice()
}

impl Default for BucketStats {
    fn default() -> Self {
        BucketStats {
            keystrokes: 0,
            keys: default_keys(),
            other: BTreeMap::new(),
            mouse: MouseStats::default(),
            invalid: false,
        }
    }
}

impl BucketStats {
    pub fn add_key(&mut self, idx: usize) {
        if idx < self.keys.len() {
            self.keys[idx] += 1;
        }
        self.keystrokes += 1;
    }

    pub fn add_other_key(&mut self, id: &str) {
        *self.other.entry(id.to_string()).or_insert(0) += 1;
        self.keystrokes += 1;
    }

    pub fn add_mouse(&mut self, ev: &MouseEv) {
        match *ev {
            MouseEv::LeftUp => self.mouse.lb += 1,
            MouseEv::RightUp => self.mouse.rb += 1,
            MouseEv::MiddleUp => self.mouse.mb += 1,
            MouseEv::XUp => self.mouse.xb += 1,
            MouseEv::Wheel => self.mouse.wheel += 1,
            MouseEv::HWheel => self.mouse.hwheel += 1,
            MouseEv::MovePx(d) => self.mouse.move_px += d,
        }
    }

    /// 键计数（越界返回 0）
    pub fn key_count(&self, idx: usize) -> u64 {
        self.keys.get(idx).copied().unwrap_or(0)
    }

    /// 鼠标按键次数（左 + 右 + 中 + 侧键，不含滚轮/横滚）
    pub fn mouse_clicks(&self) -> u64 {
        self.mouse.lb + self.mouse.rb + self.mouse.mb + self.mouse.xb
    }

    /// 从本桶里减去另一个桶（用于把作废的小时从当日/总计中扣回）
    pub fn subtract(&mut self, o: &BucketStats) {
        self.keystrokes = self.keystrokes.saturating_sub(o.keystrokes);
        for (i, n) in o.keys.iter().enumerate() {
            if let Some(v) = self.keys.get_mut(i) {
                *v = v.saturating_sub(*n);
            }
        }
        for (k, n) in &o.other {
            if let Some(v) = self.other.get_mut(k) {
                *v = v.saturating_sub(*n);
                if *v == 0 {
                    self.other.remove(k);
                }
            }
        }
        let m = &mut self.mouse;
        let o = &o.mouse;
        m.move_px = (m.move_px - o.move_px).max(0.0);
        m.lb = m.lb.saturating_sub(o.lb);
        m.rb = m.rb.saturating_sub(o.rb);
        m.mb = m.mb.saturating_sub(o.mb);
        m.xb = m.xb.saturating_sub(o.xb);
        m.wheel = m.wheel.saturating_sub(o.wheel);
        m.hwheel = m.hwheel.saturating_sub(o.hwheel);
    }
}

/// 分时校验阈值：某个小时超过阈值即整体作废（原始数值保留，但不计入当日/总计）。
/// 0 表示关闭该项校验。
#[derive(Clone, Copy, Debug, Default)]
pub struct HourLimits {
    /// 鼠标移动（像素；由配置里的公里数换算而来）
    pub move_px: f64,
    /// 鼠标点击总数（左+右+中+侧）
    pub clicks: u64,
    /// 键盘敲击总数
    pub keystrokes: u64,
}

impl HourLimits {
    /// 从配置构造：公里用界面同一套换算（米 = 像素 × 屏宽毫米 / 屏宽像素 / 1000）换成像素
    pub fn from_config(cfg: &crate::config::Config, px_per_meter: f64) -> HourLimits {
        HourLimits {
            move_px: if cfg.limit_move_km > 0.0 && px_per_meter > 0.0 {
                cfg.limit_move_km * 1000.0 * px_per_meter
            } else {
                0.0
            },
            clicks: cfg.limit_clicks,
            keystrokes: cfg.limit_keystrokes,
        }
    }

    fn exceeded(&self, b: &BucketStats) -> Option<&'static str> {
        if self.move_px > 0.0 && b.mouse.move_px > self.move_px {
            return Some("鼠标移动距离");
        }
        if self.clicks > 0 && b.mouse.lb + b.mouse.rb + b.mouse.mb + b.mouse.xb > self.clicks {
            return Some("鼠标点击次数");
        }
        if self.keystrokes > 0 && b.keystrokes > self.keystrokes {
            return Some("键盘敲击次数");
        }
        None
    }
}

/// 实时手速与当日峰值跟踪器（Actions Per Minute / 每分钟操作数）
#[derive(Debug)]
pub struct ApmTracker {
    /// 滑动窗口内的动作时间戳队列
    actions: VecDeque<Instant>,
    /// 今日峰值 APM
    today_peak: u32,
    /// 峰值对应的日期（YYYYMMDD）
    peak_date: String,
}

impl ApmTracker {
    pub fn new(today: &str, initial_peak: u32) -> Self {
        ApmTracker {
            actions: VecDeque::new(),
            today_peak: initial_peak,
            peak_date: today.to_string(),
        }
    }

    /// 记录一次动作（击键或鼠标点击），返回 (当前APM, 今日峰值)
    pub fn record(&mut self, date: &str, window_secs: u32) -> (u32, u32) {
        let now = Instant::now();
        self.actions.push_back(now);
        self.cleanup(now, date, window_secs);
        (self.calc_apm(), self.today_peak)
    }

    /// 获取当前实时 APM 与当日峰值
    pub fn get(&mut self, date: &str, window_secs: u32) -> (u32, u32) {
        let now = Instant::now();
        self.cleanup(now, date, window_secs);
        (self.calc_apm(), self.today_peak)
    }

    /// 清理滑动窗口外的过期动作，并维护跨天峰值
    fn cleanup(&mut self, now: Instant, date: &str, window_secs: u32) {
        if self.peak_date != date {
            self.peak_date = date.to_string();
            self.today_peak = 0;
            self.actions.clear();
            return;
        }
        let window = std::time::Duration::from_secs(window_secs.max(1) as u64);
        while let Some(&front) = self.actions.front() {
            if now.saturating_duration_since(front) > window {
                self.actions.pop_front();
            } else {
                break;
            }
        }
        let current = self.calc_apm();
        if current > self.today_peak {
            self.today_peak = current;
        }
    }

    /// 数值 = 滑动窗口内的动作数，**不做倍率外推**：
    /// - 60 秒窗口下就是标准 APM（点一次 = 1，不会像以前那样 ×12）；
    /// - 更长的窗口只是更平滑；
    /// - 窗口调短时也不会把单次动作放大（旧版 5 秒窗口 "点一次 +12" 就是这么来的）。
    fn calc_apm(&self) -> u32 {
        self.actions.len() as u32
    }

    #[allow(dead_code)]
    pub fn peak(&self) -> u32 {
        self.today_peak
    }

    pub fn reset_peak(&mut self) {
        self.today_peak = 0;
    }
}

/// osu-kps 的激活插值（`main.cpp` 的 `_interpolate` 的比值部分）：
/// `x < t1` 时 0（不激活），到 `t2` 时约 0.8，之后按 sigmoid 趋近 1。
/// 两条颜色动画（速度→颜色、松开→淡出）都用它，保证行为和原项目一致。
pub fn osu_ramp(x: f64, t1: f64, t2: f64) -> f32 {
    if x < t1 || t2 <= t1 {
        return 0.0;
    }
    let crt = x - t1;
    const RATIO_T2: f64 = 0.9; // 在 t2 处 sigmoid 取到 0.9（映射后约 0.8）
    let a = -(1.0 / RATIO_T2 - 1.0).ln() / (t2 - t1);
    let sigmoid = 1.0 / (1.0 + (-(a * crt)).exp());
    (((sigmoid - 0.5) * 2.0) as f32).clamp(0.0, 1.0)
}

/// 按键热度：按该键自己的 KPS 插值（对齐 osu-kps：阈值 2.0 / 4.0）
pub fn kps_key_heat(per_key_kps: f64) -> f32 {
    osu_ramp(per_key_kps, 2.0, 4.0)
}

/// 监控按键的实时状态
#[derive(Clone, Debug, Default)]
pub struct KpsKeyInfo {
    pub name: String,
    /// 累计击打次数
    pub hits: u64,
    /// 当前是否**按住中**（输入层的按住 / 松开事件维护；悬浮窗「长按保持点亮」用）
    pub held: bool,
    /// 该键自己的 KPS（按当前计算方式与统计窗口）
    pub kps: f64,
    /// 发光强度（0~1）：按住期间为 1；松开后沿 osu-kps 的 50→250ms 曲线淡出。
    /// （没有松开事件来源时，退回「按下窗口内满亮」的近似）
    pub glow: f32,
}

/// KPS 计算方式（对齐 osu-kps 的「硬算 / 灵敏」）
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum KpsMethod {
    /// 硬算：数统计窗口内的击键次数（稳定）
    #[default]
    Hard,
    /// 灵敏：对齐 osu-kps 的 `kps_implement_sensitive` —— 只看最近 2.5 秒的记录，
    /// 从新到旧累加次数 k，取 `min(k × 1.5, k / 距现在最远那次的时间)` 的最大值：
    /// 单次击键最多显示 1.5（不会虚高），连击时贴近真实速率，停下来后立刻往下掉。
    Sensitive,
}

impl KpsMethod {
    pub fn from_config(s: &str) -> Self {
        match s {
            "sensitive" => Self::Sensitive,
            _ => Self::Hard,
        }
    }
    pub fn as_config(self) -> &'static str {
        match self {
            Self::Hard => "hard",
            Self::Sensitive => "sensitive",
        }
    }
}

/// KPS 图的历史条数上限（一秒一条）——对齐 osu-kps 的 300 条（近 5 分钟）
pub const KPS_HISTORY_MAX: usize = 300;
/// 「灵敏」方式只看最近这么长的记录（对齐 osu-kps 的 considered_length = 2.5s）
const SENSITIVE_CONSIDERED: std::time::Duration = std::time::Duration::from_millis(2500);

/// 键名归一后的"规范名"：左右修饰键合并到 shift / ctrl / alt。
/// 监控列表、单键计数、单键 KPS 都用这个口径，避免"监控 Shift 却按了 LShift 不算"。
fn canonical_kps_key(norm: &str) -> String {
    match norm {
        "lshift" | "rshift" => "shift".into(),
        "lctrl" | "rctrl" => "ctrl".into(),
        "lalt" | "ralt" => "alt".into(),
        _ => norm.to_string(),
    }
}

/// KPS (Keys Per Second / 按键每秒) 跟踪器（参照 osu-kps 与 KeysPerSecond）
pub struct KpsTracker {
    actions: VecDeque<Instant>,
    today_peak: f64,
    peak_date: String,
    /// 监控键最近按下的时间戳（用于界面发光点亮反馈）
    recent_presses: BTreeMap<String, Instant>,
    /// 当前**按住中**的键（输入层的按住 / 松开事件维护；与计次解耦）——「长按保持点亮」用
    held: BTreeSet<String>,
    /// 各键最近一次**松开**的时刻 —— 松开后 50→250ms 淡出用
    recent_releases: BTreeMap<String, Instant>,
    /// 监控键累计按压次数
    key_hits: BTreeMap<String, u64>,
    /// 每个监控键自己的击键时刻（单键 KPS 用）
    key_times: BTreeMap<String, VecDeque<Instant>>,
    /// 监控键集合（规范化后的键名）。**速率只统计这些键**，与 osu-kps / KeysPerSecond
    /// 一致：否则打字时的普通按键也会把 KPS 抬起来；
    /// 空集合 = 不过滤（兜底，保持没有配置监控键时的旧行为）。
    monitored: BTreeSet<String>,
    /// 本次会话（自上次重置 / 跨天）起算的监控键动作数，用于平均 KPS
    session_actions: u64,
    /// 上面那个计数的起点时刻
    session_start: Instant,
    /// 计算方式（硬算 / 灵敏）
    method: KpsMethod,
    /// KPS 图：每秒一条「该秒内的最大值」（对齐 osu-kps 的 calc_kps_recent）
    history: VecDeque<(Instant, f64)>,
    /// 当前这一秒里见过的最大值
    bucket_max: f64,
    /// 上一次采样时刻
    last_sample: Option<Instant>,
}

impl KpsTracker {
    pub fn new(today: &str) -> Self {
        KpsTracker {
            actions: VecDeque::new(),
            today_peak: 0.0,
            peak_date: today.to_string(),
            recent_presses: BTreeMap::new(),
            held: BTreeSet::new(),
            recent_releases: BTreeMap::new(),
            key_hits: BTreeMap::new(),
            key_times: BTreeMap::new(),
            monitored: BTreeSet::new(),
            session_actions: 0,
            session_start: Instant::now(),
            method: KpsMethod::Hard,
            history: VecDeque::new(),
            bucket_max: 0.0,
            last_sample: None,
        }
    }

    /// 设置计算方式（配置变化时调用）
    pub fn set_method(&mut self, method: KpsMethod) {
        self.method = method;
    }

    pub fn method(&self) -> KpsMethod {
        self.method
    }

    /// 更新监控键列表（配置变化 / 启动时调用）。键名按 [`normalize_kps_key`] 归一，空项忽略。
    pub fn set_monitored(&mut self, keys: &[String]) {
        let set: BTreeSet<String> = keys
            .iter()
            .map(|k| normalize_kps_key(k))
            .filter(|k| !k.is_empty())
            .collect();
        if set != self.monitored {
            log::debug!("KPS 监控键更新为 {set:?}");
            self.monitored = set;
        }
    }

    /// 记录一次动作（击键或鼠标），key_id 比如 "Z", "Space", "M1"。
    /// 返回 (当前 KPS, 今日峰值)，两个都是 f64（灵敏方式是小数）。
    pub fn record(&mut self, date: &str, key_id: &str, window_ms: u32) -> (f64, f64) {
        let now = Instant::now();
        let norm = normalize_kps_key(key_id);
        let canon = canonical_kps_key(&norm);
        // 速率只统计监控键（未配置监控键时退化为统计全部动作）
        let counted =
            self.monitored.is_empty() || self.monitored.contains(&norm) || self.monitored.contains(&canon);
        if counted {
            self.actions.push_back(now);
            self.session_actions += 1;
            if !canon.is_empty() {
                let q = self.key_times.entry(canon.clone()).or_default();
                q.push_back(now);
            }
        }
        if !norm.is_empty() {
            self.recent_presses.insert(norm.clone(), now);
            *self.key_hits.entry(norm.clone()).or_insert(0) += 1;
            if canon != norm {
                self.recent_presses.insert(canon.clone(), now);
                *self.key_hits.entry(canon.clone()).or_insert(0) += 1;
            }
        }
        self.cleanup(now, date, window_ms);
        (self.calc_kps(&self.actions, now, window_ms), self.today_peak)
    }

    /// 更新某键的「按住」状态（输入层在按下 / 松开时调用）。
    /// 与 [`record`](Self::record) 完全解耦：不计次、不动 KPS、不落盘 ——
    /// 悬浮窗的「长按保持点亮」靠它 + `recent_releases` 实现。
    pub fn set_held(&mut self, key_id: &str, down: bool) {
        let now = Instant::now();
        let norm = normalize_kps_key(key_id);
        if norm.is_empty() {
            return;
        }
        let canon = canonical_kps_key(&norm);
        if down {
            self.held.insert(norm.clone());
            if canon != norm {
                self.held.insert(canon);
            }
        } else {
            // 松开：清「按住」并记下时刻（淡出起点）。
            // 注：左右修饰键同时按住这类极端情况按「任一松开即灭」处理，与计次口径一致。
            let distinct = canon != norm;
            self.held.remove(&norm);
            self.recent_releases.insert(norm, now);
            if distinct {
                self.held.remove(&canon);
                self.recent_releases.insert(canon, now);
            }
        }
    }

    /// 获取当前实时 KPS 与今日峰值
    pub fn get(&mut self, date: &str, window_ms: u32) -> (f64, f64) {
        let now = Instant::now();
        self.cleanup(now, date, window_ms);
        (self.calc_kps(&self.actions, now, window_ms), self.today_peak)
    }

    /// 查询监控按键列表的当前状态（名称、命中次数、是否刚按下、该键自己的 KPS）
    pub fn query_keys(&self, keys: &[String], window_ms: u32) -> Vec<KpsKeyInfo> {
        let now = Instant::now();
        let active_duration = std::time::Duration::from_millis(130);
        keys.iter()
            .map(|k| {
                let norm = normalize_kps_key(k);
                let canon = canonical_kps_key(&norm);
                let hits = self.key_hits.get(&norm).copied().unwrap_or(0);
                // 按住中：输入层的按住 / 松开事件维护（长按保持点亮；含左右修饰键的规范名）
                let held = self.held.contains(&norm) || self.held.contains(&canon);
                let empty = VecDeque::new();
                let times = self.key_times.get(&canon).unwrap_or(&empty);
                // 发光（底板亮度）：按住期间保持满亮（对齐 osu-kps 的 down 状态）；
                // 松开后沿 50→250ms 曲线渐近淡出（250ms 处约 80%）；没有松开信息时退回
                // 「按下窗口内满亮」的近似
                let glow = if held {
                    1.0
                } else if let Some(r) = self.recent_releases.get(&norm) {
                    1.0 - osu_ramp(now.saturating_duration_since(*r).as_secs_f64(), 0.05, 0.25)
                } else {
                    self.recent_presses
                        .get(&norm)
                        .map(|t| {
                            let dt = now.saturating_duration_since(*t).as_secs_f64();
                            let win = active_duration.as_secs_f64(); // 按下窗口（无松开信息时的近似）
                            if dt < win {
                                1.0
                            } else {
                                1.0 - osu_ramp(dt - win, 0.05, 0.25)
                            }
                        })
                        .unwrap_or(0.0)
                };
                KpsKeyInfo {
                    name: k.trim().to_string(),
                    hits,
                    held,
                    kps: self.calc_kps(times, now, window_ms),
                    glow,
                }
            })
            .collect()
    }

    /// 平均 KPS：自上次重置 / 跨天以来的监控键动作数 ÷ 经过秒数。
    /// 参照 KeysPerSecond 的 "AVG" 计数器 —— 统计的是**会话平均**，不是滑动窗口速率。
    pub fn avg(&self) -> f64 {
        let secs = self.session_start.elapsed().as_secs_f64();
        if self.session_actions == 0 || secs < 1.0 {
            0.0
        } else {
            self.session_actions as f64 / secs
        }
    }

    /// 今日峰值（设置窗口的重置按钮 / 测试用）
    #[allow(dead_code)]
    pub fn peak(&self) -> f64 {
        self.today_peak
    }

    /// KPS 图的历史（每秒一条，最新在末尾）
    pub fn history(&self) -> &VecDeque<(Instant, f64)> {
        &self.history
    }

    fn cleanup(&mut self, now: Instant, date: &str, window_ms: u32) {
        if self.peak_date != date {
            self.peak_date = date.to_string();
            self.today_peak = 0.0;
            self.actions.clear();
            self.key_hits.clear();
            self.recent_presses.clear();
            self.recent_releases.clear();
            self.key_times.clear();
            self.history.clear();
            self.bucket_max = 0.0;
            self.last_sample = None;
            self.session_actions = 0;
            self.session_start = now;
            return;
        }
        // 保留长度：至少覆盖统计窗口；灵敏方式还要 2.5 秒
        let keep = std::time::Duration::from_millis(window_ms.max(100) as u64).max(SENSITIVE_CONSIDERED);
        while let Some(&front) = self.actions.front() {
            if now.saturating_duration_since(front) > keep {
                self.actions.pop_front();
            } else {
                break;
            }
        }
        for q in self.key_times.values_mut() {
            while let Some(&front) = q.front() {
                if now.saturating_duration_since(front) > keep {
                    q.pop_front();
                } else {
                    break;
                }
            }
        }
        let current = self.calc_kps(&self.actions, now, window_ms);
        if current > self.today_peak {
            self.today_peak = current;
        }
        // KPS 图：每秒采一条「该秒内的最大值」（对齐 osu-kps 一秒一条的历史）
        match self.last_sample {
            Some(t) if now.saturating_duration_since(t) < std::time::Duration::from_millis(1000) => {
                if current > self.bucket_max {
                    self.bucket_max = current;
                }
            }
            _ => {
                let sample = self.bucket_max.max(current);
                self.history.push_back((now, sample));
                self.bucket_max = current;
                self.last_sample = Some(now);
                while self.history.len() > KPS_HISTORY_MAX {
                    self.history.pop_front();
                }
            }
        }
    }

    /// 按当前计算方式算 KPS（`times` 是该口径下的击键时刻表）
    fn calc_kps(&self, times: &VecDeque<Instant>, now: Instant, window_ms: u32) -> f64 {
        match self.method {
            KpsMethod::Hard => {
                let window = std::time::Duration::from_millis(window_ms.max(100) as u64);
                let n = times
                    .iter()
                    .rev()
                    .take_while(|t| now.saturating_duration_since(**t) <= window)
                    .count();
                let sec = (window_ms.max(100) as f64 / 1000.0).max(1.0);
                n as f64 / sec
            }
            KpsMethod::Sensitive => {
                let mut best = 0.0f64;
                let mut count = 0u32;
                for t in times.iter().rev() {
                    let dt = now.saturating_duration_since(*t);
                    if dt > SENSITIVE_CONSIDERED {
                        break;
                    }
                    count += 1;
                    let elapsed = dt.as_secs_f64().max(0.001);
                    let cur = ((count as f64) * 1.5).min(count as f64 / elapsed);
                    if cur >= best {
                        best = cur;
                    } else {
                        break;
                    }
                }
                best
            }
        }
    }

    /// 只清今日峰值（设置窗口「重置 → 最大 KPS」）
    pub fn reset_peak(&mut self) {
        self.today_peak = 0.0;
    }

    /// 只清按键总次数（每个键的累计击打数；「重置 → 按键总次数」）
    pub fn reset_counts(&mut self) {
        self.key_hits.clear();
    }

    /// 只清 KPS 图的历史（「重置 → KPS 图」）
    pub fn reset_graph(&mut self) {
        self.history.clear();
        self.bucket_max = 0.0;
        self.last_sample = None;
    }

    /// 全部清零（「重置 → 以上所有」）：峰值 + 按键总次数 + KPS 图 + 平均 KPS 的起算点
    pub fn reset_all(&mut self) {
        self.reset_peak();
        self.reset_counts();
        self.reset_graph();
        self.session_actions = 0;
        self.session_start = Instant::now();
        // 长按状态也归零（清掉可能残留的点亮）
        self.held.clear();
        self.recent_releases.clear();
    }
}


pub fn normalize_kps_key(k: &str) -> String {
    let s = k.trim().to_lowercase();
    match s.as_str() {
        "m1" | "mouse1" | "left" | "lbutton" => "m1".into(),
        "m2" | "mouse2" | "right" | "rbutton" => "m2".into(),
        "m3" | "mouse3" | "middle" | "mbutton" => "m3".into(),
        "m4" | "mouse4" | "side" | "xbutton" | "xb1" => "m4".into(),
        "m5" | "mouse5" | "extra" | "xb2" => "m5".into(),
        "d0" => "0".into(),
        "d1" => "1".into(),
        "d2" => "2".into(),
        "d3" => "3".into(),
        "d4" => "4".into(),
        "d5" => "5".into(),
        "d6" => "6".into(),
        "d7" => "7".into(),
        "d8" => "8".into(),
        "d9" => "9".into(),
        _ => s,
    }
}

fn is_zero_u32(v: &u32) -> bool {
    *v == 0
}

/// 一天的数据：当天汇总（扁平字段，与旧版文件兼容）+ 各小时明细
#[derive(Clone, Default, Serialize, Deserialize, Debug)]
pub struct DayEntry {
    #[serde(flatten)]
    pub bucket: BucketStats,
    /// 当日峰值 APM（每分钟操作数）
    #[serde(default, skip_serializing_if = "is_zero_u32")]
    pub peak_apm: u32,
    /// 小时（0-23）→ 该小时明细；只存有数据的小时
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub hours: BTreeMap<u8, BucketStats>,
}

// 让 `day.keystrokes` / `day.keys` / `day.mouse` 这类读取保持简洁
impl std::ops::Deref for DayEntry {
    type Target = BucketStats;
    fn deref(&self) -> &BucketStats {
        &self.bucket
    }
}

impl std::ops::DerefMut for DayEntry {
    fn deref_mut(&mut self) -> &mut BucketStats {
        &mut self.bucket
    }
}

/// 一个小时里的三项数据（键盘上方那条分时折线图用），各自独立量程
#[derive(Clone, Copy, Default, Debug, PartialEq)]
pub struct HourPoint {
    /// 键盘敲击次数
    pub keystrokes: u64,
    /// 鼠标按键次数（左 + 右 + 中 + 侧键）
    pub clicks: u64,
    /// 鼠标移动像素
    pub move_px: f64,
}

impl DayEntry {
    /// 取某小时的桶（不存在则创建）
    pub fn hour_mut(&mut self, h: u8) -> &mut BucketStats {
        self.hours.entry(h.min(HOURS_PER_DAY as u8 - 1)).or_default()
    }

    pub fn hour(&self, h: u8) -> Option<&BucketStats> {
        self.hours.get(&h)
    }

    /// 视图取值：None = 整天/总计，Some(h) = 该小时（无数据时返回空桶）。
    /// 作废的小时按 0 计（原始数值仍留在桶里，只有明确要看证据时才直接读 `hour()`）。
    pub fn view(&self, hour: Option<u8>) -> BucketStats {
        match hour {
            None => self.bucket.clone(),
            Some(h) => self.hours.get(&h).filter(|u| !u.invalid).cloned().unwrap_or_default(),
        }
    }

    /// 某小时是否被判定作废（无该小时 = false）
    pub fn hour_invalid(&self, h: u8) -> bool {
        self.hours.get(&h).map(|u| u.invalid).unwrap_or(false)
    }

    /// 24 小时分时曲线（键盘敲击、鼠标按键、鼠标移动），无数据的小时为 0
    pub fn hour_profile(&self) -> Vec<HourPoint> {
        let mut out = vec![HourPoint::default(); HOURS_PER_DAY];
        for (h, u) in &self.hours {
            if let Some(slot) = out.get_mut(*h as usize) {
                *slot = HourPoint { keystrokes: u.keystrokes, clicks: u.mouse_clicks(), move_px: u.mouse.move_px };
            }
        }
        out
    }
}

/// `keys` 的单行紧凑序列化：101 个数字若按 pretty 缩进会铺成 101 行，
/// 再乘以 24 个小时会让文件膨胀十倍，因此写成一行 `[0,0,3,…]`（仍是标准 JSON 数组）。
mod keys_compact {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    pub fn serialize<S: Serializer>(keys: &[u64], ser: S) -> Result<S::Ok, S::Error> {
        let mut s = String::with_capacity(keys.len() * 3 + 2);
        s.push('[');
        for (i, k) in keys.iter().enumerate() {
            if i > 0 {
                s.push(',');
            }
            s.push_str(&k.to_string());
        }
        s.push(']');
        // RawValue：原样写入，不会被 pretty 打印器展开成 101 行
        let raw = serde_json::value::RawValue::from_string(s)
            .map_err(<S::Error as serde::ser::Error>::custom)?;
        raw.serialize(ser)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(de: D) -> Result<Box<[u64]>, D::Error> {
        // 单行与缩进展开两种写法解析结果一致（标准 JSON 忽略空白）
        Ok(Vec::<u64>::deserialize(de)?.into_boxed_slice())
    }
}

#[derive(Clone, Copy, Debug)]
pub enum MouseEv {
    LeftUp,
    RightUp,
    MiddleUp,
    XUp,
    Wheel,
    HWheel,
    MovePx(f64),
}

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct Store {
    #[serde(default = "default_version")]
    pub version: u32,
    /// 日期 "YYYYMMDD" → 当日数据（含分时明细）
    #[serde(default)]
    pub days: BTreeMap<String, DayEntry>,
    /// 全历史总计（不随过期清理而减少，与原版一致）；其 hours 为“一天中各小时的累计”
    #[serde(default)]
    pub total: DayEntry,
    /// 分时校验阈值（运行时设置，不写进文件）
    #[serde(skip)]
    pub limits: HourLimits,
}

fn default_version() -> u32 {
    1
}

impl Default for Store {
    fn default() -> Self {
        Store { version: default_version(), days: BTreeMap::new(), total: DayEntry::default(), limits: HourLimits::default() }
    }
}

/// 容错：确保 keys 数组长度正确（布局升级 / 旧数据 / 手改文件），
/// 并把早期版本记在 `other` 里的系统键计数搬回按键槽位。
fn normalize(b: &mut BucketStats) {
    let mut v = std::mem::take(&mut b.keys).into_vec();
    v.resize(N_KEYS, 0);
    b.keys = v.into_boxed_slice();
    migrate_other_system_keys(b);
}

/// 早期版本的键盘布局里没有 PrtScr / ScrollLock / Pause，这几个键的计数记在 `other`
/// （`"win:311"` 这种 ID）。布局补上这三个键后把历史计数搬回按键槽位：
/// 搬过的从 `other` 删除，所以重复执行无副作用；其他布局外按键保持不动。
fn migrate_other_system_keys(b: &mut BucketStats) {
    const MOVES: [(&str, &str); 6] = [
        ("win:311", "PrtScr"),
        ("lin:99", "PrtScr"),
        ("win:70", "ScrollLock"),
        ("lin:70", "ScrollLock"),
        ("win:69", "Pause"),
        ("lin:119", "Pause"),
    ];
    for (id, name) in MOVES {
        // 只有确实存在目标槽位时才搬走计数；找不到槽位就留在 other 里，
        // 不能白白删掉（布局再变时这条计数还能显示出来）
        if let Some(i) = crate::keys::idx_by_name(name) {
            if let Some(n) = b.other.remove(id) {
                b.keys[i] = b.keys[i].saturating_add(n);
            }
        }
    }
}

impl Store {
    pub fn load(path: &Path) -> Store {
        crate::fsutil::clean_stale_tmp(path);
        match std::fs::read(path) {
            Ok(bytes) => match serde_json::from_slice::<Store>(&bytes) {
                Ok(mut s) => {
                    for d in s.days.values_mut() {
                        for h in d.hours.values_mut() {
                            normalize(h);
                        }
                        normalize(&mut d.bucket);
                    }
                    normalize(&mut s.total.bucket);
                    for h in s.total.hours.values_mut() {
                        normalize(h);
                        // 旧版本把「某天某小时作废」的标记误打在了跨天累计桶上，
                        // 读进来时清掉，否则总计视图里该小时会被永久画成作废
                        h.invalid = false;
                    }
                    s
                }
                Err(e) => {
                    // 尝试备份损坏文件
                    let bak = path.with_extension("json.bak");
                    let _ = std::fs::rename(path, &bak);
                    log::warn!("统计数据解析失败({e})，原文件已备份为 {}，从头开始统计", bak.display());
                    Store::default()
                }
            },
            Err(_) => Store::default(),
        }
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        // 临时名带 pid/序号：GUI、后台 saver、信号处理等多个写入方可能同时保存，
        // 固定名会互相覆盖、落盘出无法解析的 stats.json（启动时会被当成损坏文件清空）
        let tmp = crate::fsutil::unique_tmp(path);
        let written = (|| -> std::io::Result<()> {
            let f = std::fs::File::create(&tmp)?;
            let mut w = std::io::BufWriter::new(f);
            serde_json::to_writer_pretty(&mut w, self)?;
            w.flush()?;
            w.get_ref().sync_all().ok();
            Ok(())
        })();
        if let Err(e) = written {
            let _ = std::fs::remove_file(&tmp);
            return Err(e);
        }
        if let Err(e) = std::fs::rename(&tmp, path) {
            let _ = std::fs::remove_file(&tmp);
            return Err(e);
        }
        Ok(())
    }

    /// 获取（或创建）某日数据
    #[allow(dead_code)] // 主流程走 bump_*，此接口供测试/工具使用
    pub fn day_mut(&mut self, date: &str) -> &mut DayEntry {
        self.days.entry(date.to_string()).or_insert_with(DayEntry::default)
    }

    pub fn day(&self, date: &str) -> Option<&DayEntry> {
        self.days.get(date)
    }

    /// 某日峰值 APM
    pub fn day_peak_apm(&self, date: &str) -> u32 {
        self.days.get(date).map(|d| d.peak_apm).unwrap_or(0)
    }

    /// 某日某小时（只读）
    pub fn hour(&self, date: &str, hour: u8) -> Option<&BucketStats> {
        self.days.get(date).and_then(|d| d.hour(hour))
    }

    /// 清理超期历史数据，返回删除的天数（小时明细随天一起删除）
    pub fn purge(&mut self, history_days: u32) -> usize {
        let cutoff = (Local::now().date_naive() - Duration::days(history_days as i64))
            .format("%Y%m%d")
            .to_string();
        let before = self.days.len();
        self.days.retain(|d, _| d.as_str() >= cutoff.as_str());
        before - self.days.len()
    }

    /// 按索引给键计次（该小时 + 当日 + 总计 + 总计分时）。
    /// 若该小时已被判定作废，则只累加它自己的原始数值，不再计入当日/总计。
    pub fn bump_key(&mut self, date: &str, hour: u8, idx: usize) {
        let limits = self.limits;
        let d = self.days.entry(date.to_string()).or_default();
        let h = d.hour_mut(hour);
        h.add_key(idx);
        if h.invalid {
            return;
        }
        d.bucket.add_key(idx);
        self.total.bucket.add_key(idx);
        self.total.hour_mut(hour).add_key(idx);
        self.check_limits(date, hour, limits);
    }

    pub fn bump_other_key(&mut self, date: &str, hour: u8, id: &str) {
        let limits = self.limits;
        let d = self.days.entry(date.to_string()).or_default();
        let h = d.hour_mut(hour);
        h.add_other_key(id);
        if h.invalid {
            return;
        }
        d.bucket.add_other_key(id);
        self.total.bucket.add_other_key(id);
        self.total.hour_mut(hour).add_other_key(id);
        self.check_limits(date, hour, limits);
    }

    pub fn bump_mouse(&mut self, date: &str, hour: u8, ev: &MouseEv) {
        let limits = self.limits;
        let d = self.days.entry(date.to_string()).or_default();
        let h = d.hour_mut(hour);
        h.add_mouse(ev);
        if h.invalid {
            return;
        }
        d.bucket.add_mouse(ev);
        self.total.bucket.add_mouse(ev);
        self.total.hour_mut(hour).add_mouse(ev);
        self.check_limits(date, hour, limits);
    }

    /// 校验某个小时是否越限；越限则把它整小时从当日/总计里扣回并打上标记
    fn check_limits(&mut self, date: &str, hour: u8, limits: HourLimits) {
        let Some(reason) = self
            .days
            .get(date)
            .and_then(|d| d.hour(hour))
            .and_then(|h| if h.invalid { None } else { limits.exceeded(h) })
        else {
            return;
        };
        let Some(hour_bucket) = self.days.get(date).and_then(|d| d.hour(hour)).cloned() else {
            return;
        };
        if let Some(d) = self.days.get_mut(date) {
            d.bucket.subtract(&hour_bucket);
            if let Some(h) = d.hours.get_mut(&hour) {
                h.invalid = true; // 原始数值保留，只打标记
            }
        }
        self.total.bucket.subtract(&hour_bucket);
        // total.hours 是「所有天该小时的累计」，作废的是「某天的某小时」：
        // 只把这一份从累计里扣掉，不能给累计桶打 invalid —— 否则其它天同一小时的有效数据
        // 会被永久隐藏（标记没有任何地方会复位）。
        if let Some(h) = self.total.hours.get_mut(&hour) {
            h.subtract(&hour_bucket);
            h.invalid = false; // 兼容旧文件里被误打在累计桶上的标记
        }
        log::warn!(
            "{} {hour} 时{reason}超过阈值，该小时已作废（原始数值保留，不计入当日/总计）",
            date
        );
    }

    /// 重置某日数据（总计不回退，与原版无重置功能的行为差异见 README）
    pub fn reset_day(&mut self, date: &str) {
        self.days.remove(date);
    }
}

/// 输入线程与 GUI 之间的共享状态
pub struct Shared {
    pub store: Mutex<Store>,
    /// 当前日期（跨夜由任意一方滚动）
    pub today: Mutex<String>,
    /// 当前小时 0-23（随每次输入事件刷新，供分时统计落桶）
    pub hour: AtomicU32,
    /// 有新数据未显示/未保存
    pub dirty: AtomicBool,
    /// 实时手速跟踪器（滑动窗口 + 当日峰值）
    pub apm: Mutex<ApmTracker>,
    /// KPS 跟踪器（键位实时发光与按键每秒）
    pub kps: Mutex<KpsTracker>,
    /// APM 滑动窗口大小（秒），默认 60
    pub apm_window_secs: AtomicU32,
    /// KPS 滑动窗口大小（毫秒），默认 1000
    pub kps_window_ms: AtomicU32,
    /// 「设置键位」捕获模式：为 true 时输入线程把每个动作的键名推进 `kps_capture_buf`
    pub kps_capture_active: AtomicBool,
    /// 捕获到的键名（按发生顺序，最多留 26 个）
    pub kps_capture_buf: Mutex<Vec<String>>,
    /// 主显示器宽度（像素），用于像素→米换算
    pub screen_px_w: AtomicU32,
    /// 主显示器物理宽度（毫米），启动时解析
    pub screen_w_mm: AtomicU32,
    pub screen_h_mm: AtomicU32,
    /// 窗口是否处于“收起”状态（Wayland 下 1x1 收缩）
    pub ui_hidden: AtomicBool,
    /// 输入捕获是否正常工作
    pub input_ok: AtomicBool,
    /// 输入捕获的状态说明（警告信息）
    pub input_msg: Mutex<String>,
}

impl Shared {
    pub fn new(store: Store, screen_w_mm: u32, screen_h_mm: u32) -> Arc<Shared> {
        let today = today_string();
        let initial_peak = store.day_peak_apm(&today);
        Arc::new(Shared {
            today: Mutex::new(today.clone()),
            hour: AtomicU32::new(Local::now().hour()),
            apm: Mutex::new(ApmTracker::new(&today, initial_peak)),
            kps: Mutex::new(KpsTracker::new(&today)),
            apm_window_secs: AtomicU32::new(60),
            kps_window_ms: AtomicU32::new(1000),
            kps_capture_active: AtomicBool::new(false),
            kps_capture_buf: Mutex::new(Vec::new()),
            store: Mutex::new(store),
            dirty: AtomicBool::new(false),
            screen_px_w: AtomicU32::new(0),
            screen_w_mm: AtomicU32::new(screen_w_mm),
            screen_h_mm: AtomicU32::new(screen_h_mm),
            ui_hidden: AtomicBool::new(false),
            input_ok: AtomicBool::new(false),
            input_msg: Mutex::new(String::new()),
        })
    }

    pub fn set_input_status(&self, ok: bool, msg: String) {
        self.input_ok.store(ok, Ordering::Relaxed);
        *self.input_msg.lock() = msg;
    }

    /// 跨夜/跨小时滚动：刷新 today 与 hour（输入线程每收到一个事件调用一次）。
    /// 返回日期是否变化（跨夜需要清理与整轮快照刷新）。
    pub fn rollover_if_needed(&self) -> bool {
        let now = Local::now();
        let date = now.format("%Y%m%d").to_string();
        self.hour.store(now.hour(), Ordering::Relaxed);
        let mut t = self.today.lock();
        if *t != date {
            *t = date.clone();
            let apm_w = self.apm_window_secs.load(Ordering::Relaxed);
            let kps_w = self.kps_window_ms.load(Ordering::Relaxed);
            self.apm.lock().cleanup(Instant::now(), &date, apm_w);
            self.kps.lock().cleanup(Instant::now(), &date, kps_w);
            self.dirty.store(true, Ordering::Relaxed);
            true
        } else {
            false
        }
    }

    /// 当前小时（供输入线程落桶；GUI 需要“此刻”时直接读系统时钟）
    pub fn current_hour(&self) -> u8 {
        self.hour.load(Ordering::Relaxed).min(HOURS_PER_DAY as u32 - 1) as u8
    }

    /// 像素 → 米
    pub fn px_to_meters(&self, px: f64) -> f64 {
        let mm = self.screen_w_mm.load(Ordering::Relaxed) as f64;
        let pxw = self.screen_px_w.load(Ordering::Relaxed) as f64;
        if mm <= 0.0 || pxw <= 0.0 {
            return 0.0;
        }
        px * mm / pxw / 1000.0
    }
}

/// 布局外按键的 ID 前缀
#[allow(dead_code)] // 仅 Windows 捕获使用
pub fn other_key_id_win(sc: u16) -> String {
    format!("win:{sc}")
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub fn other_key_id_lin(code: u16) -> String {
    format!("lin:{code}")
}

/// 布局外按键的显示名
pub fn other_key_label(id: &str) -> String {
    if let Some(code) = id.strip_prefix("win:") {
        format!("Win sc{}", code)
    } else if let Some(code) = id.strip_prefix("lin:") {
        format!("Lin code{}", code)
    } else {
        id.to_string()
    }
}

/// 按键索引 → 内部名
#[allow(dead_code)]
pub fn key_name(idx: usize) -> &'static str {
    KEYS[idx].name
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("kmcounter_test_{tag}_{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    use std::path::PathBuf;

    /// 布局外记在 other 里的 PrtScr / ScrollLock / Pause 应搬回按键槽位（幂等）
    #[test]
    fn migrates_system_keys_from_other() {
        let name = |n: &str| crate::keys::idx_by_name(n).unwrap();
        let mut b = BucketStats::default();
        b.other.insert("win:311".into(), 5); // PrintScreen（Windows 扫描码）
        b.other.insert("win:70".into(), 3); // ScrollLock
        b.other.insert("lin:119".into(), 2); // Pause（Linux evdev 119）
        b.other.insert("lin:114".into(), 99); // 音量键：不是布局内的键，不该被动
        normalize(&mut b);
        assert_eq!(b.key_count(name("PrtScr")), 5);
        assert_eq!(b.key_count(name("ScrollLock")), 3);
        assert_eq!(b.key_count(name("Pause")), 2);
        assert_eq!(b.other.get("lin:114"), Some(&99), "布局外按键应原样保留");
        assert!(!b.other.contains_key("win:311"), "搬过的条目应删除");
        // 再跑一次不会重复累加
        normalize(&mut b);
        assert_eq!(b.key_count(name("PrtScr")), 5);
        assert_eq!(b.key_count(name("ScrollLock")), 3);
    }

    /// keys 数组长度按当前布局补齐（布局升级后旧文件仍是 101 长度）
    #[test]
    fn normalize_pads_keys_to_layout_len() {
        let mut b = BucketStats::default();
        b.keys = vec![1u64; 101].into_boxed_slice();
        normalize(&mut b);
        assert_eq!(b.keys.len(), crate::keys::N_KEYS);
        assert!(b.keys[..101].iter().all(|v| *v == 1));
        assert!(b.keys[101..].iter().all(|v| *v == 0));
    }

    #[test]
    fn store_roundtrip_with_hours() {
        let mut s = Store::default();
        let d = today_string();
        s.bump_key(&d, 9, 5);
        s.bump_key(&d, 9, 5);
        s.bump_key(&d, 14, 5);
        s.bump_other_key(&d, 14, "win:822"); // 布局外的键（PrtSc 之类现在已在布局里）
        s.bump_mouse(&d, 14, &MouseEv::LeftUp);
        s.bump_mouse(&d, 14, &MouseEv::MovePx(123.0));

        // 分时：9 时两次、14 时一次
        assert_eq!(s.hour(&d, 9).unwrap().key_count(5), 2);
        assert_eq!(s.hour(&d, 9).unwrap().keystrokes, 2);
        assert_eq!(s.hour(&d, 14).unwrap().keystrokes, 2); // 一次按键 + 一次布局外按键
        assert_eq!(s.hour(&d, 14).unwrap().mouse.move_px, 123.0);
        assert!(s.hour(&d, 10).is_none(), "没有输入的小时不应建桶");
        // 总计分时与该小时一致
        assert_eq!(s.total.hour(14).unwrap().key_count(5), 1);
        assert_eq!(s.total.hour(9).unwrap().mouse.lb, 0);

        let dir = tmp_dir("roundtrip");
        let path = dir.join("stats.json");
        s.save(&path).unwrap();

        // 单行紧凑键数组：101 个数字不铺行
        let text = std::fs::read_to_string(&path).unwrap();
        let keys_lines = text.lines().filter(|l| l.trim_start().starts_with("\"keys\":")).count();
        assert!(keys_lines >= 3, "预期 daily/hourly/total 都有 keys 字段");
        for l in text.lines().filter(|l| l.trim_start().starts_with("\"keys\":")) {
            assert!(l.trim_end().trim_end_matches(',').ends_with(']'), "keys 必须是单行数组: {l}");
        }

        let s2 = Store::load(&path);
        assert_eq!(s2.days[&d].key_count(5), 3);
        assert_eq!(s2.days[&d].keystrokes, 4); // 三次按键 + 一次布局外按键
        assert_eq!(s2.total.mouse.lb, 1);
        assert_eq!(s2.days[&d].other["win:822"], 1);
        assert_eq!(s2.hour(&d, 9).unwrap().key_count(5), 2);
        assert_eq!(s2.hour(&d, 14).unwrap().mouse.move_px, 123.0);
        assert_eq!(s2.days[&d].hours.len(), 2, "只保留有数据的小时");
        std::fs::remove_dir_all(dir).ok();
    }

    /// 旧文件把作废标记打在跨天累计桶 `total.hours` 上：读取时清掉，
    /// 否则总计视图里该小时会被永久画成作废（标记没有任何地方会复位）
    #[test]
    fn legacy_total_hour_invalid_flag_is_cleared_on_load() {
        let json = "{\"version\":1,\"days\":{},\"total\":{\"hours\":{\"10\":{\"keystrokes\":5,\"invalid\":true}}}}";
        let dir = tmp_dir("legacy_total_invalid");
        let path = dir.join("stats.json");
        std::fs::write(&path, json).unwrap();
        let s = Store::load(&path);
        assert!(!s.total.hours[&10].invalid, "累计桶上的旧作废标记应被清掉");
        assert_eq!(s.total.hours[&10].keystrokes, 5, "数值本身保留");
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn loads_legacy_day_without_hours() {
        // 旧版（无 hours、keys 缩进展开）文件必须能读
        let mut day = String::from("{\"keystrokes\": 7, \"keys\": [");
        for i in 0..N_KEYS {
            if i > 0 {
                day.push(',');
            }
            day.push(if i == 3 { '5' } else { '0' });
        }
        day.push_str("], \"mouse\": {\"move_px\": 10.0, \"lb\": 1}}");
        let json = format!("{{\"version\":1,\"days\":{{\"20260101\":{day}}},\"total\":{day}}}");
        let dir = tmp_dir("legacy");
        let path = dir.join("stats.json");
        std::fs::write(&path, json).unwrap();
        let s = Store::load(&path);
        assert_eq!(s.days["20260101"].key_count(3), 5);
        assert_eq!(s.days["20260101"].keystrokes, 7);
        assert!(s.days["20260101"].hours.is_empty());
        assert_eq!(s.total.mouse.lb, 1);
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn short_keys_array_is_resized() {
        let json = "{\"version\":1,\"days\":{\"20260101\":{\"keystrokes\":1,\"keys\":[1,2]}},\"total\":{\"keys\":[]}}";
        let dir = tmp_dir("resize");
        let path = dir.join("stats.json");
        std::fs::write(&path, json).unwrap();
        let s = Store::load(&path);
        assert_eq!(s.days["20260101"].keys.len(), N_KEYS);
        assert_eq!(s.days["20260101"].key_count(1), 2);
        assert_eq!(s.total.keys.len(), N_KEYS);
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn hour_profile_and_view() {
        let mut s = Store::default();
        let d = today_string();
        s.bump_key(&d, 8, 1);
        s.bump_key(&d, 8, 2);
        s.bump_key(&d, 20, 3);
        // 鼠标：8 时左键 3 次 + 滚轮 9 次（滚轮不算「鼠标按键」），20 时移动 500px
        for _ in 0..3 {
            s.bump_mouse(&d, 8, &MouseEv::LeftUp);
        }
        for _ in 0..9 {
            s.bump_mouse(&d, 8, &MouseEv::Wheel);
        }
        s.bump_mouse(&d, 20, &MouseEv::MovePx(500.0));
        let day = s.day(&d).unwrap();
        let prof = day.hour_profile();
        assert_eq!(prof.len(), 24);
        assert_eq!((prof[8].keystrokes, prof[8].clicks, prof[8].move_px), (2, 3, 0.0));
        assert_eq!(prof[20].keystrokes, 1);
        assert_eq!(prof[20].move_px, 500.0);
        assert_eq!(prof[20].clicks, 0);
        assert_eq!(prof[21].keystrokes, 0);
        // 滚轮不计入 mouse_clicks
        assert_eq!(day.view(Some(8)).mouse_clicks(), 3);
        assert_eq!(day.view(Some(8)).mouse.wheel, 9);
        // 视图：整天 / 指定小时 / 无数据小时
        assert_eq!(day.view(None).keystrokes, 3);
        assert_eq!(day.view(Some(8)).keystrokes, 2);
        assert_eq!(day.view(Some(8)).key_count(3), 0);
        assert_eq!(day.view(Some(23)).keystrokes, 0);
    }

    #[test]
    fn hour_limits_void_the_hour_only() {
        let mut s = Store::default();
        let d = today_string();
        // 阈值：鼠标 1 km（换算成像素）、点击 10 次、键盘 100 次
        s.limits = HourLimits { move_px: 1000.0, clicks: 10, keystrokes: 100 };

        // 9 时：鼠标只走 800 px（未越限）→ 正常计入
        s.bump_mouse(&d, 9, &MouseEv::MovePx(800.0));
        assert!(!s.hour(&d, 9).unwrap().invalid);
        assert_eq!(s.days[&d].mouse.move_px, 800.0);
        assert_eq!(s.total.mouse.move_px, 800.0);

        // 10 时：越限 → 该小时作废，原始值保留，当日/总计不含它
        s.bump_mouse(&d, 10, &MouseEv::MovePx(600.0));
        s.bump_mouse(&d, 10, &MouseEv::MovePx(600.0)); // 1200 > 1000
        let h10 = s.hour(&d, 10).unwrap();
        assert!(h10.invalid, "越限的小时应被标记作废");
        assert_eq!(h10.mouse.move_px, 1200.0, "原始数值应保留在案");
        assert_eq!(s.days[&d].mouse.move_px, 800.0, "作废小时不计入当日");
        assert_eq!(s.total.mouse.move_px, 800.0, "作废小时不计入总计");
        // 总计分时桶：要扣掉作废的这一份，但不能被标记作废
        //（它是跨天累计，打了标记就再也不会复位，其它天同一小时会被永久隐藏）
        assert_eq!(s.total.hour(10).unwrap().mouse.move_px, 0.0, "作废小时从总计分时里扣回");
        assert!(!s.total.hour(10).unwrap().invalid, "跨天累计桶不应带作废标记");
        assert_eq!(s.total.hour(9).unwrap().mouse.move_px, 800.0);
        assert_eq!(s.day(&d).unwrap().view(Some(10)).mouse.move_px, 0.0, "视图里作废小时按 0 显示");

        // 之后再动鼠标：只进该小时自己的桶（继续留证据），不再影响当日/总计
        s.bump_mouse(&d, 10, &MouseEv::MovePx(500.0));
        assert_eq!(s.hour(&d, 10).unwrap().mouse.move_px, 1700.0);
        assert_eq!(s.days[&d].mouse.move_px, 800.0);
        assert_eq!(s.total.mouse.move_px, 800.0);
        assert_eq!(s.total.hour(10).unwrap().mouse.move_px, 0.0, "作废后不再累加进总计分时");

        // 键盘越限（100 次/小时）
        for i in 0..101 {
            s.bump_key(&d, 11, i % N_KEYS);
        }
        assert!(s.hour(&d, 11).unwrap().invalid);
        assert_eq!(s.hour(&d, 11).unwrap().keystrokes, 101, "原始敲击数保留");
        assert!(s.days[&d].keystrokes + s.hour(&d, 11).unwrap().keystrokes <= 0 + 0 + 101);
        assert_eq!(s.days[&d].keystrokes, 0, "作废小时的整体不计入当日");

        // 点击越限（10 次/小时）
        for _ in 0..11 {
            s.bump_mouse(&d, 12, &MouseEv::LeftUp);
        }
        assert!(s.hour(&d, 12).unwrap().invalid);
        assert_eq!(s.hour(&d, 12).unwrap().mouse.lb, 11);
        assert_eq!(s.days[&d].mouse.lb, 0);

        // 关闭阈值（0）后一切照常计入
        s.limits = HourLimits::default();
        s.bump_mouse(&d, 13, &MouseEv::MovePx(99999.0));
        assert!(!s.hour(&d, 13).unwrap().invalid);
        assert_eq!(s.days[&d].mouse.move_px, 800.0 + 99999.0);
    }

    #[test]
    fn limits_from_config_converts_km_to_pixels() {
        let mut cfg = crate::config::Config::default();
        assert_eq!(cfg.limit_move_km, 20.0, "默认 20 公里/小时");
        assert_eq!((cfg.limit_clicks, cfg.limit_keystrokes), (0, 0), "点击/键盘阈值默认关闭");
        // 本机比例：340mm / 2560px → 每米 7529 像素
        let ppm = 1000.0 * 2560.0 / 340.0;
        let l = HourLimits::from_config(&cfg, ppm);
        assert!((l.move_px - 20.0 * 1000.0 * ppm).abs() < 1.0);
        // 屏幕尺寸未知（ppm = 0）→ 距离阈值无法换算，自动忽略
        assert_eq!(HourLimits::from_config(&cfg, 0.0).move_px, 0.0);
        cfg.limit_move_km = 0.0;
        assert_eq!(HourLimits::from_config(&cfg, ppm).move_px, 0.0);
    }

    #[test]
    fn rollover_tracks_current_hour() {
        let shared = Shared::new(Store::default(), 0, 0);
        shared.hour.store(0, Ordering::Relaxed);
        shared.rollover_if_needed();
        assert_eq!(shared.current_hour() as u32, Local::now().hour());
        assert!(!shared.rollover_if_needed(), "同一天不应重复触发跨夜");
    }

    #[test]
    fn purge_keeps_recent() {
        let mut s = Store::default();
        let old = (Local::now().date_naive() - Duration::days(100)).format("%Y%m%d").to_string();
        s.day_mut(&old).hour_mut(3).add_key(1);
        s.day_mut(&today_string());
        assert_eq!(s.purge(30), 1);
        assert_eq!(s.days.len(), 1);
        assert!(s.day(&old).is_none(), "过期天连同小时明细一起删除");
    }

    #[test]
    fn apm_tracker_sliding_window_and_peak() {
        let mut tracker = ApmTracker::new("20261013", 0);
        assert_eq!(tracker.peak(), 0);
        let (apm, peak) = tracker.get("20261013", 60);
        assert_eq!(apm, 0);
        assert_eq!(peak, 0);

        // 单次操作绝不放大：点 1 次必须是 1 APM
        tracker.record("20261013", 5);
        let (apm1, _) = tracker.get("20261013", 5);
        assert_eq!(apm1, 1, "单次点击绝不能放大为 12");

        // 数值就是"窗口内的动作数"：窗口调短也不会放大（旧版 5 秒窗口点 5 次会显示 60）
        for _ in 1..5 {
            tracker.record("20261013", 5);
        }
        let (apm, peak) = tracker.get("20261013", 5);
        assert_eq!(apm, 5, "窗口内动作数原样呈现，不乘 60/窗口");
        let (apm30, _) = tracker.get("20261013", 30);
        assert_eq!(apm30, 5, "换窗口长度只影响平滑程度，不改数值");
        assert_eq!(peak, 5);
        assert_eq!(tracker.peak(), 5);

        // 跨天重置
        let (apm_next, peak_next) = tracker.get("20261014", 5);
        assert_eq!(apm_next, 0);
        assert_eq!(peak_next, 0);
    }

    #[test]
    fn kps_tracker_sliding_window_and_hits() {
        let mut tracker = KpsTracker::new("20261013");
        let (kps, peak) = tracker.get("20261013", 1000);
        assert_eq!((kps, peak), (0.0, 0.0));

        tracker.record("20261013", "Z", 1000);
        tracker.record("20261013", "X", 1000);
        tracker.record("20261013", "M1", 1000);
        let (kps, peak) = tracker.get("20261013", 1000);
        assert_eq!((kps, peak), (3.0, 3.0));

        let query =
            tracker.query_keys(&["Z".into(), "X".into(), "C".into(), "M1".into(), "M2".into()], 1000);
        assert_eq!(query.len(), 5);
        assert_eq!(query[0].name, "Z");
        assert_eq!(query[0].hits, 1);
        assert!(query[0].glow > 0.9, "刚按过应点亮");
        assert!(!query[0].held, "没送按住事件时 held 应为 false");
        assert_eq!(query[0].kps, 1.0, "单键也带自己的 KPS");
        assert_eq!(query[1].hits, 1);
        assert!(query[1].glow > 0.9);
        assert_eq!(query[2].hits, 0);
        assert_eq!(query[2].glow, 0.0, "没按过不亮");
        assert_eq!(query[2].kps, 0.0);
        assert_eq!(query[3].hits, 1);
        assert!(query[3].glow > 0.9);
        assert_eq!(query[4].hits, 0);
        assert_eq!(query[4].glow, 0.0);

        // KPS 历史：第一次取值就落一条采样（之后每秒一条），重置时清空
        assert_eq!(tracker.history().len(), 1);
        tracker.reset_all();
        assert_eq!(tracker.history().len(), 0, "重置要把 KPS 图一起清掉");
    }

    /// 重置菜单的四个选项各管各的（对齐 osu-kps：按键总次数 / 最大 KPS / KPS 图 / 以上所有）
    #[test]
    fn kps_reset_options_are_independent() {
        let mut t = KpsTracker::new("20261013");
        t.set_monitored(&["Z".into()]);
        for _ in 0..3 {
            t.record("20261013", "Z", 1000);
        }
        assert_eq!(t.peak(), 3.0);
        assert_eq!(t.query_keys(&["Z".into()], 1000)[0].hits, 3);
        assert!(!t.history().is_empty(), "取值后应有历史采样");

        // 只清最大 KPS
        t.reset_peak();
        assert_eq!(t.peak(), 0.0, "峰值被清零");
        assert_eq!(t.query_keys(&["Z".into()], 1000)[0].hits, 3, "计数不受影响");
        assert!(!t.history().is_empty(), "图不受影响");

        // 只清按键总次数
        t.reset_counts();
        assert_eq!(t.query_keys(&["Z".into()], 1000)[0].hits, 0);
        assert_eq!(t.get("20261013", 1000).0, 3.0, "当前 KPS 不受影响");

        // 只清 KPS 图
        t.reset_graph();
        assert!(t.history().is_empty(), "历史被清掉");

        // 以上所有：连平均的起算点一起归零
        t.record("20261013", "Z", 1000);
        t.reset_all();
        assert_eq!(t.peak(), 0.0);
        assert_eq!(t.query_keys(&["Z".into()], 1000)[0].hits, 0);
        assert!(t.history().is_empty());
    }

    /// 计算方式的配置映射可来回转换（from_config / as_config）
    #[test]
    fn kps_method_config_roundtrip() {
        for m in [KpsMethod::Hard, KpsMethod::Sensitive] {
            assert_eq!(KpsMethod::from_config(m.as_config()), m);
        }
        assert_eq!(KpsMethod::from_config("别的值"), KpsMethod::Hard, "未知值退回硬算");
    }

    /// 灵敏方式（对齐 osu-kps）：单次击键被限制在 1.5 以内，连击时贴近真实速率
    #[test]
    fn kps_sensitive_caps_single_press() {
        let mut t = KpsTracker::new("20261013");
        t.set_method(KpsMethod::Sensitive);
        let (kps, _) = t.record("20261013", "Z", 1000);
        assert!((1.0..=1.5).contains(&kps), "单次击键最多 1.5（不虚高），实际 {kps}");
        for _ in 0..5 {
            t.record("20261013", "Z", 1000);
        }
        let (kps, _) = t.get("20261013", 1000);
        assert!(kps > 1.5, "连击时数值应该上去：{kps}");
        // 同批记录在硬算模式下就是"窗口内次数"
        t.set_method(KpsMethod::Hard);
        assert_eq!(t.get("20261013", 1000).0, 6.0);
    }

    /// KPS 速率只统计监控键：打字时的普通按键不该把 KPS 抬起来（对齐 osu-kps / KeysPerSecond）
    #[test]
    fn kps_rate_only_counts_monitored_keys() {
        let mut tracker = KpsTracker::new("20261013");
        tracker.set_monitored(&["Z".into(), "X".into(), "M1".into()]);
        tracker.record("20261013", "Z", 1000);
        tracker.record("20261013", "A", 1000); // 不在监控列表 → 不计入速率
        tracker.record("20261013", "Space", 1000); // 同上
        tracker.record("20261013", "M1", 1000);
        let (kps, peak) = tracker.get("20261013", 1000);
        assert_eq!((kps, peak), (2.0, 2.0), "只有 Z 与 M1 计入 KPS（A / Space 不算）");

        // 单键计数照旧按"每个键各数各的"记：把某键加进监控列表后能立刻看到已有次数
        let q = tracker.query_keys(&["Z".into(), "A".into(), "M1".into()], 1000);
        assert_eq!((q[0].hits, q[1].hits, q[2].hits), (1, 1, 1));
        assert_eq!(q[0].kps, 1.0, "Z 自己的 KPS");
        assert_eq!(q[1].kps, 0.0, "没被监控的 A，速率永远是 0");

        // 空列表 = 不过滤（兜底行为，配置为空时不至于永远 0）
        let mut t2 = KpsTracker::new("20261013");
        t2.record("20261013", "Q", 1000);
        t2.record("20261013", "W", 1000);
        assert_eq!(t2.get("20261013", 1000).0, 2.0, "未配置监控键时统计全部动作");
        assert_eq!(t2.record("20261013", "Q", 1000).0, 3.0, "空监控列表不做过滤");
    }

    /// 长按：按住期间保持满亮（旧实现在 ~380ms 内就灭了），松开后 50→250ms 淡出
    #[test]
    fn kps_set_held_keeps_key_lit_until_release() {
        let mut t = KpsTracker::new("20261014");
        // 按住
        t.set_held("Q", true);
        let q = &t.query_keys(&["Q".into()], 1000)[0];
        assert!(q.held, "按下后应处于按住状态");
        assert_eq!(q.glow, 1.0, "按住期间应满亮");
        // 按住 220ms（比旧的 130ms 激活窗更久）仍是满亮 —— 这正是旧实现的短板
        std::thread::sleep(std::time::Duration::from_millis(220));
        let q = &t.query_keys(&["Q".into()], 1000)[0];
        assert!(q.held && q.glow == 1.0, "长按中途不该灭：{q:?}");
        // 松开：先亮着走淡出，250ms 后完全熄灭
        t.set_held("Q", false);
        let q = &t.query_keys(&["Q".into()], 1000)[0];
        assert!(!q.held);
        assert!(q.glow > 0.9, "刚松开仍在亮：{q:?}");
        // osu-kps 的淡出是**渐近**的（250ms 处约到 80%，之后趋近 0，不精确归零）：
        // 320ms 时应已基本熄灭（<0.15）
        std::thread::sleep(std::time::Duration::from_millis(320));
        let q = &t.query_keys(&["Q".into()], 1000)[0];
        assert!(q.glow < 0.15, "松开后应基本熄灭：{q:?}");
        // 修饰键别名：LShift 的按住会合并到 shift 口径
        t.set_held("LShift", true);
        let q = &t.query_keys(&["Shift".into()], 1000)[0];
        assert!(q.held, "LShift 按住应在 Shift 口径上可见");
    }
}
