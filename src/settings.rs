//! 设置抽屉：通用 / 画面填充 / 画面比例 / 色彩调整 / 快捷键 / 关于。
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

use std::borrow::Cow;

use iced::widget::{button, checkbox, column, container, row, scrollable, slider, space, text};
use iced::{
    Alignment, Background, Border, Color, Element, Fill, Length, Padding, Shadow, Vector,
    alignment::{Horizontal, Vertical},
};

use crate::assoc::Report;
use crate::icon;
use crate::video::{Aspect, FitMode, Look};
use crate::{Message, audio, playlist};

/// 抽屉宽度（逻辑像素）。比播放列表宽一点：这里有滑块和两排分段按钮。
pub const W: f32 = 344.0;
/// 内容区左右内边距
const PAD: f32 = 16.0;

/// ← / → 能选的步长（秒）。设置 → 通用里一排按钮。
pub const SEEK_STEPS: [u64; 5] = [3, 5, 10, 20, 30];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Tab {
    General,
    #[default]
    Picture,
    Color,
    Keys,
    About,
}

impl Tab {
    fn label(self) -> &'static str {
        match self {
            Tab::General => "通用",
            Tab::Picture => "画面",
            Tab::Color => "色彩",
            Tab::Keys => "快捷键",
            Tab::About => "关于",
        }
    }

    const ALL: [Tab; 5] = [
        Tab::General,
        Tab::Picture,
        Tab::Color,
        Tab::Keys,
        Tab::About,
    ];
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

