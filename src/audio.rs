//! 纯音频播放：rodio（cpal 送 WASAPI，symphonia / hound 解码）。
//!
//! 为什么不干脆也交给 mpv：mpv 那条路要起一整个实例，还得每帧把画面软件渲染成
//! RGBA —— 放一首 mp3 也占着 1080p 的像素缓冲，纯属浪费。rodio 直接往声卡送采样，
//! 内存只有解码器那点缓冲，暂停/跳转也不用绕一层属性系统。
//!
//! 但 rodio 的覆盖面比想象的窄：wma、ape、opus、mka 它解不了；flac 和 m4a 虽然
//! 解得了，**seek 却不行**（详见 [`RODIO_EXTS`] 上的说明）。这些 `player.rs` 都会
//! **回退给 mpv** 放 —— mpv 本来就能放纯音频，只是没有画面而已。所以这里有两张表：
//!
//! - [`is_audio`]：这是不是音频文件（决定要不要走「音频模式」的界面和状态处理）
//! - [`is_rodio`]：这个文件走 rodio 后**拖不拖得动进度条**（决定用哪个后端）

use std::path::Path;
use std::time::Duration;

// `Decoder::total_duration` 是 `Source` trait 上的方法，不从 trait 引进来就找不到
use rodio::Source;

/// 「这是音频文件」的扩展名（小写、不带点）。
///
/// 这张表要**宽**：里面有些格式 rodio 解不了，但 mpv 能，照样能出声。
pub const AUDIO_EXTS: &[&str] = &[
    "mp3", "flac", "wav", "wave", "ogg", "oga", "opus", "m4a", "m4b", "aac", "wma", "ape", "aiff",
    "aif", "aifc", "alac", "mka", "mp2", "mpa", "ac3", "dts", "amr", "wv", "tta", "tak", "dsf",
    "dff", "mid", "midi", "caf", "ra", "au", "snd",
];

/// rodio 能解码**并且能 seek** 的扩展名。和 [`AUDIO_EXTS`] 的交集才是「走 rodio」。
///
/// 判据不是「解不解得开」，而是「拖不拖得动进度条」。这里踩过两个坑，都是
/// 实测出来的（`audio-seek` 那行日志就是为这个留的）：
///
/// - **flac**：rodio 原生后端是 claxon，`try_seek` 直接返回 `NotSupported`；
///   换成 symphonia-flac 也不行 —— 它的二分查找要 `MediaSourceStream::byte_len()`，
///   而 rodio 包源的 `ReadSeekSource::byte_len()` 恒为 `None`，于是
///   `SeekError::Unseekable`。这条路被 rodio 自己堵死了。
/// - **m4a / aac**（symphonia-isomp4）：初始化阶段就可能抛 SeekError，rodio 里
///   对应分支写的是 `unreachable!("Seek errors should not occur during
///   initialization")` —— 直接 panic，整条播放线程没了，表现是「一点声音都没有」。
///
/// 所以这两个都交给 mpv（`player.rs` 里 `is_rodio` 为 false 就走 mpv，纯音频
/// mpv 照放，只是没画面）。留在表里的只有实测能 seek 的：
///
/// - mp3 → symphonia（不是 minimp3，minimp3 的 seek 也没实现）
/// - wav/wave → hound（它自己实现了按采样数定位）
///
/// `opus` 从来就不在：ogg 容器里的 opus 走 symphonia 的 ogg 解复用，
/// rodio 0.19 没开这条，一直是交给 mpv 的。
const RODIO_EXTS: &[&str] = &["mp3", "wav", "wave"];

fn ext_of(path: &Path) -> Option<String> {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
}

/// 是不是音频文件（大写后缀也认）。
pub fn is_audio(path: &Path) -> bool {
    ext_of(path).is_some_and(|e| AUDIO_EXTS.contains(&e.as_str()))
}

/// rodio 能不能自己解这个文件。
pub fn is_rodio(path: &Path) -> bool {
    ext_of(path).is_some_and(|e| RODIO_EXTS.contains(&e.as_str()))
}

/// 一个音频输出 + 一个正在播的 Sink。
///
/// **设备只开一次**：`cpal` 每次 `try_default` 都要走一遍 WASAPI 初始化（几十毫秒，
/// 有时还能听到一声轻响）。所以切歌只换 `Sink`，`OutputStream` 一直留着。
pub struct AudioEngine {
    /// 这个字段看起来没用，其实是命根子：它一 drop，音频线程就退出了。
    _out: rodio::OutputStream,
    handle: rodio::OutputStreamHandle,
    sink: Option<rodio::Sink>,
    /// 解码器报的时长（秒）。报不出来就是 0，界面那边会退回去用播放列表里探测的值。
    duration: f64,
}

impl AudioEngine {
    /// 打开默认音频设备。没有声卡 / 被独占时会失败，错误直接显示给用户。
    pub fn new() -> Result<Self, String> {
        let (out, handle) = rodio::OutputStream::try_default()
            .map_err(|e| format!("音频设备打不开：\n{e}"))?;
        Ok(Self {
            _out: out,
            handle,
            sink: None,
            duration: 0.0,
        })
    }

