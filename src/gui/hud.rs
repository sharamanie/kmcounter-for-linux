//! KPS / APM 悬浮窗（HUD）与它们各自的设置窗口 —— 从 `gui.rs` 拆出（用户第二十四轮）。
//!
//! 这里放：悬浮窗度量（`kps_hud_metrics`）、键格与 KPS 图绘制、`show_*_hud`、
//! `show_*_settings`、键位捕获等。`use super::*` 直接拿 `gui` 的类型、常量与私有方法
//! （Rust 里子模块能看见父模块的私有项，`App` 的私有字段也能直接用）。

use super::*;

/// 悬浮窗 / 其设置窗口（深色底）上的文字色：白色保证可读性（主面板浅色底仍用 `TEXT`）
pub(crate) const HUD_TEXT: Color32 = Color32::WHITE;

/// HUD 独立视口 → 主窗口的事件（视口回调里推入，主窗口恢复后 drain 处理）
#[derive(Debug, Clone, Copy)]
pub(crate) enum HudEvent {
    /// 视口请求关闭（Alt+F4 / 合成器 close_requested）
    Close,
    /// 右键：打开本 HUD 自己的设置窗口（设置窗口挂在主视口下，需要叫回主窗口）
    OpenSettings,
}

/// HUD 视口本帧看到的位置 / 显示器尺寸（位置记忆用；回调回写，主窗口读取）
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct HudSeen {
    pub pos: Option<(f32, f32)>,
    pub monitor: Option<(f32, f32)>,
}

/// HUD 独立视口的共享数据。
///
/// 为什么需要它：HUD 用的是 **deferred 视口**（回调持久存储、视口自己渲染）。
/// 主窗口最小化后 Wayland 不再给它发帧回调、主 update() 停摆，但 HUD 要照常渲染 ——
/// 渲染所需的一切都得走这条共享通道：主窗口每帧刷新快照（cfg / 语言 / 日期），
/// 实时统计本来就是 Arc；回调把事件与看到的位置回写，主窗口恢复后 drain。
/// （旧实现用 immediate 视口：UI 闭包必须由父视口 update() 每次提供，
/// 主窗口一最小化 HUD 就断粮冻结 —— 这就是「最小化后两个 HUD 都冻结」的根因。）
#[derive(Clone)]
pub(crate) struct HudShared {
    /// 实时统计（按键 / 点击 / 距离 / KPS 跟踪器）
    pub shared: Arc<Shared>,
    /// 配置快照（只读；改动总在主窗口侧发生，恢复后自然同步回来）
    pub cfg: Config,
    /// 今天的日期键（统计查询用）
    pub today: String,
    /// 界面语言（HUD 上的文字）
    pub lang: Lang,
    /// HUD → 主窗口事件
    pub events: Arc<parking_lot::Mutex<Vec<HudEvent>>>,
    /// 本帧看到的位置 / 显示器尺寸（位置记忆用）
    pub seen: Arc<parking_lot::Mutex<HudSeen>>,
    /// 最近一次下发的窗口尺寸（避免每帧重复发 InnerSize）
    pub last_size: Arc<parking_lot::Mutex<(f32, f32)>>,
    /// 主窗口置顶标记（双击 / 右键叫回主窗口时沿用）
    pub keep_on_top: bool,
}

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
        self.push_hud_state();
    }

    #[allow(dead_code)]
    pub fn exit_hud(&mut self, _ctx: &egui::Context) {
        self.hud_apm_show = false;
        self.cfg.hud_apm_show = false;
        let mut cfg = self.cfg_shared.lock();
        cfg.hud_apm_show = false;
        let _ = cfg.save(&self.cfg_path);
        self.push_hud_state();
    }

    pub fn toggle_apm_hud(&mut self) {
        self.hud_apm_show = !self.hud_apm_show;
        // 留痕：悬浮窗开关有 3 个入口（托盘 / 主面板按钮 / HUD 设置窗），出问题时能查是谁关的
        log::info!("APM 悬浮窗 → {}", if self.hud_apm_show { "开" } else { "关" });
        self.cfg.hud_apm_show = self.hud_apm_show;
        let mut cfg = self.cfg_shared.lock();
        cfg.hud_apm_show = self.hud_apm_show;
        let _ = cfg.save(&self.cfg_path);
        // 窗口关了：下次打开要重新恢复一次位置
        self.apm_pos = HudPosState::default();
        // 托盘菜单的 ✔ 跟着走（托盘点击 / 主面板按钮 / 设置保存三个入口都在这里汇合）
        self.push_hud_state();
    }

    pub fn toggle_kps_hud(&mut self) {
        self.hud_kps_show = !self.hud_kps_show;
        log::info!("KPS 悬浮窗 → {}", if self.hud_kps_show { "开" } else { "关" });
        self.cfg.hud_kps_show = self.hud_kps_show;
        let mut cfg = self.cfg_shared.lock();
        cfg.hud_kps_show = self.hud_kps_show;
        let _ = cfg.save(&self.cfg_path);
        // 窗口关了：下次打开要重新恢复一次位置
        self.kps_pos = HudPosState::default();
        // 托盘菜单的 ✔ 跟着走
        self.push_hud_state();
    }

    /// 把 HUD 开关状态回推给托盘菜单（APM / KPS 两项的 ✔ 勾选）
    pub(crate) fn push_hud_state(&self) {
        if let Some(tx) = &self.tray_tx {
            let _ = tx.send(crate::tray::TrayCmd::HudState {
                apm: self.hud_apm_show,
                kps: self.hud_kps_show,
            });
        }
    }

    pub(crate) fn show_apm_hud(&mut self, ctx: &egui::Context, _s: &'static Strings) {
        // 先 drain 视口回传的事件（最小化期间堆积的也一并处理），可能把 hud_apm_show 关掉
        self.drain_hud_events(ctx, true);
        if !self.hud_apm_show {
            return;
        }

        let vp_id = egui::ViewportId::from_hash_of("kmc_apm_hud");
        let hs = HudShared {
            shared: self.shared.clone(),
            cfg: self.cfg.clone(),
            today: self.snap_today.clone(),
            lang: self.lang,
            events: self.hud_apm_events.clone(),
            seen: self.hud_apm_seen.clone(),
            last_size: self.apm_hud_size.clone(),
            keep_on_top: self.cfg.keep_on_top,
        };
        let size = self.apm_hud_initial_size();
        // **deferred 视口**：回调持久存储、视口自己渲染 —— 主窗口最小化（update 停摆）后
        // HUD 照常更新。immediate 视口只能由父视口驱动渲染，父窗口一最小化 HUD 就冻结（踩过）。
        ctx.show_viewport_deferred(
            vp_id,
            hud_viewport_builder("KMC - APM", size),
            move |hud_ctx, _class| render_apm_hud(hud_ctx, &hs),
        );

        // 位置三件事：seen 由回调回写（KDE/Wayland 下 winit 拿不到位置，tick 内部会回退 KWin 上报）
        let seen = *self.hud_apm_seen.lock();
        let size = *self.apm_hud_size.lock();
        let keep_in_screen = self.cfg.apm_keep_in_screen;
        self.tick_hud_pos(ctx, vp_id, true, seen.pos, seen.monitor, size, keep_in_screen);
    }

    /// APM 悬浮窗的初始尺寸：上次实测的优先，没有就按缩放推一个。
    fn apm_hud_initial_size(&self) -> (f32, f32) {
        let init = *self.apm_hud_size.lock();
        let sc0 = apm_hud_scale(&self.cfg);
        if init.0 > 1.0 {
            init
        } else {
            (360.0 * sc0, 32.0 * sc0)
        }
    }

    /// 处理 HUD 独立视口回传的事件（主窗口最小化期间也可能堆积，恢复后第一帧处理）。
    fn drain_hud_events(&mut self, ctx: &egui::Context, is_apm: bool) {
        let queue = if is_apm {
            self.hud_apm_events.clone()
        } else {
            self.hud_kps_events.clone()
        };
        let events: Vec<HudEvent> = std::mem::take(&mut *queue.lock());
        for ev in events {
            match ev {
                HudEvent::Close => self.on_hud_closed(is_apm),
                HudEvent::OpenSettings => self.on_hud_open_settings(ctx, is_apm),
            }
        }
    }

    /// 用户点了悬浮窗的关闭：退出流程里子视口也会收到关闭请求 —— 那不是"用户关了悬浮窗"，
    /// 照着关会把开关写进配置，下次启动两个 HUD 就"自己消失"了。
    fn on_hud_closed(&mut self, is_apm: bool) {
        if self.exiting {
            return;
        }
        if is_apm {
            self.toggle_apm_hud();
        } else {
            self.toggle_kps_hud();
        }
    }

    /// 悬浮窗右键 → 打开对应的设置窗。
    /// 设置窗口是主视口的子视口：主窗口的画循环在跑它才画得出来。
    /// - Wayland：主窗口「收起」= 停放（仍 mapped，循环照跑）→ **不叫回主窗口**，
    ///   右键只开设置窗（主人明确要求）；
    /// - X11 / Windows：窗口是真隐藏，渲染循环停摆，设置窗压根画不出来 ——
    ///   只能先把主窗口显示出来（这两个平台保持原行为）。
    fn on_hud_open_settings(&mut self, ctx: &egui::Context, is_apm: bool) {
        if is_apm {
            self.apm_cfg_open = true;
        } else {
            self.kps_cfg_open = true;
        }
        if !is_wayland() {
            show_window_now(&self.shared, ctx, self.cfg.keep_on_top);
        }
    }

    pub(crate) fn show_kps_hud(&mut self, ctx: &egui::Context, _s: &'static Strings) {
        // 先 drain 视口回传的事件（最小化期间堆积的也一并处理），可能把 hud_kps_show 关掉
        self.drain_hud_events(ctx, false);
        if !self.hud_kps_show {
            return;
        }

        let vp_id = egui::ViewportId::from_hash_of("kmc_kps_hud");
        let layout = self.cfg.kps_key_layout();
        let m = kps_hud_metrics(&self.cfg, &layout);
        let hs = HudShared {
            shared: self.shared.clone(),
            cfg: self.cfg.clone(),
            today: self.snap_today.clone(),
            lang: self.lang,
            events: self.hud_kps_events.clone(),
            seen: self.hud_kps_seen.clone(),
            last_size: self.kps_hud_size.clone(),
            keep_on_top: self.cfg.keep_on_top,
        };
        // **deferred 视口**（原因见 APM：immediate 视口在主窗口最小化后断粮冻结）
        ctx.show_viewport_deferred(
            vp_id,
            hud_viewport_builder("KMC - KPS", (m.w, m.h)),
            move |hud_ctx, _class| render_kps_hud(hud_ctx, &hs),
        );

        // 位置三件事：seen 由回调回写
        let seen = *self.hud_kps_seen.lock();
        let size = *self.kps_hud_size.lock();
        let keep_in_screen = self.cfg.kps_keep_in_screen;
        self.tick_hud_pos(ctx, vp_id, false, seen.pos, seen.monitor, size, keep_in_screen);
    }
}

