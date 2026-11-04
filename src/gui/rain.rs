//! 键雨（Key Rain）—— 按压区间驱动的生长竖条。
//!
//! 与 JipperKeyViewer 的差异：那边是 Unity 对象池 + 逐滴 Update；这里是 egui 立即模式，
//! **不保存任何粒子状态** —— 每帧从 `KpsTracker::rain_spans` 拿「每个键最近的按压区间
//! （按下距今 / 松开距今）」，按**纯几何**重建整条雨。
//!
//! 运动学（对齐 JipperKeyViewer 的 RawRain::UpdateLocation）：
//!
//! - `front = 按下至今秒数 × speed`：**前沿**（远端）距键格顶边的高度，只要时间在走就一直增大，
//!   松手也不会停；
//! - `len`：条的长度。按住期间 `len = front`（从键格一路长到前沿）；**松手那一刻定格**，
//!   之后不再变长 —— 于是整条雨原速继续向上飘，这就是"松手不消失"的关键；
//! - 可见区间 = `[front - len, front]` 与轨道 `[0, H]` 求交：前沿出顶后，条的**底部开始被裁**，
//!   越来越短，直到整条出顶才消失（对齐 JipperKeyViewer 的 `sizeY = FinalSize.y - dropY + height`）。
//!
//! 坐标约定：
//! - `speed` 由「走完整个轨道要几毫秒」换算（换轨道高度不改观感）；
//! - 渐隐带固定在**轨道顶部 1/3**：进入 [2H/3, H] 后从满亮线性淡到 0，H 以上完全不可见；
//! - 条宽 = 键格宽 × `kps_rain_width_pct`（**略窄于键格**，看起来是键格上长出的一根柱子）；
//! - 颜色固定取 `kps_rain_color`（默认纯白），**不随 KPS 热度变色** —— 雨是"运动轨迹"，
//!   跟着热度变色会让人误以为自己按多快，而且和键格本身的发光混在一起看不出层次。
//!
//! 多层布局（`kps_keys` 里换行 = 分行）：所有排共用同一条轨道（轨道底边 = 第一排键格顶边），
//! 但**排号越大越窄、越实**（`ROW_WIDTH_STEP` / `ROW_ALPHA_TABLE`），且**第 3 排起换色相**：
//! - 宽度：第 1 排原始宽度，之后 ×0.70 / ×0.49 / ×0.34，一层套一层地收窄；
//! - 不透明度：第 1 排刻意压到 0.35（明显淡于实心），之后 0.72 / 0.92 / 1.00 逐排抬升 ——
//!   窄条画在宽条中心，中心最终呈现的就是最深那一排的颜色，所以必须让后面的排最实，
//!   否则中心早早被第一排糊成纯白，换色也看不出来；表外为 0 = 完全不画；
//! - 色相：第 3 排起从基色（默认纯白，先抬饱和度再转）逐排 +0.15，一眼分出前后几层。
//! 于是多排同列同时按下时，看到的是「淡而宽的底条 + 实而彩的窄心」，而不是两根同宽白条糊成一块。
//!
//! 来源（样式参照）：https://github.com/adofaiex/JipperKeyViewer

use super::*;

