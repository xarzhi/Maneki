// 量一次「悬停缩略图」的抽帧耗时，并对比两种定位方式。
//
//   cargo run --release --example thumbbench -- <视频> [时间点...]
//
// 每种定位跑两遍：
//   关键帧：`seek absolute+keyframes` —— 只解一个 I 帧，快，但画面可能早一个 GOP
//   精确  ：`set time-pos`（hr-seek=yes）—— 从关键帧一路解到目标帧，准，但慢
//
// 出帧那一步刻意**不等**「有新帧」标志（那是 thumbs.rs 老写法里白等的 250ms），
// 直接画当前帧，画出来是空底才回头等。
#[path = "../src/mpv.rs"]
#[allow(dead_code)]
mod mpv;

use std::time::{Duration, Instant};

fn mean(buf: &[u8]) -> f64 {
    let n = buf.chunks_exact(4).count().max(1);
    let sum: u64 = buf
        .chunks_exact(4)
        .map(|p| p[0] as u64 + p[1] as u64 + p[2] as u64)
        .sum();
    sum as f64 / (n * 3) as f64
}

struct Shot {
    seek: Duration,
    render: Duration,
    total: Duration,
    lit: f64,
    pos: f64,
    retries: u32,
}

fn shot(m: &mpv::Mpv, t: f64, keyframes: bool, th: u32, buf: &mut Vec<u8>) -> Shot {
    shot_inner(m, t, keyframes, th, buf, 0)
}

/// `pump` = 等寻址的同时不停出帧。用来验证一件事：mpv 是不是要等外面
/// 把帧画走，才肯发 `playback-restart`。
fn shot_inner(
    m: &mpv::Mpv,
    t: f64,
    keyframes: bool,
    th: u32,
    buf: &mut Vec<u8>,
    pump: u8,
) -> Shot {
    let _ = m.pump_events();
    let t0 = Instant::now();
    if keyframes {
        m.seek_keyframes(t);
    } else {
        m.seek_absolute(t);
    }
    if pump == 2 {
        // 只在开头画一次，然后老实等事件
        let _ = m.render_into(buf, 160, th);
        wait_event(m);
    } else if pump == 1 {
        while t0.elapsed() < Duration::from_secs(8) {
            let _ = m.render_into(buf, 160, th);
            if m.see_playback_restart() {
                break;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    } else if pump == 3 {
        // 每 4ms 画一次（少一点缩放/色调映射的开销）
        while t0.elapsed() < Duration::from_secs(8) {
            let _ = m.render_into(buf, 160, th);
            if m.see_playback_restart() {
                break;
            }
            std::thread::sleep(Duration::from_millis(4));
        }
    } else {
        wait_event(m);
    }
    let seek = t0.elapsed();

    let t1 = Instant::now();
    let mut retries = 0;
    let mut lit = 0.0;
    loop {
        let ok = m.render_into(buf, 160, th);
        if ok {
            lit = mean(buf);
            if lit > 0.0 {
                break;
            }
        }
        retries += 1;
        if t1.elapsed() > Duration::from_secs(5) || retries > 40 {
            break;
        }
        // 空底：这回老实等「有新帧」标志
        let w = Instant::now();
        while !m.needs_render() && w.elapsed() < Duration::from_millis(120) {
            std::thread::sleep(Duration::from_millis(2));
        }
    }
    Shot {
        seek,
        render: t1.elapsed(),
        total: t0.elapsed(),
        lit,
        pos: m.get_f64("time-pos").unwrap_or(-1.0),
        retries,
    }
}

/// 干等事件，一帧都不画（老写法）。
fn wait_event(m: &mpv::Mpv) -> bool {
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(8) {
        let mut hit = false;
        for _ in 0..64 {
            match m.try_event() {
                0 => break,
                21 => hit = true,
                7 => return false,
                _ => {}
            }
        }
        if hit {
            return true;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    false
}

fn main() {
    let mut args = std::env::args().skip(1);
    let path = args.next().expect("usage: thumbbench <视频> [时间点...]");
    let times: Vec<f64> = if args.len() > 0 {
        args.map(|a| a.parse().expect("时间点要数字")).collect()
    } else {
        vec![5.0, 45.0, 120.0, 300.0, 900.0, 2700.0]
    };

    let m = mpv::Mpv::frames().expect("起不了抽帧实例");
    m.load(&path).expect("load");

    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(8) {
        if m.get_f64("duration").unwrap_or(0.0) > 0.0 {
            break;
        }
        let _ = m.pump_events();
        std::thread::sleep(Duration::from_millis(10));
    }
    let (vw, vh) = (
        m.get_i64("width").unwrap_or(0).max(2) as u32,
        m.get_i64("height").unwrap_or(0).max(2) as u32,
    );
    println!(
        "{}x{}  时长 {:?}  打开 {}ms  hwdec={:?}",
        vw,
        vh,
        m.get_f64("duration"),
        start.elapsed().as_millis(),
        m.get_string("hwdec-current"),
    );
    let th = (160u64 * vh as u64 / vw as u64).clamp(40, 120) as u32;
    let mut buf: Vec<u8> = Vec::new();

    for (label, keyframes, pump) in [
        ("精确（老写法）", false, 0),
        ("精确+每 1ms 画一次", false, 1),
        ("精确+开头画一次", false, 2),
        ("精确+每 4ms 画一次", false, 3),
        ("关键帧+每 1ms 画一次", true, 1),
    ] {
        println!("--- {label} ---");
        let mut totals = Vec::new();
        for t in &times {
            let s = shot_inner(&m, *t, keyframes, th, &mut buf, pump);
            totals.push(s.total.as_millis());
            println!(
                "t={:>7.1}  寻址 {:>5}ms  出帧 {:>4}ms  合计 {:>5}ms  亮度 {:>6.1}  pos={:>8.2}  重试 {}",
                t,
                s.seek.as_millis(),
                s.render.as_millis(),
                s.total.as_millis(),
                s.lit,
                s.pos,
                s.retries
            );
        }
        totals.sort();
        let n = totals.len();
        let sum: u128 = totals.iter().map(|x| *x as u128).sum();
        println!(
            "  平均 {}ms  中位 {}ms  最慢 {}ms\n",
            sum as usize / n.max(1),
            totals[n / 2],
            totals[n - 1]
        );
    }
}
