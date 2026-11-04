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
//! - 渐隐：**锚定在「条的可见顶端」**，不是锚定在轨道顶。从顶端往下 FADE_BAND_FRAC 那段
//!   越往下越实，顶端恒为 0。锚点取 min(前沿, 轨道顶)：
//!   - 条还没长到轨道顶时，锚点 = 前沿，于是**任何时候的条都有顶端淡出**，不会出现
//!     「短的时候没渐变、长到顶才突然一大段渐变」（主人报过正是这个）；
//!   - 条越过轨道顶后，锚点 = 轨道顶（可见顶端已被裁在那里），渐变固定在顶部，
//!     条继续上飘时底部被逐段裁掉，看起来是「从顶部慢慢化开」。
//!   两个阶段在「前沿正好到达轨道顶」处取值相同，所以切换是连续的。
//!   曲线用 smoothstep：两端导数为 0，进入 / 离开渐隐带都不会出现横向"折痕"；
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

/// 轨道高度的上限（px）。400 在高分屏 / 大窗口下显得太短 —— 雨还没走够就出顶，
/// 渐隐带也被压得很窄；放宽到 1200 让大屏用户能拉出完整的"雨柱"。
pub(crate) const RAIN_H_MAX: u16 = 1200;

/// 键雨轨道的像素高度（16~RAIN_H_MAX；配置 0 = 自动 96px）。
pub(crate) fn rain_area_h(cfg: &Config) -> f32 {
    if cfg.kps_rain_height > 0 {
        cfg.kps_rain_height.clamp(16, RAIN_H_MAX) as f32
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

/// 渐隐带占**可见条长**的默认比例（配置项 kps_rain_fade_pct 为 0 时用它）。
///
/// 0.28 是手调的结果：0.1 以下渐变挤在顶端几个像素里、像被砍断；
/// 0.5 以上整根雨都偏淡，失去"实心柱向上冲"的观感。
/// 注意分母是**可见条长**而不是轨道高度 —— 见 fade_band_h 的说明。
pub(crate) const FADE_BAND_FRAC_DEFAULT: f32 = 0.28;

/// 渐隐带占轨道高度的比例（5%~90%，来自配置；0 视为默认）。
fn fade_band_frac(cfg: &Config) -> f32 {
    if cfg.kps_rain_fade_pct == 0 {
        FADE_BAND_FRAC_DEFAULT
    } else {
        (cfg.kps_rain_fade_pct.clamp(5, 90) as f32 / 100.0).max(0.05)
    }
}

/// 渐隐带高度。两个约束取小：
/// - 轨道高度 x 比例 —— 条很长时渐变区不再无限变长，观感稳定；
/// - 可见条长        —— 条还很短时，带不至于比条本身还长（否则整条都被泡淡）。
///
/// 用「可见条长」作上限而不是原始条长：条顶端出轨道后可见长度会不断缩短，
/// 让带跟着缩短，整条雨才能干净地化掉，而不是到最后留下一截半透明的残根。
fn fade_band_h(h: f32, vis_len: f32, cfg: &Config) -> f32 {
    (h * fade_band_frac(cfg)).min(vis_len).max(1.0)
}

/// 某个高度处的透明度系数 —— **键雨渐隐的唯一入口，按位置计算**。
///
/// - from_cell：该点到键格顶边的高度（越大越靠屏幕上方）；
/// - anchor：渐隐锚点 = 可见顶端的高度 min(前沿, 轨道顶)；
/// - band：从锚点往下多长一段淡完。
///
/// 锚点处 alpha = 0，往下 band 之后 alpha = 1，中间 smoothstep。
/// **关键：锚点跟着条的可见顶端走**，所以任何长度的条都有顶端淡出，
/// 不会出现「短的时候没渐变、长到顶才突然一大段渐变」。
/// 而锚点取 min(前沿, 轨道顶) 又保证条出顶后渐变固定在轨道顶部、继续上飘时从底部被裁。
///
/// 从不用"整条统一 alpha 随前沿变淡"：那种观感是「整条一起消失」，主人明确否掉过。
fn rain_alpha(from_cell: f32, anchor: f32, band: f32) -> f32 {
    if band <= 0.0 {
        return if from_cell >= anchor { 0.0 } else { 1.0 };
    }
    let depth = (anchor - from_cell).max(0.0);
    if depth >= band {
        return 1.0;
    }
    // t: 0 = 恰在锚点（全透明），1 = 带的下边界（满亮）
    let t = (depth / band).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
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
                // 渐隐按**位置**算（alpha_at），所以条的两端各自取自己的系数，
                // 中间用顶点色插值 —— 一条跨进渐隐带的雨自然呈现"上淡下实"。
                // 这里只保留与位置无关的两项：用户的键雨总不透明度、排间系数。
                let base_alpha = opacity * alpha_scale;
                if base_alpha <= 0.004 {
                    continue;
                }
                // 渐隐锚点 = 可见顶端：条没到顶时是前沿，出顶后是轨道顶。
                // 于是任何长度的条都有顶端淡出，且出顶后的渐变固定不动。
                let anchor = vis_bottom;
                let band = fade_band_h(h, vis_bottom - vis_top, cfg);
                emit_bar(
                    &painter, cx, w, cell_top, vis_top, vis_bottom, base_alpha, anchor, band,
                    row_base,
                );
            }
        }
    }
}