/// 最短可见长度（px）：极短按压也画得出来。
const MIN_BAR_H: f32 = 3.0;
/// 条宽相对键格宽的默认比例：略窄于键格，两侧各留约 14% 的边。
/// 运行期取 cfg.kps_rain_width_pct（20~100），这里只是缺省值。
const BAR_W_RATIO: f32 = 0.72;
/// 条宽绝对上下限（px）：极小缩放下不至于细成线，极大缩放下不至于糊成整块。
const BAR_W_MIN: f32 = 4.0;
const BAR_W_MAX: f32 = 64.0;
/// 排间宽度递进系数：第 1 排 ×1.00，第 2 排 ×0.60，第 3 排 ×0.36，第 4 排 ×0.22…
/// 给得这么狠是有原因的：**同一列的不同排共用同一条竖线**，窄条整个落在宽条内部，
/// 只能靠宽条两边露出来的那一圈看出层次；衰减太小就只是一根粗柱子，看不出有几排。
/// 这样叠出来是一层层「套娃」的效果，一眼就能数出几排同时按着。
const ROW_WIDTH_STEP: f32 = 0.70;
/// 每排的不透明度系数（按排号索引，超出表长 = 0，即**彻底透明、完全不画**）。
/// 方向是**越靠后的排越实**：几排的雨共用同一条轨道与同一水平中心，其实是**层层叠在一起**的，
/// 又因为后画的排更窄、盖在中心，所以中心最终呈现的正是最深那一排的颜色。
/// 若第 1 排就已经是实心白，中心早早饱和成纯白，后面的换色排再怎么调都叠不上去 ——
/// 这正是「深浅对比看不出来」的根因。因此第 1 排压到 0.35（明显淡于实心），
/// 逐排抬升到 1.00，表外为 0（**允许一路衰减到完全透明**，对标 JipperKeyViewer 的逐排消失）。
const ROW_ALPHA_TABLE: [f32; 5] = [0.35, 0.72, 0.92, 1.00, 0.0];
/// 排宽下限（px）：深层排不能被连乘到看不见。
const ROW_WIDTH_MIN: f32 = 1.5;
/// 排号 → 色相表（0 起；`None` = 沿用 `kps_rain_color`，不做色相变换）。
/// 用固定表而不是逐排累加，是为了让相邻两排**一眼就是两种颜色**：
/// 累加出来的 0.70 / 0.85 都落在紫红一带，叠起来仍是一片粉，分不出前后。
/// 0.55 ≈ 天青、0.80 ≈ 紫 —— 冷色段彼此距离够远，且不会撞上键格本身的热力青绿。
const ROW_HUE_TABLE: [Option<f32>; 5] = [None, None, Some(0.55), Some(0.80), None];
/// 换色排的最低饱和度：纯白没有色相，必须先给饱和度，否则色相原地旋转等于没变。
const ROW_TINT_SAT: f32 = 0.88;
/// 取样窗口（秒）。**故意给得远大于"走完一条轨道"**：
/// 松手后的雨还要继续上飘、还要能被裁短，若窗口等于走完轨道的时间，
/// 松手瞬间整条雨就会因为"按下太久"被丢掉 —— 那正是"松手即消失"的根因。
/// 取 20 秒：即便速度为 20 秒/轨道这种极端值，条也早已飘出视野。
const RAIN_SAMPLE_SECS: f32 = 20.0;

/// 键雨轨道的像素高度（16~400；配置 0 = 自动 96px）。
pub(crate) fn rain_area_h(cfg: &Config) -> f32 {
    if cfg.kps_rain_height > 0 {
        cfg.kps_rain_height.clamp(16, 400) as f32
    } else {
        96.0
    }
}

/// 条宽占键格宽的比例（0.20~1.00）。默认略窄于键格，让条看起来像键格上长出来的柱子。
fn rain_width_ratio(cfg: &Config) -> f32 {
    if cfg.kps_rain_width_pct == 0 {
        BAR_W_RATIO
    } else {
        (cfg.kps_rain_width_pct.clamp(20, 100) as f32 / 100.0).max(0.2)
    }
}

/// 上升速度（px/s）= 轨道高度 ÷ 走完轨道的秒数。`kps_rain_speed` 存的是毫秒。
fn rain_px_per_sec(cfg: &Config) -> f32 {
    let travel_ms = cfg.kps_rain_speed.clamp(200, 20_000) as f32;
    rain_area_h(cfg) / (travel_ms / 1000.0)
}

/// 取样窗口（秒）：固定一个远大于轨道穿越时间的值，剩余可见性全部交给几何裁剪。
pub(crate) fn rain_lifespan_secs(_cfg: &Config) -> f32 {
    RAIN_SAMPLE_SECS
}

/// 渐隐带高度：轨道顶部 1/3。
fn fade_band_h(h: f32) -> f32 {
    h / 3.0
}

/// 条上某一点的透明度系数：`d_from_top` = 该点到轨道顶的距离（0=最顶）。
/// 顶部 1/3 内从 0 线性升到 1，其余满亮。
fn fade_at(d_from_top: f32, h: f32) -> f32 {
    let band = fade_band_h(h);
    if band <= 0.5 {
        return 1.0;
    }
    d_from_top.max(0.0).min(band) / band
}

