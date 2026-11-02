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

    loop {
        // ---------- 1. 处理指令 ----------
        while let Ok(cmd) = cmd_rx.try_recv() {
            match cmd {
                Cmd::Load(path) => {
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
                    if let Err(e) = mpv.load(&path.to_string_lossy()) {
                        snap.error = Some(e);
                        open_at = None;
                    } else {
                        mpv.set_flag("pause", false);
                        // 立刻先设一次：大部分片子在解析阶段就能吃住，
                        // 首帧那一下还会再兜一次（见下面 open_ms 分支）
                        mpv.set_speed(speed);
                    }
                }
                Cmd::Pause(p) => mpv.set_flag("pause", p),
                Cmd::Seek(t) => {
                    snap.position = t;
                    mpv.seek_absolute(t);
                }
                Cmd::Volume(v) => mpv.set_double("volume", v),
                Cmd::Speed(v) => {
                    speed = v;
                    speed_dirty = false;
                    mpv.set_speed(v);
                }
                Cmd::RefreshTracks => tracks_at = Some(Instant::now() - Duration::from_secs(1)),
                Cmd::RefreshInfo => snap.media_info = Some(mpv.media_info()),
                Cmd::SubAdd(path) => {
                    if let Err(e) = mpv.sub_add(&path.to_string_lossy()) {
                        snap.error = Some(format!("字幕加载失败：{e}"));
                    }
                    // 新轨要等 mpv 处理完命令才出现在 track-list 里
                    tracks_at = Some(Instant::now() - Duration::from_secs(1));
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
        if let Some(e) = outcome.error {
            snap.error = Some(format!("这个文件打不开：\n{e}"));
            open_at = None;
        }

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

        // 放完了没有。要求「已经出过帧」是因为刚 loadfile 时 mpv 还没开始放，
        // 某些版本会先报一次 yes，照抄会让列表一打开就跳到下一集。
        snap.eof = snap.frame.is_some()
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

        // 有画面在动就快轮询（约 500Hz），暂停/空闲时放慢，省 CPU
        let busy = !snap.paused && snap.frame.is_some();
        std::thread::sleep(if busy {
            Duration::from_millis(2)
        } else {
            Duration::from_millis(10)
        });
    }
}
