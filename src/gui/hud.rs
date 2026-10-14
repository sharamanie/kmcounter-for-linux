//! KPS / APM 悬浮窗（HUD）与它们各自的设置窗口 —— 从 `gui.rs` 拆出（用户第二十四轮）。
//!
//! 这里放：悬浮窗度量（`kps_hud_metrics`）、键格与 KPS 图绘制、`show_*_hud`、
//! `show_*_settings`、键位捕获等。`use super::*` 直接拿 `gui` 的类型、常量与私有方法
//! （Rust 里子模块能看见父模块的私有项，`App` 的私有字段也能直接用）。

use super::*;

/// 悬浮窗 / 其设置窗口（深色底）上的文字色：白色保证可读性（主面板浅色底仍用 `TEXT`）
pub(crate) const HUD_TEXT: Color32 = Color32::WHITE;

impl App {
    #[allow(dead_code)]
    pub fn toggle_hud(&mut self, _ctx: &egui::Context) {
        self.toggle_apm_hud();
    }

    #[allow(dead_code)]
    pub fn enter_hud(&mut self, _ctx: &egui::Context) {
        self.hud_apm_show = true;
        self.cfg.hud_apm_show = true;
        let mut cfg = self.cfg_shared.lock();
        cfg.hud_apm_show = true;
        let _ = cfg.save(&self.cfg_path);
    }

    #[allow(dead_code)]
    pub fn exit_hud(&mut self, _ctx: &egui::Context) {
        self.hud_apm_show = false;
        self.cfg.hud_apm_show = false;
        let mut cfg = self.cfg_shared.lock();
        cfg.hud_apm_show = false;
        let _ = cfg.save(&self.cfg_path);
    }

    pub fn toggle_apm_hud(&mut self) {
        self.hud_apm_show = !self.hud_apm_show;
        self.cfg.hud_apm_show = self.hud_apm_show;
        let mut cfg = self.cfg_shared.lock();
        cfg.hud_apm_show = self.hud_apm_show;
        let _ = cfg.save(&self.cfg_path);
        // 窗口关了：下次打开要重新恢复一次位置
        self.apm_pos = HudPosState::default();
    }

    pub fn toggle_kps_hud(&mut self) {
        self.hud_kps_show = !self.hud_kps_show;
        self.cfg.hud_kps_show = self.hud_kps_show;
        let mut cfg = self.cfg_shared.lock();
        cfg.hud_kps_show = self.hud_kps_show;
        let _ = cfg.save(&self.cfg_path);
        // 窗口关了：下次打开要重新恢复一次位置
        self.kps_pos = HudPosState::default();
    }

    pub(crate) fn show_apm_hud(&mut self, ctx: &egui::Context, s: &'static Strings) {
        let (today_keys, today_clicks, today_dist_m) = {
            let store = self.shared.store.lock();
            if let Some(entry) = store.days.get(&self.snap_today) {
                (entry.keystrokes, entry.mouse_clicks(), self.shared.px_to_meters(entry.mouse.move_px))
            } else {
                (0, 0, 0.0)
            }
        };
        let apm_w = self.shared.apm_window_secs.load(Ordering::Relaxed);
        let (cur_apm, peak_apm) = self.shared.apm.lock().get(&self.snap_today, apm_w);

        let apm_color = if cur_apm >= 200 {
            Color32::from_rgb(0xFF, 0x55, 0x55)
        } else if cur_apm >= 100 {
            Color32::from_rgb(0xF5, 0xA6, 0x23)
        } else {
            Color32::from_rgb(0x4E, 0xC9, 0xB0)
        };

        // 显示项与顺序都固定；「设置 → 显示」里可以关掉其中任意几项
        let val = HUD_TEXT;
        let mut items: Vec<(String, String, Color32)> = Vec::new();
        if self.cfg.apm_show_keys {
            items.push((
                format!("⌨ {}", fmt_thousands(today_keys as f64)),
                format!("{}: {}", s.keystrokes, today_keys),
                val,
            ));
        }
        if self.cfg.apm_show_clicks {
            items.push((
                format!("🖱 {}", fmt_thousands(today_clicks as f64)),
                format!("{}: {}", s.mouse_clicks, today_clicks),
                val,
            ));
        }
        if self.cfg.apm_show_dist {
            items.push((
                format!("📏 {}", format_dist_m(today_dist_m)),
                format!("{}: {:.2} {}", s.mouse_move, today_dist_m, s.unit_m),
                val,
            ));
        }
        items.push((
            format!("⚡ {} APM", cur_apm),
            format!("{}: {} | {}: {}", s.apm_label, cur_apm, s.apm_peak_label, peak_apm),
            apm_color,
        ));

        let vp_id = egui::ViewportId::from_hash_of("kmc_apm_hud");
        // 宽度贴合内容：实测各项文字 + 分隔符 + 内边距，右侧不再空一大块；缩放作用在所有尺寸上。
        // 尺寸变化时连 min/max 一起下发 —— 与 KPS 悬浮窗同一个坑：
        // `with_resizable(false)` 会把 min==max 锁成初始尺寸，只发 InnerSize 会被挡住。
        let sc = apm_hud_scale(&self.cfg);
        let texts: Vec<String> = items.iter().map(|(t, _, _)| t.clone()).collect();
        let want_size = (apm_hud_measure(ctx, &texts, sc), 32.0 * sc);
        if self.apm_hud_size != want_size {
            self.apm_hud_size = want_size;
            let size = egui::vec2(want_size.0, want_size.1);
            ctx.send_viewport_cmd_to(vp_id, egui::ViewportCommand::InnerSize(size));
            ctx.send_viewport_cmd_to(vp_id, egui::ViewportCommand::MinInnerSize(size));
            ctx.send_viewport_cmd_to(vp_id, egui::ViewportCommand::MaxInnerSize(size));
        }

        // 本帧读到的窗口位置与显示器尺寸（位置记忆 / 限制在屏幕内用）。
        // Wayland 下 winit 拿不到位置（None），KDE 由 KWin 监听脚本上报。
        let mut seen_pos: Option<(f32, f32)> = None;
        let mut monitor_size: Option<(f32, f32)> = None;

        let mut close_requested = false;
        let mut open_main_requested = false;
        let mut open_settings_requested = false;

        ctx.show_viewport_immediate(
            vp_id,
            egui::ViewportBuilder::default()
                .with_title("KMC - APM")
                .with_inner_size([want_size.0, want_size.1])
                .with_min_inner_size([want_size.0, want_size.1])
                .with_max_inner_size([want_size.0, want_size.1])
                .with_resizable(false)
                .with_decorations(false)
                .with_always_on_top()
                .with_transparent(true),
            |hud_ctx, _class| {
                if hud_ctx.input(|i| i.viewport().close_requested()) {
                    close_requested = true;
                    return;
                }
                hud_ctx.request_repaint_after(Duration::from_millis(200));
                let vp = hud_ctx.input(|i| i.viewport().clone());
                if let Some(r) = vp.outer_rect {
                    seen_pos = Some((r.min.x, r.min.y));
                }
                if let Some(m) = vp.monitor_size {
                    monitor_size = Some((m.x, m.y));
                }

                let bg_fill = hud_bg_color((24, 26, 34), self.cfg.apm_hud_opacity);
                let border_stroke = egui::Stroke::new(1.0_f32, Color32::from_rgba_unmultiplied(59, 66, 82, 220));

                egui::CentralPanel::default()
                    .frame(
                        egui::Frame::none()
                            .fill(bg_fill)
                            .inner_margin(egui::Margin::symmetric(8.0 * sc, 4.0 * sc))
                            .stroke(border_stroke)
                            .rounding(6.0),
                    )
                    .show(hud_ctx, |ui| {
                        let full_rect = ui.max_rect();
                        let bg_resp = ui.interact(full_rect, ui.id().with("apm_hud_drag"), Sense::click_and_drag());
                        if bg_resp.drag_started() {
                            hud_ctx.send_viewport_cmd(egui::ViewportCommand::StartDrag);
                        }
                        if bg_resp.double_clicked() {
                            open_main_requested = true;
                        }
                        // 右键 → 打开本悬浮窗自己的设置窗口（独立视口）。
                        // 不要在 HUD 里弹右键菜单：菜单画在本窗口内，340×32 会把第二行起全裁掉（踩过）。
                        if bg_resp.clicked_by(egui::PointerButton::Secondary) {
                            open_settings_requested = true;
                        }
                        bg_resp.on_hover_text(s.hud_drag_hint);

                        ui.horizontal_centered(|ui| {
                            ui.spacing_mut().item_spacing = Vec2::new(6.0 * sc, 0.0);
                            let dim = Color32::from_rgb(0x60, 0x68, 0x78);

                            // 各项已在外层按当前显示开关备好（窗口宽度也是按它实测的）
                            for (i, (text, tip, color)) in items.iter().enumerate() {
                                if i > 0 {
                                    ui.label(egui::RichText::new("·").size(12.0 * sc).color(dim));
                                }
                                ui.label(egui::RichText::new(text.clone()).strong().size(12.0 * sc).color(*color))
                                    .on_hover_text(tip.clone());
                            }
                        });
                    });
            },
        );

        // 位置三件事：恢复一次 → 限制在屏幕内 → 稳定后落盘（记忆）
        self.tick_hud_pos(
            ctx,
            vp_id,
            true,
            seen_pos,
            monitor_size,
            want_size,
            self.cfg.apm_keep_in_screen,
        );

        if close_requested {
            self.toggle_apm_hud();
        }
        if open_main_requested {
            show_window_now(&self.shared, ctx, self.cfg.keep_on_top);
        }
        if open_settings_requested {
            self.apm_cfg_open = true; // 自己的设置窗口（见 show_apm_settings）
        }
    }

