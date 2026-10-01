//! 趋势数据：把按天/按小时统计聚合成 时/日/周/月/年 时间桶，并补齐无数据的桶（填 0）。

use crate::stats::{BucketStats, Store};
use chrono::{Datelike, Duration, Months, NaiveDate, NaiveDateTime, Timelike};

/// 小时粒度的展示窗口（最近 72 小时 = 3 天，足以看出昼夜节律）
pub const HOURLY_WINDOW: u32 = 72;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Granularity {
    Hourly,
    Daily,
    Weekly,
    Monthly,
    Yearly,
}

impl Granularity {
    /// 各粒度的展示窗口长度（末尾对齐今天）
    pub fn window(self) -> Duration {
        match self {
            Granularity::Hourly => Duration::hours(HOURLY_WINDOW as i64),
            Granularity::Daily => Duration::days(120),
            Granularity::Weekly => Duration::weeks(104),
            Granularity::Monthly => Duration::days(800), // 约 26 个月，下面按月精确对齐
            Granularity::Yearly => Duration::days(36500),
        }
    }

    /// 把日期对齐到桶的起始日（小时粒度不走这里，见 [`build_hourly_trends`]）
    pub fn bucket_start(self, date: NaiveDate) -> NaiveDate {
        match self {
            Granularity::Hourly | Granularity::Daily => date,
            Granularity::Weekly => date - Duration::days(date.weekday().num_days_from_monday() as i64),
            Granularity::Monthly => NaiveDate::from_ymd_opt(date.year(), date.month(), 1).unwrap(),
            Granularity::Yearly => NaiveDate::from_ymd_opt(date.year(), 1, 1).unwrap(),
        }
    }

    /// 桶的起始标签（横轴刻度用）
    pub fn label(self, date: NaiveDate) -> String {
        match self {
            Granularity::Hourly | Granularity::Daily => date.format("%m-%d").to_string(),
            Granularity::Weekly => date.format("%m-%d").to_string(),
            Granularity::Monthly => date.format("%Y-%m").to_string(),
            Granularity::Yearly => date.format("%Y").to_string(),
        }
    }

    /// 桶的完整标签（悬停提示用）
    pub fn full_label(self, date: NaiveDate) -> String {
        match self {
            Granularity::Hourly => format!("{} 各小时", date.format("%Y-%m-%d")),
            Granularity::Daily => date.format("%Y-%m-%d").to_string(),
            Granularity::Weekly => {
                let end = date + Duration::days(6);
                format!("{} ~ {}", date.format("%Y-%m-%d"), end.format("%m-%d"))
            }
            Granularity::Monthly => format!("{} 当月", date.format("%Y-%m")),
            Granularity::Yearly => format!("{} 当年", date.format("%Y")),
        }
    }
}

/// 一个趋势数据点（一个时间桶）
#[derive(Clone, Debug, PartialEq)]
pub struct TrendPoint {
    /// 横轴短标签
    pub label: String,
    /// 悬停完整标签
    pub full_label: String,
    /// 桶对应的日期（小时粒度＝那一刻所在的那一天），点击图表时用来跳转
    pub date: NaiveDate,
    /// 小时粒度时该点对应的小时（日期桶为 None）
    pub hour: Option<u8>,
    /// 键盘敲击次数
    pub keystrokes: f64,
    /// 鼠标移动（米）
    pub move_meters: f64,
}

