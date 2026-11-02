// 实测 mpv 的 speed 属性：
//   1. 原速 1.0 时 2 秒里 time-pos 前进多少
//   2. 设成 2.0 之后同样 2 秒前进多少（应当是两倍）
//   3. **重新 loadfile 之后** speed 会不会被 mpv 复位（决定连播是否必须重新下发）
//
//   cargo run --release --example speed -- <a.mp4> <b.mp4>
#[path = "../src/mpv.rs"]
#[allow(dead_code)]
mod mpv;

use std::time::{Duration, Instant};

fn run_for(m: &mpv::Mpv, secs: f64) -> f64 {
    // 和 player.rs 一样：不停地排空事件 + 出帧，mpv 才会真的往下播
    let mut buf = Vec::new();
    let start = Instant::now();
    let from = m.get_f64("time-pos").unwrap_or(0.0);
    while start.elapsed().as_secs_f64() < secs {
        let _ = m.pump_events();
        let _ = m.needs_render();
        let w = m.get_i64("dwidth").unwrap_or(0).max(2) as u32;
        let h = m.get_i64("dheight").unwrap_or(0).max(2) as u32;
        let _ = m.render_into(&mut buf, w.min(1280), h.min(720));
        std::thread::sleep(Duration::from_millis(4));
    }
    m.get_f64("time-pos").unwrap_or(0.0) - from
}

fn main() {
    let a = std::env::args().nth(1).expect("usage: speed <a> <b>");
    let b = std::env::args().nth(2).expect("usage: speed <a> <b>");

    let m = mpv::Mpv::new().expect("mpv");
    m.load(&a).expect("load a");
    std::thread::sleep(Duration::from_millis(600));

    let d1 = run_for(&m, 2.0);
    println!("speed=1.0  2s 内前进 {d1:.2}s");

    m.set_speed(2.0);
    std::thread::sleep(Duration::from_millis(120));
    let d2 = run_for(&m, 2.0);
    println!("speed=2.0  2s 内前进 {d2:.2}s   (倍数 {:.2})", d2 / 2.0);

    // 换文件：模拟自动连播
    m.load(&b).expect("load b");
    std::thread::sleep(Duration::from_millis(600));
    let d3 = run_for(&m, 1.0);
    println!("换片后（未重新下发） 1s 内前进 {d3:.2}s   读到 speed = {:?}", m.get_f64("speed"));

    // 我们 player.rs 的做法：首帧之后重新下发一次
    m.set_speed(2.0);
    std::thread::sleep(Duration::from_millis(120));
    let d4 = run_for(&m, 1.0);
    println!("换片后（重新下发 2.0） 1s 内前进 {d4:.2}s");
}
