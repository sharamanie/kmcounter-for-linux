//! 排行页：按键 / 鼠标 / 区域聚合的排行表 —— 从 gui.rs 拆出。
//!
//! 主方法只留编排：筛选行 → 分时折线 → 排序行 → 名次表头 → 名次列表。
//!
//! use super::* 直接拿 gui 的类型、常量、自由函数与 App 的私有字段。

use super::*;

impl App {
    /// 排行页：区间 / 键位范围 / 钟点 / 指标 / 排序 筛选，下面是名次列表
    pub(crate) fn show_rank_page(&mut self, ui: &mut egui::Ui, s: &'static Strings, content_top: f32) {
        let dim = Color32::from_rgb(0x99, 0x99, 0x99);
        let (from, to) = self.rank_window();

        // ---- 筛选行 1：区间 + 键位范围 ----
        let row1_rect = self.rank_filter_row(ui, s, dim);
        ui.add_space(2.0);

        // ---- 右上角：与热力图同款的 24 小时分时折线 ----
        self.rank_hour_chart(ui, s, row1_rect);

        // ---- 筛选行 2：钟点 + 排序 + 指标 + 只看已选 ----
        self.rank_sort_row(ui, s, dim);
        ui.add_space(2.0);

        // ---- 汇总 + 列表 ----
        let only = self.rank_only_selected.then_some(&self.selected);
        let mut rows = rank_rows(&self.snap, from, to, self.hour_sel, &self.rank_scopes, only, s);
        let metric = self.rank_metric;
        rows.sort_by(|a, b| {
            let (va, vb) = (rank_value(a, metric), rank_value(b, metric));
            let ord = vb.partial_cmp(&va).unwrap_or(std::cmp::Ordering::Equal);
            let ord = if self.rank_desc { ord } else { ord.reverse() };
            ord.then_with(|| a.label.cmp(&b.label))
        });

        let days = (to - from).num_days() + 1;
        let total: u64 = rows.iter().map(|r| r.count).sum();
        // 区间已经写在导航行的「当前显示数据」里了，这里不重复，只给天数与合计
        let sum_line = format!("{} {} · {} {}", days, s.rank_days, s.rank_total, fmt_thousands(total as f64));
        ui.label(egui::RichText::new(sum_line).small().color(dim));
        ui.add_space(2.0);

        self.rank_header(ui, s, dim);

        let used_above = ui.cursor().min.y - content_top;
        let list_h = (self.base_h - CONTROLS_H - used_above - 8.0).max(120.0);
        self.rank_list(ui, s, &rows, metric, total, list_h, dim);
    }

    /// 筛选行 1：区间（当日/周/月/年/全部）+ 键位范围多选（全部 = 全勾上）。
    /// 返回这一行的矩形 —— 右上角分时折线要接着它的右边缘与顶边排布。
    fn rank_filter_row(&mut self, ui: &mut egui::Ui, s: &'static Strings, dim: Color32) -> Rect {
        let row1 = ui.horizontal(|ui| {
            self.rank_range_picker(ui, s, dim);
            ui.separator();
            self.rank_scope_picker(ui, s, dim);
        });
        geo_log("rank filters 1", row1.response.rect);
        row1.response.rect
    }

    /// 区间单选：当日 / 周 / 月 / 年 / 全部。
    fn rank_range_picker(&mut self, ui: &mut egui::Ui, s: &'static Strings, dim: Color32) {
        ui.label(egui::RichText::new(s.rank_range).small().color(dim));
        ui.selectable_value(&mut self.rank_range, RankRange::Day, s.rank_r_day);
        ui.selectable_value(&mut self.rank_range, RankRange::Week, s.rank_r_week);
        ui.selectable_value(&mut self.rank_range, RankRange::Month, s.rank_r_month);
        ui.selectable_value(&mut self.rank_range, RankRange::Year, s.rank_r_year);
        ui.selectable_value(&mut self.rank_range, RankRange::All, s.rank_r_all);
    }

