//! 快捷键：**键位表和键 → 消息的映射都在这一个文件里**。
//!
//! 设置页的「快捷键」标签直接遍历 [`GROUPS`] 渲染，所以文档和真实行为不会走散：
//! 加一个键，在 [`bind`] 里加一条、在 [`GROUPS`] 里补一行文字，两边一起改。
//! （以前这类东西最容易腐坏 —— 说明写着有、按下去没反应。）
//!
//! 约定：
//! - 用**逻辑键**（`key`）判断，不用 `modified_key`：按住 Ctrl 时后者会变成控制
//!   字符（Ctrl+O → 0x0F），拿它跟 'o' 比永远不相等。
//! - 会「切换状态」的键（暂停、全屏、静音、截图）只认第一次按下，按住不放不该
//!   疯狂翻面；连续的键（快进、调音量、调速）连 repeat 一起收，按住才有连续手感。

use crate::Message;
use iced::keyboard::key::Named;
use iced::keyboard::{Key, Modifiers};

/// 一条快捷键说明
pub struct Shortcut {
    /// 键位写法，给设置页显示用
    pub keys: &'static str,
    /// 干什么用的
    pub what: &'static str,
}

/// 设置页「快捷键」标签的内容，按用途分组。
///
/// 方向键这里故意写成「上 / 下」「左 / 右」而不是 `↑ ↓ ← →`：
/// 键位块里的字号是 12px，Segoe UI 的箭头在这个尺寸下只剩一条两像素的横线，
/// 箭头尖完全糊掉，用户看到的是「− / −」，根本分不出左右。
pub const GROUPS: &[(&str, &[Shortcut])] = &[
    (
        "播放",
        &[
            Shortcut {
                keys: "空格 / K",
                what: "播放 / 暂停",
            },
            Shortcut {
                keys: "M",
                what: "静音 / 取消静音",
            },
            Shortcut {
                keys: "上 / 下",
                what: "音量 +5 / -5",
            },
            Shortcut {
                keys: "[ / ]",
                what: "减速 / 加速",
            },
            Shortcut {
                keys: "S",
                what: "截一张图",
            },
        ],
    ),
    (
        "跳转",
        &[
            Shortcut {
                keys: "左 / 右",
                what: "快退 / 快进 5 秒",
            },
            Shortcut {
                keys: "J / L",
                what: "快退 / 快进 10 秒",
            },
            Shortcut {
                keys: "Ctrl + 左右",
                what: "上一集 / 下一集",
            },
        ],
    ),
    (
        "窗口",
        &[
            Shortcut {
                keys: "F",
                what: "全屏 / 退出全屏",
            },
            Shortcut {
                keys: "Ctrl + O",
                what: "打开文件",
            },
            Shortcut {
                keys: "Esc",
                what: "先关浮层，再退出全屏",
            },
        ],
    ),
];

/// 快退 / 快进一次的秒数（← / →）
pub const SEEK_STEP: f64 = 5.0;
/// J / L 跳的秒数。比方向键大一点，长片子里翻得动。
pub const JUMP_STEP: f64 = 10.0;

