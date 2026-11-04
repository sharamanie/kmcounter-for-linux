//! 设置面板与统计列表（底部内嵌展开的浅色区块）—— 从 gui.rs 拆出。
//!
//! 第二步把 288 行的 `show_settings_section` 拆开：每个小标题一个方法（历史 / 显示器 /
//! 窗口 / 语言 / 布局 / 数据校验），排布与底栏各自独立，主方法只剩外框与顺序。
//!
//! use super::* 直接拿 gui 的类型、常量、自由函数与 App 的私有字段。

use super::*;

/// 设置项左侧的说明文字（统一正文色）。
fn settings_label(ui: &mut egui::Ui, text: &str) {
    ui.label(egui::RichText::new(text).color(TEXT));
}

/// 统计表的列布局：决定每行给几格、哪一格是哪一列。
/// 抽出来只是为了让拼行的三处（普通行 / 细分行 / 只占总计格）写成一个风格。
struct StatsCols {
    with_hour: bool,
    show_view: bool,
}

impl StatsCols {
    /// 一行若干列：该小时 / 该日（或今日）/ 总计，缺哪列就跳过哪列。
    fn mk(&self, label: String, h: String, v: String, t: String) -> (String, Vec<String>, bool) {
        self.mk_row(label, h, v, t, false)
    }

    /// 细分行（键盘分区）：渲染时缩进一级。
    fn mk_sub(&self, label: String, h: String, v: String, t: String) -> (String, Vec<String>, bool) {
        self.mk_row(label, h, v, t, true)
    }

    /// 只占最后一格（总计列）的行，比如屏幕尺寸。
    fn mono(&self, label: &str, total: String) -> (String, Vec<String>, bool) {
        let mut cells = vec![String::new(); self.cells_len()];
        cells[self.cells_len() - 1] = total;
        (label.to_string(), cells, false)
    }

    fn cells_len(&self) -> usize {
        1 + usize::from(self.with_hour) + usize::from(self.show_view)
    }

    fn mk_row(&self, label: String, h: String, v: String, t: String, sub: bool) -> (String, Vec<String>, bool) {
        let mut cells = Vec::with_capacity(self.cells_len());
        if self.with_hour {
            cells.push(h);
        }
        if self.show_view {
            cells.push(v);
        }
        cells.push(t);
        (label, cells, sub)
    }
}

impl App {
    pub(crate) fn stats_table(&self, s: &'static Strings) -> (Vec<String>, Vec<(String, Vec<String>, bool)>) {
        let entry = self.day_for_view();
        let view_is_total = self.view_idx == 0;
        let view_is_today = self.view_label() == self.snap_today;
        let hour_unit = entry.view(self.hour_sel);
        let day_unit = entry.view(None);
        let total_unit = self.snap.total.view(None);

        let header = self.stats_header(s, view_is_total, view_is_today);
        let cols = StatsCols {
            with_hour: self.hour_sel.is_some(),
            show_view: !view_is_total,
        };

        let mut rows: Vec<(String, Vec<String>, bool)> = Vec::new();
        self.push_stats_key_rows(&mut rows, s, &cols, &hour_unit, &day_unit, &total_unit);
        self.push_stats_mouse_rows(&mut rows, s, &cols, &hour_unit, &day_unit, &total_unit);
        // 屏幕尺寸与时段无关，只占一格：固定放在「总计」列（最后一列），
        // 否则列数随视图变化时它会从总计列跳到「今日」列
        rows.push(cols.mono(s.monitor, monitor_size_str(&self.shared, s)));

        (header, rows)
    }

    /// 统计表的表头：列随视图变化 —— 选中小时时插入「该小时」列，
    /// 查看「总计」时不再重复两列相同的数字。
    fn stats_header(&self, s: &'static Strings, view_is_total: bool, view_is_today: bool) -> Vec<String> {
        let mut header: Vec<String> = vec![s.col_item.to_string()];
        if let Some(h) = self.hour_sel {
            header.push(hour_label(h, s));
        }
        if !view_is_total {
            header.push(if view_is_today { s.col_today.to_string() } else { self.view_label() });
        }
        header.push(s.col_total.to_string());
        header
    }

