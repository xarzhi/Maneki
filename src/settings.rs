//! 设置抽屉：画面填充 / 画面比例 / 色彩调整 / 自动连播。
//!
//! 原来是一个居中的弹窗，现在是**贴在右边的抽屉**，和播放列表长同一副样子：
//! 同样宽度的竖条、同样的黑色半透明底、同样由 `main.rs` 用 `Float::translate`
//! 做滑入滑出。挤在中间弹窗里，窗口一小就得缩，反而看不清；靠边一条抽屉
//! 高度跟着窗口走，内容多了就地滚动。
//!
//! 字幕设置已经搬到下方控制栏的 popover（见 `subtitle.rs`）——它跟播放强相关，
//! 每集都可能要换轨，躲在两层目录里太深了。
//!
//! 画面类设置（填充、比例、色彩）**完全在我们的着色器里生效**，不碰 mpv：
//! mpv 的 brightness/contrast 只在 gpu 类 VO 上有效，而这里是 libmpv 软渲染出帧，
//! 走 mpv 那套在这条路径上不会生效，反而会让人怀疑是不是没刷新。

use iced::widget::{button, checkbox, column, container, row, scrollable, slider, space, text};
use iced::{
    Alignment, Background, Border, Color, Element, Fill, Length, Padding, Shadow, Vector,
    alignment::{Horizontal, Vertical},
};

use crate::icon;
use crate::video::{Aspect, FitMode, Look};
use crate::Message;

/// 抽屉宽度（逻辑像素）。比播放列表宽一点：这里有滑块和两排分段按钮。
pub const W: f32 = 344.0;
/// 内容区左右内边距
const PAD: f32 = 16.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Tab {
    #[default]
    Picture,
    Color,
}

impl Tab {
    fn label(self) -> &'static str {
        match self {
            Tab::Picture => "画面",
            Tab::Color => "色彩",
        }
    }

    const ALL: [Tab; 2] = [Tab::Picture, Tab::Color];
}

/// 色彩调整里能被点的那几项
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Field {
    Brightness,
    Contrast,
    Saturation,
    Hue,
    Sharpen,
}

#[derive(Debug, Clone, Default)]
pub struct Settings {
    pub open: bool,
    pub tab: Tab,
    pub fit: FitMode,
    pub aspect: Aspect,
    pub look: Look,
    /// 自动连播。**默认关**：一个视频放完就停在那里，
    /// 不要替用户决定「接着往下看」。
    pub autoplay: bool,
}

impl Settings {
    /// 关掉抽屉（其余设置保留，下次打开还在）
    pub fn close(&mut self) {
        self.open = false;
    }

    pub fn set_look(&mut self, field: Field, value: f32) {
        let value = value.round();
        match field {
            Field::Brightness => self.look.brightness = value,
            Field::Contrast => self.look.contrast = value,
            Field::Saturation => self.look.saturation = value,
            Field::Hue => self.look.hue = value,
            Field::Sharpen => self.look.sharpen = value.max(0.0),
        }
    }
}

// ---------------- 视图 ----------------

