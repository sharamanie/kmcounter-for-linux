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
        // 语言名用各自母语书写，两种语言下都在
        assert!(en.iter().any(|t| t == "中文") && en.iter().any(|t| t == "English"), "语言选项应列出两种语言");

        // 中文：只有系统里真的有中文字体时才切得过去（没有字体时程序会回退英文，CI 上就是这种情况）
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