    /// 键位范围多选：全部 = 全勾上；其它按钮各自开关（可同时看主键盘区 + 控制键区…）。
    fn rank_scope_picker(&mut self, ui: &mut egui::Ui, s: &'static Strings, dim: Color32) {
        ui.label(egui::RichText::new(s.rank_scope).small().color(dim));
        let all_on = ALL_SCOPES.iter().all(|z| self.rank_scopes.contains(z));
        if ui.selectable_label(all_on, s.rank_scope_all).clicked() {
            if all_on {
                self.rank_scopes.clear();
            } else {
                self.rank_scopes = ALL_SCOPES.iter().copied().collect();
            }
        }
        for (scope, label) in [
            (RankScope::Main, s.zone_main),
            (RankScope::Function, s.zone_function),
            (RankScope::Control, s.zone_control),
            (RankScope::Numpad, s.zone_numpad),
            (RankScope::Mouse, s.rank_scope_mouse),
            (RankScope::Other, s.rank_scope_other),
        ] {
            let on = self.rank_scopes.contains(&scope);
            if ui.selectable_label(on, label).clicked() {
                if on {
                    self.rank_scopes.remove(&scope);
                } else {
                    self.rank_scopes.insert(scope);
                }
            }
        }
    }

    /// 右上角的 24 小时分时折线：悬停看三项数值，点某一小时即按该小时筛选。
    fn rank_hour_chart(&mut self, ui: &mut egui::Ui, s: &'static Strings, row1_rect: Rect) {
        let right_edge = ui.max_rect().right();
        let chart = Rect::from_min_max(
            Pos2::new((right_edge - 400.0).max(row1_rect.right() + 16.0), row1_rect.top()),
            Pos2::new(right_edge, row1_rect.top() + 48.0),
        );
        let (prof, invalid, now_hour) = self.hour_strip_data();
        let painter = ui.painter().clone();
        paint_hour_lines(&painter, chart, &prof, self.hour_sel, now_hour, &invalid);
        let resp = ui.interact(chart, ui.id().with("rank_hour_chart"), Sense::click());
        if let Some(pos) = resp.hover_pos() {
            self.hour_strip_hover(ui, &painter, chart, &prof, &invalid, s, pos);
        }
        self.hour_strip_click(&resp, chart);
        self.rank_chart_rect = chart;
    }

    /// 筛选行 2：升降序 + 只看已选键。
    fn rank_sort_row(&mut self, ui: &mut egui::Ui, s: &'static Strings, dim: Color32) {
        let row2 = ui.horizontal(|ui| {
            ui.label(egui::RichText::new(s.rank_sort).small().color(dim));
            ui.selectable_value(&mut self.rank_desc, true, s.rank_desc);
            ui.selectable_value(&mut self.rank_desc, false, s.rank_asc);
            ui.separator();
            ui.checkbox(&mut self.rank_only_selected, s.rank_only_selected);
        });
        geo_log("rank filters 2", row2.response.rect);
    }

