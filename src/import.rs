//! 导入原版 KMCounter（AutoHotkey 版）的 `KMCounter.ini` 数据。
//!
//! 原版格式（见原版脚本 `IniWrite` 部分）：
//! ```ini
//! [history]     storage=9999                 ; 历史保留天数
//! [devicecaps]  w=345  h=215                 ; 显示器物理尺寸（毫米）
//! [layout]      kw=50 kh=45 ks=2 khs=10 kvs=10
//! [20240929]    lbcount=464 rbcount=14 … move=94.947834 keystrokes=5401 sc14=77 sc1=33 …
//! [total]       同上，为全历史累计
//! ```
//! 要点：
//! - 按键名是 `sc<扫描码>`，扫描码 = `(Extended<<8) | scanCode`，与本项目 `keys.rs` 的 `win_sc` 完全一致；
//! - `move` 是**米**（原版把每段位移按 `mm/屏幕像素宽` 换算成米后累加），本项目按像素存储，
//!   因此导入时用同一个换算因子反算回像素，保证界面上显示的距离与原版一致；
//! - 文件为 UTF-16LE（带 BOM），也兼容 UTF-8/BE。

use crate::keys::{idx_by_win_sc, N_KEYS};
use crate::stats::Store;
use std::collections::BTreeMap;
use std::path::Path;

#[derive(Debug, Default)]
pub struct Ini {
    /// 日期 "YYYYMMDD" → 键值对（原样保留字符串，解析在 import 里做）
    pub days: BTreeMap<String, BTreeMap<String, String>>,
    /// [total] 节
    pub total: BTreeMap<String, String>,
    /// [history] storage
    pub history_days: Option<u32>,
    /// [devicecaps] (w, h) 毫米
    pub screen_mm: Option<(u32, u32)>,
    /// [layout] (kw, kh, ks, khs, kvs)
    pub layout: Option<[u32; 5]>,
}

/// 读取 INI（自动识别 UTF-16LE / UTF-16BE / UTF-8）
pub fn read_ini(path: &Path) -> Result<Ini, String> {
    let raw = std::fs::read(path).map_err(|e| format!("读取 {} 失败: {e}", path.display()))?;
    let text = decode(&raw);
    parse(&text)
}

fn decode(raw: &[u8]) -> String {
    match raw {
        [0xFF, 0xFE, rest @ ..] => utf16(rest, true),
        [0xFE, 0xFF, rest @ ..] => utf16(rest, false),
        _ => String::from_utf8_lossy(raw).to_string(),
    }
}

fn utf16(bytes: &[u8], little: bool) -> String {
    let mut units = Vec::with_capacity(bytes.len() / 2);
    for c in bytes.chunks_exact(2) {
        units.push(if little { u16::from_le_bytes([c[0], c[1]]) } else { u16::from_be_bytes([c[0], c[1]]) });
    }
    String::from_utf16_lossy(&units)
}

fn parse(text: &str) -> Result<Ini, String> {
    let mut ini = Ini::default();
    let mut cur = String::new();
    for line in text.lines() {
        let line = line.trim().trim_start_matches('\u{feff}');
        if line.is_empty() {
            continue;
        }
        if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            cur = name.trim().to_string();
            continue;
        }
        let Some((k, v)) = line.split_once('=') else { continue };
        let (k, v) = (k.trim().to_string(), v.trim().to_string());
        match cur.as_str() {
            "" => {}
            "history" => {
                if k == "storage" {
                    ini.history_days = v.parse().ok();
                }
            }
            "devicecaps" => {
                let mut e = ini.screen_mm.unwrap_or((0, 0));
                match k.as_str() {
                    "w" => e.0 = v.parse().unwrap_or(0),
                    "h" => e.1 = v.parse().unwrap_or(0),
                    _ => {}
                }
                ini.screen_mm = Some(e);
            }
            "layout" => {
                let mut e = ini.layout.unwrap_or([0; 5]);
                let slot = match k.as_str() {
                    "kw" => Some(0),
                    "kh" => Some(1),
                    "ks" => Some(2),
                    "khs" => Some(3),
                    "kvs" => Some(4),
                    _ => None,
                };
                if let Some(i) = slot {
                    e[i] = v.parse().unwrap_or(0);
                }
                ini.layout = Some(e);
            }
            "total" => {
                ini.total.insert(k, v);
            }
            day => {
                ini.days.entry(day.to_string()).or_default().insert(k, v);
            }
        }
    }
    if ini.days.is_empty() && ini.total.is_empty() {
        return Err("文件里没有找到按天数据（既没有 [YYYYMMDD] 节也没有 [total]）".into());
    }
    Ok(ini)
}

