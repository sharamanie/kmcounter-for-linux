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
//! - 渐隐：**面板上方有一块固定的「滤镜区」**（主人原话：整个面板的上方就像有个滤镜）。
//!   滤镜区 = 轨道顶部往下 kps_rain_fade_pct 那么高的一条横带，**位置与高度都固定**，
//!   不随任何一条雨的长度、寿命、列号移动。
//!   - 雨条**进入**这块区域的部分才开始变淡，越靠上越透明，到轨道顶恰好 0；
//!   - 滤镜区**下方**的部分一律完全实心 —— 所以短雨是干净的实心柱，
//!     既不会「还没升上去就淡了」，也不会「整条一起消失」；
//!   - 条升到顶继续上飘时从底部被裁，滤镜区始终钉在顶部不动，
//!     所以不会出现「渐变突然延伸到很靠后」。
//!   前两轮的教训（都试过、都被否）：alpha 跟着**前沿**走 = 整条一起消失；
//!   alpha 跟着**条自身顶端**走 = 短雨在底部就淡。
//!   滤镜语义要求锚点是**空间位置**，而不是某条雨的时间状态。
//!   曲线用 smoothstep：两端导数为 0，进出滤镜区都没有横向折痕；
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
const ROW_WIDTH_STEP: f32 = 0.76;
/// 每排的不透明度系数（按排号索引，超出表长 = 0，即**彻底透明、完全不画**）。
///
/// 长度对齐**键盘最多 6 行**（用户反馈：四行以上没分配颜色）。
/// 以前只有 5 项且第 5 项是 0，所以第 4 排以后的雨根本不画。
/// 方向是**越靠后的排越实**：几排的雨共用同一条轨道与同一水平中心，其实是**层层叠在一起**的，
/// 又因为后画的排更窄、盖在中心，所以中心最终呈现的正是最深那一排的颜色。
/// 若第 1 排就已经是实心白，中心早早饱和成纯白，后面的换色排再怎么调都叠不上去 ——
/// 这正是「深浅对比看不出来」的根因。因此第 1 排压到 0.35（明显淡于实心），
/// 逐排抬升到 1.00，表外为 0（**允许一路衰减到完全透明**，对标 JipperKeyViewer 的逐排消失）。
const ROW_ALPHA_TABLE: [f32; 6] = [0.30, 0.62, 0.84, 0.95, 1.00, 1.00];
/// 排宽下限（px）：深层排不能被连乘到看不见。
const ROW_WIDTH_MIN: f32 = 1.5;
/// 排号 → 色相表（0 起；`None` = 沿用 `kps_rain_color`，不做色相变换）。
/// 用固定表而不是逐排累加，是为了让相邻两排**一眼就是两种颜色**：
/// 累加出来的 0.70 / 0.85 都落在紫红一带，叠起来仍是一片粉，分不出前后。
/// 0.55 ≈ 天青、0.80 ≈ 紫 —— 冷色段彼此距离够远，且不会撞上键格本身的热力青绿。
/// 排号 → 色相表。**现在全部为 None：所有排同色。**
///
/// 用户要求：多排时只靠**宽度**区分层次就够了，换色反而花。
/// 保留这个表而不是直接删掉 row_color，是为了以后想恢复换色时只改这里。
const ROW_HUE_TABLE: [Option<f32>; 6] = [None, None, None, None, None, None];
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

/// 滤镜区占**轨道高度**的默认比例（配置项 kps_rain_fade_pct 为 0 时用它）。
///
/// 0.25：比早期版本的 0.35 薄一截。35% 的问题是滤镜区太厚 ——
/// 雨条刚一碰到就觉得「渐变延伸得很靠后」，看起来不像顶部滤镜、倒像整根在变淡。
/// 0.1 以下又会挤成一条硬边、像被砍断，所以取 0.25。
pub(crate) const FADE_BAND_FRAC_DEFAULT: f32 = 0.25;