    /// 换一首。老的先停（`Sink` 一 drop 声音立刻断），再建新的。
    pub fn load(&mut self, path: &Path) -> Result<(), String> {
        self.sink = None;
        self.duration = 0.0;

        let file = std::fs::File::open(path).map_err(|e| format!("打不开文件：\n{e}"))?;
        // 包一层 BufReader：mp3 是逐帧读的，裸 File 每读一次就是一次系统调用
        let decoder = rodio::Decoder::new(std::io::BufReader::new(file))
            .map_err(|e| format!("这个音频文件解不了：\n{e}"))?;

        // 时长拿不到就留 0（VBR 的 mp3 没有 Xing 头时常见）
        self.duration = decoder
            .total_duration()
            .map(|d| d.as_secs_f64())
            .unwrap_or(0.0);

        let sink =
            rodio::Sink::try_new(&self.handle).map_err(|e| format!("音频输出初始化失败：\n{e}"))?;
        sink.append(decoder);
        self.sink = Some(sink);
        Ok(())
    }

    /// 停掉当前的（切到视频时用）。
    pub fn stop(&mut self) {
        self.sink = None;
        self.duration = 0.0;
    }

    pub fn is_empty(&self) -> bool {
        self.sink.as_ref().is_none_or(|s| s.empty())
    }

    pub fn duration(&self) -> f64 {
        self.duration
    }

    pub fn position(&self) -> f64 {
        self.sink
            .as_ref()
            .map(|s| s.get_pos().as_secs_f64())
            .unwrap_or(0.0)
    }

    pub fn is_paused(&self) -> bool {
        self.sink.as_ref().is_some_and(|s| s.is_paused())
    }

    pub fn set_paused(&self, paused: bool) {
        let Some(sink) = &self.sink else { return };
        if paused {
            sink.pause();
        } else {
            sink.play();
        }
    }

    /// 跳到第 `t` 秒。rodio 的 `try_seek` 会阻塞 0~5ms 等音频线程确认，
    /// 所以只在播放线程里调。
    ///
    /// 失败**不能默默吞掉**：`try_seek` 对不支持的源会返回 Err，界面那边照样
    /// 把进度条和歌词挪过去了，用户听到的却还是原来那段 —— 看着就是「点了没反应」。
    /// release 下没法往界面上弹（会把整块播放界面换成错误面板），至少在 debug
    /// 里留一条能被捞出来的日志。
    pub fn seek(&self, t: f64) {
        let Some(sink) = &self.sink else { return };
        let t = t.max(0.0);
        let result = sink.try_seek(Duration::from_secs_f64(t));
        #[cfg(debug_assertions)]
        match &result {
            Ok(()) => eprintln!("audio-seek: {t:.2}s ok"),
            Err(e) => eprintln!("audio-seek: {t:.2}s 失败：{e:?}"),
        }
        let _ = result;
    }

    /// 音量：界面用的是 0~100，rodio 要的是线性增益（1.0 = 原音量）。
    /// 上限放到 1.5 是给「音量条拉满」留一点余量，和 mpv 那套的手感对齐。
    pub fn set_volume(&self, vol: f64) {
        if let Some(sink) = &self.sink {
            sink.set_volume((vol / 100.0).clamp(0.0, 1.5) as f32);
        }
    }

    /// 倍速。rodio 是重采样式的变速（音调会跟着变），和 mpv 的 `speed` 一样。
    pub fn set_speed(&self, speed: f64) {
        if let Some(sink) = &self.sink {
            sink.set_speed(speed.clamp(0.25, 8.0) as f32);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn p(s: &str) -> PathBuf {
        PathBuf::from(s)
    }

    #[test]
    fn audio_extensions_are_recognised() {
        for name in [
            "a.mp3",
            "a.FLAC",
            "a.wav",
            "a.m4a",
            "a.ogg",
            "a.wma",
            "a.ape",
            "a.MKA",
        ] {
            assert!(is_audio(&p(name)), "{name} 应该是音频");
        }
    }

    #[test]
    fn video_and_junk_are_not_audio() {
        for name in ["a.mp4", "a.mkv", "a.txt", "noext", "a.jpg"] {
            assert!(!is_audio(&p(name)), "{name} 不该算音频");
        }
    }

    #[test]
    fn only_seekable_audio_goes_to_rodio() {
        // rodio 这边只剩实测能拖进度条的两个
        assert!(is_rodio(&p("a.mp3")));
        assert!(is_rodio(&p("a.wav")));
        assert!(is_rodio(&p("a.WAVE")));

        // 解不开的：rodio 没这些解码器
        assert!(is_audio(&p("a.wma")) && !is_rodio(&p("a.wma")));
        assert!(is_audio(&p("a.ape")) && !is_rodio(&p("a.ape")));

        // 解得出、但**拖不动**的：flac 缺 byte_len、m4a 会 panic、ogg 的
        // lewton 没实现 seek。这三个必须回退给 mpv，否则用户看到的就是
        // 「界面挪了、声音没动」。
        for name in ["a.flac", "a.FLAC", "a.m4a", "a.aac", "a.ogg", "a.oga"] {
            assert!(is_audio(&p(name)), "{name} 是音频");
            assert!(!is_rodio(&p(name)), "{name} 不能走 rodio：它 seek 不了");
        }

        // 视频永远不走 rodio
        assert!(!is_rodio(&p("a.mp4")));
    }
}