    /// 键盘相关行：鼠标移动距离 + 键盘敲击 + 四个键盘分区明细。
    fn push_stats_key_rows(
        &self,
        rows: &mut Vec<(String, Vec<String>, bool)>,
        s: &'static Strings,
        cols: &StatsCols,
        hour_unit: &BucketStats,
        day_unit: &BucketStats,
        total_unit: &BucketStats,
    ) {
        let fmt_move = |px: f64| format!("{:.2} {}", self.shared.px_to_meters(px), s.unit_m);
        rows.push(cols.mk(
            s.mouse_move.into(),
            fmt_move(hour_unit.mouse.move_px),
            fmt_move(day_unit.mouse.move_px),
            fmt_move(total_unit.mouse.move_px),
        ));
        rows.push(cols.mk(
            s.keystrokes.into(),
            hour_unit.keystrokes.to_string(),
            day_unit.keystrokes.to_string(),
            total_unit.keystrokes.to_string(),
        ));
        // 键盘敲击的细分：四个键盘分区（只统计布局内的按键，布局外的不计）
        for z in crate::keys::ZONES {
            let sum = |b: &BucketStats| crate::keys::zone_sum(z, |i| b.key_count(i));
            rows.push(cols.mk_sub(
                zone_label(z, s).to_string(),
                sum(hour_unit).to_string(),
                sum(day_unit).to_string(),
                sum(total_unit).to_string(),
            ));
        }
    }

    /// 鼠标各键六行：标签与取值函数排成一张表，省掉六段重复的闭包。
    fn push_stats_mouse_rows(
        &self,
        rows: &mut Vec<(String, Vec<String>, bool)>,
        s: &'static Strings,
        cols: &StatsCols,
        hour_unit: &BucketStats,
        day_unit: &BucketStats,
        total_unit: &BucketStats,
    ) {
        let mouse_rows: [(&str, fn(&MouseStats) -> u64); 6] = [
            (s.lbutton, |u| u.lb),
            (s.rbutton, |u| u.rb),
            (s.mbutton, |u| u.mb),
            (s.wheel, |u| u.wheel),
            (s.hwheel, |u| u.hwheel),
            (s.xbutton, |u| u.xb),
        ];
        for (label, pick) in mouse_rows {
            rows.push(cols.mk(
                label.to_string(),
                pick(&hour_unit.mouse).to_string(),
                pick(&day_unit.mouse).to_string(),
                pick(&total_unit.mouse).to_string(),
            ));
        }
    }

    /// 统计列表：内嵌展开，走正常布局流，不遮挡任何按键
    pub(crate) fn show_stats_list(&mut self, ui: &mut egui::Ui, s: &'static Strings) {
        let (header, rows) = self.stats_table(s);
        let avail_before = ui.available_width();
        let pad = group_pad(ui);
        let inner_w = (avail_before - pad.x - 2.0).max(160.0);
        let frame = egui::Frame::group(ui.style())
            .fill(Color32::from_rgb(0xF7, 0xF7, 0xF7))
            .stroke(egui::Stroke::new(1.0_f32, Color32::from_rgb(0xDD, 0xDD, 0xDD)))
            .show(ui, |ui| {
                // 展开面板的父布局是横向的；Frame 会继承父布局，这里显式竖排
                ui.vertical(|ui| {
                    // 宽度写死（min=max）：外框宽度精确等于列宽
                    ui.set_width(inner_w);
                    stats_grid(ui, &header, &rows, avail_before);
                });
            });
        geo_log("stats frame", frame.response.rect);
        geo_log("stats col avail", Rect::from_min_size(Pos2::ZERO, Vec2::new(avail_before, 0.0)));
    }

    /// 设置：底部内嵌展开（与统计列表同一套浅色风格）。
    ///
    /// 这里只搭外框与顺序；六块设置项各自一个方法，由 settings_columns 决定两栏还是三栏。
    pub(crate) fn show_settings_section(&mut self, ui: &mut egui::Ui, s: &'static Strings) {
        let avail_before = ui.available_width();
        let pad = group_pad(ui);
        let inner_w = (avail_before - pad.x - 8.0).max(200.0);
        let frame = egui::Frame::group(ui.style())
            .fill(Color32::from_rgb(0xF7, 0xF7, 0xF7))
            .stroke(egui::Stroke::new(1.0_f32, Color32::from_rgb(0xDD, 0xDD, 0xDD)))
            .show(ui, |ui| {
                ui.vertical(|ui| {
                    ui.set_max_width(inner_w);
                    ui.set_width(inner_w);

                    ui.strong(egui::RichText::new(s.settings_title).size(16.0).color(TEXT));
                    ui.add_space(2.0);

                    self.settings_columns(ui, s, inner_w);

                    ui.add_space(6.0);
                    self.settings_bottom_bar(ui, s);
                });
            });
        geo_log("settings frame", frame.response.rect);
        geo_log(
            "settings col avail",
            Rect::from_min_size(Pos2::ZERO, Vec2::new(avail_before, 0.0)),
        );
    }

