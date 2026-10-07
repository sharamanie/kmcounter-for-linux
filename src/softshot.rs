//! 无 GPU 环境下的软件光栅化出图（编译需 `--features softshot`）。
//!
//! 用途：在没有可用 GL 的机器/CI 上生成 README 配图，也可作为界面回归的像素级检查手段。
//! 做法：离屏跑 `gui::App::draw`（不依赖 eframe/窗口），把 egui 的三角形网格
//! 按“顶点色 × 字体图集覆盖率”混合到一张 RGBA 画布上——与 egui 官方 shader 的合成一致。
//!
//! 用法：
//! ```bash
//! KMCOUNTER_SHOT_STATS=/tmp/demo/stats.json cargo test --release --features softshot \
//!     -- --ignored --nocapture render_readme_shots
//! ```

#![allow(dead_code)] // 该模块只在 --features softshot 下编译，主要经 #[ignore] 测试驱动

use crate::config::Config;
use crate::gui::App;
use crate::stats::{Shared, Store};
use eframe::egui;
use egui::{Color32, Pos2, Rect, TextureId};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::Arc;

thread_local! {
    /// 每个线程一条单调递增的虚拟时间轴。
    /// egui 在 debug 构建里会断言 “Time shouldn't move backwards”（release 下被编译掉），
    /// 所以每次 `ctx.run` 的时间必须严格递增——CI 跑的是 debug 构建，离屏测试曾经就炸在这里。
    static FRAME_TIME: std::cell::Cell<f64> = const { std::cell::Cell::new(0.0) };
}

/// 下一帧的虚拟时间（+0.2s，保证单调）
pub(crate) fn next_time() -> f64 {
    FRAME_TIME.with(|t| {
        let v = t.get() + 0.2;
        t.set(v);
        v
    })
}

/// RGBA 画布（预乘 alpha，与 egui 顶点色一致）
struct Canvas {
    w: usize,
    h: usize,
    px: Vec<u8>,
}

impl Canvas {
    fn new(w: usize, h: usize, clear: Color32) -> Canvas {
        let [r, g, b, a] = clear.to_array();
        let mut px = Vec::with_capacity(w * h * 4);
        for _ in 0..w * h {
            px.extend_from_slice(&[r, g, b, a]);
        }
        Canvas { w, h, px }
    }

