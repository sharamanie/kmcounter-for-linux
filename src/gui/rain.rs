//! 键雨（Key Rain）—— 按压区间驱动的两种样式。
//!
//! 与 JipperKeyViewer 的差异：那边是 Unity 对象池 + 逐滴 Update；这里是 egui 立即模式，
//! **不保存任何粒子状态** —— 每帧从 `KpsTracker::rain_spans` 拿「每个键最近的按压区间
//! （按下距今 / 松开距今）」，按**纯几何**重建整条雨。
//!
//! 两种样式都遵守同一个运动学（对齐 JipperKeyViewer 的 RawRain::UpdateLocation）：
//!
//! - `front = 按下至今秒数 × speed`：**前沿**（远端）距键格顶边的高度，只要时间在走就一直增大，
//!   松手也不会停；
//! - `len`：条的长度。按住期间 `len = front`（从键格一路长到前沿）；**松手那一刻定格**，
//!   之后不再变长 —— 于是整条雨原速继续向上飘，这就是"松手不消失"的关键；
//! - 可见区间 = `[front - len, front]` 与轨道 `[0, H]` 求交：前沿出顶后，条的**底部开始被裁**，
//!   越来越短，直到整条出顶才消失（对齐 JipperKeyViewer 的 `sizeY = FinalSize.y - dropY + height`）。
//!
//! - **trail 生长竖条**：一条连续竖条（JipperKeyViewer 的 RainLayer 样式）。
//! - **block 方块串**：按住期间每 `D` 像素发射一个短方块，`D` 固定，块本身也按 `speed` 上飘。
//!
//! 两种样式共用同一套坐标约定：
//! - `speed` 由「走完整个轨道要几毫秒」换算（换轨道高度不改观感）；
//! - 渐隐带固定在**轨道顶部 1/3**：进入 [2H/3, H] 后从满亮线性淡到 0，H 以上完全不可见；
//! - 多排布局时第 2 排起逐排**变窄 + 加深一档**，所有排共用同一条轨道（轨道底边 = 第一排键格顶边）。
//!
//! 来源（样式参照）：https://github.com/adofaiex/JipperKeyViewer

use super::*;

/// 最短可见长度（px）：极短按压也画得出来。
const MIN_BAR_H: f32 = 3.0;
/// 方块串里条宽相对键格宽的比例（再做绝对上下限），细条才像"雨"而不是"方块"。
const BAR_W_RATIO: f32 = 0.20;
/// 条宽绝对上下限（px）：太细看不见，太粗像方块。
const BAR_W_MIN: f32 = 3.0;
const BAR_W_MAX: f32 = 10.0;
/// 方块串：条长 = 条宽 × 这个系数。
const BAR_H_RATIO: f32 = 1.15;
/// 方块串里条占间距的比例（剩下是缝）：0.62 → 缝约 38%，块之间连成「串」而不是散点。
const BLOCK_FILL: f32 = 0.62;
/// 生长竖条的宽度系数（比方块略宽，看起来是"柱"）。
const TRAIL_W_RATIO: f32 = 0.16;
/// 每排键格的递进系数：第 2 排起宽度 ×0.85、颜色加深一档。
const ROW_WIDTH_STEP: f32 = 0.85;
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

