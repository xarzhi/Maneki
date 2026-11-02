//! 播放列表：目录扫描 + 右侧抽屉面板。
//!
//! 拖进一个文件夹（或点「打开文件夹」）时，把里面所有视频按**自然顺序**排好 ——
//! 「第2集」必须排在「第10集」前面，直接按字符串排会排成 1、10、2、3…
//! 列表本身是贴在右边的一条抽屉，点哪一条就播哪一条。
//!
//! 抽屉不吃画面的事件：整块面板包一层 `button`（`on_press(Noop)`），
//! 点空白处不会穿透到下面的画面去暂停播放。

use std::cmp::Ordering;
use std::path::{Path, PathBuf};

use iced::widget::text::Wrapping;
use iced::widget::{button, column, container, row, scrollable, space, text};
use iced::{
    Alignment, Background, Border, Color, Element, Fill, Length, Padding, Shadow, Vector,
    alignment::{Horizontal, Vertical},
};

use crate::icon;
use crate::ui::{elide, fmt_dur};
use crate::Message;

/// 抽屉宽度（逻辑像素）。比原来宽了 14px：行尾要放时长，名字还得留够地方。
pub const W: f32 = 316.0;
/// 一行的高度。原来是 34，两行字（14 + 12）挤在里面既放不下也没法居中，
/// 提到 40 之后 `align_y(Center)` 才有余量把内容真正摆在中间。
const ROW_H: f32 = 40.0;
/// 行的左右内边距。
///
/// 之前这里是 8 + 竖条 3 + 间距 8 + 序号 22，一行文字要到 65px 才开始 ——
/// 左边空出一大片，看着很松。现在竖条贴到行的最左侧（像条「当前播放」指示带），
/// 序号直接跟在它后面，文字从 33px 起步，左边只留 6px。
///
/// 左内边距再收到 6：10 的时候竖条离面板边缘还有一段空当，配合行高看左边
/// 一片虚位。抽屉头那行文件夹名也用这个值（见 `panel()`），两者才对得上。
const ROW_PAD_L: f32 = 6.0;
const ROW_PAD_R: f32 = 10.0;
/// 序号那一格的宽度（两位数也要对得上）
///
/// 原来 18 且**右对齐**：个位数被推到格子右边，竖条和数字之间空出一截 15px 的
/// 白，看着像数字被踢远了。现在收到 15（两位数 13px 字体正好放得下）并改成居中，
/// 单数字的左右余量对半分，竖条那侧只剩 6px 出头。
const NUM_W: f32 = 15.0;
/// 竖条和序号之间的间距。单独拎出来：行内其它两处用 ROW_GAP，这里要更紧。
const NUM_GAP: f32 = 3.0;
/// 行内后两处间距（序号|名字、名字|时长）
const ROW_GAP: f32 = 6.0;
/// 行尾时长那一格的宽度（"--:--" 也得放得下）
const DUR_W: f32 = 46.0;
/// 名字 / 子目录能用的横向像素：
/// 面板宽 - 左右内边距 - 竖条 - 竖条到序号的间距 - 序号 - 两处行距 - 时长
const NAME_W: f32 = W - ROW_PAD_L - ROW_PAD_R - 3.0 - NUM_GAP - NUM_W - 2.0 * ROW_GAP - DUR_W;
/// 一个文件夹最多收多少个视频（误拖 `C:\` 时不至于把界面搞崩）
const MAX_FILES: usize = 4000;
/// 子目录最多往下钻几层（剧集一般「剧名\第1季\」两层就够）
const MAX_DEPTH: usize = 5;

/// 认得的视频扩展名（小写比较）。
///
/// 只影响「扫目录时算不算视频」，真的解码还是 libmpv 说了算，
/// 所以这里宁宽勿窄 —— 少收一个格式用户会以为程序不支持。
const VIDEO_EXTS: &[&str] = &[
    "mp4", "mkv", "avi", "mov", "flv", "wmv", "webm", "ts", "m2ts", "mts", "m2t", "tp", "trp",
    "rmvb", "rm", "m4v", "mpg", "mpeg", "mpe", "ogv", "ogm", "3gp", "3g2", "asf", "vob", "divx",
    "f4v", "mxf", "dv", "wtv", "nut", "y4m", "amv", "mod", "m1v", "m2v", "mpv",
];

pub fn is_video(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| {
            let e = e.to_ascii_lowercase();
            VIDEO_EXTS.contains(&e.as_str())
        })
        .unwrap_or(false)
}