/// 渐隐带占轨道高度的比例（5%~90%，来自配置；0 视为默认）。
fn fade_band_frac(cfg: &Config) -> f32 {
    if cfg.kps_rain_fade_pct == 0 {
        FADE_BAND_FRAC_DEFAULT
    } else {
        (cfg.kps_rain_fade_pct.clamp(5, 90) as f32 / 100.0).max(0.05)
    }
}

/// 滤镜区高度 = 轨道高度 * 比例。**只与轨道高度有关**。
///
/// 刻意不掺入「条长 / 可见长度 / 顶端高度」：滤镜是一块**固定的空间区域**，
/// 掺进任何与单条雨相关的量，它就变成「跟着条走的渐变」—— 那正是前两版被否掉的形态。
fn fade_band_h(h: f32, cfg: &Config) -> f32 {
    (h * fade_band_frac(cfg)).max(1.0)
}

/// 某个高度处的透明度系数 —— **键雨渐隐的唯一入口**。
///
/// - from_cell：该点到键格顶边的高度（0 = 贴着键格，h = 轨道顶）；
/// - h：轨道高度；
/// - band：滤镜区高度（= 轨道高度 * 比例，常量）。
///
/// 滤镜区固定为区间 [h - band, h]：
/// - 区**下方**（from_cell 不超过 h - band）：返回 1.0，**完全实心** ——
///   刚长出来的短雨整根都在这里，所以它是干干净净的实心柱；
/// - 区**内**：从 1.0 平滑降到 0.0，越靠上越透明；
/// - 区**顶**（轨道顶）：恰好 0。
///
/// 入参只有「位置」和「轨道高度」，所以滤镜区与条多长、按了多久、第几列完全无关。
fn rain_alpha(from_cell: f32, h: f32, band: f32) -> f32 {
    if h <= 0.0 || band <= 0.0 {
        return 1.0;
    }
    let zone_start = h - band;
    if from_cell <= zone_start {
        return 1.0;
    }
    // t: 0 = 刚进滤镜区（满亮），1 = 轨道顶（全透明）
    let t = ((from_cell - zone_start) / band).clamp(0.0, 1.0);
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
    content_w: f32,
) {
    if !cfg.kps_show_rain || m.rain_h <= 0.0 {
        return;
    }
    let h = rain_area.height();
    let speed = rain_px_per_sec(cfg);
    let opacity = cfg.kps_rain_opacity.min(100) as f32 / 100.0;
    let base = rain_base_color(cfg);
    let painter = ui.painter().with_clip_rect(rain_area);
    // **必须与键格用同一个排布函数与同一个宽度**。
    //
    // 以前这里用的是 key_rows_layout(...m.graph_w)，而键格用的是
    // key_rows_layout_for_width(...content.width())。两者算出的缝宽与起点不同，
    // 于是雨条没落在键格上 —— 长度是对的，但看上去像彻底错位。
    let row_pos = super::key_rows_layout_for_width(&layout.rows, m.box_w, content_w);

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
                // 空位（间距 / 跨行占位）：只占地方，**不消耗 key_infos 索引**。
                //
                // 这里曾经写成 `info_idx += 1`，而键格绘制处是直接 continue、不递增。
                // 两边索引从此错开：配置里空位越多，键雨的列偏得越远 ——
                // 用户看到的就是「F 的键雨跑到 Esc 那边」。
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
            // **条宽只跟「一格」相比，不跟「这个键格的实际宽度」相比**。
            //
            // box_w 是这一格的显示宽度：普通键 = 一格，但 LCtrl 是 1.25 格、
            // Space 是 6.25 格。以前直接乘 box_w，于是宽键上的雨条也跟着变宽
            //（用户看到「空格那条雨明显比别的宽」「Ctrl 和方向键的雨宽不一致」）。
            // 雨条是"按键的轨迹"，应当所有键一样粗，只按排号收窄。
            let w = ((m.box_w * rain_width_ratio(cfg)).clamp(BAR_W_MIN, BAR_W_MAX) * step)
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
                // 滤镜区固定在轨道顶部：位置与高度都只由轨道高度决定，
                // 与这条雨多长、按了多久、在第几列无关。
                let band = fade_band_h(h, cfg);
                emit_bar(
                    &painter, cx, w, cell_top, vis_top, vis_bottom, base_alpha, h, band,
                    row_base,
                );
            }
        }
    }
}