/// 从最早的数据日起（或窗口起点，取较晚者）到今天逐日聚合。
/// `px_to_m`：像素 → 米 的换算函数。
pub fn build_trends(
    store: &Store,
    today: NaiveDate,
    g: Granularity,
    px_to_m: impl Fn(f64) -> f64,
) -> Vec<TrendPoint> {
    if g == Granularity::Hourly {
        // 小时粒度请用 build_hourly_trends（日期桶表达不了一小时）
        return Vec::new();
    }
    let first_data = store
        .days
        .keys()
        .next()
        .and_then(|s| NaiveDate::parse_from_str(s, "%Y%m%d").ok());
    let window_start = g.bucket_start(today - g.window());
    let start = match first_data {
        Some(d) => g.bucket_start(d).max(window_start),
        None => g.bucket_start(today),
    };

    let mut points: Vec<TrendPoint> = Vec::new();
    let mut cur_start: Option<NaiveDate> = None;
    let mut cur_keys = 0f64;
    let mut cur_px = 0f64;

    let mut d = start;
    while d <= today {
        let bs = g.bucket_start(d);
        if cur_start != Some(bs) {
            // 收尾上一个桶
            if let Some(s) = cur_start {
                points.push(TrendPoint {
                    label: g.label(s),
                    full_label: g.full_label(s),
                    date: s,
                    hour: None,
                    keystrokes: cur_keys,
                    move_meters: px_to_m(cur_px),
                });
            }
            cur_start = Some(bs);
            cur_keys = 0.0;
            cur_px = 0.0;
        }
        if let Some(day) = store.days.get(&d.format("%Y%m%d").to_string()) {
            cur_keys += day.keystrokes as f64;
            cur_px += day.mouse.move_px;
        }
        d += Duration::days(1);
    }
    if let Some(s) = cur_start {
        points.push(TrendPoint {
            label: g.label(s),
            full_label: g.full_label(s),
            date: s,
            hour: None,
            keystrokes: cur_keys,
            move_meters: px_to_m(cur_px),
        });
    }
    points
}

/// 逐小时聚合最近 `hours` 个小时（末尾对齐 `now`，含当前这一小时）。
/// 没有输入的小时补 0；数据首日之前的时段不铺零。
pub fn build_hourly_trends(
    store: &Store,
    now: NaiveDateTime,
    hours: u32,
    px_to_m: impl Fn(f64) -> f64,
) -> Vec<TrendPoint> {
    let first_data = store
        .days
        .keys()
        .next()
        .and_then(|s| NaiveDate::parse_from_str(s, "%Y%m%d").ok())
        .and_then(|d| d.and_hms_opt(0, 0, 0));
    let Some(first_dt) = first_data else {
        return Vec::new(); // 还没有任何数据
    };

    let hours = hours.max(2);
    let mut t = (now - Duration::hours(hours as i64 - 1))
        .with_minute(0)
        .and_then(|d| d.with_second(0))
        .and_then(|d| d.with_nanosecond(0))
        .unwrap_or(now);
    if first_dt > t {
        t = first_dt; // 从首个数据日的 0 点开始
    }

    let mut points = Vec::with_capacity(hours as usize);
    while t <= now {
        let key = t.format("%Y%m%d").to_string();
        let h = t.hour() as u8;
        // 被判作废的小时按 0 计：原始值仍留在数据里，但不该把曲线带偏
        let (ks, px) = store
            .hour(&key, h)
            .filter(|u| !u.invalid)
            .map(|u| (u.keystrokes as f64, u.mouse.move_px))
            .unwrap_or((0.0, 0.0));
        points.push(TrendPoint {
            label: t.format("%m-%d %H").to_string(),
            full_label: format!("{} {:02}:00–{:02}:00", t.format("%Y-%m-%d"), h, (h + 1) % 24),
            date: t.date(),
            hour: Some(h),
            keystrokes: ks,
            move_meters: px_to_m(px),
        });
        t += Duration::hours(1);
    }
    points
}

/// 本小时（当前整点）对比昨日同一小时：等长窗口（各 1 小时），避免“本小时才过一半”的误导
pub fn hour_compare(store: &Store, today: NaiveDate, hour: u8, metric: impl Fn(&BucketStats) -> f64) -> PeriodDelta {
    let cur = store
        .hour(&today.format("%Y%m%d").to_string(), hour)
        .map(&metric)
        .unwrap_or(0.0);
    let prev = store
        .hour(&(today - Duration::days(1)).format("%Y%m%d").to_string(), hour)
        .map(&metric)
        .unwrap_or(0.0);
    delta(cur, prev)
}

