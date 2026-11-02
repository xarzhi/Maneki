//! 图标字体（assets/iconfont/iconfont.ttf）的接入。
//!
//! 字体自带 21 个图标，**没有「暂停」字形**，所以暂停用两个圆角竖条手绘，
//! 尺寸和间距按图标字形的视觉重量对过，混在按钮里看不出来。

use iced::Font;
use iced::font::Family;

/// 字体族名取自 ttf 的 name 表（nameID=1），就是 `iconfont`
pub const ICON_FONT: Font = Font {
    family: Family::Name("iconfont"),
    weight: iced::font::Weight::Normal,
    style: iced::font::Style::Normal,
    stretch: iced::font::Stretch::Normal,
};

/// 界面文字字体：系统无衬线，**Normal 字重**。
///
/// 这里踩过一次坑：12~13px 的中文用 `Semibold` 反而更糊 ——
/// Segoe UI / 微软雅黑在系统里都没有 SemiBold 这个字面，cosmic-text
/// 只能「合成加粗」（把轮廓加厚），笔画边缘会多出一圈灰色过渡，
/// 看起来就是发虚。真正存在的是 Regular 和 Bold，需要强调时用后者。
/// 字号也统一往上提了 1px：同样一块屏幕，字越大每个笔画占的像素越多，
/// 灰阶抗锯齿的边缘占比就越小，观感明显更实。
pub const UI_FONT: Font = Font {
    family: Family::SansSerif,
    weight: iced::font::Weight::Normal,
    style: iced::font::Style::Normal,
    stretch: iced::font::Stretch::Normal,
};

/// 需要强调的文字（标题、当前选中项）：用**真实存在**的 Bold 字面
pub const UI_BOLD: Font = Font {
    family: Family::SansSerif,
    weight: iced::font::Weight::Bold,
    style: iced::font::Style::Normal,
    stretch: iced::font::Stretch::Normal,
};

pub const MINIMIZE: char = '\u{e711}';
pub const MAXIMIZE: char = '\u{e655}';
pub const CLOSE: char = '\u{e609}';
pub const PLAY: char = '\u{e622}';
pub const PREV: char = '\u{e66c}';
pub const NEXT: char = '\u{eca6}';
pub const SETTING: char = '\u{e611}';
pub const PLAYLIST: char = '\u{e60c}';
pub const FULLSCREEN: char = '\u{e63f}';
pub const VOLUME: char = '\u{eca5}';
/// 字幕（iconfont 里的 `zimu`）
pub const SUBTITLE: char = '\u{e6f6}';
/// 播放速度（`speed`）
pub const SPEED: char = '\u{e646}';
/// 文件详情（`xiangqing`）
pub const INFO: char = '\u{e637}';
/// 在资源管理器里打开（`folder`）
pub const FOLDER: char = '\u{e69d}';

// ---------------- 配色 ----------------
pub const ACCENT: iced::Color = iced::Color::from_rgb(0.36, 0.72, 0.98);
pub const TEXT: iced::Color = iced::Color::from_rgb(0.97, 0.97, 0.98);
/// 次要文字。不能太暗：顶底栏现在是 0.34 的高透明度，压在亮色画面上时
/// 暗淡的灰色会直接糊掉；字重改回 Normal 之后，靠亮度而不是笔画粗细来保证可读。
pub const TEXT_DIM: iced::Color = iced::Color::from_rgb(0.88, 0.89, 0.91);
/// 更弱一档的文字（说明、序号、时长）。
///
/// 以前是 0.70，在 100% 缩放下看着还行，但压在抽屉的黑色半透明底上就发灰、
/// 小字号又会再糊一层；提到 0.76 之后同样字号明显更实。
pub const TEXT_FAINT: iced::Color = iced::Color::from_rgb(0.76, 0.77, 0.80);
pub const HOVER_BG: iced::Color = iced::Color::from_rgba(1.0, 1.0, 1.0, 0.12);
pub const CLOSE_HOVER: iced::Color = iced::Color::from_rgb(0.86, 0.24, 0.24);

// ---------------- 悬浮容器 ----------------
//
// 播放列表抽屉、设置抽屉、音量 / 字幕 popover 共用一个底色：
// **黑色半透明**，不是纯黑。完全不透的纯黑压在画面上像挖了一个洞，
// 而且和顶底栏（0.34 透明的黑）不是一套语言；留 14% 的透光量，
// 底下画面在动的时候面板边缘能透出一点明暗，层次才对。
pub const PANEL_BG: iced::Color = iced::Color::from_rgba(0.035, 0.038, 0.045, 0.86);
/// 悬浮容器里 1px 分隔线 / 描边的颜色
pub const PANEL_LINE: iced::Color = iced::Color::from_rgba(1.0, 1.0, 1.0, 0.09);