    /// KPS 悬浮窗：键格（每格显示**该键自己的 KPS**）+ 统计块 + KPS 图，三块可单独开关。
    /// 缩放（`kps_scale`）作用在所有尺寸上；键格分行由键表里的换行决定（默认每行最多 12 个）。
    /// KPS 悬浮窗：**上半区**键格（每格显示该键自己的 KPS）+ 右侧统计块，**下半区**整宽的 KPS 图。
    /// 三块可单独开关；缩放作用在所有长度上；键格分行/空位/宽度倍数都写在键表里。
    pub(crate) fn show_kps_hud(&mut self, ctx: &egui::Context, s: &'static Strings) {
        let layout = self.cfg.kps_key_layout();
        let keys_list = layout.names(); // 键名走布局解析：去掉宽度前缀 / 空位 / 换行
        let kps_w = self.shared.kps_window_ms.load(Ordering::Relaxed);
        let (cur_kps, peak_kps, avg_kps, method, key_infos, hist) = {
            let mut kps = self.shared.kps.lock();
            let (cur, peak) = kps.get(&self.snap_today, kps_w);
            let infos = kps.query_keys(&keys_list, kps_w);
            let method = kps.method();
            let avg = if self.cfg.kps_show_avg { kps.avg() } else { 0.0 };
            let now = Instant::now();
            let hist: Vec<(f32, f32)> = kps
                .history()
                .iter()
                .map(|(t, v)| (now.saturating_duration_since(*t).as_secs_f32(), *v as f32))
                .collect();
            (cur, peak, avg, method, infos, hist)
        };
        let m = kps_hud_metrics(&self.cfg, &layout);
        let show_hits = self.cfg.kps_show_hits;
        let show_key_kps = self.cfg.kps_show_key_kps;
        let show_max = self.cfg.kps_show_max;
        let show_avg = self.cfg.kps_show_avg;
        let span_secs = self.cfg.kps_graph_secs.clamp(10, 300);
        let graph_max = hist
            .iter()
            .filter(|(age, _)| *age <= span_secs as f32)
            .map(|(_, v)| *v)
            .fold(0.0f32, f32::max);

        let kps_color = if cur_kps >= 15.0 {
            Color32::from_rgb(0xFF, 0x47, 0x57)
        } else if cur_kps >= 8.0 {
            Color32::from_rgb(0xFF, 0xA5, 0x02)
        } else {
            Color32::from_rgb(0x2E, 0xD5, 0x73)
        };
        let vp_id = egui::ViewportId::from_hash_of("kmc_kps_hud");
        // 尺寸变化时下发命令。**必须连 min/max 一起改**：`with_resizable(false)` 在 winit 里会把
        // min==max 锁成初始尺寸，只发 InnerSize 会被这个约束挡住（"每行键数"改了窗口不长、第二行看不见）。
        let want_size = (m.w, m.h);
        if self.kps_hud_size != want_size {
            self.kps_hud_size = want_size;
            let size = egui::vec2(m.w, m.h);
            ctx.send_viewport_cmd_to(vp_id, egui::ViewportCommand::InnerSize(size));
            ctx.send_viewport_cmd_to(vp_id, egui::ViewportCommand::MinInnerSize(size));
            ctx.send_viewport_cmd_to(vp_id, egui::ViewportCommand::MaxInnerSize(size));
        }

        // 本帧读到的窗口位置与显示器尺寸（位置记忆 / 限制在屏幕内用）
        let mut seen_pos: Option<(f32, f32)> = None;
        let mut monitor_size: Option<(f32, f32)> = None;

        let mut close_requested = false;
        let mut open_main_requested = false;
        let mut open_settings_requested = false;

        ctx.show_viewport_immediate(
            vp_id,
            egui::ViewportBuilder::default()
                .with_title("KMC - KPS")
                .with_inner_size([m.w, m.h])
                // min/max 也显式给出（与 inner_size 相同 = 尺寸固定）：
                // winit 在 Wayland 下非 resizable 的窗口会把 min==max 锁死，改尺寸必须连它俩一起动
                .with_min_inner_size([m.w, m.h])
                .with_max_inner_size([m.w, m.h])
                .with_resizable(false)
                .with_decorations(false)
                .with_always_on_top()
                .with_transparent(true),
            |hud_ctx, _class| {
                if hud_ctx.input(|i| i.viewport().close_requested()) {
                    close_requested = true;
                    return;
                }
                hud_ctx.request_repaint_after(Duration::from_millis(40));
                let vp = hud_ctx.input(|i| i.viewport().clone());
                if let Some(r) = vp.outer_rect {
                    seen_pos = Some((r.min.x, r.min.y));
                }
                if let Some(m) = vp.monitor_size {
                    monitor_size = Some((m.x, m.y));
                }

                let bg_fill = hud_bg_color((16, 18, 24), self.cfg.kps_hud_opacity);
                let border_stroke = egui::Stroke::new(1.0_f32, Color32::from_rgba_unmultiplied(60, 68, 86, 220));

                egui::CentralPanel::default()
                    .frame(
                        egui::Frame::none()
                            .fill(bg_fill)
                            .inner_margin(egui::Margin::symmetric(m.pad, m.pad))
                            .stroke(border_stroke)
                            .rounding(6.0),
                    )
                    .show(hud_ctx, |ui| {
                        let content = ui.max_rect();
                        let bg_resp = ui.interact(content, ui.id().with("kps_hud_drag"), Sense::click_and_drag());
                        if bg_resp.drag_started() {
                            hud_ctx.send_viewport_cmd(egui::ViewportCommand::StartDrag);
                        }
                        if bg_resp.double_clicked() {
                            open_main_requested = true;
                        }
                        // 右键 → 打开本悬浮窗自己的设置窗口（独立视口，不会被窗口裁掉）
                        if bg_resp.clicked_by(egui::PointerButton::Secondary) {
                            open_settings_requested = true;
                        }
                        bg_resp.on_hover_text(s.hud_drag_hint);

                        // ---- 上半区：键格（按布局分行 / 空位 / 宽度倍数；**每行缝隙等宽、左右对称**）----
                        if self.cfg.kps_show_buttons {
                            let row_pos = key_rows_layout(&layout.rows, m.box_w, m.graph_w);
                            let mut info_idx = 0usize;
                            for (row, cells) in layout.rows.iter().enumerate() {
                                for (cell_idx, cell) in cells.iter().enumerate() {
                                    let Some(&(x_off, box_w)) = row_pos
                                        .get(row)
                                        .and_then(|r| r.get(cell_idx))
                                    else {
                                        break;
                                    };
                                    let rect = Rect::from_min_size(
                                        Pos2::new(
                                            content.left() + x_off,
                                            content.top() + row as f32 * (m.box_h + m.gap),
                                        ),
                                        Vec2::new(box_w, m.box_h),
                                    );
                                    if cell.name.is_empty() {
                                        continue; // 空位：占地方，不画不统计
                                    }
                                    let Some(k) = key_infos.get(info_idx) else { break };
                                    info_idx += 1;
                                    // 颜色对齐 osu-kps：heat（该键 KPS 的 2→4 插值）管边框色相，
                                    // 降速时沿原路缓慢退回；**按住期间底板保持满亮**（长按点亮），
                                    // 松开后由 glow 沿 50→250ms 曲线淡出
                                    let press = if k.held { 1.0_f32 } else { k.glow };
                                    let (box_bg, box_border, overlay) =
                                        key_heat_style(crate::stats::kps_key_heat(k.kps), press);
                                    ui.painter().rect(rect, 4.0, box_bg, box_border);
                                    if let Some(ovl) = overlay {
                                        ui.painter().rect_filled(rect, 4.0, ovl);
                                    }
                                    ui.painter().text(
                                        Pos2::new(rect.center().x, rect.min.y + m.box_h * 0.28),
                                        egui::Align2::CENTER_CENTER,
                                        &k.name,
                                        egui::FontId::proportional(m.font_key),
                                        HUD_TEXT,
                                    );
                                    let sub = match (show_hits, show_key_kps) {
                                        (true, true) => format!(
                                            "{} · {}",
                                            format_compact_num(k.hits),
                                            fmt_kps(k.kps, method)
                                        ),
                                        (true, false) => format_compact_num(k.hits),
                                        (false, true) => fmt_kps(k.kps, method),
                                        (false, false) => String::new(),
                                    };
                                    if !sub.is_empty() {
                                        ui.painter().text(
                                            Pos2::new(rect.center().x, rect.max.y - m.box_h * 0.28),
                                            egui::Align2::CENTER_CENTER,
                                            sub,
                                            egui::FontId::proportional(m.font_sub),
                                            HUD_TEXT,
                                        );
                                    }
                                    let resp =
                                        ui.interact(rect, ui.id().with(("kps_key", info_idx - 1)), Sense::hover());
                                    resp.on_hover_text(format!(
                                        "{}: {} · {} {}",
                                        k.name,
                                        k.hits,
                                        fmt_kps(k.kps, method),
                                        s.kps_label
                                    ));
                                }
                            }
                        }

                        // 缩放系数（度量里所有长度都按它算，这里用来算小间隙）
                        let sc = m.pad / 6.0;
                        let keys_h = if self.cfg.kps_show_buttons {
                            m.rows as f32 * m.box_h + (m.rows as f32 - 1.0) * m.gap
                        } else {
                            0.0
                        };

                        // ---- 第二栏：数据行（当前 KPS · Max · 平均），画在图的**上方** ----
                        if m.meter_w > 0.0 {
                            let stats_cy = content.top()
                                + keys_h
                                + if keys_h > 0.0 { m.pad } else { 0.0 }
                                + m.top_h * 0.5;
                            // 文字按**实测宽度**依次排下去：KPS 到两位数、三位数都不会和标签挤在一起
                            let mut x = content.left();
                            x = text_advance(
                                ui,
                                x,
                                stats_cy,
                                fmt_kps(cur_kps, method),
                                egui::FontId::proportional(m.font_cur),
                                kps_color,
                            );
                            x += 4.0 * sc;
                            x = text_advance(
                                ui,
                                x,
                                stats_cy + m.font_cur * 0.14,
                                s.kps_label.to_string(),
                                egui::FontId::proportional(m.font_sub),
                                HUD_TEXT,
                            );
                            if show_max {
                                x += 16.0 * sc;
                                x = text_advance(
                                    ui,
                                    x,
                                    stats_cy + m.font_cur * 0.14,
                                    format!("{}: {}", s.kps_max_label, fmt_kps(peak_kps, method)),
                                    egui::FontId::proportional(m.font_sub),
                                    HUD_TEXT,
                                );
                            }
                            if show_avg {
                                x += 16.0 * sc;
                                let _ = text_advance(
                                    ui,
                                    x,
                                    stats_cy + m.font_cur * 0.14,
                                    format!("{}: {}", s.avg_label, fmt_kps(avg_kps, method)),
                                    egui::FontId::proportional(m.font_sub),
                                    HUD_TEXT,
                                );
                            }
                        }

                        // ---- 第三栏：整宽的 KPS 图 ----
                        if m.graph_h > 0.0 {
                            let graph_top = content.top()
                                + keys_h
                                + if keys_h > 0.0 { m.pad } else { 0.0 }
                                + if m.meter_w > 0.0 { m.top_h + m.pad } else { 0.0 };
                            let rect = Rect::from_min_max(
                                Pos2::new(content.left(), graph_top),
                                Pos2::new(content.right(), content.bottom()),
                            );
                            paint_kps_graph(
                                ui,
                                rect,
                                &hist,
                                cur_kps as f32,
                                span_secs,
                                avg_kps as f32,
                                kps_color,
                            );
                            let resp = ui.interact(rect, ui.id().with("kps_graph"), Sense::hover());
                            resp.on_hover_text(format!(
                                "{}: {} {} / {} s",
                                s.kps_max_label,
                                fmt_kps(graph_max as f64, method),
                                s.kps_label,
                                span_secs
                            ));
                        }
                    });
            },
        );

        // 位置三件事：恢复一次 → 限制在屏幕内 → 稳定后落盘（记忆）
        self.tick_hud_pos(
            ctx,
            vp_id,
            false,
            seen_pos,
            monitor_size,
            (m.w, m.h),
            self.cfg.kps_keep_in_screen,
        );

        if close_requested {
            self.toggle_kps_hud();
        }
        if open_main_requested {
            show_window_now(&self.shared, ctx, self.cfg.keep_on_top);
        }
        if open_settings_requested {
            self.kps_cfg_open = true; // 自己的设置窗口（见 show_kps_settings）
        }
    }