/// 导入结果（用于终端汇报）
#[derive(Debug, Default)]
pub struct Report {
    pub days_imported: usize,
    pub days_skipped: Vec<String>,
    pub first_day: String,
    pub last_day: String,
    pub keystrokes: u64,
    pub keys_in_layout: u64,
    pub keys_outside: u64,
    /// 原版记录的鼠标距离合计（米）
    pub move_meters: f64,
    /// 换算后写入的像素数
    pub move_px: f64,
    /// 布局外按键（扫描码 → 次数）
    pub other_keys: Vec<(u16, u64)>,
    /// 原版 keystrokes 与逐键合计不一致而被修正的天数（原版偶发多计）
    pub days_adjusted: usize,
    /// 偏差最大的一天：(日期, 原版声明值, 逐键合计)；按差值（而非声明值）取最大
    pub max_deviation: Option<(String, u64, u64)>,
    /// 原版 [total] 里声明的敲击数（本工具不直接采用，见报告）
    pub ini_total_keystrokes: u64,
    pub ini_history_days: Option<u32>,
    pub ini_screen_mm: Option<(u32, u32)>,
    pub ini_layout: Option<[u32; 5]>,
}

/// 把 ini 里的数据合并进 `store`。`px_per_meter` = 当前屏幕的每米像素数
/// （= 1000 * 屏宽像素 / 屏宽毫米），用于把原版的“米”还原成像素。
pub fn import(ini: &Ini, store: &mut Store, px_per_meter: f64) -> Report {
    let mut rep = Report {
        ini_history_days: ini.history_days,
        ini_screen_mm: ini.screen_mm,
        ini_layout: ini.layout,
        ..Default::default()
    };
    // 换算因子：优先用“当前屏幕”的比例（这样界面显示的距离与原版一致）；
    // 拿不到屏幕信息时退回原版自己记录的 devicecaps（并假定 1920 像素宽）
    let px_per_meter = if px_per_meter > 0.0 {
        px_per_meter
    } else {
        let mm = ini.screen_mm.map(|(w, _)| w).filter(|w| *w > 0).unwrap_or(345) as f64;
        1000.0 * 1920.0 / mm
    };

    let mut dates: Vec<&String> = ini.days.keys().collect();
    dates.sort();
    rep.first_day = dates.first().map(|s| (*s).clone()).unwrap_or_default();
    rep.last_day = dates.last().map(|s| (*s).clone()).unwrap_or_default();

    let mut other: BTreeMap<u16, u64> = BTreeMap::new();
    for date in dates {
        if store.days.contains_key(date) {
            rep.days_skipped.push(date.clone());
            continue;
        }
        let sec = &ini.days[date];
        let declared: u64 = sec.get("keystrokes").and_then(|v| v.parse().ok()).unwrap_or(0);
        let day = store.day_mut(date);
        let mut counted_keys = 0u64;
        let mut day_other: BTreeMap<u16, u64> = BTreeMap::new();
        for (k, v) in sec {
            if let Some(sc) = k.strip_prefix("sc").and_then(|s| s.parse::<u16>().ok()) {
                let n: u64 = v.parse().unwrap_or(0);
                if n == 0 {
                    continue;
                }
                counted_keys += n;
                match idx_by_win_sc(sc) {
                    Some(idx) if idx < N_KEYS => {
                        day.bucket.keys[idx] += n;
                        rep.keys_in_layout += n;
                    }
                    _ => {
                        *day_other.entry(sc).or_insert(0) += n;
                        *other.entry(sc).or_insert(0) += n;
                        rep.keys_outside += n;
                    }
                }
                continue;
            }
            match k.as_str() {
                "lbcount" => day.bucket.mouse.lb += v.parse().unwrap_or(0),
                "rbcount" => day.bucket.mouse.rb += v.parse().unwrap_or(0),
                "mbcount" => day.bucket.mouse.mb += v.parse().unwrap_or(0),
                "xbcount" => day.bucket.mouse.xb += v.parse().unwrap_or(0),
                "wheel" => day.bucket.mouse.wheel += v.parse().unwrap_or(0),
                "hwheel" => day.bucket.mouse.hwheel += v.parse().unwrap_or(0),
                "move" => {
                    let m: f64 = v.parse().unwrap_or(0.0);
                    day.bucket.mouse.move_px += m * px_per_meter;
                    rep.move_meters += m;
                    rep.move_px += m * px_per_meter;
                }
                // keystrokes 放到整节处理完再定（以逐键合计为准）
                "keystrokes" => {}
                _ => {}
            }
        }
        // 原版每个按键抬起都会同时累加 keystrokes 与 sc*，正常情况下两者相等；
        // 但这份数据里有几十天 keystrokes 明显偏大（如 201678 vs 逐键 3948），
        // 因此以逐键合计为准，保证热力图/统计内部自洽。
        day.bucket.keystrokes = if counted_keys > 0 { counted_keys } else { declared };
        if day.bucket.keystrokes != declared {
            rep.days_adjusted += 1;
            let gap = declared.saturating_sub(day.bucket.keystrokes);
            let worse = rep.max_deviation.as_ref().map_or(true, |(_, d, a)| d.saturating_sub(*a) < gap);
            if worse {
                rep.max_deviation = Some((date.clone(), declared, day.bucket.keystrokes));
            }
        }
        rep.keystrokes += day.bucket.keystrokes;

        // 布局外按键沿用与实时统计相同的命名（"win:<扫描码>"）
        for (sc, n) in &day_other {
            let id = other_key_name(*sc);
            *day.bucket.other.entry(id).or_insert(0) += n;
        }

        // 一并累加进“全历史总计”：与原版不同，这里保证总计 == 各天之和（原版 [total] 曾重置/失真）
        let (bm, dm) = (day.bucket.clone(), day.bucket.mouse.clone());
        let t = &mut store.total.bucket;
        t.keystrokes += bm.keystrokes;
        t.mouse.move_px += dm.move_px;
        t.mouse.lb += dm.lb;
        t.mouse.rb += dm.rb;
        t.mouse.mb += dm.mb;
        t.mouse.xb += dm.xb;
        t.mouse.wheel += dm.wheel;
        t.mouse.hwheel += dm.hwheel;
        for (i, n) in bm.keys.iter().enumerate() {
            if *n > 0 {
                t.keys[i] += n;
            }
        }
        for (id, n) in &bm.other {
            *store.total.bucket.other.entry(id.clone()).or_insert(0) += n;
        }
        rep.days_imported += 1;
    }
    rep.other_keys = other.iter().map(|(k, v)| (*k, *v)).collect();

    rep.ini_total_keystrokes = ini.total.get("keystrokes").and_then(|v| v.parse().ok()).unwrap_or(0);

    store.version = store.version.max(1);
    rep
}