/// 抽屉面板本体（宽 `W`、占满可用高度）。
///
/// 摆放位置和滑入滑出动画都在 `main.rs`（`Float::translate`），
/// 这样播放列表和设置能共用同一套抽屉动作。
pub fn drawer<'a>(s: &'a Settings) -> Element<'a, Message> {
    let header = container(
        row![
            text("播放设置")
                .font(icon::UI_FONT)
                .size(14)
                .color(icon::TEXT),
            container(space()).width(Fill),
            icon_only(icon::CLOSE, 11.0, Message::CloseSettings),
        ]
        .align_y(Alignment::Center),
    )
    .width(Fill)
    .height(Length::Fixed(44.0))
    .padding(Padding {
        top: 0.0,
        right: 8.0,
        bottom: 0.0,
        left: PAD,
    })
    .align_y(Vertical::Center);

    let tabs = container(
        row(Tab::ALL.iter().map(|t| tab_button(*t, s.tab == *t)))
            .spacing(6)
            .width(Fill),
    )
    .width(Fill)
    .padding(Padding {
        top: 10.0,
        right: PAD,
        bottom: 10.0,
        left: PAD,
    });

    let content: Element<'a, Message> = match s.tab {
        Tab::Picture => picture_tab(s),
        Tab::Color => color_tab(s),
    };

    let body = container(content)
        .width(Fill)
        .padding(Padding {
            top: 12.0,
            right: PAD,
            bottom: 16.0,
            left: PAD,
        });

    container(
        column![
            header,
            rule(),
            tabs,
            rule(),
            scrollable(body).height(Fill).width(Fill),
        ]
        .width(Fill)
        .height(Fill),
    )
    .width(Length::Fixed(W))
    .height(Fill)
    .style(|_| container::Style {
        // 和播放列表同一个黑底（icon::PANEL_BG）：半透明，不是纯黑
        background: Some(Background::Color(icon::PANEL_BG)),
        border: Border {
            radius: iced::border::Radius {
                top_left: 12.0,
                top_right: 0.0,
                bottom_right: 0.0,
                bottom_left: 12.0,
            },
            width: 0.0,
            color: Color::TRANSPARENT,
        },
        // 往左的投影要收着点：之前 -8 / blur 26，左边缘外会拖出 30 多像素的暗带，
        // 看着像面板旁边糊了一块灰。现在 -3 / 18，只在紧贴边缘处留一圈过渡。
        shadow: Shadow {
            color: Color::from_rgba(0.0, 0.0, 0.0, 0.40),
            offset: Vector::new(-3.0, 0.0),
            blur_radius: 18.0,
        },
        ..container::Style::default()
    })
    .into()
}

fn rule<'a>() -> Element<'a, Message> {
    container(space())
        .width(Fill)
        .height(Length::Fixed(1.0))
        .style(|_| container::Style {
            background: Some(Background::Color(icon::PANEL_LINE)),
            ..container::Style::default()
        })
        .into()
}

// ---------------- 画面 ----------------

fn picture_tab(s: &Settings) -> Element<'_, Message> {
    let fill = row(FitMode::ALL.iter().map(|m| segment(
        m.label(),
        s.fit == *m,
        Message::SetFit(*m),
    )))
    .spacing(6);

    // 五个比例按钮一行塞不下 344px 的抽屉，拆成 3 + 2 两排；
    // 第二排补一格空白，两排的格子才对得上。
    let aspect_a = row(
        Aspect::ALL[..3]
            .iter()
            .map(|a| segment(a.label(), s.aspect == *a, Message::SetAspect(*a))),
    )
    .spacing(6);
    let mut aspect_b = row(
        Aspect::ALL[3..]
            .iter()
            .map(|a| segment(a.label(), s.aspect == *a, Message::SetAspect(*a))),
    )
    .spacing(6);
    aspect_b = aspect_b.push(container(space()).width(Fill));

    let col = column![
        section("画面填充"),
        fill,
        gap(16.0),
        section("画面比例"),
        aspect_a,
        aspect_b,
        gap(16.0),
        section("播放"),
        autoplay_row(s.autoplay),
    ]
    .spacing(8)
    .width(Fill);

    container(col).width(Fill).into()
}

/// 「自动连播」勾选项
fn autoplay_row(on: bool) -> Element<'static, Message> {
    container(
        checkbox(on)
            .label("自动连播")
            .text_size(13.5)
            .font(icon::UI_FONT)
            .size(18)
            .spacing(9)
            .on_toggle(Message::SetAutoplay)
            .style(checkbox_style),
    )
    .width(Fill)
    .padding(Padding {
        top: 2.0,
        right: 0.0,
        bottom: 2.0,
        left: 0.0,
    })
    .into()
}

fn checkbox_style(_t: &iced::Theme, status: checkbox::Status) -> checkbox::Style {
    let (checked, hovered) = match status {
        checkbox::Status::Active { is_checked } => (is_checked, false),
        checkbox::Status::Hovered { is_checked } => (is_checked, true),
        _ => (false, false),
    };

    let bg = if checked {
        Background::Color(icon::ACCENT)
    } else if hovered {
        Background::Color(Color::from_rgba(1.0, 1.0, 1.0, 0.16))
    } else {
        // 未勾选时也要看得出「这里有个框」：0.06 太淡，压在半透明黑底上几乎消失
        Background::Color(Color::from_rgba(1.0, 1.0, 1.0, 0.10))
    };

    checkbox::Style {
        background: bg,
        // 蓝底上压深色勾，对比够；没勾时这个颜色用不到
        icon_color: Color::from_rgb(0.05, 0.06, 0.08),
        border: Border {
            radius: 5.0.into(),
            width: 1.0,
            color: if checked {
                Color::TRANSPARENT
            } else {
                Color::from_rgba(1.0, 1.0, 1.0, 0.30)
            },
        },
        text_color: Some(icon::TEXT_DIM),
    }
}