    /// 进入「设置键位」捕获：清空缓冲、打开开关（输入线程会把按下的键按顺序记进来）
    fn start_kps_capture(&mut self) {
        self.shared.kps_capture_buf.lock().clear();
        self.shared.kps_capture_active.store(true, Ordering::Relaxed);
        self.kps_capturing = true;
    }

    /// 退出捕获（成功与否都要收尾，避免一直往缓冲里塞键名）
    fn stop_kps_capture(&mut self) {
        self.kps_capturing = false;
        self.shared.kps_capture_active.store(false, Ordering::Relaxed);
        self.shared.kps_capture_buf.lock().clear();
    }

    /// 悬浮窗位置三件事（每帧、每个 HUD 各一次）：
    /// ① 恢复：有记忆位置时前 3 帧连发 `OuterPosition`（窗口刚建好时命令可能被吞）；
    /// ② 限制在屏幕内：越界就把窗口推回（对齐 osu-kps 的同名选项）；
    /// ③ 记忆：位置稳定约 1.2s 且与记忆不同 → 落盘（默认开、无设置项）。
    ///
    /// Windows / X11 由 winit 直接读写位置；KDE Wayland 下 winit 拿不到位置（`seen` 为
    /// None），位置由 KWin 监听脚本上报（`kwin::hud_pos_reports`），恢复 / 夹取也在脚本里做，
    /// 这里只负责把上报的位置落盘。
    fn tick_hud_pos(
        &mut self,
        ctx: &egui::Context,
        vp_id: egui::ViewportId,
        is_apm: bool,
        seen: Option<(f32, f32)>,
        monitor: Option<(f32, f32)>,
        size: (f32, f32),
        keep_in_screen: bool,
    ) {
        let idx = if is_apm { 0 } else { 1 };
        let saved = if is_apm { self.cfg.apm_hud_pos } else { self.cfg.kps_hud_pos };

        let mut save_now: Option<(f32, f32)> = None;
        {
            let st = if is_apm { &mut self.apm_pos } else { &mut self.kps_pos };

            // ① 恢复（头三帧都发：刚建好的窗口可能吞掉第一份命令）
            if st.restored < 3 {
                if let Some(p) = saved {
                    if seen.map_or(true, |cur| pos_dist(cur, p) > 1.0) {
                        ctx.send_viewport_cmd_to(
                            vp_id,
                            egui::ViewportCommand::OuterPosition(egui::pos2(p.0, p.1)),
                        );
                    }
                }
                st.restored += 1;
            }

            // 平台拿不到位置时，看 KWin 脚本有没有上报（KDE / Wayland）
            let cur = seen.or_else(|| kde_hud_pos(idx));
            let Some(cur) = cur else { return };

            // ② 限制在屏幕内（只管 winit 能设位置的平台；KDE 的夹取在 KWin 脚本里做）
            if keep_in_screen && seen.is_some() {
                if let Some(mon) = monitor {
                    let clamped = clamp_hud_pos(cur, size, mon);
                    if pos_dist(clamped, cur) > 0.5 {
                        ctx.send_viewport_cmd_to(
                            vp_id,
                            egui::ViewportCommand::OuterPosition(egui::pos2(clamped.0, clamped.1)),
                        );
                        st.pending = None; // 正在被我们修正，中间态不落盘
                        return;
                    }
                }
            }

            // ③ 记忆：与记忆不同 → 等它安静约 1.2s → 过了启动安静期 → 落盘
            if saved.map_or(true, |p| pos_dist(p, cur) > 0.5) {
                let first_seen = match st.pending {
                    Some((p, t)) if pos_dist(p, cur) <= 0.5 => t,
                    _ => {
                        let now = Instant::now();
                        st.pending = Some((cur, now));
                        now
                    }
                };
                let now = Instant::now();
                if now.duration_since(first_seen).as_secs_f32() > 1.2
                    && now >= self.hud_pos_grace_until
                {
                    st.pending = None;
                    save_now = Some(cur);
                }
            } else {
                st.pending = None;
            }
        }
        if let Some(p) = save_now {
            self.save_hud_pos(is_apm, p);
        }
    }

    /// 记忆位置落盘（防抖后调用）：写自己的配置与共享配置；
    /// 主设置面板的副本（`edit`）靠 `copy_hud_fields` 跟上，不会把记忆冲掉。
    fn save_hud_pos(&mut self, is_apm: bool, pos: (f32, f32)) {
        if is_apm {
            self.cfg.apm_hud_pos = Some(pos);
        } else {
            self.cfg.kps_hud_pos = Some(pos);
        }
        copy_hud_fields(&self.cfg, &mut self.edit);
        let mut cfg = self.cfg_shared.lock();
        if is_apm {
            cfg.apm_hud_pos = Some(pos);
        } else {
            cfg.kps_hud_pos = Some(pos);
        }
        let _ = cfg.save(&self.cfg_path);
    }

    /// 悬浮窗设置改动后：同步到共享状态（原子量 / KPS 跟踪器）并落盘。
    /// 只改自己关心的字段、不整份覆盖 —— 托盘可能刚改过别的设置（例如开机启动）。
    fn apply_hud_cfg(&mut self) {
        self.shared.apm_window_secs.store(self.cfg.apm_window_secs, Ordering::Relaxed);
        self.shared.kps_window_ms.store(self.cfg.kps_window_ms, Ordering::Relaxed);
        {
            let mut kps = self.shared.kps.lock();
            kps.set_monitored(&self.cfg.kps_key_layout().names());
            kps.set_method(crate::stats::KpsMethod::from_config(&self.cfg.kps_method));
        }
        copy_hud_fields(&self.cfg, &mut self.edit); // 面板里的副本也跟着更新，别把新设置覆盖回去
        {
            let mut cfg = self.cfg_shared.lock();
            cfg.hud_apm_show = self.cfg.hud_apm_show;
            cfg.hud_kps_show = self.cfg.hud_kps_show;
            cfg.apm_window_secs = self.cfg.apm_window_secs;
            cfg.kps_window_ms = self.cfg.kps_window_ms;
            cfg.kps_keys = self.cfg.kps_keys.clone();
            cfg.kps_show_hits = self.cfg.kps_show_hits;
            cfg.kps_show_max = self.cfg.kps_show_max;
            cfg.kps_show_avg = self.cfg.kps_show_avg;
            cfg.apm_show_keys = self.cfg.apm_show_keys;
            cfg.apm_show_clicks = self.cfg.apm_show_clicks;
            cfg.apm_show_dist = self.cfg.apm_show_dist;
            cfg.apm_hud_opacity = self.cfg.apm_hud_opacity;
            cfg.kps_hud_opacity = self.cfg.kps_hud_opacity;
            cfg.kps_show_key_kps = self.cfg.kps_show_key_kps;
            cfg.kps_scale = self.cfg.kps_scale;
            cfg.kps_auto_width = self.cfg.kps_auto_width;
            cfg.kps_method = self.cfg.kps_method.clone();
            cfg.kps_key_count = self.cfg.kps_key_count;
            cfg.kps_show_buttons = self.cfg.kps_show_buttons;
            cfg.kps_show_stats = self.cfg.kps_show_stats;
            cfg.kps_show_graph = self.cfg.kps_show_graph;
            cfg.kps_graph_secs = self.cfg.kps_graph_secs;
            cfg.apm_scale = self.cfg.apm_scale;
            cfg.apm_keep_in_screen = self.cfg.apm_keep_in_screen;
            cfg.kps_keep_in_screen = self.cfg.kps_keep_in_screen;
            let _ = cfg.save(&self.cfg_path);
        }
        // KDE/Wayland：屏幕内限制的开关或记忆位置变了，要重载 HUD 监听脚本（幂等，没变就跳过）
        #[cfg(target_os = "linux")]
        crate::kwin::sync_hud_listener(
            self.cfg.apm_keep_in_screen,
            self.cfg.kps_keep_in_screen,
            self.cfg.apm_hud_pos,
            self.cfg.kps_hud_pos,
        );
    }