#[derive(Debug, Clone)]
pub struct Settings {
    pub open: bool,
    pub tab: Tab,
    pub fit: FitMode,
    pub aspect: Aspect,
    pub look: Look,
    /// 自动连播。**默认关**：一个视频放完就停在那里，
    /// 不要替用户决定「接着往下看」。
    pub autoplay: bool,
    /// ← / → 一次跳多少秒（默认 5）
    pub seek_step: f64,
    /// 「最近播放」。**默认关** —— 空界面不主动列出用户看过什么。
    pub recent: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            open: false,
            tab: Tab::default(),
            fit: FitMode::default(),
            aspect: Aspect::default(),
            look: Look::default(),
            autoplay: false,
            // 手写 Default 就是为了这一行：derive 出来的 f64 是 0，
            // 没有哪个地方想要「一次跳 0 秒」。
            seek_step: crate::keys::SEEK_STEP,
            recent: false,
        }
    }
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
pub fn drawer<'a>(s: &'a Settings, assoc: &'a Report) -> Element<'a, Message> {
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
        Tab::General => general_tab(s, assoc),
        Tab::Picture => picture_tab(s),
        Tab::Color => color_tab(s),
        Tab::Keys => keys_tab(s.seek_step),
        Tab::About => about_tab(),
    };

    let body = container(content).width(Fill).padding(Padding {
        top: 12.0,
        right: PAD,
        bottom: 12.0,
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

// ---------------- 通用 ----------------

/// 通用：跳转步长、自动连播、文件关联。
///
/// 这三样都不是「画面」的事，以前自动连播挤在画面页最下面，关联文件干脆没有入口
/// （只能靠命令行 `--register`）—— 用户找不到就等于没有。
fn general_tab<'a>(s: &'a Settings, assoc: &'a Report) -> Element<'a, Message> {
    let steps = row(SEEK_STEPS.iter().map(|v| {
        segment_cell(
            format!("{v} 秒"),
            (s.seek_step - *v as f64).abs() < 0.01,
            Message::SetSeekStep(*v as f64),
        )
    }))
    .spacing(6)
    .width(Fill);

    // 关联状态那行：数字必须是「刚数出来的」，所以放在这里由上层传进来，
    // 不在绘制时读注册表（一帧 38 个键，60Hz 就是每秒两千多次系统调用）。
    let status = if assoc.total == 0 {
        String::from("还没有登记任何格式")
    } else if assoc.complete() {
        format!(
            "已关联全部 {} 种格式，双击视频直接用 Maneki 打开",
            assoc.total
        )
    } else {
        format!(
            "已关联 {} / {} 种格式 · 还有 {} 种被别的程序占着",
            assoc.linked,
            assoc.total,
            assoc.pending()
        )
    };

    let hint_text = if assoc.pending() > 0 {
        "剩下的格式可以点右边的按钮，去系统的「默认应用」里手动指定。"
    } else {
        "关联之后双击视频不会再问「用什么程序打开」。"
    };

    let col = column![
        section("跳转"),
        field_label("快退 / 快进一次跳"),
        steps,
        hint("按 ← / → 时跳过的秒数；J / L 固定跳 10 秒。"),
        gap(16.0),
        section("播放"),
        toggle_row("自动连播", s.autoplay, Message::SetAutoplay),
        toggle_row("记录最近播放", s.recent, Message::SetRecent),
        hint(if s.recent {
            "空界面上会列出最近看过的片子，点一条接着看。关掉会清空已有记录。"
        } else {
            "关闭时不记录任何播放历史，空界面也不显示「最近播放」。"
        }),
        gap(16.0),
        section("文件关联"),
        hint(status),
        row![
            primary_button("一键关联全部格式", Message::AssociateAll),
            container(space()).width(Fill),
            text_button("系统默认应用", Message::OpenDefaultApps, true),
        ]
        .spacing(6)
        .align_y(Alignment::Center),
        hint(hint_text),
    ]
    .spacing(8)
    .width(Fill);

    container(col).width(Fill).into()
}

// ---------------- 画面 ----------------

fn picture_tab(s: &Settings) -> Element<'_, Message> {
    let fill = row(FitMode::ALL
        .iter()
        .map(|m| segment(m.label(), s.fit == *m, Message::SetFit(*m))))
    .spacing(6);

    // 五个比例按钮一行塞不下 344px 的抽屉，拆成 3 + 2 两排；
    // 第二排补一格空白，两排的格子才对得上。
    let aspect_a = row(Aspect::ALL[..3]
        .iter()
        .map(|a| segment(a.label(), s.aspect == *a, Message::SetAspect(*a))))
    .spacing(6);
    let mut aspect_b = row(Aspect::ALL[3..]
        .iter()
        .map(|a| segment(a.label(), s.aspect == *a, Message::SetAspect(*a))))
    .spacing(6);
    aspect_b = aspect_b.push(container(space()).width(Fill));

    let col = column![
        section("画面填充"),
        fill,
        gap(16.0),
        section("画面比例"),
        aspect_a,
        aspect_b,
    ]
    .spacing(8)
    .width(Fill);

    container(col).width(Fill).into()
}

/// 一行勾选项（自动连播 / 最近播放）
fn toggle_row(
    label: &'static str,
    on: bool,
    msg: impl Fn(bool) -> Message + 'static,
) -> Element<'static, Message> {
    container(
        checkbox(on)
            .label(label)
            .text_size(13.5)
            .font(icon::UI_FONT)
            .size(18)
            .spacing(9)
            .on_toggle(msg)
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

// ---------------- 关于 ----------------

/// 视频编码：libmpv 内置 FFmpeg 覆盖的主要编码。
///
/// 除了这里列的，FFmpeg 还有一堆老格式/采集卡格式（V210、DPX、Hap…），
/// 但那些普通人碰不到，列出来只是噪音。
const VIDEO_CODECS: &[&str] = &[
    "H.264 / AVC",
    "H.265 / HEVC",
    "AV1",
    "VP8",
    "VP9",
    "VP6",
    "MPEG-1",
    "MPEG-2",
    "MPEG-4 Part 2（DivX / Xvid）",
    "MS MPEG-4 v1/v2/v3",
    "H.261",
    "H.263",
    "VC-1",
    "WMV 7/8/9",
    "Theora",
    "RealVideo 1/2/3/4",
    "ProRes",
    "DNxHD / DNxHR",
    "CineForm",
    "FFV1",
    "Huffyuv",
    "Lagarith",
    "Ut Video",
    "MJPEG",
    "JPEG 2000",
    "DV",
    "AVS",
    "Dirac",
    "Snow",
    "Indeo 3/4/5",
    "Cinepak",
    "Sorenson H.263",
    "QTRLE",
    "MS RLE",
];

/// 音频编码：同上，只留会遇到的。
const AUDIO_CODECS: &[&str] = &[
    "AAC",
    "MP3",
    "MP2",
    "AC-3",
    "E-AC-3",
    "DTS / DTS-HD",
    "TrueHD / MLP",
    "FLAC",
    "ALAC",
    "APE",
    "WavPack",
    "TAK",
    "TTA",
    "Musepack",
    "Shorten",
    "Vorbis",
    "Opus",
    "Speex",
    "WMA v1/v2/Pro/Lossless",
    "Cook（RealAudio）",
    "ATRAC1/3/9",
    "AMR-NB/WB",
    "ADPCM",
    "PCM（WAV / AIFF / CAF）",
    "DSD（DSF / DFF）",
];

/// 把一串名字接成「A · B · C」。
fn dotted(items: &[&str]) -> String {
    items.join(" · ")
}

/// 扩展名表 → 一行大写清单。
///
/// `mid`/`midi` 排除在外：播放列表认这两个后缀（拖进来不会当垃圾丢掉），
/// 但 FFmpeg 根本没有标准 MIDI 的解复用器，列在「支持」里是骗人。
fn ext_list(exts: &[&str]) -> String {
    let names: Vec<String> = exts
        .iter()
        .filter(|e| !matches!(**e, "mid" | "midi"))
        .map(|e| e.to_ascii_uppercase())
        .collect();
    names.join(" · ")
}

/// 「小标题 + 一串格式」的一块。
fn fmt_block(label: &'static str, value: String) -> Element<'static, Message> {
    column![
        field_label(label),
        text(value)
            .font(icon::UI_FONT)
            .size(12.0)
            .color(icon::TEXT_DIM)
            .width(Fill),
    ]
    .spacing(2)
    .width(Fill)
    .into()
}

/// 「关于」页：技术栈、能解什么、开源地址。
///
/// 内容都是编译期常量（`env!("CARGO_PKG_VERSION")` + 硬编码的清单），
/// 不读运行时状态，所以不接收 `&Settings`，换页也不会有任何开销。
// ---------------- 快捷键 ----------------

/// 快捷键说明：左边键位（等宽胶囊）、右边功能。
///
/// 内容来自 `keys.rs` 的 `GROUPS` —— 和真正生效的键位是同一份数据，
/// 不会出现「说明里写了、按下去没反应」。带 `{seek}` 的那几行会把
/// 设置里选的步长填进去，改了跳转秒数说明也跟着变。
fn keys_tab(seek: f64) -> Element<'static, Message> {
    let mut col = column![].spacing(6).width(Fill);
    let step = crate::ui::fmt_secs(seek);

    for (i, (group, rows)) in crate::keys::GROUPS.iter().enumerate() {
        if i > 0 {
            col = col.push(gap(14.0));
        }
        col = col.push(section(group));
        for s in rows.iter() {
            col = col.push(key_row(s.keys, &s.what.replace("{seek}", &step)));
        }
    }

    col = col.push(gap(16.0));
    col = col.push(
        text("鼠标：滚轮在歌词区上下滚一行，Ctrl+滚轮调字号；点歌词跳到那一句")
            .font(icon::UI_FONT)
            .size(12.0)
            .color(icon::TEXT_FAINT)
            .width(Fill),
    );

    container(col).width(Fill).into()
}

/// 一行快捷键：键位块 + 说明。
///
/// 键位块**定宽**、文字**左对齐**：这样每一行的键位都从同一个 x 起步、右边又都收在
/// 同一个胶囊边里，两列（键位 / 说明）看上去就是齐的。以前文字在胶囊里居中，
/// 「M」和「Ctrl + 左右」各自往中间缩，左边缘参差不齐，整页像没对齐。
fn key_row(keys: &'static str, what: impl Into<String>) -> Element<'static, Message> {
    row![
        container(
            text(keys)
                .font(icon::UI_FONT)
                .size(12.0)
                .color(icon::TEXT)
                .align_x(Horizontal::Left)
                .width(Fill),
        )
        .width(Length::Fixed(126.0))
        .padding(Padding {
            top: 4.0,
            right: 8.0,
            bottom: 4.0,
            left: 8.0,
        })
        .style(|_| container::Style {
            background: Some(Background::Color(Color::from_rgba(1.0, 1.0, 1.0, 0.07))),
            border: Border {
                radius: 6.0.into(),
                width: 1.0,
                color: Color::from_rgba(1.0, 1.0, 1.0, 0.10),
            },
            ..container::Style::default()
        }),
        // 说明文字：长了自己换行，不把右边界撑破
        text(what.into())
            .font(icon::UI_FONT)
            .size(12.5)
            .color(icon::TEXT_DIM)
            .width(Fill),
    ]
    .spacing(10)
    .align_y(Alignment::Center)
    .into()
}

fn about_tab() -> Element<'static, Message> {
    let version = env!("CARGO_PKG_VERSION");

    let head = column![
        row![
            text("Maneki")
                .font(icon::UI_FONT)
                .size(19)
                .color(icon::TEXT),
            text(format!("{version}"))
                .font(icon::UI_FONT)
                .size(12)
                .color(icon::TEXT_FAINT),
        ]
        .spacing(8)
        .align_y(Alignment::Center),
        gap(4.0),
        text("极速打开、全格式解码的 Windows 视频播放器")
            .font(icon::UI_FONT)
            .size(12.5)
            .color(icon::TEXT_DIM)
            .width(Fill),
    ]
    .spacing(2)
    .width(Fill);

    let tech = column![
        info_row("开发语言", "Rust（edition 2024）"),
        info_row("界面框架", "iced 0.14"),
        info_row("解码内核", "libmpv · FFmpeg（硬解优先）"),
        info_row("音频输出", "rodio · WASAPI"),
    ]
    .spacing(5)
    .width(Fill);

    // 封装格式直接从扫描用的白名单里取，不另抄一份：以后加格式只改那一处，
    // 这里自动跟上，也就不会出现「关于页说支持、拖进去却打不开」。
    let formats = column![
        fmt_block("视频封装", ext_list(playlist::VIDEO_EXTS)),
        fmt_block("音频封装", ext_list(audio::AUDIO_EXTS)),
        fmt_block("视频编码", dotted(VIDEO_CODECS)),
        fmt_block("音频编码", dotted(AUDIO_CODECS)),
    ]
    .spacing(9)
    .width(Fill);

    // 网址本身就是按钮的文字：抽屉只有 344 宽，再单起一行放 URL 就得滚动才能看全
    let repo = link_button(crate::REPO_URL, Message::OpenRepo);

    // 这句话一行放不下（正文满行也就 296 左右）。交给自动换行的话，断点会落在
    // 「……版权归各自作者 / 所有。」—— 末行只剩三个字，贴着底边看着像被裁掉了。
    // 顺手在逗号后面手动分行，两行长度差不多，也读得顺。
    let credit = column![
        text("MIT 许可证 · 解码由 libmpv / FFmpeg 完成，")
            .font(icon::UI_FONT)
            .size(11.5)
            .color(Color::from_rgba(1.0, 1.0, 1.0, 0.42))
            .width(Fill),
        text("版权归各自作者所有。")
            .font(icon::UI_FONT)
            .size(11.5)
            .color(Color::from_rgba(1.0, 1.0, 1.0, 0.42))
            .width(Fill),
    ]
    .spacing(1)
    .width(Fill);

    // 顺序：技术栈 → 开源 → 格式清单。
    // 格式清单是全写出来的（几十个名字，两千多字），一屏肯定放不下要靠滚动；
    // 把开源地址放在它前面，才不至于点进来先找地址、滚半天才发现压在最后。
    let col = column![
        head,
        gap(12.0),
        section("技术栈"),
        tech,
        gap(12.0),
        section("开源"),
        repo,
        gap(12.0),
        section("支持的格式"),
        formats,
        gap(12.0),
        credit,
    ]
    .spacing(6)
    .width(Fill);

    container(col).width(Fill).into()
}

/// 「标签 — 值」一行（左侧标签定宽，值占满剩下的宽度，长了会自动换行）
fn info_row(label: &'static str, value: &'static str) -> Element<'static, Message> {
    row![
        text(label)
            .font(icon::UI_FONT)
            .size(13)
            .color(icon::TEXT_FAINT)
            .width(Length::Fixed(64.0)),
        text(value)
            .font(icon::UI_FONT)
            .size(13)
            .color(icon::TEXT)
            .width(Fill),
    ]
    .spacing(8)
    .align_y(Alignment::Start)
    .width(Fill)
    .into()
}

fn field_label(label: &'static str) -> Element<'static, Message> {
    text(label)
        .font(icon::UI_FONT)
        .size(12)
        .color(icon::TEXT_FAINT)
        .into()
}

/// 强调色文字按钮（当链接用）
fn link_button(label: &'static str, msg: Message) -> Element<'static, Message> {
    button(text(label).font(icon::UI_FONT).size(13).color(icon::ACCENT))
        .padding(Padding {
            top: 7.0,
            right: 12.0,
            bottom: 7.0,
            left: 12.0,
        })
        .on_press(msg)
        .style(|_t, status| {
            use button::Status;
            let bg = match status {
                Status::Hovered => Color::from_rgba(0.36, 0.72, 0.98, 0.16),
                Status::Pressed => Color::from_rgba(0.36, 0.72, 0.98, 0.24),
                _ => Color::from_rgba(0.36, 0.72, 0.98, 0.08),
            };
            button::Style {
                background: Some(Background::Color(bg)),
                text_color: icon::ACCENT,
                border: Border {
                    radius: 7.0.into(),
                    width: 1.0,
                    color: Color::from_rgba(0.36, 0.72, 0.98, 0.35),
                },
                ..button::Style::default()
            }
        })
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
        right: 6.0,
        bottom: 7.0,
        left: 6.0,
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

/// 分段按钮（画面填充 / 画面比例 / 跳转步长都用它）。
///
/// 文字必须**包一层容器再居中对齐**：iced 的 `button` 只按 padding 把内容摆在左上角
/// （`layout::padded` 不做居中），直接塞一段 `text` 会贴着左边，右边空一大截。
fn segment<'a>(label: &'a str, active: bool, msg: Message) -> Element<'a, Message> {
    segment_cell(Cow::Borrowed(label), active, msg)
}

fn segment_cell<'a>(
    label: impl Into<Cow<'a, str>>,
    active: bool,
    msg: Message,
) -> Element<'a, Message> {
    button(
        container(
            text(label.into())
                .font(icon::UI_FONT)
                .size(13.5)
                .color(if active {
                    Color::from_rgb(0.05, 0.06, 0.08)
                } else {
                    icon::TEXT_DIM
                }),
        )
        .width(Fill)
        .align_x(Horizontal::Center),
    )
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

/// 强调色实心按钮（「一键关联」这种要人点的主动作）
fn primary_button<'a>(label: &'a str, msg: Message) -> Element<'a, Message> {
    button(
        text(label)
            .font(icon::UI_FONT)
            .size(13)
            .color(Color::from_rgb(0.05, 0.06, 0.08)),
    )
    .padding(Padding {
        top: 7.0,
        right: 13.0,
        bottom: 7.0,
        left: 13.0,
    })
    .on_press(msg)
    .style(|_t, status| {
        use button::Status;
        let bg = match status {
            Status::Hovered => Color::from_rgb(0.47, 0.79, 1.0),
            Status::Pressed => Color::from_rgb(0.30, 0.64, 0.92),
            _ => icon::ACCENT,
        };
        button::Style {
            background: Some(Background::Color(bg)),
            text_color: Color::from_rgb(0.05, 0.06, 0.08),
            border: Border {
                radius: 7.0.into(),
                ..Border::default()
            },
            ..button::Style::default()
        }
    })
    .into()
}

/// 说明性小字（浅色、自动换行）
fn hint<'a>(content: impl Into<String>) -> Element<'a, Message> {
    text(content.into())
        .font(icon::UI_FONT)
        .size(12.0)
        .line_height(1.45)
        .color(icon::TEXT_FAINT)
        .width(Fill)
        .into()
}

fn text_button<'a>(label: &'a str, msg: Message, enabled: bool) -> Element<'a, Message> {
    let b = button(text(label).font(icon::UI_FONT).size(13).color(if enabled {
        icon::TEXT_DIM
    } else {
        Color::from_rgba(1.0, 1.0, 1.0, 0.28)
    }))
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

    if enabled {
        b.on_press(msg).into()
    } else {
        b.into()
    }
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