    /// 设置项的排布：紧凑配列（60/64/68/75/87）或窄窗口两栏，宽窗口三栏。
    fn settings_columns(&mut self, ui: &mut egui::Ui, s: &'static Strings, inner_w: f32) {
        let is_compact_layout =
            matches!(self.cfg.layout.keyboard_layout.as_str(), "60" | "64" | "68" | "75" | "87");
        if is_compact_layout || inner_w < 820.0 {
            self.settings_columns_two(ui, s, inner_w);
        } else {
            self.settings_columns_three(ui, s, inner_w);
        }
    }

    /// 两栏排布：紧凑小排列或宽度不足时用，宽敞舒展。
    fn settings_columns_two(&mut self, ui: &mut egui::Ui, s: &'static Strings, inner_w: f32) {
        ui.columns(2, |cols| {
            let c_w = (inner_w - 12.0) / 2.0;
            cols[0].set_max_width(c_w);
            cols[1].set_max_width(c_w);
            self.render_settings_history(&mut cols[0], s);
            cols[0].add_space(6.0);
            self.render_settings_screen(&mut cols[0], s);
            cols[0].add_space(6.0);
            self.render_settings_window(&mut cols[0], s);

            self.render_settings_layout(&mut cols[1], s);
            cols[1].add_space(6.0);
            self.render_settings_limits(&mut cols[1], s);
            cols[1].add_space(6.0);
            self.render_settings_lang(&mut cols[1], s);
        });
    }

    /// 三栏排布：宽窗口（如 104 配列 1260px，或大于 820px 的大窗口）用。
    fn settings_columns_three(&mut self, ui: &mut egui::Ui, s: &'static Strings, inner_w: f32) {
        ui.columns(3, |cols| {
            let c_w = (inner_w - 24.0) / 3.0;
            cols[0].set_max_width(c_w);
            cols[1].set_max_width(c_w);
            cols[2].set_max_width(c_w);
            self.render_settings_history(&mut cols[0], s);
            cols[0].add_space(6.0);
            self.render_settings_screen(&mut cols[0], s);
            cols[0].add_space(6.0);
            self.render_settings_lang(&mut cols[0], s);

            self.render_settings_layout(&mut cols[1], s);

            self.render_settings_window(&mut cols[2], s);
            cols[2].add_space(6.0);
            self.render_settings_limits(&mut cols[2], s);
        });
    }

    /// 历史：保存多少天的历史数据。
    fn render_settings_history(&mut self, ui: &mut egui::Ui, s: &'static Strings) {
        ui.strong(egui::RichText::new(s.settings_history).size(14.0).color(TEXT));
        ui.label(egui::RichText::new(s.settings_history_sub).small().color(TEXT));
        ui.horizontal(|ui| {
            settings_label(ui, s.settings_storage);
            ui.add(egui::DragValue::new(&mut self.edit.history_days).range(0..=10000).speed(1.0));
            settings_label(ui, s.settings_days);
        });
    }

    /// 显示器：真实尺寸（0 = 自动检测），用来把像素换算成米。
    fn render_settings_screen(&mut self, ui: &mut egui::Ui, s: &'static Strings) {
        ui.strong(egui::RichText::new(s.settings_screen).size(14.0).color(TEXT));
        ui.label(egui::RichText::new(s.settings_screen_sub).small().color(TEXT));
        egui::Grid::new("screen_grid").num_columns(3).spacing([6.0, 4.0]).show(ui, |ui| {
            settings_label(ui, s.settings_width);
            ui.add(egui::DragValue::new(&mut self.edit.screen_w_mm).range(0..=20000).speed(1.0));
            settings_label(ui, s.settings_mm);
            ui.end_row();
            settings_label(ui, s.settings_height);
            ui.add(egui::DragValue::new(&mut self.edit.screen_h_mm).range(0..=20000).speed(1.0));
            settings_label(ui, s.settings_mm);
            ui.end_row();
        });
    }