// ---------------- 扫描 ----------------

/// 一次扫描的结果
pub struct Scan {
    pub videos: Vec<PathBuf>,
    /// 用户拖进来几个「不是视频的文件」
    pub ignored: usize,
}

/// 把一串路径（文件 / 文件夹混杂）摊平成视频列表。
///
/// 文件夹递归扫描，按自然顺序排；重复的路径会去重（同一个文件拖两次）。
pub fn scan(inputs: &[PathBuf]) -> Scan {
    let mut videos = Vec::new();
    let mut ignored = 0usize;

    for p in inputs {
        if p.is_dir() {
            walk(p, 0, &mut videos);
        } else if is_video(p) {
            videos.push(p.clone());
        } else {
            ignored += 1;
        }
    }

    videos.sort_by(|a, b| natural_cmp(&a.to_string_lossy(), &b.to_string_lossy()));
    videos.dedup();
    videos.truncate(MAX_FILES);

    Scan { videos, ignored }
}

fn walk(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
    if depth > MAX_DEPTH || out.len() >= MAX_FILES {
        return;
    }
    let Ok(rd) = std::fs::read_dir(dir) else {
        // 没权限的目录（系统目录、别的用户）直接跳过，不要弹错
        return;
    };
    for entry in rd.flatten() {
        if out.len() >= MAX_FILES {
            return;
        }
        let Ok(ft) = entry.file_type() else { continue };
        let name = entry.file_name();
        let name = name.to_string_lossy();

        if ft.is_dir() {
            // 隐藏目录 / 回收站 / 系统目录一律不进
            if name.starts_with('.') || name.starts_with('$') {
                continue;
            }
            walk(&entry.path(), depth + 1, out);
        } else if is_video(&entry.path()) {
            out.push(entry.path());
        }
    }
}

/// 自然顺序比较：连续数字按**数值**比，其余按字符比（忽略大小写）。
///
/// "ep2" < "ep10"，"第2集" < "第10集"，"a2b10" < "a10b2"。
fn natural_cmp(a: &str, b: &str) -> Ordering {
    let (ab, bb) = (a.as_bytes(), b.as_bytes());
    let (mut i, mut j) = (0usize, 0usize);

    while i < ab.len() && j < bb.len() {
        let (ca, cb) = (ab[i], bb[j]);

        if ca.is_ascii_digit() && cb.is_ascii_digit() {
            // 记录两段数字的区间，按数值比
            let (si, sj) = (i, j);
            while i < ab.len() && ab[i].is_ascii_digit() {
                i += 1;
            }
            while j < bb.len() && bb[j].is_ascii_digit() {
                j += 1;
            }
            // 都是 ASCII 数字，切片边界一定在字符边界上
            let na = a[si..i].trim_start_matches('0');
            let nb = b[sj..j].trim_start_matches('0');

            // 位数多的数字更大；位数一样再逐位比
            let ord = na.len().cmp(&nb.len()).then_with(|| na.cmp(nb));
            if ord != Ordering::Equal {
                return ord;
            }
            // 数值相等时前导 0 多的排后面（"01" 与 "1"）
            let ord = (i - si).cmp(&(j - sj));
            if ord != Ordering::Equal {
                return ord;
            }
            continue;
        }

        if ca.to_ascii_lowercase() != cb.to_ascii_lowercase() {
            return ca.to_ascii_lowercase().cmp(&cb.to_ascii_lowercase());
        }
        i += 1;
        j += 1;
    }

    // 前缀短的排前面
    (ab.len() - i).cmp(&(bb.len() - j))
}

// ---------------- 数据 ----------------

#[derive(Debug, Clone)]
pub struct Item {
    pub path: PathBuf,
    /// 显示名（文件名）
    pub name: String,
    /// 相对来源目录的子目录，嵌套扫描时用来说明「这条在哪个子文件夹里」
    pub group: Option<String>,
    /// 时长（秒）。**扫目录时不读文件**，要等后台探测线程喂进来，
    /// 所以刚建列表这一段是 None（界面显示 `--:--`），随后逐条补齐。
    pub duration: Option<f64>,
}

