//! 换片时画面中央那个转圈指示器。
//!
//! 换集（上一集 / 下一集 / 连播 / 点列表某一行）时 `play()` 会先把 `frame` 清掉，
//! 到 mpv 解码出首帧之间有一小段空白 —— 大文件或者机械硬盘上能明显看到黑一下。
//! 这段时间就画这个圈，告诉用户「在开，不是卡死了」。
//!
//! 用 `canvas` 画，不画图片：弧长、粗细、颜色都能跟着主题走，也不用为几种 DPI
//! 各备一张图。彗尾是把 109° 的圆弧切成 26 小段、透明度从 0.04 递进到 1 画出来的
//! —— 比单色整圈好看，成本也就是 26 条线段。

use iced::widget::canvas::{self, Frame, LineCap, LineJoin, Path, Stroke};
use iced::{Color, Element, Length, Point, Rectangle, Renderer, Theme, mouse};

use crate::icon;

/// 转一圈要多久（秒）。太快像抽搐，太慢像卡住。
pub const TURN: f32 = 0.9;
/// 指示器直径（逻辑像素）
const SIZE: f32 = 34.0;
/// 彗尾扫过的角度（弧度）
const SWEEP: f32 = 1.9;
/// 彗尾切成多少段
const SEGMENTS: usize = 26;

/// 画一个转圈指示器。
///
/// `progress` 是 0..1 的相位，每帧往前走一点就会转起来；`opacity` 是整体透明度，
/// 用来做「刚切过来时淡入」，免得短暂的加载空档里硬闪一下。
pub fn view<'a>(progress: f32, opacity: f32) -> Element<'a, crate::Message> {
    canvas::Canvas::new(Spinner { progress, opacity })
        .width(Length::Fixed(SIZE))
        .height(Length::Fixed(SIZE))
        .into()
}

struct Spinner {
    progress: f32,
    opacity: f32,
}

impl canvas::Program<crate::Message> for Spinner {
    type State = ();

    fn draw(
        &self,
        _state: &(),
        renderer: &Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<canvas::Geometry> {
        let mut frame = Frame::new(renderer, bounds.size());
        let center = frame.center();
        // 留 1px 给线宽，不然圆环会被画布边缘切掉
        let radius = (bounds.width.min(bounds.height) / 2.0 - 2.0).max(1.0);

        // 底圈：一整圈很淡的白，让「转到哪儿」有参照
        frame.stroke(
            &Path::circle(center, radius),
            Stroke {
                width: 2.0,
                style: canvas::Style::Solid(Color::from_rgba(
                    1.0,
                    1.0,
                    1.0,
                    0.10 * self.opacity,
                )),
                line_cap: LineCap::Round,
                line_join: LineJoin::Round,
                ..Stroke::default()
            },
        );

        // 彗尾：越靠近头部越亮、越粗一点点
        let start = self.progress * std::f32::consts::TAU;
        let step = SWEEP / SEGMENTS as f32;

        for i in 0..SEGMENTS {
            let t0 = start + step * i as f32;
            let tail = (i + 1) as f32 / SEGMENTS as f32;
            // 1.7 次方让尾巴衰减得更快，头部一段显得更实
            let alpha = 0.04 + 0.96 * tail.powf(1.7);

            frame.stroke(
                &Path::line(
                    on_circle(center, radius, t0),
                    on_circle(center, radius, t0 + step),
                ),
                Stroke {
                    width: 2.0 + 0.7 * tail,
                    style: canvas::Style::Solid(Color {
                        a: alpha * self.opacity,
                        ..icon::ACCENT
                    }),
                    line_cap: LineCap::Round,
                    line_join: LineJoin::Round,
                    ..Stroke::default()
                },
            );
        }

        vec![frame.into_geometry()]
    }
}

/// 圆上角度 `angle`（弧度，0 = 正右，顺时针为正）处的点。
fn on_circle(center: Point, radius: f32, angle: f32) -> Point {
    Point::new(
        center.x + radius * angle.cos(),
        center.y + radius * angle.sin(),
    )
}
