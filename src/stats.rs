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
use std::collections::BTreeMap;
use std::io::Write;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;

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

/// 一天的数据：当天汇总（扁平字段，与旧版文件兼容）+ 各小时明细
#[derive(Clone, Default, Serialize, Deserialize, Debug)]
pub struct DayEntry {
    #[serde(flatten)]
    pub bucket: BucketStats,
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

impl DayEntry {
    /// 取某小时的桶（不存在则创建）
    pub fn hour_mut(&mut self, h: u8) -> &mut BucketStats {
        self.hours.entry(h.min(HOURS_PER_DAY as u8 - 1)).or_default()
    }

    pub fn hour(&self, h: u8) -> Option<&BucketStats> {
        self.hours.get(&h)
    }

    /// 视图取值：None = 整天/总计，Some(h) = 该小时（无数据时返回空桶）
    pub fn view(&self, hour: Option<u8>) -> BucketStats {
        match hour {
            None => self.bucket.clone(),
            Some(h) => self.hours.get(&h).cloned().unwrap_or_default(),
        }
    }

    /// 某小时是否被判定作废（无该小时 = false）
    pub fn hour_invalid(&self, h: u8) -> bool {
        self.hours.get(&h).map(|u| u.invalid).unwrap_or(false)
    }

    /// 24 小时分时曲线（键盘敲击次数、鼠标移动像素），无数据的小时为 0
    pub fn hour_profile(&self) -> Vec<(u64, f64)> {
        let mut out = vec![(0u64, 0.0f64); HOURS_PER_DAY];
        for (h, u) in &self.hours {
            if let Some(slot) = out.get_mut(*h as usize) {
                *slot = (u.keystrokes, u.mouse.move_px);
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
        if let Some(n) = b.other.remove(id) {
            if let Some(i) = crate::keys::idx_by_name(name) {
                b.keys[i] = b.keys[i].saturating_add(n);
            }
        }
    }
}

impl Store {
    pub fn load(path: &Path) -> Store {
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
        let tmp = path.with_extension("json.tmp");
        {
            let f = std::fs::File::create(&tmp)?;
            let mut w = std::io::BufWriter::new(f);
            serde_json::to_writer_pretty(&mut w, self)?;
            w.flush()?;
            w.get_ref().sync_all().ok();
        }
        std::fs::rename(&tmp, path)
    }

    /// 获取（或创建）某日数据
    #[allow(dead_code)] // 主流程走 bump_*，此接口供测试/工具使用
    pub fn day_mut(&mut self, date: &str) -> &mut DayEntry {
        self.days.entry(date.to_string()).or_insert_with(DayEntry::default)
    }

    pub fn day(&self, date: &str) -> Option<&DayEntry> {
        self.days.get(date)
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
        if let Some(h) = self.total.hours.get_mut(&hour) {
            h.invalid = true;
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
        Arc::new(Shared {
            today: Mutex::new(today_string()),
            hour: AtomicU32::new(Local::now().hour()),
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
            *t = date;
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
        let day = s.day(&d).unwrap();
        let prof = day.hour_profile();
        assert_eq!(prof.len(), 24);
        assert_eq!(prof[8], (2, 0.0));
        assert_eq!(prof[20].0, 1);
        assert_eq!(prof[21].0, 0);
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

        // 之后再动鼠标：只进该小时自己的桶（继续留证据），不再影响当日/总计
        s.bump_mouse(&d, 10, &MouseEv::MovePx(500.0));
        assert_eq!(s.hour(&d, 10).unwrap().mouse.move_px, 1700.0);
        assert_eq!(s.days[&d].mouse.move_px, 800.0);
        assert_eq!(s.total.mouse.move_px, 800.0);

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
}