    /// APM 悬浮窗自己的设置窗口（右键悬浮窗打开）。
    ///
    /// 用**独立视口**而不是右键菜单：菜单画在悬浮窗内部，340×32 的胶囊条会把第二行起全裁掉（踩过）。
    pub(crate) fn show_apm_settings(&mut self, ctx: &egui::Context, s: &'static Strings) {
        let vp_id = egui::ViewportId::from_hash_of("kmc_apm_cfg");
        let mut close_settings = false;
        let mut open_main = false;
        let mut reset_peak = false;
        let mut toggle_hud = false;
        let mut dirty = false;

        ctx.show_viewport_immediate(
            vp_id,
            egui::ViewportBuilder::default()
                .with_title(format!("KMC - {}", s.apm_cfg_title))
                .with_inner_size([336.0, 196.0])
                .with_resizable(false)
                .with_decorations(false)
                .with_always_on_top()
                .with_transparent(true),
            |cfg_ctx, _class| {
                if cfg_ctx.input(|i| i.viewport().close_requested()) {
                    close_settings = true;
                    return;
                }
                cfg_ctx.request_repaint_after(Duration::from_millis(400));
                let bg = hud_bg_color((24, 26, 34), self.cfg.apm_hud_opacity);
                let closed = settings_shell(cfg_ctx, "⚡", s.apm_cfg_title, s.cfg_close, bg, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new(s.settings_apm_window).small().color(HUD_TEXT));
                        if ui
                            .add(egui::DragValue::new(&mut self.cfg.apm_window_secs).range(5..=300).speed(1.0))
                            .on_hover_text(s.settings_apm_window_sub)
                            .changed()
                        {
                            dirty = true;
                        }
                        ui.label(egui::RichText::new("s").small().color(HUD_TEXT));
                        ui.add_space(10.0);
                        ui.label(egui::RichText::new(s.hud_opacity).small().color(HUD_TEXT));
                        if ui
                            .add(
                                egui::DragValue::new(&mut self.cfg.apm_hud_opacity)
                                    .range(20..=100)
                                    .speed(1.0)
                                    .suffix("%"),
                            )
                            .changed()
                        {
                            dirty = true;
                        }
                    });
                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        if ui.checkbox(&mut self.cfg.apm_show_keys, s.apm_show_keys).changed() {
                            dirty = true;
                        }
                        if ui.checkbox(&mut self.cfg.apm_show_clicks, s.apm_show_clicks).changed() {
                            dirty = true;
                        }
                        if ui.checkbox(&mut self.cfg.apm_show_dist, s.apm_show_dist).changed() {
                            dirty = true;
                        }
                    });
                    ui.add_space(4.0);
                    // 缩放 + 限制在屏幕内（对齐 osu-kps 的同名选项）
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new(s.kps_cfg_scale).small().color(HUD_TEXT));
                        if ui
                            .add(
                                egui::DragValue::new(&mut self.cfg.apm_scale)
                                    .range(60..=400)
                                    .speed(1.0)
                                    .suffix("%"),
                            )
                            .on_hover_text(s.apm_cfg_scale_hint)
                            .changed()
                        {
                            dirty = true;
                        }
                        ui.add_space(10.0);
                        if ui
                            .checkbox(&mut self.cfg.apm_keep_in_screen, s.hud_keep_in_screen)
                            .on_hover_text(s.hud_keep_in_screen_hint)
                            .changed()
                        {
                            dirty = true;
                        }
                    });
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        if ui.button(s.hud_open_main).clicked() {
                            open_main = true;
                        }
                        if ui.button(s.hud_reset_peak).clicked() {
                            reset_peak = true;
                        }
                        let hud_btn = if self.cfg.hud_apm_show { s.hud_close } else { s.hud_show };
                        if ui.button(hud_btn).clicked() {
                            toggle_hud = true;
                        }
                    });
                    ui.add_space(4.0);
                    ui.label(
                        egui::RichText::new(s.hud_cfg_hint)
                            .small()
                            .color(Color32::from_rgb(0xC8, 0xCC, 0xD4)),
                    );
                });
                if closed {
                    close_settings = true;
                }
            },
        );

        // 窗口内的改动即时生效（写盘集中在改动/关闭两个时机，拖动滑杆不会每帧写文件）
        if dirty {
            self.apply_hud_cfg();
        }
        if toggle_hud {
            self.toggle_apm_hud();
            self.apply_hud_cfg();
        }
        if reset_peak {
            self.shared.apm.lock().reset_peak();
        }
        if open_main {
            show_window_now(&self.shared, ctx, self.cfg.keep_on_top);
        }
        if close_settings {
            self.apm_cfg_open = false;
            self.apply_hud_cfg(); // 关闭时兜底落盘
        }
    }

    /// KPS 悬浮窗自己的设置窗口（右键悬浮窗打开）：窗口长度、监控键、显示项、不透明度、重置与关闭。
    pub(crate) fn show_kps_settings(&mut self, ctx: &egui::Context, s: &'static Strings) {
        let vp_id = egui::ViewportId::from_hash_of("kmc_kps_cfg");
        let mut close_settings = false;
        let mut open_main = false;
        let mut reset_counts = false;
        let mut reset_peak = false;
        let mut reset_graph = false;
        let mut reset_all = false;
        let mut toggle_hud = false;
        let mut dirty = false;
        let mut start_capture = false;
        let mut cancel_capture = false;

        ctx.show_viewport_immediate(
            vp_id,
            egui::ViewportBuilder::default()
                .with_title(format!("KMC - {}", s.kps_cfg_title))
                .with_inner_size([400.0, 312.0])
                .with_resizable(false)
                .with_decorations(false)
                .with_always_on_top()
                .with_transparent(true),
            |cfg_ctx, _class| {
                if cfg_ctx.input(|i| i.viewport().close_requested()) {
                    close_settings = true;
                    return;
                }
                // 捕获键位时要及时反馈，其它时候没必要刷那么勤
                let interval = if self.kps_capturing { 80 } else { 400 };
                cfg_ctx.request_repaint_after(Duration::from_millis(interval));
                let bg = hud_bg_color((16, 18, 24), self.cfg.kps_hud_opacity);
                let closed = settings_shell(cfg_ctx, "🎯", s.kps_cfg_title, s.cfg_close, bg, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new(s.settings_kps_window).small().color(HUD_TEXT));
                        if ui
                            .add(egui::DragValue::new(&mut self.cfg.kps_window_ms).range(200..=5000).speed(50.0))
                            .on_hover_text(s.settings_kps_window_sub)
                            .changed()
                        {
                            dirty = true;
                        }
                        ui.label(egui::RichText::new("ms").small().color(HUD_TEXT));
                        ui.add_space(8.0);
                        // 计算方式（对齐 osu-kps 的「硬算 / 灵敏」）
                        ui.label(egui::RichText::new(s.kps_cfg_method).small().color(HUD_TEXT));
                        let cur = crate::stats::KpsMethod::from_config(&self.cfg.kps_method);
                        egui::ComboBox::from_id_salt("kps_method")
                            .selected_text(match cur {
                                crate::stats::KpsMethod::Hard => s.kps_method_hard,
                                crate::stats::KpsMethod::Sensitive => s.kps_method_sensitive,
                            })
                            .width(66.0)
                            .show_ui(ui, |ui| {
                                if ui
                                    .selectable_label(
                                        cur == crate::stats::KpsMethod::Hard,
                                        egui::RichText::new(s.kps_method_hard).color(HUD_TEXT),
                                    )
                                    .on_hover_text(s.kps_method_hard_hint)
                                    .clicked()
                                {
                                    self.cfg.kps_method =
                                        crate::stats::KpsMethod::Hard.as_config().to_string();
                                    dirty = true;
                                }
                                if ui
                                    .selectable_label(
                                        cur == crate::stats::KpsMethod::Sensitive,
                                        egui::RichText::new(s.kps_method_sensitive).color(HUD_TEXT),
                                    )
                                    .on_hover_text(s.kps_method_sensitive_hint)
                                    .clicked()
                                {
                                    self.cfg.kps_method =
                                        crate::stats::KpsMethod::Sensitive.as_config().to_string();
                                    dirty = true;
                                }
                            })
                            .response
                            .on_hover_text(match cur {
                                crate::stats::KpsMethod::Hard => s.kps_method_hard_hint,
                                crate::stats::KpsMethod::Sensitive => s.kps_method_sensitive_hint,
                            });
                    });
                    ui.horizontal(|ui| {
                        // 缩放 + 每行键数 + 不透明度
                        ui.label(egui::RichText::new(s.kps_cfg_scale).small().color(HUD_TEXT));
                        if ui
                            .add(
                                egui::DragValue::new(&mut self.cfg.kps_scale)
                                    .range(60..=400)
                                    .speed(1.0)
                                    .suffix("%"),
                            )
                            .on_hover_text(s.kps_cfg_scale_hint)
                            .changed()
                        {
                            dirty = true;
                        }
                        ui.add_space(6.0);
                        if ui
                            .checkbox(&mut self.cfg.kps_auto_width, s.kps_auto_width)
                            .on_hover_text(s.kps_auto_width_hint)
                            .changed()
                        {
                            dirty = true;
                        }
                        ui.add_space(6.0);
                        ui.label(egui::RichText::new(s.hud_opacity).small().color(HUD_TEXT));
                        if ui
                            .add(
                                egui::DragValue::new(&mut self.cfg.kps_hud_opacity)
                                    .range(20..=100)
                                    .speed(1.0)
                                    .suffix("%"),
                            )
                            .changed()
                        {
                            dirty = true;
                        }
                    });
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new(s.settings_kps_keys).small().color(HUD_TEXT));
                        // 多行输入：支持"换行 = 分行 / 空项 = 空位 / 1.5H、4H = 宽度倍数"
                        if ui
                            .add(
                                egui::TextEdit::multiline(&mut self.cfg.kps_keys)
                                    .desired_width(180.0)
                                    .desired_rows(3)
                                    .font(egui::TextStyle::Monospace)
                                    .hint_text("E,R,Q,W,T,\nY,U,I,O,P,\nA,S,D,F,G,\nH,J"),
                            )
                            .on_hover_text(s.settings_kps_keys_hint)
                            .changed()
                        {
                            // 手改键表时把"键数"跟着对一下，避免两处打架
                            self.cfg.kps_key_count =
                                self.cfg.kps_key_layout().names().len().clamp(1, 26) as u8;
                            dirty = true;
                        }
                    });
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new(s.kps_key_count).small().color(HUD_TEXT));
                        let mut count = self.cfg.kps_key_count as i32;
                        if ui
                            .add(egui::DragValue::new(&mut count).range(1..=26).speed(1.0))
                            .changed()
                        {
                            self.cfg.kps_key_count = count.clamp(1, 26) as u8;
                            self.cfg.resize_kps_keys();
                            dirty = true;
                        }
                        if ui
                            .button(s.kps_set_keys)
                            .on_hover_text(s.kps_set_keys_hint)
                            .clicked()
                        {
                            start_capture = true;
                        }
                        if ui
                            .button(s.kps_default_keys)
                            .on_hover_text(s.kps_default_keys_hint)
                            .clicked()
                        {
                            self.cfg.kps_keys = crate::config::Config::default_kps_keys_for(
                                self.cfg.kps_key_count as usize,
                            )
                            .join(", ");
                            dirty = true;
                        }
                    });
                    ui.horizontal(|ui| {
                        // 重置菜单（对齐 osu-kps 的重置子菜单：按键总次数 / 最大 KPS / KPS 图 / 以上所有）
                        ui.menu_button(s.kps_reset, |ui| {
                            ui.label(egui::RichText::new(s.kps_reset_hint).small().color(HUD_TEXT));
                            ui.separator();
                            if ui.button(s.kps_reset_counts).clicked() {
                                reset_counts = true;
                                ui.close_menu();
                            }
                            if ui.button(s.kps_reset_peak).clicked() {
                                reset_peak = true;
                                ui.close_menu();
                            }
                            if ui.button(s.kps_reset_graph).clicked() {
                                reset_graph = true;
                                ui.close_menu();
                            }
                            ui.separator();
                            if ui.button(s.kps_reset_all).clicked() {
                                reset_all = true;
                                ui.close_menu();
                            }
                        });
                    });
                    // 「设置键位」捕获中：显示进度 + 已捕获的键 + 取消（凑满 N 个就自动写入）
                    if self.kps_capturing {
                        let need = self.cfg.kps_key_count.max(1) as usize;
                        let captured = self.shared.kps_capture_buf.lock().clone();
                        ui.horizontal(|ui| {
                            ui.label(
                                egui::RichText::new(s.kps_capture_prompt)
                                    .small()
                                    .color(Color32::from_rgb(0xFF, 0xD0, 0x60)),
                            );
                            ui.label(
                                egui::RichText::new(format!(
                                    "{}/{}",
                                    captured.len().min(need),
                                    need
                                ))
                                .small()
                                .strong()
                                .color(Color32::from_rgb(0xFF, 0xD0, 0x60)),
                            );
                            let names = if captured.is_empty() {
                                s.kps_capture_none.to_string()
                            } else {
                                captured.join(", ")
                            };
                            ui.label(egui::RichText::new(names).small().color(HUD_TEXT));
                            if ui.small_button(s.kps_capture_cancel).clicked() {
                                cancel_capture = true;
                            }
                        });
                        if captured.len() >= need {
                            self.cfg.kps_keys = captured.into_iter().take(need).collect::<Vec<_>>().join(", ");
                            self.cfg.kps_key_count = need as u8;
                            dirty = true;
                            cancel_capture = true;
                        }
                    }
                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        if ui.checkbox(&mut self.cfg.kps_show_buttons, s.kps_show_buttons).changed() {
                            dirty = true;
                        }
                        if ui.checkbox(&mut self.cfg.kps_show_stats, s.kps_show_stats).changed() {
                            dirty = true;
                        }
                        if ui.checkbox(&mut self.cfg.kps_show_graph, s.kps_show_graph).changed() {
                            dirty = true;
                        }
                        if self.cfg.kps_show_graph {
                            ui.label(egui::RichText::new(s.kps_cfg_graph_secs).small().color(HUD_TEXT));
                            if ui
                                .add(egui::DragValue::new(&mut self.cfg.kps_graph_secs).range(10..=300).speed(5.0))
                                .changed()
                            {
                                dirty = true;
                            }
                            ui.label(egui::RichText::new("s").small().color(HUD_TEXT));
                        }
                    });
                    ui.horizontal(|ui| {
                        if ui.checkbox(&mut self.cfg.kps_show_hits, s.settings_kps_show_hits).changed() {
                            dirty = true;
                        }
                        if ui.checkbox(&mut self.cfg.kps_show_key_kps, s.kps_show_key_kps).changed() {
                            dirty = true;
                        }
                        if ui.checkbox(&mut self.cfg.kps_show_max, s.settings_kps_show_max).changed() {
                            dirty = true;
                        }
                        if ui.checkbox(&mut self.cfg.kps_show_avg, s.kps_show_avg).changed() {
                            dirty = true;
                        }
                    });
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        if ui.button(s.hud_open_main).clicked() {
                            open_main = true;
                        }
                        let hud_btn = if self.cfg.hud_kps_show { s.hud_close } else { s.hud_show };
                        if ui.button(hud_btn).clicked() {
                            toggle_hud = true;
                        }
                        ui.add_space(10.0);
                        // 限制在屏幕内（对齐 osu-kps 的同名选项）
                        if ui
                            .checkbox(&mut self.cfg.kps_keep_in_screen, s.hud_keep_in_screen)
                            .on_hover_text(s.hud_keep_in_screen_hint)
                            .changed()
                        {
                            dirty = true;
                        }
                    });
                    ui.add_space(4.0);
                    ui.label(
                        egui::RichText::new(s.hud_cfg_hint)
                            .small()
                            .color(Color32::from_rgb(0xC8, 0xCC, 0xD4)),
                    );
                });
                if closed {
                    close_settings = true;
                }
            },
        );

        if start_capture {
            self.start_kps_capture();
        }
        if cancel_capture {
            self.stop_kps_capture();
        }
        if dirty {
            self.apply_hud_cfg();
        }
        if toggle_hud {
            self.toggle_kps_hud();
            self.apply_hud_cfg();
        }
        if reset_counts || reset_peak || reset_graph || reset_all {
            let mut kps = self.shared.kps.lock();
            if reset_all {
                kps.reset_all();
            } else {
                if reset_counts {
                    kps.reset_counts();
                }
                if reset_peak {
                    kps.reset_peak();
                }
                if reset_graph {
                    kps.reset_graph();
                }
            }
        }
        if open_main {
            show_window_now(&self.shared, ctx, self.cfg.keep_on_top);
        }
        if close_settings {
            self.kps_cfg_open = false;
            self.apply_hud_cfg();
        }
    }

    #[cfg(any(test, feature = "softshot"))]
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn debug_toggle_hud(&mut self) {
        self.toggle_apm_hud();
    }

    #[cfg(feature = "softshot")]
    #[allow(dead_code)]
    pub(crate) fn debug_is_hud(&self) -> bool {
        self.hud_apm_show
    }

    /// 直接开关"悬浮窗自己的设置窗口"（测试用：右键路径见 softshot 的右键测试）
    #[cfg(feature = "softshot")]
    #[allow(dead_code)]
    pub(crate) fn debug_set_cfg_open(&mut self, apm: bool, kps: bool) {
        self.apm_cfg_open = apm;
        self.kps_cfg_open = kps;
    }

    /// 给 KPS 跟踪器喂一次击键（测试用）
    #[cfg(feature = "softshot")]
    #[allow(dead_code)]
    pub(crate) fn debug_record_kps_key(&mut self, date: &str, key: &str) {
        let w = self.shared.kps_window_ms.load(Ordering::Relaxed);
        self.shared.kps.lock().record(date, key, w);
    }

    #[cfg(feature = "softshot")]
    #[allow(dead_code)]
    pub(crate) fn debug_record_apm_action(&mut self, date: &str) {
        let apm_w = self.shared.apm_window_secs.load(Ordering::Relaxed);
        self.shared.apm.lock().record(date, apm_w);
    }

    /// 设置某键的「按住」状态（测试用：长按点亮）
    #[cfg(feature = "softshot")]
    #[allow(dead_code)]
    pub(crate) fn debug_set_kps_held(&mut self, key: &str, down: bool) {
        self.shared.kps.lock().set_held(key, down);
    }

    /// 按悬浮窗的查询口径读某键状态 `(held, glow)`（测试用）
    #[cfg(feature = "softshot")]
    #[allow(dead_code)]
    pub(crate) fn debug_kps_key_state(&mut self, key: &str) -> (bool, f32) {
        let w = self.shared.kps_window_ms.load(Ordering::Relaxed);
        let infos = self.shared.kps.lock().query_keys(&[key.to_string()], w);
        infos.first().map(|k| (k.held, k.glow)).unwrap_or((false, 0.0))
    }

    /// APM 胶囊条最近一次下发的窗口尺寸（测试用）
    #[cfg(feature = "softshot")]
    #[allow(dead_code)]
    pub(crate) fn debug_apm_hud_size(&self) -> (f32, f32) {
        self.apm_hud_size
    }
}

