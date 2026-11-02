//! 文件详情抽屉：右键菜单里的「查看视频文件详情」打开的就是这一条。
//!
//! 和设置抽屉长同一副样子（同样的黑底、同样的滑入滑出），数据全部来自 mpv 的
//! 属性表（`file-format` / `video-codec` / `container-fps` …），我们自己不解析容器。

use iced::widget::text::Wrapping;
use iced::widget::{button, column, container, row, scrollable, space, text};
use iced::{
    Alignment, Background, Border, Color, Element, Fill, Length, Padding, Shadow, Vector,
    alignment::Horizontal,
};

use crate::icon;
use crate::mpv::{MediaInfo, Track};
use crate::Message;

/// 抽屉宽度（比设置宽一点：这里是一堆「标签 + 值」）
pub const W: f32 = 392.0;
/// 左右内边距
const PAD: f32 = 16.0;
/// 标签列宽
const LABEL_W: f32 = 68.0;

pub fn drawer<'a>(
    info: Option<&'a MediaInfo>,
    tracks: &'a [Track],
    title: &'a str,
) -> Element<'a, Message> {
    let header = container(
        row![
            text("文件详情")
                .font(icon::UI_FONT)
                .size(14)
                .color(icon::TEXT),
            container(space()).width(Fill),
            icon_only(icon::CLOSE, 11.0, Message::CloseInfo),
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
    .align_y(iced::alignment::Vertical::Center);

    let subs = tracks.iter().filter(|t| t.kind == "sub").count();
    let audios = tracks.iter().filter(|t| t.kind == "audio").count();

    let body: Element<'a, Message> = match info {
        None => container(
            text("正在读取文件信息…")
                .font(icon::UI_FONT)
                .size(13)
                .color(icon::TEXT_FAINT),
        )
        .width(Fill)
        .padding(Padding {
            top: 8.0,
            right: PAD,
            bottom: 0.0,
            left: PAD,
        })
        .into(),
        Some(i) => {
            let mut file = column![
                section("文件"),
                kv("名称", title.to_string()),
                kv("大小", fmt_size(i.size)),
                kv("容器", fmt_format(i.format.as_deref())),
                kv("时长", fmt_dur(i.duration)),
            ]
            .spacing(4)
            .width(Fill);
            // 路径单独一行、自己换行：太长了，塞在「标签 + 值」里会顶出去
            file = file.push(kv_wrap("位置", i.path.clone()));

            let video = match i.width {
                Some(w) => column![
                    section("视频"),
                    kv("分辨率", format!("{w} × {}", i.height.unwrap_or(0))),
                    kv("编码", fmt_codec(i.video_codec.as_deref())),
                    kv("码率", fmt_bitrate(i.video_bitrate)),
                    kv("帧率", fmt_fps(i.fps)),
                ]
                .spacing(4)
                .width(Fill),
                None => column![
                    section("视频"),
                    kv("画面", String::from("这个文件只有声音")),
                ]
                .spacing(4)
                .width(Fill),
            };

            let audio = column![
                section("音频"),
                kv("编码", fmt_codec(i.audio_codec.as_deref())),
                kv("声道", fmt_channels(i.audio_channels)),
                kv("采样率", fmt_hz(i.sample_rate)),
                kv("码率", fmt_bitrate(i.audio_bitrate)),
                kv("音轨数", audios.to_string()),
            ]
            .spacing(4)
            .width(Fill);

            let subs_block = column![
                section("字幕"),
                kv("字幕轨", subs.to_string()),
                kv(
                    "类型",
                    if subs == 0 {
                        String::from("无字幕")
                    } else {
                        format!(
                            "内嵌 {} · 外挂 {}",
                            tracks.iter().filter(|t| t.kind == "sub" && !t.external).count(),
                            tracks.iter().filter(|t| t.kind == "sub" && t.external).count()
                        )
                    }
                ),
            ]
            .spacing(4)
            .width(Fill);

            let content = column![file, gap(14.0), video, gap(14.0), audio, gap(14.0), subs_block]
                .width(Fill);

            container(
                column![
                    container(content).width(Fill).padding(Padding {
                        top: 10.0,
                        right: PAD,
                        bottom: 12.0,
                        left: PAD,
                    }),
                ]
                .width(Fill),
            )
            .width(Fill)
            .into()
        }
    };

    let footer = container(
        row![
            text_button("在资源管理器中打开", Message::RevealInExplorer, info.is_some()),
            container(space()).width(Fill),
            text_button("关闭", Message::CloseInfo, true),
        ]
        .spacing(6)
        .align_y(Alignment::Center),
    )
    .width(Fill)
    .padding(Padding {
        top: 8.0,
        right: 10.0,
        bottom: 8.0,
        left: 10.0,
    });

    container(
        column![
            header,
            rule(),
            scrollable(container(body).width(Fill)).height(Fill),
            rule(),
            footer,
        ]
        .width(Fill)
        .height(Fill),
    )
    .width(Length::Fixed(W))
    .height(Fill)
    .style(|_| container::Style {
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

// ---------------- 小部件 ----------------

fn section(title: &str) -> Element<'_, Message> {
    text(title)
        .font(icon::UI_FONT)
        .size(13.5)
        .color(icon::ACCENT)
        .into()
}

/// 一行「标签 —— 值」，值右对齐
fn kv(label: &'static str, value: String) -> Element<'static, Message> {
    row![
        text(label)
            .font(icon::UI_FONT)
            .size(13)
            .color(icon::TEXT_FAINT)
            .width(Length::Fixed(LABEL_W)),
        text(value)
            .font(icon::UI_FONT)
            .size(13)
            .color(icon::TEXT_DIM)
            .width(Fill)
            .align_x(Horizontal::Right)
            .wrapping(Wrapping::None),
    ]
    .spacing(10)
    .align_y(Alignment::Center)
    .into()
}

/// 值可能很长的一行（路径）：占满整行、允许换行
fn kv_wrap(label: &'static str, value: String) -> Element<'static, Message> {
    column![
        text(label)
            .font(icon::UI_FONT)
            .size(13)
            .color(icon::TEXT_FAINT),
        text(value)
            .font(icon::UI_FONT)
            .size(13)
            .line_height(1.4)
            .color(icon::TEXT_DIM)
            .wrapping(Wrapping::Word),
    ]
    .spacing(3)
    .width(Fill)
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

fn gap(h: f32) -> Element<'static, Message> {
    container(space()).height(Length::Fixed(h)).into()
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
        container(
            text(glyph)
                .font(icon::ICON_FONT)
                .size(size)
                .color(icon::TEXT_FAINT),
        )
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

// ---------------- 格式化 ----------------

/// 容器名：ffmpeg 给的是「所有能装进这个 demuxer 的扩展名用逗号连起来」，
/// 直接显示会是一行天书（`mov,mp4,m4a,3gp,3g2,mj2`），这里收敛成人看得懂的写法。
fn fmt_format(raw: Option<&str>) -> String {
    let Some(s) = raw.filter(|s| !s.is_empty()) else {
        return dash();
    };
    match s {
        "mov,mp4,m4a,3gp,3g2,mj2" => String::from("MP4 / MOV"),
        "matroska,webm" => String::from("Matroska / WebM"),
        "avi" => String::from("AVI"),
        "mpegts" => String::from("MPEG-TS"),
        "asf" => String::from("ASF / WMV"),
        "flv" => String::from("FLV"),
        other => other.to_string(),
    }
}

/// 编码名：mpv 的 `track-list/N/codec` 是 ffmpeg 的短名（h264 / hevc / aac），
/// 少数常见的换成更正式的大小写，其余原样显示。
fn fmt_codec(raw: Option<&str>) -> String {
    let Some(s) = raw.filter(|s| !s.is_empty()) else {
        return dash();
    };
    let pretty = match s {
        "h264" => "H.264",
        "hevc" => "H.265 / HEVC",
        "av1" => "AV1",
        "vp9" => "VP9",
        "vp8" => "VP8",
        "mpeg4" => "MPEG-4",
        "mpeg2video" => "MPEG-2",
        "mpeg1video" => "MPEG-1",
        "aac" => "AAC",
        "mp3" => "MP3",
        "opus" => "Opus",
        "flac" => "FLAC",
        "vorbis" => "Vorbis",
        "ac3" => "AC-3",
        "eac3" => "E-AC-3",
        "dts" => "DTS",
        "truehd" => "TrueHD",
        "pcm_s16le" => "PCM 16-bit",
        "pcm_s24le" => "PCM 24-bit",
        other => other,
    };
    pretty.to_string()
}

fn dash() -> String {
    String::from("—")
}

fn fmt_size(bytes: Option<f64>) -> String {
    let Some(b) = bytes.filter(|b| *b > 0.0) else {
        return dash();
    };
    let (v, unit) = if b >= 1024.0 * 1024.0 * 1024.0 {
        (b / 1024.0 / 1024.0 / 1024.0, "GB")
    } else if b >= 1024.0 * 1024.0 {
        (b / 1024.0 / 1024.0, "MB")
    } else {
        (b / 1024.0, "KB")
    };
    format!("{v:.2} {unit}")
}

fn fmt_dur(secs: Option<f64>) -> String {
    let Some(s) = secs.filter(|s| *s > 0.0 && s.is_finite()) else {
        return dash();
    };
    let t = s.round() as u64;
    let (h, m, sec) = (t / 3600, (t % 3600) / 60, t % 60);
    if h > 0 {
        format!("{h}:{m:02}:{sec:02}")
    } else {
        format!("{m:02}:{sec:02}")
    }
}

fn fmt_bitrate(bps: Option<f64>) -> String {
    let Some(b) = bps.filter(|b| *b > 0.0) else {
        return dash();
    };
    if b >= 1_000_000.0 {
        format!("{:.2} Mbps", b / 1_000_000.0)
    } else {
        format!("{:.0} kbps", b / 1000.0)
    }
}

fn fmt_fps(fps: Option<f64>) -> String {
    match fps.filter(|f| *f > 0.0 && f.is_finite()) {
        Some(f) => format!("{f:.3} fps"),
        None => dash(),
    }
}

fn fmt_channels(ch: Option<i64>) -> String {
    match ch {
        Some(1) => String::from("单声道"),
        Some(2) => String::from("立体声"),
        Some(n) if n > 2 => format!("{n} 声道"),
        _ => dash(),
    }
}

fn fmt_hz(hz: Option<i64>) -> String {
    match hz.filter(|h| *h > 0) {
        Some(h) => format!("{:.1} kHz", h as f64 / 1000.0),
        None => dash(),
    }
}