#[derive(Debug, Default)]
pub struct Playlist {
    /// 抽屉是不是展开着
    pub open: bool,
    /// 列表里都有谁
    pub items: Vec<Item>,
    /// 正在播第几条（不在列表里就是 None）
    pub select: Option<usize>,
    /// 抽屉头里的说明：「4K 合集 · 24 个视频」
    pub title: String,
    /// 空列表时的解释（比如「这个文件夹里没有视频」）
    pub note: Option<String>,
}

impl Playlist {
    /// 用扫出来的视频重建列表。`source` 是用来显示「来源」的目录。
    pub fn set(&mut self, source: Option<&Path>, videos: Vec<PathBuf>) {
        let mut items = Vec::with_capacity(videos.len());

        for p in videos {
            let name = p
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| p.to_string_lossy().into_owned());

            // 嵌套结构里才提示子目录：直接放在来源目录下的就不重复显示了
            let group = source.and_then(|root| {
                let parent = p.parent()?;
                let rel = parent.strip_prefix(root).ok()?;
                if rel.as_os_str().is_empty() {
                    None
                } else {
                    Some(rel.to_string_lossy().into_owned())
                }
            });

            items.push(Item {
                path: p,
                name,
                group,
                duration: None,
            });
        }

        self.title = {
            let count = items.len();
            match source.and_then(|p| p.file_name()).map(|n| n.to_string_lossy().into_owned()) {
                Some(dir) => format!("{dir} · {count} 个视频"),
                None if count == 1 => String::from("1 个视频"),
                None => format!("{count} 个视频"),
            }
        };
        self.items = items;
        self.select = None;
        self.note = None;
    }

    /// 列表清空（也把抽屉关掉）
    pub fn clear(&mut self) {
        self.items.clear();
        self.select = None;
        self.title.clear();
        self.note = None;
        self.open = false;
    }

    pub fn index_of(&self, path: &Path) -> Option<usize> {
        self.items.iter().position(|i| i.path == path)
    }

    /// 后台探测 / 正在播放的那一条回报了时长。
    pub fn set_duration(&mut self, index: usize, secs: Option<f64>) {
        if let Some(item) = self.items.get_mut(index) {
            item.duration = secs.filter(|v| *v > 0.0 && v.is_finite());
        }
    }

    /// 已经在列表里的路径补一个时长（探测线程是按路径回报的）。
    pub fn fill_duration(&mut self, path: &Path, secs: Option<f64>) {
        if let Some(i) = self.index_of(path) {
            self.set_duration(i, secs);
        }
    }

    /// 全部条目都拿到时长时的总秒数；有一条还没探测到就返回 None。
    pub fn known_total(&self) -> Option<f64> {
        let mut sum = 0.0;
        for item in &self.items {
            sum += item.duration?;
        }
        Some(sum)
    }
}

// ---------------- 面板 ----------------

/// 右侧抽屉面板本体（宽 `W`、占满可用高度）。
///
/// 返回的**不是**铺满全屏的一层：贴右边、上下让出顶底栏、以及滑入滑出的位移动画
/// 都由 `main.rs` 用 `Float::translate` 做，这样抽屉和设置抽屉能共用一套动画。
pub fn panel<'a>(p: &'a Playlist) -> Element<'a, Message> {
    let header = container(
        row![
            text("播放列表")
                .font(icon::UI_FONT)
                .size(14.5)
                .color(icon::TEXT),
            container(space()).width(Fill),
            icon_only(icon::CLOSE, 11.0, Message::TogglePlaylist),
        ]
        .align_y(Alignment::Center)
        .padding(Padding {
            top: 0.0,
            right: 6.0,
            bottom: 0.0,
            left: 0.0,
        }),
    )
    .width(Fill)
    .height(Length::Fixed(44.0))
    .padding(Padding {
        top: 0.0,
        right: 8.0,
        bottom: 0.0,
        left: 16.0,
    })
    .align_y(Vertical::Center);

    let content: Element<'a, Message> = if p.items.is_empty() {
        empty_state(p)
    } else {
        let count = p.items.len();
        let mut list = column![].spacing(1).width(Fill);
        for (i, item) in p.items.iter().enumerate() {
            list = list.push(item_row(i, item, p.select == Some(i)));
        }

        container(
            column![
                container(
                    text(match &p.note {
                        Some(n) => n.clone(),
                        None => p.title.clone(),
                    })
                    .font(icon::UI_FONT)
                    .size(13.5)
                    .color(if p.note.is_some() {
                        icon::TEXT_FAINT
                    } else {
                        icon::TEXT_DIM
                    })
                    .wrapping(Wrapping::None),
                )
                .width(Fill)
                .padding(Padding {
                    // 右不留给行的悬停高亮铺满；左边 16 和抽屉标题「播放列表」对齐。
                    //
                    // 上下各 8：上面那道分隔线要紧开一点，下面到第一条列表项
                    // 也要留同样的呼吸（原来 top 是 0，字直接贴着横线）。
                    top: 8.0,
                    right: 0.0,
                    bottom: 8.0,
                    left: 16.0,
                }),
                scrollable(list).height(Fill).width(Fill),
                rule(),
                footer(count, p.known_total()),
            ]
            .height(Fill)
            .width(Fill),
        )
        .height(Fill)
        .width(Fill)
        .into()
    };

    // 整块面板包一层 button：点面板的空白处 / 滚动条不该穿透到画面
    button(
        column![header, rule(), content]
            .width(Fill)
            .height(Fill),
    )
    .width(Length::Fixed(W))
    .height(Fill)
    .padding(0)
    .on_press(Message::Noop)
    .style(|_t, _s| button::Style {
        // 黑色半透明（见 icon::PANEL_BG）：留一点透光，底下画面动的时候能感觉到
        background: Some(Background::Color(icon::PANEL_BG)),
        border: Border {
            // 只圆左边两个角：这条抽屉贴在窗口右边缘，右边是齐的
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
        ..button::Style::default()
    })
    .into()
}

