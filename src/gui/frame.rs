//! 一帧的主循环流程 —— 从 gui.rs 的 draw 拆出。
//!
//! App::draw 现在只是一条编排线：
//! 同步状态 → 关窗 → 标题 → HUD 标志 → 窗口尺寸自适应 → 导航键 → 画界面
//! → 调试自截图 → 日历弹窗 → 复绘节拍。
//!
//! 顺序有实际意义，别随意调整：
//! - 尺寸自适应必须在 draw_ui 之前：它读的是上一帧的几何，本帧画完才知道内容高度；
//! - 日历弹窗必须最后画，才能浮在内容区之上；
//! - handle_close_request 返回 true 时要立刻 return，后面一步都不能跑。
//!
//! 这里定义的方法都由兄弟模块 gui.rs 的 draw 调用，所以统一 pub(crate)。

use super::*;

impl App {
    pub(crate) fn sync_config_and_flags(&mut self, ctx: &egui::Context) {
        // ---- 同步共享配置（托盘控制线程可能已修改，如开机启动）----
        self.cfg = self.cfg_shared.lock().clone();

        self.sync_main_hidden(ctx);
        self.run_startup_debug_hooks(ctx);
        self.apply_tray_requests();
        self.run_periodic_tasks();
    }

    /// 主窗口被「最小化」→ 记成已收起。
    ///
    /// 最小化 = unmap，会让 eframe 的事件循环停摆、两个 HUD 一起冻结（实测）。
    /// KWin 侧的常驻脚本会立刻撤销最小化并改成「停放」（透明 + 最底层，保持 mapped），
    /// 这里把状态同步过来，好让自适应逻辑别把窗口又撑回桌面（见 update_park_state）。
    fn sync_main_hidden(&mut self, ctx: &egui::Context) {
        #[cfg(target_os = "linux")]
        if crate::kwin::take_main_hidden() {
            self.shared.ui_hidden.store(true, Ordering::Relaxed);
            log::info!("主窗口被最小化：转为「停放」收起（保持 mapped，HUD 继续更新）");
            let _ = ctx;
        }
        #[cfg(not(target_os = "linux"))]
        let _ = ctx;
    }

    /// 启动期的调试钩子：启动后收起、定时切页（都由环境变量驱动）。
    fn run_startup_debug_hooks(&mut self, ctx: &egui::Context) {
        // 调试：启动后收起窗口（KMCOUNTER_START_PANELS 含 "min"；首帧执行，保证命令能被处理）
        if self.start_minimized && self.frame_count == 1 {
            self.start_minimized = false;
            log::info!("调试：启动后收起窗口");
            hide_window(&self.shared, ctx);
        }

        // 调试：定时切页
        if self.flags.switch_page_after.swap(false, Ordering::Relaxed) {
            self.page = if self.page == Page::Heatmap { Page::Trends } else { Page::Heatmap };
            log::info!("调试：切换到 {:?}", if self.page == Page::Heatmap { "热力图" } else { "趋势" });
        }
    }

    /// 托盘发来的请求：控制线程只置标志，UI 状态在渲染线程改。
    fn apply_tray_requests(&mut self) {
        // ---- 托盘请求：打开设置 ----
        if self.flags.want_settings.swap(false, Ordering::Relaxed) {
            self.edit = self.cfg.clone();
            self.more_open = true;
        }
        // ---- 托盘呼出窗口：视图回到「今日」（带小时也一起回到全天）----
        if self.flags.show_today.swap(false, Ordering::Relaxed) {
            self.view_idx = 1; // [0]=总计 [1]=今日
            self.hour_sel = None;
            self.title_dirty = true;
        }
    }

    /// 定时任务：跨夜滚动 / 清理 / 保存 / 快照刷新。
    ///
    /// 快照做了节流：带分时明细后 store 明显变大，避免每帧整份克隆。
    fn run_periodic_tasks(&mut self) {
        // ---- 定时任务：跨夜滚动 / 清理 / 保存 ----
        if self.shared.rollover_if_needed() {
            let days = self.cfg.history_days_clamped();
            self.shared.store.lock().purge(days);
            self.snap_today = self.shared.today.lock().clone();
            self.title_dirty = true;
        }
        if self.shared.dirty.load(Ordering::Relaxed) && self.last_save.elapsed() > Duration::from_secs(30) {
            self.save_all();
        }

        // ---- 快照刷新（节流：带分时明细后 store 明显变大，避免每帧整份克隆）----
        let today_changed = self.snap_today != *self.shared.today.lock();
        if (self.shared.dirty.load(Ordering::Relaxed) && self.last_snapshot.elapsed() > Duration::from_millis(400))
            || today_changed
        {
            self.refresh_snapshot();
            self.last_snapshot = Instant::now();
        }
    }

