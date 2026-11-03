//! 进度条悬停预览：把视频在某个时间点的画面抽成一格小图。
//!
//! 需求是「鼠标停在进度条上，显示那个位置的缩略图」，所以时间轴上的任意一点
//! 都可能被问到。做法是**按 2 秒分桶**，一个桶抽一张图，抽到的留在界面那边的
//! 缓存里，同一个桶再问就是零延迟。
//!
//! 为什么不在主播放器上抽：
//! - 主播放器正在放片子，对它 seek 会让画面跳一下；
//! - 抽帧本身要解码定位，混在播放循环里会掉帧。
//!
//! 所以这里是第三条独立线程 + 第三个 mpv 实例（前两条是播放、时长探测）。
//! 线程里所有 mpv 调用都是串行的，界面只往队列里塞请求、从通道里取结果，
//! 完全不阻塞。
//!
//! 抽帧的坑在「seek 之后什么时候那张图才画出来」：`time-pos` 一改就立刻读
//! 渲染上下文，读到的往往是**纯黑底色**（目标帧还在从关键帧往后解）。所以这里
//! 认 mpv 的 `playback-restart` 事件（寻址完成的信号），等它到了再取帧，
//! 并且用「整块全零」而不是「平均亮度」来判断空底 —— 细节见 [`grab`]。

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, TryRecvError, channel};
use std::time::{Duration, Instant};

use crate::mpv::Mpv;

/// 一张缩略图的宽度（高度按视频比例算，见 [`thumb_height`]）
pub const W: u32 = 160;
/// 时间轴分桶的粒度。2 秒一格：一小时也就 1800 格，够密了。
pub const STEP: f64 = 2.0;
/// 队列最多压这么多请求。用户来回扫进度条时会灌得很快，
/// 压太多只会让「他早就挪走了」的那些帧还在排队解。
const QUEUE_MAX: usize = 8;
/// 打开文件最多等多久（大文件、网络盘上 open 会慢）
const OPEN_TIMEOUT: Duration = Duration::from_secs(3);
/// 单帧最多等多久。超时就放弃这一格，界面继续保持「没有预览」。
///
/// 这个值是按最坏情况定的：4K HDR 的片子上，一次精确定位要从关键帧一路解到
/// 目标位置，实测 1.4 秒起步（还是多线程 + 跳过去块的情况下）。给 5 秒是留够
/// 余量 —— 反正它只在鼠标停住时才跑，等久一点比「弹出个黑的」强。
const FRAME_TIMEOUT: Duration = Duration::from_millis(5000);
/// 连续拿到几帧「纯黑」就认定这个位置真的是黑的，别再等了。
/// 一两帧黑是正常的（解码器还没把画面交出来，渲染上下文先画了个底色），
/// 每帧都是黑的话那就是画面本身黑，等下去也没用。
const BLANK_TRIES: u32 = 3;

/// 缓存键：哪条片子 + 第几个桶
pub type Key = (String, i64);

/// 抽好的一格
pub struct Shot {
    pub key: Key,
    pub w: u32,
    pub h: u32,
    /// RGBA，`w*h*4`，alpha 已补成 255
    pub rgba: Vec<u8>,
}

struct Job {
    path: PathBuf,
    key: Key,
    at: f64,
    /// 这是线程自己顺手预取的一格（不是用户指着的那格）。
    /// 预取的结果只进缓存、不继续往后预取，免得变成一路扫下去。
    prefetch: bool,
}

