//! 音频的封面 / 歌词 / 曲名歌手。
//!
//! 三条来路，按优先级排：
//!
//! 1. **同名 `.lrc`** —— 网上下歌最常见的就是「歌.mp3 + 歌.lrc」两个文件，用户自己
//!    放的那份通常才是他想要的那份（内嵌的常常是错的、或者压根没有）；
//! 2. **内嵌标签** —— 交给 lofty：ID3v2（`USLT`）、Vorbis Comment（`LYRICS`）、
//!    MP4 ilst（`©lyr`）、APE 都能读，封面同理（`APIC` / `METADATA_BLOCK_PICTURE` /
//!    `covr`）；
//! 3. 都没有 —— 界面说一声「没有歌词」，不留一块莫名其妙的空白。
//!
//! 读标签要解封面（几十毫秒，大图更多），不能塞在 UI 线程里 —— 换歌那一下会卡住
//! 画面。所以和时长探测（`probe`）一个套路：单开一条线程，UI 每帧来捞一次结果。

use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, TryRecvError, channel};
use std::thread::JoinHandle;

use lofty::prelude::*;

/// 封面最大边长。
///
/// 原图动辄 1500×1500，界面上一辈子用不到那么大（最大也就 220 逻辑像素），
/// 先缩到 320：每首歌少传 8 MB 像素进 GPU，切歌时的卡顿也就没了。
const COVER_MAX: u32 = 320;

/// 封面解码后的位图（RGBA8），直接交给 iced 的 `image`。
pub struct Cover {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// 一行带时间轴的歌词。
pub struct Line {
    /// 这一行从第几秒开始唱
    pub at: f64,
    pub text: String,
}

/// 歌词。带不带时间轴是两种界面，所以分成两支而不是加个 `bool`。
pub enum Lyrics {
    /// LRC，按时间排好序了
    Synced(Vec<Line>),
    /// 只有文本（内嵌的纯文本歌词、或者没写时间戳的 .lrc）
    Plain(Vec<String>),
}

impl Lyrics {
    /// 当前唱到第几行：最后一条 `at <= pos` 的。
    ///
    /// 前奏（一行都还没到）返回 `None` —— 界面这时候该显示「马上开始」的样子，
    /// 而不是把第一行一直亮着，那样第一句真的唱出来时反而没有变化。
    pub fn index_at(&self, pos: f64) -> Option<usize> {
        let Lyrics::Synced(lines) = self else {
            return None;
        };
        let i = lines.partition_point(|l| l.at <= pos);
        (i > 0).then(|| i - 1)
    }