// ---------------- 色彩 ----------------

fn color_tab(s: &Settings) -> Element<'_, Message> {
    let l = s.look;

    let rows = column![
        look_row("亮度", l.brightness, -100.0, 100.0, Field::Brightness),
        look_row("对比度", l.contrast, -100.0, 100.0, Field::Contrast),
        look_row("饱和度", l.saturation, -100.0, 100.0, Field::Saturation),
        look_row("色调", l.hue, -100.0, 100.0, Field::Hue),
        look_row("锐度", l.sharpen, 0.0, 100.0, Field::Sharpen),
    ]
    .spacing(8)
    .width(Fill);

    // 只剩「全部重置」一颗按钮，靠右；说明文字按用户要求全部去掉
    let footer = row![
        container(space()).width(Fill),
        text_button("全部重置", Message::ResetLook, !l.is_neutral()),
    ]
    .align_y(Alignment::Center)
    .padding(Padding {
        top: 16.0,
        right: 0.0,
        bottom: 0.0,
        left: 0.0,
    });

    container(column![rows, footer].spacing(6).width(Fill))
        .width(Fill)
        .into()
}

fn look_row(
    label: &'static str,
    value: f32,
    min: f32,
    max: f32,
    field: Field,
) -> Element<'static, Message> {
    let shown = if (value - value.round()).abs() < 0.01 {
        format!("{:.0}", value)
    } else {
        format!("{value:.1}")
    };

    row![
        text(label)
            .font(icon::UI_FONT)
            .size(13.5)
            .color(icon::TEXT_DIM)
            .width(Length::Fixed(52.0)),
        slider(min..=max, value, move |v| Message::SetLook(field, v))
            .step(1.0_f32)
            .width(Fill)
            .style(panel_slider),
        container(
            text(shown)
                .font(icon::UI_FONT)
                .size(13)
                .color(if value == 0.0 {
                    icon::TEXT_FAINT
                } else {
                    icon::TEXT
                })
        )
        .width(Length::Fixed(38.0))
        .align_x(Horizontal::Right),
    ]
    .spacing(10)
    .align_y(Alignment::Center)
    .height(Length::Fixed(32.0))
    .into()
}

// ---------------- 小部件 ----------------

fn tab_button(tab: Tab, active: bool) -> Element<'static, Message> {
    button(
        container(
            text(tab.label())
                .font(icon::UI_FONT)
                .size(13.5)
                .color(if active { icon::TEXT } else { icon::TEXT_DIM }),
        )
        .center_x(Fill),
    )
    .width(Fill)
    .padding(Padding {
        top: 7.0,
        right: 10.0,
        bottom: 7.0,
        left: 10.0,
    })
    .on_press(Message::SettingsTab(tab))
    .style(move |_t, status| {
        use button::Status;
        let bg = match status {
            _ if active => Some(Background::Color(Color::from_rgba(1.0, 1.0, 1.0, 0.12))),
            Status::Hovered => Some(Background::Color(Color::from_rgba(1.0, 1.0, 1.0, 0.06))),
            _ => None,
        };
        button::Style {
            background: bg,
            text_color: if active { icon::TEXT } else { icon::TEXT_DIM },
            border: Border {
                radius: 7.0.into(),
                ..Border::default()
            },
            ..button::Style::default()
        }
    })
    .into()
}