/// 键雨的基准色：跟随该键热力（与键格边框同源）。
/// `dim` = 额外压暗档位（多排递进用，0 = 不压）。
fn rain_color(heat: f32, alpha: u8, dim: u32) -> Color32 {
    let (_, border, _) = key_heat_style(heat, 0.0);
    let c = border.color.to_array();
    if dim == 0 {
        return Color32::from_rgba_unmultiplied(c[0], c[1], c[2], alpha);
    }
    let f = (1.0 - 0.18 * dim as f32).max(0.3);
    Color32::from_rgba_unmultiplied(
        (c[0] as f32 * f) as u8,
        (c[1] as f32 * f) as u8,
        (c[2] as f32 * f) as u8,
        alpha,
    )
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
    let block_style = cfg.kps_rain_style == "block";
    let h = rain_area.height();
    let speed = rain_px_per_sec(cfg);
    let opacity = cfg.kps_rain_opacity.min(100) as f32 / 100.0;
    let painter = ui.painter().with_clip_rect(rain_area);
    let row_pos = key_rows_layout(&layout.rows, m.box_w, m.graph_w);

    // 轨道底边 = 第一排键格顶边；多排布局下所有排共用同一条轨道。
    let cell_top = rain_area.bottom();

    let mut info_idx = 0usize;
    for (row, cells) in layout.rows.iter().enumerate() {
        for (cell_idx, cell) in cells.iter().enumerate() {
            let Some(&(x_off, box_w)) = row_pos.get(row).and_then(|r| r.get(cell_idx)) else {
                break;
            };
            if cell.name.is_empty() {
                info_idx += 1; // 空位：占地方，不画不统计
                continue;
            }
            let Some(k) = kd.key_infos.get(info_idx) else {
                break;
            };
            let idx = info_idx;
            info_idx += 1;
            let Some(spans) = kd.rain.get(idx) else {
                continue;
            };
            if spans.is_empty() {
                continue;
            }
            let cx = rain_area.left() + x_off + box_w * 0.5;
            let heat = crate::stats::kps_key_heat(k.kps);
            let step = ROW_WIDTH_STEP.powi(row as i32);
            let dim = row.min(3) as u32;

            for &(start_age, end_age) in spans {
                // 前沿：从按下算起按固定速度推进，**松手后继续推进**（这是"飘上去"的动力）。
                let front = start_age as f32 * speed;
                if front <= 0.0 {
                    continue;
                }
                // 按住时长（秒）：松手后 `start_age - end_age` 恒定 = 实际按住时长，
                // 所以下面的 len 与发射长度都不会随时间漂移，雨条自然"定格上飘"。
                let hold_s = if end_age <= 0.0 {
                    start_age as f32
                } else {
                    (start_age - end_age).max(0.0) as f32
                };
                let hold_len = hold_s * speed;

                if block_style {
                    // —— 方块串：按住期间每 D 像素发射一块；块本身也按 speed 上飘 ——
                    let w = (box_w * BAR_W_RATIO).clamp(BAR_W_MIN, BAR_W_MAX) * step;
                    let bar_h = (w * BAR_H_RATIO).max(MIN_BAR_H);
                    let d = (bar_h / BLOCK_FILL).max(MIN_BAR_H + 2.0);
                    // 已发射的最大序号（松手后固定）
                    let k_hi = (hold_len / d).floor() as i64;
                    if k_hi < 0 {
                        continue;
                    }
                    // 近端已飘出轨道顶的最小序号（随时间右移，块从底部被裁）
                    let k_lo = (((front - h) / d).ceil() as i64).max(0);
                    // 近端还在键格顶以上的最大序号
                    let k_top = (front / d).floor() as i64;
                    let (lo, hi) = (k_lo.max(0), k_hi.min(k_top));
                    if hi < lo {
                        continue;
                    }
                    for kk in lo..=hi {
                        let top_rise = front - kk as f32 * d;
                        let vis_top = top_rise.max(0.0);
                        let vis_bottom = (top_rise + bar_h).min(h);
                        if vis_bottom - vis_top < 0.5 {
                            continue;
                        }
                        emit_bar(
                            &painter, cx, w, cell_top, vis_top, vis_bottom, h, opacity, heat, dim,
                        );
                    }
                } else {
                    // —— 生长竖条：按住时长到前沿，松手后长度定格、整条继续上飘 ——
                    let w = (box_w * TRAIL_W_RATIO).clamp(BAR_W_MIN, BAR_W_MAX) * step;
                    let len = hold_len.max(MIN_BAR_H);
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
                        &painter, cx, w, cell_top, vis_top, vis_bottom, h, opacity, heat, dim,
                    );
                }
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
    heat: f32,
    dim: u32,
) {
    let a_top = fade_at(h - vis_top, h);
    let a_bot = fade_at(h - vis_bottom, h);
    let alpha_top = (a_top * opacity * 255.0).round() as u8;
    let alpha_bot = (a_bot * opacity * 255.0).round() as u8;
    if alpha_top == 0 && alpha_bot == 0 {
        return;
    }
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
            rain_color(heat, alpha_bot, dim),
        );
    } else {
        // 跨渐隐带：Mesh 顶点色做竖向渐变
        let mut mesh = egui::Mesh::default();
        mesh.colored_vertex(Pos2::new(xl, y_a), rain_color(heat, alpha_top, dim));
        mesh.colored_vertex(Pos2::new(xr, y_a), rain_color(heat, alpha_top, dim));
        mesh.colored_vertex(Pos2::new(xl, y_b), rain_color(heat, alpha_bot, dim));
        mesh.colored_vertex(Pos2::new(xr, y_b), rain_color(heat, alpha_bot, dim));
        mesh.add_triangle(0, 1, 2);
        mesh.add_triangle(1, 3, 2);
        painter.add(egui::Shape::mesh(mesh));
    }
}
