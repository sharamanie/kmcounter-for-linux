//! 自绘趋势图表：双 Y 轴（左右各一条曲线）、单调三次平滑、面积填充、图例、悬停定位。
//! x 轴为等距时间桶（日/周/月/年），y 轴两个指标各自独立缩放。

use eframe::egui::{self, Align2, Color32, FontId, Pos2, Rect, Shape, Stroke, Vec2};

pub struct Series {
    pub name: String,
    pub color: Color32,
    pub values: Vec<f64>,
}

pub struct ChartData {
    /// x 轴每个桶的短标签
    pub labels: Vec<String>,
    /// 左轴一条 + 右轴一条
    pub series: Vec<Series>,
}

#[derive(Clone, Copy)]
pub struct ChartHover {
    pub index: usize,
    pub pos: Pos2,
}

const PAD_TOP: f32 = 26.0;
const PAD_BOTTOM: f32 = 20.0;
const PAD_SIDE: f32 = 54.0;

/// y 轴数值格式：大数缩写（12.3k）
pub fn fmt_y(v: f64) -> String {
    if v >= 10_000.0 {
        format!("{:.0}k", v / 1000.0)
    } else if v >= 1000.0 {
        format!("{:.1}k", v / 1000.0)
    } else if v >= 10.0 {
        format!("{:.0}", v)
    } else {
        format!("{:.1}", v)
    }
}

/// 绘图区（去掉四周留白）——绘制与点击命中共用同一份几何
fn plot_rect(rect: Rect) -> Rect {
    Rect::from_min_max(
        Pos2::new(rect.left() + PAD_SIDE, rect.top() + PAD_TOP),
        Pos2::new(rect.right() - PAD_SIDE, rect.bottom() - PAD_BOTTOM),
    )
}

/// 第 `i` 个桶的中心 x（共 `n` 个桶）：点击图表跳转日期时按它换算
pub fn bucket_center_x(rect: Rect, n: usize, i: usize) -> f32 {
    let plot = plot_rect(rect);
    plot.left() + (i as f32 + 0.5) * (plot.width() / n.max(1) as f32)
}