fn segment<'a>(label: &'a str, active: bool, msg: Message) -> Element<'a, Message> {
    button(text(label).font(icon::UI_FONT).size(13.5).color(if active {
        Color::from_rgb(0.05, 0.06, 0.08)
    } else {
        icon::TEXT_DIM
    }))
    .width(Fill)
    .padding(Padding {
        top: 7.0,
        right: 4.0,
        bottom: 7.0,
        left: 4.0,
    })
    .on_press(msg)
    .style(move |_t, status| {
        use button::Status;
        let bg = match status {
            _ if active => Some(Background::Color(icon::ACCENT)),
            Status::Hovered => Some(Background::Color(Color::from_rgba(1.0, 1.0, 1.0, 0.14))),
            _ => Some(Background::Color(Color::from_rgba(1.0, 1.0, 1.0, 0.05))),
        };
        button::Style {
            background: bg,
            text_color: icon::TEXT,
            border: Border {
                radius: 7.0.into(),
                width: if active { 0.0 } else { 1.0 },
                color: Color::from_rgba(1.0, 1.0, 1.0, 0.10),
            },
            ..button::Style::default()
        }
    })
    .into()
}

fn text_button<'a>(label: &'a str, msg: Message, enabled: bool) -> Element<'a, Message> {
    let b = button(
        text(label)
            .font(icon::UI_FONT)
            .size(13)
            .color(if enabled {
                icon::TEXT_DIM
            } else {
                Color::from_rgba(1.0, 1.0, 1.0, 0.28)
            }),
    )
    .padding(Padding {
        top: 6.0,
        right: 11.0,
        bottom: 6.0,
        left: 11.0,
    })
    .style(|_t, status| {
        use button::Status;
        let bg = match status {
            Status::Hovered => Some(Background::Color(Color::from_rgba(1.0, 1.0, 1.0, 0.10))),
            Status::Pressed => Some(Background::Color(Color::from_rgba(1.0, 1.0, 1.0, 0.16))),
            _ => Some(Background::Color(Color::from_rgba(1.0, 1.0, 1.0, 0.05))),
        };
        button::Style {
            background: bg,
            text_color: icon::TEXT,
            border: Border {
                radius: 7.0.into(),
                width: 1.0,
                color: Color::from_rgba(1.0, 1.0, 1.0, 0.10),
            },
            ..button::Style::default()
        }
    });

    if enabled { b.on_press(msg).into() } else { b.into() }
}

fn icon_only<'a>(glyph: char, size: f32, msg: Message) -> Element<'a, Message> {
    button(
        container(text(glyph).font(icon::ICON_FONT).size(size).color(icon::TEXT_FAINT))
            .center(Length::Fixed(26.0)),
    )
    .padding(0)
    .on_press(msg)
    .style(|_t, status| {
        use button::Status;
        let bg = match status {
            Status::Hovered => Some(Background::Color(icon::HOVER_BG)),
            Status::Pressed => Some(Background::Color(Color::from_rgba(1.0, 1.0, 1.0, 0.2))),
            _ => None,
        };
        button::Style {
            background: bg,
            text_color: icon::TEXT,
            border: Border {
                radius: 6.0.into(),
                ..Border::default()
            },
            ..button::Style::default()
        }
    })
    .into()
}

fn section(title: &str) -> Element<'_, Message> {
    text(title)
        .font(icon::UI_FONT)
        .size(14)
        .color(icon::TEXT)
        .into()
}

fn gap(h: f32) -> Element<'static, Message> {
    container(space()).height(Length::Fixed(h)).into()
}

/// 面板里的横向滑块：轨道细、手柄常驻
fn panel_slider(_t: &iced::Theme, status: slider::Status) -> slider::Style {
    let filled = match status {
        slider::Status::Dragged => Color::from_rgb(0.55, 0.83, 1.0),
        slider::Status::Hovered => Color::from_rgb(0.48, 0.78, 1.0),
        _ => icon::ACCENT,
    };

    slider::Style {
        rail: slider::Rail {
            backgrounds: (
                Background::Color(filled),
                Background::Color(Color::from_rgba(1.0, 1.0, 1.0, 0.14)),
            ),
            width: 4.0,
            border: Border {
                radius: 2.0.into(),
                ..Border::default()
            },
        },
        handle: slider::Handle {
            shape: slider::HandleShape::Circle { radius: 6.0 },
            background: Background::Color(Color::WHITE),
            border_width: 0.0,
            border_color: Color::TRANSPARENT,
        },
    }
}
