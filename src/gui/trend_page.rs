//! 趋势页：粒度切换、双轴平滑曲线、环比卡片 —— 从 gui.rs 拆出。
//!
//! 主方法只留编排：环比卡片 → 粒度行 → 聚合 → 图表 → 悬停提示。
//!
//! use super::* 直接拿 gui 的类型、常量、自由函数与 App 的私有字段。

use super::*;

impl App {
    /// 趋势页顶部：本小时/本周/本月 相对 昨日同小时/上周同期/上月同期 的环比卡片（每行一个指标）
    pub(crate) fn comparison_strip(&mut self, ui: &mut egui::Ui, s: &'static Strings, anchor: NaiveDate, hour: u8, is_today: bool) {
        for (i, cards) in self.comparison_rows(s, anchor, hour, is_today).into_iter().enumerate() {
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

    /// 两组环比卡片：（键盘一行, 鼠标一行）× （小时 / 周 / 月）。
    fn comparison_rows(
        &self,
        s: &'static Strings,
        anchor: NaiveDate,
        hour: u8,
        is_today: bool,
    ) -> [[(String, String, crate::trend::PeriodDelta, &'static str); 3]; 2] {
        // 看今天就说「本」，看历史某天就说「该」
        let labels = if is_today {
            (s.stat_this_hour, s.stat_this_week, s.stat_this_month)
        } else {
            (s.stat_that_hour, s.stat_that_week, s.stat_that_month)
        };
        let fmt_keys = |v: f64| {
            format!("{} {}", fmt_thousands(v), s.unit_times.trim())
                .trim_end()
                .to_string()
        };
        let px_to_m = |px: f64| self.shared.px_to_meters(px);
        let fmt_meters = |px: f64| format!("{:.2} {}", px_to_m(px), s.unit_m);

        [
            self.comparison_row(
                anchor,
                hour,
                s.keystrokes,
                s,
                labels,
                |d: &BucketStats| d.keystrokes as f64,
                fmt_keys,
            ),
            self.comparison_row(
                anchor,
                hour,
                s.mouse_move,
                s,
                labels,
                |d: &BucketStats| d.mouse.move_px,
                fmt_meters,
            ),
        ]
    }

    /// 一行三张环比卡片（小时 / 周 / 月），度量与格式化由调用方给定。
    #[allow(clippy::too_many_arguments)]
    fn comparison_row(
        &self,
        anchor: NaiveDate,
        hour: u8,
        prefix: &str,
        s: &'static Strings,
        labels: (&'static str, &'static str, &'static str),
        metric: fn(&BucketStats) -> f64,
        fmt: impl Fn(f64) -> String,
    ) -> [(String, String, crate::trend::PeriodDelta, &'static str); 3] {
        use crate::trend::{hour_compare, month_compare, week_compare};
        let (h_lab, w_lab, m_lab) = labels;
        let hr = hour_compare(&self.snap, anchor, hour, metric);
        let wk = week_compare(&self.snap, anchor, metric);
        let mo = month_compare(&self.snap, anchor, metric);
        [
            (format!("{} · {}", prefix, h_lab), fmt(hr.current), hr, s.stat_vs_hour),
            (format!("{} · {}", prefix, w_lab), fmt(wk.current), wk, s.stat_vs_week),
            (format!("{} · {}", prefix, m_lab), fmt(mo.current), mo, s.stat_vs_month),
        ]
    }


    pub(crate) fn show_trend_page(&mut self, ui: &mut egui::Ui, s: &'static Strings, content_top: f32) {
        let (anchor, is_today, hour) = self.trend_anchor();

        // 日期/分时导航由外层统一绘制（与热力图页同一套，选择状态共用）
        // 环比卡片（本/该小时、本周/该周、本月/该月）
        self.comparison_strip(ui, s, anchor, hour, is_today);
        ui.add_space(2.0);

        self.trend_granularity_row(ui, s);
        ui.add_space(2.0);

        let px_to_m = |px: f64| self.shared.px_to_meters(px);
        let pts = self.trend_points(anchor, px_to_m);
        let data = self.trend_chart_data(s, &pts);

        let (rect, resp, hover) = self.trend_canvas(ui, content_top, &data);
        let painter = ui.painter().clone();
        self.paint_trend_no_data(&painter, rect, s, &pts);
        self.trend_click_jump(&resp, hover, &pts);

        // 悬停提示框
        if let Some(h) = hover {
            let key_name = s.keystrokes.to_string();
            let click_name = s.mouse_clicks.to_string();
            let mouse_name = format!("{} ({})", s.trend_mouse_distance, s.unit_m);
            self.trend_tooltip(ui, &painter, h, &pts, s, &key_name, &click_name, &mouse_name);
        }
    }

    /// 趋势的锚点：以「当前视图那天」为准（总计视图 = 今天；选中历史某天 = 那天），
    /// 钟点取选中的小时，没选就是当前钟点。返回 (锚点日期, 是否就是今天, 钟点)。
    fn trend_anchor(&self) -> (NaiveDate, bool, u8) {
        let now = chrono::Local::now();
        let today = NaiveDate::parse_from_str(&self.snap_today, "%Y%m%d").unwrap_or_else(|_| now.date_naive());
        let anchor = self.anchor_date();
        let is_today = anchor == today;
        let hour = self.anchor_hour();
        (anchor, is_today, hour)
    }

    /// 图表画布：高度由 base_h 推出（与窗口当前实际高度无关），分配区域并交给 chart::draw。
    fn trend_canvas(
        &mut self,
        ui: &mut egui::Ui,
        content_top: f32,
        data: &chart::ChartData,
    ) -> (Rect, egui::Response, Option<chart::ChartHover>) {
        // 图表高度由 base_h 推出，与窗口当前实际高度无关：
        // 否则窗口变高 → 图表变高 → 面板下移 → 又要更高，会互相追着长
        let used_above = ui.cursor().min.y - content_top;
        let height = (self.base_h - CONTROLS_H - used_above - 8.0).max(200.0);
        // Sense::click：点某一点可以直接跳到那一天（悬停提示仍在 draw 里处理）
        let (rect, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), height), Sense::click());
        geo_log("chart", rect);
        self.chart_rect = rect;
        let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
        let hover = chart::draw(ui, rect, &resp, data);
        (rect, resp, hover)
    }

    /// 数据不足（少于两个点）时在画布中央给一句提示。
    fn paint_trend_no_data(
        &self,
        painter: &egui::Painter,
        rect: Rect,
        s: &'static Strings,
        pts: &[trend::TrendPoint],
    ) {
        if pts.len() < 2 {
            painter.text(
                rect.center(),
                Align2::CENTER_CENTER,
                s.trend_no_data,
                FontId::proportional(14.0),
                Color32::from_rgb(0x99, 0x99, 0x99),
            );
        }
    }

    /// 左键点某个点 → 跳到那天（小时粒度连该小时一起选中；周/月/年粒度跳到该桶里有数据的那天）。
    fn trend_click_jump(
        &mut self,
        resp: &egui::Response,
        hover: Option<chart::ChartHover>,
        pts: &[trend::TrendPoint],
    ) {
        if !resp.clicked() {
            return;
        }
        let Some(h) = hover else {
            return;
        };
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

    /// 粒度行：每小时 / 每日 / 每周 / 每月 / 每年，右侧一句滚动与点击的提示。
    fn trend_granularity_row(&mut self, ui: &mut egui::Ui, s: &'static Strings) {
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
    }

    /// 按当前粒度聚合出曲线数据点。
    fn trend_points(&self, anchor: NaiveDate, px_to_m: impl Fn(f64) -> f64) -> Vec<trend::TrendPoint> {
        //  · 小时粒度：总计视图看“最近 72 小时”；选中某天则看那天的 24 小时（今天只画到当前钟点）
        //  · 其余粒度：窗口末端对齐锚定日期
        
        if self.granularity == Granularity::Hourly {
            // 小时粒度统一都是「以锚点结尾的最近 72 小时」：末端 = 锚点那天的「该小时」。
            // anchor_hour() = 选中小时，没选就是当前钟点 —— 今天与历史某天都走这一条，
            // 否则「今日」视图下滚轮改了钟点、曲线却锁在当前钟点不动（以前就是这么错的）。
            let end_hour = self.anchor_hour() as u32;
            let end = anchor
                .and_hms_opt(end_hour, 0, 0)
                .unwrap_or_else(|| anchor.and_hms_opt(23, 0, 0).unwrap());
            trend::build_hourly_trends(&self.snap, end, trend::HOURLY_WINDOW, px_to_m)
        } else {
            trend::build_trends(&self.snap, anchor, self.granularity, px_to_m)
        }
    }

    /// 三条曲线：键盘敲击与鼠标按键同为「次数」共用左轴，鼠标移动的距离走右轴。
    fn trend_chart_data(&self, s: &'static Strings, pts: &[trend::TrendPoint]) -> chart::ChartData {
        let key_name = s.keystrokes.to_string();
        let click_name = s.mouse_clicks.to_string();
        let mouse_name = format!("{} ({})", s.trend_mouse_distance, s.unit_m);
        // 三条曲线：键盘敲击与鼠标按键同为「次数」共用左轴，鼠标移动的距离走右轴
        chart::ChartData {
            labels: pts.iter().map(|p| p.label.clone()).collect(),
            series: vec![
                chart::Series {
                    name: key_name.clone(),
                    color: KEY_COLOR,
                    values: pts.iter().map(|p| p.keystrokes).collect(),
                    axis: chart::Axis::Left,
                },
                chart::Series {
                    name: click_name.clone(),
                    color: CLICK_COLOR,
                    values: pts.iter().map(|p| p.mouse_clicks).collect(),
                    axis: chart::Axis::Left,
                },
                chart::Series {
                    name: mouse_name.clone(),
                    color: MOUSE_COLOR,
                    values: pts.iter().map(|p| p.move_meters).collect(),
                    axis: chart::Axis::Right,
                },
            ],
        }
    }

    /// 图表悬停提示：完整标签 + 三行数值，靠边时自动翻到鼠标另一侧。
    #[allow(clippy::too_many_arguments)]
    fn trend_tooltip(
        &self,
        ui: &egui::Ui,
        painter: &egui::Painter,
        h: chart::ChartHover,
        pts: &[trend::TrendPoint],
        s: &'static Strings,
        key_name: &str,
        click_name: &str,
        mouse_name: &str,
    ) {
        let lines = self.trend_tooltip_lines(h, pts, s, key_name, click_name, mouse_name);
        paint_multiline_tooltip(painter, ui, h.pos, &lines);
    }

    /// 提示框的四行文字与配色：完整标签（灰）+ 键盘敲击 / 鼠标按键 / 鼠标移动（与曲线同色）。
    #[allow(clippy::too_many_arguments)]
    fn trend_tooltip_lines(
        &self,
        h: chart::ChartHover,
        pts: &[trend::TrendPoint],
        s: &'static Strings,
        key_name: &str,
        click_name: &str,
        mouse_name: &str,
    ) -> Vec<(String, Color32)> {
        let full = pts[h.index].full_label.clone();
        let unit = s.unit_times.trim();
        let line1 = if unit.is_empty() {
            format!("{}: {:.0}", key_name, pts[h.index].keystrokes)
        } else {
            format!("{}: {:.0} {}", key_name, pts[h.index].keystrokes, unit)
        };
        let line_click = if unit.is_empty() {
            format!("{click_name}: {:.0}", pts[h.index].mouse_clicks)
        } else {
            format!("{click_name}: {:.0} {}", pts[h.index].mouse_clicks, unit)
        };
        let line2 = format!("{}: {:.2} {}", mouse_name, pts[h.index].move_meters, s.unit_m);
        let lines: Vec<(String, Color32)> = vec![
            (full, Color32::from_rgb(0x88, 0x88, 0x88)),
            (line1, KEY_COLOR),
            (line_click, CLICK_COLOR),
            (line2, MOUSE_COLOR),
        ];
        lines
    }
}

/// 多行悬浮提示框：避让屏幕右/下边缘，靠边时翻到鼠标另一侧（单行的见 gui.rs 的 paint_tooltip）。
fn paint_multiline_tooltip(painter: &egui::Painter, ui: &egui::Ui, at: Pos2, lines: &[(String, Color32)]) {
    let font = FontId::proportional(12.0);
    let galleys: Vec<_> = lines
        .iter()
        .map(|(t, c)| painter.layout_no_wrap(t.clone(), font.clone(), *c))
        .collect();
    let w = galleys.iter().map(|g| g.size().x).fold(0.0f32, f32::max);
    let hgt: f32 = galleys.iter().map(|g| g.size().y).sum::<f32>() + 4.0 * (lines.len() as f32 - 1.0);
    let box_size = Vec2::new(w + 12.0, hgt + 12.0);
    let mut origin = at + Vec2::new(14.0, 14.0);
    let screen = ui.input(|i| i.screen_rect);
    if origin.x + box_size.x > screen.right() {
        origin.x = at.x - box_size.x - 14.0;
    }
    if origin.y + box_size.y > screen.bottom() {
        origin.y = at.y - box_size.y - 14.0;
    }
    let fg = painter
        .clone()
        .with_layer_id(egui::LayerId::new(egui::Order::Tooltip, egui::Id::new("trend_tip")));
    fg.rect_filled(Rect::from_min_size(origin, box_size), 4.0, Color32::from_rgba_unmultiplied(40, 40, 40, 240));
    let mut y = origin.y + 6.0;
    for g in &galleys {
        fg.galley(Pos2::new(origin.x + 6.0, y), g.clone(), Color32::WHITE);
        y += g.size().y + 4.0;
    }
}