/// 某个排的不透明度系数（见 `ROW_ALPHA_TABLE`）。表外的排返回 0 = 完全不画。
fn row_alpha_scale(row: usize) -> f32 {
    ROW_ALPHA_TABLE.get(row).copied().unwrap_or(0.0)
}

/// 某个排的雨条颜色：`ROW_TINT_FROM` 之前的排用 `base`，之后按排号旋转色相。
///
/// 纯白没有色相，所以白色时先落到 `ROW_HUE_BASE`（天青）并抬起饱和度再按 `ROW_HUE_STEP` 递进 ——
/// 否则 HSV 色相原地旋转，白色永远是白色，换了颜色也看不出来。
fn row_color(base: Color32, row: usize) -> Color32 {
    let Some(hue) = ROW_HUE_TABLE.get(row).copied().flatten() else {
        return base;
    };
    let hsva = egui::ecolor::Hsva::from_srgb([base.r(), base.g(), base.b()]);
    let tinted = egui::ecolor::Hsva {
        h: hue,
        s: hsva.s.max(ROW_TINT_SAT),
        v: hsva.v.max(0.85),
        a: 1.0,
    };
    let [r, g, b, _] = tinted.to_srgba_unmultiplied();
    Color32::from_rgb(r, g, b)
}

/// 键雨的固定基色（`kps_rain_color`，默认纯白）。
///
/// **不跟随 KPS 热度**：雨条是"这次按压的运动轨迹"，与手速无关；跟着热度变色会让同一个键
/// 在不同时刻呈现不同颜色，和键格自身的发光混在一起反而看不出层次。
pub(crate) fn rain_base_color(cfg: &Config) -> Color32 {
    let c = cfg.kps_rain_color;
    Color32::from_rgb(c[0], c[1], c[2])
}

/// 画一帧键雨（无状态：整条雨每帧从按压区间纯几何重建）。
///
/// `rain_area` 是键格**顶边往上**那条轨道（高度 `m.rain_h`）；超出轨道的 clip 掉。
pub(crate) fn paint_kps_rain(
    ui: &egui::Ui,
    rain_area: Rect,
    layout: &crate::config::KpsKeyLayout,
    m: &KpsHudMetrics,
    kd: &KpsHudData,
    cfg: &Config,
) {
    if !cfg.kps_show_rain || m.rain_h <= 0.0 {
        return;
    }
    let h = rain_area.height();
    let speed = rain_px_per_sec(cfg);
    let opacity = cfg.kps_rain_opacity.min(100) as f32 / 100.0;
    let base = rain_base_color(cfg);
    let painter = ui.painter().with_clip_rect(rain_area);
    let row_pos = key_rows_layout(&layout.rows, m.box_w, m.graph_w);

    // 轨道底边 = 第一排键格顶边；多层布局下所有排共用同一条轨道。
    let cell_top = rain_area.bottom();

    let mut info_idx = 0usize;
    for (row, cells) in layout.rows.iter().enumerate() {
        // 排间递进：宽度逐排收窄、不透明度逐排抬升、第 3 排起换色相。
        // 第一排刻意压得比实心淡（0.35）—— 多排是叠在同一条竖线上的，
        // 第一排若已是实心白，中心永远饱和，后面的排无论换色还是调 alpha 都看不出来。
        let alpha_scale = row_alpha_scale(row);
        if alpha_scale <= 0.0 {
            continue; // 表外的排彻底透明，连几何都不必算
        }
        let step = ROW_WIDTH_STEP.powi(row as i32);
        let row_base = row_color(base, row);
        for (cell_idx, cell) in cells.iter().enumerate() {
            let Some(&(x_off, box_w)) = row_pos.get(row).and_then(|r| r.get(cell_idx)) else {
                break;
            };
            if cell.name.is_empty() {
                info_idx += 1; // 空位：占地方，不画不统计
                continue;
            }
            if kd.key_infos.get(info_idx).is_none() {
                break;
            }
            let idx = info_idx;
            info_idx += 1;
            let Some(spans) = kd.rain.get(idx) else {
                continue;
            };
            if spans.is_empty() {
                continue;
            }
            let cx = rain_area.left() + x_off + box_w * 0.5;
            let w = ((box_w * rain_width_ratio(cfg)).clamp(BAR_W_MIN, BAR_W_MAX) * step)
                .max(ROW_WIDTH_MIN);

            for &(start_age, end_age) in spans {
                // 前沿：从按下算起按固定速度推进，**松手后继续推进**（这是"飘上去"的动力）。
                let front = start_age as f32 * speed;
                if front <= 0.0 {
                    continue;
                }
                // 按住时长（秒）：松手后 `start_age - end_age` 恒定 = 实际按住时长，
                // 所以下面的 len 不会随时间漂移，雨条自然"定格上飘"。
                let hold_s = if end_age <= 0.0 {
                    start_age as f32
                } else {
                    (start_age - end_age).max(0.0) as f32
                };
                let len = (hold_s * speed).max(MIN_BAR_H);
                let near = (front - len).max(0.0);
                if near >= h {
                    continue; // 整条已飘出轨道
                }
                let vis_top = near.min(h);
                let vis_bottom = front.min(h);
                if vis_bottom - vis_top < 0.5 {
                    continue;
                }
                emit_bar(
                    &painter, cx, w, cell_top, vis_top, vis_bottom, h, opacity, row_base,
                    alpha_scale,
                );
            }
        }
    }
}