    /// 同步歌词的行数（纯文本返回 0，界面按另一套画）
    pub fn len(&self) -> usize {
        match self {
            Lyrics::Synced(l) => l.len(),
            Lyrics::Plain(l) => l.len(),
        }
    }
}

/// 一个音频文件能读出来的东西。读不到的字段就是 `None`，界面自己兜底。
#[derive(Default)]
pub struct Meta {
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub cover: Option<Cover>,
    pub lyrics: Option<Lyrics>,
}

/// 读一个文件的标签 + 同名 .lrc。
///
/// 这个函数是**阻塞**的（读文件、解 JPEG），只该在后台线程里调。
pub fn load(path: &Path) -> Meta {
    let mut meta = Meta::default();

    if let Ok(file) = lofty::read_from_path(path) {
        // 有些容器（mkv 里的音频、部分 m4a）没有「主标签」，退到第一个能用的
        if let Some(tag) = file.primary_tag().or_else(|| file.first_tag()) {
            meta.title = tag.get_string(&ItemKey::TrackTitle).map(str::to_owned);
            meta.artist = tag.get_string(&ItemKey::TrackArtist).map(str::to_owned);
            meta.album = tag.get_string(&ItemKey::AlbumTitle).map(str::to_owned);
            // 一张专辑里可能塞了好几种图（封面 / 封底 / 艺人照），挑最大的那张：
            // 封面通常是最大的，而且比「按图片类型挑」更不容易挑错。
            meta.cover = tag
                .pictures()
                .iter()
                .max_by_key(|p| p.data().len())
                .and_then(|p| decode(p.data()));
            if let Some(text) = tag.get_string(&ItemKey::Lyrics) {
                meta.lyrics = Some(parse_lrc(text));
            }
        }
    }

    // 同名 .lrc 盖过内嵌的：空文件当没有
    if let Some(lrc) = sidecar(path)
        && let Ok(text) = std::fs::read_to_string(&lrc)
        && !text.trim().is_empty()
    {
        meta.lyrics = Some(parse_lrc(&text));
    }

    meta
}

/// `歌.mp3` 旁边找 `歌.lrc`。三种大小写都试一遍：Windows 上无所谓，但别假设。
fn sidecar(path: &Path) -> Option<PathBuf> {
    let stem = path.file_stem()?.to_str()?;
    let dir = path.parent()?;
    ["lrc", "LRC", "Lrc"]
        .into_iter()
        .map(|ext| dir.join(format!("{stem}.{ext}")))
        .find(|p| p.is_file())
}

/// 解封面。解不动的（HEIC、损坏的图）就当没有，绝不让它把整首歌的标签拖垮。
fn decode(bytes: &[u8]) -> Option<Cover> {
    let img = image::load_from_memory(bytes).ok()?;
    let img = img.thumbnail(COVER_MAX, COVER_MAX).to_rgba8();
    Some(Cover {
        width: img.width(),
        height: img.height(),
        rgba: img.into_raw(),
    })
}

/// 解析 LRC。
///
/// 支持一行挂多个时间戳（`[00:12.00][01:20.00]副歌`，副歌重复的写法），
/// `mm:ss` / `mm:ss.xx` / `hh:mm:ss.xx` 三种时长写法，`[ti:]` `[ar:]` 这类元信息
/// 直接跳过。一句时间戳都解析不出来，就当纯文本歌词。
pub fn parse_lrc(text: &str) -> Lyrics {
    let mut timed: Vec<Line> = Vec::new();
    let mut plain: Vec<String> = Vec::new();

    for raw in text.lines() {
        let (stamps, rest) = split_stamps(raw);
        let body = rest.trim();
        if body.is_empty() {
            continue;
        }
        if stamps.is_empty() {
            plain.push(body.to_owned());
        } else {
            for at in stamps {
                timed.push(Line {
                    at,
                    text: body.to_owned(),
                });
            }
        }
    }

    if timed.is_empty() {
        return Lyrics::Plain(plain);
    }
    timed.sort_by(|a, b| a.at.total_cmp(&b.at));
    Lyrics::Synced(timed)
}

/// 把行首连续的 `[...]` 全剥下来：认得出来的当时戳，认不出来的（`ti:` / `ar:` /
/// `offset:`）当元信息丢掉。返回（时间戳列表，剩下的正文字段）。
fn split_stamps(line: &str) -> (Vec<f64>, &str) {
    let mut stamps = Vec::new();
    let mut rest = line;
    while let Some(tail) = rest.strip_prefix('[') {
        let Some(end) = tail.find(']') else {
            // 没有右括号：这行当普通文本（有的歌词就爱写个「[副歌]」）
            break;
        };
        if let Some(at) = parse_stamp(&tail[..end]) {
            stamps.push(at);
        }
        rest = &tail[end + 1..];
    }
    (stamps, rest)
}

/// `02:31` / `02:31.45` / `00:02:31.45` → 秒。
fn parse_stamp(tag: &str) -> Option<f64> {
    let mut parts = tag.split(':');
    let first: f64 = parts.next()?.trim().parse().ok()?;
    let second: f64 = parts.next()?.trim().parse().ok()?;
    match parts.next() {
        None => Some(first * 60.0 + second),
        Some(third) => {
            let s: f64 = third.trim().parse().ok()?;
            Some(first * 3600.0 + second * 60.0 + s)
        }
    }
}

// ---------------- 后台读 ----------------

/// 标签读取线程。和 `probe::Prober` 一个用法：`request()` 丢路径，`drain()` 捞结果。
pub struct Loader {
    tx: Sender<PathBuf>,
    rx: Receiver<(PathBuf, Meta)>,
    handle: Option<JoinHandle<()>>,
}

impl Loader {
    pub fn spawn() -> Option<Self> {
        let (tx, cmd_rx) = channel::<PathBuf>();
        let (out_tx, out_rx) = channel::<(PathBuf, Meta)>();

        let handle = std::thread::Builder::new()
            .name(String::from("maneki-tags"))
            .spawn(move || {
                // 收到多少读多少。连点几首歌时把积压的丢掉，只读最后一首 ——
                // 中间那些反正已经翻篇了，读它们纯属浪费。
                while let Ok(first) = cmd_rx.recv() {
                    let mut latest = first;
                    while let Ok(next) = cmd_rx.try_recv() {
                        latest = next;
                    }
                    let meta = load(&latest);
                    if out_tx.send((latest, meta)).is_err() {
                        return;
                    }
                }
            })
            .ok()?;

        Some(Self {
            tx,
            rx: out_rx,
            handle: Some(handle),
        })
    }

