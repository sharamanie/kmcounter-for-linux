//! 键雨（Key Rain）—— 参照 JipperKeyViewer 的 RainLayer 思路重写。
//!
//! 与 JipperKeyViewer 的关键差异：那边是 Unity，必须维护对象池（60 滴上限 + 顶点预算 60000）；
//! 这里是 egui 的立即模式 painter，**完全不需要粒子状态** —— 每帧从
//! KpsTracker::recent_hits 拿「每个键最近若干次击键距今多少秒」，
//! 高度 = 年龄 × 速度，透明度由寿命曲线给出，画完即弃。
//!
//! 两种样式（kps_rain_style）：
//! - trail：生长竖条 —— 底端钉在键格顶边，随年龄向上生长，到顶截断（JipperKeyViewer 的原味）；
//! - block：方块串 —— 一块小方块按 年龄×速度 向上飘走（截图里的那种样式）。
//!
//! 来源：https://github.com/adofaiex/JipperKeyViewer

use super::*;

/// 键雨区域的像素高度：设置里写了具体值就用它，0 = 自动跟随 KPS 图的高度
/// （两者等高时上下两块的视觉分量一致）。
pub(crate) fn rain_area_h(cfg: &Config) -> f32 {
    if cfg.kps_rain_height > 0 {
        cfg.kps_rain_height.clamp(16, 400) as f32
    } else {
        let s = (cfg.kps_scale.clamp(60, 400) as f32) / 100.0;
        64.0 * s
    }
}

/// 到顶之后再多挂一会儿才淡净的倍率。
///
/// 不加这个系数的话，64px 高的雨区配上 120px/s 只有 0.53 秒寿命，
/// 连打时雨看起来像在闪而不是在飘。
const RAIN_HOLD: f32 = 1.6;

/// 一条键雨的寿命（秒）= 高度 ÷ 速度 × 挂留系数。
/// `recent_hits` 的取样窗口按它算：超出寿命的击键已经淡净，不必取。
pub(crate) fn rain_lifespan_secs(cfg: &Config) -> f32 {
    let speed = cfg.kps_rain_speed.clamp(20, 2000) as f32;
    (rain_area_h(cfg) / speed * RAIN_HOLD).clamp(0.25, 8.0)
}

/// 淡出曲线（照抄 JipperKeyViewer RainSystem.UpdateFade）：
/// t = 0 满亮，t = 1 全透明，alpha = 1 - t(2-t) —— 二次缓出，前段慢、后段快。
fn rain_fade(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    1.0 - t * (2.0 - t)
}

/// 键雨的基准色：跟随该键热力（与键格边框同源，整体不花）。
fn rain_color(heat: f32, alpha: u8) -> Color32 {
    let (_, border, _) = key_heat_style(heat, 0.0);
    let c = border.color.to_array();
    Color32::from_rgba_unmultiplied(c[0], c[1], c[2], alpha)
}

/// 画一帧键雨。
///
/// rain_area 是键格**顶边往上**那条雨区（高度 m.rain_h）；
/// 超出雨区的一律 clip 掉，多行键格布局时雨不会盖到别处。
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
    let lifespan = rain_lifespan_secs(cfg) as f64;
    let speed = cfg.kps_rain_speed.clamp(20, 2000) as f32;
    let opacity = cfg.kps_rain_opacity.min(100) as f32 / 100.0;
    let block_style = cfg.kps_rain_style == "block";
    let painter = ui.painter().with_clip_rect(rain_area);

    let row_pos = key_rows_layout(&layout.rows, m.box_w, m.graph_w);
    let mut info_idx = 0usize;
    for (row, cells) in layout.rows.iter().enumerate() {
        for (cell_idx, cell) in cells.iter().enumerate() {
            let Some(&(x_off, box_w)) = row_pos.get(row).and_then(|r| r.get(cell_idx)) else {
                break;
            };
            if cell.name.is_empty() {
                continue; // 空位：占地方，不画不统计
            }
            let Some(k) = kd.key_infos.get(info_idx) else {
                break;
            };
            let idx_now = info_idx;
            info_idx += 1;
            let Some(ages) = kd.rain.get(idx_now) else {
                continue;
            };
            if ages.is_empty() {
                continue;
            }
            let cell_top = rain_area.bottom() + row as f32 * (m.box_h + m.gap);
            let cx = rain_area.left() + x_off + box_w * 0.5;
            let heat = crate::stats::kps_key_heat(k.kps);
            let w = (box_w * 0.62).max(3.0);
            for &age in ages {
                let t = (age / lifespan) as f32;
                if !(0.0..=1.0).contains(&t) {
                    continue;
                }
                let alpha = (rain_fade(t) * opacity * 255.0).round() as u8;
                if alpha == 0 {
                    continue;
                }
                if block_style {
                    // 方块串：整块随年龄匀速上飘
                    let bh = (w * 0.58).max(2.0);
                    let y = cell_top - age as f32 * speed - bh;
                    let r = Rect::from_min_max(
                        Pos2::new(cx - w * 0.5, y),
                        Pos2::new(cx + w * 0.5, y + bh),
                    );
                    painter.rect_filled(r, 1.5, rain_color(heat, alpha));
                } else {
                    // 生长竖条：底端钉在键格顶边，顶端 = 年龄 × 速度（到顶由 clip 截断）
                    let rise = (age as f32 * speed).min(rain_area_h(cfg));
                    let bottom = cell_top;
                    let top = bottom - rise.max(1.0);
                    if top >= bottom {
                        continue;
                    }
                    let (xl, xr) = (cx - w * 0.5, cx + w * 0.5);
                    // 顶端渐隐、底端实色 —— 用 Mesh 顶点色做竖向渐变
                    let mut mesh = egui::Mesh::default();
                    mesh.colored_vertex(Pos2::new(xl, top), rain_color(heat, 0));
                    mesh.colored_vertex(Pos2::new(xr, top), rain_color(heat, 0));
                    mesh.colored_vertex(Pos2::new(xl, bottom), rain_color(heat, alpha));
                    mesh.colored_vertex(Pos2::new(xr, bottom), rain_color(heat, alpha));
                    mesh.add_triangle(0, 1, 2);
                    mesh.add_triangle(1, 3, 2);
                    painter.add(egui::Shape::mesh(mesh));
                }
            }
        }
    }
}
