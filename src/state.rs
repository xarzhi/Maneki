//! 本地持久化：常用配置 + 「这个文件看到哪儿了」。
//!
//! 存成一个 JSON 文件，默认在 `%APPDATA%\Maneki\state.json`（`QP_STATE_DIR`
//! 可以改掉所在目录 —— 自动化验收时指到临时目录，免得把用户真实的配置和播放
//! 进度冲掉）。
//!
//! 三条硬规则：
//!
//! 1. **读不回来就退回默认值，绝不 panic**。这个文件是给用户攒出来的顺手，
//!    不是关键数据；一个手改坏的 JSON 不该让播放器起不来。
//! 2. **写盘用「先写 .tmp 再 rename」**。直接覆盖写，断电/崩溃时正好写一半，
//!    下次启动就是半截 JSON —— 所有设置连同播放记录一起丢。
//! 3. **播放进度按「路径 + 大小 + 修改时间」认人**。只按路径存，同一个名字的
//!    另一部片子（换了一集、重下一遍）会继承上一条的进度，跳到莫名其妙的地方。

use crate::video::{Aspect, FitMode, Look};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

/// 一条播放记录
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Progress {
    /// 看到第几秒
    pub pos: f64,
    /// 这条片子的总长（用来判断「是不是已经看完了」）
    pub dur: f64,
}

impl Progress {
    /// 还值得续播吗？
    ///
    /// 三个条件都不满足才算「看完了 / 没什么可续的」：
    /// 位置太靠前（刚开头，续播没意义）、已经到结尾 5 秒以内、时长未知。
    pub fn resumable(&self, now_dur: f64) -> bool {
        let dur = if now_dur > 0.0 { now_dur } else { self.dur };
        self.pos > 3.0 && dur > 0.0 && self.pos < dur - 5.0
    }
}

#[derive(Debug, Clone)]
pub struct State {
    pub volume: f64,
    pub speed: f64,
    /// 上次退出时是全屏，下次进来还原
    pub fullscreen: bool,
    pub fit: FitMode,
    pub aspect: Aspect,
    pub look: Look,
    pub autoplay: bool,
    /// 路径键 → 播放记录
    pub progress: HashMap<String, Progress>,
    /// 最近打开过的文件（最新的在前，最多 `RECENT_MAX` 条）
    pub recent: Vec<String>,
}

/// 最近列表上限。再多也没人往下翻，还让文件越写越大。
pub const RECENT_MAX: usize = 24;

impl Default for State {
    fn default() -> Self {
        Self {
            // 和 `App::new` 里的出厂值保持一致：改这里要同步改那边
            volume: 80.0,
            speed: 1.0,
            fullscreen: false,
            fit: FitMode::Fit,
            aspect: Aspect::Auto,
            look: Look::default(),
            autoplay: false,
            progress: HashMap::new(),
            recent: Vec::new(),
        }
    }
}

/// 状态文件所在目录：`QP_STATE_DIR` 优先，否则 `%APPDATA%\Maneki`。
pub fn dir() -> PathBuf {
    if let Some(d) = std::env::var_os("QP_STATE_DIR") {
        return PathBuf::from(d);
    }
    let base = std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    base.join("Maneki")
}

fn file() -> PathBuf {
    dir().join("state.json")
}

/// 文件的身份：路径 + 大小 + 修改时间。
///
/// 拿不到元信息（路径已经不存在、权限不够）时退回纯路径 —— 总比什么都不记强。
pub fn key(path: &Path) -> String {
    let p = path.to_string_lossy();
    let Ok(m) = std::fs::metadata(path) else {
        return p.into_owned();
    };
    let mtime = m
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!("{p}|{}|{mtime}", m.len())
}

