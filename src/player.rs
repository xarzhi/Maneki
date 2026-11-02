//! 播放线程：mpv 的所有调用（事件、解码、软件渲染出 RGBA）都在这条线程里跑。
//!
//! 为什么必须单开线程：
//! libmpv 的 sw 渲染要在 CPU 上做 YUV→RGB 转换 + 缩放，1080p 一帧就是 830 万字节。
//! 之前这些全在 iced 的 update 里同步执行，每帧耗时超过 16ms 的节拍，主线程被彻底占满，
//! 表现就是「画面卡、窗口拖不动、标题栏点了没反应」。
//!
//! 现在的分工：
//! - 播放线程：独占 `Mpv`，循环解码 + 渲染，把成品的 `Arc<Vec<u8>>` 发出去
//! - UI 线程：只发命令、收最新一帧，从不阻塞

use crate::audio::{self, AudioEngine};
use crate::mpv::{MediaInfo, Mpv, Track};
use crate::video::{self, VideoFrame};
use iced::Size;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, SyncSender, TrySendError, channel, sync_channel};
use std::time::{Duration, Instant};

/// CPU 上屏的分辨率预算。超过这个尺寸就交给 GPU 缩小，
/// 否则 4K 每帧 33MB 的 CPU 拷贝会把渲染线程拖垮。
const MAX: Size<u32> = Size::new(1920, 1080);

/// 发往播放线程的指令。
pub enum Cmd {
    Load(PathBuf),
    /// 卸掉当前的片子，回到「什么都没在放」的状态。
    ///
    /// 打开文件夹时用：那时候只是把列表装好，等用户自己点一条再放。
    /// 不光要清界面状态，还得真的把引擎停掉 —— 否则上一条片子还在解码，
    /// 下一帧照样会冒出来盖住界面。
    Stop,
    Pause(bool),
    Seek(f64),
    Volume(f64),
    /// 播放速度倍率（0.5–8.0）
    Speed(f64),
    /// 重新读一遍轨道表（打开设置、加载完字幕之后都要刷）
    RefreshTracks,
    /// 重新读一遍文件元信息（打开「文件详情」抽屉之前发一次）
    RefreshInfo,
    /// 挂一个本地字幕文件
    SubAdd(PathBuf),
    /// 选字幕轨；None = 关掉字幕
    SetSid(Option<i64>),
    Shutdown,
}

/// 一帧成品画面（RGBA8）。直接用上屏模块的类型，省掉一次结构体转换。
pub type Frame = VideoFrame;

/// 播放线程周期性回报的状态快照。
#[derive(Debug, Clone, Default)]
pub struct Snapshot {
    pub frame: Option<Frame>,
    /// 这一条是纯音频：`frame` 永远是 None，界面要画音频版的舞台，
    /// 而且不能用「有没有画面」来判断「加载完了没有」。
    pub audio: bool,
    pub position: f64,
    pub duration: f64,
    pub paused: bool,
    pub error: Option<String>,
    /// 从发起 loadfile 到第一帧上屏的耗时
    pub open_ms: Option<f64>,
    /// 实际出帧速率，用来验证「是不是真的在流畅播放」
    pub fps: f32,
    /// 视频的显示尺寸（mpv 的 dwidth/dheight，已考虑像素长宽比和旋转）。
    /// UI 拿它把窗口调成同样的比例，这样画面铺满、不出黑边。
    pub video_size: Option<(u32, u32)>,
    /// 全部轨道（音视频也带着，UI 只挑 kind == "sub" 的用）
    pub tracks: Vec<Track>,
    /// 这个文件有没有内嵌字幕（外挂的不算）
    pub embedded_subs: bool,
    /// 当前选中的字幕轨 id（-1 = 关闭）
    pub sid: Option<i64>,
    /// 当前文件是不是放完了（mpv 的 `eof-reached`）。
    /// 播放列表靠它自动接下一集：只在 false → true 的那一下动作，
    /// 之后一直停在 true，不会重复触发。
    pub eof: bool,
    /// 当前文件的元信息（「文件详情」抽屉用）。
    /// 首帧出来之后自动读一次，`RefreshInfo` 可以强制重读。
    pub media_info: Option<MediaInfo>,
}

