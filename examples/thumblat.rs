// 量「用户真正感觉到」的延迟：从 `want()` 到第一张图到手。
//
//   cargo run --release --example thumblat -- <视频> [模式]
//
// 模式：
//   单点（默认）：一次要一格，量出第一张 / 精修那张各自多久
//   扫动       ：像用户那样每 120ms 挪一格，量每一格多久才出图
#[path = "../src/mpv.rs"]
#[allow(dead_code)]
mod mpv;
#[path = "../src/thumbs.rs"]
#[allow(dead_code)]
mod thumbs;

use std::time::{Duration, Instant};

fn mean(rgba: &[u8]) -> f64 {
    let n = rgba.chunks_exact(4).count().max(1);
    let sum: u64 = rgba
        .chunks_exact(4)
        .map(|p| p[0] as u64 + p[1] as u64 + p[2] as u64)
        .sum();
    sum as f64 / (n * 3) as f64
}

fn main() {
    let mut args = std::env::args().skip(1);
    let path = args.next().expect("usage: thumblat <视频> [扫动]");
    let sweep = args.next().as_deref() == Some("扫动");
    let path = std::path::PathBuf::from(path);

    let engine = thumbs::Thumbs::spawn().expect("起不了抽帧线程");

    // 先打开文件：第一次 want 会顺带 load，把它排除在计时之外
    engine.want(&path, 0);
    let open = Instant::now();
    while open.elapsed() < Duration::from_secs(10) && engine.drain().is_empty() {
        std::thread::sleep(Duration::from_millis(10));
    }
    eprintln!("（预热完成 {}ms）", open.elapsed().as_millis());

    if sweep {
        // 每 120ms 挪一格（就是界面上 THUMB_DWELL 的节奏）
        let buckets: Vec<i64> = (100..112).collect();
        let mut pending: Vec<(i64, Instant, Option<u128>, Option<u128>)> = Vec::new();
        for b in &buckets {
            engine.want(&path, *b);
            pending.push((*b, Instant::now(), None, None));
            let until = Instant::now() + Duration::from_millis(120);
            while Instant::now() < until {
                for shot in engine.drain() {
                    for e in pending.iter_mut() {
                        if e.0 == shot.key.1 {
                            let ms = e.1.elapsed().as_millis();
                            if e.2.is_none() {
                                e.2 = Some(ms);
                            } else if e.3.is_none() {
                                e.3 = Some(ms);
                            }
                        }
                    }
                }
                std::thread::sleep(Duration::from_millis(4));
            }
        }
        // 手停了，再等 3 秒把剩下的收完
        let tail = Instant::now() + Duration::from_secs(3);
        while Instant::now() < tail {
            for shot in engine.drain() {
                for e in pending.iter_mut() {
                    if e.0 == shot.key.1 {
                        let ms = e.1.elapsed().as_millis();
                        if e.2.is_none() {
                            e.2 = Some(ms);
                        } else if e.3.is_none() {
                            e.3 = Some(ms);
                        }
                    }
                }
            }
            std::thread::sleep(Duration::from_millis(4));
        }
        for (b, _, first, exact) in &pending {
            println!(
                "格 {b:<5} 第一张 {:>6}  精修 {:>6}",
                first.map(|x| format!("{x}ms")).unwrap_or_else(|| "—".into()),
                exact.map(|x| format!("{x}ms")).unwrap_or_else(|| "—".into()),
            );
        }
    } else {
        for b in [2i64, 22, 60, 150, 450] {
            let t0 = Instant::now();
            engine.want(&path, b);
            let mut first = None;
            let mut second = None;
            while t0.elapsed() < Duration::from_secs(6) && second.is_none() {
                for shot in engine.drain() {
                    if shot.key.1 != b {
                        continue;
                    }
                    let ms = t0.elapsed().as_millis();
                    let m = mean(&shot.rgba);
                    if first.is_none() {
                        first = Some(ms);
                        println!("  格 {b:<4} 第一张 {ms:>5}ms 亮度 {m:.1}");
                    } else {
                        second = Some(ms);
                        println!("  格 {b:<4} 精修 {ms:>5}ms 亮度 {m:.1}");
                    }
                }
                std::thread::sleep(Duration::from_millis(2));
            }
        }
    }
}