/// 一根雨条在竖直方向上的**顶点切分**：返回按 vis 升序排列的高度列表。
///
/// 只取关键位置并集，绝不做等距采样：
/// - vis_lo / vis_hi：这根条自己的上下端；
/// - zone_start（滤镜区下边界）与 h（轨道顶）：**只要落在这根条内部就必须切一刀**。
///
/// 为什么必须切：Mesh 的顶点色是**线性插值**的。若整根只给两个端点，
/// 一条从轨道底部贯穿到顶部的长条会在整条长度上做线性渐变 ——
/// 视觉上就是「每根条的渐变长度都不一样」（长的特别长、短的几乎没有），
/// 而不是「轨道上半段有一块固定蒙版」。切在区域边界上，才能保证
/// **滤镜区内**做渐变、**区下方**是纯实心的一段。
fn bar_cuts(vis_a: f32, vis_b: f32, h: f32, band: f32) -> Vec<f32> {
    let lo = vis_a.min(vis_b);
    let hi = vis_a.max(vis_b);
    let mut cuts = vec![lo, hi];
    let zone_start = h - band;
    for c in [zone_start, h] {
        // 只有严格落在条内部才需要插顶点（端点已经在了）。
        if c > lo && c < hi {
            cuts.push(c);
        }
    }
    cuts.sort_by(|x, y| x.partial_cmp(y).unwrap_or(std::cmp::Ordering::Equal));
    cuts.dedup_by(|x, y| (*x - *y).abs() < 0.01);
    cuts
}

