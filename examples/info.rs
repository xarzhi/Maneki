// 调试用：把候选的元信息属性全打一遍，看哪些真的拿得到。
//   cargo run --example info -- "<视频路径>"
#[path = "../src/mpv.rs"]
#[allow(dead_code)]
mod mpv;

use std::time::Duration;

fn main() {
    let path = std::env::args().nth(1).expect("usage: info <file>");
    let m = mpv::Mpv::probe().expect("mpv");
    m.load(&path).expect("load");

    // 等它把容器解出来
    std::thread::sleep(Duration::from_millis(1200));

    let names = [
        "path",
        "file-format",
        "file-size",
        "duration",
        "dwidth",
        "dheight",
        "width",
        "height",
        "video-codec",
        "video-bitrate",
        "container-fps",
        "estimated-vf-fps",
        "audio-codec",
        "audio-bitrate",
        "audio-params/channel-count",
        "audio-params/sample-rate",
        "track-list/count",
    ];
    for n in names {
        println!("{n:32} = {:?}", m.get_string(n));
    }

    let count = m.get_i64("track-list/count").unwrap_or(0);
    for i in 0..count {
        for f in [
            "type",
            "demux-w",
            "demux-h",
            "demux-channel-count",
            "demux-samplerate",
            "demux-bitrate",
            "codec",
        ] {
            let k = format!("track-list/{i}/{f}");
            println!("{k:32} = {:?}", m.get_string(&k));
        }
    }
    m.stop();
}
