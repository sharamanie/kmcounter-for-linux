//! 日期 / 分时导航：上下翻阅、日历弹窗、视图与粒度切换 —— 从 gui.rs 拆出（第一步纯搬运）。
//!
//! use super::* 直接拿 gui 的类型、常量、自由函数与 App 的私有字段。

use super::*;

impl App {
    pub(crate) fn view_label(&self) -> String {
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
    pub(crate) fn goto_day(&mut self, date: &str) -> bool {
        match self.view_days.iter().position(|o| o.as_deref() == Some(date)) {
            Some(i) => {
                self.view_idx = i;
                self.title_dirty = true;
                true
            }
            None => false,
        }
    }

    pub(crate) fn nav_history(&mut self, newer: bool) {
        let len = self.view_days.len();
        if len <= 1 {
            return;
        }
        let old_idx = self.view_idx;
        self.view_idx = if newer {
            self.view_idx.saturating_sub(1)
        } else {
            (self.view_idx + 1).min(len - 1)
        };
        if self.view_idx != old_idx {
            self.title_dirty = true;
        }
    }

    /// 当前页面的显示区间（当周/当月/当年、排行页的当周/当月/当年/全部这类「一段」）。
    /// 返回 None 表示这里就是一个单日（照旧只显示那一天）。
    /// 当前这一段还没走完时，止日期截到今天 —— 未来没有数据，显示出来只会让人误以为在统计未来。
    pub(crate) fn nav_range(&self) -> Option<(NaiveDate, NaiveDate)> {
        if self.view_idx == 0 {
            return None; // 总计：标题就是「总计」
        }
        let anchor = self.anchor_date();
        let today = NaiveDate::parse_from_str(&self.snap_today, "%Y%m%d").unwrap_or(anchor);
        let (f, t) = match self.page {
            Page::Trends => {
                let g = match self.granularity {
                    Granularity::Weekly => Granularity::Weekly,
                    Granularity::Monthly => Granularity::Monthly,
                    Granularity::Yearly => Granularity::Yearly,
                    _ => return None, // 每小时 / 每日：就是那一天
                };
                bucket_range(g.bucket_start(anchor), g)
            }
            Page::Ranking => match self.rank_range {
                RankRange::Day => return None,
                _ => self.rank_window(),
            },
            Page::Heatmap => return None,
        };
        let t = t.min(today);
        if f >= t {
            None // 区间退化成一天（例如当月第一天）：照旧显示单日
        } else {
            Some((f, t))
        }
    }

    /// 小时选择（全天 / 整点）此刻是否可用：
    /// - 趋势页：只有「每小时」粒度才按小时看，每日/每周/每月/每年都置灰；
    /// - 排行页：始终可用（rank_rows 支持跨天按钟点筛选）；
    /// - 热力图页：始终可用（分时就是它的主要视图之一）。
    pub(crate) fn hour_controls_enabled(&self) -> bool {
        match self.page {
            // 趋势页：只有「每小时」粒度才按小时看
            Page::Trends => self.granularity == Granularity::Hourly,
            // 排行页：保留「看某个钟点」的功能（钟点栏只有导航行那一个）
            Page::Ranking | Page::Heatmap => true,
        }
    }

    /// 导航行里显示的「当前显示数据」：区间形态显示起止日期，否则还是单日/总计
    pub(crate) fn nav_display_label(&self) -> String {
        match self.nav_range() {
            Some((f, t)) => format!("{}~{}", f.format("%Y%m%d"), t.format("%Y%m%d")),
            None => self.view_label(),
        }
    }

    /// 排行页的统计区间：锚定日期（导航行选中的那天）+ 区间长度。
    /// 对齐到自然周期后**末尾截到今天**：未来没有数据，不截断会与导航行显示的区间
    /// （nav_range 同样截断）自相矛盾，汇总行的「N 天」也会把未来算进去。
    pub(crate) fn rank_window(&self) -> (NaiveDate, NaiveDate) {
        let anchor = self.anchor_date();
        let today = NaiveDate::parse_from_str(&self.snap_today, "%Y%m%d").unwrap_or(anchor);
        let g = match self.rank_range {
            RankRange::Day => Granularity::Daily,
            RankRange::Week => Granularity::Weekly,
            RankRange::Month => Granularity::Monthly,
            RankRange::Year => Granularity::Yearly,
            // 全部：从最早的数据日到锚定日期
            RankRange::All => {
                let first = self
                    .snap
                    .days
                    .keys()
                    .next()
                    .and_then(|k| NaiveDate::parse_from_str(k, "%Y%m%d").ok())
                    .unwrap_or(anchor);
                return (first.min(anchor), anchor.min(today));
            }
        };
        // 先对齐到桶的起点再取整桶：当周 = 所在那一周（周一到周日）、当月 = 那个自然月、当年 = 那个自然年。
        // 之前直接用锚点当起点，窗口落到了「锚点 → 未来」，过去的数据全被排除，看上去像是没生效。
        let (f, t) = bucket_range(g.bucket_start(anchor), g);
        (f, t.min(today).max(f))
    }

    /// 翻页（◀ ▶ / 上下键 / 滚轮共用）：
    /// 趋势页在周/月/年粒度下按对应单位滚动（滚一格 = 一周/一月/一年），其余情况仍按天翻。
    pub(crate) fn nav_step(&mut self, newer: bool) {
        // 小时粒度：一次滚一格就是一小时（跨天自动进位），与周/月/年那套一致
        if self.page == Page::Trends && self.granularity == Granularity::Hourly {
            self.nav_hour_step(newer);
            return;
        }
        let target = match self.page {
            // 趋势页：跟着粒度走
            Page::Trends => match self.granularity {
                Granularity::Weekly => Some(shift_days(self.anchor_date(), if newer { 7 } else { -7 })),
                Granularity::Monthly => Some(shift_months(self.anchor_date(), 1, newer)),
                Granularity::Yearly => Some(shift_months(self.anchor_date(), 12, newer)),
                _ => None,
            },
            // 排行页：跟着区间走（当日/全部时按天翻）
            Page::Ranking => match self.rank_range {
                RankRange::Week => Some(shift_days(self.anchor_date(), if newer { 7 } else { -7 })),
                RankRange::Month => Some(shift_months(self.anchor_date(), 1, newer)),
                RankRange::Year => Some(shift_months(self.anchor_date(), 12, newer)),
                _ => None,
            },
            _ => None,
        };
        match target {
            Some(t) => self.jump_anchor(t, newer),
            None => self.nav_history(newer),
        }
    }

    /// 小时粒度下前进/后退一小时（跨天自动进位；跨天时按有数据的日期就近落点）
    pub(crate) fn nav_hour_step(&mut self, newer: bool) {
        let date = self.anchor_date();
        let h = self.anchor_hour() as i32;
        let today = NaiveDate::parse_from_str(&self.snap_today, "%Y%m%d")
            .unwrap_or_else(|_| chrono::Local::now().date_naive());
        let (nd, nh) = if newer {
            if date >= today && h >= 23 {
                (date, 23)
            } else if h >= 23 {
                (shift_days(date, 1), 0)
            } else {
                (date, h + 1)
            }
        } else if h <= 0 {
            let earliest = self.view_days.last()
                .and_then(|o| o.as_deref())
                .and_then(|d| NaiveDate::parse_from_str(d, "%Y%m%d").ok())
                .unwrap_or(date);
            if date <= earliest {
                (date, 0)
            } else {
                (shift_days(date, -1), 23)
            }
        } else {
            (date, h - 1)
        };
        if nd != date {
            self.jump_anchor(nd, newer);
        }
        self.hour_sel = Some(nh as u8);
        self.title_dirty = true;
    }

    /// 把显示日期挪到 `target` 附近有数据的那天（该天没有数据就取最接近的一天）
    pub(crate) fn jump_anchor(&mut self, target: NaiveDate, newer: bool) {
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
                (self.view_idx + 1).min(self.view_days.len() - 1)
            };
            if idx == self.view_idx {
                return;
            }
        }
        self.view_idx = idx;
        self.title_dirty = true;
    }

    /// `view_days`（不含「总计」）里最接近 `target` 的那一项
    pub(crate) fn nearest_view_idx(&self, target: NaiveDate) -> Option<usize> {
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
    pub(crate) fn goto_trend_point(&mut self, date: NaiveDate, hour: Option<u8>) -> bool {
        let key = date.format("%Y%m%d").to_string();
        let on_this_day = self.view_idx != 0 && self.view_label() == key;
        if !on_this_day && !self.goto_day(&key) {
            return false; // 那天没有数据：不跳
        }
        self.hour_sel = hour;
        true
    }

    /// 点击趋势图上某个桶 → 跳到该桶里（或最接近的）有数据的那天
    pub(crate) fn goto_trend_bucket(&mut self, start: NaiveDate, g: Granularity) -> bool {
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

    /// 小时选择：全天 ↔ 0-23 时循环
    pub(crate) fn nav_hour(&mut self, prev: bool) {
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
    pub(crate) fn hour_title(&self, s: &'static Strings) -> String {
        match self.hour_sel {
            None => s.all_day.to_string(),
            Some(h) => hour_label(h, s),
        }
    }

    /// 日期 + 分时导航行：日期标题（点开日历）、钟点、分时折线、清除选择
    pub(crate) fn nav_row(&mut self, ui: &mut egui::Ui, s: &'static Strings) {
        ui.horizontal(|ui| {
            // 「当前显示数据」和后面的「钟点」一样：小字标签放在箭头前面
            ui.label(egui::RichText::new(s.view_date_prefix).small().color(Color32::from_rgb(0x99, 0x99, 0x99)));
            self.nav_date_controls(ui, s);
            self.nav_hour_controls(ui, s);
            self.nav_strip(ui, s);
            self.nav_clear_selection(ui, s);
        });
    }

    /// 日期部分：◀ 标题（点开日历）▶
    fn nav_date_controls(&mut self, ui: &mut egui::Ui, s: &'static Strings) {
        if ui.button("◀").clicked() {
            self.nav_step(true);
        }
        ui.add_space(4.0);
        // 固定宽度的日期标题：右箭头位置不随文字长度变化；点击打开日历。
        // 单日与区间共用同一个宽度（区间文字实测约 215px，235 够放），容器长度始终一致。
        let title = self.nav_display_label();
        let title_resp = ui
            .add_sized(
                [NAV_TITLE_W, 20.0],
                egui::Label::new(egui::RichText::new(title).strong())
                    .truncate()
                    .sense(Sense::click()),
            )
            .on_hover_cursor(egui::CursorIcon::PointingHand)
            .on_hover_text(s.cal_open_hint);
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
    }

    /// 钟点部分：全天 / 0-23 时循环切换（趋势页非每小时粒度时置灰）
    fn nav_hour_controls(&mut self, ui: &mut egui::Ui, s: &'static Strings) {
        // 分时选择：全天 / 0-23 时（循环）。
        // 趋势页在每日/每周/每月/每年粒度下用不到它（只有每小时粒度才按小时看），置灰不让改。
        let hour_enabled = self.hour_controls_enabled();
        ui.separator();
        // 页面上只有一个钟点栏（就是这里）：加个小标签说明它的用途（三个页面都显示）
        ui.label(egui::RichText::new(s.hour_sel_label).small().color(Color32::from_rgb(0x99, 0x99, 0x99)));
        ui.add_enabled_ui(hour_enabled, |ui| {
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
        });
    }

    /// 键盘内分时折线（仅 104 键布局且折线放在导航行时出现）
    fn nav_strip(&mut self, ui: &mut egui::Ui, s: &'static Strings) {
        if !(self.page == Page::Heatmap
            && self.active_layout().hour_strip == HourStripPlacement::NavRow)
        {
            return;
        }
        ui.add_space(8.0);
        let (strip, resp) = ui.allocate_exact_size(Vec2::new(260.0, 48.0), Sense::click());
        self.strip_rect = strip;
        let (prof, invalid, now_hour) = self.hour_strip_data();
        let painter = ui.painter().clone();
        paint_hour_lines(&painter, strip, &prof, self.hour_sel, now_hour, &invalid);
        if let Some(pos) = resp.hover_pos() {
            self.hour_strip_hover(ui, &painter, strip, &prof, &invalid, s, pos);
        }
        self.hour_strip_click(&resp, strip);
    }

    /// 右端：清除已选按键（只在热力图页且有选中时出现）
    fn nav_clear_selection(&mut self, ui: &mut egui::Ui, s: &'static Strings) {
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if self.page == Page::Heatmap && !self.selected.is_empty() && ui.button(s.sel_clear).clicked() {
                self.selected.clear();
            }
        });
    }

    /// 当前视图对应的“那天”：总计视图下取今天（趋势窗口以今天为末端）
    pub(crate) fn anchor_date(&self) -> NaiveDate {
        let today = NaiveDate::parse_from_str(&self.snap_today, "%Y%m%d").unwrap_or_else(|_| chrono::Local::now().date_naive());
        if self.view_idx == 0 {
            today
        } else {
            NaiveDate::parse_from_str(&self.view_label(), "%Y%m%d").unwrap_or(today)
        }
    }

    /// 趋势页“本小时”取哪个小时：选中了小时就用它，否则用现在的钟点
    pub(crate) fn anchor_hour(&self) -> u8 {
        self.hour_sel.unwrap_or_else(|| chrono::Local::now().hour().min(23) as u8)
    }

    /// 日历弹窗：点日期标题打开，直接点某天即切换（没有数据的日子不可点）
    pub(crate) fn show_calendar(&mut self, ctx: &egui::Context, s: &'static Strings) {
        let dim = Color32::from_rgb(0x99, 0x99, 0x99);
        let faint = Color32::from_rgb(0xC8, 0xC8, 0xC8);
        let today = NaiveDate::parse_from_str(&self.snap_today, "%Y%m%d").ok();
        let selected = if self.view_idx == 0 {
            None
        } else {
            NaiveDate::parse_from_str(&self.view_label(), "%Y%m%d").ok()
        };
        let (mut y, mut m) = self.cal_month;
        let (first, days_in_month, lead) = self.calendar_span();
        let pos = self.nav_title_rect.left_bottom() + Vec2::new(0.0, 4.0);
        let mut close = false;

        let win = egui::Window::new("kmc_calendar")
            .title_bar(false)
            .resizable(false)
            .collapsible(false)
            .fixed_pos(pos)
            .show(ctx, |ui| {
                ui.set_min_width(258.0);
                self.calendar_header(ui, s, first, days_in_month, &mut y, &mut m, &mut close);
                ui.add_space(2.0);
                self.calendar_grid(ui, s, first, days_in_month, lead, today, selected, dim, faint, &mut close);
            });
        self.cal_month = (y, m);
        if close {
            self.cal_open = false;
        }
        self.calendar_dismiss(ctx, win.as_ref().map(|w| w.response.rect));
    }

    /// 当前日历月份的网格参数：(1 号, 当月天数, 从周一起的空白格数)。
    fn calendar_span(&self) -> (NaiveDate, i64, usize) {
        let (y, m) = self.cal_month;
        let first = NaiveDate::from_ymd_opt(y, m, 1).unwrap_or_else(|| chrono::Local::now().date_naive());
        let next = if m == 12 {
            NaiveDate::from_ymd_opt(y + 1, 1, 1)
        } else {
            NaiveDate::from_ymd_opt(y, m + 1, 1)
        }
        .unwrap_or(first);
        (first, (next - first).num_days(), first.weekday().num_days_from_monday() as usize)
    }

    /// Esc 或点击弹窗外关闭日历。
    fn calendar_dismiss(&mut self, ctx: &egui::Context, win_rect: Option<Rect>) {
        let esc = ctx.input(|i| i.key_pressed(egui::Key::Escape));
        let clicked_out = ctx.input(|i| i.pointer.any_click())
            && ctx.input(|i| i.pointer.interact_pos()).map_or(false, |p| {
                !win_rect.map(|r| r.contains(p)).unwrap_or(false) && !self.nav_title_rect.contains(p)
            });
        if esc || clicked_out {
            self.cal_open = false;
        }
    }

    fn calendar_header(
        &mut self,
        ui: &mut egui::Ui,
        s: &'static Strings,
        first: NaiveDate,
        days_in_month: i64,
        y: &mut i32,
        m: &mut u32,
        close: &mut bool,
    ) {
        ui.horizontal(|ui| {
            if ui.button("◀").clicked() {
                let p = first - chrono::Duration::days(1);
                *y = p.year();
                *m = p.month();
            }
            ui.add_sized(
                [110.0, 20.0],
                egui::Label::new(
                    egui::RichText::new(format!("{} / {:02}", y, m)).strong().color(TEXT),
                ),
            );
            if ui.button("▶").clicked() {
                let n = first + chrono::Duration::days(days_in_month);
                *y = n.year();
                *m = n.month();
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                self.calendar_jump_buttons(ui, s, close);
            });
        });
    }

    /// 日历标题行右侧的「今天」「总计」：点了直接跳视图并关掉弹窗。
    fn calendar_jump_buttons(&mut self, ui: &mut egui::Ui, s: &'static Strings, close: &mut bool) {
        if ui.button(s.cal_today).clicked() {
            self.view_idx = 1;
            self.hour_sel = None;
            self.title_dirty = true;
            *close = true;
        }
        if ui.button(s.total_label).clicked() {
            self.view_idx = 0;
            self.title_dirty = true;
            *close = true;
        }
    }

    /// 日历网格：6 行 × 7 列，没有数据的日子不可点
    #[allow(clippy::too_many_arguments)]
    fn calendar_grid(
        &mut self,
        ui: &mut egui::Ui,
        s: &'static Strings,
        first: NaiveDate,
        days_in_month: i64,
        lead: usize,
        today: Option<NaiveDate>,
        selected: Option<NaiveDate>,
        dim: Color32,
        faint: Color32,
        close: &mut bool,
    ) {
        egui::Grid::new("kmc_cal_grid").num_columns(7).spacing([2.0, 2.0]).show(ui, |ui| {
            for wd in s.weekdays.split_whitespace() {
                ui.add_sized([32.0, 14.0], egui::Label::new(egui::RichText::new(wd).small().color(dim)));
            }
            ui.end_row();
            for cell in 0..42usize {
                match self.calendar_cell(cell, first, lead, days_in_month, today, selected, faint) {
                    Some((key, btn, has_data)) => {
                        if ui.add_enabled(has_data, btn).clicked() {
                            self.goto_day(&key);
                            *close = true;
                        }
                    }
                    None => {
                        ui.add_sized([32.0, 22.0], egui::Label::new(""));
                    }
                }
                if cell % 7 == 6 {
                    ui.end_row();
                }
            }
        });
    }

    /// 日历里的一格：返回 (日期键, 按钮, 是否有数据)；越界格返回 None。
    #[allow(clippy::too_many_arguments)]
    fn calendar_cell(
        &self,
        cell: usize,
        first: NaiveDate,
        lead: usize,
        days_in_month: i64,
        today: Option<NaiveDate>,
        selected: Option<NaiveDate>,
        faint: Color32,
    ) -> Option<(String, egui::Button<'static>, bool)> {
        let day = cell as i64 - lead as i64 + 1;
        if day < 1 || day > days_in_month {
            return None;
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
        Some((key, btn, has_data))
    }
}
