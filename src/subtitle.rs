//! 字幕设置：挂在底栏「字幕」键上的一块 popover。
//!
//! 以前它藏在设置弹窗的第三个标签页里 —— 换个字幕要「点设置 → 找标签 → 挑轨道」，
//! 而换字幕是**每集都会做**的动作，放那么深不合适。现在它是底栏音量键右边的一个
//! 图标，点一下就在原位展开。
//!
//! 数据全部来自 `mpv` 的 `track-list`（`main.rs` 每帧同步进 `App::tracks`），
//! 这里只负责画，动作通过 `Message` 发出去。

use iced::widget::{button, column, container, row, scrollable, space, text};
use iced::widget::text::Wrapping;
use iced::{
    Alignment, Background, Border, Color, Element, Fill, Length, Padding, Shadow, Vector,
    alignment::Vertical,
};

use crate::icon;
use crate::mpv::Track;
use crate::ui::elide;
use crate::Message;

/// popover 宽度
pub const W: f32 = 300.0;
/// 每条轨道行的高度
const ROW_H: f32 = 32.0;
/// 轨道列表最多长多高（超过就滚动）
const LIST_MAX: f32 = 208.0;

pub fn popover<'a>(
    tracks: &'a [Track],
    sid: Option<i64>,
    hint: Option<&'a str>,
) -> Element<'a, Message> {
    let subs: Vec<&Track> = tracks.iter().filter(|t| t.kind == "sub").collect();
    let embedded = subs.iter().filter(|t| !t.external).count();
    let external = subs.len() - embedded;

    let status = if tracks.is_empty() {
        String::from("正在读取这个文件的字幕信息…")
    } else if embedded == 0 && external == 0 {
        String::from("没有内嵌字幕，可以载入本地字幕文件")
    } else {
        let mut s = format!("内嵌 {embedded} 条");
        if external > 0 {
            s.push_str(&format!("，外挂 {external} 条"));
        }
        s
    };

    // 「不使用字幕」永远排第一，方便一键关掉
    let mut list = column![].spacing(2).width(Fill);
    list = list.push(sub_row(String::from("不使用字幕"), None, sid.is_none()));
    for t in &subs {
        let badge = if t.external { "外挂" } else { "内嵌" };
        list = list.push(sub_row(
            format!("{}  ·  {badge}", t.label()),
            Some(t.id),
            sid == Some(t.id),
        ));
    }

    // 列表高度按条数算，但要有上限：几十条轨道不能把 popover 顶穿屏幕
    let rows = subs.len() + 1;
    let list_h = (rows as f32 * (ROW_H + 2.0) + 2.0).min(LIST_MAX);

    let header = row![
        text("字幕")
            .font(icon::UI_FONT)
            .size(13.5)
            .color(icon::TEXT),
        container(space()).width(Fill),
        icon_only(icon::CLOSE, 10.0, Message::ToggleSubtitlePopover),
    ]
    .align_y(Alignment::Center);

    let actions = row![
        text_button("载入本地字幕…", Message::PickSubtitle, true),
        text_button("重新扫描", Message::RefreshTracks, true),
    ]
    .spacing(6)
    .align_y(Alignment::Center);

    let hint_line: Element<'a, Message> = match hint {
        Some(h) => text(h)
            .font(icon::UI_FONT)
            .size(12)
            .color(icon::ACCENT)
            .wrapping(Wrapping::Word)
            .into(),
        None => text("支持 srt / ass / ssa / vtt / sub 等格式，外挂不会改动原文件。")
            .font(icon::UI_FONT)
            .size(12)
            .line_height(1.4)
            .color(icon::TEXT_FAINT)
            .wrapping(Wrapping::Word)
            .into(),
    };

    let body = column![
        header,
        text(status)
            .font(icon::UI_FONT)
            .size(12)
            .color(icon::TEXT_DIM)
            .wrapping(Wrapping::Word),
        container(scrollable(list).width(Fill))
            .height(Length::Fixed(list_h))
            .width(Fill),
        rule(),
        actions,
        hint_line,
    ]
    .spacing(8)
    .width(Fill);

    container(body)
        .width(Length::Fixed(W))
        .padding(Padding {
            top: 10.0,
            right: 10.0,
            bottom: 10.0,
            left: 12.0,
        })
        .style(|_| container::Style {
            // 和抽屉同一套：黑色半透明，不是纯黑
            background: Some(Background::Color(icon::PANEL_BG)),
            border: Border {
                radius: 12.0.into(),
                width: 1.0,
                color: icon::PANEL_LINE,
            },
            shadow: Shadow {
                color: Color::from_rgba(0.0, 0.0, 0.0, 0.45),
                offset: Vector::new(0.0, 6.0),
                blur_radius: 20.0,
            },
            ..container::Style::default()
        })
        .into()
}

fn sub_row(label: String, id: Option<i64>, selected: bool) -> Element<'static, Message> {
    let dot = container(space())
        .width(Length::Fixed(7.0))
        .height(Length::Fixed(7.0))
        .style(move |_| container::Style {
            background: Some(Background::Color(if selected {
                icon::ACCENT
            } else {
                Color::from_rgba(1.0, 1.0, 1.0, 0.18)
            })),
            border: Border {
                radius: 4.0.into(),
                ..Border::default()
            },
            ..container::Style::default()
        });

    // 轨道名可能很长（「简体中文 [ASS] · 内嵌」），按像素预算截一下
    let label = elide(&label, W - 40.0 - 7.0 - 9.0, 12.5);

    button(
        container(
            row![
                dot,
                text(label)
                    .font(icon::UI_FONT)
                    .size(13.5)
                    .color(if selected { icon::TEXT } else { icon::TEXT_DIM })
                    .wrapping(Wrapping::None),
            ]
            .spacing(9)
            .align_y(Alignment::Center),
        )
        .height(Length::Fixed(ROW_H))
        .padding(Padding {
            top: 0.0,
            right: 10.0,
            bottom: 0.0,
            left: 10.0,
        })
        .align_y(Vertical::Center)
        .width(Fill),
    )
    .width(Fill)
    .padding(0)
    .on_press(Message::SelectSub(id))
    .style(move |_t, status| {
        use button::Status;
        let bg = match status {
            _ if selected => Some(Background::Color(Color::from_rgba(0.36, 0.72, 0.98, 0.16))),
            Status::Hovered => Some(Background::Color(Color::from_rgba(1.0, 1.0, 1.0, 0.08))),
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

fn icon_only<'a>(glyph: char, size: f32, msg: Message) -> Element<'a, Message> {
    button(
        container(
            text(glyph)
                .font(icon::ICON_FONT)
                .size(size)
                .color(icon::TEXT_FAINT),
        )
        .center(Length::Fixed(22.0)),
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
                radius: 5.0.into(),
                ..Border::default()
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
            .size(12)
            .color(if enabled {
                icon::TEXT_DIM
            } else {
                Color::from_rgba(1.0, 1.0, 1.0, 0.28)
            }),
    )
    .padding(Padding {
        top: 6.0,
        right: 10.0,
        bottom: 6.0,
        left: 10.0,
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
