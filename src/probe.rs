//! 后台时长探测：播放列表里每一行末尾的「12:34」就是它算出来的。
//!
//! 为什么不在扫目录时算：能解容器的只有 libmpv，几百个文件挨个真读一遍文件头，
//! 卡的正好是点下「打开文件夹」的那一下。所以单开一条线程、一个**独立的 mpv 实例**
//! （`vo=null` / `ao=null` / `pause=yes`），一条一条读 `duration`，读到就报给 UI，
//! UI 再按路径把时长填进列表。
//!
//! 用独立实例是因为主播放器正拿着另一个 ctx 放片子，不能让它去开别的文件。
//! 探测线程完全是「尽力而为」：读不到（网络流、损坏文件、没权限）就报 None，
//! 界面显示 `--:--`，不影响任何播放行为。

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, TryRecvError, channel};
use std::time::{Duration, Instant};

use crate::mpv::Mpv;

/// 单个文件最多等多久。大文件 / 网络盘上 open 会慢，给足 6 秒；
/// 超时就放弃（界面保持 `--:--`），绝不把整条队列卡死。
const TIMEOUT: Duration = Duration::from_secs(6);
/// 轮询间隔：容器头解出来通常只要几十毫秒
const POLL: Duration = Duration::from_millis(15);
/// 队列空了之后的睡眠间隔（别空转烧 CPU）
const IDLE: Duration = Duration::from_millis(120);

pub struct Prober {
    tx: Sender<Vec<PathBuf>>,
    rx: Receiver<(PathBuf, Option<f64>)>,
    stop: Arc<AtomicBool>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl Prober {
    pub fn spawn() -> Result<Self, String> {
        let (tx, cmd_rx) = channel::<Vec<PathBuf>>();
        let (out_tx, out_rx) = channel::<(PathBuf, Option<f64>)>();
        let stop = Arc::new(AtomicBool::new(false));

        let flag = stop.clone();
        let handle = std::thread::Builder::new()
            .name(String::from("maneki-probe"))
            .spawn(move || run(cmd_rx, out_tx, flag))
            .map_err(|e| format!("无法创建时长探测线程：{e}"))?;

        Ok(Self {
            tx,
            rx: out_rx,
            stop,
            handle: Some(handle),
        })
    }

    /// 换一批要探测的文件。旧队列（连同还没读的那些）直接丢掉。
    pub fn queue(&self, paths: Vec<PathBuf>) {
        let _ = self.tx.send(paths);
    }

    /// 取走这一轮已经探到的结果，非阻塞。
    pub fn drain(&self) -> Vec<(PathBuf, Option<f64>)> {
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

impl Drop for Prober {
    fn drop(&mut self) {
        // 线程可能正卡在等某个文件的 timeout 上，靠这个标记让它尽快返回
        self.stop.store(true, Ordering::Relaxed);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

fn run(
    cmd_rx: Receiver<Vec<PathBuf>>,
    out_tx: Sender<(PathBuf, Option<f64>)>,
    stop: Arc<AtomicBool>,
) {
    let mut mpv: Option<Mpv> = None;
    let mut queue: Vec<PathBuf> = Vec::new();
    let mut next: usize = 0;

    loop {
        if stop.load(Ordering::Relaxed) {
            return;
        }

        // 收新队列：新的一批直接顶替旧的（用户又拖了个文件夹进来）
        match cmd_rx.try_recv() {
            Ok(list) => {
                queue = list;
                next = 0;
            }
            Err(TryRecvError::Disconnected) => return,
            Err(TryRecvError::Empty) => {}
        }

        if next >= queue.len() {
            queue.clear();
            next = 0;
            std::thread::sleep(IDLE);
            continue;
        }

        // 所有 mpv 相关操作都留在这一条线程里，和播放线程互不影响
        if mpv.is_none() {
            match Mpv::probe() {
                Ok(m) => mpv = Some(m),
                Err(_) => {
                    // 起不来（DLL 没了）就不要空转重试
                    std::thread::sleep(Duration::from_secs(1));
                    queue.clear();
                    continue;
                }
            }
        }

        let path = std::mem::take(&mut queue[next]);
        next += 1;

        if path.as_os_str().is_empty() {
            continue;
        }

        let dur = mpv.as_ref().and_then(|m| read_duration(m, &path, &stop));
        if out_tx.send((path, dur)).is_err() {
            return;
        }
    }
}

/// 打开一个文件，等它的 `duration` 出现。
///
/// 关键是先确认 `path` 属性已经切到目标文件：`loadfile` 是异步的，
/// 切过去之前读到的 `duration` 还是**上一个文件**的，那样列表会成片显示错时长。
fn read_duration(mpv: &Mpv, path: &std::path::Path, stop: &AtomicBool) -> Option<f64> {
    let target = path.to_string_lossy().into_owned();
    if mpv.load(&target).is_err() {
        return None;
    }

    // 丢掉上一个文件残留的事件（end-file 之类）
    let _ = mpv.pump_events();

    let mut result = None;
    let start = Instant::now();

    while start.elapsed() < TIMEOUT {
        if stop.load(Ordering::Relaxed) {
            break;
        }
        if current_path_is(mpv, &target) {
            if let Some(d) = mpv.get_f64("duration") {
                if d > 0.0 && d.is_finite() {
                    result = Some(d);
                    break;
                }
            }
        }
        std::thread::sleep(POLL);
    }

    mpv.stop();
    result
}

fn current_path_is(mpv: &Mpv, target: &str) -> bool {
    let Some(cur) = mpv.get_string("path") else {
        return false;
    };
    // mpv 会把路径规范化（分隔符、盘符大小写），别用 == 直接比
    cur.replace('/', "\\").to_ascii_lowercase() == target.replace('/', "\\").to_ascii_lowercase()
}