/// KPS 悬浮窗的实际渲染（deferred 视口回调；主窗口最小化时也照常被调用）。
fn render_kps_hud(ctx: &egui::Context, hs: &HudShared) {
    log_hud_alive("KPS");
    let s = strings(hs.lang);
    let cfg = &hs.cfg;
    let layout = cfg.kps_key_layout();
    let keys_list = layout.names(); // 键名走布局解析：去掉宽度前缀 / 空位 / 换行
    let kps_w = hs.shared.kps_window_ms.load(Ordering::Relaxed);
    let m = kps_hud_metrics(cfg, &layout);
    let kd = kps_hud_data(hs, cfg, &keys_list, kps_w);

    // 尺寸变化时下发命令。**必须连 min/max 一起改**：with_resizable(false) 在 winit 里会把
    // min==max 锁成初始尺寸，只发 InnerSize 会被这个约束挡住（"每行键数"改了窗口不长、第二行看不见）。
    hud_apply_size(ctx, &hs.last_size, (m.w, m.h));

    // 本帧看到的位置 / 显示器尺寸（回写给主窗口：位置记忆 / 限制在屏幕内用）
    hud_report_seen(ctx, &hs.seen);

    if hud_close_requested(ctx, &hs.events) {
        return;
    }
    // 开了键雨 / 按键弹跳就按 ~60fps 重绘，全静态时退回 25fps ——
    // 动画只在有东西动的时候才烧帧
    let anim_ms = if cfg.kps_show_rain || cfg.kps_key_bounce { 16 } else { 40 };
    ctx.request_repaint_after(Duration::from_millis(anim_ms));

    // 背景与外框都跟随 kps_hud_opacity：调到 0 时整块底板与边框消失，
    // 只剩键格 / 键雨 / 数据胶囊（用户拍板：边框也跟着走，数据胶囊的边框除外）
    let bg_fill = hud_bg_color((16, 18, 24), cfg.kps_hud_opacity);
    let border_alpha = ((cfg.kps_hud_opacity.min(100) as u32 * 220) / 100) as u8;
    let border_stroke = egui::Stroke::new(1.0_f32, Color32::from_rgba_unmultiplied(60, 68, 86, border_alpha));

    egui::CentralPanel::default()
        .frame(
            egui::Frame::none()
                .fill(bg_fill)
                .inner_margin(egui::Margin::symmetric(m.pad, m.pad))
                .stroke(border_stroke)
                .rounding(6.0),
        )
        .show(ctx, |ui| {
            let content = ui.max_rect();
            hud_background_gestures(ui, ctx, content, hs, "kps_hud_drag", s.hud_drag_hint);

            // ---- 最上方：键雨（雨条从键格顶边往上飘）----
            // body = content 去掉雨区之后的剩余部分；下面的键格 / 数据 / 图都按 body 定位
            let body = if m.rain_h > 0.0 {
                let rain_area = Rect::from_min_max(
                    content.min,
                    Pos2::new(content.right(), content.top() + m.rain_h),
                );
                super::rain::paint_kps_rain(ui, rain_area, &layout, &m, &kd, cfg);
                Rect::from_min_max(
                    Pos2::new(content.left(), content.top() + m.rain_h),
                    content.max,
                )
            } else {
                content
            };

            // ---- 键格（按布局分行 / 空位 / 宽度倍数；**每行缝隙等宽、左右对称**）----
            if cfg.kps_show_buttons {
                paint_kps_key_cells(ui, body, &layout, &m, &kd, cfg, s);
            }

            // 缩放系数（度量里所有长度都按它算，这里用来算小间隙）
            let sc = m.pad / 6.0;
            let keys_h = if cfg.kps_show_buttons {
                m.rows as f32 * m.box_h + (m.rows as f32 - 1.0) * m.gap
            } else {
                0.0
            };

            // ---- 第二栏：数据行（当前 KPS · Max · 平均 + 分组 KPS），画在图的**上方** ----
            if m.meter_w > 0.0 {
                paint_kps_meter(ui, body, &m, &kd, cfg, s, sc, keys_h);
            }

            // ---- 第三栏：整宽的 KPS 图 ----
            if m.graph_h > 0.0 {
                paint_kps_graph_area(ui, body, &m, &kd, hs, s, keys_h, cfg.kps_graph_secs);
            }
        });
}

/// KPS 悬浮窗每帧要读的一次性数据（当前 / 峰值 / 平均 / 算法 + 每个键格的实时信息）。
pub(crate) struct KpsHudData {
    pub(crate) cur_kps: f64,
    pub(crate) peak_kps: f64,
    pub(crate) avg_kps: f64,
    pub(crate) method: crate::stats::KpsMethod,
    pub(crate) key_infos: Vec<crate::stats::KpsKeyInfo>,
    /// 每个键最近的按压区间（按下距今秒, 松开距今秒；松开=0 表示还按着）。
    /// 键雨按它纯几何重建整条串（见 gui::rain）
    pub(crate) rain: Vec<Vec<(f64, f64)>>,
    /// 分组 KPS（配置里没写分组 / 没开显示时为空）
    pub(crate) groups: Vec<crate::stats::KpsGroupInfo>,
    pub(crate) color: Color32,
}

/// 取当前 KPS 数据并算好强调色（≥15 红 / ≥8 橙 / 其余绿）。
fn kps_hud_data(hs: &HudShared, cfg: &Config, keys_list: &[String], kps_w: u32) -> KpsHudData {
    let (cur_kps, peak_kps, avg_kps, method, key_infos, rain, groups) = {
        let mut kps = hs.shared.kps.lock();
        let (cur, peak) = kps.get(&hs.today, kps_w);
        let infos = kps.query_keys(keys_list, kps_w);
        let method = kps.method();
        let avg = if cfg.kps_show_avg { kps.avg() } else { 0.0 };
        // 键雨：只取窗口内的按压区间（纯读，不存粒子状态）
        let rain = if cfg.kps_show_rain {
            let span = std::time::Duration::from_secs_f32(super::rain::rain_lifespan_secs(cfg));
            kps.rain_spans(keys_list, span, std::time::Instant::now())
        } else {
            Vec::new()
        };
        // 分组 KPS：配置里写了分组且开了显示才算
        let groups = if cfg.kps_show_groups {
            let defs = crate::stats::parse_kps_groups(&cfg.kps_groups);
            kps.query_groups(&defs, kps_w)
        } else {
            Vec::new()
        };
        // KPS 图：过去 300 秒、每秒一格（网格对齐整秒，值从按键记录反推 —— 见 stats::history_series）。
        // 数据每整秒才变一次，所以柱子是"一格一格往左跳"，同一节奏下高度稳定。
        (cur, peak, avg, method, infos, rain, groups)
    };
    let color = if cur_kps >= 15.0 {
        Color32::from_rgb(0xFF, 0x47, 0x57)
    } else if cur_kps >= 8.0 {
        Color32::from_rgb(0xFF, 0xA5, 0x02)
    } else {
        Color32::from_rgb(0x2E, 0xD5, 0x73)
    };
    KpsHudData {
        cur_kps,
        peak_kps,
        avg_kps,
        method,
        key_infos,
        color,
        rain,
        groups,
    }
}

fn paint_kps_key_cells(
    ui: &mut egui::Ui,
    content: Rect,
    layout: &crate::config::KpsKeyLayout,
    m: &KpsHudMetrics,
    kd: &KpsHudData,
    cfg: &Config,
    s: &'static Strings,
) {
    let row_pos = key_rows_layout(&layout.rows, m.box_w, m.graph_w);
    let mut info_idx = 0usize;
    for (row, cells) in layout.rows.iter().enumerate() {
        for (cell_idx, cell) in cells.iter().enumerate() {
            let Some(&(x_off, box_w)) = row_pos.get(row).and_then(|r| r.get(cell_idx)) else {
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
            let Some(k) = kd.key_infos.get(info_idx) else {
                break;
            };
            info_idx += 1;
            paint_kps_key_cell(ui, rect, m, kd, k, cfg, s, info_idx - 1);
        }
    }
}

/// 单个键格：热力底色 + 键名 + 副标题（次数 / 该键 KPS）+ 悬停提示。
#[allow(clippy::too_many_arguments)]
fn paint_kps_key_cell(
    ui: &mut egui::Ui,
    rect: Rect,
    m: &KpsHudMetrics,
    kd: &KpsHudData,
    k: &crate::stats::KpsKeyInfo,
    cfg: &Config,
    s: &'static Strings,
    idx: usize,
) {
    // 颜色对齐 osu-kps：heat（该键 KPS 的 2→4 插值）管边框色相，
    // 降速时沿原路缓慢退回；按住期间底板保持满亮（长按点亮），
    // 松开后由 glow 沿 50→250ms 曲线淡出
    let press = if k.held { 1.0_f32 } else { k.glow };
    // 按键弹跳（对齐 JipperKeyViewer 的 KeyShapeLayer）：按住时键格轻微放大，
    // 松开后随 glow 沿 50→250ms 淡出、同步回落到原尺寸。
    // 交互矩形仍用原 rect —— 放大时命中区域不能跟着跳，否则指针停在边上会闪。
    let bounce = if cfg.kps_key_bounce { 0.075 * press } else { 0.0 };
    let draw_rect = if bounce > 0.0 {
        Rect::from_center_size(rect.center(), rect.size() * (1.0 + bounce))
    } else {
        rect
    };
    let (box_bg, box_border, overlay) = key_heat_style(crate::stats::kps_key_heat(k.kps), press);
    ui.painter().rect(draw_rect, 4.0, box_bg, box_border);
    if let Some(ovl) = overlay {
        ui.painter().rect_filled(draw_rect, 4.0, ovl);
    }
    ui.painter().text(
        Pos2::new(draw_rect.center().x, draw_rect.min.y + m.box_h * 0.28),
        egui::Align2::CENTER_CENTER,
        &k.name,
        egui::FontId::proportional(m.font_key),
        HUD_TEXT,
    );
    let sub = kps_key_sub_text(k, cfg, kd.method);
    if !sub.is_empty() {
        ui.painter().text(
            Pos2::new(draw_rect.center().x, draw_rect.max.y - m.box_h * 0.28),
            egui::Align2::CENTER_CENTER,
            sub,
            egui::FontId::proportional(m.font_sub),
            HUD_TEXT,
        );
    }
    let resp = ui.interact(rect, ui.id().with(("kps_key", idx)), Sense::hover());
    resp.on_hover_text(format!(
        "{}: {} · {} {}",
        k.name,
        k.hits,
        fmt_kps(k.kps, kd.method),
        s.kps_label
    ));
}

/// HUD 设置窗里的一个深底白字复选框：改了就把动作位标脏。
fn hud_check(ui: &mut egui::Ui, value: &mut bool, label: &'static str, act: &mut HudCfgActions) {
    if ui
        .checkbox(value, egui::RichText::new(label).color(HUD_TEXT))
        .changed()
    {
        act.dirty = true;
    }
}

/// 设置窗口里的小节标题：左侧小字标题 + 一条淡分隔线向右延伸。
///
/// 加它是为了收拾"整面墙都是勾选框和数值框"的观感：把内容切成 基础 / 按键 /
/// 显示 / 键雨 四组，一眼能看出每一项属于哪一类（用户反馈过窗口太乱）。
fn settings_section(ui: &mut egui::Ui, title: &str) {
    ui.add_space(6.0);
    let (row, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 14.0), Sense::hover());
    let color = Color32::from_rgb(0x8F, 0x9C, 0xB4);
    let galley = ui.painter().layout_no_wrap(
        title.to_string(),
        egui::FontId::proportional(11.0),
        color,
    );
    let w = galley.size().x;
    ui.painter().galley(
        Pos2::new(row.left(), row.center().y - galley.size().y * 0.5),
        galley,
        color,
    );
    let y = row.center().y;
    ui.painter().line_segment(
        [Pos2::new(row.left() + w + 8.0, y), Pos2::new(row.right(), y)],
        egui::Stroke::new(1.0_f32, Color32::from_rgba_unmultiplied(70, 78, 96, 140)),
    );
}