    pub fn request(&self, path: PathBuf) {
        let _ = self.tx.send(path);
    }

    /// 取走已经读好的结果，非阻塞。
    pub fn drain(&self) -> Vec<(PathBuf, Meta)> {
        let mut out = Vec::new();
        loop {
            match self.rx.try_recv() {
                Ok(v) => out.push(v),
                Err(TryRecvError::Empty | TryRecvError::Disconnected) => break,
            }
        }
        out
    }
}

impl Drop for Loader {
    fn drop(&mut self) {
        // 故意不 join：线程可能正卡在解一张大封面，等它会把退出流程拖住。
        // `tx` 随结构体一起丢掉，线程下一次 `recv()` 就会拿到 Err 自己退出。
        let _ = self.handle.take();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lrc_parses_stamps_and_sorts() {
        let text = "[ti:测试]\n[00:12.50]第二句\n[00:02.00]第一句\n[00:20]第三句\n";
        let Lyrics::Synced(lines) = parse_lrc(text) else {
            panic!("有时间戳应该是 Synced");
        };
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0].text, "第一句");
        assert!((lines[0].at - 2.0).abs() < 1e-6);
        assert!((lines[1].at - 12.5).abs() < 1e-6);
        assert_eq!(lines[2].text, "第三句");
        assert!((lines[2].at - 20.0).abs() < 1e-6);
    }

    #[test]
    fn lrc_supports_repeated_chorus_lines() {
        let Lyrics::Synced(lines) = parse_lrc("[00:10.00][00:40.00]副歌\n") else {
            panic!("应该是 Synced");
        };
        assert_eq!(lines.len(), 2, "一行挂两个时间戳应该拆成两行");
        assert!((lines[0].at - 10.0).abs() < 1e-6);
        assert!((lines[1].at - 40.0).abs() < 1e-6);
        assert!(lines.iter().all(|l| l.text == "副歌"));
    }

    #[test]
    fn lyrics_without_stamps_fall_back_to_plain() {
        let Lyrics::Plain(lines) = parse_lrc("第一句\n\n第二句\n") else {
            panic!("没有时间戳应该是 Plain");
        };
        assert_eq!(lines, ["第一句", "第二句"], "空行要丢掉");
    }

    #[test]
    fn active_line_follows_the_clock() {
        let lyrics = parse_lrc("[00:01.00]一\n[00:10.00]二\n[00:20.00]三\n");
        assert_eq!(lyrics.index_at(0.5), None, "前奏时还没到第一句");
        assert_eq!(lyrics.index_at(1.0), Some(0), "刚好卡在时间点上算到了");
        assert_eq!(lyrics.index_at(9.99), Some(0));
        assert_eq!(lyrics.index_at(10.0), Some(1));
        assert_eq!(lyrics.index_at(999.0), Some(2), "唱完之后停在最后一句");
    }

    #[test]
    fn hour_long_stamps_are_understood() {
        assert_eq!(parse_stamp("01:02:03"), Some(3723.0));
        assert_eq!(parse_stamp("02:31.45"), Some(151.45));
        assert_eq!(parse_stamp("ti"), None);
    }

    #[test]
    fn plain_lyrics_have_no_active_line() {
        let lyrics = parse_lrc("只有文字\n");
        assert_eq!(lyrics.index_at(30.0), None);
        assert_eq!(lyrics.len(), 1);
    }
}