/// 画一根竖条：从 `vis_top`（距轨道顶的距离）到 `vis_bottom`，顶端进渐隐带就做竖向渐变。
#[allow(clippy::too_many_arguments)]
fn emit_bar(
    painter: &egui::Painter,
    cx: f32,
    w: f32,
    cell_top: f32,
    vis_top: f32,
    vis_bottom: f32,
    h: f32,
    opacity: f32,
    base: Color32,
    alpha_scale: f32,
) {
    let a_top = fade_at(h - vis_top, h);
    let a_bot = fade_at(h - vis_bottom, h);
    let alpha_top = (a_top * opacity * alpha_scale * 255.0).round() as u8;
    let alpha_bot = (a_bot * opacity * alpha_scale * 255.0).round() as u8;
    if alpha_top == 0 && alpha_bot == 0 {
        return;
    }
    let tint = |a: u8| Color32::from_rgba_unmultiplied(base.r(), base.g(), base.b(), a);
    // vis_* 是「距键格顶边的高度」：数值大的一端离**轨道顶**近，在屏幕上更靠上（y 更小）。
    // Rect::from_min_max 要求 min <= max，而这里 vis 越大 y 越小，两者方向相反 ——
    // 必须显式取 min/max，否则画出来是空矩形（rect_filled 直接不可见，
    // 而 Mesh 分支因不剔除背面仍能看见，表现为「只有跨渐隐带的条才显示一半」）。
    let y_a = cell_top - vis_top;
    let y_b = cell_top - vis_bottom;
    let (y_hi, y_lo) = (y_a.min(y_b), y_a.max(y_b));
    let (xl, xr) = (cx - w * 0.5, cx + w * 0.5);
    if alpha_top == alpha_bot {
        painter.rect_filled(
            Rect::from_min_max(Pos2::new(xl, y_hi), Pos2::new(xr, y_lo)),
            1.5,
            tint(alpha_bot),
        );
    } else {
        // 跨渐隐带：Mesh 顶点色做竖向渐变
        let mut mesh = egui::Mesh::default();
        mesh.colored_vertex(Pos2::new(xl, y_a), tint(alpha_top));
        mesh.colored_vertex(Pos2::new(xr, y_a), tint(alpha_top));
        mesh.colored_vertex(Pos2::new(xl, y_b), tint(alpha_bot));
        mesh.colored_vertex(Pos2::new(xr, y_b), tint(alpha_bot));
        mesh.add_triangle(0, 1, 2);
        mesh.add_triangle(1, 3, 2);
        painter.add(egui::Shape::mesh(mesh));
    }
}