impl State {
    /// 读状态文件。任何问题都退回默认值，不报错、不打断启动。
    pub fn load() -> Self {
        let Ok(text) = std::fs::read_to_string(file()) else {
            return Self::default();
        };
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) else {
            return Self::default();
        };
        Self::from_json(&v)
    }

    fn from_json(v: &serde_json::Value) -> Self {
        let mut s = Self::default();
        let num = |k: &str| v.get(k).and_then(|x| x.as_f64());
        let flag = |k: &str| v.get(k).and_then(|x| x.as_bool());

        if let Some(x) = num("volume") {
            s.volume = x.clamp(0.0, 100.0);
        }
        if let Some(x) = num("speed") {
            s.speed = x.clamp(0.1, 8.0);
        }
        if let Some(x) = flag("fullscreen") {
            s.fullscreen = x;
        }
        if let Some(x) = flag("autoplay") {
            s.autoplay = x;
        }
        if let Some(x) = v.get("fit").and_then(|x| x.as_str()) {
            s.fit = fit_from_id(x);
        }
        if let Some(x) = v.get("aspect").and_then(|x| x.as_str()) {
            s.aspect = aspect_from_id(x);
        }
        if let Some(l) = v.get("look") {
            let f = |k: &str, cur: f32| {
                l.get(k)
                    .and_then(|x| x.as_f64())
                    .map(|x| x as f32)
                    .unwrap_or(cur)
            };
            s.look = Look {
                brightness: f("brightness", 0.0).clamp(-100.0, 100.0),
                contrast: f("contrast", 0.0).clamp(-100.0, 100.0),
                saturation: f("saturation", 0.0).clamp(-100.0, 100.0),
                hue: f("hue", 0.0).clamp(-100.0, 100.0),
                sharpen: f("sharpen", 0.0).clamp(0.0, 100.0),
            };
        }
        if let Some(map) = v.get("progress").and_then(|x| x.as_object()) {
            for (k, item) in map {
                let pos = item.get("pos").and_then(|x| x.as_f64()).unwrap_or(0.0);
                let dur = item.get("dur").and_then(|x| x.as_f64()).unwrap_or(0.0);
                if pos > 0.0 {
                    s.progress.insert(k.clone(), Progress { pos, dur });
                }
            }
        }
        if let Some(list) = v.get("recent").and_then(|x| x.as_array()) {
            s.recent = list
                .iter()
                .filter_map(|x| x.as_str().map(str::to_string))
                .take(RECENT_MAX)
                .collect();
        }
        s
    }

    pub fn to_json(&self) -> serde_json::Value {
        let mut progress = serde_json::Map::new();
        for (k, p) in &self.progress {
            progress.insert(
                k.clone(),
                serde_json::json!({ "pos": p.pos, "dur": p.dur }),
            );
        }

        serde_json::json!({
            "version": 1,
            "volume": self.volume,
            "speed": self.speed,
            "fullscreen": self.fullscreen,
            "fit": fit_id(self.fit),
            "aspect": aspect_id(self.aspect),
            "look": {
                "brightness": self.look.brightness,
                "contrast": self.look.contrast,
                "saturation": self.look.saturation,
                "hue": self.look.hue,
                "sharpen": self.look.sharpen,
            },
            "autoplay": self.autoplay,
            "progress": progress,
            "recent": self.recent,
        })
    }

    /// 落盘。失败只是本次没存上，不要把错误弹给用户 —— 播放器还得继续用。
    pub fn save(&self) -> std::io::Result<()> {
        let dir = dir();
        std::fs::create_dir_all(&dir)?;
        let final_path = file();
        let tmp = dir.join("state.json.tmp");

        let text = serde_json::to_string_pretty(&self.to_json())
            .map_err(|e| std::io::Error::other(e.to_string()))?;
        std::fs::write(&tmp, text)?;
        // rename 在同一分区上是原子的：要么是旧的完整文件，要么是新的完整文件，
        // 不会出现「写了一半」的中间态。
        std::fs::rename(&tmp, &final_path)?;
        Ok(())
    }

    /// 记一条「最近打开」
    pub fn remember(&mut self, path: &Path) {
        let s = path.to_string_lossy().into_owned();
        self.recent.retain(|x| x != &s);
        self.recent.insert(0, s);
        self.recent.truncate(RECENT_MAX);
    }

    /// 空界面上那几条「最近播放」，最新的在前。
    ///
    /// 顺手把已经不存在的文件筛掉：用户经常下完就看、看完就删，
    /// 留一堆点了没反应的死条目比不显示还糟。
    pub fn recent_files(&self, limit: usize) -> Vec<(PathBuf, Option<Progress>)> {
        self.recent
            .iter()
            .map(PathBuf::from)
            .filter(|p| p.is_file())
            .take(limit)
            .map(|p| {
                let at = self.progress_of(&p);
                (p, at)
            })
            .collect()
    }

    /// 记下看到哪儿了。返回 true 表示「值得写盘」——
    /// 每帧都写会把磁盘和 UI 一起拖死，所以只有位置挪了 5 秒以上才算数。
    pub fn note_position(&mut self, path: &Path, pos: f64, dur: f64) -> bool {
        if pos <= 0.0 {
            return false;
        }
        let k = key(path);
        let slot = self.progress.entry(k).or_default();
        let moved = (pos - slot.pos).abs() >= 5.0;
        let dur_changed = (dur - slot.dur).abs() >= 1.0;
        if moved || dur_changed {
            slot.pos = pos;
            slot.dur = dur;
            return true;
        }
        false
    }

    pub fn progress_of(&self, path: &Path) -> Option<Progress> {
        self.progress.get(&key(path)).copied()
    }

    /// 一条片子看完 / 用户主动从头开始：把记录清掉，下次不再问「要不要续播」
    pub fn forget_position(&mut self, path: &Path) {
        self.progress.remove(&key(path));
    }
}