    /// 窗口：从托盘恢复时的行为。
    fn render_settings_window(&mut self, ui: &mut egui::Ui, s: &'static Strings) {
        ui.strong(egui::RichText::new(s.settings_window).size(14.0).color(TEXT));
        ui.label(egui::RichText::new(s.settings_window_sub).small().color(TEXT));
        ui.checkbox(&mut self.edit.keep_on_top, s.settings_keep_on_top);
        ui.add(egui::Label::new(egui::RichText::new(s.settings_keep_on_top_sub).small().color(TEXT)).wrap());
    }

    /// 语言：跟随系统 / 中文 / 英文，保存后立刻生效。
    fn render_settings_lang(&mut self, ui: &mut egui::Ui, s: &'static Strings) {
        ui.strong(egui::RichText::new(s.settings_language).size(14.0).color(TEXT));
        ui.label(egui::RichText::new(s.settings_language_sub).small().color(TEXT));
        ui.horizontal(|ui| {
            ui.radio_value(&mut self.edit.language, "auto".to_string(), s.lang_auto);
            for (code, name) in crate::lang::LANG_CHOICES {
                if code == "zh" && !self.has_cjk {
                    continue;
                }
                ui.radio_value(&mut self.edit.language, code.to_string(), name);
            }
        });
        if !self.has_cjk {
            ui.label(egui::RichText::new(s.lang_no_cjk).small().color(DELTA_DOWN));
        }
    }

    /// 布局：配列与按键尺寸（改配列保存时会按新配列重设窗口宽度）。
    fn render_settings_layout(&mut self, ui: &mut egui::Ui, s: &'static Strings) {
        ui.strong(egui::RichText::new(s.settings_layout).size(14.0).color(TEXT));
        ui.label(egui::RichText::new(s.settings_layout_sub).small().color(TEXT));
        self.settings_layout_profile(ui, s);
        ui.add_space(2.0);
        self.settings_layout_sizes(ui, s);
    }