/// 等长窗口环比结果：current / previous 为两个等长时间窗的聚合值，pct 为百分比变化
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PeriodDelta {
    pub current: f64,
    pub previous: f64,
    /// previous == 0 时为 None（无法计算百分比）
    pub pct: Option<f64>,
}

/// 闭区间 [start, end] 的指标求和
pub fn sum_range(store: &Store, start: NaiveDate, end: NaiveDate, metric: impl Fn(&BucketStats) -> f64) -> f64 {
    let mut sum = 0.0;
    let mut d = start;
    while d <= end {
        if let Some(day) = store.days.get(&d.format("%Y%m%d").to_string()) {
            sum += metric(day);
        }
        d += Duration::days(1);
    }
    sum
}

fn delta(current: f64, previous: f64) -> PeriodDelta {
    let pct = if previous > 0.0 {
        Some((current - previous) / previous * 100.0)
    } else {
        None
    };
    PeriodDelta { current, previous, pct }
}

/// 本周（周一起至今）对比上周同期（等长窗口）
pub fn week_compare(store: &Store, today: NaiveDate, metric: impl Fn(&BucketStats) -> f64) -> PeriodDelta {
    let monday = Granularity::Weekly.bucket_start(today);
    let len = (today - monday).num_days();
    let cur = sum_range(store, monday, today, &metric);
    let prev_start = monday - Duration::days(7);
    let prev = sum_range(store, prev_start, prev_start + Duration::days(len), &metric);
    delta(cur, prev)
}

/// 本月（1 日起至今）对比上月同期（等长窗口；上月不足时截到月末）
pub fn month_compare(store: &Store, today: NaiveDate, metric: impl Fn(&BucketStats) -> f64) -> PeriodDelta {
    let first = NaiveDate::from_ymd_opt(today.year(), today.month(), 1).unwrap();
    let cur = sum_range(store, first, today, &metric);
    let prev_start = add_months(first, -1);
    let offset = (today - first).num_days();
    let prev_end = (prev_start + Duration::days(offset)).min(first - Duration::days(1));
    let prev = sum_range(store, prev_start, prev_end, &metric);
    delta(cur, prev)
}

/// Fritsch–Carlson 单调三次插值：在保证不越过数据范围的前提下生成平滑曲线。
/// 返回密集采样点（每段 `samples` 个），可直接连线。
pub fn smooth_series(values: &[f64], samples: usize) -> Vec<f64> {
    let n = values.len();
    if n < 3 {
        return values.to_vec();
    }
    let samples = samples.max(2);

    // 相邻差商（x 等距）
    let delta: Vec<f64> = (0..n - 1).map(|i| values[i + 1] - values[i]).collect();
    // 切线初值
    let mut m: Vec<f64> = vec![0.0; n];
    m[0] = delta[0];
    m[n - 1] = delta[n - 2];
    for i in 1..n - 1 {
        if delta[i - 1] * delta[i] <= 0.0 {
            m[i] = 0.0; // 极值点切线水平
        } else {
            m[i] = (delta[i - 1] + delta[i]) / 2.0;
        }
    }
    // FC 限制器，防止过冲
    for i in 0..n - 1 {
        if delta[i] == 0.0 {
            m[i] = 0.0;
            m[i + 1] = 0.0;
        } else {
            let a = m[i] / delta[i];
            let b = m[i + 1] / delta[i];
            let s = a * a + b * b;
            if s > 9.0 {
                let t = 3.0 / s.sqrt();
                m[i] = t * a * delta[i];
                m[i + 1] = t * b * delta[i];
            }
        }
    }

    let mut out = Vec::with_capacity((n - 1) * samples + 1);
    for i in 0..n - 1 {
        let h = 1.0; // 等距
        let y0 = values[i];
        let y1 = values[i + 1];
        for k in 0..samples {
            let t = k as f64 / samples as f64;
            let t2 = t * t;
            let t3 = t2 * t;
            let h00 = 2.0 * t3 - 3.0 * t2 + 1.0;
            let h10 = t3 - 2.0 * t2 + t;
            let h01 = -2.0 * t3 + 3.0 * t2;
            let h11 = t3 - t2;
            out.push(h00 * y0 + h10 * h * m[i] + h01 * y1 + h11 * h * m[i + 1]);
        }
    }
    out.push(values[n - 1]);
    out
}