fn fit_id(f: FitMode) -> &'static str {
    match f {
        FitMode::Fit => "fit",
        FitMode::Cover => "cover",
        FitMode::Stretch => "stretch",
    }
}

fn fit_from_id(s: &str) -> FitMode {
    match s {
        "cover" => FitMode::Cover,
        "stretch" => FitMode::Stretch,
        _ => FitMode::Fit,
    }
}

fn aspect_id(a: Aspect) -> &'static str {
    match a {
        Aspect::Auto => "auto",
        Aspect::R16x9 => "16:9",
        Aspect::R4x3 => "4:3",
        Aspect::R5x4 => "5:4",
        Aspect::R21x9 => "21:9",
    }
}

fn aspect_from_id(s: &str) -> Aspect {
    match s {
        "16:9" => Aspect::R16x9,
        "4:3" => Aspect::R4x3,
        "5:4" => Aspect::R5x4,
        "21:9" => Aspect::R21x9,
        _ => Aspect::Auto,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("maneki_state_{tag}_{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// 存 → 读 一圈，所有字段都要原样回来
    #[test]
    fn round_trips_through_json() {
        let mut s = State::default();
        s.volume = 42.0;
        s.speed = 1.5;
        s.fullscreen = true;
        s.fit = FitMode::Cover;
        s.aspect = Aspect::R21x9;
        s.look.brightness = 12.0;
        s.look.sharpen = 30.0;
        s.autoplay = true;
        s.progress.insert(
            "D:/a.mkv|10|20".into(),
            Progress { pos: 61.5, dur: 600.0 },
        );
        s.recent = vec!["D:/a.mkv".into(), "D:/b.mp4".into()];

        let back = State::from_json(&s.to_json());
        assert_eq!(back.volume, 42.0);
        assert_eq!(back.speed, 1.5);
        assert!(back.fullscreen);
        assert_eq!(back.fit, FitMode::Cover);
        assert_eq!(back.aspect, Aspect::R21x9);
        assert_eq!(back.look.brightness, 12.0);
        assert_eq!(back.look.sharpen, 30.0);
        assert!(back.autoplay);
        assert_eq!(
            back.progress.get("D:/a.mkv|10|20"),
            Some(&Progress { pos: 61.5, dur: 600.0 })
        );
        assert_eq!(back.recent, vec!["D:/a.mkv", "D:/b.mp4"]);
    }

    /// 坏 JSON / 缺字段 / 类型不对：一律退回默认值，不能 panic
    #[test]
    fn garbage_falls_back_to_defaults() {
        for bad in [
            serde_json::json!({}),
            serde_json::json!({ "volume": "很大" }),
            serde_json::json!({ "volume": 1e9, "speed": -3.0 }),
            serde_json::json!({ "progress": 7 }),
            serde_json::json!(null),
        ] {
            let s = State::from_json(&bad);
            assert!((0.0..=100.0).contains(&s.volume), "音量必须夹在 0~100");
            assert!((0.1..=8.0).contains(&s.speed), "倍速必须夹在 0.1~8");
            assert!(s.progress.is_empty() || s.progress.len() == 1);
        }
    }

    /// 「看完了」和「刚开头」都不该续播
    #[test]
    fn resume_only_when_it_makes_sense() {
        let at = |pos: f64, dur: f64| Progress { pos, dur };
        assert!(at(60.0, 600.0).resumable(600.0));
        assert!(!at(1.0, 600.0).resumable(600.0), "刚看两秒不值得续");
        assert!(!at(598.0, 600.0).resumable(600.0), "已经到结尾了");
        assert!(!at(60.0, 0.0).resumable(0.0), "时长未知时不要瞎跳");
        // 实时时长优先于存档里的旧时长
        assert!(!at(60.0, 600.0).resumable(61.0));
    }

    /// 真的落盘再读回来（走一遍文件系统，验证 .tmp + rename）
    #[test]
    fn saves_and_loads_from_disk() {
        let d = tmp_dir("disk");
        // SAFETY: 测试进程里只有这条用例碰这个环境变量
        unsafe { std::env::set_var("QP_STATE_DIR", &d) };

        let mut s = State::default();
        s.volume = 33.0;
        s.save().unwrap();
        assert!(d.join("state.json").is_file(), "应该写出 state.json");
        assert!(!d.join("state.json.tmp").exists(), "临时文件必须被 rename 掉");

        let back = State::load();
        assert_eq!(back.volume, 33.0);

        // 手写坏文件：不 panic，退回默认值
        std::fs::write(d.join("state.json"), "{ 这不是 json").unwrap();
        assert_eq!(State::load().volume, State::default().volume);

        unsafe { std::env::remove_var("QP_STATE_DIR") };
        let _ = std::fs::remove_dir_all(&d);
    }

    /// 位置没挪够就不该标记「需要写盘」，否则每帧都会写一次磁盘
    #[test]
    fn position_is_throttled() {
        let mut s = State::default();
        let p = Path::new("D:/x.mp4");
        assert!(!s.note_position(p, 0.0, 100.0), "0 秒不记");
        assert!(s.note_position(p, 10.0, 100.0), "第一次记");
        assert!(!s.note_position(p, 12.0, 100.0), "才挪 2 秒，别写盘");
        assert!(s.note_position(p, 16.0, 100.0), "挪了 6 秒，该写");
        assert_eq!(s.progress_of(p).map(|x| x.pos), Some(16.0));
        s.forget_position(p);
        assert!(s.progress_of(p).is_none());
    }

    /// 同一个路径、不同文件（大小/时间变了）不能共用进度
    #[test]
    fn key_distinguishes_different_files() {
        let d = tmp_dir("key");
        let f = d.join("a.mp4");
        std::fs::write(&f, b"1").unwrap();
        let k1 = key(&f);
        std::fs::write(&f, b"123456").unwrap();
        let k2 = key(&f);
        assert_ne!(k1, k2, "文件变了，键也必须变，否则会继承别人看到的进度");
        let _ = std::fs::remove_dir_all(&d);
    }
}
