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
//! - 渐隐：**按位置**。轨道顶部往下 FADE_BAND_FRAC 那一段是渐隐带，带内越靠上越透明，
//!   到轨道顶刚好 0，带下方完全满亮。于是任何一条雨（不论多长、按了多久、第几列）
//!   只要长进这一段就会"上端先化开"，视觉上像贴着轨道顶淡出，而不是整条一起变淡。
//!   曲线用 smoothstep 而非线性：线性在带的下边界有斜率突变，雨条经过时会出现一条
//!   横向的"折痕"，那正是主人报过的"下方一定区域突然出现渐隐"；
//!   smoothstep 在两端导数都为 0，进出渐隐带都是连续的；
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

/// 渐隐带占轨道高度的默认比例（配置项 kps_rain_fade_pct 为 0 时用它）。
///
/// 0.35 是手调的结果：太小（0.1 以下）时渐变挤在顶部几个像素里，看上去就是"到顶被砍断"；
/// 太大（0.6 以上）时整根雨都半透明，反而失去"实心柱向上冲、冲到顶才化开"的观感。
pub(crate) const FADE_BAND_FRAC_DEFAULT: f32 = 0.35;

/// 渐隐带占轨道高度的比例（5%~90%，来自配置；0 视为默认）。
fn fade_band_frac(cfg: &Config) -> f32 {
    if cfg.kps_rain_fade_pct == 0 {
        FADE_BAND_FRAC_DEFAULT
    } else {
        (cfg.kps_rain_fade_pct.clamp(5, 90) as f32 / 100.0).max(0.05)
    }
}

/// 渐隐带高度：轨道高度 × 比例（至少 1px，避免除零）。
fn fade_band_h(h: f32, cfg: &Config) -> f32 {
    (h * fade_band_frac(cfg)).max(1.0)
}

/// 轨道上某个高度处的透明度系数 —— **键雨渐隐的唯一入口，按位置计算**。
///
/// from_cell 是该点到键格顶边的高度（0 = 贴着键格，h = 轨道顶）。
/// - 渐隐带下方（from_cell <= h - band）：返回 1.0，完全满亮；
/// - 带内：从 1.0 平滑降到 0.0，轨道顶恰好为 0。
///
/// 为什么按位置而不是按时间/按前沿：主人要的是"顶端往下一段区域内透明度逐渐下降"，
/// 这是**空间**上的效果 —— 条只要长进这段就会上端化开，与它多长、按了多久、在第几列无关。
/// 曾经试过的"整条统一 alpha、随前沿一起变淡"看起来是"整条一起消失"，不符合这个预期。
///
/// 曲线用 smoothstep（inv*inv*(3-2inv)，inv = 1-t）：线性函数在带的下边界处
/// 斜率从 0 突变为常数，条经过那条界线时会出现一道横向"折痕"；
/// smoothstep 两端导数都为 0，雨进入 / 离开渐隐带都是无缝的。
fn alpha_at(from_cell: f32, h: f32, cfg: &Config) -> f32 {
    if h <= 0.0 {
        return 1.0;
    }
    let band = fade_band_h(h, cfg);
    let band_start = h - band;
    if from_cell <= band_start {
        return 1.0;
    }
    // 距轨道顶的相对位置：0 = 恰在带的下边界，1 = 已到轨道顶
    let t = ((from_cell - band_start) / band).clamp(0.0, 1.0);
    let inv = 1.0 - t;
    inv * inv * (3.0 - 2.0 * inv)
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
                emit_bar(
                    &painter, cx, w, cell_top, vis_top, vis_bottom, base_alpha, h, cfg, row_base,
                );
            }
        }
    }
}