/// KPS 悬浮窗的尺寸与字号（缩放后的像素值）——抽成纯函数，便于测试与排查布局
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct KpsHudMetrics {
    pub w: f32,
    pub h: f32,
    /// 每行键格数
    pub cols: usize,
    /// 行数
    pub rows: usize,
    pub pad: f32,
    pub box_w: f32,
    pub box_h: f32,
    pub gap: f32,
    pub meter_w: f32,
    pub graph_w: f32,
    pub graph_h: f32,
    /// 上半区（键格 + 统计块）的高度
    pub top_h: f32,
    pub font_key: f32,
    pub font_sub: f32,
    pub font_cur: f32,
}

/// 计算 KPS 悬浮窗尺寸：**三栏竖排 —— 键位 / 数据 / 图**。
/// - 键位：按用户写的分行 / 空位 / 宽度倍数（默认每行最多 12 个）；
/// - 数据：一行横排「当前 KPS · Max · 平均」（画在图的**上方**，不会再压到图上）；
/// - 图：整宽的 KPS 图。
/// 缩放（`kps_scale`）作用在所有长度上。
pub(crate) fn kps_hud_metrics(
    cfg: &crate::config::Config,
    layout: &crate::config::KpsKeyLayout,
) -> KpsHudMetrics {
    let s = (cfg.kps_scale.clamp(60, 400) as f32) / 100.0;
    let pad = 6.0 * s;
    let gap = 4.0 * s;
    let unit = 44.0 * s; // 一"格"的宽度
    let box_h = 30.0 * s;
    let v_gap = 6.0 * s;
    let pitch = unit + gap;
    let font_cur = 16.0 * s;
    let font_sub = 9.0 * s;

    let rows = layout.rows.len().max(1);
    let keys_w = if cfg.kps_show_buttons {
        layout
            .rows
            .iter()
            .map(|r| r.iter().map(|c| c.width).sum::<f32>() * pitch - gap)
            .fold(0.0f32, f32::max)
            .max(unit)
    } else {
        0.0
    };
    let keys_h = if cfg.kps_show_buttons {
        rows as f32 * box_h + (rows as f32 - 1.0) * gap
    } else {
        0.0
    };
    // 数据行：一行文字（当前 / Max / 平均），宽度给足，数字变长也不至于被窗口裁掉
    let stats_w = if cfg.kps_show_stats { 210.0 * s } else { 0.0 };
    let stats_h = if cfg.kps_show_stats { font_cur * 1.3 } else { 0.0 };
    let graph_h = if cfg.kps_show_graph { 64.0 * s } else { 0.0 };
    let min_graph_w = 200.0 * s;

    let content_w = keys_w.max(stats_w).max(if graph_h > 0.0 { min_graph_w } else { 0.0 }).max(unit);
    let mut h = pad * 2.0;
    let mut blocks = 0;
    if keys_h > 0.0 {
        h += keys_h;
        blocks += 1;
    }
    if stats_h > 0.0 {
        if blocks > 0 {
            h += v_gap;
        }
        h += stats_h;
        blocks += 1;
    }
    if graph_h > 0.0 {
        if blocks > 0 {
            h += v_gap;
        }
        h += graph_h;
    }

    KpsHudMetrics {
        w: (pad * 2.0 + content_w).ceil(),
        h: h.ceil(),
        cols: layout.cols.round() as usize,
        rows,
        pad,
        box_w: unit,
        box_h,
        gap,
        meter_w: stats_w,
        graph_w: content_w,
        graph_h,
        top_h: stats_h,
        font_key: 11.0 * s,
        font_sub,
        font_cur,
    }
}

