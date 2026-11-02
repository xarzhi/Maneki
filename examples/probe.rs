//! 无 UI 探针：单独验证 libmpv 的软件渲染是否真的出画。
//! 用法：cargo run --release --example probe -- assets/sample.mp4
//!
//! 放在 examples/ 而不是 src/bin/：它只是开发期的诊断工具，
//! 不该被打进安装包（cargo packager 只收 bin，不收 examples）。

// 整个 mpv.rs 都拉进来只用其中一部分（播放相关的方法这里用不上），
// 没被用到的那几个会报 dead_code，压掉就行
#[path = "../src/mpv.rs"]
#[allow(dead_code)]
mod mpv;

use std::time::{Duration, Instant};

// ---------------- Windows 进程 I/O 计数（用来证明"打开只读了头部"） ----------------
#[repr(C)]
#[derive(Default)]
struct IoCounters {
    read_operations: u64,
    write_operations: u64,
    other_operations: u64,
    read_transfer: u64,
    write_transfer: u64,
    other_transfer: u64,
}

unsafe extern "system" {
    fn GetCurrentProcess() -> *mut std::ffi::c_void;
    fn GetProcessIoCounters(handle: *mut std::ffi::c_void, counters: *mut IoCounters) -> i32;
}

/// 进程累计从磁盘读到的字节数
fn bytes_read() -> u64 {
    let mut c = IoCounters::default();
    unsafe {
        if GetProcessIoCounters(GetCurrentProcess(), &mut c) != 0 {
            c.read_transfer
        } else {
            0
        }
    }
}

fn main() {
    let path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| String::from("assets/sample.mp4"));

    let t_init = Instant::now();
    let mpv = match mpv::Mpv::new() {
        Ok(m) => m,
        Err(e) => {
            println!("初始化失败: {e}");
            return;
        }
    };

    println!("引擎初始化耗时: {:.0} ms", t_init.elapsed().as_secs_f64()*1000.0);
    let io_before = bytes_read();
    let t0 = Instant::now();
    if let Err(e) = mpv.load(&path) {
        println!("加载失败: {e}");
        return;
    }

    let mut buf: Vec<u8> = Vec::new();
    let mut rendered = 0;
    let mut first_frame_ms: Option<f64> = None;
    let mut last_mean = 0.0f64;

    for i in 0..300 {
        std::thread::sleep(Duration::from_millis(10));
        let outcome = mpv.pump_events();
        if let Some(e) = outcome.error {
            println!("mpv 报错: {e}");
        }

        let dw = mpv.get_i64("dwidth").unwrap_or(0);
        let dh = mpv.get_i64("dheight").unwrap_or(0);
        let pos = mpv.get_f64("time-pos").unwrap_or(-1.0);
        let paused = mpv.get_string("pause").unwrap_or_else(|| "?".into());
        let idle = mpv.get_string("idle-active").unwrap_or_else(|| "?".into());

        if dw > 1 && dh > 1 && mpv.needs_render() {
            let (w, h) = (dw as u32, dh as u32);
            if mpv.render_into(&mut buf, w, h) {
                rendered += 1;
                if first_frame_ms.is_none() {
                    first_frame_ms = Some(t0.elapsed().as_secs_f64() * 1000.0);
                }
                // 统计像素亮度，判断是不是纯黑
                let mut sum: u64 = 0;
                let mut nonblack = 0usize;
                let n = buf.len() / 4;
                for p in 0..n {
                    let r = buf[p * 4] as u64;
                    let g = buf[p * 4 + 1] as u64;
                    let b = buf[p * 4 + 2] as u64;
                    sum += r + g + b;
                    if r + g + b > 24 {
                        nonblack += 1;
                    }
                }
                let mean = sum as f64 / (n as f64 * 3.0);
                last_mean = mean;
                if rendered <= 5 || i % 50 == 0 {
                    println!(
                        "[{:>4}ms] 渲染第{}帧 {}x{} 平均亮度={:.1} 非黑像素={:.1}% 进度={:.2}s",
                        (t0.elapsed().as_secs_f64() * 1000.0) as i64,
                        rendered,
                        w,
                        h,
                        mean,
                        nonblack as f64 / n as f64 * 100.0,
                        pos
                    );
                }
            } else {
                println!("[{i}] render_into 返回失败");
            }
        }

        if i % 50 == 0 {
            println!(
                "  └ 采样 {i}: dwidth={dw} dheight={dh} time-pos={pos:.2} pause={paused} idle={idle} 已渲染={rendered}"
            );
        }

        if rendered > 0 && i > 200 {
            break;
        }
    }

    println!("----------------------------------------");
    println!("首个有效帧耗时: {first_frame_ms:?} ms");
    println!("累计渲染帧数: {rendered}");
    println!("最后平均亮度: {last_mean:.1}（0=纯黑）");
    println!("时长: {:?} s", mpv.get_f64("duration"));

    // 文件体积 vs 实际读取量：这是 O(1) 的直接证据
    let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
    let read = bytes_read().saturating_sub(io_before);
    println!(
        "文件体积: {:.2} GB",
        size as f64 / 1024.0 / 1024.0 / 1024.0
    );
    println!(
        "打开+播放期间实际读盘: {:.2} MB（占文件 {:.4}%）",
        read as f64 / 1024.0 / 1024.0,
        if size > 0 { read as f64 / size as f64 * 100.0 } else { 0.0 }
    );
}
