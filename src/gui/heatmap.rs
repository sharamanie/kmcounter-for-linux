//! 热力图页：按键热力图绘制、分时分布条区域 —— 从 gui.rs 拆出（第一步纯搬运）。
//!
//! use super::* 直接拿 gui 的类型、常量、自由函数与 App 的私有字段
//! （Rust 的私有性对本模块及其后代可见，所以无需把父模块的项提升为 pub(crate)）。

use super::*;

impl App {
    pub(crate) fn show_page_content(&mut self, ui: &mut egui::Ui, s: &'static Strings, content_top: f32) {
        // 日期/分时导航（热力图页与趋势页共用）
        self.nav_row(ui, s);

        // 提示行只在热力图页显示（涉及按键选择，趋势页用不到）
        if self.page == Page::Heatmap {
            self.heatmap_hint_line(ui, s);
        }

        // 欢迎横幅（仅首次运行）
        self.welcome_banner(ui, s);

        // 输入捕获警告
        self.input_capture_warning(ui, s);

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
            Page::Ranking => {
                self.show_rank_page(ui, s, content_top);
            }
        }

        // 把底部按钮行推到「收起高度」的底部：布局与当前窗口实际高度无关，
        // 因此 resize 尚未落地时也不会跳动（消除展开瞬间的一帧闪屏）
        self.content_bottom = ui.cursor().min.y;
    }

    /// 热力图页的操作提示行：选中小时时先说明该小时的键盘/鼠标量，
    /// 有选中按键时列出各键按压次数与合计，否则显示导航/选择提示。
    fn heatmap_hint_line(&mut self, ui: &mut egui::Ui, s: &'static Strings) {
        let unit = self.unit_for_view();
        let head = self.hint_head(s, &unit);
        // 提示行：有选中按键时显示各键按压次数与合计，否则显示操作提示
        let (body, color) = if self.selected.is_empty() {
            // 数据不足时合并提示
            let enough = unit.keystrokes >= if self.hour_sel.is_some() { HOUR_ENOUGH_KEYS } else { 100 };
            let body = if enough {
                format!("{} · {}", s.nav_hint, s.sel_hint)
            } else {
                format!("{} · {} · {}", s.not_enough_hint, s.nav_hint, s.sel_hint)
            };
            (body, TEXT)
        } else {
            (self.selected_keys_text(s, &unit), KEY_COLOR)
        };
        let text = if head.is_empty() { body } else { format!("{head} · {body}") };
        ui.label(egui::RichText::new(text).small().color(color));
    }

    /// 分时摘要头部：选中小时时先说明该小时的键盘/鼠标量（作废小时直接给提示）。
    fn hint_head(&self, s: &'static Strings, unit: &BucketStats) -> String {
        let unit_t = s.unit_times.trim();
        // 作废判定要读原始桶：view() 已把作废小时按 0 返回（与统计表/趋势图一致）
        if self.hour_sel.is_some_and(|h| self.day_for_view().hour_invalid(h)) {
            return s.hour_invalid.to_string();
        }
        let Some(h) = self.hour_sel else {
            return String::new();
        };
        format!(
            "{}{} · {} {} {} · {} {:.2} {}",
            h,
            s.hour_suffix,
            s.keystrokes,
            unit.keystrokes,
            unit_t,
            s.mouse_move,
            self.shared.px_to_meters(unit.mouse.move_px),
            s.unit_m
        )
    }

    /// 选中按键的明细：各键按压次数 + 合计。
    fn selected_keys_text(&self, s: &'static Strings, unit: &BucketStats) -> String {
        let unit_t = s.unit_times.trim();
        let mut parts: Vec<String> = Vec::new();
        for &i in &self.selected {
            let k = &KEYS[i];
            let raw_label = self
                .active_layout()
                .keys
                .iter()
                .find(|lk| lk.idx == i)
                .and_then(|lk| lk.label_override)
                .unwrap_or(k.label);
            let label = if raw_label.is_empty() { k.name } else { raw_label };
            let n = unit.key_count(i);
            parts.push(if unit_t.is_empty() {
                format!("{label} {n}")
            } else {
                format!("{label} {n} {unit_t}")
            });
        }
        let sum = sum_selected(unit, &self.selected);
        let sum_text = if unit_t.is_empty() {
            format!("{} {}", s.sel_sum, sum)
        } else {
            format!("{} {} {}", s.sel_sum, sum, unit_t)
        };
        format!("{}: {} · {}", s.sel_prefix, parts.join(" · "), sum_text)
    }

    /// 首次运行的欢迎横幅（× 关闭后写回配置，不再出现）
    fn welcome_banner(&mut self, ui: &mut egui::Ui, s: &'static Strings) {
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
    }

    /// 输入捕获未生效时的警告框（Linux 额外给一行排查提示）
    fn input_capture_warning(&mut self, ui: &mut egui::Ui, s: &'static Strings) {
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
    }

    pub(crate) fn paint_heatmap(&mut self, ui: &mut egui::Ui, rect: Rect, resp: egui::Response, u: f32, pitch: f32, gap: f32) {
        let painter = ui.painter().clone();
        self.kbd_rect = rect;
        let entry = self.day_for_view();
        let day = entry.view(self.hour_sel);
        let maxcount = (day.keystrokes as f64) / 10.0;
        // 数据被判定作废的小时不画热力（原始数值仍在数据里）
        let enough = !day.invalid && day.keystrokes >= if self.hour_sel.is_some() { HOUR_ENOUGH_KEYS } else { 100 };
        let key_font = FontId::proportional((self.cfg.layout.key_w as f32 * 0.21).clamp(8.0, 12.0));
        let prof = self.active_layout();

        self.paint_keyboard_keys(&painter, rect, prof, &day, enough, maxcount, &key_font, u, pitch, gap);

        // ---- 鼠标五键（自适应位置：104/87在方向键上方，60/64在键盘右侧列）----
        let pool = mouse_key_pool(&day.mouse);
        self.paint_mouse_keys(&painter, rect, &day, enough, pool, &key_font, u, pitch, gap);

        // ---- 键盘内部分时折线（104 键在小键盘上方；87/60/64 移至顶部日期导航行）----
        let strip = self.paint_kbd_hour_strip(&painter, rect, &entry, u, pitch, gap);

        // ---- 点击：键盘内分布条优先，其次是按键（选中/取消，查看按压次数）----
        self.heatmap_click(&resp, &strip, prof, rect, u, pitch, gap);

        // ---- 悬停提示：分布条显示该小时数值，按键显示该键敲击次数 ----
        if resp.hovered() {
            if let Some(pos) = ui.input(|i| i.pointer.hover_pos()) {
                self.heatmap_hover(ui, &painter, &strip, prof, &day, rect, u, pitch, gap, pos);
            }
        }
    }

    /// 键盘按键格：热力填充 + 选中描边 + 键名/方向箭头
    #[allow(clippy::too_many_arguments)]
    fn paint_keyboard_keys(
        &self,
        painter: &egui::Painter,
        rect: Rect,
        prof: &'static LayoutProfile,
        day: &BucketStats,
        enough: bool,
        maxcount: f64,
        key_font: &FontId,
        u: f32,
        pitch: f32,
        gap: f32,
    ) {
        for lk in prof.keys {
            let i = lk.idx;
            let k = &KEYS[i];
            let r = self.key_rect_for(lk, rect, u, pitch, gap);
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

            let label = lk.label_override.unwrap_or(k.label);
            if let Some(dir) = k.arrow {
                let (a, b, c) = arrow_points(dir, r);
                painter.add(Shape::convex_polygon(vec![a, b, c], TEXT, egui::Stroke::NONE));
            } else if !label.is_empty() {
                painter.text(r.center(), Align2::CENTER_CENTER, label, key_font.clone(), TEXT);
            }
        }
    }

    /// 鼠标五键格：独立绿色量程（与键盘热度池互不影响）
    #[allow(clippy::too_many_arguments)]
    fn paint_mouse_keys(
        &self,
        painter: &egui::Painter,
        rect: Rect,
        day: &BucketStats,
        enough: bool,
        pool: f64,
        key_font: &FontId,
        u: f32,
        pitch: f32,
        gap: f32,
    ) {
        for (mk_idx, mk) in MOUSE_KEYS.iter().enumerate() {
            let r = self.mouse_key_rect(rect, u, pitch, gap, mk_idx);
            let v = (mk.value)(&day.mouse);
            let fill = if enough && v > 0.0 { mouse_heat_color(v, pool) } else { BG };
            painter.rect_filled(r, 2.0, fill);
            painter.rect_stroke(r, 2.0, egui::Stroke::new(1.0_f32, Color32::WHITE));
            painter.text(r.center(), Align2::CENTER_CENTER, (mk.label)(self.s()), key_font.clone(), TEXT);
        }
    }

    /// 键盘内的 24 小时分布条：返回区域、分时数据与作废小时表（供点击/悬停复用）
    fn paint_kbd_hour_strip(
        &mut self,
        painter: &egui::Painter,
        rect: Rect,
        entry: &DayEntry,
        u: f32,
        pitch: f32,
        gap: f32,
    ) -> Option<(Rect, Vec<HourPoint>, Vec<bool>)> {
        let in_kbd_strip = matches!(self.active_layout().hour_strip, HourStripPlacement::KeyboardRect { .. });
        if in_kbd_strip {
            let s = self.hour_strip_rect(rect, u, pitch, gap);
            self.strip_rect = s;
            let hprof = entry.hour_profile();
            let hstrip_sel = self.hour_sel;
            let now_hour = (self.view_idx > 0 && self.view_label() == self.snap_today)
                .then(|| chrono::Local::now().hour().min(23) as u8);
            let invalid_hours: Vec<bool> = (0..HOURS).map(|h| entry.hour_invalid(h as u8)).collect();
            paint_hour_lines(painter, s, &hprof, hstrip_sel, now_hour, &invalid_hours);
            Some((s, hprof, invalid_hours))
        } else {
            None
        }
    }

    /// 点击热力图：分布条上选小时，按键格上增删选中
    #[allow(clippy::too_many_arguments)]
    fn heatmap_click(
        &mut self,
        resp: &egui::Response,
        strip: &Option<(Rect, Vec<HourPoint>, Vec<bool>)>,
        prof: &'static LayoutProfile,
        rect: Rect,
        u: f32,
        pitch: f32,
        gap: f32,
    ) {
        if resp.clicked() {
            if let Some(pos) = resp.interact_pointer_pos() {
                let mut hit_strip = false;
                if let Some((strip_box, _, _)) = strip {
                    if strip_box.contains(pos) {
                        if let Some(idx) = hour_bar_index(*strip_box, pos.x) {
                            let h = idx as u8;
                            self.hour_sel = if self.hour_sel == Some(h) { None } else { Some(h) };
                        }
                        hit_strip = true;
                    }
                }
                if !hit_strip {
                    for lk in prof.keys {
                        let i = lk.idx;
                        if self.key_rect_for(lk, rect, u, pitch, gap).contains(pos) {
                            if !self.selected.remove(&i) {
                                self.selected.insert(i);
                            }
                            break;
                        }
                    }
                }
            }
        }
    }

    /// 悬停提示：分布条显示该小时数值，鼠标五键显示各自计数，按键显示敲击次数
    ///
    /// 按优先级依次尝试：分时条 → 鼠标五键 → 键盘按键，前一个命中就不再往下走。
    #[allow(clippy::too_many_arguments)]
    fn heatmap_hover(
        &mut self,
        ui: &egui::Ui,
        painter: &egui::Painter,
        strip: &Option<(Rect, Vec<HourPoint>, Vec<bool>)>,
        prof: &'static LayoutProfile,
        day: &BucketStats,
        rect: Rect,
        u: f32,
        pitch: f32,
        gap: f32,
        pos: Pos2,
    ) {
        if self.hover_hour_strip(ui, painter, strip, pos) {
            return;
        }
        if self.hover_mouse_key(ui, painter, day, rect, u, pitch, gap, pos) {
            return;
        }
        self.hover_keyboard_key(ui, painter, prof, day, rect, u, pitch, gap, pos);
    }

    /// 悬停命中分时分布条：显示该小时的按键 / 点击 / 移动距离
    fn hover_hour_strip(
        &self,
        ui: &egui::Ui,
        painter: &egui::Painter,
        strip: &Option<(Rect, Vec<HourPoint>, Vec<bool>)>,
        pos: Pos2,
    ) -> bool {
        let Some((strip_box, hprof, invalid_hours)) = strip else {
            return false;
        };
        if !strip_box.contains(pos) {
            return false;
        }
        if let Some(idx) = hour_bar_index(*strip_box, pos.x) {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
            let text = self.hour_strip_tip_text(idx, hprof, invalid_hours);
            paint_tooltip(painter, ui, pos, &text);
        }
        true
    }

    /// 悬停命中鼠标五键：显示该键自己的计数（为 0 时不弹提示）
    fn hover_mouse_key(
        &self,
        ui: &egui::Ui,
        painter: &egui::Painter,
        day: &BucketStats,
        rect: Rect,
        u: f32,
        pitch: f32,
        gap: f32,
        pos: Pos2,
    ) -> bool {
        for (mk_idx, mk) in MOUSE_KEYS.iter().enumerate() {
            if !self.mouse_key_rect(rect, u, pitch, gap, mk_idx).contains(pos) {
                continue;
            }
            let v = (mk.value)(&day.mouse) as u64;
            if v > 0 {
                let unit = self.s().unit_times.trim();
                let text = if unit.is_empty() {
                    format!("{}: {v}", (mk.label)(self.s()))
                } else {
                    format!("{}: {v} {unit}", (mk.label)(self.s()))
                };
                paint_tooltip(painter, ui, pos, &text);
            }
            return true;
        }
        false
    }

    /// 分时分布条某一小时的提示文本：按键 / 点击 / 移动距离 + 作废标记。
    fn hour_strip_tip_text(&self, idx: usize, hprof: &[HourPoint], invalid_hours: &[bool]) -> String {
        let s = self.s();
        let p = hprof.get(idx).copied().unwrap_or_default();
        let unit = s.unit_times.trim();
        let mark = if invalid_hours.get(idx).copied().unwrap_or(false) {
            s.invalid_mark
        } else {
            ""
        };
        format!(
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
        )
    }

    /// 悬停命中键盘按键：显示敲击次数；Fn 键即使为 0 也提示用途
    #[allow(clippy::too_many_arguments)]
    fn hover_keyboard_key(
        &mut self,
        ui: &egui::Ui,
        painter: &egui::Painter,
        prof: &'static LayoutProfile,
        day: &BucketStats,
        rect: Rect,
        u: f32,
        pitch: f32,
        gap: f32,
        pos: Pos2,
    ) {
        for lk in prof.keys {
            let i = lk.idx;
            if !self.key_rect_for(lk, rect, u, pitch, gap).contains(pos) {
                continue;
            }
            if let Some(text) = self.key_tip_text(lk, i, day) {
                paint_tooltip(painter, ui, pos, &text);
            }
            return;
        }
    }

    /// 单个键的提示文本；没有可说的（非 Fn 且计数为 0）返回 None。
    fn key_tip_text(&self, lk: &LayoutKey, i: usize, day: &BucketStats) -> Option<String> {
        let s = self.s();
        let k = &KEYS[i];
        let count = day.key_count(i);
        let raw_label = lk.label_override.unwrap_or(k.label);
        let label = if raw_label.is_empty() { k.name } else { raw_label };
        let is_fn = i == crate::keys::FN_INDEX || label == "Fn" || k.name == "Fn";
        let unit = s.unit_times.trim();
        let text = if is_fn {
            if count > 0 {
                if unit.is_empty() {
                    format!("{label}: {count} · {}", s.fn_key_hint)
                } else {
                    format!("{label}: {count} {unit} · {}", s.fn_key_hint)
                }
            } else {
                format!("{label}: {}", s.fn_key_hint)
            }
        } else if count > 0 {
            if unit.is_empty() {
                format!("{label}: {count}")
            } else {
                format!("{label}: {count} {unit}")
            }
        } else {
            return None;
        };
        Some(text)
    }

    /// 分时分布条的位置：若在键盘内部，计算其几何区域；若在导航行，返回已分配的 strip_rect。
    pub(crate) fn hour_strip_rect(&self, rect: Rect, u: f32, pitch: f32, gap: f32) -> Rect {
        let spacing = self.cfg.layout.key_spacing as f32;
        match self.active_layout().hour_strip {
            HourStripPlacement::KeyboardRect { x, y, w, gap_count } => {
                let ox = x * u + gap_count as f32 * gap;
                let oy = y * pitch;
                Rect::from_min_size(
                    Pos2::new(rect.left() + ox, rect.top() + oy),
                    Vec2::new(w * u - spacing, pitch - spacing),
                )
            }
            HourStripPlacement::NavRow => self.strip_rect,
        }
    }
}