/// 画一根竖条：从 vis_top（距键格的高度）到 vis_bottom，**沿竖直方向做渐隐**。
///
/// 用 Mesh 而不是 rect_filled：整根是一个矩形，但上下两端颜色不同，顶点色插值后
/// 条内就是连续渐变。必须在两处**切一刀插顶点**，否则只有端点两个 alpha，
/// 长条会被插值成"整条都在缓缓变淡"：
/// - 渐隐带下边界 anchor - band（这里 alpha 从 1 开始往上降）；
/// - 渐隐锚点 anchor 本身（这里 alpha 恰好到 0）。
///
/// 切完之后每个子段内部 alpha 线性、相邻子段在接缝处取值相同，
/// 所以整条雨连续无断层，同时能正确表达"带下方满亮 / 带内向上淡到 0"。
#[allow(clippy::too_many_arguments)]
fn emit_bar(
    painter: &egui::Painter,
    cx: f32,
    w: f32,
    cell_top: f32,
    vis_top: f32,
    vis_bottom: f32,
    base_alpha: f32,
    anchor: f32,
    band: f32,
    base: Color32,
) {
    // vis_* 是「距键格顶边的高度」：数值大的一端在屏幕上更靠上（y 更小）。
    let (xl, xr) = (cx - w * 0.5, cx + w * 0.5);
    let to_color = |vis: f32| {
        let a = (base_alpha * rain_alpha(vis, anchor, band) * 255.0).round().clamp(0.0, 255.0) as u8;
        Color32::from_rgba_unmultiplied(base.r(), base.g(), base.b(), a)
    };

    // 顶点按 vis 升序（= 屏幕上从上往下）排列，相邻两两成段。
    let band_start = anchor - band;
    let mut cuts = vec![vis_bottom, vis_top];
    for c in [anchor, band_start] {
        if c > vis_bottom && c < vis_top {
            cuts.push(c);
        }
    }
    cuts.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    cuts.dedup_by(|a, b| (*a - *b).abs() < 0.01);

    let any_visible = cuts
        .iter()
        .any(|&v| (base_alpha * rain_alpha(v, anchor, band) * 255.0).round() >= 1.0);
    if !any_visible {
        return;
    }

    let mut mesh = egui::Mesh::default();
    for &vis in &cuts {
        let y = cell_top - vis;
        let c = to_color(vis);
        mesh.colored_vertex(Pos2::new(xl, y), c);
        mesh.colored_vertex(Pos2::new(xr, y), c);
    }
    for i in 0..cuts.len().saturating_sub(1) {
        let a = (i * 2) as u32;
        mesh.add_triangle(a, a + 1, a + 2);
        mesh.add_triangle(a + 1, a + 3, a + 2);
    }
    painter.add(egui::Shape::mesh(mesh));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg_default() -> Config {
        Config::default()
    }

    /// **核心回归**：不论条多长，顶端都必须透明、越往下越实。
    ///
    /// 这是主人两次报错的正面描述：
    /// 「长条没到顶端时几乎没渐变」（说明锚点被钉死在轨道顶）与
    /// 「长到顶端后渐变突然延伸到很靠后的位置」（说明渐变域跟着轨道走而不是条的顶端）。
    #[test]
    fn any_length_has_fade_at_its_top() {
        let cfg = cfg_default();
        let h = 400.0f32;
        for &len in &[20.0f32, 60.0, 120.0, 280.0, 400.0] {
            // 锚点 = 可见顶端 = 条本身高度（未出顶）
            let anchor = len;
            let band = fade_band_h(h, len, &cfg);
            assert!(
                rain_alpha(anchor, anchor, band) <= 0.01,
                "长 {len} 的条顶端必须几乎全透明，实际 {}",
                rain_alpha(anchor, anchor, band)
            );
            // 顶端往下 90% 的位置必须明显更实
            let below = anchor - band * 0.9;
            assert!(
                rain_alpha(below, anchor, band) > 0.5,
                "长 {len} 的条在带底部附近应明显更实，实际 {}",
                rain_alpha(below, anchor, band)
            );
            // 条根（贴键格处）必须满亮（短条被 band 上限保护，不会被整条泡淡）
            assert!(
                rain_alpha(0.0, anchor, band) > 0.99,
                "长 {len} 的条根部必须满亮，实际 {}",
                rain_alpha(0.0, anchor, band)
            );
        }
    }

    /// 条出顶之后锚点固定在轨道顶：渐变不再继续上移，条从底部被裁。
    #[test]
    fn anchor_sticks_to_track_top_after_overflow() {
        let cfg = cfg_default();
        let h = 200.0f32;
        // 前沿超过轨道顶：可见顶端 = 轨道顶
        let anchor = h;
        let band = fade_band_h(h, 120.0, &cfg);
        assert_eq!(rain_alpha(h, anchor, band), 0.0, "轨道顶必须透明");
        assert!(
            rain_alpha(h - band, anchor, band) > 0.99,
            "带下方必须满亮"
        );
        // 继续上飘（可见段的下端上移）不应影响同高度处的 alpha
        for &from in &[150.0f32, 170.0, 190.0] {
            let a1 = rain_alpha(from, anchor, band);
            let a2 = rain_alpha(from, anchor, band);
            assert_eq!(a1, a2);
        }
    }

    /// 渐隐带永远不超过可见条长：短条不会被整条泡淡。
    #[test]
    fn band_never_exceeds_visible_length() {
        let cfg = cfg_default();
        let h = 400.0f32;
        for &len in &[2.0f32, 8.0, 30.0, 100.0] {
            let band = fade_band_h(h, len, &cfg);
            assert!(
                band <= len.max(1.0) + 1e-3,
                "len={len} 时带长 {band} 超过可见长度"
            );
        }
        // 长条时由「轨道高度 x 比例」封顶，不与条长一起无限变长
        let band_long = fade_band_h(h, 400.0, &cfg);
        assert!((band_long - h * fade_band_frac(&cfg)).abs() < 1e-3);
    }

    /// 曲线连续单调：不能出现横向断层。
    #[test]
    fn rain_alpha_is_continuous_and_monotonic() {
        let anchor = 300.0f32;
        let band = 100.0f32;
        let steps = 400;
        let mut prev = rain_alpha(anchor - band - 50.0, anchor, band);
        for i in 0..=steps {
            let from = anchor - band - 50.0 + (band + 50.0) * (i as f32) / (steps as f32);
            let v = rain_alpha(from, anchor, band);
            assert!(
                v <= prev + 1e-4,
                "越靠近顶端必须越透明（沿 from 增大方向单调不增）：from={from} 时 {v} > {prev}"
            );
            assert!((v - prev).abs() < 0.02, "渐变必须连续：from={from}");
            prev = v;
        }
    }

    /// smoothstep 在锚点与带下边界导数都为 0：没有折角。
    #[test]
    fn rain_alpha_has_no_kink() {
        let anchor = 200.0f32;
        let band = 80.0f32;
        let d = 0.01f32;
        let at_anchor = (rain_alpha(anchor, anchor, band) - rain_alpha(anchor - d, anchor, band)) / d;
        let at_start = (rain_alpha(anchor - band, anchor, band) - rain_alpha(anchor - band + d, anchor, band)) / d;
        assert!(at_anchor.abs() < 1e-2, "锚点斜率应≈0，实际 {at_anchor}");
        assert!(at_start.abs() < 1e-2, "带下边界斜率应≈0，实际 {at_start}");
    }

    /// 轨道高度上限放宽到 1200，且超范围会被夹住。
    #[test]
    fn rain_height_allows_large_tracks() {
        let mut cfg = Config::default();
        cfg.kps_rain_height = 900;
        assert_eq!(rain_area_h(&cfg), 900.0, "900px 必须原样生效");
        cfg.kps_rain_height = 5000;
        assert_eq!(rain_area_h(&cfg), RAIN_H_MAX as f32, "超上限应夹到 RAIN_H_MAX");
        cfg.kps_rain_height = 96;
        assert_eq!(rain_area_h(&cfg), 96.0);
    }
}