fn item_row<'a>(index: usize, item: &'a Item, playing: bool) -> Element<'a, Message> {
    // 正在播的那条：左边一条竖杠 + 更亮的字 + 淡淡的蓝底
    let marker = container(space())
        .width(Length::Fixed(3.0))
        .height(Length::Fixed(18.0))
        .style(move |_| container::Style {
            background: Some(Background::Color(if playing {
                icon::ACCENT
            } else {
                Color::TRANSPARENT
            })),
            border: Border {
                radius: 2.0.into(),
                ..Border::default()
            },
            ..container::Style::default()
        });

    // 序号固定宽度**居中**：位数不同的行整体还能对齐，个位数也不会被推到格子
    // 右边、在竖条旁边留一截空当。
    let number = container(
        text((index + 1).to_string())
            .font(icon::UI_FONT)
            .size(13)
            .color(if playing {
                icon::ACCENT
            } else {
                Color::from_rgba(1.0, 1.0, 1.0, 0.42)
            }),
    )
    .width(Length::Fixed(NUM_W))
    .align_x(Horizontal::Center);

    // 竖条和序号打包成一小格：两者之间用 NUM_GAP（比行距更紧），
    // 免得竖条像插在空白中间。
    let head = row![marker, number]
        .spacing(NUM_GAP)
        .align_y(Alignment::Center);

    // 名字按像素预算截断。iced 的 text 在 `Wrapping::None` 下既不裁切也不加省略号，
    // 长文件名会直接画到右边的时长上去，只能自己量着截。
    let mut lines = column![
        text(elide(&item.name, NAME_W, 15.0))
            .font(icon::UI_FONT)
            .size(15)
            .line_height(1.25)
            .color(if playing { icon::TEXT } else { icon::TEXT_DIM })
            .wrapping(Wrapping::None),
    ]
    .spacing(1);

    // 第二行只有嵌套子目录才有：直接放在来源目录下的文件不重复显示目录名
    if let Some(group) = &item.group {
        lines = lines.push(
            text(elide(group, NAME_W, 13.0))
                .font(icon::UI_FONT)
                .size(13)
                .line_height(1.25)
                .color(icon::TEXT_FAINT)
                .wrapping(Wrapping::None),
        );
    }

    let dur = container(
        text(fmt_dur(item.duration))
            .font(icon::UI_FONT)
            .size(13)
            .color(if playing {
                icon::TEXT_DIM
            } else {
                icon::TEXT_FAINT
            }),
    )
    .width(Length::Fixed(DUR_W))
    .align_x(Horizontal::Right);

    button(
        // 内容整体在 40px 的行高里**垂直居中**：容器自己不带 align_y 时内容会贴顶，
        // 之前文字偏上就是这个原因。
        container(
            row![head, container(lines).width(Fill), dur]
                .spacing(ROW_GAP)
                .align_y(Alignment::Center),
        )
        .width(Fill)
        .height(Length::Fixed(ROW_H))
        .padding(Padding {
            top: 0.0,
            right: ROW_PAD_R,
            bottom: 0.0,
            left: ROW_PAD_L,
        })
        .align_y(Vertical::Center),
    )
    .width(Fill)
    .padding(0)
    .on_press(Message::PlayIndex(index))
    .style(move |_t, status| {
        use button::Status;
        let bg = match status {
            _ if playing => Some(Background::Color(Color::from_rgba(0.36, 0.72, 0.98, 0.18))),
            Status::Hovered => Some(Background::Color(Color::from_rgba(1.0, 1.0, 1.0, 0.07))),
            Status::Pressed => Some(Background::Color(Color::from_rgba(1.0, 1.0, 1.0, 0.12))),
            _ => None,
        };
        button::Style {
            background: bg,
            text_color: icon::TEXT,
            border: Border {
                radius: 7.0.into(),
                ..Border::default()
            },
            ..button::Style::default()
        }
    })
    .into()
}