/// 两个 RGBA 颜色的线性插值（`t=0` → `a`，`t=1` → `b`）
pub(crate) fn mix_rgba(a: (u8, u8, u8, u8), b: (u8, u8, u8, u8), t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0);
    let m = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color32::from_rgba_unmultiplied(m(a.0, b.0), m(a.1, b.1), m(a.2, b.2), m(a.3, b.3))
}

/// 键格配色（对齐 osu-kps 的两条颜色动画）：
/// - `heat`（该键 KPS 经 2→4 的 sigmoid 插值）管**色相**：边框从灰蓝渐到青绿，
///   按压频率降下来时沿原路缓慢退回；
/// - `press`（按下窗口内 = 1，之后沿 50→250ms 曲线淡出）管**底板**：按下瞬间亮起。
///
/// 返回 `(底板, 描边, 叠加层)`；叠加层 `None` 表示当前不用叠亮。
pub(crate) fn key_heat_style(heat: f32, press: f32) -> (Color32, egui::Stroke, Option<Color32>) {
    const ACCENT: (u8, u8, u8) = (0x00, 0xBD, 0xB0); // 原「激活高亮青绿」
    const PRESS_FROM: (u8, u8, u8) = (0xD0, 0xD4, 0xDC); // 慢速按下闪的是这个浅灰（theme 色）
    let heat = heat.clamp(0.0, 1.0);
    let press = press.clamp(0.0, 1.0);
    let border = mix_rgba((70, 75, 95, 160), (ACCENT.0, ACCENT.1, ACCENT.2, 235), heat);
    let fill = Color32::from_rgba_unmultiplied(35, 38, 48, 220);
    // 叠加层的 alpha 上限 0.675 → 对齐 osu-kps（按下 10→30ms 亮到 0.675、松开后淡出）
    let a = (0.675 * press * 255.0).round() as u8;
    let overlay = if a == 0 {
        None
    } else {
        Some(mix_rgba(
            (PRESS_FROM.0, PRESS_FROM.1, PRESS_FROM.2, a),
            (ACCENT.0, ACCENT.1, ACCENT.2, a),
            heat,
        ))
    };
    (fill, egui::Stroke::new(1.0 + 0.5 * heat, border), overlay)
}

/// 一行键格的对称排布：**每条缝等宽（含左右两侧的边距）** ——
/// 把内容区的富余宽度平均分给每条缝，右侧不再空一大块（用户第二十四轮第 2 条）。
/// 多行时取「各行都铺得下」的最紧缝宽，其余行整体居中，行内仍左右对称。
///
/// 返回每行每格的 `(相对内容区左缘的 x, 格宽)`，与 `rows` 逐格对应；
/// 宽度倍数 `w` 的格宽 = `w × 盒宽 + (w−1) × 缝宽`。
pub(crate) fn key_rows_layout(
    rows: &[Vec<crate::config::KpsKeyCell>],
    box_w: f32,
    content_w: f32,
) -> Vec<Vec<(f32, f32)>> {
    let units_of = |cells: &[crate::config::KpsKeyCell]| -> f32 {
        cells.iter().map(|c| c.width.max(0.1)).sum()
    };
    // 全场统一的缝宽：取各行里最「挤」的那个（内容宽 − 盒子总宽）÷（格数 + 1）
    let gap = rows
        .iter()
        .map(|r| {
            let u = units_of(r);
            if u <= 0.0 {
                f32::INFINITY
            } else {
                ((content_w - box_w * u) / (u + 1.0)).max(0.0)
            }
        })
        .fold(f32::INFINITY, f32::min);
    let gap = if gap.is_finite() { gap } else { 0.0 };
    rows.iter()
        .map(|r| {
            let u = units_of(r);
            if u <= 0.0 {
                return Vec::new();
            }
            // 行宽（不含两侧边距）与两侧边距；限制行恰好左右各一个 gap，其余行居中
            let row_w = box_w * u + gap * (u - 1.0);
            let mut x = (content_w - row_w) * 0.5;
            let mut out = Vec::with_capacity(r.len());
            for c in r {
                let w = c.width.max(0.1);
                let bw = w * box_w + (w - 1.0).max(0.0) * gap;
                out.push((x, bw));
                x += bw + gap;
            }
            out
        })
        .collect()
}

/// 悬浮窗「位置记忆」的每窗口状态（`App` 持有，两个 HUD 各一份）
#[derive(Default)]
pub(crate) struct HudPosState {
    /// 恢复命令已下发的帧数（窗口刚建好时命令可能被吞，前 3 帧都发；到 3 即完成）
    restored: u8,
    /// 等待「安静下来」再落盘的候选位置：(位置, 首次出现时刻)
    pending: Option<((f32, f32), Instant)>,
}

/// 两点间的切比雪夫距离（判断「位置变没变」用，防抖动抖到亚像素）
fn pos_dist(a: (f32, f32), b: (f32, f32)) -> f32 {
    (a.0 - b.0).abs().max((a.1 - b.1).abs())
}

/// 把窗口位置夹进 (0, 0)~(显示器宽高) 的屏幕范围内
///（对齐 osu-kps 的「限制在屏幕内」；多显示器负坐标布局会被当作越界，见 README）
pub(crate) fn clamp_hud_pos(pos: (f32, f32), size: (f32, f32), monitor: (f32, f32)) -> (f32, f32) {
    let max_x = (monitor.0 - size.0).max(0.0);
    let max_y = (monitor.1 - size.1).max(0.0);
    (pos.0.clamp(0.0, max_x), pos.1.clamp(0.0, max_y))
}

/// KDE/Wayland 下 KWin 监听脚本上报的最新 HUD 位置（[APM, KPS]；其他平台恒为 None）
#[cfg(target_os = "linux")]
fn kde_hud_pos(idx: usize) -> Option<(f32, f32)> {
    crate::kwin::hud_pos_reports()[idx]
}
#[cfg(not(target_os = "linux"))]
fn kde_hud_pos(_idx: usize) -> Option<(f32, f32)> {
    None
}