/// 月份加减（用于测试与未来扩展）
#[allow(dead_code)]
pub fn add_months(date: NaiveDate, months: i32) -> NaiveDate {
    if months >= 0 {
        date.checked_add_months(Months::new(months as u32))
    } else {
        date.checked_sub_months(Months::new((-months) as u32))
    }
    .unwrap_or(date)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stats::MouseEv;

    fn store_with(days: &[(&str, u64, f64)]) -> Store {
        let mut s = Store::default();
        for (date, keys, px) in days {
            let d = s.day_mut(date);
            d.keystrokes = *keys;
            d.mouse.move_px = *px;
        }
        s
    }

    #[test]
    fn daily_buckets_fill_gaps() {
        let today = NaiveDate::from_ymd_opt(2026, 9, 26).unwrap();
        let store = store_with(&[
            ("20260924", 10, 100.0),
            ("20260926", 30, 300.0),
        ]);
        let pts = build_trends(&store, today, Granularity::Daily, |px| px / 1000.0);
        // 起点对齐今天-120 天，但数据只有 24 日开始 → 从 24 日起
        assert_eq!(pts.len(), 3);
        assert_eq!(pts[0].label, "09-24");
        assert_eq!(pts[0].keystrokes, 10.0);
        assert_eq!(pts[1].keystrokes, 0.0); // 补零
        assert_eq!(pts[2].keystrokes, 30.0);
        assert_eq!(pts[2].full_label, "2026-09-26");
    }

    #[test]
    fn weekly_buckets() {
        // 2026-09-21 是周一
        let today = NaiveDate::from_ymd_opt(2026, 9, 26).unwrap();
        let store = store_with(&[
            ("20260921", 7, 0.0),
            ("20260923", 5, 0.0),
            ("20260915", 9, 0.0), // 上周二（9-14 是周一）
        ]);
        let pts = build_trends(&store, today, Granularity::Weekly, |_px| 0.0);
        assert!(pts.len() >= 2);
        let last = pts.last().unwrap();
        assert_eq!(last.keystrokes, 12.0); // 7 + 5 同一周
        assert_eq!(last.full_label, "2026-09-21 ~ 09-27");
        let prev = &pts[pts.len() - 2];
        assert_eq!(prev.keystrokes, 9.0);
        assert_eq!(prev.label, "09-14");
    }

    #[test]
    fn monthly_and_yearly() {
        let today = NaiveDate::from_ymd_opt(2026, 9, 26).unwrap();
        let store = store_with(&[
            ("20260815", 3, 0.0),
            ("20260901", 4, 0.0),
            ("20250610", 100, 0.0),
        ]);
        // 月桶：从首个数据月（2025-06）对齐铺零到今天 = 16 个桶
        let pts = build_trends(&store, today, Granularity::Monthly, |_px| 0.0);
        assert_eq!(pts.len(), 16);
        assert_eq!(pts[0].label, "2025-06");
        assert_eq!(pts[0].keystrokes, 100.0);
        assert_eq!(pts[14].label, "2026-08");
        assert_eq!(pts[14].keystrokes, 3.0);
        assert_eq!(pts[15].label, "2026-09");
        assert_eq!(pts[15].keystrokes, 4.0);

        let yearly = build_trends(&store, today, Granularity::Yearly, |_px| 0.0);
        assert_eq!(yearly.len(), 2); // 2025、2026
        assert_eq!(yearly[0].keystrokes, 100.0);
        assert_eq!(yearly[1].keystrokes, 7.0);
    }

    #[test]
    fn smooth_stays_in_range_and_hits_values() {
        let v = vec![0.0, 100.0, 0.0, 50.0];
        let out = smooth_series(&v, 8);
        assert_eq!(out.len(), (4 - 1) * 8 + 1);
        for &x in &out {
            assert!((-5.0..=105.0).contains(&x), "过冲: {x}");
        }
        assert_eq!(out[0], 0.0);
        assert_eq!(out[8], 100.0); // 节点值精确保留
        assert_eq!(*out.last().unwrap(), 50.0);
        // 平滑输出单调段不反向震荡
        assert!(out[1] > 0.0);
    }

    #[test]
    fn smooth_small_input() {
        assert_eq!(smooth_series(&[], 8).len(), 0);
        assert_eq!(smooth_series(&[1.0], 8), vec![1.0]);
        assert_eq!(smooth_series(&[1.0, 2.0], 8), vec![1.0, 2.0]);
    }

    #[test]
    fn mouse_move_converted() {
        let today = NaiveDate::from_ymd_opt(2026, 9, 26).unwrap();
        let mut store = store_with(&[("20260926", 0, 0.0)]);
        store.day_mut("20260926").add_mouse(&MouseEv::MovePx(1500.0));
        let pts = build_trends(&store, today, Granularity::Daily, |px| px / 1000.0);
        assert_eq!(pts.last().unwrap().move_meters, 1.5);
    }

    #[test]
    fn week_compare_same_length() {
        // 2026-09-26 是周六；周一为 09-21
        let today = NaiveDate::from_ymd_opt(2026, 9, 26).unwrap();
        let store = store_with(&[
            ("20260921", 10, 0.0), // 本周一
            ("20260926", 20, 0.0), // 今天
            ("20260914", 5, 0.0),  // 上周一
            ("20260916", 7, 0.0),  // 上周三 —— 在等长窗口内（周一~周六）
            ("20260919", 100, 0.0), // 上周六之后? 09-19 是上周六，也在窗口内
            ("20260920", 500, 0.0), // 上周日 —— 不在等长窗口内（本期只到周六）
        ]);
        let d = week_compare(&store, today, |day| day.keystrokes as f64);
        // 本周: 10+20=30；上周同期(周一~周六): 5+7+100=112
        assert_eq!(d.current, 30.0);
        assert_eq!(d.previous, 112.0);
        let pct = d.pct.unwrap();
        assert!((pct - (30.0 - 112.0) / 112.0 * 100.0).abs() < 1e-9);
    }

    #[test]
    fn week_compare_on_monday() {
        let monday = NaiveDate::from_ymd_opt(2026, 9, 21).unwrap();
        let store = store_with(&[("20260921", 8, 0.0), ("20260914", 4, 0.0)]);
        let d = week_compare(&store, monday, |day| day.keystrokes as f64);
        assert_eq!(d.current, 8.0);
        assert_eq!(d.previous, 4.0);
    }

    #[test]
    fn month_compare_clamps_to_prev_month_end() {
        // 3 月 31 日 vs 2 月：等长窗口 02-01 起算 31 天会越界，需截断到 02-28
        let today = NaiveDate::from_ymd_opt(2026, 3, 31).unwrap();
        let store = store_with(&[
            ("20260310", 1, 0.0),
            ("20260331", 1, 0.0), // 本月 31 天 → current = 2
            ("20260201", 3, 0.0),
            ("20260228", 3, 0.0), // 2 月只有 28 天 → previous = 6
        ]);
        let d = month_compare(&store, today, |day| day.keystrokes as f64);
        assert_eq!(d.current, 2.0);
        assert_eq!(d.previous, 6.0);
    }

    #[test]
    fn hourly_buckets_zero_fill_and_labels() {
        // 存 2026-09-26 的 9 时、11 时（10 时无数据应补零）
        let mut store = Store::default();
        store.bump_key("20260926", 9, 1);
        store.bump_key("20260926", 9, 2);
        store.bump_mouse("20260926", 11, &MouseEv::MovePx(5000.0));
        let now = NaiveDate::from_ymd_opt(2026, 9, 26).unwrap().and_hms_opt(11, 30, 0).unwrap();
        // 窗口 24 小时本该回溯到昨天 11:00，但首个数据日是今天 → 从今天 00:00 起
        let pts = build_hourly_trends(&store, now, 24, |px| px / 1000.0);
        assert_eq!(pts.len(), 12);
        assert_eq!(pts[0].label, "09-26 00");
        let last = pts.last().unwrap();
        assert_eq!(last.label, "09-26 11");
        assert_eq!(last.full_label, "2026-09-26 11:00–12:00");
        assert_eq!(last.move_meters, 5.0);
        // 9 时那两个按键计数
        let at9 = pts.iter().find(|p| p.label == "09-26 09").unwrap();
        assert_eq!(at9.keystrokes, 2.0);
        // 10 时无数据 → 0
        let at10 = pts.iter().find(|p| p.label == "09-26 10").unwrap();
        assert_eq!(at10.keystrokes, 0.0);
        assert_eq!(at10.move_meters, 0.0);
    }

    #[test]
    fn hourly_window_starts_at_first_data_day() {
        // 数据只有今天，窗口 72 小时也不该往前铺两天零
        let mut store = Store::default();
        store.bump_key("20260926", 8, 1);
        let now = NaiveDate::from_ymd_opt(2026, 9, 26).unwrap().and_hms_opt(10, 5, 0).unwrap();
        let pts = build_hourly_trends(&store, now, HOURLY_WINDOW, |px| px);
        assert_eq!(pts.len(), 11); // 00:00 ~ 10:00
        assert_eq!(pts[0].label, "09-26 00");
        assert_eq!(pts[8].keystrokes, 1.0);
        // 没有任何数据时返回空
        assert!(build_hourly_trends(&Store::default(), now, HOURLY_WINDOW, |px| px).is_empty());
        // build_trends 不接受小时粒度
        assert!(build_trends(&store, now.date(), Granularity::Hourly, |px| px).is_empty());
    }

    #[test]
    fn hour_compare_uses_yesterday_same_hour() {
        let today = NaiveDate::from_ymd_opt(2026, 9, 26).unwrap();
        let mut store = Store::default();
        store.bump_key("20260926", 14, 1); // 本小时 1 次
        store.bump_key("20260925", 14, 1);
        store.bump_key("20260925", 14, 2);
        store.bump_key("20260925", 14, 3); // 昨天同小时 3 次
        store.bump_key("20260925", 15, 1); // 昨天别的小时不参与
        let d = hour_compare(&store, today, 14, |u| u.keystrokes as f64);
        assert_eq!(d.current, 1.0);
        assert_eq!(d.previous, 3.0);
        assert!(d.pct.unwrap() < 0.0);
        // 昨日无同小时数据 → previous = 0，pct = None
        let d2 = hour_compare(&store, today, 9, |u| u.keystrokes as f64);
        assert_eq!(d2.previous, 0.0);
        assert_eq!(d2.pct, None);
    }

    #[test]
    fn compare_zero_previous_is_none() {
        let today = NaiveDate::from_ymd_opt(2026, 9, 26).unwrap();
        let store = store_with(&[("20260926", 10, 0.0)]);
        let d = week_compare(&store, today, |day| day.keystrokes as f64);
        assert_eq!(d.current, 10.0);
        assert_eq!(d.previous, 0.0);
        assert_eq!(d.pct, None);
    }
}