fn empty_state<'a>(p: &'a Playlist) -> Element<'a, Message> {
    let hint = p
        .note
        .clone()
        .unwrap_or_else(|| String::from("把视频文件或文件夹拖进窗口，就会出现在这里"));

    container(
        column![
            text(hint)
                .font(icon::UI_FONT)
                .size(13.5)
                .line_height(1.45)
                .color(icon::TEXT_FAINT)
                .wrapping(Wrapping::Word),
            text_button("打开文件夹…", Message::PickFolder, true),
        ]
        .spacing(14)
        .align_x(Alignment::Center),
    )
    .width(Fill)
    .height(Fill)
    .center_x(Fill)
    .padding(Padding {
        top: 40.0,
        right: 24.0,
        bottom: 24.0,
        left: 24.0,
    })
    .into()
}

fn footer<'a>(count: usize, known: Option<f64>) -> Element<'a, Message> {
    // 全列表时长已知时才报总时长：探测是后台一条条补的，
    // 半路显示一个「已经统计到的合计」会让人以为文件时长不对。
    let summary = match known {
        Some(secs) if count > 1 => format!("{count} 个视频 · 共 {}", fmt_dur(Some(secs))),
        _ => format!("{count} 个视频"),
    };

    container(
        row![
            text(summary)
                .font(icon::UI_FONT)
                .size(13)
                .color(icon::TEXT_FAINT),
            container(space()).width(Fill),
            text_button("添加文件夹…", Message::PickFolder, true),
            text_button("清空", Message::ClearPlaylist, count > 0),
        ]
        .spacing(6)
        .align_y(Alignment::Center),
    )
    .width(Fill)
    .padding(Padding {
        top: 8.0,
        right: 8.0,
        bottom: 8.0,
        left: 16.0,
    })
    .into()
}

// ---------------- 小工具 ----------------


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

fn text_button<'a>(label: &'a str, msg: Message, enabled: bool) -> Element<'a, Message> {
    let b = button(
        text(label)
            .font(icon::UI_FONT)
            .size(13.5)
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
            .center(Length::Fixed(24.0)),
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

#[cfg(test)]
mod tests {
    use super::*;

    fn sorted(mut v: Vec<&str>) -> Vec<String> {
        v.sort_by(|a, b| natural_cmp(a, b));
        v.into_iter().map(String::from).collect()
    }

    #[test]
    fn natural_order_puts_2_before_10() {
        assert_eq!(
            sorted(vec!["ep10.mkv", "ep2.mkv", "ep1.mkv"]),
            vec!["ep1.mkv", "ep2.mkv", "ep10.mkv"]
        );
    }

    #[test]
    fn natural_order_handles_chinese_episode_names() {
        assert_eq!(
            sorted(vec!["第10集.mp4", "第2集.mp4", "第1集.mp4"]),
            vec!["第1集.mp4", "第2集.mp4", "第10集.mp4"]
        );
    }

    #[test]
    fn natural_order_is_not_confused_by_leading_zeros() {
        // 01 和 1 数值相同，短的排前面；两者都该排在 2 前面
        assert_eq!(
            sorted(vec!["v2.mp4", "v01.mp4", "v1.mp4"]),
            vec!["v1.mp4", "v01.mp4", "v2.mp4"]
        );
    }

    #[test]
    fn extension_check_is_case_insensitive() {
        assert!(is_video(Path::new(r"C:\a\b\Movie.MKV")));
        assert!(!is_video(Path::new(r"C:\a\b\readme.txt")));
    }
}