    /// 配列下拉：104 / 98 / 87 / 75 / 68 / 64 / 60。
    fn settings_layout_profile(&mut self, ui: &mut egui::Ui, s: &'static Strings) {
        ui.horizontal(|ui| {
            settings_label(ui, s.settings_keyboard_profile);
            let cur_layout = self.edit.layout.keyboard_layout.as_str();
            let cur_text = match cur_layout {
                "98" => s.layout_98,
                "87" => s.layout_87,
                "75" => s.layout_75,
                "68" => s.layout_68,
                "64" => s.layout_64,
                "60" => s.layout_60,
                _ => s.layout_104,
            };
            egui::ComboBox::from_id_salt("keyboard_profile_combo")
                .selected_text(cur_text)
                .width(80.0)
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut self.edit.layout.keyboard_layout, "104".to_string(), s.layout_104);
                    ui.selectable_value(&mut self.edit.layout.keyboard_layout, "98".to_string(), s.layout_98);
                    ui.selectable_value(&mut self.edit.layout.keyboard_layout, "87".to_string(), s.layout_87);
                    ui.selectable_value(&mut self.edit.layout.keyboard_layout, "75".to_string(), s.layout_75);
                    ui.selectable_value(&mut self.edit.layout.keyboard_layout, "68".to_string(), s.layout_68);
                    ui.selectable_value(&mut self.edit.layout.keyboard_layout, "64".to_string(), s.layout_64);
                    ui.selectable_value(&mut self.edit.layout.keyboard_layout, "60".to_string(), s.layout_60);
                });
        });
    }

    /// 按键尺寸网格：宽 / 高 / 横间距 / 纵间距。
    fn settings_layout_sizes(&mut self, ui: &mut egui::Ui, s: &'static Strings) {
        egui::Grid::new("layout_grid").num_columns(3).spacing([6.0, 4.0]).show(ui, |ui| {
            settings_label(ui, s.settings_key_w);
            ui.add(egui::DragValue::new(&mut self.edit.layout.key_w).range(30..=200).speed(1.0));
            settings_label(ui, s.settings_px);
            ui.end_row();
            settings_label(ui, s.settings_key_h);
            ui.add(egui::DragValue::new(&mut self.edit.layout.key_h).range(25..=200).speed(1.0));
            settings_label(ui, s.settings_px);
            ui.end_row();
            settings_label(ui, s.settings_key_spacing);
            ui.add(egui::DragValue::new(&mut self.edit.layout.key_spacing).range(0..=20).speed(1.0));
            settings_label(ui, s.settings_px);
            ui.end_row();
            settings_label(ui, s.settings_h_spacing);
            ui.add(egui::DragValue::new(&mut self.edit.layout.h_spacing).range(0..=60).speed(1.0));
            settings_label(ui, s.settings_px);
            ui.end_row();
        });
    }

    /// 数据校验：单项超过阈值的小时整点作废（0 = 关闭）。
    fn render_settings_limits(&mut self, ui: &mut egui::Ui, s: &'static Strings) {
        ui.strong(egui::RichText::new(s.settings_limits).size(14.0).color(TEXT));
        ui.scope(|ui| {
            ui.set_max_width((ui.available_width() - 14.0).max(100.0));
            ui.add(egui::Label::new(egui::RichText::new(s.settings_limits_sub).small().color(TEXT)).wrap());
        });
        egui::Grid::new("limits_grid").num_columns(3).spacing([6.0, 4.0]).show(ui, |ui| {
            settings_label(ui, s.settings_limit_move);
            ui.add(egui::DragValue::new(&mut self.edit.limit_move_km).range(0.0..=100000.0).speed(1.0).fixed_decimals(0));
            settings_label(ui, s.unit_km_per_hour);
            ui.end_row();
            settings_label(ui, s.settings_limit_click);
            ui.add(egui::DragValue::new(&mut self.edit.limit_clicks).range(0..=10_000_000).speed(10.0));
            settings_label(ui, s.unit_per_hour);
            ui.end_row();
            settings_label(ui, s.settings_limit_keys);
            ui.add(egui::DragValue::new(&mut self.edit.limit_keystrokes).range(0..=10_000_000).speed(10.0));
            settings_label(ui, s.unit_per_hour);
            ui.end_row();
        });
    }

    /// 底栏：退出（左）、导出全部历史、导出结果提示、保存 / 取消（右）。
    fn settings_bottom_bar(&mut self, ui: &mut egui::Ui, s: &'static Strings) {
        ui.horizontal(|ui| {
            // 退出放在设置里（左下角，离保存/取消远一些，避免误触）
            if ui.button(s.menu_exit).clicked() {
                self.exiting = true;
            }
            self.settings_export_button(ui, s);
            self.settings_export_status(ui);
            // 按钮靠右：宽窗口下也在容器右下角，右边缘预留充分呼吸空隙（36px）杜绝裁切
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.add_space(36.0);
                if ui.button(s.settings_save).clicked() {
                    self.settings_save(ui.ctx());
                }
                if ui.button(s.settings_cancel).clicked() {
                    self.more_open = false;
                }
            });
        });
    }

    /// 导出全部历史：CSV（每天一行）+ JSON（完整原始数据）→ <数据目录>/export/
    fn settings_export_button(&mut self, ui: &mut egui::Ui, s: &'static Strings) {
        if !ui.button(s.export_btn).clicked() {
            return;
        }
        let store = self.shared.store.lock().clone();
        let shared = self.shared.clone();
        let dir = self
            .cfg_path
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| std::path::PathBuf::from("."))
            .join("export");
        let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
        let csv = dir.join(format!("KMCounter-{stamp}.csv"));
        self.export_msg = Some((
            match crate::export::export_to(&store, &self.cfg, &csv, |px| shared.px_to_meters(px)) {
                Ok(out) => {
                    log::info!(
                        "已导出 {} 天 / {} 小时明细，文件 {:?}",
                        out.days,
                        out.hours,
                        out.files
                    );
                    format!("{} {} 天 → {}", s.export_done, out.days, dir.display())
                }
                Err(e) => {
                    log::warn!("导出失败: {e}");
                    format!("{}: {e}", s.export_fail)
                }
            },
            std::time::Instant::now(),
        ));
    }

    /// 导出结果提示（十来秒后自动消失）
    fn settings_export_status(&mut self, ui: &mut egui::Ui) {
        let Some((msg, at)) = &self.export_msg else { return };
        if at.elapsed() < Duration::from_secs(12) {
            ui.label(egui::RichText::new(msg).small().color(TEXT));
        } else {
            self.export_msg = None;
        }
    }

    /// 「保存」：落盘配置、同步语言与 HUD 开关、监控键、窗口尺寸与阈值，最后收起面板。
    fn settings_save(&mut self, ctx: &egui::Context) {
        let layout_changed = self.cfg.layout.keyboard_layout != self.edit.layout.keyboard_layout;
        let apm_window_changed = self.cfg.apm_window_secs != self.edit.apm_window_secs;
        let kps_window_changed = self.cfg.kps_window_ms != self.edit.kps_window_ms;
        self.persist_settings();
        self.apply_saved_settings(apm_window_changed, kps_window_changed);
        if layout_changed {
            self.apply_layout_resize(ctx);
        }
        self.sync_hour_limits();
        self.more_open = false;
    }

    /// 抄回悬浮窗字段后落盘，并把新配置作为当前生效值。
    fn persist_settings(&mut self) {
        // 悬浮窗设置由各自的设置窗口管理：先抄回来，避免用面板打开时的旧副本覆盖它们
        copy_hud_fields(&self.cfg, &mut self.edit);
        {
            let mut cfg = self.cfg_shared.lock();
            *cfg = self.edit.clone();
            let _ = cfg.save(&self.cfg_path);
        }
        self.cfg = self.edit.clone();
    }

    /// 保存后立刻生效的部分：HUD 开关与托盘 ✔、监控键、语言的窗口秒数。
    fn apply_saved_settings(&mut self, apm_window_changed: bool, kps_window_changed: bool) {
        self.hud_apm_show = self.cfg.hud_apm_show;
        self.hud_kps_show = self.cfg.hud_kps_show;
        // 设置里改了 HUD 开关：同步托盘菜单的 ✔
        self.push_hud_state();
        // 监控键变化立刻生效（KPS 速率只统计这些键）
        self.shared.kps.lock().set_monitored(&self.cfg.kps_key_list());
        self.apply_language();
        if apm_window_changed {
            self.shared.apm_window_secs.store(self.cfg.apm_window_secs, Ordering::Relaxed);
        }
        if kps_window_changed {
            self.shared.kps_window_ms.store(self.cfg.kps_window_ms, Ordering::Relaxed);
        }
    }

    /// 配列变了：按新配列重设窗口宽度，高度沿用当前值。
    fn apply_layout_resize(&mut self, ctx: &egui::Context) {
        let new_w = default_width_for_layout(&self.cfg.layout.keyboard_layout, &self.cfg.layout);
        self.resize_target_w = Some(new_w);
        self.user_sized = false;
        self.resize_deadline = Instant::now() + Duration::from_secs(3);
        let cur_h = self.last_applied_h.max(BASE_H);
        ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(Vec2::new(new_w, cur_h)));
    }

    /// 阈值同步给输入线程（它按这个判定小时是否作废）：
    /// 先把新屏幕尺寸写回 shared（0 = 恢复自动检测），再用同一套比例换算公里阈值，
    /// 否则同一次保存里改了屏幕尺寸的话，阈值会按旧尺寸算。
    fn sync_hour_limits(&mut self) {
        let (mm_w, mm_h) = crate::resolve_screen_mm(&self.cfg);
        let px_w = crate::resolve_screen_px_w(&self.cfg);
        self.shared.screen_w_mm.store(mm_w, Ordering::Relaxed);
        self.shared.screen_h_mm.store(mm_h, Ordering::Relaxed);
        self.shared.screen_px_w.store(px_w, Ordering::Relaxed);
        let ppm = if mm_w > 0 && px_w > 0 {
            1000.0 * px_w as f64 / mm_w as f64
        } else {
            0.0
        };
        self.shared.store.lock().limits = crate::stats::HourLimits::from_config(&self.cfg, ppm);
    }
}

/// 统计表本体：表头 + 数据行，列宽由 header 的长度决定。
fn stats_grid(
    ui: &mut egui::Ui,
    header: &[String],
    rows: &[(String, Vec<String>, bool)],
    avail_before: f32,
) {
    let dim = Color32::from_rgb(0x99, 0x99, 0x99);
    let grid_spacing_x = if avail_before < 260.0 { 12.0 } else { 16.0 };
    egui::Grid::new("stats_grid")
        .num_columns(header.len())
        .spacing([grid_spacing_x, 3.0])
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

            for (label, cells, sub) in rows {
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
}