/// 布局外按键在统计里的 ID（与实时统计保持一致：`win:<扫描码>`）
fn other_key_name(sc: u16) -> String {
    crate::stats::other_key_id_win(sc)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "\u{feff}[history]\r\nstorage=9999\r\n[devicecaps]\r\nw=345\r\nh=215\r\n[layout]\r\nkw=50\r\nkh=45\r\nks=2\r\nkhs=10\r\nkvs=10\r\n[20240929]\r\nlbcount=464\r\nrbcount=14\r\nmbcount=0\r\nxbcount=0\r\nwheel=1271\r\nhwheel=0\r\nmove=94.947834\r\nkeystrokes=5\r\nsc14=2\r\nsc328=2\r\nsc554=1\r\n[total]\r\nlbcount=10\r\nmove=100.0\r\nkeystrokes=7\r\nsc14=3\r\nsc554=4\r\n";

    fn write_tmp(tag: &str, text: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("kmc_import_{tag}_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("KMCounter.ini");
        // 按原版习惯写成 UTF-16LE + BOM
        let mut bytes = vec![0xFF, 0xFE];
        for u in text.encode_utf16() {
            bytes.extend_from_slice(&u.to_le_bytes());
        }
        std::fs::write(&p, bytes).unwrap();
        p
    }

    #[test]
    fn parses_ini_and_imports_days() {
        let p = write_tmp("parse", SAMPLE);
        let ini = read_ini(&p).unwrap();
        assert_eq!(ini.history_days, Some(9999));
        assert_eq!(ini.screen_mm, Some((345, 215)));
        assert_eq!(ini.layout, Some([50, 45, 2, 10, 10]));
        assert_eq!(ini.days.len(), 1);
        assert!(ini.total.contains_key("keystrokes"));

        let mut store = Store::default();
        // 每米 5565 像素（等价于 345mm / 1920px）
        let rep = import(&ini, &mut store, 1000.0 * 1920.0 / 345.0);
        assert_eq!(rep.days_imported, 1);
        assert_eq!(rep.first_day, "20240929");
        assert_eq!(store.days["20240929"].keystrokes, 5);
        // sc14 → Backspace（索引 13 附近）；sc328 → Up；sc554 → 布局外
        let up = crate::keys::KEYS.iter().position(|k| k.name == "Up").unwrap();
        assert_eq!(store.days["20240929"].keys[up], 2);
        assert_eq!(rep.keys_in_layout, 4);
        assert_eq!(rep.keys_outside, 1);
        assert_eq!(rep.other_keys, vec![(554, 1)]);
        assert_eq!(rep.days_adjusted, 0, "样本数据逐键合计与 keystrokes 一致");
        // move：米 → 像素
        let expect_px = 94.947834 * 1000.0 * 1920.0 / 345.0;
        assert!((store.days["20240929"].mouse.move_px - expect_px).abs() < 1e-6);
        assert_eq!(store.days["20240929"].mouse.lb, 464);
        assert_eq!(store.days["20240929"].mouse.wheel, 1271);
        // 总计按导入的天数汇总（不是原版 [total] 节：它可能被重置过）
        assert_eq!(store.total.keystrokes, 5);
        assert_eq!(store.total.mouse.move_px, expect_px);
        assert_eq!(store.total.mouse.lb, 464);
        assert_eq!(rep.ini_total_keystrokes, 7, "原版 [total] 的敲击数只做展示");
        std::fs::remove_dir_all(p.parent().unwrap()).ok();
    }

    #[test]
    fn adjusts_inflated_keystrokes() {
        // 原版偶发多计：keystrokes 远大于逐键合计 → 以逐键合计为准
        let text = "[20250820]\r\nkeystrokes=201678\r\nsc30=10\r\nsc31=5\r\n";
        let p = write_tmp("inflate", text);
        let ini = read_ini(&p).unwrap();
        let mut store = Store::default();
        let rep = import(&ini, &mut store, 1000.0);
        assert_eq!(store.days["20250820"].keystrokes, 15);
        assert_eq!(store.total.keystrokes, 15);
        assert_eq!(rep.days_adjusted, 1);
        assert_eq!(rep.max_deviation, Some(("20250820".into(), 201678, 15)));
        std::fs::remove_dir_all(p.parent().unwrap()).ok();
    }

    #[test]
    fn skips_existing_days_and_keeps_total_separate() {
        let p = write_tmp("skip", SAMPLE);
        let ini = read_ini(&p).unwrap();
        let mut store = Store::default();
        store.day_mut("20240929").bucket.keystrokes = 42; // 该天已存在（新版本记录过）
        let rep = import(&ini, &mut store, 0.0); // 无屏幕信息 → 回退 345mm/1920px
        assert_eq!(rep.days_imported, 0);
        assert_eq!(rep.days_skipped, vec!["20240929".to_string()]);
        assert_eq!(store.days["20240929"].keystrokes, 42, "已存在的天不应被覆盖");
        assert_eq!(store.total.keystrokes, 0, "跳过的天不并入总计");
        std::fs::remove_dir_all(p.parent().unwrap()).ok();
    }

    #[test]
    fn reads_utf8_fallback() {
        let dir = std::env::temp_dir().join(format!("kmc_import_utf8_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("KMCounter.ini");
        std::fs::write(&p, "[20250101]\nkeystrokes=3\nsc30=3\n").unwrap();
        let ini = read_ini(&p).unwrap();
        assert_eq!(ini.days.len(), 1);
        let mut store = Store::default();
        let rep = import(&ini, &mut store, 1000.0);
        assert_eq!(rep.days_imported, 1);
        assert_eq!(store.days["20250101"].keystrokes, 3);
        std::fs::remove_dir_all(&dir).ok();
    }
}