    /// 预乘 alpha 的 over 混合
    #[inline]
    fn blend(&mut self, x: usize, y: usize, src: [f32; 4]) {
        if x >= self.w || y >= self.h {
            return;
        }
        let sa = src[3];
        if sa <= 0.0 {
            return;
        }
        let i = (y * self.w + x) * 4;
        for c in 0..4 {
            let dst = self.px[i + c] as f32 / 255.0;
            let v = src[c] + dst * (1.0 - sa);
            self.px[i + c] = (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
        }
    }

    /// P6 PPM（零依赖，外部可用 ImageMagick 转 PNG）
    fn save_ppm(&self, path: &Path) -> std::io::Result<()> {
        use std::io::Write;
        let mut f = std::io::BufWriter::new(std::fs::File::create(path)?);
        write!(f, "P6\n{} {}\n255\n", self.w, self.h)?;
        let mut buf = Vec::with_capacity(self.w * self.h * 3);
        for px in self.px.chunks_exact(4) {
            // 画布整体不透明，直接把预乘色当直通色写出
            buf.extend_from_slice(&px[..3]);
        }
        f.write_all(&buf)?;
        f.flush()
    }
}

/// 纹理（字体图集）：RGBA 预乘
struct Tex {
    w: usize,
    h: usize,
    px: Vec<u8>,
}

impl Tex {
    fn sample(&self, u: f32, v: f32) -> [f32; 4] {
        // 双线性采样（图集边缘做钳制）
        let x = u * self.w as f32 - 0.5;
        let y = v * self.h as f32 - 0.5;
        let x0 = x.floor();
        let y0 = y.floor();
        let fx = x - x0;
        let fy = y - y0;
        let mut out = [0.0f32; 4];
        for (dx, dy, wgt) in [(0.0, 0.0, (1.0 - fx) * (1.0 - fy)), (1.0, 0.0, fx * (1.0 - fy)), (0.0, 1.0, (1.0 - fx) * fy), (1.0, 1.0, fx * fy)] {
            if wgt <= 0.0 {
                continue;
            }
            let xi = (x0 + dx).clamp(0.0, self.w as f32 - 1.0) as usize;
            let yi = (y0 + dy).clamp(0.0, self.h as f32 - 1.0) as usize;
            let i = (yi * self.w + xi) * 4;
            for c in 0..4 {
                out[c] += self.px[i + c] as f32 / 255.0 * wgt;
            }
        }
        out
    }
}

fn apply_textures(store: &mut HashMap<TextureId, Tex>, delta: &egui::TexturesDelta) {
    for (id, d) in &delta.set {
        let (w, h, px) = match &d.image {
            egui::ImageData::Color(img) => {
                let mut v = Vec::with_capacity(img.pixels.len() * 4);
                for p in &img.pixels {
                    v.extend_from_slice(&p.to_array());
                }
                (img.size[0], img.size[1], v)
            }
            egui::ImageData::Font(f) => {
                let mut v = Vec::with_capacity(f.pixels.len() * 4);
                for c in &f.pixels {
                    let b = (c.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
                    v.extend_from_slice(&[b, b, b, b]); // 白色 × 覆盖率（预乘）
                }
                (f.size[0], f.size[1], v)
            }
        };
        match d.pos {
            None => {
                store.insert(*id, Tex { w, h, px });
            }
            Some([ox, oy]) => {
                if let Some(t) = store.get_mut(id) {
                    for row in 0..h {
                        for col in 0..w {
                            if ox + col >= t.w || oy + row >= t.h {
                                continue;
                            }
                            let si = (row * w + col) * 4;
                            let di = ((oy + row) * t.w + ox + col) * 4;
                            t.px[di..di + 4].copy_from_slice(&px[si..si + 4]);
                        }
                    }
                }
            }
        }
    }
    for id in &delta.free {
        store.remove(id);
    }
}

#[inline]
fn edge(a: Pos2, b: Pos2, x: f32, y: f32) -> f32 {
    (x - a.x) * (b.y - a.y) - (y - a.y) * (b.x - a.x)
}

fn draw_mesh(canvas: &mut Canvas, tex: Option<&Tex>, mesh: &egui::Mesh, clip: Rect, ppp: f32) {
    let to_px = |p: Pos2| (p.x * ppp, p.y * ppp);
    let (cmin, cmax) = (to_px(clip.min), to_px(clip.max));
    let clip_px = Rect::from_min_max(Pos2::new(cmin.0, cmin.1), Pos2::new(cmax.0, cmax.1));
    for tri in mesh.indices.chunks_exact(3) {
        let vs = [
            mesh.vertices[tri[0] as usize],
            mesh.vertices[tri[1] as usize],
            mesh.vertices[tri[2] as usize],
        ];
        let p: [(f32, f32); 3] = [to_px(vs[0].pos), to_px(vs[1].pos), to_px(vs[2].pos)];
        // 与 edge() 同一套符号约定，保证三角形内部的三个权重同号
        let area = edge(Pos2::new(p[0].0, p[0].1), Pos2::new(p[1].0, p[1].1), p[2].0, p[2].1);
        if area.abs() < 1e-6 {
            continue;
        }
        let minx = p[0].0.min(p[1].0).min(p[2].0).max(clip_px.left()).max(0.0);
        let maxx = p[0].0.max(p[1].0).max(p[2].0).min(clip_px.right()).min(canvas.w as f32);
        let miny = p[0].1.min(p[1].1).min(p[2].1).max(clip_px.top()).max(0.0);
        let maxy = p[0].1.max(p[1].1).max(p[2].1).min(clip_px.bottom()).min(canvas.h as f32);
        if maxx <= minx || maxy <= miny {
            continue;
        }
        let cols = [
            vs[0].color.to_array(),
            vs[1].color.to_array(),
            vs[2].color.to_array(),
        ];
        for yi in miny.floor() as usize..(maxy.ceil() as usize).min(canvas.h) {
            for xi in minx.floor() as usize..(maxx.ceil() as usize).min(canvas.w) {
                let cx = xi as f32 + 0.5;
                let cy = yi as f32 + 0.5;
                let w0 = edge(Pos2::new(p[1].0, p[1].1), Pos2::new(p[2].0, p[2].1), cx, cy) / area;
                let w1 = edge(Pos2::new(p[2].0, p[2].1), Pos2::new(p[0].0, p[0].1), cx, cy) / area;
                let w2 = edge(Pos2::new(p[0].0, p[0].1), Pos2::new(p[1].0, p[1].1), cx, cy) / area;
                // 允许极小负值，避免相邻三角形之间的缝
                if w0 < -1e-3 || w1 < -1e-3 || w2 < -1e-3 {
                    continue;
                }
                let (w0, w1, w2) = (w0.max(0.0), w1.max(0.0), w2.max(0.0));
                let mut col = [0.0f32; 4];
                for c in 0..4 {
                    col[c] = (w0 * cols[0][c] as f32 + w1 * cols[1][c] as f32 + w2 * cols[2][c] as f32) / 255.0;
                }
                let uv = (
                    w0 * vs[0].uv.x + w1 * vs[1].uv.x + w2 * vs[2].uv.x,
                    w0 * vs[0].uv.y + w1 * vs[1].uv.y + w2 * vs[2].uv.y,
                );
                if let Some(t) = tex {
                    let s = t.sample(uv.0, uv.1);
                    for c in 0..4 {
                        col[c] *= s[c];
                    }
                }
                canvas.blend(xi, yi, col);
            }
        }
    }
}

/// 离屏渲染 App：先跑 `settle` 帧让布局稳定，再参考布局锚点确定画布高度，
/// 最后把目标帧光栅化到 PPM。
pub struct Shot {
    pub ppm: PathBuf,
    pub w: usize,
    pub h: usize,
}

#[allow(clippy::too_many_arguments)]
pub fn render(store: Store, cfg: Config, cfg_dir: &Path, out_prefix: &str, width: f32, ppp: f32) -> std::io::Result<Vec<Shot>> {
    let ctx = egui::Context::default();
    ctx.set_pixels_per_point(ppp);
    let shared = Shared::new(store, cfg.screen_w_mm.max(527), cfg.screen_h_mm.max(296));
    // 离屏渲染不启动输入捕获，别把“权限未就绪”的告警画进配图
    shared.set_input_status(true, String::new());
    shared.screen_px_w.store(crate::resolve_screen_px_w(&cfg), Ordering::Relaxed);
    let cfg_shared = Arc::new(parking_lot::Mutex::new(cfg.clone()));
    let mut app = App::new(
        &ctx,
        shared,
        cfg_shared,
        cfg_dir.join("config.toml"),
        cfg_dir.join("stats.json"),
        cfg_dir.join("show.request"),
        false,
        None,
        Arc::new(std::sync::atomic::AtomicBool::new(false)),
    );

    let mut textures: HashMap<TextureId, Tex> = HashMap::new();
    let mut last: Option<Vec<egui::ClippedPrimitive>> = None;
    let mut canvas_h = 0.0f32;

    for frame in 0..8 {
        // 前几帧给足高度，之后按布局锚点（内容高度 + 展开面板高度）定画布
        let h = if frame < 3 { 1400.0 } else { canvas_h };
        let mut raw = egui::RawInput::default();
        raw.screen_rect = Some(Rect::from_min_size(Pos2::ZERO, egui::vec2(width, h)));
        raw.time = Some(next_time());
        let out = ctx.run(raw, |ctx| app.draw(ctx));
        apply_textures(&mut textures, &out.textures_delta);
        let (base_h, panel_extra, content_bottom, bottom_top) = app.debug_layout();
        if panel_extra > 0.0 || base_h > 0.0 {
            canvas_h = (base_h + panel_extra).ceil();
        }
        if frame == 5 {
            eprintln!(
                "[geo] ==== 最终帧 canvas_h={canvas_h:.1} base_h={base_h:.1} panel_extra={panel_extra:.1} content_bottom={content_bottom:.1} bottom_top={bottom_top:.1} ===="
            );
        }
        last = Some(ctx.tessellate(out.shapes, ppp));
        // 面板高度需要两帧才稳定（首帧用估算值），第三帧起画布高度可靠
        if frame >= 5 {
            break;
        }
    }

    let prims = last.unwrap_or_default();
    {
        let mut tris = 0usize;
        let mut bb = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
        for p in &prims {
            if let egui::epaint::Primitive::Mesh(m) = &p.primitive {
                tris += m.indices.len() / 3;
                for v in &m.vertices {
                    bb.0 = bb.0.min(v.pos.x);
                    bb.1 = bb.1.min(v.pos.y);
                    bb.2 = bb.2.max(v.pos.x);
                    bb.3 = bb.3.max(v.pos.y);
                }
            }
        }
        eprintln!(
            "[softshot] 图元 {} 个 / 三角形 {} 个，内容包围盒 ({:.1},{:.1})-({:.1},{:.1})，画布 {}x{} @{}x",
            prims.len(),
            tris,
            bb.0,
            bb.1,
            bb.2,
            bb.3,
            width,
            canvas_h,
            ppp
        );
    }
    let mut canvas = Canvas::new((width * ppp).round() as usize, (canvas_h * ppp).round() as usize, crate::gui::BG);
    for prim in &prims {
        if let egui::epaint::Primitive::Mesh(mesh) = &prim.primitive {
            let tex = mesh.texture_id;
            draw_mesh(&mut canvas, textures.get(&tex), mesh, prim.clip_rect, ppp);
        }
    }
    let ppm = PathBuf::from(format!("{out_prefix}.ppm"));
    canvas.save_ppm(&ppm)?;
    Ok(vec![Shot { ppm, w: canvas.w, h: canvas.h }])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stats::MouseEv;

    /// 收集一帧里所有文字（galley），用来断言界面文案
    fn frame_texts(ctx: &egui::Context, app: &mut App, width: f32, height: f32) -> Vec<String> {
        let mut raw = egui::RawInput::default();
        raw.screen_rect = Some(Rect::from_min_size(Pos2::ZERO, egui::vec2(width, height)));
        raw.time = Some(next_time());
        let out = ctx.run(raw, |ctx| app.draw(ctx));
        let mut texts = Vec::new();
        let mut push_shape = |sh: &egui::Shape| {
            match sh {
                egui::Shape::Text(t) => texts.push(t.galley.text().to_string()),
                egui::Shape::Vec(v) => {
                    for s in v {
                        if let egui::Shape::Text(t) = s {
                            texts.push(t.galley.text().to_string());
                        }
                    }
                }
                _ => {}
            }
        };
        for cs in &out.shapes {
            push_shape(&cs.shape);
        }
        texts
    }

    /// 离屏跑若干帧；`events` 是最后一帧要注入的输入事件
    fn drive(app: &mut App, ctx: &egui::Context, width: f32, height: f32, frames: usize, events: Vec<egui::Event>) {
        for i in 0..frames {
            let mut raw = egui::RawInput::default();
            raw.screen_rect = Some(Rect::from_min_size(Pos2::ZERO, egui::vec2(width, height)));
            raw.time = Some(next_time());
            if i + 1 == frames {
                raw.events = events.clone();
            }
            let _out = ctx.run(raw, |ctx| app.draw(ctx));
        }
    }

    fn click_at(pos: Pos2, pressed: bool) -> egui::Event {
        egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::default(),
        }
    }

    fn headless_app(store: Store) -> (App, egui::Context) {
        let ctx = egui::Context::default();
        ctx.set_pixels_per_point(1.0);
        // 与真实程序一致：配置里的屏幕参数（tests 用默认配置）
        let cfg = match std::env::var("KMCOUNTER_SHOT_STATS") {
            Ok(p) => Config::load(&Path::new(&p).with_file_name("config.toml")).0,
            Err(_) => Config::default(),
        };
        let (mm_w, mm_h) = crate::resolve_screen_mm(&cfg);
        let shared = Shared::new(store, mm_w, mm_h);
        shared.set_input_status(true, String::new());
        shared.screen_px_w.store(crate::resolve_screen_px_w(&cfg), Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("kmc_ui_test_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let cfg_shared = Arc::new(parking_lot::Mutex::new(cfg));
        let app = App::new(
            &ctx,
            shared,
            cfg_shared,
            dir.join("config.toml"),
            dir.join("stats.json"),
            dir.join("show.request"),
            false,
            None,
            Arc::new(std::sync::atomic::AtomicBool::new(false)),
        );
        (app, ctx)
    }

    /// 阈值作废：界面要能看出来（提示行出现「已作废」，且该小时不再画热力）
    #[test]
    fn voided_hour_is_visible_in_ui() {
        // 9 时正常、10 时鼠标距离越限（阈值 1 km）
        let mut store = Store::default();
        let date = crate::stats::today_string();
        store.limits = crate::stats::HourLimits { move_px: 1000.0, clicks: 0, keystrokes: 0 };
        for _ in 0..60 {
            store.bump_key(&date, 9, 30);
        }
        store.bump_mouse(&date, 9, &MouseEv::MovePx(500.0));
        for _ in 0..60 {
            store.bump_key(&date, 10, 30);
        }
        for _ in 0..5 {
            store.bump_mouse(&date, 10, &MouseEv::MovePx(500.0)); // 2500 > 1000
        }
        assert!(store.hour(&date, 10).unwrap().invalid, "10 时应当已作废");
        assert!(!store.hour(&date, 9).unwrap().invalid);

        let (mut app, ctx) = headless_app(store);
        app.debug_set_hour(Some(10));
        drive(&mut app, &ctx, 1260.0, 900.0, 3, vec![]);
        let texts = frame_texts(&ctx, &mut app, 1260.0, 900.0);
        // 文案随语言变（CI 上 LANG=C 会回退英文），所以按文案表匹配而不是硬编码中文
        let invalid_mark = app.debug_strings().hour_invalid;
        assert!(
            texts.iter().any(|t| t.contains(invalid_mark.trim_end_matches('）').trim_end_matches(')'))),
            "提示行应说明该小时已作废：{texts:?}"
        );
        // 切到 9 时（未作废）就不该再有作废提示
        let (mut app9, ctx9) = headless_app({
            let mut s = Store::default();
            let d = crate::stats::today_string();
            for _ in 0..60 {
                s.bump_key(&d, 9, 30);
            }
            s.bump_mouse(&d, 9, &MouseEv::MovePx(500.0));
            s
        });
        app9.debug_set_hour(Some(9));
        drive(&mut app9, &ctx9, 1260.0, 900.0, 3, vec![]);
        let texts9 = frame_texts(&ctx9, &mut app9, 1260.0, 900.0);
        assert!(!texts9.iter().any(|t| t.contains(invalid_mark)), "9 时未作废，不应有提示");
    }

    /// 托盘呼出 → 视图切回今日；日历里点某天 → 直接切过去（没数据的天不生效）
    #[test]
    fn show_today_and_goto_day() {
        // 造三天数据：今天、前天、以及一个月前
        let mut store = Store::default();
        let today = chrono::Local::now().date_naive();
        let days = [
            today,
            today - chrono::Duration::days(2),
            today - chrono::Duration::days(40),
        ];
        for d in days {
            let key = d.format("%Y%m%d").to_string();
            for _ in 0..120 {
                store.bump_key(&key, 10, 30);
            }
        }
        let (mut app, ctx) = headless_app(store);
        drive(&mut app, &ctx, 1260.0, 900.0, 3, vec![]);
        assert_eq!(app.debug_view_label(), app.debug_strings().total_label, "初始视图是总计");

        // 托盘呼出标志 → 下一帧切到今日
        app.debug_flags().show_today.store(true, Ordering::Relaxed);
        drive(&mut app, &ctx, 1260.0, 900.0, 1, vec![]);
        assert_eq!(app.debug_view_label(), today.format("%Y%m%d").to_string());

        // 日历点某天（有数据 → 生效；没数据 → 不动）
        let older = (today - chrono::Duration::days(40)).format("%Y%m%d").to_string();
        assert!(app.debug_goto_day(&older));
        assert_eq!(app.debug_view_label(), older);
        assert!(!app.debug_goto_day("19990101"), "没数据的日子不该跳过去");
        assert_eq!(app.debug_view_label(), older);
    }

    /// 日历弹窗：打开后能看到月份、星期表头与「今天/总计」入口，且默认收起
    #[test]
    fn calendar_popup_renders() {
        let mut store = Store::default();
        let today = chrono::Local::now().date_naive();
        for d in [today, today - chrono::Duration::days(1)] {
            let key = d.format("%Y%m%d").to_string();
            for _ in 0..150 {
                store.bump_key(&key, 12, 30);
            }
        }
        let (mut app, ctx) = headless_app(store);
        drive(&mut app, &ctx, 1260.0, 900.0, 3, vec![]);
        assert!(!app.debug_calendar_open(), "默认不开日历");

        // 打开日历（等同点日期标题）
        let (mut app2, ctx2) = headless_app({
            let mut s = Store::default();
            let key = today.format("%Y%m%d").to_string();
            for _ in 0..150 {
                s.bump_key(&key, 12, 30);
            }
            s
        });
        app2.debug_open_calendar();
        drive(&mut app2, &ctx2, 1260.0, 900.0, 3, vec![]);
        assert!(app2.debug_calendar_open());
        let texts = frame_texts(&ctx2, &mut app2, 1260.0, 900.0);
        let month = today.format("%Y / %m").to_string();
        assert!(texts.iter().any(|t| t == &month), "应显示月份 {month}：{texts:?}");
        for wd in app.debug_strings().weekdays.split(' ') {
            assert!(texts.iter().any(|t| t == wd), "缺少星期表头 {wd}");
        }
        for want in [app.debug_strings().cal_today, app.debug_strings().total_label] {
            assert!(texts.iter().any(|t| t == want), "缺少「{want}」入口");
        }
        // 日期格子：本月 1 号与今天的日号都应出现
        assert!(texts.iter().any(|t| t.trim() == "1"), "缺少年历格子");
        assert!(texts.iter().any(|t| t.trim() == today.format("%d").to_string().trim_start_matches('0') || t.trim() == today.format("%d").to_string()));
    }

    /// 趋势页的日期导航要和热力图页一样：滚轮/上下键翻日期、点标题开日历
    #[test]
    fn trend_page_nav_row_works_like_heatmap() {
        let mut store = Store::default();
        let today = chrono::Local::now().date_naive();
        for d in [today, today - chrono::Duration::days(1), today - chrono::Duration::days(2)] {
            let key = d.format("%Y%m%d").to_string();
            for _ in 0..150 {
                store.bump_key(&key, 12, 30);
            }
            store.bump_mouse(&key, 12, &MouseEv::MovePx(5000.0));
        }
        let (mut app, ctx) = headless_app(store);
        app.debug_set_trend_page();
        drive(&mut app, &ctx, 1260.0, 900.0, 4, vec![]);
        let total = app.debug_view_label();
        assert_ne!(total, today.format("%Y%m%d").to_string(), "初始应停在「{}」视图", total);

        // ① 滚轮翻页（指针在图表区域，不在底部面板）
        let in_chart = Pos2::new(600.0, 260.0);
        let wheel = |dy: f32| {
            vec![
                egui::Event::PointerMoved(in_chart),
                egui::Event::MouseWheel {
                    unit: egui::MouseWheelUnit::Point,
                    delta: egui::vec2(0.0, dy),
                    modifiers: egui::Modifiers::default(),
                },
            ]
        };
        drive(&mut app, &ctx, 1260.0, 900.0, 1, wheel(-40.0));
        drive(&mut app, &ctx, 1260.0, 900.0, 1, vec![]);
        let today_key = today.format("%Y%m%d").to_string();
        assert_eq!(app.debug_view_label(), today_key, "趋势页向下滚应翻到最近一天");
        // 曲线跟着切：日粒度横轴末端就是这一天
        let texts = frame_texts(&ctx, &mut app, 1260.0, 900.0);
        let last_label = today.format("%m-%d").to_string();
        assert!(texts.iter().any(|t| t == &last_label), "趋势横轴应出现 {last_label}：{texts:?}");
        drive(&mut app, &ctx, 1260.0, 900.0, 1, wheel(40.0));
        drive(&mut app, &ctx, 1260.0, 900.0, 1, vec![]);
        assert_eq!(app.debug_view_label(), total, "趋势页向上滚应回到「{total}」");

        // ② 上下键翻页
        drive(&mut app, &ctx, 1260.0, 900.0, 1, vec![egui::Event::Key {
            key: egui::Key::ArrowDown,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::default(),
        }]);
        drive(&mut app, &ctx, 1260.0, 900.0, 1, vec![]);
        assert_eq!(app.debug_view_label(), today_key, "趋势页按 ↓ 应翻到最近一天");

        // ③ 点日期标题 → 日历真的画出来（与热力图页同一条路径）
        let title = app.debug_nav_title_rect();
        assert!(title.width() > 100.0, "导航行日期标题应有固定宽度: {title:?}");
        let p = title.center();
        drive(&mut app, &ctx, 1260.0, 900.0, 1, vec![egui::Event::PointerMoved(p), click_at(p, true)]);
        drive(&mut app, &ctx, 1260.0, 900.0, 1, vec![click_at(p, false)]);
        assert!(app.debug_calendar_open(), "点趋势页的日期标题应打开日历");
        let texts = frame_texts(&ctx, &mut app, 1260.0, 900.0);
        let month = today.format("%Y / %m").to_string();
        assert!(texts.iter().any(|t| t == &month), "趋势页日历应显示月份 {month}");
        for want in [app.debug_strings().cal_today, app.debug_strings().total_label] {
            assert!(texts.iter().any(|t| t == want), "趋势页日历缺少「{want}」入口");
        }
        // 再点一次标题 → 收起
        let p2 = app.debug_nav_title_rect().center();
        drive(&mut app, &ctx, 1260.0, 900.0, 1, vec![egui::Event::PointerMoved(p2), click_at(p2, true)]);
        drive(&mut app, &ctx, 1260.0, 900.0, 1, vec![click_at(p2, false)]);
        assert!(!app.debug_calendar_open(), "再点一次标题应收起日历");
    }

    /// 造一段「连续 N 天都有数据」的历史，方便验证按周/月/年翻页
    fn store_with_days(n: i64) -> Store {
        let mut store = Store::default();
        let today = chrono::Local::now().date_naive();
        for k in 0..n {
            let key = (today - chrono::Duration::days(k)).format("%Y%m%d").to_string();
            for _ in 0..120 {
                store.bump_key(&key, 12, 5);
            }
            store.bump_mouse(&key, 12, &MouseEv::MovePx(5_000.0));
        }
        store
    }

    /// 趋势页滚轮翻页要跟着粒度走：周/月/年粒度滚一格就是一周/一月/一年
    #[test]
    fn trend_wheel_steps_by_granularity() {
        let today = chrono::Local::now().date_naive();
        let key = |d: chrono::NaiveDate| d.format("%Y%m%d").to_string();
        for (gran, step_days, use_months) in [
            (crate::trend::Granularity::Daily, 1i64, false),
            (crate::trend::Granularity::Weekly, 7, false),
            (crate::trend::Granularity::Monthly, 0, true),
            (crate::trend::Granularity::Yearly, 0, true),
        ] {
            let (mut app, ctx) = headless_app(store_with_days(500));
            app.debug_set_trend_page();
            app.debug_goto_day(&key(today));
            app.debug_set_granularity(gran);
            drive(&mut app, &ctx, 1260.0, 900.0, 4, vec![]);
            assert_eq!(app.debug_view_label(), key(today), "起点应是今天");

            // 向下滚一格（往更早翻）
            let in_chart = Pos2::new(600.0, 260.0);
            let wheel = vec![
                egui::Event::PointerMoved(in_chart),
                egui::Event::MouseWheel {
                    unit: egui::MouseWheelUnit::Point,
                    delta: egui::vec2(0.0, -40.0),
                    modifiers: egui::Modifiers::default(),
                },
            ];
            drive(&mut app, &ctx, 1260.0, 900.0, 1, wheel);
            drive(&mut app, &ctx, 1260.0, 900.0, 1, vec![]);
            let expect = if use_months {
                let n = if gran == crate::trend::Granularity::Monthly { 1 } else { 12 };
                today.checked_sub_months(chrono::Months::new(n)).unwrap()
            } else {
                today - chrono::Duration::days(step_days)
            };
            assert_eq!(
                app.debug_view_label(),
                key(expect),
                "{gran:?} 粒度下滚一格应到 {expect}（按 {:?} 为单位）",
                gran
            );
        }
    }

    /// 左键点趋势图上的点 → 直接跳到那一天（周/月/年粒度跳到该桶里有数据的那天）
    #[test]
    fn clicking_chart_jumps_to_that_day() {
        let today = chrono::Local::now().date_naive();
        let key = |d: chrono::NaiveDate| d.format("%Y%m%d").to_string();
        let (mut app, ctx) = headless_app(store_with_days(60));
        app.debug_set_trend_page();
        drive(&mut app, &ctx, 1260.0, 900.0, 4, vec![]);

        // 日粒度：60 天数据 → 60 个桶，点第 10 个桶应跳到 today-49
        let rect = app.debug_chart_rect();
        assert!(rect.width() > 800.0, "趋势图区域异常: {rect:?}");
        let n = 60usize;
        let idx = 10usize;
        let x = crate::chart::bucket_center_x(rect, n, idx);
        let p = Pos2::new(x, rect.center().y);
        drive(&mut app, &ctx, 1260.0, 900.0, 1, vec![egui::Event::PointerMoved(p), click_at(p, true)]);
        drive(&mut app, &ctx, 1260.0, 900.0, 1, vec![click_at(p, false)]);
        let want = today - chrono::Duration::days((n - 1 - idx) as i64);
        assert_eq!(app.debug_view_label(), key(want), "点第 {idx} 个桶应跳到 {want}");

        // 周粒度：点某个周桶应跳到「那一周里有数据的那天」
        app.debug_goto_day(&key(today)); // 回到今天（上一步把视图跳到了 49 天前）
        app.debug_set_granularity(crate::trend::Granularity::Weekly);
        drive(&mut app, &ctx, 1260.0, 900.0, 2, vec![]);
        let rect = app.debug_chart_rect();
        let first = today - chrono::Duration::days(59);
        let bs = |d| crate::trend::Granularity::Weekly.bucket_start(d);
        let n_weeks = ((bs(today) - bs(first)).num_days() / 7 + 1) as usize;
        let widx = n_weeks - 2; // 倒数第二个周桶
        let x = crate::chart::bucket_center_x(rect, n_weeks, widx);
        let p = Pos2::new(x, rect.center().y);
        drive(&mut app, &ctx, 1260.0, 900.0, 1, vec![egui::Event::PointerMoved(p), click_at(p, true)]);
        drive(&mut app, &ctx, 1260.0, 900.0, 1, vec![click_at(p, false)]);
        let got = chrono::NaiveDate::parse_from_str(&app.debug_view_label(), "%Y%m%d").expect("周粒度点击后应跳到某一天");
        let bucket_start = bs(today) - chrono::Duration::days(7 * (n_weeks - 1 - widx) as i64);
        assert!(
            got >= bucket_start && got <= bucket_start + chrono::Duration::days(6),
            "点第 {widx}/{n_weeks} 个周桶应落在 {bucket_start} 那一周内，实际 {got}"
        );
    }

    /// 调试用：按 y 顺序打印一帧里的所有文案（排查设置面板排版）
    #[test]
    #[ignore]
    fn dump_frame_texts() {
        std::env::set_var(
            "KMCOUNTER_START_PANELS",
            std::env::var("KMCOUNTER_DUMP_PANELS").unwrap_or_else(|_| "both".into()),
        );
        // 有 KMCOUNTER_SHOT_STATS 时用真实/演示数据，便于核对界面数字
        let store = match std::env::var("KMCOUNTER_SHOT_STATS") {
            Ok(p) => Store::load(Path::new(&p)),
            Err(_) => Store::default(),
        };
        let (mut app, ctx) = headless_app(store);
        std::env::remove_var("KMCOUNTER_START_PANELS");
        if let Ok(code) = std::env::var("KMCOUNTER_DUMP_LANG") {
            app.debug_set_language(&code);
        }
        drive(&mut app, &ctx, 1260.0, 1400.0, 3, vec![]);
        let mut raw = egui::RawInput::default();
        raw.screen_rect = Some(Rect::from_min_size(Pos2::ZERO, egui::vec2(1260.0, 1400.0)));
        raw.time = Some(next_time());
        let out = ctx.run(raw, |ctx| app.draw(ctx));
        let mut items: Vec<(f32, f32, String)> = Vec::new();
        let mut walk = |sh: &egui::Shape| match sh {
            egui::Shape::Text(t) => items.push((t.pos.y, t.pos.x, t.galley.text().to_string())),
            egui::Shape::Vec(v) => {
                for s in v {
                    if let egui::Shape::Text(t) = s {
                        items.push((t.pos.y, t.pos.x, t.galley.text().to_string()));
                    }
                }
            }
            _ => {}
        };
        for cs in &out.shapes {
            walk(&cs.shape);
        }
        items.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
        let min_y = std::env::var("KMCOUNTER_DUMP_FROM").ok().and_then(|v| v.parse().ok()).unwrap_or(0.0);
        for (y, x, t) in items.iter().filter(|(y, _, _)| *y >= min_y) {
            println!("[text] y={y:7.1} x={x:7.1} {t}");
        }
    }

    /// 指定颜色、落在 `area` 内的折线点位（核对折线是否真有起伏）
    fn frame_path_points_of(
        ctx: &egui::Context,
        app: &mut App,
        width: f32,
        height: f32,
        area: Rect,
        want: Color32,
    ) -> Vec<Pos2> {
        let mut raw = egui::RawInput::default();
        raw.screen_rect = Some(Rect::from_min_size(Pos2::ZERO, egui::vec2(width, height)));
        raw.time = Some(next_time());
        let out = ctx.run(raw, |ctx| app.draw(ctx));
        let mut pts: Vec<Pos2> = Vec::new();
        let mut push = |sh: &egui::Shape| {
            if let egui::Shape::Path(p) = sh {
                if let egui::epaint::ColorMode::Solid(c) = p.stroke.color {
                    if c == want && p.points.iter().any(|q| area.contains(*q)) {
                        pts.extend(p.points.iter().copied());
                    }
                }
            }
        };
        for cs in &out.shapes {
            push(&cs.shape);
            if let egui::Shape::Vec(v) = &cs.shape {
                for s in v {
                    push(s);
                }
            }
        }
        pts
    }

    /// 一帧里所有矩形的 (位置, 填充色)——用来核对热力配色
    fn frame_rect_fills(ctx: &egui::Context, app: &mut App, width: f32, height: f32) -> Vec<(Rect, Color32)> {
        let mut raw = egui::RawInput::default();
        raw.screen_rect = Some(Rect::from_min_size(Pos2::ZERO, egui::vec2(width, height)));
        raw.time = Some(next_time());
        let out = ctx.run(raw, |ctx| app.draw(ctx));
        let mut out_rects: Vec<(Rect, Color32)> = Vec::new();
        let mut push = |sh: &egui::Shape| {
            if let egui::Shape::Rect(r) = sh {
                out_rects.push((r.rect, r.fill));
            }
        };
        for cs in &out.shapes {
            push(&cs.shape);
            if let egui::Shape::Vec(v) = &cs.shape {
                for s in v {
                    push(s);
                }
            }
        }
        out_rects
    }

    /// 一帧里落在 `area` 内的折线颜色（趋势图三条曲线用）
    fn frame_path_colors(ctx: &egui::Context, app: &mut App, width: f32, height: f32, area: Rect) -> Vec<Color32> {
        let mut raw = egui::RawInput::default();
        raw.screen_rect = Some(Rect::from_min_size(Pos2::ZERO, egui::vec2(width, height)));
        raw.time = Some(next_time());
        let out = ctx.run(raw, |ctx| app.draw(ctx));
        let mut colors: Vec<Color32> = Vec::new();
        let mut push = |sh: &egui::Shape| {
            if let egui::Shape::Path(p) = sh {
                // egui 0.29 的描边颜色是 ColorMode（通常为 Solid）
                if let egui::epaint::ColorMode::Solid(c) = p.stroke.color {
                    if p.points.iter().any(|q| area.contains(*q)) && c != Color32::TRANSPARENT {
                        colors.push(c);
                    }
                }
            }
        };
        for cs in &out.shapes {
            push(&cs.shape);
            if let egui::Shape::Vec(v) = &cs.shape {
                for s in v {
                    push(s);
                }
            }
        }
        colors
    }

    /// 一帧里所有文字及其**视觉**位置（排查“标签 - 数值”对应关系用）。
    /// 返回 `(行中心 y, 左边缘 x, 宽度, 文字)`：右对齐的标签 `TextShape.pos.x` 是右边缘，
    /// 直接用 `pos` 会算错，所以这里取 galley 的 mesh_bounds。
    fn frame_texts_pos(ctx: &egui::Context, app: &mut App, width: f32, height: f32) -> Vec<(f32, f32, f32, String)> {
        let mut raw = egui::RawInput::default();
        raw.screen_rect = Some(Rect::from_min_size(Pos2::ZERO, egui::vec2(width, height)));
        raw.time = Some(next_time());
        let out = ctx.run(raw, |ctx| app.draw(ctx));
        let mut items: Vec<(f32, f32, f32, String)> = Vec::new();
        let mut push = |t: &egui::epaint::TextShape| {
            let b = t.galley.mesh_bounds.translate(t.pos.to_vec2());
            // 空文本（例如统计表里占位的空格子）没有网格包围盒，退回 pos
            let (y, left, w) = if b.is_finite() {
                (b.center().y, b.left(), b.width())
            } else {
                (t.pos.y, t.pos.x, 0.0)
            };
            items.push((y, left, w, t.galley.text().to_string()));
        };
        let mut walk = |sh: &egui::Shape| match sh {
            egui::Shape::Text(t) => push(t),
            egui::Shape::Vec(v) => {
                for s in v {
                    if let egui::Shape::Text(t) = s {
                        push(t);
                    }
                }
            }
            _ => {}
        };
        for cs in &out.shapes {
            walk(&cs.shape);
        }
        items.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
        items
    }

    /// 补上的三个系统键要画在导航区顶部；统计表要按四个键盘分区统计（键盘敲击的细分项）
    #[test]
    fn system_keys_drawn_and_zones_counted() {
        let idx = |n: &str| crate::keys::idx_by_name(n).unwrap();
        let d = crate::stats::today_string();
        let mut store = Store::default();
        let counts = [("Space", 5u64), ("F1", 3), ("Insert", 2), ("Num1", 4), ("PrtScr", 7)];
        for (name, n) in counts {
            for _ in 0..n {
                store.bump_key(&d, 12, idx(name));
            }
        }
        store.bump_other_key(&d, 12, "win:822"); // 布局外按键：不计入任何分区

        let (mut app, ctx) = headless_app(store);
        app.debug_open_panel();
        drive(&mut app, &ctx, 1260.0, 1400.0, 4, vec![]);

        // ① 热力图上画出三个新键
        let texts = frame_texts(&ctx, &mut app, 1260.0, 1400.0);
        for want in ["PrtSc", "ScrLk", "Pause"] {
            assert!(texts.iter().any(|t| t == want), "热力图缺少新键「{want}」");
        }
        // ② 分布条只剩数字键盘上方那一块（导航区顶部被新键占用了）
        let strip = app.debug_strip_rect();
        assert!(strip.left() > 900.0, "分布条应移到数字键盘上方: {strip:?}");
        assert!(strip.width() < 260.0, "分布条应只剩一个数字键盘宽: {strip:?}");

        // ③ 统计表：四个分区行，数值 = 该分区各键之和（总计视图只有一列数字）
        let items = frame_texts_pos(&ctx, &mut app, 1260.0, 1400.0);
        let value_of = |label: &str| -> Option<u64> {
            let (ly, lx) = items.iter().find(|(_, _, _, t)| t == label).map(|(y, x, _, _)| (*y, *x))?;
            items
                .iter()
                .filter(|(y, x, _, t)| (y - ly).abs() < 4.0 && *x > lx + 10.0 && t.parse::<u64>().is_ok())
                .map(|(_, _, _, t)| t.parse::<u64>().unwrap())
                .next()
        };
        let s = app.debug_strings();
        // 「退出」只在设置面板里出现（底部按钮行不再放，避免误触）
        let items2 = frame_texts_pos(&ctx, &mut app, 1260.0, 1400.0);
        let exits: Vec<f32> = items2
            .iter()
            .filter(|(_, _, _, t)| t == s.menu_exit)
            .map(|(_, x, _, _)| *x)
            .collect();
        assert_eq!(exits.len(), 1, "「退出」应只出现一次（设置面板里）：{items2:?}");
        // 统计列宽 468.7（x 8..476.7），设置面板在它右边；旧的底部按钮行在 x≈98
        assert!(exits[0] > 480.0, "「退出」应在设置面板里（不是底部按钮行），实际 x={}", exits[0]);
        assert_eq!(value_of(s.zone_main), Some(5), "主键盘区应为 Space 的 5 次");
        assert_eq!(value_of(s.zone_function), Some(3), "功能键区应为 F1 的 3 次");
        assert_eq!(value_of(s.zone_control), Some(9), "控制键区应为 Insert + PrtScr = 9 次");
        assert_eq!(value_of(s.zone_numpad), Some(4), "数字键区应为 Num1 的 4 次");
        assert_eq!(value_of(s.keystrokes), Some(22), "键盘敲击合计（含布局外的 1 次）");

        // ④ 分区行是「键盘敲击」的细分项：排在它下面，且比它缩进
        let y_of = |label: &str| items.iter().find(|(_, _, _, t)| t == label).map(|(y, _, _, _)| *y).unwrap();
        let x_of = |label: &str| items.iter().find(|(_, _, _, t)| t == label).map(|(_, x, _, _)| *x).unwrap();
        let ks_y = y_of(s.keystrokes);
        for z in [s.zone_main, s.zone_function, s.zone_control, s.zone_numpad] {
            assert!(y_of(z) > ks_y, "「{z}」应排在「{}」下方", s.keystrokes);
            assert!(x_of(z) > x_of(s.keystrokes), "「{z}」应缩进显示");
        }
    }

    /// 方向键上方那 5 个鼠标热力键：画得出来、悬停有数值；分时折线悬停显示三个数据
    #[test]
    fn mouse_keys_and_hour_lines_render() {
        let d = crate::stats::today_string();
        let mut store = Store::default();
        for h in [3u8, 9, 15] {
            for _ in 0..120 {
                store.bump_key(&d, h, 5);
            }
            store.bump_mouse(&d, h, &MouseEv::LeftUp);
            store.bump_mouse(&d, h, &MouseEv::RightUp);
            store.bump_mouse(&d, h, &MouseEv::Wheel);
            store.bump_mouse(&d, h, &MouseEv::MovePx(4_000.0));
        }
        let (mut app, ctx) = headless_app(store);
        drive(&mut app, &ctx, 1260.0, 900.0, 3, vec![]);

        // ① 5 个鼠标键的短标签都画出来了
        let texts = frame_texts(&ctx, &mut app, 1260.0, 900.0);
        let s = app.debug_strings();
        for want in [s.mkey_l, s.mkey_m, s.mkey_r, s.mkey_wheel, s.mkey_side] {
            assert!(texts.iter().any(|t| t == want), "键盘上应画出鼠标键「{want}」：{texts:?}");
        }

        // ② 悬停分时折线：提示里同时有三个数据（键盘 / 鼠标按键 / 鼠标移动）
        let strip = app.debug_strip_rect();
        let p = Pos2::new(strip.left() + strip.width() * (15.5 / 24.0), strip.center().y);
        drive(&mut app, &ctx, 1260.0, 900.0, 1, vec![egui::Event::PointerMoved(p)]);
        let texts = frame_texts(&ctx, &mut app, 1260.0, 900.0);
        let tip = texts.iter().find(|t| {
            t.contains(s.keystrokes) && t.contains(s.mouse_clicks) && t.contains(s.mouse_move)
        });
        assert!(tip.is_some(), "悬停小时应同时显示三个数值：{texts:?}");
        let tip = tip.unwrap();
        // 三个数值按「键盘 / 鼠标按键 / 鼠标移动」的顺序出现，且各带着自己的数字与单位
        let after_keys = tip.split(s.keystrokes).nth(1).unwrap_or("");
        assert!(after_keys.trim_start().starts_with("120"), "键盘敲击应为 120：{tip}");
        let after_clicks = tip.split(s.mouse_clicks).nth(1).unwrap_or("");
        assert!(after_clicks.trim_start().starts_with('2'), "鼠标按键应为 2（左+右，滚轮不算）：{tip}");
        let after_move = tip.split(s.mouse_move).nth(1).unwrap_or("");
        assert!(after_move.contains(s.unit_m.trim()), "鼠标移动应带单位：{tip}");

        // ③ 悬停鼠标键（滚轮那一格）：显示该键的计数
        let rect = app.debug_kbd_rect();
        let (u, pitch, gap) = app.debug_metrics();
        let mk = Pos2::new(rect.left() + (crate::keys::MAIN_UNITS * u + gap) + 0.5 * u, rect.top() + 4.5 * pitch);
        drive(&mut app, &ctx, 1260.0, 900.0, 1, vec![egui::Event::PointerMoved(mk)]);
        let texts = frame_texts(&ctx, &mut app, 1260.0, 900.0);
        assert!(texts.iter().any(|t| t.contains(s.mkey_wheel) && t.contains('3')), "滚轮键悬停应显示计数：{texts:?}");

        // ④ 配色：分时折线三条线都在（红/绿/蓝）
        let strip = app.debug_strip_rect();
        let colors = frame_path_colors(&ctx, &mut app, 1260.0, 900.0, strip);
        for (name, c) in [
            ("键盘(红)", crate::gui::KEY_COLOR),
            ("鼠标按键(绿)", crate::gui::CLICK_COLOR),
            ("鼠标移动(蓝)", crate::gui::MOUSE_COLOR),
        ] {
            assert!(colors.contains(&c), "分时折线缺少{name}：{colors:?}");
            // 折线要有起伏：说明确实画的是这一天各小时的数据，而不是一条压平的直线
            let pts = frame_path_points_of(&ctx, &mut app, 1260.0, 900.0, strip, c);
            let ys: Vec<f32> = pts.iter().map(|p| p.y).collect();
            let span = ys.iter().cloned().fold(f32::MIN, f32::max) - ys.iter().cloned().fold(f32::MAX, f32::min);
            assert!(span > 4.0, "{name} 折线应随小时变化（实际高度差 {span:.1}px）");
        }

        // ⑤ 鼠标五键用绿色系、键盘用红色系（同一帧里各自的热力色）
        let fills = frame_rect_fills(&ctx, &mut app, 1260.0, 900.0);
        let fill_of = |r: Rect| fills.iter().find(|(rr, _)| *rr == r).map(|(_, c)| *c);
        let kbd = app.debug_kbd_rect();
        let (u, pitch, gap) = app.debug_metrics();
        let nav_x = crate::keys::MAIN_UNITS * u + gap;
        let spacing = 2.0_f32; // 测试用默认配置（key_spacing = 2）
        let key_rect_at = |x: f32, y: f32| {
            Rect::from_min_size(
                Pos2::new(kbd.left() + nav_x + x * u, kbd.top() + y * pitch),
                egui::vec2(u - spacing, pitch - spacing),
            )
        };
        let m_l = fill_of(key_rect_at(0.0, 3.0)).expect("左键那一格应有底色");
        assert!(m_l.g() >= m_l.r(), "鼠标左键热力应是绿色系：{m_l:?}");
        let m_wheel = fill_of(key_rect_at(0.0, 4.0)).expect("滚轮那一格应有底色");
        assert!(m_wheel.g() >= m_wheel.r(), "滚轮热力应是绿色系：{m_wheel:?}");
        // 键盘里最常按的键（演示数据里是 Space 附近）应是红色系：取键盘区里颜色最深的一格
        // 只看主键区（导航区那 5 格是鼠标键，属于绿色池）
        let main_right = kbd.left() + crate::keys::MAIN_UNITS * u;
        let kbd_keys: Vec<Color32> = fills
            .iter()
            .filter(|(r, c)| {
                kbd.contains(r.center())
                    && r.center().x < main_right
                    && r.width() > 20.0
                    && r.height() > 20.0
                    && c.a() == 255
            })
            .map(|(_, c)| *c)
            .collect();
        let deepest = kbd_keys
            .into_iter()
            .min_by_key(|c| (c.r() as u32 + c.g() as u32 + c.b() as u32))
            .expect("键盘区应有按键底色");
        assert!(deepest.r() > deepest.g(), "键盘最深的一格应是红色系：{deepest:?}");
    }

    /// 排行页：列出名次、点某一行跳到热力图并选中该键
    #[test]
    fn rank_page_lists_and_jumps() {
        let d = crate::stats::today_string();
        let mut store = Store::default();
        let space = crate::keys::idx_by_name("Space").unwrap();
        let f1 = crate::keys::idx_by_name("F1").unwrap();
        for _ in 0..30 {
            store.bump_key(&d, 12, space);
        }
        for _ in 0..10 {
            store.bump_key(&d, 12, f1);
        }
        let (mut app, ctx) = headless_app(store);
        app.debug_open_rank_page();
        app.debug_set_rank(
            crate::gui::RankRange::Day,
            &crate::gui::ALL_SCOPES,
            crate::gui::RankMetric::Count,
            true,
        );
        drive(&mut app, &ctx, 1260.0, 900.0, 4, vec![]);

        // 聚合结果：Space 30 第一、F1 10 第二
        let rows = app.debug_rank_rows();
        assert_eq!(rows[0].0, "Space");
        assert_eq!(rows[0].1, 30);
        assert_eq!(rows[1].0, "F1");
        assert_eq!(rows[1].1, 10);

        // 界面：页签、键名、次数都在
        let texts = frame_texts(&ctx, &mut app, 1260.0, 900.0);
        let s = app.debug_strings();
        assert!(texts.iter().any(|t| t == s.tab_rank), "应有「排行」页签：{texts:?}");
        assert!(texts.iter().any(|t| t == "Space"), "列表里应有 Space");
        assert!(texts.iter().any(|t| t == "30"), "应显示次数 30");

        // 点第二行（F1）→ 跳到热力图并选中 F1
        let list = app.debug_rank_rect();
        assert!(list.height() > 20.0, "排行列表应有可见高度: {list:?}");
        let p = Pos2::new(list.left() + 60.0, list.top() + crate::gui::RANK_ROW_H * 1.5);
        drive(&mut app, &ctx, 1260.0, 900.0, 1, vec![egui::Event::PointerMoved(p), click_at(p, true)]);
        drive(&mut app, &ctx, 1260.0, 900.0, 1, vec![click_at(p, false)]);
        assert!(!app.debug_page_is_rank(), "点行后应跳到热力图页");
        assert_eq!(app.debug_selected(), vec![f1], "应选中被点击的那一行对应的键");
    }

    /// 「当前显示数据」在当周/当月/当年（以及排行页的非单日区间）下显示成起止区间
    #[test]
    fn nav_label_shows_range_for_periods() {
        use chrono::Datelike;
        let today = chrono::Local::now().date_naive();
        let (mut app, ctx) = headless_app(store_with_days(400));
        app.debug_set_trend_page();
        app.debug_goto_day(&today.format("%Y%m%d").to_string());
        drive(&mut app, &ctx, 1260.0, 900.0, 3, vec![]);

        // 每小时 / 每日：还是单日
        for g in [crate::trend::Granularity::Hourly, crate::trend::Granularity::Daily] {
            app.debug_set_granularity(g);
            drive(&mut app, &ctx, 1260.0, 900.0, 2, vec![]);
            let label = app.debug_nav_label();
            assert!(!label.contains('~'), "{g:?} 应显示单日：{label}");
            assert_eq!(label, today.format("%Y%m%d").to_string());
        }
        // 每周：本周周一到今天
        app.debug_set_granularity(crate::trend::Granularity::Weekly);
        drive(&mut app, &ctx, 1260.0, 900.0, 2, vec![]);
        let label = app.debug_nav_label();
        let parts: Vec<&str> = label.split('~').collect();
        assert_eq!(parts.len(), 2, "当周应显示区间：{label}");
        assert_eq!(parts[0].len(), 8);
        let f = chrono::NaiveDate::parse_from_str(parts[0], "%Y%m%d").unwrap();
        let t = chrono::NaiveDate::parse_from_str(parts[1], "%Y%m%d").unwrap();
        assert_eq!(f.weekday().num_days_from_monday(), 0, "区间应从周一开始：{label}");
        assert_eq!(t, today, "止日期应截到今天：{label}");
        // 每月：本月 1 号到今天
        app.debug_set_granularity(crate::trend::Granularity::Monthly);
        drive(&mut app, &ctx, 1260.0, 900.0, 2, vec![]);
        let label = app.debug_nav_label();
        let parts: Vec<&str> = label.split('~').collect();
        assert_eq!(parts[0], format!("{}{:02}01", today.year(), today.month()), "当月应从 1 号开始：{label}");
        assert!(label.ends_with(&today.format("%Y%m%d").to_string()), "止日期应是今天：{label}");
        // 每年：今年 1 月 1 日到今天
        app.debug_set_granularity(crate::trend::Granularity::Yearly);
        drive(&mut app, &ctx, 1260.0, 900.0, 2, vec![]);
        let label = app.debug_nav_label();
        assert!(label.starts_with(&format!("{}0101", today.year())), "当年应从 1 月 1 日开始：{label}");

        // 排行页同理：当周显示区间、当日显示单日
        app.debug_open_rank_page();
        app.debug_set_rank(crate::gui::RankRange::Week, &crate::gui::ALL_SCOPES, crate::gui::RankMetric::Count, true);
        drive(&mut app, &ctx, 1260.0, 900.0, 2, vec![]);
        assert!(app.debug_nav_label().contains('~'), "排行页当周也应显示区间：{}", app.debug_nav_label());
        app.debug_set_rank(crate::gui::RankRange::Day, &crate::gui::ALL_SCOPES, crate::gui::RankMetric::Count, true);
        drive(&mut app, &ctx, 1260.0, 900.0, 2, vec![]);
        assert!(!app.debug_nav_label().contains('~'), "排行页当日还是单日：{}", app.debug_nav_label());
    }

    /// 小时选择：趋势页在每日/每周/每月/每年粒度下置灰；每小时粒度、热力图页、排行页都能用
    #[test]
    fn hour_controls_disabled_on_non_hourly_trends() {
        let (mut app, ctx) = headless_app(store_with_days(30));
        drive(&mut app, &ctx, 1260.0, 900.0, 2, vec![]);
        assert!(app.debug_hour_controls_enabled(), "热力图页应能用小时选择");
        app.debug_set_trend_page();
        for (g, want) in [
            (crate::trend::Granularity::Hourly, true),
            (crate::trend::Granularity::Daily, false),
            (crate::trend::Granularity::Weekly, false),
            (crate::trend::Granularity::Monthly, false),
            (crate::trend::Granularity::Yearly, false),
        ] {
            app.debug_set_granularity(g);
            drive(&mut app, &ctx, 1260.0, 900.0, 2, vec![]);
            assert_eq!(app.debug_hour_controls_enabled(), want, "{g:?} 粒度下小时选择可用性不对");
        }
        // 排行页：保留「看某个钟点」的功能，任何区间都能用（钟点栏只有导航行那一个）
        app.debug_open_rank_page();
        for (r, want) in [
            (crate::gui::RankRange::Day, true),
            (crate::gui::RankRange::Week, true),
            (crate::gui::RankRange::Month, true),
            (crate::gui::RankRange::Year, true),
            (crate::gui::RankRange::All, true),
        ] {
            app.debug_set_rank(r, &crate::gui::ALL_SCOPES, crate::gui::RankMetric::Count, true);
            drive(&mut app, &ctx, 1260.0, 900.0, 2, vec![]);
            assert_eq!(app.debug_hour_controls_enabled(), want, "排行页 {r:?} 区间下小时选择可用性不对");
        }
    }

    /// 导航标题宽度固定：单日与区间两种形态都用同一个容器宽度，且区间文字放得下
    #[test]
    fn nav_title_width_is_fixed() {
        let today = chrono::Local::now().date_naive();
        let (mut app, ctx) = headless_app(store_with_days(400));
        app.debug_set_trend_page();
        app.debug_goto_day(&today.format("%Y%m%d").to_string());
        let w = app.debug_nav_title_w();
        let s = app.debug_strings();
        let key = today.format("%Y%m%d").to_string();

        // 单日形态（每日粒度）：标题就是那一天的日期，「当前显示数据」单独作为前面的小标签
        app.debug_set_granularity(crate::trend::Granularity::Daily);
        drive(&mut app, &ctx, 1260.0, 900.0, 3, vec![]);
        let items = frame_texts_pos(&ctx, &mut app, 1260.0, 900.0);
        assert!(
            items.iter().any(|(_, _, _, t)| t == s.view_date_prefix),
            "「{}」应作为独立小标签出现：{items:?}",
            s.view_date_prefix
        );
        let single = items
            .iter()
            .find(|(_, _, _, t)| *t == key)
            .map(|(_, _, tw, _)| *tw)
            .expect("应有单日标题");
        assert!(single < w, "单日标题应放得下：{single:.0} < {w:.0}");

        // 区间形态（每周粒度）：同一个宽度，也要放得下
        app.debug_set_granularity(crate::trend::Granularity::Weekly);
        drive(&mut app, &ctx, 1260.0, 900.0, 2, vec![]);
        let items = frame_texts_pos(&ctx, &mut app, 1260.0, 900.0);
        let range = items
            .iter()
            .find(|(_, _, _, t)| t.contains('~'))
            .map(|(_, _, tw, _)| *tw)
            .expect("区间形态应有起止日期");
        assert!(range < w, "区间标题也要放得下（容器宽度 {w:.0}）：{range:.0}");
        assert!(range > single, "区间文字本来就比单日长（说明两者确实不同形态）");
    }

    /// 去过排行页之后，趋势页的滚轮翻页不能失效（rank_rect 是上一帧榜单的位置，别把它当成全局禁区）
    #[test]
    fn wheel_still_works_on_trends_after_visiting_rank() {
        let today = chrono::Local::now().date_naive();
        let (mut app, ctx) = headless_app(store_with_days(30));
        // 先去排行页逛一圈（此时会记下榜单矩形）
        app.debug_open_rank_page();
        drive(&mut app, &ctx, 1260.0, 900.0, 3, vec![]);
        let list = app.debug_rank_rect();
        assert!(list.height() > 50.0, "排行列表应有高度: {list:?}");

        // 切到趋势页，把指针放在趋势图正中（正好落在旧榜单矩形里）
        app.debug_set_trend_page();
        app.debug_goto_day(&today.format("%Y%m%d").to_string());
        drive(&mut app, &ctx, 1260.0, 900.0, 3, vec![]);
        let before = app.debug_view_label();
        let pos = Pos2::new(list.center().x, list.center().y);
        assert!(list.contains(pos));
        drive(&mut app, &ctx, 1260.0, 900.0, 1, vec![
            egui::Event::PointerMoved(pos),
            egui::Event::MouseWheel {
                unit: egui::MouseWheelUnit::Point,
                delta: egui::vec2(0.0, -40.0),
                modifiers: egui::Modifiers::default(),
            },
        ]);
        drive(&mut app, &ctx, 1260.0, 900.0, 2, vec![]);
        assert_ne!(app.debug_view_label(), before, "趋势页滚轮应仍然能翻页（旧榜单矩形不该挡）");
    }

    /// 排行页：区间跟着「当日/当周/当月/当年/全部」走，且对齐到自然周期
    #[test]
    fn rank_ranges_and_total_coupling() {
        let today = chrono::Local::now().date_naive();
        let (mut app, ctx) = headless_app(store_with_days(400));
        app.debug_open_rank_page();
        app.debug_goto_day(&today.format("%Y%m%d").to_string());
        drive(&mut app, &ctx, 1260.0, 900.0, 3, vec![]);

        use chrono::Datelike;
        let expect = |f: chrono::NaiveDate, t: chrono::NaiveDate| (f, t);
        app.debug_set_rank(crate::gui::RankRange::Day, &crate::gui::ALL_SCOPES, crate::gui::RankMetric::Count, true);
        drive(&mut app, &ctx, 1260.0, 900.0, 2, vec![]);
        assert_eq!(app.debug_rank_window(), expect(today, today), "当日 = 那一天");

        app.debug_set_rank(crate::gui::RankRange::Week, &crate::gui::ALL_SCOPES, crate::gui::RankMetric::Count, true);
        drive(&mut app, &ctx, 1260.0, 900.0, 2, vec![]);
        let (f, t) = app.debug_rank_window();
        assert_eq!(t.weekday().num_days_from_monday(), 6, "当周应到周日为止：{f}~{t}");
        assert_eq!((t - f).num_days(), 6, "当周应是 7 天：{f}~{t}");

        app.debug_set_rank(crate::gui::RankRange::Month, &crate::gui::ALL_SCOPES, crate::gui::RankMetric::Count, true);
        drive(&mut app, &ctx, 1260.0, 900.0, 2, vec![]);
        let (f, t) = app.debug_rank_window();
        assert_eq!((f.day(), t.day()), (1, 31), "当月应是整个 10 月：{f}~{t}");

        app.debug_set_rank(crate::gui::RankRange::Year, &crate::gui::ALL_SCOPES, crate::gui::RankMetric::Count, true);
        drive(&mut app, &ctx, 1260.0, 900.0, 2, vec![]);
        let (f, t) = app.debug_rank_window();
        assert_eq!((f.month(), f.day(), t.month()), (1, 1, 12), "当年应是整个自然年：{f}~{t}");

        // 切到「总计」时区间自动变成全部
        app.debug_set_rank(crate::gui::RankRange::Day, &crate::gui::ALL_SCOPES, crate::gui::RankMetric::Count, true);
        drive(&mut app, &ctx, 1260.0, 900.0, 2, vec![]);
        // 注意：指针要放在排行列表之外——列表里的滚轮按设计用来滚动列表，不翻日期
        let in_filters = Pos2::new(200.0, 70.0);
        drive(&mut app, &ctx, 1260.0, 900.0, 1, vec![
            egui::Event::PointerMoved(in_filters),
            egui::Event::MouseWheel {
                unit: egui::MouseWheelUnit::Point,
                delta: egui::vec2(0.0, 40.0),
                modifiers: egui::Modifiers::default(),
            },
        ]);
        drive(&mut app, &ctx, 1260.0, 900.0, 2, vec![]);
        assert_eq!(app.debug_view_label(), app.debug_strings().total_label, "向上滚应到总计");
        let (f, t) = app.debug_rank_window();
        assert!(f.year() <= 2024 || (t - f).num_days() > 30, "总计视图下区间应变成「全部」：{f}~{t}");
    }

    /// 排行页：右侧三列（占比/日均/次数）都在，右上角有分时折线图且能点选小时
    #[test]
    fn rank_columns_and_hour_chart() {
        let d = crate::stats::today_string();
        let mut store = Store::default();
        let space = crate::keys::idx_by_name("Space").unwrap(); // 主键盘区，配合下面的 Main 范围
        for h in [3u8, 15] {
            for _ in 0..50 {
                store.bump_key(&d, h, space);
            }
            store.bump_mouse(&d, h, &MouseEv::LeftUp);
        }
        let (mut app, ctx) = headless_app(store);
        app.debug_open_rank_page();
        app.debug_set_rank(crate::gui::RankRange::Day, &[crate::gui::RankScope::Main], crate::gui::RankMetric::Count, true);
        drive(&mut app, &ctx, 1260.0, 900.0, 4, vec![]);

        // 三列数值都在（占比带 %、日均带单位）
        let texts = frame_texts(&ctx, &mut app, 1260.0, 900.0);
        let s = app.debug_strings();
        assert!(texts.iter().any(|t| t.ends_with('%')), "应有占比列：{texts:?}");
        assert!(texts.iter().any(|t| t.contains(s.rank_per_day)), "应有日均列：{texts:?}");
        assert!(texts.iter().any(|t| t == "100"), "应有次数列 100（3 时 + 15 时 各 50）");
        // 指标不再占一行选项：行里只有 降序/升序 两个排序按钮
        assert!(texts.iter().any(|t| t == s.rank_desc) && texts.iter().any(|t| t == s.rank_asc));
        assert_eq!(
            texts.iter().filter(|t| *t == s.rank_metric_count || *t == s.rank_metric_share || *t == s.rank_metric_day).count(),
            3,
            "三个指标名只应出现在表头（各一次）"
        );

        // 页面上只有一个钟点栏（导航行那一个），筛选行里不再重复
        assert_eq!(
            texts.iter().filter(|t| *t == s.all_day).count(),
            1,
            "排行页只应有一个钟点栏：{texts:?}"
        );

        // 右上角分时折线图：非空，点某个小时会选中该小时
        let chart = app.debug_rank_chart_rect();
        assert!(chart.width() > 200.0 && chart.height() > 30.0, "排行页右上角应有分时折线图: {chart:?}");
        let p = Pos2::new(chart.left() + chart.width() * (15.5 / 24.0), chart.center().y);
        drive(&mut app, &ctx, 1260.0, 900.0, 1, vec![egui::Event::PointerMoved(p), click_at(p, true)]);
        drive(&mut app, &ctx, 1260.0, 900.0, 1, vec![click_at(p, false)]);
        assert_eq!(app.debug_hour_sel(), Some(15), "点折线图的第 15 小时应筛选该小时");
    }

    /// 趋势页：小时粒度下，历史某天也画「以该天结尾的最近 72 小时」（与今天一致）
    #[test]
    fn trend_hourly_window_is_consistent() {
        let today = chrono::Local::now().date_naive();
        let past = (today - chrono::Duration::days(3)).format("%Y%m%d").to_string();
        let (mut app, ctx) = headless_app(store_with_days(10));
        app.debug_set_trend_page();
        app.debug_set_granularity(crate::trend::Granularity::Hourly);
        app.debug_set_hour(Some(12));
        app.debug_goto_day(&past);
        drive(&mut app, &ctx, 1260.0, 900.0, 4, vec![]);
        let texts = frame_texts(&ctx, &mut app, 1260.0, 900.0);
        // 横轴标签形如「10-03 12」，统计出现的不同日期数
        let mut dates: Vec<&str> = texts
            .iter()
            .filter(|t| t.len() == 8 && t.as_bytes()[2] == b'-' && t.as_bytes()[5] == b' ')
            .map(|t| &t[..5])
            .collect();
        dates.sort();
        dates.dedup();
        assert!(dates.len() >= 3, "历史某天的小时粒度也应跨 3 天（72 小时），实际标签日期：{dates:?}");
    }

    /// 趋势页「今日」视图下，小时粒度滚轮必须让曲线跟着走（以前曲线锁在当前钟点不动）
    #[test]
    fn trend_hourly_follows_hour_on_today() {
        let today = chrono::Local::now().date_naive();
        let (mut app, ctx) = headless_app(store_with_days(5));
        app.debug_set_trend_page();
        app.debug_set_granularity(crate::trend::Granularity::Hourly);
        app.debug_goto_day(&today.format("%Y%m%d").to_string()); // 明确停在「今日」
        app.debug_set_hour(Some(12));
        drive(&mut app, &ctx, 1260.0, 900.0, 4, vec![]);

        // 悬停最后一个时间桶，工具提示的第一行是「YYYY-MM-DD HH:00–HH:00」
        let hover_last = |app: &mut App, ctx: &egui::Context| -> String {
            let rect = app.debug_chart_rect();
            let n = crate::trend::HOURLY_WINDOW as usize;
            let p = Pos2::new(crate::chart::bucket_center_x(rect, n, n - 1), rect.center().y);
            drive(app, ctx, 1260.0, 900.0, 1, vec![egui::Event::PointerMoved(p)]);
            let texts = frame_texts(ctx, app, 1260.0, 900.0);
            texts
                .iter()
                .find(|t| t.contains(":00–"))
                .cloned()
                .unwrap_or_else(|| panic!("悬停最后一个桶应显示完整标签：{texts:?}"))
        };
        let day = today.format("%Y-%m-%d").to_string();
        let t = hover_last(&mut app, &ctx);
        assert!(t.contains(&format!("{day} 12:00–13:00")), "末端应是 12 点：{t}");

        // 滚轮退一格 → 末端变 11 点（曲线跟着动）
        drive(&mut app, &ctx, 1260.0, 900.0, 1, vec![
            egui::Event::PointerMoved(Pos2::new(600.0, 200.0)),
            egui::Event::MouseWheel {
                unit: egui::MouseWheelUnit::Point,
                delta: egui::vec2(0.0, -40.0),
                modifiers: egui::Modifiers::default(),
            },
        ]);
        drive(&mut app, &ctx, 1260.0, 900.0, 2, vec![]);
        assert_eq!(app.debug_hour_sel(), Some(11));
        let t = hover_last(&mut app, &ctx);
        assert!(t.contains(&format!("{day} 11:00–12:00")), "滚轮后末端应变成 11 点：{t}");
    }

    /// 趋势页：小时粒度下滚轮一次走一小时
    #[test]
    fn trend_wheel_steps_one_hour() {
        let (mut app, ctx) = headless_app(store_with_days(5));
        app.debug_set_trend_page();
        app.debug_set_granularity(crate::trend::Granularity::Hourly);
        app.debug_set_hour(Some(12));
        drive(&mut app, &ctx, 1260.0, 900.0, 3, vec![]);
        assert_eq!(app.debug_hour_sel(), Some(12));

        let wheel = |dy: f32| {
            vec![
                egui::Event::PointerMoved(Pos2::new(600.0, 200.0)),
                egui::Event::MouseWheel {
                    unit: egui::MouseWheelUnit::Point,
                    delta: egui::vec2(0.0, dy),
                    modifiers: egui::Modifiers::default(),
                },
            ]
        };
        drive(&mut app, &ctx, 1260.0, 900.0, 1, wheel(-40.0));
        drive(&mut app, &ctx, 1260.0, 900.0, 1, vec![]);
        assert_eq!(app.debug_hour_sel(), Some(11), "向下滚一格应退到 11 时");
        drive(&mut app, &ctx, 1260.0, 900.0, 1, wheel(40.0));
        drive(&mut app, &ctx, 1260.0, 900.0, 1, vec![]);
        assert_eq!(app.debug_hour_sel(), Some(12), "向上滚一格应回到 12 时");

        // 0 时再往前 → 23 时，并且日期退一天
        app.debug_set_hour(Some(0));
        let before = app.debug_view_label();
        drive(&mut app, &ctx, 1260.0, 900.0, 1, wheel(-40.0));
        drive(&mut app, &ctx, 1260.0, 900.0, 2, vec![]);
        assert_eq!(app.debug_hour_sel(), Some(23), "0 时退一格应到前一天 23 时");
        assert_ne!(app.debug_view_label(), before, "跨天时显示日期应退一天");
    }

    /// 趋势页：三条曲线（键盘敲击红 / 鼠标按键绿 / 鼠标移动蓝）都在，图例与配色正确
    #[test]
    fn trend_has_three_series() {
        let (mut app, ctx) = headless_app(store_with_days(30));
        app.debug_set_trend_page();
        drive(&mut app, &ctx, 1260.0, 900.0, 4, vec![]);
        let texts = frame_texts(&ctx, &mut app, 1260.0, 900.0);
        let s = app.debug_strings();
        assert!(texts.iter().any(|t| t == s.keystrokes), "图例应有键盘敲击：{texts:?}");
        assert!(texts.iter().any(|t| t == s.mouse_clicks), "图例应有鼠标按键：{texts:?}");
        assert!(
            texts.iter().any(|t| t.starts_with(s.trend_mouse_distance)),
            "图例应有鼠标移动距离：{texts:?}"
        );

        // 图表区域里三条曲线的颜色都要出现
        let rect = app.debug_chart_rect();
        let colors = frame_path_colors(&ctx, &mut app, 1260.0, 900.0, rect);
        for (name, c) in [
            ("键盘(红)", crate::gui::KEY_COLOR),
            ("鼠标按键(绿)", crate::gui::CLICK_COLOR),
            ("鼠标移动(蓝)", crate::gui::MOUSE_COLOR),
        ] {
            assert!(colors.contains(&c), "图表里缺少{name}曲线：{colors:?}");
        }
    }

    /// 屏幕尺寸固定落在「总计」列：视图列数变化（总计 → 今日 → 该小时+今日）时不能跳列
    #[test]
    fn monitor_size_stays_in_total_column() {
        let d = crate::stats::today_string();
        let yest = (chrono::Local::now().date_naive() - chrono::Duration::days(1)).format("%Y%m%d").to_string();
        let mut store = Store::default();
        for date in [d.clone(), yest] {
            for _ in 0..300 {
                store.bump_key(&date, 12, 5);
            }
            store.bump_mouse(&date, 12, &MouseEv::MovePx(10_000.0));
        }
        let (mut app, ctx) = headless_app(store);
        app.debug_set_screen_mm(340, 220); // 15.9 寸，屏幕尺寸一行有确定数值
        app.debug_open_panel();
        drive(&mut app, &ctx, 1260.0, 1400.0, 4, vec![]);
        let views: [(&str, Option<String>, Option<u8>); 3] = [
            ("总计", None, None),
            ("今日", Some(d.clone()), None),
            ("该小时+今日", Some(d.clone()), Some(12)),
        ];
        for (name, day, hour) in views {
            if let Some(day) = day {
                app.debug_goto_day(&day);
            }
            app.debug_set_hour(hour);
            drive(&mut app, &ctx, 1260.0, 1400.0, 2, vec![]);
            let items = frame_texts_pos(&ctx, &mut app, 1260.0, 1400.0);
            let s = app.debug_strings();
            // 屏幕尺寸那一行的值（形如 “15.9 寸”）
            let found = items
                .iter()
                .find(|(_, _, _, t)| t.ends_with(s.unit_inch) && t.chars().next().map_or(false, |c| c.is_ascii_digit()))
                .unwrap_or_else(|| panic!("{name} 视图里找不到屏幕尺寸数值：{items:?}"));
            let (my, mx, mw, _) = found.clone();
            let mright = mx + mw;
            // 「总计」列数值的右边缘 = 「键盘敲击」那一行最右文字（本表数值列右对齐）
            let ks_y = items.iter().find(|(_, _, _, t)| t == s.keystrokes).map(|(y, _, _, _)| *y).unwrap();
            let total_right = items
                .iter()
                .filter(|(y, _, _, t)| (y - ks_y).abs() < 4.0 && t.parse::<u64>().is_ok())
                .map(|(_, x, w, _)| x + w)
                .fold(f32::MIN, f32::max);
            assert!(
                (mright - total_right).abs() < 3.0,
                "{name} 视图：屏幕尺寸右边缘 {mright:.1} 应贴齐总计列 {total_right:.1}（行 y={my:.1}）"
            );
        }
        app.debug_set_hour(None);
    }

    /// 语言切换：设置里选中文/English 并保存后，界面文案整体跟着变
    #[test]
    fn language_switch_changes_all_ui_text() {
        // 展开面板，设置区的文案才会出现在这一帧里
        let (mut app, ctx) = headless_app(Store::default());
        app.debug_open_panel();
        drive(&mut app, &ctx, 1260.0, 1400.0, 3, vec![]);

        // 切到英文（与点“保存”同路径）。英文不依赖字体，任何环境都能切
        assert_eq!(app.debug_set_language("en"), "en");
        let en = frame_texts(&ctx, &mut app, 1260.0, 1400.0);
        for want in ["Setting", "Language", "Layout", "Follow system"] {
            assert!(en.iter().any(|t| t == want), "英文界面缺少「{want}」：{en:?}");
        }
        // English 选项任何环境都在；「中文」在没有中文字体的机器上按设计不提供（选了也会回退英文），
        // Ubuntu CI runner 就是这种情况，所以这里要分开断言
        assert!(en.iter().any(|t| t == "English"), "语言选项应列出 English");
        if app.debug_has_cjk() {
            assert!(en.iter().any(|t| t == "中文"), "有中文字体时应列出「中文」选项");
        } else {
            assert!(!en.iter().any(|t| t == "中文"), "没有中文字体时不应提供「中文」选项");
        }

        // 中文：只有系统里真的有中文字体时才切得过去（没有字体时程序会回退英文）
        if app.debug_has_cjk() {
            assert_eq!(app.debug_set_language("zh"), "zh");
            let zh = frame_texts(&ctx, &mut app, 1260.0, 1400.0);
            for want in ["设置", "语言", "键盘布局", "跟随系统"] {
                assert!(zh.iter().any(|t| t == want), "中文界面缺少「{want}」：{zh:?}");
            }
            assert!(!zh.iter().any(|t| t == "Setting"), "中文界面不应出现英文文案");
            // 再切回英文同样生效
            assert_eq!(app.debug_set_language("en"), "en");
            let back = frame_texts(&ctx, &mut app, 1260.0, 1400.0);
            assert!(back.iter().any(|t| t == "Setting"), "切回英文应立刻生效");
        } else {
            println!("[softshot] 系统没有中文字体，跳过中文界面断言");
        }
    }

    /// 点击键盘右上角的分布条 → 选中该小时；再点一次 → 回到全天；
    /// 点击按键区域仍然只选按键（分布条优先，不误选到键）
    #[test]
    fn clicking_strip_selects_hour_not_keys() {
        // 造数据：9 时 100 次、15 时 500 次（其余小时 0）
        let mut store = Store::default();
        let date = crate::stats::today_string();
        for _ in 0..100 {
            store.bump_key(&date, 9, 5);
        }
        for _ in 0..500 {
            store.bump_key(&date, 15, 7);
        }
        store.bump_mouse(&date, 15, &MouseEv::MovePx(1234.0));

        let (mut app, ctx) = headless_app(store);
        // 先稳定布局（画布用布局锚点高度）
        drive(&mut app, &ctx, 1260.0, 1400.0, 4, vec![]);
        let (base_h, panel_extra, _c, _b) = app.debug_layout();
        let h = (base_h + panel_extra).ceil();
        drive(&mut app, &ctx, 1260.0, h, 3, vec![]);

        let strip = app.debug_strip_rect();
        assert!(strip.width() > 200.0 && strip.height() > 20.0, "分布条尺寸异常: {strip:?}");
        // 位置与尺寸：键盘右上角那块空白（一个键位高、宽度横跨导航区+数字键盘区）
        assert!((40.0..=50.0).contains(&strip.height()), "分布条应正好一个键位高: {strip:?}");
        assert!(strip.left() > 700.0, "分布条应在 F 行右侧: {strip:?}");

        let slot = strip.width() / 24.0;
        let nine = Pos2::new(strip.left() + 9.5 * slot, strip.center().y);
        drive(&mut app, &ctx, 1260.0, h, 1, vec![egui::Event::PointerMoved(nine), click_at(nine, true)]);
        assert_eq!(app.debug_hour_sel(), None, "仅按下不应触发选中");
        drive(&mut app, &ctx, 1260.0, h, 1, vec![click_at(nine, false)]);
        assert_eq!(app.debug_hour_sel(), Some(9), "点击 9 时柱子应选中 9 时");
        assert!(app.debug_selected().is_empty(), "点击分布条不应选中按键");

        // 再点一次同一根柱子 → 取消（回到全天）
        let again = Pos2::new(strip.left() + 9.2 * slot, strip.center().y);
        drive(&mut app, &ctx, 1260.0, h, 1, vec![egui::Event::PointerMoved(again), click_at(again, true)]);
        drive(&mut app, &ctx, 1260.0, h, 1, vec![click_at(again, false)]);
        assert_eq!(app.debug_hour_sel(), None, "再次点击应取消选中");

        // 点 15 时柱子 → 选中 15 时
        let fifteen = Pos2::new(strip.left() + 15.5 * slot, strip.center().y);
        drive(&mut app, &ctx, 1260.0, h, 1, vec![egui::Event::PointerMoved(fifteen), click_at(fifteen, true)]);
        drive(&mut app, &ctx, 1260.0, h, 1, vec![click_at(fifteen, false)]);
        assert_eq!(app.debug_hour_sel(), Some(15));

        // 点键盘上的 F1（第一行主键区）→ 只选按键，不动小时
        let f1 = Pos2::new(1260.0 * 0.5, 0.0);
        let _ = f1;
        let key_pos = Pos2::new(strip.left() - 30.0, strip.center().y); // F 行右侧附近的键位（F12 附近）
        drive(&mut app, &ctx, 1260.0, h, 1, vec![egui::Event::PointerMoved(key_pos), click_at(key_pos, true)]);
        drive(&mut app, &ctx, 1260.0, h, 1, vec![click_at(key_pos, false)]);
        assert_eq!(app.debug_hour_sel(), Some(15), "点击按键不应改变小时选择");
        assert_eq!(app.debug_selected().len(), 1, "点击按键应选中该键");
    }

    /// 生成 README 配图（默认 ignore；用 --ignored 跑）
    /// 环境变量：KMCOUNTER_SHOT_STATS 指定演示数据；KMCOUNTER_SHOT_DIR 指定输出目录
    #[test]
    #[ignore]
    fn render_readme_shots() {
        let stats = std::env::var("KMCOUNTER_SHOT_STATS").expect("需要 KMCOUNTER_SHOT_STATS 指向演示 stats.json");
        let dir = std::env::var("KMCOUNTER_SHOT_DIR").unwrap_or_else(|_| "/tmp/kmc-softshot".into());
        std::fs::create_dir_all(&dir).unwrap();
        let (cfg, _) = Config::load(&Path::new(&stats).with_file_name("config.toml"));

        // (输出名, 环境变量组合)
        // 点选按键用真实高频键（演示数据里次数最多的三个），出图更直观
        let top3 = {
            let store = Store::load(Path::new(&stats));
            let day = store.day(&crate::stats::today_string()).cloned().unwrap_or_default();
            let mut v: Vec<(usize, u64)> = (0..crate::keys::KEYS.len()).map(|i| (i, day.key_count(i))).collect();
            v.sort_by(|a, b| b.1.cmp(&a.1));
            v.iter().take(3).map(|(i, _)| i.to_string()).collect::<Vec<_>>().join(",")
        };
        let shots: Vec<(&str, Vec<(&str, String)>)> = vec![
            ("shot-heatmap", vec![]),
            ("shot-expand", vec![("KMCOUNTER_START_PANELS", "both".into())]),
            ("shot-hour", vec![("KMCOUNTER_HOUR", "3".into())]),
            (
                "shot-hour-panel",
                vec![("KMCOUNTER_START_PANELS".into(), "both".into()), ("KMCOUNTER_HOUR", "3".into())],
            ),
            ("shot-hour-today", vec![("KMCOUNTER_HOUR", "3".into()), ("KMCOUNTER_VIEW_IDX", "1".into()), ("KMCOUNTER_START_PANELS", "both".into())]),
            ("shot-select", vec![("KMCOUNTER_SELECT", top3)]),
            ("shot-calendar", vec![("KMCOUNTER_CAL", "1".into())]),
            ("shot-trend", vec![("KMCOUNTER_START_PANELS", "trend".into())]),
            ("shot-trend-panel", vec![("KMCOUNTER_START_PANELS", "trend,stats".into())]),
            ("shot-trend-hour", vec![("KMCOUNTER_START_PANELS", "trend".into()), ("KMCOUNTER_GRAN", "hour".into())]),
            ("shot-rank", vec![("KMCOUNTER_START_PANELS", "rank".into())]),
        ];
        for (name, envs) in shots {
            for (k, v) in &envs {
                std::env::set_var(k, v);
            }
            let store = Store::load(Path::new(&stats));
            let out = format!("{dir}/{name}");
            let dir_of_stats = Path::new(&stats).parent().unwrap_or(Path::new("."));
            let width: f32 = std::env::var("KMCOUNTER_SHOT_W").ok().and_then(|v| v.parse().ok()).unwrap_or(1260.0);
            let made = render(store, cfg.clone(), dir_of_stats, &out, width, 1.25).unwrap();
            for s in made {
                println!("[softshot] {name} → {} ({}x{})", s.ppm.display(), s.w, s.h);
            }
            for (k, _) in &envs {
                std::env::remove_var(k);
            }
        }

        // 英文界面（配置文件里 language = "en"）：展示设置里的语言选项
        {
            std::env::set_var("KMCOUNTER_START_PANELS", "both");
            let mut cfg_en = cfg.clone();
            cfg_en.language = "en".into();
            let store = Store::load(Path::new(&stats));
            let out = format!("{dir}/shot-settings-en");
            let dir_of_stats = Path::new(&stats).parent().unwrap_or(Path::new("."));
            for s in render(store, cfg_en, dir_of_stats, &out, 1260.0, 1.25).unwrap() {
                println!("[softshot] settings-en → {} ({}x{})", s.ppm.display(), s.w, s.h);
            }
            std::env::remove_var("KMCOUNTER_START_PANELS");
        }
    }
}
