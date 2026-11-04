//! 数据导出：**一份实现，两个入口**（设置面板的「导出数据…」按钮、命令行 `--export`）。
//!
//! 产出三份，各有各的用处：
//! - `<名字>.csv` —— 每天一行，给人看（Excel）；
//! - `<名字>_hourly.csv` —— 分时明细，有分时数据才写（作废的小时按 0 导，与界面一致）；
//! - `<名字>.json` —— 完整原始数据（含分时与逐键计数、配列 id），给程序读/备份。
//!
//! 为什么 CSV 和 JSON 都要：CSV 便于人看和二次统计，JSON 保留原始精度与结构，
//! 真出问题时还能拿它手动恢复。

use std::path::{Path, PathBuf};

use crate::config::Config;
use crate::stats::{BucketStats, DayEntry, Store};

const DAILY_HEADER: &str = "date,keystrokes,peak_apm,mouse_move_px,mouse_move_meters,mouse_lb,mouse_rb,mouse_mb,mouse_xb,mouse_wheel,mouse_hwheel";
const HOURLY_HEADER: &str = "date,hour,keystrokes,mouse_move_px,mouse_move_meters,mouse_lb,mouse_rb,mouse_mb,mouse_xb,mouse_wheel,mouse_hwheel";

pub struct Exported {
    /// 实际写出的文件（每日 CSV / 分时 CSV / JSON）
    pub files: Vec<PathBuf>,
    /// 导出的天数
    pub days: usize,
    /// 分时行数（没有分时数据时为 0，此时不写分时文件）
    pub hours: usize,
}

/// 导出全部历史到 `out_csv`（分时与 JSON 由它派生文件名）。
///
/// `px_to_m` 由调用方给：屏幕物理尺寸只有 `Shared` / `resolve_screen_mm` 知道，
/// 这里不重复推算（CLI 与 GUI 走的换算必须完全一致）。
pub fn export_to(
    store: &Store,
    cfg: &Config,
    out_csv: &Path,
    px_to_m: impl Fn(f64) -> f64,
) -> std::io::Result<Exported> {
    let mut files = Vec::new();

    // ---- 每日 ----
    let mut lines: Vec<String> = vec![DAILY_HEADER.to_string()];
    for (date, d) in &store.days {
        lines.push(daily_row(date, d, &px_to_m));
    }
    crate::fsutil::atomic_write(out_csv, (lines.join("\n") + "\n").as_bytes())?;
    files.push(out_csv.to_path_buf());

    // ---- 分时明细（有数据才写）----
    let stem = out_csv
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "kmcounter_export".into());
    let hourly_path = out_csv.with_file_name(format!("{stem}_hourly.csv"));
    let mut hlines: Vec<String> = vec![HOURLY_HEADER.to_string()];
    for (date, d) in &store.days {
        let mut hrs: Vec<(&u8, &BucketStats)> = d.hours.iter().collect();
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
    let hours = hlines.len() - 1;
    if hours > 0 {
        crate::fsutil::atomic_write(&hourly_path, (hlines.join("\n") + "\n").as_bytes())?;
        files.push(hourly_path);
    }

    // ---- JSON：完整原始数据 ----
    #[derive(serde::Serialize)]
    struct Meta<'a> {
        app: &'a str,
        version: &'a str,
        exported_at: String,
        /// 逐键计数是按键盘配列的下标存的，导出时带上配列 id 才能解释
        keyboard_layout: &'a str,
    }
    #[derive(serde::Serialize)]
    struct Dump<'a> {
        meta: Meta<'a>,
        total: &'a DayEntry,
        days: &'a std::collections::BTreeMap<String, DayEntry>,
    }
    let dump = Dump {
        meta: Meta {
            app: "KMCounter-rs",
            version: env!("CARGO_PKG_VERSION"),
            exported_at: chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string(),
            keyboard_layout: &cfg.layout.keyboard_layout,
        },
        total: &store.total,
        days: &store.days,
    };
    let json = serde_json::to_string_pretty(&dump)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
    let json_path = out_csv.with_extension("json");
    crate::fsutil::atomic_write(&json_path, json.as_bytes())?;
    files.push(json_path);

    Ok(Exported {
        files,
        days: store.days.len(),
        hours,
    })
}

fn daily_row(date: &str, d: &DayEntry, px_to_m: &impl Fn(f64) -> f64) -> String {
    format!(
        "{},{},{},{},{:.4},{},{},{},{},{},{}",
        date,
        d.keystrokes,
        d.peak_apm,
        d.mouse.move_px,
        px_to_m(d.mouse.move_px),
        d.mouse.lb,
        d.mouse.rb,
        d.mouse.mb,
        d.mouse.xb,
        d.mouse.wheel,
        d.mouse.hwheel
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store_with_one_day() -> Store {
        let mut s = Store::default();
        let mut d = DayEntry::default();
        d.bucket.keystrokes = 100;
        d.bucket.mouse.lb = 10;
        d.bucket.mouse.rb = 5;
        d.bucket.mouse.move_px = 9600.0; // 9600px → 2.54m
        d.peak_apm = 461;
        let mut h = BucketStats::default();
        h.keystrokes = 7;
        d.hours.insert(9, h);
        s.days.insert("20261013".into(), d);
        s.total.bucket.keystrokes = 100;
        s
    }

    #[test]
    fn export_writes_daily_hourly_and_json() {
        let dir = std::env::temp_dir().join(format!("kmc_export_test_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let out = dir.join("sample.csv");
        let r = export_to(&store_with_one_day(), &Config::default(), &out, |px| px / 9600.0 * 2.54).unwrap();

        assert_eq!(r.days, 1);
        assert_eq!(r.hours, 1, "9 点那条分时应该被导出");
        assert_eq!(r.files.len(), 3, "每日 CSV / 分时 CSV / JSON");

        let daily = std::fs::read_to_string(&out).unwrap();
        assert!(daily.starts_with("date,keystrokes,peak_apm,"), "表头: {daily}");
        assert!(daily.contains("20261013,100,461,9600,2.5400,10,5,0,0,0,0"), "数据行: {daily}");

        let hourly = std::fs::read_to_string(dir.join("sample_hourly.csv")).unwrap();
        assert!(hourly.contains("20261013,9,7,"), "分时行: {hourly}");

        let json: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(dir.join("sample.json")).unwrap()).unwrap();
        assert_eq!(json["meta"]["app"], "KMCounter-rs");
        assert_eq!(json["days"]["20261013"]["keystrokes"], 100);

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 没有分时数据时不写分时文件（避免空文件让人以为导出坏了）
    #[test]
    fn export_skips_hourly_file_when_empty() {
        let dir = std::env::temp_dir().join(format!("kmc_export_empty_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut s = Store::default();
        s.days.insert("20261013".into(), DayEntry::default());
        let out = dir.join("empty.csv");
        let r = export_to(&s, &Config::default(), &out, |px| px).unwrap();
        assert_eq!(r.hours, 0);
        assert_eq!(r.files.len(), 2, "只有每日 CSV + JSON");
        assert!(!dir.join("empty_hourly.csv").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