    /// 表头：按键 + 占比/日均/次数 三列（点某一列即按它排序；当前排序列加深）。
    fn rank_header(&mut self, ui: &mut egui::Ui, s: &'static Strings, dim: Color32) {
        let (hrect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 16.0), Sense::hover());
        ui.painter().text(
            Pos2::new(hrect.left() + 40.0, hrect.center().y),
            Align2::LEFT_CENTER,
            s.rank_col_key,
            FontId::proportional(11.0),
            dim,
        );
        for (m, name, off) in [
            (RankMetric::Share, s.rank_metric_share, RANK_COL_SHARE),
            (RankMetric::PerDay, s.rank_metric_day, RANK_COL_DAY),
            (RankMetric::Count, s.rank_metric_count, RANK_COL_COUNT),
        ] {
            self.rank_header_column(ui, hrect, dim, m, name, off);
        }
    }

    /// 单列表头：文字 + 点击热区（点表头切换排序指标，不用再单独放一排选项）。
    fn rank_header_column(
        &mut self,
        ui: &mut egui::Ui,
        hrect: Rect,
        dim: Color32,
        m: RankMetric,
        name: &str,
        off: f32,
    ) {
        let x = hrect.right() - off;
        let col = if self.rank_metric == m { TEXT } else { dim };
        ui.painter().text(
            Pos2::new(x, hrect.center().y),
            Align2::RIGHT_CENTER,
            name,
            FontId::proportional(11.0),
            col,
        );
        let hit = Rect::from_min_max(Pos2::new(x - 46.0, hrect.top()), Pos2::new(x + 4.0, hrect.bottom()));
        let resp = ui.interact(hit, ui.id().with(("rank_head", off as i32)), Sense::click());
        if resp.clicked() {
            self.rank_metric = m;
        }
        if resp.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
    }

    /// 名次列表：斑马纹 + 当前排序指标的条形，点一行跳到热力图页并选中该键。
    #[allow(clippy::too_many_arguments)]
    fn rank_list(
        &mut self,
        ui: &mut egui::Ui,
        s: &'static Strings,
        rows: &[RankRow],
        metric: RankMetric,
        total: u64,
        list_h: f32,
        dim: Color32,
    ) {
        let maxv = rows.iter().map(|r| rank_value(r, metric)).fold(0.0_f64, f64::max);
        let font = FontId::proportional(12.0);
        let small = FontId::proportional(11.0);

        let out = egui::ScrollArea::vertical()
            .max_height(list_h)
            .auto_shrink([false, false])
            .show(ui, |ui| {
                self.rank_list_body(ui, s, rows, metric, maxv, total, &font, &small, dim);
            });
        self.rank_rect = out.inner_rect;
        geo_log("rank list", out.inner_rect);
    }

    /// 滚动区内容：空态提示 + 逐行渲染 + 点行跳回热力图。
    #[allow(clippy::too_many_arguments)]
    fn rank_list_body(
        &mut self,
        ui: &mut egui::Ui,
        s: &'static Strings,
        rows: &[RankRow],
        metric: RankMetric,
        maxv: f64,
        total: u64,
        font: &FontId,
        small: &FontId,
        dim: Color32,
    ) {
        ui.set_width(ui.available_width());
        // 行距归零：行高就是 RANK_ROW_H（列表更紧凑，测试按行高定位也精确）
        ui.spacing_mut().item_spacing.y = 0.0;
        if rows.is_empty() || total == 0 {
            ui.add_space(24.0);
            ui.vertical_centered(|ui| {
                ui.label(egui::RichText::new(s.trend_no_data).color(dim));
            });
        }
        let w = ui.available_width();
        let mut jump: Option<usize> = None;
        for (i, r) in rows.iter().enumerate() {
            if let Some(idx) = self.rank_row_item(ui, s, r, i, metric, maxv, w, font, small, dim) {
                jump = Some(idx);
            }
        }
        if let Some(idx) = jump {
            // 点某一行 → 跳到热力图页并选中该键
            self.selected.clear();
            self.selected.insert(idx);
            self.page = Page::Heatmap;
            self.hour_sel = None;
        }
    }

    /// 单行：斑马纹 / 悬停底色、名次、键名、占比与日均与次数三列、条形、悬停提示。
    /// 返回被点击那行的键下标（鼠标行没有键下标）。
    #[allow(clippy::too_many_arguments)]
    fn rank_row_item(
        &mut self,
        ui: &mut egui::Ui,
        s: &'static Strings,
        r: &RankRow,
        i: usize,
        metric: RankMetric,
        maxv: f64,
        w: f32,
        font: &FontId,
        small: &FontId,
        dim: Color32,
    ) -> Option<usize> {
        let (rect, resp) = ui.allocate_exact_size(Vec2::new(w, RANK_ROW_H), Sense::click());
        if !ui.is_rect_visible(rect) {
            return None;
        }
        let painter = ui.painter().clone();
        self.paint_rank_row_bg(&painter, rect, i, resp.hovered());
        self.paint_rank_row_labels(&painter, rect, r, i, font, small, dim);
        self.paint_rank_row_columns(&painter, rect, r, metric, s, font);
        self.paint_rank_row_bar(&painter, rect, r, metric, maxv);
        self.rank_row_hover(ui, &resp, s, r);
        if resp.clicked() {
            r.key_idx
        } else {
            None
        }
    }

    /// 行的底色：悬停高亮优先，其次是斑马纹。
    fn paint_rank_row_bg(&self, painter: &egui::Painter, rect: Rect, i: usize, hovered: bool) {
        if hovered {
            painter.rect_filled(rect, 2.0, Color32::from_rgb(0xEC, 0xEF, 0xF3));
        } else if i % 2 == 1 {
            painter.rect_filled(rect, 2.0, Color32::from_rgb(0xFA, 0xFA, 0xFA));
        }
    }

    /// 名次与键名（左对齐）。
    fn paint_rank_row_labels(
        &self,
        painter: &egui::Painter,
        rect: Rect,
        r: &RankRow,
        i: usize,
        font: &FontId,
        small: &FontId,
        dim: Color32,
    ) {
        painter.text(
            Pos2::new(rect.left() + 6.0, rect.center().y),
            Align2::LEFT_CENTER,
            format!("{}", i + 1),
            small.clone(),
            dim,
        );
        painter.text(
            Pos2::new(rect.left() + 40.0, rect.center().y),
            Align2::LEFT_CENTER,
            r.label.clone(),
            font.clone(),
            TEXT,
        );
    }

    /// 三列数值：占比 / 日均 / 次数（都右对齐；宽窗口下也能对齐成表）。
    fn paint_rank_row_columns(
        &self,
        painter: &egui::Painter,
        rect: Rect,
        r: &RankRow,
        metric: RankMetric,
        s: &'static Strings,
        font: &FontId,
    ) {
        for (text, off, is_metric) in [
            (format!("{:.1}%", r.share * 100.0), RANK_COL_SHARE, metric == RankMetric::Share),
            (format!("{:.1} {}", r.per_day, s.rank_per_day), RANK_COL_DAY, metric == RankMetric::PerDay),
            (fmt_thousands(r.count as f64), RANK_COL_COUNT, metric == RankMetric::Count),
        ] {
            painter.text(
                Pos2::new(rect.right() - off, rect.center().y),
                Align2::RIGHT_CENTER,
                text,
                font.clone(),
                if is_metric { TEXT } else { Color32::from_rgb(0x77, 0x77, 0x77) },
            );
        }
    }

    /// 条形（按当前排序指标的最大值归一化）。
    fn paint_rank_row_bar(
        &self,
        painter: &egui::Painter,
        rect: Rect,
        r: &RankRow,
        metric: RankMetric,
        maxv: f64,
    ) {
        let frac = if maxv > 0.0 { (rank_value(r, metric) / maxv) as f32 } else { 0.0 };
        let bar_x0 = rect.left() + 130.0;
        let bar_x1 = rect.right() - RANK_COL_SHARE - 56.0;
        if bar_x1 > bar_x0 + 4.0 {
            let bar = Rect::from_min_size(
                Pos2::new(bar_x0, rect.top() + 5.0),
                Vec2::new((bar_x1 - bar_x0) * frac.clamp(0.0, 1.0), RANK_ROW_H - 10.0),
            );
            let color = if r.mouse { CLICK_COLOR } else { KEY_COLOR };
            painter.rect_filled(bar, 2.0, color.gamma_multiply(0.85));
        }
    }

    /// 悬停：光标变手形；键位行再给出一次数值提示。
    fn rank_row_hover(&self, ui: &egui::Ui, resp: &egui::Response, s: &'static Strings, r: &RankRow) {
        if !resp.hovered() {
            return;
        }
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        if r.key_idx.is_some() {
            let tip = format!(
                "{}: {} {} · {:.1}% · {:.1} {}{}",
                r.label,
                r.count,
                s.unit_times.trim(),
                r.share * 100.0,
                r.per_day,
                s.rank_per_day,
                s.rank_click_hint
            );
            resp.clone().on_hover_text(tip);
        }
    }
}