/// 画一根竖条：从 vis_top（距键格的高度）到 vis_bottom，**沿竖直方向做渐隐**。
///
/// 用 Mesh 而不是 rect_filled：整根是一个矩形，但上下两端的颜色不同，
/// 插值后条内就是一条连续的渐变。关键是要在**渐隐带下边界**（h - band）处切一刀、
/// 插入一排顶点 —— 否则整条只有端点两个 alpha，一条比渐隐带还长的雨会被插值成
/// "整条都在缓缓变淡"，就不再有"下半截实心、只有上端化开"的观感了。
///
/// 切完之后每个子段内部 alpha 都是线性的，且相邻子段在接缝处取值相同，
/// 所以整条雨是连续的：既不会出现横向断层，也能正确表达带下方满亮、带内向上淡到 0。
#[allow(clippy::too_many_arguments)]
fn emit_bar(
    painter: &egui::Painter,
    cx: f32,
    w: f32,
    cell_top: f32,
    vis_top: f32,
    vis_bottom: f32,
    base_alpha: f32,
    h: f32,
    cfg: &Config,
    base: Color32,
) {
    // vis_* 是「距键格顶边的高度」：数值大的一端在屏幕上更靠上（y 更小）。
    let (xl, xr) = (cx - w * 0.5, cx + w * 0.5);
    let to_color = |vis: f32| {
        let a = (base_alpha * alpha_at(vis, h, cfg) * 255.0).round().clamp(0.0, 255.0) as u8;
        Color32::from_rgba_unmultiplied(base.r(), base.g(), base.b(), a)
    };

    // 顶点按 vis 升序（= 屏幕上从上往下）排列，相邻两两成段。
    let band_start = h - fade_band_h(h, cfg);
    let mut cuts = vec![vis_bottom, vis_top];
    if band_start > vis_bottom && band_start < vis_top {
        cuts.push(band_start); // 渐隐带下边界必须是一个顶点
    }
    cuts.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    cuts.dedup_by(|a, b| (*a - *b).abs() < 0.01);

    // 整条都不可见就别提交图元
    let any_visible = cuts
        .iter()
        .any(|&v| (base_alpha * alpha_at(v, h, cfg) * 255.0).round() >= 1.0);
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
    // 每对相邻顶点（i*2, i*2+1 与 i*2+2, i*2+3）组成一个四边形
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

    /// 测试用的默认配置（渐隐带取默认 35%）。
    fn cfg_default() -> Config {
        Config::default()
    }

    /// 渐隐是**按位置**的：带下方满亮、带内单调下降、轨道顶恰好 0。
    ///
    /// 这是主人要的那句「顶端往下一段区域内透明度逐渐下降」的回归守卫 ——
    /// 曾经的实现让整条雨按前沿一起变淡，效果变成「整条一起消失」，方向就错了。
    #[test]
    fn alpha_depends_on_position_only() {
        let h = 96.0f32;
        let band = fade_band_h(h, &cfg_default());
        let band_start = h - band;
        // 带下方（含边界）满亮
        assert_eq!(alpha_at(0.0, h, &cfg_default()), 1.0, "贴键格处必须满亮");
        assert_eq!(alpha_at(band_start, h, &cfg_default()), 1.0, "渐隐带下边界仍应满亮");
        assert_eq!(alpha_at(band_start - 1.0, h, &cfg_default()), 1.0, "带下方必须满亮");
        // 带内
        let mid = band_start + band * 0.5;
        let a_mid = alpha_at(mid, h, &cfg_default());
        assert!(a_mid < 1.0 && a_mid > 0.0, "带中段应在 0..1 之间，实际 {a_mid}");
        // 轨道顶恰好 0
        assert_eq!(alpha_at(h, h, &cfg_default()), 0.0, "轨道顶必须刚好透明");
        // 越过轨道顶保持 0（前沿出顶后的裁剪段落不会反向变亮）
        assert_eq!(alpha_at(h * 1.5, h, &cfg_default()), 0.0, "越过轨道顶必须保持透明");
    }

    /// 在同一高度上，alpha 与「谁在按、按了多久、第几列」完全无关。
    ///
    /// alpha_at 只接受位置与轨道高度两个参数，天然满足；这里把它写成测试，
    /// 是为了防止以后有人给 alpha 再加「时长 / 前沿」之类的入参。
    #[test]
    fn alpha_is_independent_of_bar_length_or_column() {
        let h = 200.0f32;
        for &height in &[0.0f32, 37.0, 99.5, 130.0, 150.0, 199.0, 200.0] {
            let expected = alpha_at(height, h, &cfg_default());
            // 同一位置的多次求值必须完全一致
            assert_eq!(alpha_at(height, h, &cfg_default()), expected);
            // 且在合法范围内
            assert!((0.0..=1.0).contains(&expected), "alpha 越界：{expected}");
        }
    }

    /// 曲线必须连续单调：不能出现「下方突然出现一条渐隐」那种断层。
    #[test]
    fn alpha_is_continuous_and_monotonic() {
        let h = 96.0f32;
        let steps = 400;
        let mut prev = alpha_at(0.0, h, &cfg_default());
        for i in 1..=steps {
            let x = h * (i as f32) / (steps as f32);
            let v = alpha_at(x, h, &cfg_default());
            assert!(
                v <= prev + 1e-4,
                "随高度增加必须单调不增：h={x} 时 {v} > {prev}"
            );
            // 相邻取样间隔约 0.24px，跳变超过 0.02 就说明存在硬边
            assert!(
                (prev - v).abs() < 0.02,
                "渐变必须连续：x={x} 处一次跳变 {}",
                (prev - v).abs()
            );
            prev = v;
        }
    }

    /// smoothstep 在带的下边界与轨道顶导数都为 0：没有折角。
    #[test]
    fn alpha_has_no_kink_at_band_edges() {
        let h = 96.0f32;
        let band = fade_band_h(h, &cfg_default());
        let start = h - band;
        let d = 0.01f32;
        let at_start = (alpha_at(start + d, h, &cfg_default()) - alpha_at(start, h, &cfg_default())) / d;
        let at_end = (alpha_at(h, h, &cfg_default()) - alpha_at(h - d, h, &cfg_default())) / d;
        assert!(at_start.abs() < 1e-2, "带下边界斜率应≈0，实际 {at_start}");
        assert!(at_end.abs() < 1e-2, "轨道顶斜率应≈0，实际 {at_end}");
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