/// APM 胶囊条的缩放系数（`apm_scale` 百分比 → 0.6~4.0，对齐 KPS 的缩放）
pub(crate) fn apm_hud_scale(cfg: &crate::config::Config) -> f32 {
    (cfg.apm_scale.clamp(60, 400) as f32) / 100.0
}

/// APM 胶囊条的目标宽度（纯函数，便于测试）：
/// `item_w` 是各项文字的实测宽度、`dot_w` 是分隔符「·」的宽度；
/// 布局 = 内边距 8×2 + 各项 + 分隔符（其左右各有一段 6px 的 item_spacing）+ 4px 余量。
pub(crate) fn apm_hud_width(item_w: &[f32], dot_w: f32, scale: f32) -> f32 {
    let mut w = 16.0 * scale; // 左右内边距 8×2
    for (i, iw) in item_w.iter().enumerate() {
        w += iw;
        if i + 1 < item_w.len() {
            w += 12.0 * scale + dot_w; // 分隔符两侧各 6px 的 item_spacing
        }
    }
    (w + 4.0 * scale).ceil()
}

/// 用 egui 字体实测 APM 胶囊条各项文字的宽度并算出目标宽度（与绘制同字号）。
pub(crate) fn apm_hud_measure(ctx: &egui::Context, texts: &[String], scale: f32) -> f32 {
    let item_font = egui::FontId::proportional(12.0 * scale);
    let dot_font = item_font.clone();
    let item_w: Vec<f32> = texts
        .iter()
        .map(|t| ctx.fonts(|f| f.layout_no_wrap(t.clone(), item_font.clone(), Color32::WHITE).size().x))
        .collect();
    let dot_w = ctx.fonts(|f| f.layout_no_wrap("·".into(), dot_font, Color32::WHITE).size().x);
    apm_hud_width(&item_w, dot_w, scale)
}

/// 画一段文字并返回它的右边缘 x —— 横向排布时按实测宽度接下去，
/// 数字变长（比如 KPS 到两位数）也不会和后面的标签挤在一起。
pub(crate) fn text_advance(
    ui: &egui::Ui,
    left: f32,
    center_y: f32,
    text: String,
    font: egui::FontId,
    color: Color32,
) -> f32 {
    let galley = ui.painter().layout_no_wrap(text, font, color);
    let w = galley.size().x;
    let h = galley.size().y;
    ui.painter().galley(Pos2::new(left, center_y - h * 0.5), galley, color);
    left + w
}

/// KPS 数值的显示格式：硬算是整数，灵敏方式是 1 位小数
pub(crate) fn fmt_kps(v: f64, method: crate::stats::KpsMethod) -> String {
    match method {
        crate::stats::KpsMethod::Hard => format!("{}", v.round() as i64),
        crate::stats::KpsMethod::Sensitive => format!("{v:.1}"),
    }
}

/// 纵轴刻度取"好看"的上界（1、2、5、10、20、50…），让柱高稳定、不随每帧重算跳变
pub(crate) fn nice_kps_max(v: f32) -> f32 {
    let v = v.max(1.0);
    let mut unit = 1.0f32;
    while unit * 10.0 <= v {
        unit *= 10.0;
    }
    for m in [1.0f32, 2.0, 5.0, 10.0] {
        if v <= unit * m {
            return unit * m;
        }
    }
    unit * 10.0
}

/// KPS 图（参照 osu-kps：每秒一条的历史柱状图；再叠一条 KeysPerSecond 那样的平均线）。
///
/// **按整秒分桶**画：`hist` 里每条样本按"年龄向下取整"落到对应那一秒的柱子上，当前这一秒另外补上
/// `current`。这样数值每秒只向左移一格 —— 不会像按小数年龄连续映射那样每帧抖动，也不会把旧峰值
/// 涂抹到后面的时间里（之前"高峰迟迟不消失 + 一直闪 + 一次填掉 1/3 界面"就是这么来的）。
pub(crate) fn paint_kps_graph(
    ui: &mut egui::Ui,
    rect: Rect,
    hist: &[(f32, f32)],
    current: f32,
    span_secs: u32,
    avg: f32,
    accent: Color32,
) {
    let painter = ui.painter();
    painter.rect_filled(rect, 3.0, Color32::from_rgba_unmultiplied(10, 12, 18, 190));

    let span = span_secs.clamp(10, 300) as usize;
    let mut buckets = vec![0.0f32; span];
    for (age, v) in hist {
        let idx = if *age < 0.0 { 0 } else { *age as usize };
        if idx < span {
            let pos = span - 1 - idx;
            buckets[pos] = buckets[pos].max(*v);
        }
    }
    buckets[span - 1] = buckets[span - 1].max(current.max(0.0));

    let maxv = nice_kps_max(buckets.iter().copied().fold(0.0f32, f32::max));
    let usable_h = (rect.height() - 2.0).max(1.0);
    // 按**整数像素列**画、每列只画一次、颜色不透明：
    // 半透明柱在 0.75px 步进下会互相叠加（窗口一动颜色就变 → 闪），这里彻底避开。
    let cols = rect.width().max(1.0).floor() as usize;
    let dim_accent = {
        let c = accent.to_array();
        Color32::from_rgb((c[0] as u32 * 3 / 5) as u8, (c[1] as u32 * 3 / 5) as u8, (c[2] as u32 * 3 / 5) as u8)
    };
    let px_per_bucket = cols as f32 / span as f32;
    for col in 0..cols {
        // 这一列覆盖的桶（可能跨多个桶，取其中最大值）
        let b0 = (col as f32 / px_per_bucket).floor() as usize;
        let b1 = (((col + 1) as f32 / px_per_bucket).ceil() as usize).min(span);
        let v = buckets[b0.min(span.saturating_sub(1))..b1.max(b0 + 1).min(span)]
            .iter()
            .copied()
            .fold(0.0f32, f32::max);
        let h = (v / maxv) * usable_h;
        if h <= 0.4 {
            continue;
        }
        let x = rect.left() + col as f32;
        painter.rect_filled(
            Rect::from_min_max(Pos2::new(x, rect.bottom() - h), Pos2::new(x + 1.0, rect.bottom())),
            0.0,
            dim_accent,
        );
        // 顶部亮一截
        painter.rect_filled(
            Rect::from_min_max(
                Pos2::new(x, rect.bottom() - h),
                Pos2::new(
                    x + 1.0,
                    (rect.bottom() - h + 1.5 * ui.ctx().pixels_per_point()).min(rect.bottom()),
                ),
            ),
            0.0,
            accent,
        );
    }
    // 平均线
    if avg > 0.0 {
        let y = rect.bottom() - ((avg as f32 / maxv) * usable_h).clamp(0.0, usable_h);
        painter.line_segment(
            [Pos2::new(rect.left(), y), Pos2::new(rect.right(), y)],
            egui::Stroke::new(1.0_f32, Color32::from_rgba_unmultiplied(0xDD, 0xDD, 0xEE, 110)),
        );
    }
    painter.rect_stroke(
        rect,
        3.0,
        egui::Stroke::new(1.0_f32, Color32::from_rgba_unmultiplied(70, 78, 96, 150)),
    );
}

/// 悬浮窗相关设置（由各自的设置窗口维护）：把 `from` 的这批字段抄给 `to`。
///
/// 主设置面板保存时用的是一份**打开面板那一刻的旧副本**（`self.edit`），
/// 不做这一步的话，面板里的「保存」会把悬浮窗里的新设置覆盖回旧值。
pub(crate) fn copy_hud_fields(from: &crate::config::Config, to: &mut crate::config::Config) {
    to.hud_apm_show = from.hud_apm_show;
    to.hud_kps_show = from.hud_kps_show;
    to.apm_window_secs = from.apm_window_secs;
    to.kps_window_ms = from.kps_window_ms;
    to.kps_keys = from.kps_keys.clone();
    to.kps_show_hits = from.kps_show_hits;
    to.kps_show_max = from.kps_show_max;
    to.kps_show_avg = from.kps_show_avg;
    to.apm_show_keys = from.apm_show_keys;
    to.apm_show_clicks = from.apm_show_clicks;
    to.apm_show_dist = from.apm_show_dist;
    to.apm_hud_opacity = from.apm_hud_opacity;
    to.kps_hud_opacity = from.kps_hud_opacity;
    to.kps_show_key_kps = from.kps_show_key_kps;
    to.kps_scale = from.kps_scale;
    to.kps_auto_width = from.kps_auto_width;
    to.kps_method = from.kps_method.clone();
    to.kps_key_count = from.kps_key_count;
    to.kps_show_buttons = from.kps_show_buttons;
    to.kps_show_stats = from.kps_show_stats;
    to.kps_show_graph = from.kps_show_graph;
    to.kps_graph_secs = from.kps_graph_secs;
    to.apm_scale = from.apm_scale;
    to.apm_keep_in_screen = from.apm_keep_in_screen;
    to.kps_keep_in_screen = from.kps_keep_in_screen;
    to.apm_hud_pos = from.apm_hud_pos;
    to.kps_hud_pos = from.kps_hud_pos;
}

/// 悬浮窗背景色：深色底 + 可调不透明度（0~100；`rgb` 是底色 RGB）
pub(crate) fn hud_bg_color(rgb: (u8, u8, u8), opacity: u8) -> Color32 {
    let a = ((opacity.min(100) as u32 * 255) / 100) as u8;
    Color32::from_rgba_unmultiplied(rgb.0, rgb.1, rgb.2, a)
}