/// 把一个按键翻译成要发的消息。`None` = 这个键我们不管，交给别人。
pub fn bind(key: &Key, modifiers: Modifiers, repeat: bool) -> Option<Message> {
    let ctrl = modifiers.control();

    match key {
        // ---- 不论有没有 Ctrl 都成立的行为键 ----
        Key::Named(Named::Escape) => return Some(Message::Escape),

        // ---- Ctrl 组合 ----
        Key::Named(Named::ArrowLeft) if ctrl => return Some(Message::Prev),
        Key::Named(Named::ArrowRight) if ctrl => return Some(Message::Next),
        Key::Character(c) if ctrl && (c.as_str() == "o" || c.as_str() == "O") => {
            return Some(Message::OpenDialog);
        }

        // ---- 方向键：跳进度 / 调音量 ----
        //
        // 这几条**故意**连 repeat 一起收：按住 ← 应该一路快退，
        // 按住 ↑ 应该匀速升音量。进度那边有节流（见 `nudge`），不会把请求堆满。
        Key::Named(Named::ArrowRight) => return Some(Message::Nudge(SEEK_STEP)),
        Key::Named(Named::ArrowLeft) => return Some(Message::Nudge(-SEEK_STEP)),
        Key::Named(Named::ArrowUp) => return Some(Message::NudgeVolume(crate::VOL_STEP)),
        Key::Named(Named::ArrowDown) => {
            return Some(Message::NudgeVolume(-crate::VOL_STEP));
        }

        // ---- 字母键 ----
        Key::Character(c) => {
            // 按住 Ctrl 时不再把字母当单键用（否则 Ctrl+O 会连带触发 O 的手感问题）
            if ctrl {
                return None;
            }
            let lower = c.to_lowercase();
            return match lower.as_str() {
                // 会切换状态的键：只认第一次按下
                "k" if !repeat => Some(Message::TogglePlay),
                "m" if !repeat => Some(Message::ToggleMute),
                "f" if !repeat => Some(Message::ToggleFullscreen),
                "s" if !repeat => Some(Message::Screenshot),
                "o" if !repeat => Some(Message::OpenDialog),
                // 连续的键：按住要一直走
                "j" => Some(Message::Nudge(-JUMP_STEP)),
                "l" => Some(Message::Nudge(JUMP_STEP)),
                "[" => Some(Message::SpeedStep(-1)),
                "]" => Some(Message::SpeedStep(1)),
                _ => None,
            };
        }

        // ---- 空格 ----
        Key::Named(Named::Space) if !repeat => Some(Message::TogglePlay),

        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ch(c: &str) -> Key {
        Key::Character(c.into())
    }

    fn plain() -> Modifiers {
        Modifiers::default()
    }

    fn ctrl() -> Modifiers {
        Modifiers::CTRL
    }

    /// 主键位都在：空格、K、M、F、S、J、L、[、]
    #[test]
    fn common_keys_are_bound() {
        assert!(matches!(
            bind(&Key::Named(Named::Space), plain(), false),
            Some(Message::TogglePlay)
        ));
        assert!(matches!(
            bind(&ch("k"), plain(), false),
            Some(Message::TogglePlay)
        ));
        assert!(matches!(
            bind(&ch("K"), plain(), false),
            Some(Message::TogglePlay)
        ));
        assert!(matches!(
            bind(&ch("m"), plain(), false),
            Some(Message::ToggleMute)
        ));
        assert!(matches!(
            bind(&ch("f"), plain(), false),
            Some(Message::ToggleFullscreen)
        ));
        assert!(matches!(
            bind(&ch("s"), plain(), false),
            Some(Message::Screenshot)
        ));
        assert!(matches!(
            bind(&ch("["), plain(), true),
            Some(Message::SpeedStep(-1))
        ));
        assert!(matches!(
            bind(&ch("]"), plain(), true),
            Some(Message::SpeedStep(1))
        ));
    }

    /// Ctrl + ← / → 是切集，不是跳进度；不带 Ctrl 才是跳进度
    #[test]
    fn ctrl_arrows_switch_episodes() {
        assert!(matches!(
            bind(&Key::Named(Named::ArrowLeft), ctrl(), false),
            Some(Message::Prev)
        ));
        assert!(matches!(
            bind(&Key::Named(Named::ArrowRight), ctrl(), false),
            Some(Message::Next)
        ));
        // matches! 的模式里放不了常量表达式，只能取出来比
        match bind(&Key::Named(Named::ArrowLeft), plain(), false) {
            Some(Message::Nudge(d)) => assert_eq!(d, -SEEK_STEP),
            other => panic!("← 应该是快退，拿到 {other:?}"),
        }
        match bind(&Key::Named(Named::ArrowRight), plain(), true) {
            Some(Message::Nudge(d)) => assert_eq!(d, SEEK_STEP),
            other => panic!("→ 应该是快进，拿到 {other:?}"),
        }
    }

    /// 切换类的键按住不放不能连发，连续类的键要能连发
    #[test]
    fn toggles_ignore_repeat_but_continuous_keys_do_not() {
        for k in ["k", "m", "f", "s"] {
            assert!(
                bind(&ch(k), plain(), true).is_none(),
                "{k} 按住不放不该反复切换"
            );
        }
        assert!(bind(&Key::Named(Named::Space), plain(), true).is_none());
        for k in ["j", "l"] {
            assert!(bind(&ch(k), plain(), true).is_some(), "{k} 按住应该一直走");
        }
        assert!(bind(&Key::Named(Named::ArrowUp), plain(), true).is_some());
    }

    /// Ctrl + 字母只认 Ctrl+O，别的组合键不能顺手触发单键行为
    #[test]
    fn ctrl_letters_do_not_fall_through() {
        assert!(matches!(
            bind(&ch("o"), ctrl(), false),
            Some(Message::OpenDialog)
        ));
        assert!(bind(&ch("k"), ctrl(), false).is_none());
        assert!(bind(&ch("s"), ctrl(), false).is_none());
    }

    /// 说明表里不能有重复键位、也不能空着
    #[test]
    fn the_cheatsheet_is_complete() {
        assert!(!GROUPS.is_empty());
        for (name, list) in GROUPS {
            assert!(!name.is_empty());
            assert!(!list.is_empty(), "{name} 这一组是空的");
            for s in *list {
                assert!(!s.keys.is_empty() && !s.what.is_empty());
            }
        }
        // 按键位去重：同一个键写在两处会让人照着说明按却没反应
        let mut seen = Vec::new();
        for (_, list) in GROUPS {
            for s in *list {
                assert!(!seen.contains(&s.keys), "{} 写了两遍", s.keys);
                seen.push(s.keys);
            }
        }
    }
}
