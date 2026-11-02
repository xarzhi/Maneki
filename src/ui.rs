//! 几个跨面板复用的小工具（播放列表 / 设置 / 字幕 popover 都要用）。

/// 按像素宽度截断，末尾补省略号。
///
/// iced 的 `text` 没有 ellipsis 能力：`Wrapping::None` 时既不裁切也不加点点，
/// 超长文件名会直接压到右边的时长上。这里按字符粗量一遍宽度 ——
/// 全角字符算 1em、ASCII 算 0.56em（雅黑 / Segoe UI 实测差不多是这个比例），
/// 再留 4% 余量，免得真的顶出去。
pub fn elide(s: &str, max_px: f32, size: f32) -> String {
    let budget = max_px.max(8.0) * 0.96;
    let mut width = 0.0f32;
    let mut out = String::with_capacity(s.len());

    for ch in s.chars() {
        let w = char_w(ch, size);
        if width + w > budget {
            let dots = size * 0.9;
            while !out.is_empty() && width + dots > budget {
                let last = out.pop().unwrap_or(' ');
                width -= char_w(last, size);
            }
            out.push('…');
            return out;
        }
        width += w;
        out.push(ch);
    }
    out
}

fn char_w(ch: char, size: f32) -> f32 {
    if (ch as u32) < 0x2E80 {
        size * 0.56
    } else {
        size
    }
}

/// 时长文本。还没拿到（没探测到 / 流媒体）就占位成 `--:--`，
/// 不要显示 00:00 —— 那是在说「零秒」。
pub fn fmt_dur(d: Option<f64>) -> String {
    match d {
        Some(v) if v > 0.0 && v.is_finite() => {
            let s = v.round() as u64;
            let (h, m, sec) = (s / 3600, (s % 3600) / 60, s % 60);
            if h > 0 {
                format!("{h}:{m:02}:{sec:02}")
            } else {
                format!("{m:02}:{sec:02}")
            }
        }
        _ => String::from("--:--"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn elide_keeps_short_names() {
        assert_eq!(elide("第1集.mp4", 200.0, 14.0), "第1集.mp4");
    }

    #[test]
    fn elide_cuts_long_names() {
        let long = "这是一个特别特别长的文件名用来测试截断逻辑到底有没有生效.mp4";
        let cut = elide(long, 120.0, 14.0);
        assert!(cut.ends_with('…'));
        assert!(cut.chars().count() < long.chars().count());
    }

    #[test]
    fn duration_formatting() {
        assert_eq!(fmt_dur(None), "--:--");
        assert_eq!(fmt_dur(Some(0.0)), "--:--");
        assert_eq!(fmt_dur(Some(75.0)), "01:15");
        assert_eq!(fmt_dur(Some(3725.0)), "1:02:05");
    }
}