pub struct Player {
    cmd_tx: Sender<Cmd>,
    snap_rx: Receiver<Snapshot>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl Player {
    /// 起一条播放线程。`Mpv` 在这里创建，创建失败（通常是 DLL 找不到）直接返回 Err。
    pub fn spawn() -> Result<Self, String> {
        let mpv = Mpv::new()?;
        let (cmd_tx, cmd_rx) = channel::<Cmd>();
        // 容量 2：UI 偶尔慢一拍也不会把内存撑爆，多余快照直接丢（丢帧好过卡死）
        let (snap_tx, snap_rx) = sync_channel::<Snapshot>(2);

        let handle = std::thread::Builder::new()
            .name(String::from("maneki-player"))
            .spawn(move || run(mpv, cmd_rx, snap_tx))
            .map_err(|e| format!("无法创建播放线程：{e}"))?;

        Ok(Self {
            cmd_tx,
            snap_rx,
            handle: Some(handle),
        })
    }

    pub fn send(&self, cmd: Cmd) {
        let _ = self.cmd_tx.send(cmd);
    }

    /// 取走所有待处理的快照，只保留最后一个。非阻塞。
    pub fn poll(&self) -> Option<Snapshot> {
        let mut latest = None;
        while let Ok(s) = self.snap_rx.try_recv() {
            latest = Some(s);
        }
        latest
    }
}

impl Drop for Player {
    fn drop(&mut self) {
        let _ = self.cmd_tx.send(Cmd::Shutdown);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

fn run(mpv: Mpv, cmd_rx: Receiver<Cmd>, snap_tx: SyncSender<Snapshot>) {
    let mut snap = Snapshot::default();
    let mut buf: Vec<u8> = Vec::new();

    // 打开计时
    let mut open_at: Option<Instant> = None;
    let mut open_ms: Option<f64> = None;

    // fps 统计
    let mut frames_in_window: u32 = 0;
    let mut window_start = Instant::now();

    // 轨道表读一次就缓存，命令行里没变就不用每帧重读
    let mut tracks_at: Option<Instant> = None;

    // 播放速度：mpv 每 loadfile 一次就会把 speed 复位成 1.0，
    // 所以这里自己记着，新文件首帧出来时重新下发一次（连播保持倍速）。
    let mut speed: f64 = 1.0;
    let mut speed_dirty = false;

    // 音频后端。设备懒着开（起一次要几十毫秒），切歌只换 Sink。
    let mut audio_engine: Option<AudioEngine> = None;
    // 当前这条是纯音频（界面要按音频的样子画，且不能用「有没有画面」判断加载完没完）
    let mut audio_mode = false;
    // 当前这条真的走 rodio。wma / ape 这些是音频但 rodio 解不了，交给 mpv，
    // 这时 `audio_mode` 是 true 而这里是 false。
    let mut rodio_active = false;
    // 记住音量：每次新建 Sink 都得重新下发一遍（新 Sink 默认是 1.0）
    let mut volume: f64 = 80.0;
    // 已经 Stop 掉了：这期间不读任何引擎状态，只回报一个「空」快照
    let mut stopped = false;

    loop {
        // ---------- 1. 处理指令 ----------
        while let Ok(cmd) = cmd_rx.try_recv() {
            match cmd {
                Cmd::Load(path) => {
                    stopped = false;
                    snap.error = None;
                    snap.frame = None;
                    snap.position = 0.0;
                    snap.duration = 0.0;
                    snap.video_size = None;
                    snap.tracks.clear();
                    snap.embedded_subs = false;
                    snap.sid = None;
                    snap.media_info = None;
                    snap.eof = false;
                    open_ms = None;
                    open_at = Some(Instant::now());
                    tracks_at = None;
                    speed_dirty = true;

                    audio_mode = audio::is_audio(&path);
                    rodio_active = false;
                    snap.audio = audio_mode;

                    if audio_mode {
                        // 上一条视频还占着解码器就先放掉，免得两路同时出声
                        mpv.stop();
                    }

                    if audio_mode && audio::is_rodio(&path) {
                        if audio_engine.is_none() {
                            match AudioEngine::new() {
                                Ok(e) => audio_engine = Some(e),
                                Err(msg) => {
                                    snap.error = Some(msg);
                                    open_at = None;
                                }
                            }
                        }
                        if let Some(e) = audio_engine.as_mut() {
                            match e.load(&path) {
                                Ok(()) => {
                                    // 新 Sink 的音量/倍速都是默认值，得重新下发
                                    e.set_volume(volume);
                                    e.set_speed(speed);
                                    e.set_paused(false);
                                    rodio_active = true;
                                    open_ms = open_at.map(|t| t.elapsed().as_secs_f64() * 1000.0);
                                }
                                Err(msg) => {
                                    snap.error = Some(msg);
                                    open_at = None;
                                }
                            }
                        }
                    } else {
                        // 视频，或者 rodio 解不了的音频（wma / ape ...）—— 都交给 mpv。
                        // mpv 放纯音频本来就没问题，只是没有画面。
                        if let Some(e) = audio_engine.as_mut() {
                            e.stop();
                        }
                        if let Err(e) = mpv.load(&path.to_string_lossy()) {
                            snap.error = Some(e);
                            open_at = None;
                        } else {
                            mpv.set_flag("pause", false);
                            // 音量要重新下发：放音频时调的音量只发给了 rodio，
                            // mpv 那边还是上一次的值，不补这一下切回视频就会
                            // 「界面上是 40，听起来是 80」。
                            mpv.set_double("volume", volume);
                            // 立刻先设一次：大部分片子在解析阶段就能吃住，
                            // 首帧那一下还会再兜一次（见下面 open_ms 分支）
                            mpv.set_speed(speed);
                        }
                    }
                }
                Cmd::Stop => {
                    mpv.stop();
                    if let Some(e) = audio_engine.as_mut() {
                        e.stop();
                    }
                    rodio_active = false;
                    audio_mode = false;
                    stopped = true;
                    snap = Snapshot::default();
                    snap.paused = true;
                    open_at = None;
                    open_ms = None;
                    tracks_at = None;
                }
                Cmd::Pause(p) => {
                    if rodio_active {
                        if let Some(e) = &audio_engine {
                            e.set_paused(p);
                        }
                    } else {
                        mpv.set_flag("pause", p);
                    }
                }
                Cmd::Seek(t) => {
                    snap.position = t;
                    if rodio_active {
                        if let Some(e) = &audio_engine {
                            e.seek(t);
                        }
                    } else {
                        mpv.seek_absolute(t);
                    }
                }
                Cmd::Volume(v) => {
                    volume = v;
                    if rodio_active {
                        if let Some(e) = &audio_engine {
                            e.set_volume(v);
                        }
                    } else {
                        mpv.set_double("volume", v);
                    }
                }
                Cmd::Speed(v) => {
                    speed = v;
                    speed_dirty = false;
                    if rodio_active {
                        if let Some(e) = &audio_engine {
                            e.set_speed(v);
                        }
                    } else {
                        mpv.set_speed(v);
                    }
                }
                Cmd::RefreshTracks => tracks_at = Some(Instant::now() - Duration::from_secs(1)),
                Cmd::RefreshInfo => {
                    // 音频没有 mpv 实例在放它，读出来的是上一条视频的信息，干脆不读
                    if !audio_mode {
                        snap.media_info = Some(mpv.media_info());
                    }
                }
                Cmd::SubAdd(path) => {
                    if !audio_mode {
                        if let Err(e) = mpv.sub_add(&path.to_string_lossy()) {
                            snap.error = Some(format!("字幕加载失败：{e}"));
                        }
                        // 新轨要等 mpv 处理完命令才出现在 track-list 里
                        tracks_at = Some(Instant::now() - Duration::from_secs(1));
                    }
                }
                Cmd::SetSid(id) => {
                    mpv.set_sid(id);
                    snap.sid = id;
                }
                Cmd::Shutdown => return,
            }
        }

        // ---------- 2. 排空 mpv 事件（解码错误在这里暴露） ----------
        let outcome = mpv.pump_events();
        // 放音频时 mpv 是空闲的（甚至刚被 stop 过），它吐出来的错误不是当前文件的问题
        if let Some(e) = outcome.error
            && !audio_mode
            && !stopped
        {
            snap.error = Some(format!("这个文件打不开：\n{e}"));
            open_at = None;
        }

        if stopped {
            // 卸载之后一律回报「空」。这里必须整个跳过读状态的逻辑：
            // mpv 的 `time-pos` / `eof-reached` 在 stop 之后还留着上一条的尾巴，
            // 照抄回去要么进度条跳一下，要么直接触发「放完了」去播下一集。
            snap.frame = None;
            snap.video_size = None;
            snap.position = 0.0;
            snap.duration = 0.0;
            snap.eof = false;
            snap.paused = true;
            snap.audio = false;
            snap.open_ms = None;
            snap.media_info = None;
            snap.fps = 0.0;
        } else if rodio_active {
            // ---------- 2a. 音频：状态全部问引擎 ----------
            if let Some(e) = &audio_engine {
                // 解码器报不出时长（没有 Xing 头的 VBR mp3）时保留上一次的值，
                // UI 那边还会退回去用播放列表里探测出来的时长。
                let d = e.duration();
                if d > 0.0 {
                    snap.duration = d;
                }
                snap.position = e.position();
                snap.paused = e.is_paused();
                // sink 空了 = 这一首放完了
                snap.eof = e.is_empty();
            }
            snap.frame = None;
            snap.video_size = None;
            snap.open_ms = open_ms;
        } else {
            // ---------- 3. 有新帧就渲染 ----------
            let dw = mpv.get_i64("dwidth").unwrap_or(0).max(0) as u32;
            let dh = mpv.get_i64("dheight").unwrap_or(0).max(0) as u32;

            if dw > 1 && dh > 1 {
                snap.video_size = Some((dw, dh));
            }

            if dw > 1 && dh > 1 && mpv.needs_render() {
                let size = video::fit(Size::new(dw, dh), MAX);
                if mpv.render_into(&mut buf, size.width, size.height) {
                    // 注意：mpv 的 "rgb0" 第 4 字节是填充位，但着色器最终写的是
                    // vec4(rgb, 1.0)，alpha 根本用不到 —— 所以这里不需要逐字节补 255。
                    // 之前那段每帧 200 万次迭代的循环是纯粹的浪费。

                    let data = Arc::new(std::mem::take(&mut buf));
                    snap.frame = Some(Frame {
                        data,
                        width: size.width,
                        height: size.height,
                    });

                    frames_in_window += 1;
                    if open_ms.is_none() {
                        if let Some(t) = open_at {
                            open_ms = Some(t.elapsed().as_secs_f64() * 1000.0);
                        }
                        snap.error = None;

                        // 首帧到手：容器已经解出来了，这时候读元信息和重新下发
                        // 播放速度都最准。只做一次，不进每帧的热路径。
                        if speed_dirty {
                            mpv.set_speed(speed);
                            speed_dirty = false;
                        }
                        snap.media_info = Some(mpv.media_info());
                    }
                }
            }

            // ---------- 4. 读状态 ----------
            if let Some(d) = mpv.get_f64("duration") {
                snap.duration = d;
            }
            if let Some(p) = mpv.get_f64("time-pos") {
                snap.position = p;
            }
            snap.paused = matches!(mpv.get_string("pause").as_deref(), Some("yes"));
            snap.open_ms = open_ms;

            // 放完了没有。要求「已经出过画面」是因为刚 loadfile 时 mpv 还没开始放，
            // 某些版本会先报一次 yes，照抄会让列表一打开就跳到下一集。
            // 纯音频（mpv 回退那一档）本来就没有画面，这时只要加载过就算数。
            snap.eof = (snap.frame.is_some() || audio_mode)
                && matches!(
                    mpv.get_string("eof-reached").as_deref(),
                    Some("yes") | Some("true")
                );

            // ---------- 4b. 轨道表 ----------
            // 首帧出来之后再读（文件没解析完时 track-list 是空的），
            // 之后只在被要求时刷新（打开设置面板 / 加载字幕），不放进每帧的热路径。
            let want_tracks = match tracks_at {
                Some(t) => t.elapsed() >= Duration::from_millis(60),
                None => snap.frame.is_some(),
            };
            if want_tracks {
                let list = mpv.tracks();
                snap.embedded_subs = list.iter().any(|t| t.kind == "sub" && !t.external);
                snap.tracks = list;
                snap.sid = mpv.get_i64("sid").filter(|v| *v >= 0);
                tracks_at = Some(Instant::now());
            }
        }

        // ---------- 5. fps ----------
        let elapsed = window_start.elapsed();
        if elapsed >= Duration::from_millis(500) {
            let fps = frames_in_window as f32 / elapsed.as_secs_f32();
            frames_in_window = 0;
            window_start = Instant::now();
            snap.fps = fps;
        }

        // ---------- 6. 回报 ----------
        match snap_tx.try_send(snap.clone()) {
            Ok(_) | Err(TrySendError::Full(_)) => {}
            Err(TrySendError::Disconnected(_)) => return,
        }

        // 有画面在动就快轮询（约 500Hz），暂停 / 空闲时放慢，省 CPU。
        // 音频不用那么勤：进度条到毫秒级就够顺了，15ms 一轮足够。
        let sleep = if rodio_active {
            Duration::from_millis(15)
        } else if !snap.paused && snap.frame.is_some() {
            Duration::from_millis(2)
        } else {
            Duration::from_millis(10)
        };
        std::thread::sleep(sleep);
    }
}