/// 绘制图表，返回悬停命中的桶下标。
/// `rect`/`resp` 由调用方通过 `allocate_exact_size` 提供；要求恰好 2 条曲线（左轴、右轴）。
pub fn draw(ui: &mut egui::Ui, rect: Rect, resp: &egui::Response, data: &ChartData) -> Option<ChartHover> {
    let painter = ui.painter();
    let n = data.labels.len();
    let plot = plot_rect(rect);

    // 背景与边框
    painter.rect_filled(rect, 4.0, Color32::from_rgb(0xF7, 0xF7, 0xF7));
    painter.rect_stroke(rect, 4.0, Stroke::new(1.0_f32, Color32::from_rgb(0xDD, 0xDD, 0xDD)));
    if n == 0 {
        return None;
    }

    let small = FontId::proportional(10.0);
    let dim = Color32::from_rgb(0x99, 0x99, 0x99);

    // 每条曲线的最大值（用于各自 y 轴缩放）
    let maxes: Vec<f64> = data
        .series
        .iter()
        .map(|s| s.values.iter().cloned().fold(1.0f64, f64::max))
        .collect();

    // x 位置：每个桶占一等分，点在桶中心（与点击命中用同一个换算）
    let band = plot.width() / n as f32;
    let x_at = |i: usize| bucket_center_x(rect, n, i);
    let y_at = |v: f64, max: f64| plot.bottom() - ((v / max) * plot.height() as f64) as f32;

    // 横向网格线 + 双侧刻度
    let grid = Stroke::new(1.0_f32, Color32::from_rgb(0xE4, 0xE4, 0xE4));
    for k in 0..=4 {
        let frac = k as f64 / 4.0;
        let y = plot.bottom() - (frac * plot.height() as f64) as f32;
        painter.line_segment([Pos2::new(plot.left(), y), Pos2::new(plot.right(), y)], grid);
        let v0 = maxes[0] * frac;
        let v1 = maxes[1] * frac;
        painter.text(
            Pos2::new(plot.left() - 6.0, y),
            Align2::RIGHT_CENTER,
            fmt_y(v0),
            small.clone(),
            data.series[0].color.gamma_multiply(0.75),
        );
        painter.text(
            Pos2::new(plot.right() + 6.0, y),
            Align2::LEFT_CENTER,
            fmt_y(v1),
            small.clone(),
            data.series[1].color.gamma_multiply(0.75),
        );
    }

    // x 轴刻度标签（避免重叠，按需抽稀）
    let label_every = ((n as f32) / (plot.width() / 64.0)).ceil().max(1.0) as usize;
    for i in (0..n).step_by(label_every) {
        painter.text(
            Pos2::new(x_at(i), plot.bottom() + 12.0),
            Align2::CENTER_CENTER,
            data.labels[i].clone(),
            small.clone(),
            dim,
        );
    }

    let samples = 16;
    for (si, s) in data.series.iter().enumerate() {
        if s.values.is_empty() {
            continue;
        }
        let ys = crate::trend::smooth_series(&s.values, samples);
        // n≥3 时 ys 是插值后的密集序列（每 samples 个点跨一个桶）；n<3 时 ys 就是原始点
        let denom = if s.values.len() >= 3 { samples as f64 } else { 1.0 };
        let pts: Vec<Pos2> = ys
            .iter()
            .enumerate()
            .map(|(j, v)| {
                let t = j as f64 / denom;
                let x = plot.left() + ((t + 0.5) * band as f64) as f32;
                Pos2::new(x, y_at(*v, maxes[si]))
            })
            .collect();

        // 面积填充：逐段四边形（凸、共享边），避免非凸路径扇形三角化导致的半透明重叠暗块
        if pts.len() >= 2 {
            let fill = s.color.gamma_multiply(0.16);
            for w in pts.windows(2) {
                let (a, b) = (w[0], w[1]);
                painter.add(Shape::convex_polygon(
                    vec![a, b, Pos2::new(b.x, plot.bottom()), Pos2::new(a.x, plot.bottom())],
                    fill,
                    Stroke::NONE,
                ));
            }
            painter.add(Shape::line(pts.clone(), Stroke::new(2.0_f32, s.color)));
        }

        // 原始数据点（小圆点）
        for (i, v) in s.values.iter().enumerate() {
            let c = Pos2::new(x_at(i), y_at(*v, maxes[si]));
            painter.circle_filled(c, 2.5, s.color);
        }
    }

    // 图例（右上）：方块在文字左侧，依次向左排列（注意方块与文字不能重叠）
    let font = FontId::proportional(11.0);
    let sw = 9.0; // 方块边长
    let gap = 4.0; // 方块与文字间距
    let item_gap = 14.0; // 图例项间距
    let mut right = plot.right() - 8.0;
    for s in data.series.iter().rev() {
        let galley = painter.layout_no_wrap(s.name.clone(), font.clone(), s.color);
        let text_w = galley.size().x;
        let total = sw + gap + text_w;
        let x0 = right - total;
        painter.rect_filled(
            Rect::from_min_size(Pos2::new(x0, rect.top() + 10.0 - sw / 2.0), Vec2::new(sw, sw)),
            2.0,
            s.color,
        );
        painter.text(
            Pos2::new(x0 + sw + gap, rect.top() + 10.0),
            Align2::LEFT_CENTER,
            s.name.clone(),
            font.clone(),
            s.color,
        );
        right -= total + item_gap;
    }

    // 悬停：最近桶 + 引导线与圆点
    if resp.hovered() {
        if let Some(pos) = ui.input(|i| i.pointer.hover_pos()) {
            if plot.contains(pos) {
                let idx = (((pos.x - plot.left()) / band).floor() as usize).min(n - 1);
                let xg = x_at(idx);
                painter.line_segment(
                    [Pos2::new(xg, plot.top()), Pos2::new(xg, plot.bottom())],
                    Stroke::new(1.0_f32, Color32::from_rgb(0xAA, 0xAA, 0xAA)),
                );
                for (si, s) in data.series.iter().enumerate() {
                    let v = s.values.get(idx).copied().unwrap_or(0.0);
                    let c = Pos2::new(xg, y_at(v, maxes[si]));
                    painter.circle_filled(c, 4.0, s.color);
                    painter.circle_stroke(c, 4.0, Stroke::new(1.5_f32, Color32::WHITE));
                }
                return Some(ChartHover { index: idx, pos });
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use eframe::egui::Sense;

    fn run_frame(ctx: &egui::Context, pointer: Option<Pos2>) {
        let mut raw = egui::RawInput::default();
        raw.screen_rect = Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1200.0, 420.0)));
        if let Some(p) = pointer {
            raw.events.push(egui::Event::PointerMoved(p));
        }
        let _ = ctx.run(raw, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let (rect, resp) = ui.allocate_exact_size(Vec2::new(1100.0, 360.0), Sense::hover());
                let data = ChartData {
                    labels: (0..30).map(|i| format!("d{i}")).collect(),
                    series: vec![
                        Series {
                            name: "键盘".into(),
                            color: Color32::BLUE,
                            values: (0..30).map(|i| ((i * 37) % 100) as f64).collect(),
                        },
                        Series {
                            name: "鼠标".into(),
                            color: Color32::RED,
                            values: (0..30).map(|i| ((i * 11) % 50) as f64).collect(),
                        },
                    ],
                };
                let hover = draw(ui, rect, &resp, &data);
                if let Some(h) = hover {
                    assert!(h.index < 30);
                }
            });
        });
    }

    #[test]
    fn chart_draw_smoke() {
        let ctx = egui::Context::default();
        run_frame(&ctx, None);
        // 悬停在图表中部
        run_frame(&ctx, Some(Pos2::new(300.0, 200.0)));
    }

    #[test]
    fn chart_draw_smoke_small_n() {
        // 少于 3 个数据点的边界情况
        let ctx = egui::Context::default();
        let mut raw = egui::RawInput::default();
        raw.screen_rect = Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1200.0, 420.0)));
        let _ = ctx.run(raw, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let (rect, resp) = ui.allocate_exact_size(Vec2::new(1100.0, 360.0), Sense::hover());
                for n in [1usize, 2] {
                    let data = ChartData {
                        labels: (0..n).map(|i| format!("d{i}")).collect(),
                        series: vec![
                            Series { name: "a".into(), color: Color32::BLUE, values: vec![3.0; n] },
                            Series { name: "b".into(), color: Color32::RED, values: vec![7.0; n] },
                        ],
                    };
                    let _ = draw(ui, rect, &resp, &data);
                }
            });
        });
    }

    #[test]
    fn fmt_y_readables() {
        assert_eq!(fmt_y(0.0), "0.0");
        assert_eq!(fmt_y(5.2), "5.2");
        assert_eq!(fmt_y(1234.0), "1.2k");
        assert_eq!(fmt_y(12345.0), "12k");
    }
}