    /// 窗口关闭：按配置收起窗口（继续后台统计）或直接退出。
    ///
    /// 返回 true = 这次是真的要退出，调用方必须立刻 return。
    pub(crate) fn handle_close_request(&mut self, ctx: &egui::Context) -> bool {
        // ---- 窗口关闭：按配置收起窗口（继续后台统计）或直接退出 ----
        let close_requested = ctx.input(|i| i.viewport().close_requested());
        if close_requested && !self.exiting && self.tray_available && self.cfg.close_to_tray {
            if crate::session::session_ending_fresh() {
                // 注销/关机流程里的关闭请求：直接退出，别收起窗口把它卡住
                log::info!("会话正在结束，本次关闭按退出处理");
                self.exiting = true;
            } else {
                hide_window(&self.shared, ctx);
                crate::session::mark_close_cancelled();
                ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            }
        }
        if self.exiting {
            self.save_all();
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            return true;
        }
        false
    }

    /// 窗口标题：只在 title_dirty 时下发，避免每帧调 Win32 接口。
    pub(crate) fn update_title(&mut self, ctx: &egui::Context, s: &'static Strings) {
        // ---- 标题 ----
        if self.title_dirty {
            self.title_dirty = false;
            // 帮助页是独立页面：标题跟着变成「帮助」，不再显示日期（免得看着像还在数据页）
            let tail = if self.help_open {
                s.tab_help.to_string()
            } else {
                format!("{} - {}", s.view_date_prefix, self.view_label())
            };
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(format!(
                "{} v{} | {}",
                s.app_title,
                env!("CARGO_PKG_VERSION"),
                tail
            )));
        }
    }

    /// 托盘请求切换 HUD / APM / KPS，以及各悬浮窗（含它们自己的设置窗）的渲染。
    pub(crate) fn poll_hud_flags(&mut self, ctx: &egui::Context, s: &'static Strings) {

        // 托盘等跨线程请求切换 HUD / APM / KPS
        if self.flags.toggle_hud.swap(false, Ordering::Relaxed) || self.flags.toggle_apm_hud.swap(false, Ordering::Relaxed) {
            self.toggle_apm_hud();
        }
        if self.flags.toggle_kps_hud.swap(false, Ordering::Relaxed) {
            self.toggle_kps_hud();
        }

        // HUD 独立悬浮窗渲染（多视口，与主程序并存）
        if self.hud_apm_show {
            self.show_apm_hud(ctx, s);
        }
        if self.hud_kps_show {
            self.show_kps_hud(ctx, s);
        }
        // 悬浮窗各自的设置窗口（右键悬浮窗打开；也是独立视口，不会被胶囊条裁掉）
        if self.apm_cfg_open {
            self.show_apm_settings(ctx, s);
        }
        if self.kps_cfg_open {
            self.show_kps_settings(ctx, s);
        }
    }

    /// 窗口宽度与高度自适应：底部面板展开时向下加高，配列切换时重设宽度。
    ///
    /// 这里只做编排，每段判定各自独立成方法（顺序见下方注释，改之前先读懂为什么这么排）：
    /// 读实际尺寸 → 停放状态 → 贴合配列宽度 → 识别用户缩放 → 记 last_seen
    /// → 重算基准高度 → 算出目标高度 → 让宽度/高度目标生效并下发。
    pub(crate) fn adapt_window_size(&mut self, ctx: &egui::Context) {
        let (actual_w, actual_h) = Self::current_inner_size(ctx);
        let (parked, settling) = self.update_park_state(ctx, actual_w, actual_h);
        let want_w = self.auto_fit_width(parked, actual_w);
        self.detect_user_resize(parked, settling, actual_w, actual_h);
        if !parked && !settling {
            // 停放期间窗口被压成 32px，这个尺寸不是用户的选择 —— 记下来会让"用户手动缩放"
            // 的判定误触发（见上），也会让恢复时的基准高度失真；解除停放的静默期同理
            self.last_seen_h = actual_h;
            self.last_seen_w = actual_w;
        }
        self.recompute_base_height(parked, settling, actual_h);
        let desired_h = self.desired_height();
        self.request_height_if_changed(desired_h);
        // 快照两个待处理的 resize 目标：下面的 force_restore_height 会在"没有待处理 resize"
        // 时补发高度，执行完这两个字段就变了，所以必须先取快照，再交给 apply_pending_resize。
        let w_target = self.resize_target_w;
        let h_target = self.resize_target;
        self.force_restore_height(parked, actual_h, desired_h);
        self.apply_pending_resize(ctx, parked, actual_w, actual_h, want_w, w_target, h_target);
    }

    /// 主窗口可用的实际内容尺寸（视口未就绪时退回屏幕尺寸）。
    fn current_inner_size(ctx: &egui::Context) -> (f32, f32) {
        let scr_size = ctx.screen_rect().size();
        ctx.input(|i| {
            let r = i.viewport().inner_rect;
            (
                r.map(|r| r.width()).unwrap_or(scr_size.x),
                r.map(|r| r.height()).unwrap_or(scr_size.y),
            )
        })
    }

    /// 停放状态机：同步 is_parked、维护刚解除停放的静默期。
    ///
    /// 主窗口「停放」中（已收起到托盘）时几何变化都是我们自己压出来的，
    /// 下面所有自适应尺寸逻辑都得让路：不下发尺寸、不记 last_seen、不判定"用户手动缩放"
    ///（否则刚缩到 32px 就被拉回原尺寸，等于把窗口又摆回桌面）。细节见 gui::window。
    ///
    /// 返回 (parked, settling)；settling = 刚解除停放、窗口还没恢复尺寸的静默期内。
    fn update_park_state(&mut self, ctx: &egui::Context, actual_w: f32, actual_h: f32) -> (bool, bool) {
        let parked = window::is_parked(&self.shared);
        window::parked_tick(&self.shared, ctx, Vec2::new(actual_w, actual_h));
        // 刚解除停放：窗口还是停放时的 32px（WM 与 KWin 脚本还没恢复），
        // 这一小段时间里的几何变化不是用户的选择 —— 静默期内不判定「用户手动缩放」、不记 last_seen
        if self.was_parked && !parked {
            self.park_release_until = Some(Instant::now() + Duration::from_millis(1500));
        }
        self.was_parked = parked;
        let settling = self.park_release_until.map_or(false, |t| Instant::now() < t);
        if !settling {
            self.park_release_until = None;
        }
        (parked, settling)
    }

    /// 配列默认宽度：若未手动拉伸且无处理中 resize，自动贴合当前配列宽度（杜绝右侧留白）。
    fn auto_fit_width(&mut self, parked: bool, actual_w: f32) -> f32 {
        let want_w = default_width_for_layout(&self.cfg.layout.keyboard_layout, &self.cfg.layout);
        if !parked && !self.user_sized && self.resize_target_w.is_none() && (actual_w - want_w).abs() > 4.0 {
            self.resize_target_w = Some(want_w);
            self.resize_deadline = Instant::now() + Duration::from_secs(3);
        }
        want_w
    }

    /// 识别用户手动缩放：无待处理 resize 时宽度变了（或收起状态下高度变了）→ 尊重用户设定。
    fn detect_user_resize(&mut self, parked: bool, settling: bool, actual_w: f32, actual_h: f32) {
        if !parked && !settling && self.resize_target.is_none() && self.resize_target_w.is_none() && !self.user_sized {
            if self.last_seen_w > 100.0 && (actual_w - self.last_seen_w).abs() > 4.0 {
                self.user_sized = true;
                log::debug!("检测到用户手动调整窗口宽度，停止自动贴合");
            }
            if !self.more_open && self.last_seen_h > 100.0 && (actual_h - self.last_seen_h).abs() > 4.0 {
                self.user_sized = true;
                log::debug!("检测到用户手动调整窗口高度，停止自动贴合");
            }
        }
    }

    /// 重算基准高度 base_h（未含展开面板）。
    fn recompute_base_height(&mut self, parked: bool, settling: bool, actual_h: f32) {
        if self.user_sized {
            // 用户手动定过尺寸：收起时以实际高度为准
            if self.panel_extra_h() <= 0.0 && self.resize_target.is_none() && !self.shared.ui_hidden.load(Ordering::Relaxed) {
                if actual_h > 100.0 {
                    self.base_h = actual_h;
                }
            }
        } else if !parked && !settling && self.page == Page::Heatmap && self.content_bottom > 100.0 {
            // 热力图页内容尺寸固定：锚点直接由内容算出 → 按钮始终紧贴内容，无留白
            self.base_h = self.content_bottom + CONTROLS_H + 8.0;
        }
    }

    /// 本帧期望的窗口高度：展开面板时取「基准 + 面板实测」与「面板底 + 内边距」的较大者。
    ///
    /// 实测底部优先：字体/DPI/行高变化时也保证面板底边完整可见。
    fn desired_height(&self) -> f32 {
        if self.more_open {
            (self.base_h + self.panel_extra_h()).max(self.panel_bottom + PANEL_BOTTOM_PAD)
        } else {
            self.base_h
        }
    }

    /// 目标高度变了就登记一次 resize（3 秒内反复校验，见 apply_pending_resize）。
    fn request_height_if_changed(&mut self, desired_h: f32) {
        if (self.last_applied_h - desired_h).abs() > 0.5 {
            // 展开状态或实测高度变化：设定目标，按实际尺寸校验并重发
            self.last_applied_h = desired_h;
            self.resize_target = Some(desired_h);
            self.resize_deadline = Instant::now() + Duration::from_secs(3);
        }
    }

    /// 停放解除后窗口可能还停在 32px 高（自适应逻辑认为尺寸没变、不会下发）：强制重发一次。
    ///
    /// 否则恢复出来的是一条细长条。用「当前想要的高度」（内容高度或内容+面板），
    /// 只保底到窗口最小高度 —— 以前写的是 last_applied_h.max(BASE_H)，
    /// 会把窗口撑到 BASE_H(500)，比内容高出一大截，底部留白（呼出后"高度偏高"就是这么来的）。
    fn force_restore_height(&mut self, parked: bool, actual_h: f32, desired_h: f32) {
        if !parked && actual_h < 100.0 && self.resize_target.is_none() {
            self.resize_target = Some(desired_h.max(MAIN_MIN_H));
            self.last_applied_h = desired_h.max(MAIN_MIN_H);
            self.resize_deadline = Instant::now() + Duration::from_secs(3);
        }
    }

    /// 下发待处理的尺寸变更：尺寸已到位、或被卡住超时，就清除目标；否则每帧重发。
    fn apply_pending_resize(
        &mut self,
        ctx: &egui::Context,
        parked: bool,
        actual_w: f32,
        actual_h: f32,
        want_w: f32,
        w_target: Option<f32>,
        h_target: Option<f32>,
    ) {
        if (w_target.is_some() || h_target.is_some()) && !parked {
            let w_done = match w_target {
                Some(tw) => (actual_w - tw).abs() <= 3.0,
                None => true,
            };
            let h_done = match h_target {
                Some(th) => (actual_h - th).abs() <= 2.0,
                None => true,
            };
            if (w_done && h_done) || Instant::now() > self.resize_deadline {
                self.resize_target = None;
                self.resize_target_w = None;
            } else {
                let send_w = if self.user_sized {
                    w_target.unwrap_or(actual_w)
                } else {
                    w_target.unwrap_or(want_w)
                };
                let send_h = h_target.unwrap_or(actual_h);
                ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(Vec2::new(send_w, send_h)));
            }
        }
    }

    /// 切到「总计」时，排行页的区间自动变成「全部」（用户之后仍可自己改成当周/当月…）。
    pub(crate) fn reset_rank_range_on_total(&mut self) {
        if self.last_view_idx != 0 && self.view_idx == 0 {
            self.rank_range = RankRange::All;
        }
        self.last_view_idx = self.view_idx;
    }

    /// 历史翻页：翻页键 / 上下键 / 滚轮（仅统计窗口激活时）。
    pub(crate) fn handle_navigation_keys(&mut self, ctx: &egui::Context) {
        let focused = ctx.input(|i| i.focused);
        let (up, down) = ctx.input(|i| {
            (
                i.key_pressed(egui::Key::ArrowUp) || i.key_pressed(egui::Key::PageUp),
                i.key_pressed(egui::Key::ArrowDown) || i.key_pressed(egui::Key::PageDown),
            )
        });
        let wheel_y = ctx.input(|i| i.raw_scroll_delta.y);
        // 翻页键与滚轮都要求：窗口激活、不在帮助页、指针不在底部/榜单/日历上。
        // 否则整帧算作「没在翻页」，同时把滚轮累加值清零。
        if !(focused && !self.help_open && !self.pointer_blocks_navigation(ctx) && !self.cal_open) {
            self.wheel_delta_accum = 0.0;
            return;
        }
        if up {
            self.nav_step(true);
        }
        if down {
            self.nav_step(false);
        }
        self.handle_wheel_navigation(wheel_y);
    }

    /// 指针是否压在「不归翻页管」的区域上：底部按钮行 / 展开面板，或排行页的榜单。
    ///
    /// 注意必须加上「当前就在排行页」这个条件：rank_rect 是上一帧榜单的位置，
    /// 离开排行页后它还在原处（正好压着趋势图那块），会让趋势页的滚轮翻页整个失效。
    fn pointer_blocks_navigation(&self, ctx: &egui::Context) -> bool {
        let pos = ctx.input(|i| i.pointer.hover_pos());
        let pointer_in_rank_list =
            self.page == Page::Ranking && pos.map_or(false, |p| self.rank_rect.contains(p));
        let pointer_in_bottom = pos.map_or(false, |p| p.y >= self.bottom_region_top);
        pointer_in_rank_list || pointer_in_bottom
    }

    /// 滚轮翻页：累加到阈值才翻一页，方向反转立即清零，无输入时衰减。
    fn handle_wheel_navigation(&mut self, wheel_y: f32) {
        const WHEEL_THRESHOLD: f32 = 20.0;
        if wheel_y.abs() > 0.0 {
            // 方向反转时立即清空累加，避免反向迟滞
            if (self.wheel_delta_accum > 0.0 && wheel_y < 0.0)
                || (self.wheel_delta_accum < 0.0 && wheel_y > 0.0)
            {
                self.wheel_delta_accum = 0.0;
            }
            self.wheel_delta_accum += wheel_y;
            if self.wheel_delta_accum >= WHEEL_THRESHOLD {
                self.nav_step(true);
                self.wheel_delta_accum = 0.0;
            } else if self.wheel_delta_accum <= -WHEEL_THRESHOLD {
                self.nav_step(false);
                self.wheel_delta_accum = 0.0;
            }
        } else {
            // 无滚轮输入时衰减累加值，防止残留增量在下一次滚动时误触
            self.wheel_delta_accum *= 0.5;
            if self.wheel_delta_accum.abs() < 1.0 {
                self.wheel_delta_accum = 0.0;
            }
        }
    }

    /// 主界面骨架：页签行 + 当前页内容 + 底部按钮行 + 展开面板。
    ///
    /// 内容区一律画在 CentralPanel 里；展开面板的高度靠「窗口向下生长」容纳，
    /// 所以按钮行的位置与本帧窗口实际高度无关（锚在 base_h 上，见 draw_bottom_bar）。
    pub(crate) fn draw_ui(&mut self, ctx: &egui::Context, s: &'static Strings) {
        egui::CentralPanel::default()
            .frame(egui::Frame::central_panel(&ctx.style()).fill(BG))
            .show(ctx, |ui| {
                // 面板内容区起点（用于把底部区域锚定到 base_h，与当前窗口实际高度无关）
                let content_top = ui.cursor().min.y;
                // 本帧开始时的可用宽度：用于展开面板的左右分栏。
                // 注意不能用分栏处的 available_width——热力图页的固定尺寸内容会把它撑大，
                // 与趋势页不一致，导致设置栏左右移动
                let panel_area_w = ui.available_width();

                self.draw_tabs(ui, s);

                // ---- 右上角 "?" 打开的「悬浮窗设置说明」：一个独立页面 ----
                // show_chrome：底栏（展开更多）与展开面板只在普通页面画。
                // 不进帮助页时 content_bottom / bottom_region_top 也在普通页面里更新，
                // 所以窗口尺寸锚点保持进入前的值 —— 开关帮助页不会让窗口跳动。
                let show_chrome = !self.help_open;
                if self.help_open {
                    self.show_hud_help(ui, s, content_top);
                } else {
                    self.show_page_content(ui, s, content_top);
                }
                if show_chrome {
                    self.draw_bottom_bar(ui, s, content_top);
                }

                // ---- 按钮下方的展开面板（窗口向下生长，按钮位置不变）----
                if self.more_open && show_chrome {
                    self.draw_expand_panel(ui, s, panel_area_w);
                } else {
                    self.panel_bottom = 0.0;
                }
            });
    }

    /// 页签行：热力图 / 趋势 / 排行 + 右侧实时 APM 与 HUD 悬浮按钮。
    fn draw_tabs(&mut self, ui: &mut egui::Ui, s: &'static Strings) {
        ui.horizontal(|ui| {
            // 帮助页是**独立页面**（不是盖在别的页上的一层）：整排页签让位，
            // 只留右边的实时 APM 与 "?" —— 点 "?" 就是唯一的出口
            if !self.help_open {
                ui.selectable_value(&mut self.page, Page::Heatmap, s.tab_heatmap);
                ui.selectable_value(&mut self.page, Page::Trends, s.tab_trend);
                ui.selectable_value(&mut self.page, Page::Ranking, s.tab_rank);
            }

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                self.draw_help_toggle(ui, s);
                ui.add_space(6.0);
                self.draw_hud_toggles(ui, s);
                self.draw_apm_badge(ui, s);
            });
        });
    }

    /// 最右的 "?"：悬浮窗设置说明。它是一块「盖住内容区」的说明页，
    /// 所以用可选中样式显示开关状态，跟页签的观感一致。
    fn draw_help_toggle(&mut self, ui: &mut egui::Ui, s: &'static Strings) {
        if ui
            .selectable_label(self.help_open, egui::RichText::new("?").small().strong())
            .on_hover_text(s.help_btn_hint)
            .clicked()
        {
            self.toggle_help();
        }
    }

    /// KPS / APM 两个 HUD 开关按钮（显示中带 ✓）
    fn draw_hud_toggles(&mut self, ui: &mut egui::Ui, s: &'static Strings) {
        let kps_btn_text = if self.hud_kps_show {
            format!("🎯 {} ✓", s.kps_label)
        } else {
            format!("🎯 {}", s.kps_label)
        };
        if ui
            .button(egui::RichText::new(kps_btn_text).small())
            .on_hover_text(s.hud_kps_title)
            .clicked()
        {
            self.toggle_kps_hud();
        }

        let apm_btn_text = if self.hud_apm_show {
            format!("⚡ {} ✓", s.hud_mode_btn)
        } else {
            format!("⚡ {}", s.hud_mode_btn)
        };
        if ui
            .button(egui::RichText::new(apm_btn_text).small())
            .on_hover_text(s.hud_apm_title)
            .clicked()
        {
            self.toggle_apm_hud();
        }
    }

    /// 实时 APM 徽标（按 200 / 100 分档着色）
    fn draw_apm_badge(&mut self, ui: &mut egui::Ui, s: &'static Strings) {
        let apm_w = self.shared.apm_window_secs.load(Ordering::Relaxed);
        let (cur_apm, peak_apm) = self.shared.apm.lock().get(&self.snap_today, apm_w);
        let apm_color = if cur_apm >= 200 {
            Color32::from_rgb(0xFF, 0x55, 0x55)
        } else if cur_apm >= 100 {
            Color32::from_rgb(0xF5, 0xA6, 0x23)
        } else {
            Color32::from_rgb(0x99, 0x99, 0x99)
        };
        let apm_text = format!("⚡ {} APM ({} {})", cur_apm, s.apm_peak_label, peak_apm);
        ui.label(egui::RichText::new(apm_text).small().strong().color(apm_color))
            .on_hover_text(format!("{}: {} | {}: {}", s.apm_label, cur_apm, s.apm_peak_label, peak_apm));
    }

    /// 底部按钮行：先把内容区垫到 base_h，再画「展开更多 / 收起」。
    ///
    /// remaining 用的是 base_h 而不是实际窗口高度 —— 窗口加高时按钮不跟着往下漂。
    fn draw_bottom_bar(&mut self, ui: &mut egui::Ui, s: &'static Strings, content_top: f32) {
        let remaining = (content_top + self.base_h - CONTROLS_H - self.content_bottom).max(0.0);
        ui.add_space(remaining);
        self.bottom_region_top = ui.cursor().min.y;

        ui.horizontal(|ui| {
            let label = if self.more_open { s.more_collapse } else { s.more_label };
            // 固定宽度：避免“展开更多/收起”文字长度不同导致退出按钮左右移动
            if ui
                .add_sized(
                    [MORE_BTN_W, 20.0],
                    egui::SelectableLabel::new(self.more_open, egui::RichText::new(label)),
                )
                .clicked()
            {
                self.more_open = !self.more_open;
                if self.more_open {
                    self.edit = self.cfg.clone();
                }
            }
            // 「退出」已移进设置面板（放这儿容易误触）
        });
    }

    /// 按钮下方的展开面板：统计在左、设置在右。
    ///
    /// 宽度按窗口宽度合理分配：宽窗口（>=1200）给统计约 280px；
    /// 紧凑窗口（<1200）给统计约 210px，保证右侧设置面板有充足空间。
    /// 画完回填实测高度，下一帧据此调整窗口高度（见 desired_height）。
    fn draw_expand_panel(&mut self, ui: &mut egui::Ui, s: &'static Strings, panel_area_w: f32) {
        let before = ui.cursor().min.y;
        // 左右两栏：统计在左、设置在右，宽度按窗口宽度合理分配
        // 宽窗口（>=1200）给统计约 280px；紧凑窗口（<1200）给统计约 210px，保证右侧设置面板有充足空间
        let left_w = if panel_area_w >= 1200.0 {
            (panel_area_w * 0.38 - 4.0).clamp(330.0, 480.0)
        } else {
            (panel_area_w * 0.25).clamp(200.0, 240.0)
        };
        geo_log("panel area", Rect::from_min_size(Pos2::ZERO, Vec2::new(panel_area_w, 0.0)));
        let item_gap = ui.spacing().item_spacing.x;
        ui.horizontal_top(|ui| {
            let stats_resp = ui.allocate_ui(Vec2::new(left_w, 0.0), |ui| {
                self.show_stats_list(ui, s);
            });
            let actual_left_w = stats_resp.response.rect.width().max(left_w);
            let right_w = (panel_area_w - actual_left_w - item_gap).max(200.0);
            geo_log("right col alloc", Rect::from_min_size(Pos2::ZERO, Vec2::new(right_w, 0.0)));
            ui.allocate_ui(Vec2::new(right_w, 0.0), |ui| {
                self.show_settings_section(ui, s);
            });
        });
        let measured = ui.cursor().min.y - before;
        self.panel_bottom = ui.cursor().min.y;
        // 实测高度回填，下一帧按实际内容调整窗口高度
        if (self.measured_panel_h - measured).abs() > 1.0 {
            self.measured_panel_h = measured;
            self.last_applied_h = 0.0; // 强制下一帧重算窗口高度
        }
    }

    /// 调试自截图：不依赖合成器，直接取 egui 帧缓冲。
    pub(crate) fn debug_take_screenshots(&mut self, ctx: &egui::Context) {
        if let Some(prefix) = self.screenshot_prefix.clone() {
            // 等窗口尺寸稳定后连拍若干张
            if matches!(self.frame_count, 6 | 12 | 20) && self.shots_taken < 3 {
                ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot);
            }
            let shot = ctx.input(|i| {
                i.events.iter().find_map(|e| match e {
                    egui::Event::Screenshot { image, .. } => Some(image.clone()),
                    _ => None,
                })
            });
            if let Some(img) = shot {
                self.shots_taken += 1;
                let path = format!("{prefix}-{}.ppm", self.shots_taken);
                match save_ppm(&path, &img) {
                    Ok(_) => log::info!("已保存截图 {path} ({}x{})", img.size[0], img.size[1]),
                    Err(e) => log::warn!("截图保存失败: {e}"),
                }
            }
        }
    }

    /// 调试布局日志：KMCOUNTER_LAYOUT_LOG 设了才打，每 6 帧一次。
    pub(crate) fn debug_layout_log(&mut self, ctx: &egui::Context) {
        if std::env::var("KMCOUNTER_LAYOUT_LOG").is_ok() && self.frame_count % 6 == 0 {
            log::info!(
                "LAYOUT|page={:?}|base_h={:.1}|content_bottom={:.1}|bottom_top={:.1}|desired_h={:.1}|actual_h={:?}|panel_extra={:.1}",
                if self.page == Page::Heatmap { "heat" } else { "trend" },
                self.base_h,
                self.content_bottom,
                self.bottom_region_top,
                self.base_h + self.panel_extra_h(),
                ctx.input(|i| i.viewport().inner_rect.map(|r| r.height())),
                self.panel_extra_h()
            );
        }
    }
}