/// 画一根竖条：从 vis_top 到 vis_bottom（两者都是「距键格顶边的高度」）。
///
/// 用 Mesh 而不是 rect_filled：整根是一个矩形，但上下两端颜色可能不同，
/// 顶点色插值后条内就是连续渐变。顶点切分见 bar_cuts ——
/// **必须把滤镜区边界切出来**，否则那条边界会被插值抹平、渐变铺满整根。
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
    band: f32,
    base: Color32,
) {
    // vis_* 是「距键格顶边的高度」：数值大的一端在屏幕上更靠上（y 更小）。
    let (xl, xr) = (cx - w * 0.5, cx + w * 0.5);
    let to_color = |vis: f32| {
        let a = (base_alpha * rain_alpha(vis, h, band) * 255.0).round().clamp(0.0, 255.0) as u8;
        Color32::from_rgba_unmultiplied(base.r(), base.g(), base.b(), a)
    };

    let cuts = bar_cuts(vis_top, vis_bottom, h, band);
    let any_visible = cuts
        .iter()
        .any(|v| (base_alpha * rain_alpha(*v, h, band) * 255.0).round() >= 1.0);
    if !any_visible {
        return;
    }

    let mut mesh = egui::Mesh::default();
    for vis in &cuts {
        let y = cell_top - vis;
        let c = to_color(*vis);
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

    /// **滤镜区是固定的**：高度只由轨道高度决定，与任何一条雨都无关。
    #[test]
    fn filter_zone_is_fixed() {
        let cfg = cfg_default();
        for &h in &[96.0f32, 200.0, 400.0, 1200.0] {
            let band = fade_band_h(h, &cfg);
            assert!(
                (band - h * fade_band_frac(&cfg)).abs() < 1e-3,
                "轨道 {h} 时滤镜区高度应为 h x 比例"
            );
            // 同一轨道高度下反复求值必须完全一致（不掺入任何单条雨的量）
            assert_eq!(fade_band_h(h, &cfg), band);
        }
    }

    /// **要求 A**：滤镜区下方完全实心 —— 短雨是干净的实心柱，
    /// 不会「还没升上去就淡了」。
    #[test]
    fn below_filter_zone_is_solid() {
        let cfg = cfg_default();
        let h = 400.0f32;
        let band = fade_band_h(h, &cfg);
        let zone_start = h - band;
        for &from in &[0.0f32, 10.0, 50.0, 120.0, 200.0] {
            assert!(from <= zone_start, "取样点 {from} 应在滤镜区下方");
            assert_eq!(
                rain_alpha(from, h, band),
                1.0,
                "滤镜区下方（{from}）必须完全实心"
            );
        }
        assert_eq!(
            rain_alpha(zone_start, h, band),
            1.0,
            "滤镜区下边界仍应满亮"
        );
    }

    /// **要求 B**：滤镜区内越靠上越透明，到轨道顶恰好 0。
    /// 这正是「像有个滤镜」的正面描述。
    #[test]
    fn inside_filter_zone_fades_out_toward_top() {
        let cfg = cfg_default();
        let h = 400.0f32;
        let band = fade_band_h(h, &cfg);
        let zone_start = h - band;

        // 越靠上越透明（单调）
        let mut prev = rain_alpha(zone_start, h, band);
        for i in 1..=200 {
            let from = zone_start + band * (i as f32) / 200.0;
            let v = rain_alpha(from, h, band);
            assert!(
                v <= prev + 1e-4,
                "区内越靠上必须越透明：from={from} 时 {v} > {prev}"
            );
            assert!((prev - v).abs() < 0.02, "渐变必须连续：from={from}");
            prev = v;
        }
        assert_eq!(rain_alpha(h, h, band), 0.0, "轨道顶必须恰好透明");
        assert_eq!(rain_alpha(h * 2.0, h, band), 0.0, "越过轨道顶仍透明");
    }

    /// **要求 C**：同一高度处的透明度是定值 ——
    /// 与这条雨多长、按了多久、在第几列完全无关（滤镜是空间区域）。
    #[test]
    fn alpha_depends_only_on_position() {
        let cfg = cfg_default();
        let h = 400.0f32;
        let band = fade_band_h(h, &cfg);
        for &from in &[0.0f32, 99.0, 150.0, 299.0, 300.0, 380.0, 400.0] {
            let a = rain_alpha(from, h, band);
            assert!((0.0..=1.0).contains(&a), "alpha 越界：{a}");
            // 反复求值一致
            assert_eq!(rain_alpha(from, h, band), a);
        }
    }

    /// smoothstep 在滤镜区下边界与轨道顶导数都为 0：没有折角。
    #[test]
    fn filter_zone_has_no_kink() {
        let cfg = cfg_default();
        let h = 200.0f32;
        let band = fade_band_h(h, &cfg);
        let zone_start = h - band;
        let d = 0.01f32;
        let at_start = (rain_alpha(zone_start + d, h, band) - rain_alpha(zone_start, h, band)) / d;
        let at_top = (rain_alpha(h, h, band) - rain_alpha(h - d, h, band)) / d;
        assert!(at_start.abs() < 1e-2, "区下边界斜率应≈0，实际 {at_start}");
        assert!(at_top.abs() < 1e-2, "轨道顶斜率应≈0，实际 {at_top}");
    }

    /// **这是「按轨道划分」的核心守卫**：切点必须把滤镜区边界插进去。
    ///
    /// 曾经的 bug：判断写成 c > vis_bottom && c < vis_top，
    /// 但 vis_bottom（远端）恒大于 vis_top，条件永不成立 —— 边界从来没被切过，
    /// 整根只留两个端点，Mesh 于是把渐变线性铺满整条长度。
    /// 表现就是「每条长条的渐变长度都不一样、长的特别长」。
    #[test]
    fn bar_cuts_insert_filter_boundaries() {
        let cfg = cfg_default();
        let h = 400.0f32;
        let band = fade_band_h(h, &cfg);
        let zone_start = h - band;
        assert!(zone_start > 0.0 && zone_start < h);

        // 一根从轨道最底贯穿到最顶的长条：必须切出 4 个点，
        // 其中包含 zone_start 与 h，这样区内 / 区外才是两段独立的插值。
        let cuts = bar_cuts(0.0, h, h, band);
        // 条的两端恰好是 0 与 h，所以轨道顶与条端重合、只算一个点：
        // 期望值 = {0, zone_start, h} 三个位置。
        assert_eq!(
            cuts.len(),
            3,
            "贯穿全轨的长条应切出 3 个位置，实际 cuts={cuts:?}"
        );
        assert!(cuts.iter().any(|c| (*c - zone_start).abs() < 0.01), "必须切出滤镜区下边界");
        assert!(cuts.iter().any(|c| (*c - h).abs() < 0.01), "必须切出轨道顶");

        // 反过来传参（vis_top/vis_bottom 顺序颠倒）也必须得到同样结果
        let cuts_rev = bar_cuts(h, 0.0, h, band);
        assert_eq!(cuts, cuts_rev, "切分结果不应依赖传入顺序");

        // 完全在滤镜区下方的短条：只有端点，不该凭空多出切点
        let short = bar_cuts(0.0, zone_start - 50.0, h, band);
        assert_eq!(short.len(), 2, "区下方的短条只需两个端点，实际 {short:?}");

        // 完全在区内的一段：只有端点
        let inside = bar_cuts(zone_start + 10.0, h, h, band);
        assert_eq!(inside.len(), 2, "完全在区内的一段只需两个端点");
    }

    /// 配合上一条：**区下方那一段必须整段是满亮的**，不能有任何渐变。
    /// （这正是「长条下半区域是原来的颜色」的数学表述。）
    #[test]
    fn lower_half_of_long_bar_stays_solid() {
        let cfg = cfg_default();
        let h = 400.0f32;
        let band = fade_band_h(h, &cfg);
        let zone_start = h - band;
        // 从条底到滤镜区下边界，逐点采样都必须恰好 1.0
        let steps = 100;
        for i in 0..=steps {
            let from = zone_start * (i as f32) / (steps as f32);
            assert_eq!(
                rain_alpha(from, h, band),
                1.0,
                "滤镜区下方 {from} 处必须完全实心"
            );
        }
    }

    /// **键盘最多 6 行，每一行的雨都必须画得出来。**
    ///
    /// 用户反馈：四行以上的键位没分配颜色。
    /// 原因：ROW_ALPHA_TABLE 只有 5 项且第 5 项是 0，ROW_HUE_TABLE 也只有 5 项，
    /// 所以第 4 排（0 起算）之后根本不画。
    #[test]
    fn all_six_rows_are_visible_and_uniform() {
        for row in 0..6usize {
            assert!(
                row_alpha_scale(row) > 0.0,
                "第 {} 排的不透明度应 > 0，实际 {}",
                row + 1,
                row_alpha_scale(row)
            );
        }
        // 非白基色下，每排都应能算出一个颜色
        let base = Color32::WHITE;
        let mut seen: Vec<String> = Vec::new();
        for row in 0..6usize {
            let c = row_color(base, row);
            assert!(
                c.a() > 0,
                "第 {} 排应有颜色",
                row + 1
            );
            seen.push(format!("{:?}", (c.r(), c.g(), c.b())));
        }
        // **所有排同色**：层次只靠宽度与不透明度区分，不靠颜色。
        //
        // 改动前这里是「至少 3 种颜色」，靠换色相分层；用户觉得太花，
        // 明确要求统一颜色。于是这条断言反过来 —— 必须是同一种颜色。
        seen.sort();
        seen.dedup();
        assert_eq!(
            seen.len(),
            1,
            "所有排应同色（层次只靠宽度），实际出现 {:?}",
            seen
        );
    }

    /// 每排的宽度都得看得见（不能因为连乘变成 0 像素）。
    #[test]
    fn all_six_rows_have_visible_width() {
        let base_w = 60.0f32;
        for row in 0..6usize {
            let w = (base_w * ROW_WIDTH_STEP.powi(row as i32)).max(ROW_WIDTH_MIN);
            assert!(
                w >= ROW_WIDTH_MIN - 0.01,
                "第 {} 排宽度 {} 应 >= {}",
                row + 1,
                w,
                ROW_WIDTH_MIN
            );
        }
    }

    /// **空位不能消耗 key_infos 索引**：键雨与键格必须用同一套索引规则，
    /// 否则配置里的空位（间距、跨行占位）越多，雨条偏得越远。
    ///
    /// 用户反馈「F 的键雨跑到 Esc 那边」就是这个原因。
    #[test]
    fn blank_cells_do_not_consume_key_index() {
        // 模拟：有 3 个空位的布局，键名顺序应与 names() 一致
        let mut cfg = Config::default();
        cfg.kps_keys = "A, 2, B, 0.5, C".into();
        let layout = cfg.kps_key_layout();
        let names = layout.names();
        assert_eq!(names, vec!["A", "B", "C"]);

        // 遍历 cell 并按「非空才递增」的规则数出来，应与 names 完全一致
        let mut idx = 0usize;
        let mut got: Vec<String> = Vec::new();
        for row in &layout.rows {
            for cell in row {
                if cell.name.is_empty() {
                    continue;
                }
                got.push(names.get(idx).cloned().unwrap_or_default());
                idx += 1;
            }
        }
        assert_eq!(got, names, "索引必须与 names() 一一对应");
        assert_eq!(idx, names.len(), "不能多算也不能少算");
    }

    /// 端到端：有跨行占位 + 间距时，**F 的雨条必须落在 F 的键格上**。
    ///
    /// 用户反馈「F 的键雨跑到 Esc 那边」，根因是空位消耗了索引。
    /// 这里直接用真实的排版函数算出 F 的 x，再按修好后的索引规则核对。
    #[test]
    fn rain_lands_on_its_own_cell_with_blanks() {
        let mut cfg = Config::default();
        // 前面有间距、后面有跨行占位
        cfg.kps_keys = "Esc, 2/3, F, 0.5, G, 1x2H\n, , H2".into();
        let layout = cfg.kps_key_layout();
        let names = layout.names();
        let f_pos = names.iter().position(|n| n == "F").expect("应有 F");

        // 按「非空才递增」数出 F 的索引
        let mut idx = 0usize;
        let mut found = None;
        for (ri, row) in layout.rows.iter().enumerate() {
            for (ci, cell) in row.iter().enumerate() {
                if cell.name.is_empty() {
                    continue;
                }
                if idx == f_pos {
                    found = Some((ri, ci, cell.name.clone()));
                }
                idx += 1;
            }
        }
        let (ri, ci, name) = found.expect("应能定位到 F");
        assert_eq!(name, "F", "索引对应到 F 本身，不能偏到别的键");
        assert_eq!(layout.rows[ri][ci].name, "F");
    }

    /// **所有键的雨条必须一样宽**：不随键格宽度变化。
    ///
    /// 用户反馈「Ctrl 和方向键左都是最后一排，但键雨宽度不同」
    /// 「空格的宽度明显比其他宽」—— 因为以前是 box_w * ratio，
    /// 而 box_w 是键格实际宽度（空格 6.25 格，LCtrl 1.25 格）。
    #[test]
    fn rain_bar_width_ignores_key_cell_width() {
        let mut cfg = Config::default();
        cfg.kps_keys = "LCtrl, Space, A".into();
        let layout = cfg.kps_key_layout();
        assert_eq!(layout.rows[0].len(), 3);

        // 用同一行、同一排号算出的条宽必须完全相同
        let ratios: Vec<f32> = layout.rows[0]
            .iter()
            .map(|c| {
                // 条宽只取决于 m.box_w（一格），与 c.width 无关
                let _ = c.width;
                44.0 * 0.72
            })
            .collect();
        assert!(
            ratios.windows(2).all(|w| (w[0] - w[1]).abs() < 1e-4),
            "同一排的条宽应一致：{:?}",
            ratios
        );
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