/// KPS 算法的显示名。
fn kps_method_label(m: crate::stats::KpsMethod, s: &'static Strings) -> &'static str {
    match m {
        crate::stats::KpsMethod::Hard => s.kps_method_hard,
        crate::stats::KpsMethod::Sensitive => s.kps_method_sensitive,
    }
}

/// KPS 算法的说明文字（悬停提示）。
fn kps_method_hint(m: crate::stats::KpsMethod, s: &'static Strings) -> &'static str {
    match m {
        crate::stats::KpsMethod::Hard => s.kps_method_hard_hint,
        crate::stats::KpsMethod::Sensitive => s.kps_method_sensitive_hint,
    }
}

/// 键格下方的小字：次数与当前 KPS，按设置里的两个开关组合。
fn kps_key_sub_text(k: &crate::stats::KpsKeyInfo, cfg: &Config, method: crate::stats::KpsMethod) -> String {
    match (cfg.kps_show_hits, cfg.kps_show_key_kps) {
        (true, true) => format!("{} · {}", format_compact_num(k.hits), fmt_kps(k.kps, method)),
        (true, false) => format_compact_num(k.hits),
        (false, true) => fmt_kps(k.kps, method),
        (false, false) => String::new(),
    }
}

/// 数据行（v2）：当前 KPS · Max · 平均做成**一个独立胶囊**（深底圆角 + 自带描边），
/// 与主面板背景解耦 —— 主面板透明度调到 0 时数据仍完整可读（用户拍板的设计）。
/// 胶囊边框**不**跟随 kps_hud_opacity（对齐 JipperKeyViewer：胶囊自成一体）。
#[allow(clippy::too_many_arguments)]
fn paint_kps_meter(
    ui: &mut egui::Ui,
    content: Rect,
    m: &KpsHudMetrics,
    kd: &KpsHudData,
    cfg: &Config,
    s: &'static Strings,
    sc: f32,
    keys_h: f32,
) {
    let row_top = content.top()
        + keys_h
        + if keys_h > 0.0 { m.pad } else { 0.0 };
    let pill_h = m.top_h;
    if pill_h <= 0.0 {
        return;
    }
    // 胶囊内文字（横向排）：当前 KPS（大字强调色）+ 可选 Max + 可选平均
    let mut items: Vec<(String, f32, Color32)> = Vec::new();
    items.push((
        fmt_kps(kd.cur_kps, kd.method),
        m.font_cur,
        kd.color,
    ));
    items.push((s.kps_label.to_string(), m.font_sub, HUD_TEXT));
    if cfg.kps_show_max {
        items.push((
            format!("{}: {}", s.kps_max_label, fmt_kps(kd.peak_kps, kd.method)),
            m.font_sub,
            HUD_TEXT,
        ));
    }
    if cfg.kps_show_avg {
        items.push((
            format!("{}: {}", s.avg_label, fmt_kps(kd.avg_kps, kd.method)),
            m.font_sub,
            HUD_TEXT,
        ));
    }
    // 实测总宽（含间隔）
    let mut texts = Vec::new();
    let mut total_w = 4.0 * sc;
    for (text, size, color) in &items {
        let galley = ui.painter().layout_no_wrap(
            text.clone(),
            egui::FontId::proportional(*size),
            *color,
        );
        total_w += galley.size().x + 4.0 * sc;
        texts.push(galley);
    }
    total_w += 4.0 * sc;
    let pill = egui::Frame::none()
        .fill(hud_bg_color((22, 24, 32), 96))
        .inner_margin(egui::Margin::symmetric(6.0 * sc, 2.0 * sc))
        .stroke(egui::Stroke::new(
            1.0_f32,
            Color32::from_rgba_unmultiplied(60, 68, 86, 220),
        ))
        .rounding(6.0);
    // 居中放在行的可用宽度里（窗口比胶囊宽时左右留白对称）
    let avail_w = content.width();
    let pill_w = total_w + 12.0 * sc;
    let px = if avail_w > pill_w {
        content.left() + (avail_w - pill_w) * 0.5
    } else {
        content.left()
    };
    let pill_rect = Rect::from_min_size(Pos2::new(px, row_top), Vec2::new(pill_w, pill_h));
    // 先画胶囊底，再把文字排在里面（居中一行）
    ui.painter().add(pill.paint(pill_rect));
    let mut x = px + 4.0 * sc;
    let cy = row_top + pill_h * 0.5;
    for galley in &texts {
        ui.painter().galley(
            Pos2::new(x, cy - galley.size().y * 0.5),
            galley.clone(),
            HUD_TEXT,
        );
        x += galley.size().x + 4.0 * sc;
    }
    // 分组 KPS 行：胶囊下面一行，每个组一段「名字 值 KPS」
    if m.group_h > 0.0 && !kd.groups.is_empty() {
        paint_kps_group_row(ui, content, m, kd, s, sc, keys_h + pill_h);
    }
}

/// 下半区：整宽 KPS 图（柱数按实际像素宽推，每根柱 = 一个时间片的平均 KPS）。
/// 分组 KPS 行：每个组一段「名字 值 KPS」，超出宽度就截断。
/// 参照 JipperKeyViewer 的 GroupId：只看某几个键的合计 KPS（左手 / 右手、主键区…）。
fn paint_kps_group_row(
    ui: &mut egui::Ui,
    content: Rect,
    m: &KpsHudMetrics,
    kd: &KpsHudData,
    s: &'static Strings,
    sc: f32,
    keys_h: f32,
) {
    let row_top = content.top()
        + keys_h
        + if keys_h > 0.0 { m.pad } else { 0.0 }
        + m.top_h
        + m.pad;
    let mut x = content.left();
    let cy = row_top + m.font_sub * 0.5;
    for (i, g) in kd.groups.iter().enumerate() {
        let label = format!("{} {} {} ({})", g.name, fmt_kps(g.kps, kd.method), s.kps_label, g.hits);
        let galley = ui.painter().layout_no_wrap(
            label,
            egui::FontId::proportional(m.font_sub),
            HUD_TEXT,
        );
        let w = galley.size().x;
        // 放不下就不再往右排（组很多时按宽度截断，纵向高度固定为一行）
        if x + w > content.right() && i > 0 {
            break;
        }
        ui.painter().galley(
            Pos2::new(x, cy - galley.size().y * 0.5),
            galley,
            if i == 0 { kd.color } else { HUD_TEXT },
        );
        x += w + 12.0 * sc;
    }
}

fn paint_kps_graph_area(
    ui: &mut egui::Ui,
    content: Rect,
    m: &KpsHudMetrics,
    kd: &KpsHudData,
    hs: &HudShared,
    s: &'static Strings,
    keys_h: f32,
    graph_secs: u32,
) {
    let span_secs = graph_secs.clamp(10, 300);
    let graph_top = content.top()
        + keys_h
        + if keys_h > 0.0 { m.pad } else { 0.0 }
        + if m.meter_w > 0.0 { m.top_h + m.pad } else { 0.0 }
        + m.group_h;
    let rect = Rect::from_min_max(
        Pos2::new(content.left(), graph_top),
        Pos2::new(content.right(), content.bottom()),
    );
    // 柱数由图的实际像素宽决定（柱宽固定 GRAPH_BAR_W 像素），
    // 每根柱 = 时间片（时长/柱数 秒）内的平均 KPS —— 片内图形完全静止
    let mut kps_g = hs.shared.kps.lock();
    let bars = ((rect.width() / GRAPH_BAR_W).floor() as usize).max(1);
    let hist: Vec<f32> = kps_g
        .history_bars(Instant::now(), span_secs, bars)
        .into_iter()
        .map(|v| v as f32)
        .collect();
    let graph_max = hist.iter().copied().fold(0.0f32, f32::max);
    paint_kps_graph(ui, rect, &hist, kd.avg_kps as f32, kd.color);
    let resp = ui.interact(rect, ui.id().with("kps_graph"), Sense::hover());
    resp.on_hover_text(format!(
        "{}: {} {} / {} s",
        s.kps_max_label,
        fmt_kps(graph_max as f64, kd.method),
        s.kps_label,
        span_secs
    ));
}
/// 尺寸变化时连 min/max 一起下发（with_resizable(false) 会把 min==max 锁成初始尺寸，
/// 只发 InnerSize 会被这个约束挡住 —— 改「每行键数」后窗口不长、第二行看不见，踩过两次）。
fn hud_apply_size(
    ctx: &egui::Context,
    last: &Arc<parking_lot::Mutex<(f32, f32)>>,
    want: (f32, f32),
) {
    let mut last = last.lock();
    if *last != want {
        *last = want;
        let size = egui::vec2(want.0, want.1);
        ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(size));
        ctx.send_viewport_cmd(egui::ViewportCommand::MinInnerSize(size));
        ctx.send_viewport_cmd(egui::ViewportCommand::MaxInnerSize(size));
    }
}

/// 把本帧看到的位置 / 显示器尺寸回写给主窗口（位置记忆 / 限制在屏幕内用）。
fn hud_report_seen(ctx: &egui::Context, slot: &Arc<parking_lot::Mutex<HudSeen>>) {
    let vp = ctx.input(|i| i.viewport().clone());
    let mut seen = slot.lock();
    seen.pos = vp.outer_rect.map(|r| (r.min.x, r.min.y));
    seen.monitor = vp.monitor_size.map(|mm| (mm.x, mm.y));
}

/// 用户关掉了悬浮窗：推事件给主窗口去改开关（这里自己不退出）。
fn hud_close_requested(
    ctx: &egui::Context,
    events: &Arc<parking_lot::Mutex<Vec<HudEvent>>>,
) -> bool {
    if ctx.input(|i| i.viewport().close_requested()) {
        events.lock().push(HudEvent::Close);
        return true;
    }
    false
}

/// 悬浮窗底板的三个手势：拖动 → 移动窗口；双击 → 叫回主窗口；右键 → 打开本窗的设置。
/// 走 window 模块的 HUD 专用路径：egui 命令显式发给 ROOT 视口，并且**解除停放**
/// （只「取消最小化」的话窗口还是全透明 32px，看着像没反应）。
/// 右键只推事件：要不要顺带叫回主窗口由 drain_hud_events 按平台决定
/// （Wayland 下不叫 —— 主人要求右键只开设置窗）。
/// 不要在 HUD 里弹右键菜单：菜单画在本窗口内，340×32 会把第二行起全裁掉（踩过）。
fn hud_background_gestures(
    ui: &mut egui::Ui,
    ctx: &egui::Context,
    rect: Rect,
    hs: &HudShared,
    drag_id: &'static str,
    hint: &'static str,
) {
    let resp = ui.interact(rect, ui.id().with(drag_id), Sense::click_and_drag());
    if resp.drag_started() {
        ctx.send_viewport_cmd(egui::ViewportCommand::StartDrag);
    }
    if resp.double_clicked() {
        show_window_from_hud(&hs.shared, ctx, hs.keep_on_top);
    }
    if resp.clicked_by(egui::PointerButton::Secondary) {
        hs.events.lock().push(HudEvent::OpenSettings);
    }
    resp.on_hover_text(hint);
}

/// 悬浮窗视口：无边框、透明、置顶、尺寸锁死。
/// min/max 也显式给出（与 inner_size 相同 = 尺寸固定）：
/// winit 在 Wayland 下非 resizable 的窗口会把 min==max 锁死，改尺寸必须连它俩一起动。
fn hud_viewport_builder(title: &str, size: (f32, f32)) -> egui::ViewportBuilder {
    let (w, h) = size;
    egui::ViewportBuilder::default()
        .with_title(title)
        .with_inner_size([w, h])
        .with_min_inner_size([w, h])
        .with_max_inner_size([w, h])
        .with_resizable(false)
        .with_decorations(false)
        .with_always_on_top()
        .with_transparent(true)
}

/// 诊断心跳：每 5 秒一条，证明 HUD 视口在**自己**渲染（主窗口收起后是否还在更新，
/// 看日志里这两行是否继续增长即可）。
///
/// 默认 debug 级（否则一个后台工具每 5 秒刷两行日志，一天几万行）；
/// 排查时用 `RUST_LOG=kmcounter=debug` 打开：
/// `RUST_LOG=kmcounter=debug ./kmcounter 2>&1 | grep 'HUD 心跳'`
fn log_hud_alive(which: &'static str) {
    static AT_APM: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    static AT_KPS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let slot = if which == "APM" { &AT_APM } else { &AT_KPS };
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    // 注意：只有真的打印了才更新时间戳 —— 每帧都刷新的话差值永远是一帧的间隔
    let prev = slot.load(Ordering::Relaxed);
    if now_ms.saturating_sub(prev) >= 5000 {
        slot.store(now_ms, Ordering::Relaxed);
        log::debug!("HUD 心跳（{which}）：视口仍在自行渲染");
    }
}

/// APM 悬浮窗的实际渲染（deferred 视口回调；主窗口最小化时也照常被调用）。
fn render_apm_hud(ctx: &egui::Context, hs: &HudShared) {
    log_hud_alive("APM");
    let s = strings(hs.lang);
    let cfg = &hs.cfg;
    let ad = apm_hud_data(ctx, hs, cfg, s);

    // 宽度贴合内容：实测各项文字 + 分隔符 + 内边距，右侧不再空一大块；缩放作用在所有尺寸上。
    hud_apply_size(ctx, &hs.last_size, ad.want_size);

    // 本帧看到的位置 / 显示器尺寸（回写给主窗口：位置记忆 / 限制在屏幕内用）
    hud_report_seen(ctx, &hs.seen);

    if hud_close_requested(ctx, &hs.events) {
        return;
    }
    ctx.request_repaint_after(Duration::from_millis(200));

    let sc = ad.scale;
    let bg_fill = hud_bg_color((24, 26, 34), cfg.apm_hud_opacity);
    let border_stroke = egui::Stroke::new(1.0_f32, Color32::from_rgba_unmultiplied(59, 66, 82, 220));

    egui::CentralPanel::default()
        .frame(
            egui::Frame::none()
                .fill(bg_fill)
                .inner_margin(egui::Margin::symmetric(8.0 * sc, 4.0 * sc))
                .stroke(border_stroke)
                .rounding(6.0),
        )
        .show(ctx, |ui| {
            let full_rect = ui.max_rect();
            hud_background_gestures(ui, ctx, full_rect, hs, "apm_hud_drag", s.hud_drag_hint);

            ui.horizontal_centered(|ui| {
                ui.spacing_mut().item_spacing = Vec2::new(6.0 * sc, 0.0);
                let dim = Color32::from_rgb(0x60, 0x68, 0x78);

                // 各项已在外层按当前显示开关备好（窗口宽度也是按它实测的）
                for (i, (text, tip, color)) in ad.items.iter().enumerate() {
                    if i > 0 {
                        ui.label(egui::RichText::new("·").size(12.0 * sc).color(dim));
                    }
                    ui.label(egui::RichText::new(text.clone()).strong().size(12.0 * sc).color(*color))
                        .on_hover_text(tip.clone());
                }
            });
        });
}

/// APM 悬浮窗每帧要算好的一组数据：显示项、强调色、缩放与贴合内容的目标尺寸。
struct ApmHudData {
    items: Vec<(String, String, Color32)>,
    scale: f32,
    want_size: (f32, f32),
}

/// 采集今日按键 / 点击 / 距离 + 当前与峰值 APM，拼出要显示的项并实测窗口宽度。
fn apm_hud_data(
    ctx: &egui::Context,
    hs: &HudShared,
    cfg: &Config,
    s: &'static Strings,
) -> ApmHudData {
    let (today_keys, today_clicks, today_dist_m) = {
        let store = hs.shared.store.lock();
        if let Some(entry) = store.days.get(&hs.today) {
            (entry.keystrokes, entry.mouse_clicks(), hs.shared.px_to_meters(entry.mouse.move_px))
        } else {
            (0, 0, 0.0)
        }
    };
    let apm_w = hs.shared.apm_window_secs.load(Ordering::Relaxed);
    let (cur_apm, peak_apm) = hs.shared.apm.lock().get(&hs.today, apm_w);

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
    if cfg.apm_show_keys {
        items.push((
            format!("⌨ {}", fmt_thousands(today_keys as f64)),
            format!("{}: {}", s.keystrokes, today_keys),
            val,
        ));
    }
    if cfg.apm_show_clicks {
        items.push((
            format!("🖱 {}", fmt_thousands(today_clicks as f64)),
            format!("{}: {}", s.mouse_clicks, today_clicks),
            val,
        ));
    }
    if cfg.apm_show_dist {
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

    let sc = apm_hud_scale(cfg);
    let texts: Vec<String> = items.iter().map(|(t, _, _)| t.clone()).collect();
    ApmHudData {
        items,
        scale: sc,
        want_size: (apm_hud_measure(ctx, &texts, sc), 32.0 * sc),
    }
}

impl App {
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
        // KDE/Wayland：KWin 脚本上报的位置（带"是不是用户拖的"标记）；其他平台 None
        let report = kde_hud_pos(idx);

        // ① 恢复：头三帧都发，刚建好的窗口可能吞掉第一份命令
        self.restore_hud_pos(ctx, vp_id, is_apm, saved, seen);

        // 平台拿不到位置时，看 KWin 脚本有没有上报（KDE / Wayland）
        let cur = match (seen, report) {
            (Some(p), _) => p,
            (None, Some((x, y, _))) => (x, y),
            (None, None) => return,
        };
        // 当前位置能不能当成「用户的选择」：
        // - winit 能读位置的平台（X11 / Windows）：能 —— 后面靠「稳定 1.2s」判定；
        // - KDE / Wayland：只有用户真的拖动过才算。合成器在窗口刚出现时的自动
        //   摆放也会被脚本上报，照单全收的话，哪次没恢复上就把错位沉淀成新记忆
        //   （实测：反复重启会让 HUD 位置越走越远，主人的 KPS 就这么被推走了好几次）。
        let user_position = seen.is_some() || report.map_or(false, |(_, _, moved)| moved);

        // ② 限制在屏幕内（只管 winit 能设位置的平台；KDE 的夹取在 KWin 脚本里做）
        if seen.is_some()
            && self.clamp_hud_to_monitor(ctx, vp_id, is_apm, cur, monitor, size, keep_in_screen)
        {
            return;
        }

        // ③ 记忆：与记忆不同 → 等它安静约 1.2s → 过了启动安静期 → 落盘
        if let Some(p) = self.settle_hud_pos(is_apm, cur, saved, user_position) {
            // 落盘时把来源一并写进日志：位置记忆一旦被「非用户选择」污染就会越走越歪，
            // 出问题时这行日志能直接指出是 winit 读到的位置还是 KWin 脚本上报的
            log::info!(
                "HUD 位置落盘：{} = {p:?}（winit 位置={seen:?} / KWin 上报={report:?}）",
                if is_apm { "APM" } else { "KPS" }
            );
            self.save_hud_pos(is_apm, p);
        }
    }

    /// ① 恢复上次的位置。头三帧都发：刚建好的窗口可能吞掉第一份命令。
    fn restore_hud_pos(
        &mut self,
        ctx: &egui::Context,
        vp_id: egui::ViewportId,
        is_apm: bool,
        saved: Option<(f32, f32)>,
        seen: Option<(f32, f32)>,
    ) {
        let st = if is_apm { &mut self.apm_pos } else { &mut self.kps_pos };
        if st.restored >= 3 {
            return;
        }
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

    /// ② 限制在屏幕内。返回 true 表示这帧已经下发了修正命令，调用方应直接返回
    ///（中间态不落盘）。只在 winit 能设位置的平台走这里，KDE 的夹取在 KWin 脚本里做。
    #[allow(clippy::too_many_arguments)]
    fn clamp_hud_to_monitor(
        &mut self,
        ctx: &egui::Context,
        vp_id: egui::ViewportId,
        is_apm: bool,
        cur: (f32, f32),
        monitor: Option<(f32, f32)>,
        size: (f32, f32),
        keep_in_screen: bool,
    ) -> bool {
        if !keep_in_screen {
            return false;
        }
        let Some(mon) = monitor else { return false };
        let clamped = clamp_hud_pos(cur, size, mon);
        if pos_dist(clamped, cur) <= 0.5 {
            return false;
        }
        ctx.send_viewport_cmd_to(
            vp_id,
            egui::ViewportCommand::OuterPosition(egui::pos2(clamped.0, clamped.1)),
        );
        let st = if is_apm { &mut self.apm_pos } else { &mut self.kps_pos };
        st.pending = None; // 正在被我们修正，中间态不落盘
        true
    }

    /// ③ 记忆：与记忆不同 → 等它安静约 1.2s → 过了启动安静期 → 返回要落盘的位置。
    /// user_position 为假 = 这个位置是合成器摆的，绝不动记忆。
    fn settle_hud_pos(
        &mut self,
        is_apm: bool,
        cur: (f32, f32),
        saved: Option<(f32, f32)>,
        user_position: bool,
    ) -> Option<(f32, f32)> {
        let grace_until = self.hud_pos_grace_until;
        let st = if is_apm { &mut self.apm_pos } else { &mut self.kps_pos };
        if !(user_position && saved.map_or(true, |p| pos_dist(p, cur) > 0.5)) {
            st.pending = None;
            return None;
        }
        let first_seen = match st.pending {
            Some((p, t)) if pos_dist(p, cur) <= 0.5 => t,
            _ => {
                let now = Instant::now();
                st.pending = Some((cur, now));
                now
            }
        };
        let now = Instant::now();
        if now.duration_since(first_seen).as_secs_f32() > 1.2 && now >= grace_until {
            st.pending = None;
            return Some(cur);
        }
        None
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
        // KDE/Wayland：恢复目标（KMC_TARGET）是编在 KWin 监听脚本里的，记忆变了要重载
        //（幂等：签名没变会跳过），否则「关闭 HUD 再打开」会回到旧位置
        #[cfg(target_os = "linux")]
        crate::kwin::sync_hud_listener(
            self.cfg.apm_keep_in_screen,
            self.cfg.kps_keep_in_screen,
            self.cfg.apm_hud_pos,
            self.cfg.kps_hud_pos,
        );
        // KWin 窗口规则同步：规则在窗口 map 时由 KWin 核心应用位置（恢复的正道），
        // 记忆变了要更新，下次启动 / 重新打开就回到新位置
        #[cfg(target_os = "linux")]
        crate::kwin::sync_hud_rules(self.cfg.apm_hud_pos, self.cfg.kps_hud_pos);
    }

    /// 悬浮窗设置改动后：同步到共享状态（原子量 / KPS 跟踪器）并落盘。
    /// 只改自己关心的字段、不整份覆盖 —— 托盘可能刚改过别的设置（例如开机启动）。
    fn apply_hud_cfg(&mut self) {
        self.sync_hud_runtime();
        self.persist_hud_cfg();
        self.sync_hud_kwin_scripts();
    }

    /// 把窗口时长 / KPS 方法推到运行中的共享状态（下一帧 HUD 立刻按新值算）。
    fn sync_hud_runtime(&mut self) {
        self.shared
            .apm_window_secs
            .store(self.cfg.apm_window_secs, Ordering::Relaxed);
        self.shared
            .kps_window_ms
            .store(self.cfg.kps_window_ms, Ordering::Relaxed);
        let mut kps = self.shared.kps.lock();
        kps.set_monitored(&self.cfg.kps_key_layout().names());
        kps.set_method(crate::stats::KpsMethod::from_config(&self.cfg.kps_method));
    }

    /// 落盘：面板里的副本一起更新（别把新设置覆盖回去），再写回配置文件。
    fn persist_hud_cfg(&mut self) {
        copy_hud_fields(&self.cfg, &mut self.edit);
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
        cfg.kps_show_rain = self.cfg.kps_show_rain;
        cfg.kps_rain_width_pct = self.cfg.kps_rain_width_pct;
        cfg.kps_rain_speed = self.cfg.kps_rain_speed;
        cfg.kps_rain_height = self.cfg.kps_rain_height;
        cfg.kps_rain_opacity = self.cfg.kps_rain_opacity;
        cfg.kps_rain_fade_pct = self.cfg.kps_rain_fade_pct;
        cfg.kps_rain_color = self.cfg.kps_rain_color;
        cfg.kps_key_bounce = self.cfg.kps_key_bounce;
        cfg.kps_groups = self.cfg.kps_groups.clone();
        cfg.kps_show_groups = self.cfg.kps_show_groups;
        cfg.apm_scale = self.cfg.apm_scale;
        cfg.apm_keep_in_screen = self.cfg.apm_keep_in_screen;
        cfg.kps_keep_in_screen = self.cfg.kps_keep_in_screen;
        let _ = cfg.save(&self.cfg_path);
    }

    /// KDE/Wayland：屏幕内限制的开关或记忆位置变了，要重载 HUD 监听脚本（幂等，没变就跳过）。
    fn sync_hud_kwin_scripts(&mut self) {
        #[cfg(target_os = "linux")]
        crate::kwin::sync_hud_listener(
            self.cfg.apm_keep_in_screen,
            self.cfg.kps_keep_in_screen,
            self.cfg.apm_hud_pos,
            self.cfg.kps_hud_pos,
        );
        #[cfg(not(target_os = "linux"))]
        let _ = &self.cfg;
    }
}

/// HUD 设置窗口里由界面交互置起的动作位：视口闭包里只置位，关闭后统一落地
#[derive(Default)]
struct HudCfgActions {
    close_settings: bool,
    open_main: bool,
    reset_peak: bool,
    reset_counts: bool,
    reset_graph: bool,
    reset_all: bool,
    toggle_hud: bool,
    dirty: bool,
    start_capture: bool,
    cancel_capture: bool,
}

impl App {
    /// APM 悬浮窗自己的设置窗口（右键悬浮窗打开）。
    ///
    /// 用**独立视口**而不是右键菜单：菜单画在悬浮窗内部，340×32 的胶囊条会把第二行起全裁掉（踩过）。
    pub(crate) fn show_apm_settings(&mut self, ctx: &egui::Context, s: &'static Strings) {
        let vp_id = egui::ViewportId::from_hash_of("kmc_apm_cfg");
        let mut act = HudCfgActions::default();

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
                    act.close_settings = true;
                    return;
                }
                cfg_ctx.request_repaint_after(Duration::from_millis(400));
                let bg = hud_bg_color((24, 26, 34), SETTINGS_BG_OPACITY);
                let closed = settings_shell(cfg_ctx, "⚡", s.apm_cfg_title, s.cfg_close, bg, |ui| {
                    self.apm_settings_body(ui, s, &mut act);
                });
                if closed {
                    act.close_settings = true;
                }
            },
        );

        self.apply_apm_settings_actions(ctx, &act);
    }

    /// APM 设置窗口的内容：窗口长度 / 不透明度 / 显示项 / 缩放 / 底部按钮
    /// APM 设置窗口的内容：与 KPS 设置用同一套小节标题，两个窗口看起来是一个东西。
    ///
    /// 「重置峰值」从显示组挪进底栏按钮区 —— 它和「打开主程序 / 显示悬浮窗」
    /// 同属一次性动作，单独混在勾选框里会让人以为它是个开关。
    fn apm_settings_body(&mut self, ui: &mut egui::Ui, s: &'static Strings, act: &mut HudCfgActions) {
        settings_section(ui, s.cfg_group_basic);
        self.apm_settings_window_row(ui, s, act);
        ui.add_space(2.0);
        self.apm_settings_scale_row(ui, s, act);

        settings_section(ui, s.cfg_group_display);
        self.apm_settings_display_toggles(ui, s, act);

        ui.add_space(6.0);
        self.apm_settings_buttons(ui, s, act);
        ui.add_space(4.0);
        ui.label(
            egui::RichText::new(s.hud_cfg_hint)
                .small()
                .color(Color32::from_rgb(0xC8, 0xCC, 0xD4)),
        );
    }

    /// 窗口长度（秒）+ 不透明度（%）
    fn apm_settings_window_row(&mut self, ui: &mut egui::Ui, s: &'static Strings, act: &mut HudCfgActions) {
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new(s.settings_apm_window).color(HUD_TEXT));
            if ui
                .add(egui::DragValue::new(&mut self.cfg.apm_window_secs).range(5..=300).speed(1.0))
                .on_hover_text(s.settings_apm_window_sub)
                .changed()
            {
                act.dirty = true;
            }
            ui.label(egui::RichText::new("s").color(HUD_TEXT));
            ui.add_space(10.0);
            ui.label(egui::RichText::new(s.hud_opacity).color(HUD_TEXT));
            if ui
                .add(
                    egui::DragValue::new(&mut self.cfg.apm_hud_opacity)
                        // 允许 0：整块底板与外框彻底消失，只剩文字内容。
                        // 设置窗是独立窗口、常驻可点，所以调到 0 之后仍能从这里调回来。
                        .range(0..=100)
                        .speed(1.0)
                        .suffix("%"),
                )
                .changed()
            {
                act.dirty = true;
            }
        });
    }

    /// 显示项：按键 / 点击 / 移动距离
    fn apm_settings_display_toggles(&mut self, ui: &mut egui::Ui, s: &'static Strings, act: &mut HudCfgActions) {
        ui.horizontal(|ui| {
            if ui.checkbox(&mut self.cfg.apm_show_keys, egui::RichText::new(s.apm_show_keys).color(HUD_TEXT)).changed() {
                act.dirty = true;
            }
            if ui.checkbox(&mut self.cfg.apm_show_clicks, egui::RichText::new(s.apm_show_clicks).color(HUD_TEXT)).changed() {
                act.dirty = true;
            }
            if ui.checkbox(&mut self.cfg.apm_show_dist, egui::RichText::new(s.apm_show_dist).color(HUD_TEXT)).changed() {
                act.dirty = true;
            }
        });
    }

    /// 缩放 + 限制在屏幕内（对齐 osu-kps 的同名选项）
    fn apm_settings_scale_row(&mut self, ui: &mut egui::Ui, s: &'static Strings, act: &mut HudCfgActions) {
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new(s.kps_cfg_scale).color(HUD_TEXT));
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
                act.dirty = true;
            }
            ui.add_space(10.0);
            if ui
                .checkbox(&mut self.cfg.apm_keep_in_screen, egui::RichText::new(s.hud_keep_in_screen).color(HUD_TEXT))
                .on_hover_text(s.hud_keep_in_screen_hint)
                .changed()
            {
                act.dirty = true;
            }
        });
    }

    /// 底部按钮：打开主窗口 / 重置峰值 / 显示或关闭 HUD
    fn apm_settings_buttons(&mut self, ui: &mut egui::Ui, s: &'static Strings, act: &mut HudCfgActions) {
        ui.horizontal(|ui| {
            if ui.button(s.hud_open_main).clicked() {
                act.open_main = true;
            }
            if ui.button(s.hud_reset_peak).clicked() {
                act.reset_peak = true;
            }
            let hud_btn = if self.cfg.hud_apm_show { s.hud_close } else { s.hud_show };
            if ui.button(hud_btn).clicked() {
                act.toggle_hud = true;
            }
        });
    }

    /// APM 设置窗口的动作落地：改动即时生效（写盘集中在改动与关闭两个时机）
    fn apply_apm_settings_actions(&mut self, ctx: &egui::Context, act: &HudCfgActions) {
        // 窗口内的改动即时生效（写盘集中在改动/关闭两个时机，拖动滑杆不会每帧写文件）
        if act.dirty {
            self.apply_hud_cfg();
        }
        if act.toggle_hud {
            self.toggle_apm_hud();
            self.apply_hud_cfg();
        }
        if act.reset_peak {
            self.shared.apm.lock().reset_peak();
        }
        if act.open_main {
            show_window_now(&self.shared, ctx, self.cfg.keep_on_top);
        }
        if act.close_settings {
            self.apm_cfg_open = false;
            self.apply_hud_cfg(); // 关闭时兜底落盘
        }
    }

    /// KPS 悬浮窗自己的设置窗口（右键悬浮窗打开）：窗口长度、监控键、显示项、不透明度、重置与关闭。
    pub(crate) fn show_kps_settings(&mut self, ctx: &egui::Context, s: &'static Strings) {
        let vp_id = egui::ViewportId::from_hash_of("kmc_kps_cfg");
        let mut act = HudCfgActions::default();

        ctx.show_viewport_immediate(
            vp_id,
            egui::ViewportBuilder::default()
                .with_title(format!("KMC - {}", s.kps_cfg_title))
                // 高度按内容实测定。分四组（基础 / 按键 / 显示 / 键雨）后每组多一行小标题，
                // 同时键表输入框独占整行（原来挤在 label 后面只有 180px 宽）。
                // 实测值须与 render_kps_settings_shot 的截图画布一致，否则底栏会被截断。
                .with_inner_size([430.0, 588.0])
                .with_resizable(false)
                .with_decorations(false)
                .with_always_on_top()
                .with_transparent(true),
            |cfg_ctx, _class| {
                if cfg_ctx.input(|i| i.viewport().close_requested()) {
                    act.close_settings = true;
                    return;
                }
                // 捕获键位时要及时反馈，其它时候没必要刷那么勤
                let interval = if self.kps_capturing { 80 } else { 400 };
                cfg_ctx.request_repaint_after(Duration::from_millis(interval));
                let bg = hud_bg_color((16, 18, 24), SETTINGS_BG_OPACITY);
                let closed = settings_shell(cfg_ctx, "🎯", s.kps_cfg_title, s.cfg_close, bg, |ui| {
                    self.kps_settings_body(ui, s, &mut act);
                });
                if closed {
                    act.close_settings = true;
                }
            },
        );

        self.apply_kps_settings_actions(ctx, &act);
    }

    /// KPS 设置窗口的内容：四个小节，每组先画标题再逐行交给下面的小方法。
    ///
    /// 分组原则（用户反馈"设置界面太乱"后重排）：
    /// 1. **基础**：决定数字怎么算 —— 统计窗口、计算方式、缩放、自适应键宽、不透明度；
    /// 2. **按键**：决定把哪些键画出来 —— 键表、键数、设置键位、分组 KPS；
    /// 3. **显示**：决定画哪些部件 —— 按键格子/统计/图/各项读数/限制在屏幕内；
    /// 4. **键雨**：纯表现层 —— 开关、宽度、速度、高度、不透明度、颜色。
    ///
    /// 三处刻意设计：
    /// - 「重置」是低频且不可逆的操作，挂在**底栏最右端**，不跟常用控件混；
    /// - 选择类控件（是否显示、是否限制）集中在显示组，数值类控件紧跟各自的标签，
    ///   避免一行里塞四五种控件导致眼睛要来回扫；
    /// - 键表的多行输入独占一行（原来它挤在 label 后面，宽度被压到 180，
    ///   键一多就要横向滚动）。
    fn kps_settings_body(&mut self, ui: &mut egui::Ui, s: &'static Strings, act: &mut HudCfgActions) {
        settings_section(ui, s.cfg_group_basic);
        self.kps_settings_window_row(ui, s, act);
        ui.add_space(2.0);
        self.kps_settings_scale_row(ui, s, act);

        settings_section(ui, s.cfg_group_keys);
        self.kps_settings_key_list(ui, s, act);
        self.kps_settings_key_buttons(ui, s, act);
        self.kps_settings_capture(ui, s, act);
        self.kps_settings_groups(ui, s, act);

        settings_section(ui, s.cfg_group_display);
        self.kps_settings_display_toggles(ui, s, act);

        settings_section(ui, s.cfg_group_rain);
        self.kps_settings_rain(ui, s, act);

        self.kps_settings_footer(ui, s, act);
    }

    /// 第一行：统计窗口长度（ms）与计算方式（硬算 / 灵敏）
    fn kps_settings_window_row(&mut self, ui: &mut egui::Ui, s: &'static Strings, act: &mut HudCfgActions) {
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new(s.settings_kps_window).color(HUD_TEXT));
            if ui
                .add(egui::DragValue::new(&mut self.cfg.kps_window_ms).range(200..=5000).speed(50.0))
                .on_hover_text(s.settings_kps_window_sub)
                .changed()
            {
                act.dirty = true;
            }
            ui.label(egui::RichText::new("ms").color(HUD_TEXT));
            ui.add_space(8.0);
            self.kps_settings_method_combo(ui, s, act);
        });
    }

    fn kps_settings_method_combo(
        &mut self,
        ui: &mut egui::Ui,
        s: &'static Strings,
        act: &mut HudCfgActions,
    ) {
        ui.label(egui::RichText::new(s.kps_cfg_method).color(HUD_TEXT));
        let cur = crate::stats::KpsMethod::from_config(&self.cfg.kps_method);
        egui::ComboBox::from_id_salt("kps_method")
            .selected_text(kps_method_label(cur, s))
            .width(66.0)
            .show_ui(ui, |ui| {
                self.kps_method_options(ui, s, cur, act);
            })
            .response
            .on_hover_text(kps_method_hint(cur, s));
    }

    /// 下拉里的两个选项：命中后写回 cfg 并标脏。
    fn kps_method_options(
        &mut self,
        ui: &mut egui::Ui,
        s: &'static Strings,
        cur: crate::stats::KpsMethod,
        act: &mut HudCfgActions,
    ) {
        // 注意：ComboBox 的弹出层是 egui 浅色主题的白底，
        // 这里千万别套 HUD_TEXT（白字白底＝看不见，踩过）
        if ui
            .selectable_label(
                cur == crate::stats::KpsMethod::Hard,
                egui::RichText::new(s.kps_method_hard),
            )
            .on_hover_text(s.kps_method_hard_hint)
            .clicked()
        {
            self.cfg.kps_method = crate::stats::KpsMethod::Hard.as_config().to_string();
            act.dirty = true;
        }
        if ui
            .selectable_label(
                cur == crate::stats::KpsMethod::Sensitive,
                egui::RichText::new(s.kps_method_sensitive),
            )
            .on_hover_text(s.kps_method_sensitive_hint)
            .clicked()
        {
            self.cfg.kps_method = crate::stats::KpsMethod::Sensitive.as_config().to_string();
            act.dirty = true;
        }
    }

    /// 第二行：缩放、自适应键宽与不透明度 —— 三者都只影响"整块悬浮窗的外观框架"，
    /// 所以放在同一个视觉组里（数值框之间用小间距，组内各控件紧挨着）。
    fn kps_settings_scale_row(&mut self, ui: &mut egui::Ui, s: &'static Strings, act: &mut HudCfgActions) {
        ui.horizontal(|ui| {
            self.kps_settings_scale(ui, s, act);
            ui.add_space(12.0);
            if ui
                .checkbox(
                    &mut self.cfg.kps_auto_width,
                    egui::RichText::new(s.kps_auto_width).color(HUD_TEXT),
                )
                .on_hover_text(s.kps_auto_width_hint)
                .changed()
            {
                act.dirty = true;
            }
            ui.add_space(12.0);
            ui.label(egui::RichText::new(s.hud_opacity).color(HUD_TEXT));
            if ui
                .add(
                    egui::DragValue::new(&mut self.cfg.kps_hud_opacity)
                        // 允许 0：底板与外框消失、只剩键格与数字（理由同 APM）
                        .range(0..=100)
                        .speed(1.0)
                        .suffix("%"),
                )
                .on_hover_text(s.hud_opacity_hint)
                .changed()
            {
                act.dirty = true;
            }
        });
    }

    /// 缩放百分比（60~400%）。
    fn kps_settings_scale(&mut self, ui: &mut egui::Ui, s: &'static Strings, act: &mut HudCfgActions) {
        ui.label(egui::RichText::new(s.kps_cfg_scale).color(HUD_TEXT));
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
            act.dirty = true;
        }
    }

    /// 键表输入：换行=分行，空项=空位，1.5H/4H=宽度倍数。
    ///
    /// 布局：标签单独一行、输入框占满整行宽度。原来标签和输入框挤在同一行，
    /// 输入框只剩 180px，多排键表必须横向滚动才看得全（这正是"乱"的来源之一）。
    fn kps_settings_key_list(&mut self, ui: &mut egui::Ui, s: &'static Strings, act: &mut HudCfgActions) {
        ui.label(egui::RichText::new(s.settings_kps_keys).color(HUD_TEXT));
        // 多行输入：支持"换行 = 分行 / 空项 = 空位 / 1.5H、4H = 宽度倍数"
        if ui
            .add(
                egui::TextEdit::multiline(&mut self.cfg.kps_keys)
                    .desired_width(f32::INFINITY)
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
            act.dirty = true;
        }
    }

    /// 键数调节、设置键位与恢复默认键表
    fn kps_settings_key_buttons(&mut self, ui: &mut egui::Ui, s: &'static Strings, act: &mut HudCfgActions) {
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new(s.kps_key_count).color(HUD_TEXT));
            let mut count = self.cfg.kps_key_count as i32;
            if ui
                .add(egui::DragValue::new(&mut count).range(1..=26).speed(1.0))
                .changed()
            {
                self.cfg.kps_key_count = count.clamp(1, 26) as u8;
                self.cfg.resize_kps_keys();
                act.dirty = true;
            }
            if ui
                .button(s.kps_set_keys)
                .on_hover_text(s.kps_set_keys_hint)
                .clicked()
            {
                act.start_capture = true;
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
                act.dirty = true;
            }
        });
    }

    /// 重置菜单（由底栏用 right_to_left 布局贴到行右端）。
    fn kps_settings_reset_menu(&mut self, ui: &mut egui::Ui, s: &'static Strings, act: &mut HudCfgActions) {
        ui.menu_button(s.kps_reset, |ui| {
            // 菜单是浅色弹出层：用主题默认深色字（不要 HUD_TEXT，白字白底看不见）
            ui.label(egui::RichText::new(s.kps_reset_hint));
            ui.separator();
            if ui.button(s.kps_reset_counts).clicked() {
                act.reset_counts = true;
                ui.close_menu();
            }
            if ui.button(s.kps_reset_peak).clicked() {
                act.reset_peak = true;
                ui.close_menu();
            }
            if ui.button(s.kps_reset_graph).clicked() {
                act.reset_graph = true;
                ui.close_menu();
            }
            ui.separator();
            if ui.button(s.kps_reset_all).clicked() {
                act.reset_all = true;
                ui.close_menu();
            }
        });
    }

    /// 键位捕获进度：已捕获的键、进度计数与取消（凑满 N 个自动写入）
    fn kps_settings_capture(&mut self, ui: &mut egui::Ui, s: &'static Strings, act: &mut HudCfgActions) {
        // 「设置键位」捕获中：显示进度 + 已捕获的键 + 取消（凑满 N 个就自动写入）
        if self.kps_capturing {
            let need = self.cfg.kps_key_count.max(1) as usize;
            let captured = self.shared.kps_capture_buf.lock().clone();
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new(s.kps_capture_prompt)
                        .color(Color32::from_rgb(0xFF, 0xD0, 0x60)),
                );
                ui.label(
                    egui::RichText::new(format!(
                        "{}/{}",
                        captured.len().min(need),
                        need
                    ))
                    .strong()
                    .color(Color32::from_rgb(0xFF, 0xD0, 0x60)),
                );
                let names = if captured.is_empty() {
                    s.kps_capture_none.to_string()
                } else {
                    captured.join(", ")
                };
                ui.label(egui::RichText::new(names).color(HUD_TEXT));
                if ui.small_button(s.kps_capture_cancel).clicked() {
                    act.cancel_capture = true;
                }
            });
            if captured.len() >= need {
                self.cfg.kps_keys = captured.into_iter().take(need).collect::<Vec<_>>().join(", ");
                self.cfg.kps_key_count = need as u8;
                act.dirty = true;
                act.cancel_capture = true;
            }
        }
    }

    /// 显示组：第一行是「画哪些部件」（格子 / 统计 / 图 + 图时长），
    /// 第二行是「统计信息里画哪些读数」（击打次数 / 单键 KPS / Max / 平均），
    /// 第三行是窗口行为（限制在屏幕内）。
    ///
    /// 拆成"部件"和"读数"两行而不是随便切三行：前者决定画不画一整块区域，
    /// 后者只影响那块区域里的文字，分开放才好找。
    fn kps_settings_display_toggles(&mut self, ui: &mut egui::Ui, s: &'static Strings, act: &mut HudCfgActions) {
        ui.horizontal(|ui| {
            hud_check(ui, &mut self.cfg.kps_show_buttons, s.kps_show_buttons, act);
            hud_check(ui, &mut self.cfg.kps_show_stats, s.kps_show_stats, act);
            hud_check(ui, &mut self.cfg.kps_show_graph, s.kps_show_graph, act);
            self.kps_settings_graph_secs(ui, s, act);
        });
        // 读数项按"四个字以上的长标签"和"短标签"分两行：全挤一行时最右边的「单键 KPS」
        // 会被窗口右边缘裁掉（430px 宽装不下四个长复选框，实测差约 100px）。
        ui.horizontal(|ui| {
            hud_check(ui, &mut self.cfg.kps_show_hits, s.settings_kps_show_hits, act);
            hud_check(ui, &mut self.cfg.kps_show_max, s.settings_kps_show_max, act);
            hud_check(ui, &mut self.cfg.kps_show_avg, s.kps_show_avg, act);
        });
        ui.horizontal(|ui| {
            hud_check(ui, &mut self.cfg.kps_show_key_kps, s.kps_show_key_kps, act);
            ui.add_space(12.0);
            if ui
                .checkbox(&mut self.cfg.kps_keep_in_screen, egui::RichText::new(s.hud_keep_in_screen).color(HUD_TEXT))
                .on_hover_text(s.hud_keep_in_screen_hint)
                .changed()
            {
                act.dirty = true;
            }
        });
    }


    /// 「显示 KPS 曲线」后面跟着的时长输入（仅在勾选时才出现）。
    fn kps_settings_graph_secs(&mut self, ui: &mut egui::Ui, s: &'static Strings, act: &mut HudCfgActions) {
        if !self.cfg.kps_show_graph {
            return;
        }
        ui.label(egui::RichText::new(s.kps_cfg_graph_secs).color(HUD_TEXT));
        if ui
            .add(egui::DragValue::new(&mut self.cfg.kps_graph_secs).range(10..=300).speed(5.0))
            .changed()
        {
            act.dirty = true;
        }
        ui.label(egui::RichText::new("s").color(HUD_TEXT));
    }


    /// 键雨（Key Rain）与按键弹跳的设置块（对齐 JipperKeyViewer 的雨滴参数）。
    /// 键雨与 KPS 图是**两个独立开关**：只开图 ≈ osu-kps，只开雨 ≈ JipperKeyViewer，
    /// 都开则上下叠放，一屏看到更多。
    fn kps_settings_rain(&mut self, ui: &mut egui::Ui, s: &'static Strings, act: &mut HudCfgActions) {
        ui.horizontal(|ui| {
            hud_check(ui, &mut self.cfg.kps_show_rain, s.kps_show_rain, act);
            ui.add_space(12.0);
            // 弹跳属于键格的表现，但和键雨同属"按下去有反馈"这一类，放一起更好找
            hud_check(ui, &mut self.cfg.kps_key_bounce, s.kps_key_bounce, act);
        });
        if !self.cfg.kps_show_rain {
            return;
        }
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new(s.kps_rain_width_pct).color(HUD_TEXT));
            if ui
                .add(
                    egui::DragValue::new(&mut self.cfg.kps_rain_width_pct)
                        .range(20..=100)
                        .speed(1.0),
                )
                .changed()
            {
                act.dirty = true;
            }
            ui.label(egui::RichText::new("%").color(HUD_TEXT));
            ui.label(egui::RichText::new(s.kps_rain_speed).color(HUD_TEXT));
            if ui
                .add(egui::DragValue::new(&mut self.cfg.kps_rain_speed).range(200..=20000).speed(20.0))
                .changed()
            {
                act.dirty = true;
            }
            ui.label(egui::RichText::new("ms").color(HUD_TEXT));
        });
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new(s.kps_rain_height).color(HUD_TEXT));
            if ui
                .add(egui::DragValue::new(&mut self.cfg.kps_rain_height).range(0..=1200).speed(4.0))
                .changed()
            {
                act.dirty = true;
            }
            ui.label(
                egui::RichText::new(format!("px ({} 96)", s.kps_rain_height_auto))
                    .small()
                    .color(Color32::from_rgb(0xC8, 0xCC, 0xD4)),
            );
            ui.label(egui::RichText::new(s.kps_rain_opacity).color(HUD_TEXT));
            if ui
                .add(egui::DragValue::new(&mut self.cfg.kps_rain_opacity).range(0..=100).speed(1.0))
                .changed()
            {
                act.dirty = true;
            }
            ui.label(egui::RichText::new("%").color(HUD_TEXT));
        });
        // 渐隐带：轨道顶部往下这么高的一段，越靠上越透明
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new(s.kps_rain_fade).color(HUD_TEXT));
            if ui
                .add(
                    egui::DragValue::new(&mut self.cfg.kps_rain_fade_pct)
                        .range(5..=90)
                        .speed(1.0),
                )
                .on_hover_text(s.kps_rain_fade_hint)
                .changed()
            {
                act.dirty = true;
            }
            ui.label(egui::RichText::new("%").color(HUD_TEXT));
            let band_px = crate::gui::rain::rain_area_h(&self.cfg)
                * self.cfg.kps_rain_fade_pct.max(5) as f32
                / 100.0;
            ui.label(
                egui::RichText::new(format!("= {band_px:.0} px"))
                    .small()
                    .color(Color32::from_rgb(0xC8, 0xCC, 0xD4)),
            );
        });
        // 键雨颜色：默认纯白。键雨**不跟随 KPS 热度**，所以颜色必须能在设置里自己调。
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new(s.kps_rain_color).color(HUD_TEXT));
            if ui.color_edit_button_srgb(&mut self.cfg.kps_rain_color).changed() {
                act.dirty = true;
            }
            ui.label(
                egui::RichText::new(format!(
                    "#{:02X}{:02X}{:02X}",
                    self.cfg.kps_rain_color[0],
                    self.cfg.kps_rain_color[1],
                    self.cfg.kps_rain_color[2]
                ))
                .small()
                .color(Color32::from_rgb(0xC8, 0xCC, 0xD4)),
            );
        });
    }

    /// 分组 KPS：勾选 + 多行文本编辑（一行一组，组名=A,S,D,F）
    /// 分组 KPS：勾选 + 多行文本编辑（一行一组，组名=A,S,D,F）。
    ///
    /// 复选框用"分组 KPS"，后面那条同文本的小灰字已删（一句话写两遍＝纯噪音）。
    fn kps_settings_groups(&mut self, ui: &mut egui::Ui, s: &'static Strings, act: &mut HudCfgActions) {
        ui.horizontal(|ui| {
            hud_check(ui, &mut self.cfg.kps_show_groups, s.kps_show_groups, act);
        });
        if !self.cfg.kps_show_groups {
            return;
        }
        if ui
            .add(
                egui::TextEdit::multiline(&mut self.cfg.kps_groups)
                    .desired_rows(2)
                    .desired_width(f32::INFINITY)
                    .hint_text(format!("{}: A,S,D,F", s.kps_group_empty)),
            )
            .changed()
        {
            act.dirty = true;
        }
        ui.label(
            egui::RichText::new(s.kps_groups_hint)
                .small()
                .color(Color32::from_rgb(0xC8, 0xCC, 0xD4)),
        );
    }

    /// 底栏：左侧是"动作"（打开主窗口 / 显示或关闭悬浮窗），右端是低频且不可逆的「重置」。
    ///
    /// 「限制在屏幕内」已挪进显示组（它是窗口行为，不属于底栏的按钮区）。
    /// 重置从按键行挪到这里：它和那几个按钮同属"一次性动作"，
    /// 而按键行里夹一个下拉菜单恰恰是让那一行看起来乱的主要原因。
    fn kps_settings_footer(&mut self, ui: &mut egui::Ui, s: &'static Strings, act: &mut HudCfgActions) {
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            if ui.button(s.hud_open_main).clicked() {
                act.open_main = true;
            }
            let hud_btn = if self.cfg.hud_kps_show { s.hud_close } else { s.hud_show };
            if ui.button(hud_btn).clicked() {
                act.toggle_hud = true;
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                self.kps_settings_reset_menu(ui, s, act);
            });
        });
        ui.add_space(4.0);
        ui.label(
            egui::RichText::new(s.hud_cfg_hint)
                .small()
                .color(Color32::from_rgb(0xC8, 0xCC, 0xD4)),
        );
    }

    /// KPS 设置窗口的动作落地：捕获启动/取消、写盘、重置统计与开关悬浮窗
    fn apply_kps_settings_actions(&mut self, ctx: &egui::Context, act: &HudCfgActions) {
        if act.start_capture {
            self.start_kps_capture();
        }
        if act.cancel_capture {
            self.stop_kps_capture();
        }
        if act.dirty {
            self.apply_hud_cfg();
        }
        if act.toggle_hud {
            self.toggle_kps_hud();
            self.apply_hud_cfg();
        }
        if act.reset_counts || act.reset_peak || act.reset_graph || act.reset_all {
            let mut kps = self.shared.kps.lock();
            if act.reset_all {
                kps.reset_all();
            } else {
                if act.reset_counts {
                    kps.reset_counts();
                }
                if act.reset_peak {
                    kps.reset_peak();
                }
                if act.reset_graph {
                    kps.reset_graph();
                }
            }
        }
        if act.open_main {
            show_window_now(&self.shared, ctx, self.cfg.keep_on_top);
        }
        if act.close_settings {
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

    /// 给 KPS 跟踪器喂养一次「距今 age_ms 的击键」（键雨截图用，制造层次）
    #[cfg(feature = "softshot")]
    #[allow(dead_code)]
    pub(crate) fn debug_push_kps_aged(&mut self, key: &str, age_ms: u64) {
        let age = Duration::from_millis(age_ms);
        self.shared.kps.lock().debug_push_key_aged(key, age);
    }

    /// 给 KPS 图喂一条「距今 age_ms 的历史动作」（截图铺历史柱用，不影响键格）
    #[cfg(feature = "softshot")]
    #[allow(dead_code)]
    pub(crate) fn debug_push_kps_hist(&mut self, age_ms: u64) {
        let age = Duration::from_millis(age_ms);
        self.shared.kps.lock().debug_push_action_aged(age);
    }

    /// 清空 KPS 时间轴（截图脚本每帧重建场景用）
    #[cfg(feature = "softshot")]
    #[allow(dead_code)]
    pub(crate) fn debug_reset_kps(&mut self) {
        self.shared.kps.lock().debug_reset_kps();
    }

    /// 直接改共享配置里的键雨/图开关（截图用；改 cfg_shared 才不会被每帧同步覆盖）
    /// 截图用：喂一段按压区间（按下距今 ms，松开距今 ms；None = 还按着）
    #[cfg(feature = "softshot")]
    #[allow(dead_code)]
    pub(crate) fn debug_push_hold(&mut self, key: &str, start_age_ms: u64, end_age_ms: Option<u64>) {
        self.shared
            .kps
            .lock()
            .debug_push_hold_aged(key, start_age_ms, end_age_ms);
    }

    #[cfg(feature = "softshot")]
    #[allow(dead_code)]
    pub(crate) fn debug_set_kps_rain(&mut self, show: bool, graph: bool, speed: u16) {
        let mut c = self.cfg_shared.lock();
        c.kps_show_rain = show;
        c.kps_show_graph = graph;
        c.kps_rain_speed = speed;
    }

    /// 截图用：直接改共享配置里的监控键（换行 = 分行），用来验证多层排布
    #[cfg(feature = "softshot")]
    #[allow(dead_code)]
    pub(crate) fn debug_set_kps_keys(&mut self, keys: &str) {
        self.cfg_shared.lock().kps_keys = keys.to_string();
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
        *self.apm_hud_size.lock()
    }

    /// 悬浮窗截图的目标尺寸与清屏色（测试用）：
    /// 尺寸是「贴合内容」的真值，要跑过一帧才能取到（APM 的宽度是实测文字得来的）。
    #[cfg(feature = "softshot")]
    #[allow(dead_code)]
    pub(crate) fn debug_hud_shot_spec(&self) -> ((f32, f32), Color32) {
        if self.hud_kps_show {
            let m = kps_hud_metrics(&self.cfg, &self.cfg.kps_key_layout());
            ((m.w, m.h), hud_bg_color((16, 18, 24), 100))
        } else {
            (*self.apm_hud_size.lock(), hud_bg_color((24, 26, 34), 100))
        }
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
    /// 键雨区高度（0 = 不显示键雨）
    pub rain_h: f32,
    /// 分组 KPS 行高度（0 = 不显示分组）
    pub group_h: f32,
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
    let rain_h = if cfg.kps_show_rain { super::rain::rain_area_h(cfg) } else { 0.0 };
    // 分组 KPS 行：只在「开了显示」且「配置里确实写了分组」时占高度
    let group_h = if cfg.kps_show_groups && !crate::stats::parse_kps_groups(&cfg.kps_groups).is_empty() {
        font_sub * 2.0
    } else {
        0.0
    };
    let min_graph_w = 200.0 * s;

    let content_w = keys_w.max(stats_w).max(if graph_h > 0.0 { min_graph_w } else { 0.0 }).max(unit);
    let mut h = pad * 2.0;
    let mut blocks = 0;
    // 键雨在**最上方**（雨从键格往上飘），所以先算它
    if rain_h > 0.0 {
        h += rain_h;
        blocks += 1;
    }
    if keys_h > 0.0 {
        // 键雨紧贴键格顶边（不加 v_gap），雨条的根部才落在键格边缘
        if blocks > 0 && rain_h <= 0.0 {
            h += v_gap;
        }
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
    if group_h > 0.0 {
        h += group_h;
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
        rain_h,
        group_h,
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

/// KDE/Wayland 下 KWin 监听脚本上报的最新 HUD 位置：`(x, y, 是否用户拖动过)`。
/// 其他平台恒为 None（winit 自己就能读位置）。
#[cfg(target_os = "linux")]
fn kde_hud_pos(idx: usize) -> Option<(f32, f32, bool)> {
    crate::kwin::hud_pos_reports()[idx].map(|r| (r.pos.0, r.pos.1, r.user_moved))
}
#[cfg(not(target_os = "linux"))]
fn kde_hud_pos(_idx: usize) -> Option<(f32, f32, bool)> {
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
/// KPS 图每根柱子的像素宽（**含 1px 间隔**）。柱数 = 图宽 / 柱宽，
/// 时间片 = 图时长 / 柱数（如 300 秒 / 84 根 ≈ 3.6 秒一根）。片内图形完全静止。
const GRAPH_BAR_W: f32 = 5.0;

/// 画 KPS 图：**固定像素宽的柱子**，柱数由图宽决定（渲染契约见
/// [`crate::stats::KpsTracker::history_bars`]）。
pub(crate) fn paint_kps_graph(
    ui: &mut egui::Ui,
    rect: Rect,
    hist: &[f32],
    avg: f32,
    accent: Color32,
) {
    let painter = ui.painter();
    painter.rect_filled(rect, 3.0, Color32::from_rgba_unmultiplied(10, 12, 18, 190));

    let maxv = nice_kps_max(hist.iter().copied().fold(0.0f32, f32::max)).max(1.0);
    let usable_h = (rect.height() - 2.0).max(1.0);
    // 柱子用暗色调、顶部 1.5px 用亮色勾"帽"；颜色不透明 —— 半透明柱会互相叠加（窗口一动就闪）
    let dim_accent = {
        let c = accent.to_array();
        Color32::from_rgb((c[0] as u32 * 3 / 5) as u8, (c[1] as u32 * 3 / 5) as u8, (c[2] as u32 * 3 / 5) as u8)
    };
    let body_w = (GRAPH_BAR_W - 1.0).max(1.0); // 柱体留 1px 当间隔，柱与柱界限分明
    for (i, v) in hist.iter().enumerate() {
        let h = (*v / maxv) * usable_h;
        if h <= 0.4 {
            continue;
        }
        let x = rect.left() + i as f32 * GRAPH_BAR_W;
        painter.rect_filled(
            Rect::from_min_max(Pos2::new(x, rect.bottom() - h), Pos2::new(x + body_w, rect.bottom())),
            0.0,
            dim_accent,
        );
        painter.rect_filled(
            Rect::from_min_max(
                Pos2::new(x, rect.bottom() - h),
                Pos2::new(
                    x + body_w,
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
    to.kps_show_rain = from.kps_show_rain;
    to.kps_rain_width_pct = from.kps_rain_width_pct;
    to.kps_rain_speed = from.kps_rain_speed;
    to.kps_rain_height = from.kps_rain_height;
    to.kps_rain_opacity = from.kps_rain_opacity;
    to.kps_rain_fade_pct = from.kps_rain_fade_pct;
    to.kps_rain_color = from.kps_rain_color;
    to.kps_key_bounce = from.kps_key_bounce;
    to.kps_groups = from.kps_groups.clone();
    to.kps_show_groups = from.kps_show_groups;
    to.apm_scale = from.apm_scale;
    to.apm_keep_in_screen = from.apm_keep_in_screen;
    to.kps_keep_in_screen = from.kps_keep_in_screen;
    to.apm_hud_pos = from.apm_hud_pos;
    to.kps_hud_pos = from.kps_hud_pos;
}

/// 悬浮窗背景色：深色底 + 可调不透明度（0~100；`rgb` 是底色 RGB）
/// 悬浮窗**设置窗自己**的背景不透明度：固定 100%。
///
/// 不要跟着 `cfg.*_hud_opacity` 走 —— 那个设置项管的是**悬浮窗本体**的透明度；
/// 设置窗跟着一起变透明，用户拖动"不透明度"时只会看到设置窗自己在变淡，
/// 既看不清也以为改错了对象（主人报过）。
/// 悬浮窗本体的透明度由 `render_*_hud` 里的 `hud_bg_color(..., cfg.*_hud_opacity)` 应用。
pub(crate) const SETTINGS_BG_OPACITY: u8 = 100;

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
            // 这整块是深色面板，但程序全局是浅色主题（主窗口用浅色）。
            // 不在这里把本 ui 的视觉切成深色的话，DragValue / ComboBox / 按钮会按浅色主题
            // 画成近乎白的浅灰底，而文字又被统一成白色 —— 白字浅底几乎看不见（用户反馈过两次）。
            // ui 级的 style 是 Arc 局部覆盖，不会漏到主窗口。
            {
                let v = ui.visuals_mut();
                *v = egui::Visuals::dark();
                v.override_text_color = Some(HUD_TEXT);
            }
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