pub struct Thumbs {
    tx: Sender<Job>,
    rx: Receiver<Shot>,
    stop: Arc<AtomicBool>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl Thumbs {
    pub fn spawn() -> Result<Self, String> {
        let (tx, job_rx) = channel::<Job>();
        let (out_tx, out_rx) = channel::<Shot>();
        let stop = Arc::new(AtomicBool::new(false));

        let flag = stop.clone();
        let handle = std::thread::Builder::new()
            .name(String::from("maneki-thumbs"))
            .spawn(move || run(job_rx, out_tx, flag))
            .map_err(|e| format!("无法创建缩略图线程：{e}"))?;

        Ok(Self {
            tx,
            rx: out_rx,
            stop,
            handle: Some(handle),
        })
    }

    /// 要一格。同一个键重复要也没关系，界面那边的缓存会挡住；
    /// 这里不做去重，省一张表。
    pub fn want(&self, path: &std::path::Path, bucket: i64) {
        let at = bucket_time(bucket);
        let _ = self.tx.send(Job {
            path: path.to_path_buf(),
            key: (path.to_string_lossy().into_owned(), bucket),
            at,
            prefetch: false,
        });
    }

    /// 取走已经抽好的，非阻塞。
    pub fn drain(&self) -> Vec<Shot> {
        let mut out = Vec::new();
        loop {
            match self.rx.try_recv() {
                Ok(s) => out.push(s),
                Err(TryRecvError::Empty | TryRecvError::Disconnected) => break,
            }
        }
        out
    }
}

impl Drop for Thumbs {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

/// 第 `bucket` 格对应的时间点：取桶的**正中**，这样边缘也不会落到隔壁格。
pub fn bucket_time(bucket: i64) -> f64 {
    (bucket as f64 + 0.5) * STEP
}

/// 时间点落在第几格
pub fn bucket_of(t: f64) -> i64 {
    (t / STEP).floor() as i64
}

/// 按视频比例算缩略图高度。太扁的（宽银幕）压到底线，太方的不超高。
pub fn thumb_height(video_w: u32, video_h: u32) -> u32 {
    if video_w == 0 || video_h == 0 {
        return (W * 9 / 16).max(1);
    }
    let h = (W as u64 * video_h as u64 / video_w as u64) as u32;
    h.clamp(40, 120)
}

fn run(rx: Receiver<Job>, tx: Sender<Shot>, stop: Arc<AtomicBool>) {
    let mut mpv: Option<Mpv> = None;
    let mut queue: VecDeque<Job> = VecDeque::new();
    let mut loaded: Option<String> = None;
    let mut size: (u32, u32) = (0, 0);
    // 当前这条片子的时长（预取时要拿它判断有没有越过片尾）
    let mut dur: f64 = 0.0;
    // 刚服务完的那一格（路径, 桶）。队列空的时候顺着它预取下一格 ——
    // 用户往往是往右扫的，先备好下一张，扫起来才不一顿一顿。
    let mut last: Option<(PathBuf, i64, f64)> = None;

    loop {
        if stop.load(Ordering::Relaxed) {
            return;
        }

        // 新请求一律插到队头：用户刚指的那一格才是他要看的，
        // 前面排着的（他已经扫过去的）可以慢慢来，实在积压就丢掉。
        loop {
            match rx.try_recv() {
                Ok(job) => {
                    queue.push_front(job);
                    if queue.len() > QUEUE_MAX {
                        queue.pop_back();
                    }
                }
                Err(TryRecvError::Disconnected) => return,
                Err(TryRecvError::Empty) => break,
            }
        }

        let Some(job) = queue.pop_front() else {
            // 队列空了：顺手把刚才那一格的下一格预取出来（只预取一格，
            // 预取出来的不再继续往后预取，否则就变成一路扫到底了）。
            match last.take() {
                Some((path, bucket, dur)) if bucket_time(bucket + 1) < dur - 0.5 => {
                    let next = bucket + 1;
                    queue.push_back(Job {
                        key: (path.to_string_lossy().into_owned(), next),
                        at: bucket_time(next),
                        path,
                        prefetch: true,
                    });
                }
                _ => std::thread::sleep(Duration::from_millis(80)),
            }
            continue;
        };
        if log_enabled() {
            eprintln!("thumb job: bucket={} t={:.1} {}", job.key.1, job.at, job.path.display());
        }

        if mpv.is_none() {
            match Mpv::frames() {
                Ok(m) => mpv = Some(m),
                Err(_) => {
                    // DLL 都没了，重试也没意义
                    std::thread::sleep(Duration::from_secs(1));
                    continue;
                }
            }
        }
        let Some(m) = mpv.as_ref() else { continue };

        let path_str = job.path.to_string_lossy().into_owned();

        // 换片子才重新 load —— 同一部片子来回 seek 便宜得多
        if loaded.as_deref() != Some(path_str.as_str()) {
            loaded = None;
            size = (0, 0);
            let opened = open(m, &path_str, &stop);
            if let Some(d) = opened {
                dur = d;
                size = video_size(m);
                loaded = Some(path_str.clone());
            }
            if log_enabled() {
                eprintln!(
                    "thumb open: {} dur={:?} 尺寸 {}x{} 像素格式={:?} primaries={:?} gamma={:?} hdr={:?}",
                    if opened.is_some() { "成功" } else { "失败" },
                    opened,
                    size.0,
                    size.1,
                    m.get_string("video-params/pixelformat"),
                    m.get_string("video-params/primaries"),
                    m.get_string("video-params/gamma"),
                    m.get_string("video-params/sig-peak"),
                );
            }
        }
        if loaded.is_none() {
            continue;
        }

        if let Some(shot) = grab(m, &job, size, &stop)
            && tx.send(shot).is_err()
        {
            return;
        }

        // 预取出来的那一格不参与「继续往后预取」，否则会一路扫到片尾。
        if !job.prefetch {
            last = Some((job.path.clone(), job.key.1, dur));
        }
    }
}

/// 打开文件，等到它的 `path` 属性和 `duration` 都就位。
fn open(m: &Mpv, target: &str, stop: &AtomicBool) -> Option<f64> {
    m.load(target).ok()?;
    let _ = m.pump_events();

    let start = Instant::now();
    while start.elapsed() < OPEN_TIMEOUT {
        if stop.load(Ordering::Relaxed) {
            return None;
        }
        if current_path_is(m, target)
            && let Some(d) = m.get_f64("duration")
            && d > 0.0
        {
            return Some(d);
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    None
}

fn current_path_is(m: &Mpv, target: &str) -> bool {
    let Some(cur) = m.get_string("path") else {
        return false;
    };
    // mpv 会把路径规范化（分隔符、盘符大小写），别用 == 直接比
    cur.replace('/', "\\").to_ascii_lowercase() == target.replace('/', "\\").to_ascii_lowercase()
}

/// 视频的像素尺寸。纯音频没有视频轨，这里会拿到 0。
fn video_size(m: &Mpv) -> (u32, u32) {
    let w = m.get_i64("width").unwrap_or(0).max(0) as u32;
    let h = m.get_i64("height").unwrap_or(0).max(0) as u32;
    (w, h)
}

/// seek 到目标位置，等那张图真的画出来，再缩成缩略图。
///
/// 这个函数踩过的坑值得写下来，全是「交出一张黑图」的成因：
///
/// 1. `time-pos` 设完**立刻**就变成目标值，但画面还得从关键帧一路解过来。
///    这期间 mpv 会先画一帧纯黑底色。以前看完 `time-pos` 就取帧，于是在长 GOP /
///    4K 的片子上永远拿到那张黑帧 —— 表现就是「鼠标指哪儿都是黑的」。
/// 2. `needs_render()` 会把「有新帧」那个**一次性**标志吃掉。暂停状态下 mpv
///    只会在寻址完成时再发一次；要是在寻址过程中乱读，就把那唯一一次吃掉了，
///    之后永远等不到 —— 所以「等寻址结束」这一段**一次都不能读**。
/// 3. 因此寻址结束后不再死等标志：mpv 的渲染本来就是画「当前帧」，寻址完成后
///    当前帧就是目标帧。等不到标志就定时硬画一帧，比干等靠谱。
fn grab(m: &Mpv, job: &Job, size: (u32, u32), stop: &AtomicBool) -> Option<Shot> {
    let (vw, vh) = size;
    if vw == 0 || vh == 0 {
        return None;
    }
    let th = thumb_height(vw, vh);

    // 先清掉上一条命令留下的事件，否则这里会立刻读到**上一次**寻址的
    // playback-restart，等于没等。
    let _ = m.pump_events();

    m.seek_absolute(job.at);
    let start = Instant::now();

    // ---- 1. 等寻址结束（全程不碰渲染上下文）----
    //
    // 认 `playback-restart` 这条事件，不认 `time-pos`，也不轮询 `needs_render`。
    // 前者一设就变（画面还没解出来），后者是一次性标志，读了就没了。
    let done = m.wait_seek_done(FRAME_TIMEOUT);
    if stop.load(Ordering::Relaxed) {
        return None;
    }
    if !done && log_enabled() {
        eprintln!("thumb 寻址事件没来: t={:.1} 当前位置 {:?}", job.at, m.get_f64("time-pos"));
    }
    // 事件没来也往下走：定位也许已经完成、只是事件被前面清掉了，
    // 后面还有黑帧检测兜底。

    // ---- 2. 把这一帧画出来 ----
    let mut buf: Vec<u8> = Vec::new();
    let mut blanks: u32 = 0;
    loop {
        if start.elapsed() >= FRAME_TIMEOUT {
            if log_enabled() {
                eprintln!("thumb 等画面超时: t={:.1} 黑帧 {}", job.at, blanks);
            }
            return None;
        }
        if stop.load(Ordering::Relaxed) {
            return None;
        }

        // 有「新帧」标志就等它（省得白画），但最多等 250ms —— 解码慢的时候
        // 标志可能已经被吃掉了，这时候直接画当前帧照样能拿到目标画面。
        let waited = Instant::now();
        while !m.needs_render() && waited.elapsed() < Duration::from_millis(250) {
            if stop.load(Ordering::Relaxed) {
                return None;
            }
            std::thread::sleep(Duration::from_millis(4));
        }

        if !m.render_into(&mut buf, W, th) {
            std::thread::sleep(Duration::from_millis(4));
            continue;
        }

        if log_enabled() {
            eprintln!(
                "  try t={:.1} {}ms pos={:?} idle={:?} vo={:?} 亮度={:.1}",
                job.at,
                start.elapsed().as_millis(),
                m.get_f64("time-pos"),
                m.get_string("core-idle"),
                m.get_string("vo-configured"),
                mean(&buf)
            );
        }

        if is_blank(&buf) {
            // 解码器还没交出画面，mpv 画的是底色。再等等，但别无限等：
            // 真·全黑的画面等多久都是黑的。
            blanks += 1;
            if blanks >= BLANK_TRIES {
                if log_enabled() {
                    eprintln!("thumb 放弃: t={:.1} 连续 {} 帧纯黑", job.at, blanks);
                }
                return None;
            }
            std::thread::sleep(Duration::from_millis(20));
            continue;
        }

        // rgb0 → rgba：mpv 交出来的第 4 个字节是填充位，iced 要的是 alpha
        for px in buf.chunks_exact_mut(4) {
            px[3] = 255;
        }
        if log_enabled() {
            eprintln!(
                "thumb ok: t={:.1} 用时 {}ms 平均亮度 {:.1}",
                job.at,
                start.elapsed().as_millis(),
                mean(&buf)
            );
        }
        return Some(Shot {
            key: job.key.clone(),
            w: W,
            h: th,
            rgba: buf,
        });
    }
}

/// 这张图是不是 mpv「还没画出画面」时的那张空底。
///
/// 这里**不能用平均亮度**判断。4K HDR 的片子里，暗场画面经 tonemap 之后
/// 平均亮度只有 0.5 上下（实测一部 4K HDR 片子 41 秒处 ffmpeg 直解是 13.9/255，
/// mpv 映射到 SDR 只有 0.6），跟空底几乎分不开 —— 用亮度判会把
/// 正常的暗场当成空底，用户看到的就是「这段没有预览」。
///
/// 空底的特征是**整块全是 0**（mpv 直接清成黑），真正的画面哪怕再暗也会有
/// 几个非零像素。所以只认「一个亮着的像素都没有」。
fn is_blank(rgba: &[u8]) -> bool {
    let lit = rgba
        .chunks_exact(4)
        .filter(|p| p[0] > 0 || p[1] > 0 || p[2] > 0)
        .take(4)
        .count();
    lit == 0
}

/// RGB 三通道的平均值（0~255），只看亮不亮，不管 alpha。
fn mean(rgba: &[u8]) -> f64 {
    let px = rgba.chunks_exact(4);
    let n = px.len().max(1);
    let sum: u64 = rgba
        .chunks_exact(4)
        .map(|p| p[0] as u64 + p[1] as u64 + p[2] as u64)
        .sum();
    sum as f64 / (n * 3) as f64
}

/// `QP_THUMB_LOG=1` 时把抽帧情况打到 stderr，排查「预览是黑的」用。
fn log_enabled() -> bool {
    std::env::var_os("QP_THUMB_LOG").is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bucket_grid_is_stable() {
        assert_eq!(bucket_of(0.0), 0);
        assert_eq!(bucket_of(1.9), 0);
        assert_eq!(bucket_of(2.0), 1);
        // 桶的正中就是它代表的时刻
        assert!((bucket_time(0) - 1.0).abs() < 1e-9);
        assert!((bucket_time(3) - 7.0).abs() < 1e-9);
        // 落到格子里再取时间，必须还在同一格里
        for t in [0.0, 0.4, 5.5, 59.99, 3600.0] {
            let b = bucket_of(t);
            let back = bucket_time(b);
            assert_eq!(bucket_of(back), b, "{t} 落格不闭合");
            assert!((back - t).abs() <= STEP, "{t} 偏差过大：{back}");
        }
    }

    #[test]
    fn thumb_height_follows_aspect() {
        // 16:9 就是 160x90
        assert_eq!(thumb_height(1920, 1080), 90);
        // 4:3 更高一点，但不超过上限
        assert_eq!(thumb_height(640, 480), 120);
        // 竖屏 / 方屏压到上限，不会撑出一条又高又窄的图
        assert_eq!(thumb_height(1080, 1920), 120);
        // 电影院宽银幕压到底线
        assert_eq!(thumb_height(1920, 800), 66);
        // 拿不到尺寸时退回 16:9
        assert_eq!(thumb_height(0, 0), 90);
    }

    /// 真抽一帧出来 —— 要 libmpv，还要一个真视频，所以默认不跑。
    ///
    /// 手动验：`QP_THUMB_TEST=<视频路径> cargo test -- --ignored grabs_a_real_frame`
    ///
    /// 验三件事：尺寸对、alpha 补成了 255、**不同时间点抽出来的画面不一样**。
    /// 最后一条最关键：seek 没生效的话每格都会是同一张首帧，看着「有图」其实没用。
    #[test]
    #[ignore = "需要 libmpv 和真实视频，用 QP_THUMB_TEST 指定路径手动跑"]
    fn grabs_a_real_frame() {
        let Ok(path) = std::env::var("QP_THUMB_TEST") else {
            return;
        };
        let path = std::path::Path::new(&path);
        let engine = match Thumbs::spawn() {
            Ok(t) => t,
            Err(e) => panic!("起不了抽帧线程：{e}"),
        };

        let wait = |bucket: i64| -> Option<Shot> {
            engine.want(path, bucket);
            let deadline = Instant::now() + Duration::from_secs(8);
            while Instant::now() < deadline {
                if let Some(s) = engine.drain().into_iter().next() {
                    return Some(s);
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            None
        };

        let a = wait(2).expect("第 2 格抽不出来");
        assert_eq!(a.w, W);
        assert!(a.h > 0 && a.h <= 120, "高度不合理：{}", a.h);
        assert_eq!(a.rgba.len(), (a.w * a.h * 4) as usize);
        assert!(a.rgba.chunks_exact(4).all(|px| px[3] == 255), "alpha 没补齐");

        let b = wait(20).expect("第 20 格抽不出来");
        assert_eq!(b.w, a.w);
        assert_ne!(a.rgba, b.rgba, "两个时间点抽出了一模一样的图，seek 八成没生效");

        // 亮不亮：全黑（或几乎全黑）说明渲染上下文根本没出画面
        let mean = |s: &Shot| {
            let n = s.rgba.chunks_exact(4).count().max(1);
            let sum: u64 = s.rgba.chunks_exact(4).map(|p| p[0] as u64 + p[1] as u64 + p[2] as u64).sum();
            sum as f64 / (n * 3) as f64
        };
        eprintln!("bucket2 mean={:.1} bucket20 mean={:.1}", mean(&a), mean(&b));
        // 不能拿「亮不亮」当合格线：暗场画面本来就接近黑，但不该是空底
        assert!(!is_blank(&a.rgba), "第 2 格是空底（mpv 没画出画面）");
        assert!(!is_blank(&b.rgba), "第 20 格是空底（mpv 没画出画面）");
    }
}