/// 悬浮窗设置窗口的公共外壳：深色圆角背景 + 可拖动标题行（右端是关闭按钮）+ Esc 关闭。
/// `body` 里画各自的设置项；返回 true 表示用户要求关闭本窗口。
pub(crate) fn settings_shell(
    cfg_ctx: &egui::Context,
    icon: &str,
    title: &str,
    close_hint: &str,
    bg: Color32,
    body: impl FnOnce(&mut egui::Ui),
) -> bool {
    let mut close = false;
    egui::CentralPanel::default()
        .frame(
            egui::Frame::none()
                .fill(bg)
                .inner_margin(egui::Margin::symmetric(10.0, 8.0))
                .stroke(egui::Stroke::new(1.0_f32, Color32::from_rgba_unmultiplied(70, 78, 96, 230)))
                .rounding(8.0),
        )
        .show(cfg_ctx, |ui| {
            // 标题行：左段可拖动（Wayland 下无边框窗口只能靠这招移动），右端是关闭按钮。
            // 两者用互不重叠的矩形交互，避免"点关闭却拖动了窗口"。
            let (row, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 22.0), Sense::hover());
            let btn_rect = Rect::from_min_size(Pos2::new(row.right() - 22.0, row.top()), Vec2::splat(22.0));
            let drag_rect = Rect::from_min_max(row.min, Pos2::new(btn_rect.left() - 4.0, row.bottom()));
            let drag = ui.interact(drag_rect, ui.id().with("cfg_drag"), Sense::click_and_drag());
            if drag.drag_started() {
                cfg_ctx.send_viewport_cmd(egui::ViewportCommand::StartDrag);
            }
            ui.painter().text(
                Pos2::new(row.left() + 2.0, row.center().y),
                egui::Align2::LEFT_CENTER,
                format!("{icon} {title}"),
                egui::FontId::proportional(13.0),
                Color32::from_rgb(0xDD, 0xDD, 0xEE),
            );
            // 关闭按钮：**用两条线画 ✕**，不走字体（"✕" 这类字符在部分字体里没有，
            // 会渲染成豆腐块 —— 之前设置窗口右上角那个方框就是这么来的）
            let btn = ui.interact(btn_rect, ui.id().with("cfg_close"), Sense::click());
            let color = if btn.hovered() { Color32::WHITE } else { Color32::from_rgb(0xAA, 0xBB, 0xCC) };
            let arm = 3.5;
            let c = btn_rect.center();
            ui.painter().line_segment(
                [Pos2::new(c.x - arm, c.y - arm), Pos2::new(c.x + arm, c.y + arm)],
                egui::Stroke::new(1.4_f32, color),
            );
            ui.painter().line_segment(
                [Pos2::new(c.x + arm, c.y - arm), Pos2::new(c.x - arm, c.y + arm)],
                egui::Stroke::new(1.4_f32, color),
            );
            if btn.clicked() {
                close = true;
            }
            btn.on_hover_text(close_hint);
            ui.add_space(4.0);
            body(ui);
        });
    // Esc 也能关掉设置窗口
    if cfg_ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        close = true;
    }
    close
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cell(name: &str) -> crate::config::KpsKeyCell {
        crate::config::KpsKeyCell { name: name.into(), width: 1.0 }
    }

    /// 键行对称：每条缝（含左右两侧边距）等宽 —— 右侧不再空一大块（用户第二十四轮第 2 条）
    #[test]
    fn key_rows_layout_is_symmetric() {
        // 4 键 1 行：内容宽 210、盒宽 44 → 每条缝 = (210 − 4×44) ÷ 5 = 6.8
        let rows = vec![vec![cell("Q"), cell("W"), cell("E"), cell("R")]];
        let pos = key_rows_layout(&rows, 44.0, 210.0);
        let r = &pos[0];
        assert_eq!(r.len(), 4);
        let mut gaps = vec![r[0].0];
        for w in r.windows(2) {
            gaps.push(w[1].0 - (w[0].0 + w[0].1));
        }
        gaps.push(210.0 - (r.last().unwrap().0 + r.last().unwrap().1));
        for g in &gaps {
            assert!((g - 6.8).abs() < 0.05, "每条缝都应等宽 6.8：{gaps:?}");
        }

        // 宽度倍数（2 格宽）的盒宽 = 2 个盒 + 1 条缝；缝仍处处等宽
        let rows = vec![vec![
            crate::config::KpsKeyCell { name: "H".into(), width: 2.0 },
            cell("J"),
        ]];
        let pos = key_rows_layout(&rows, 44.0, 210.0);
        let r = &pos[0];
        let m = r[0].0; // 左边距
        assert!((r[1].0 - (r[0].0 + r[0].1) - m).abs() < 0.05, "中间缝 == 边距");
        assert!((210.0 - (r[1].0 + r[1].1) - m).abs() < 0.05, "右边距 == 左边距");
        assert!((r[0].1 - (88.0 + m)).abs() < 0.05, "2 格宽盒 = 2×44 + 1 条缝");

        // 多行：缝宽统一（取最紧的那行），窄行整体居中、行内左右对称
        let rows = vec![
            vec![cell("A"), cell("B"), cell("C"), cell("D")],
            vec![cell("E"), cell("F")],
        ];
        let pos = key_rows_layout(&rows, 44.0, 210.0);
        let r2 = &pos[1];
        let inner = r2[1].0 - (r2[0].0 + r2[0].1);
        assert!((inner - 6.8).abs() < 0.05, "两行缝宽要一致：{inner}");
        let right = 210.0 - (r2[1].0 + r2[1].1);
        assert!((r2[0].0 - right).abs() < 0.05, "窄行左右对称：{} vs {right}", r2[0].0);
    }

    /// 键格配色（对齐 osu-kps）：heat 管边框色相、press 管底板亮度
    #[test]
    fn key_heat_style_endpoints() {
        // 注意：egui 的 Color32 内部是**预乘 alpha**（而且在线性空间里乘），
        // 不该拿 `r()` 原值和名义值硬对 —— 期望值用同一构造器生成再比。
        let (fill, border, ovl) = key_heat_style(0.0, 0.0);
        assert_eq!(fill, Color32::from_rgba_unmultiplied(35, 38, 48, 220));
        assert_eq!(
            border.color,
            Color32::from_rgba_unmultiplied(70, 75, 95, 160),
            "冷状态边框是灰蓝"
        );
        assert!((border.width - 1.0).abs() < 1e-6);
        assert!(ovl.is_none(), "没按下 → 底板不叠亮");

        let (_, border, ovl) = key_heat_style(1.0, 1.0);
        assert_eq!(
            border.color,
            Color32::from_rgba_unmultiplied(0, 189, 176, 235),
            "满热度 = 青绿"
        );
        assert!((border.width - 1.5).abs() < 1e-6);
        let ovl = ovl.expect("按下 → 底板亮");
        assert_eq!(ovl.a(), (0.675_f32 * 255.0).round() as u8, "osu-kps 的 0.675 上限");
        assert_eq!(
            ovl,
            Color32::from_rgba_unmultiplied(0, 189, 176, ovl.a()),
            "高速按下闪青绿"
        );

        // 慢速按下：闪的是 theme 色的浅灰，不是青绿
        let (_, _, ovl) = key_heat_style(0.0, 1.0);
        let o = ovl.expect("按下要亮");
        assert_eq!(o, Color32::from_rgba_unmultiplied(0xD0, 0xD4, 0xDC, o.a()));

        // 热度越高、边框越偏青绿：红通道单调下降
        let r0 = key_heat_style(0.0, 0.0).1.color.r();
        let r50 = key_heat_style(0.5, 0.0).1.color.r();
        let r100 = key_heat_style(1.0, 0.0).1.color.r();
        assert!(r0 > r50 && r50 > r100, "红通道应随热度单调下降：{r0} / {r50} / {r100}");
    }

    /// APM 胶囊条宽度贴合内容：加项变宽、与手算一致、缩放跟着长（右侧不再空一块的回归）
    #[test]
    fn apm_hud_width_fits_content() {
        // 3 项各 40、分隔符 6：16 + 3×40 + 2×(12+6) = 172 → +4 余量 = 176
        assert_eq!(apm_hud_width(&[40.0, 40.0, 40.0], 6.0, 1.0), 176.0);
        // 4 项：16 + 4×40 + 3×18 = 230 → +4 = 234
        assert_eq!(apm_hud_width(&[40.0, 40.0, 40.0, 40.0], 6.0, 1.0), 234.0);
        // 单项：16 + 40 + 4 = 60
        assert_eq!(apm_hud_width(&[40.0], 0.0, 1.0), 60.0);
        // 200%：内边距 / 分隔 / 余量全部 ×2（分隔符宽度本身由实测得出，这里传定值）
        // → 32 + 3×40 + 2×(24+6) + 8 = 220
        assert_eq!(apm_hud_width(&[40.0, 40.0, 40.0], 6.0, 2.0), 220.0);
    }

    /// 限制在屏幕内：各方向越界都推回，窗口比屏幕大时贴 (0,0)
    #[test]
    fn clamp_hud_pos_keeps_window_in_screen() {
        let mon = (1920.0, 1080.0);
        let size = (200.0, 32.0);
        assert_eq!(clamp_hud_pos((500.0, 700.0), size, mon), (500.0, 700.0), "屏内不动");
        assert_eq!(clamp_hud_pos((-50.0, 200.0), size, mon), (0.0, 200.0), "左边越界推回");
        assert_eq!(clamp_hud_pos((1800.0, 200.0), size, mon), (1720.0, 200.0), "右边越界推回");
        assert_eq!(clamp_hud_pos((100.0, 1070.0), size, mon), (100.0, 1048.0), "下边越界推回");
        // 窗口两个方向都比屏幕大 → 贴 (0,0)；只宽不高时 Y 保持不动
        assert_eq!(clamp_hud_pos((10.0, 10.0), (2200.0, 2000.0), mon), (0.0, 0.0), "窗口比屏大贴左上");
        assert_eq!(clamp_hud_pos((10.0, 10.0), (2200.0, 40.0), mon), (0.0, 10.0), "只比屏宽时 Y 不用动");
    }
}
