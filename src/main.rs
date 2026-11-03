// release 版不要黑底控制台窗口（双击打开视频时不能闪一个 cmd 出来）；
// debug 版保留控制台，方便看 panic 和日志。
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod assoc;
mod audio;
mod icon;
mod info;
mod meta;
mod mpv;
mod player;
mod playlist;
mod probe;
mod settings;
mod single;
mod spinner;
mod spring;
mod subtitle;
mod ui;
mod video;
mod win32;

use iced::alignment::{Horizontal, Vertical};
use iced::widget::{
    button, column, container, mouse_area, row, slider, space, stack, text, vertical_slider, Float,
};
use iced::window::Direction;
use iced::{
    keyboard, mouse, Alignment, Background, Border, Color, Element, Fill, Length, Padding, Point,
    Rectangle, Shadow, Size, Subscription, Task, Theme, Vector,
};
use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use player::{Cmd, Player};
use playlist::Playlist;
use settings::{Field, Settings, Tab};
use video::{Aspect, FitMode, VideoProgram};

/// 顶栏高度
const TOP_H: f32 = 38.0;
/// 底栏高度
const BOTTOM_H: f32 = 96.0;
/// 鼠标静止多久后自动收起
const HIDE_AFTER: Duration = Duration::from_secs(2);
/// 点完播放/暂停之后，最多容忍 mpv 多久还没「回声」。
///
/// 正常情况下播放线程几毫秒就追上来了，这个值只是兜底：万一 mpv 不认这条命令
/// （文件坏了之类），超时之后还是要老老实实跟着快照走，否则按钮会永远停在
/// 用户点出来的那个状态上，跟实际播放状况对不上。
const PAUSE_ECHO: Duration = Duration::from_millis(300);
/// 一批拖放要「安静」这么久才算拖完（一次拖多个文件时事件是连着来的）
const DROP_SETTLE: Duration = Duration::from_millis(120);
/// ← / → 一次快退 / 快进多少秒
const SEEK_STEP: f64 = 5.0;
/// ↑ / ↓ 一次调多少音量（音量刻度是 0~100）
const VOL_STEP: f64 = 5.0;
/// 音频舞台左边那块封面的边长上限（窗口窄的时候按比例缩）
const ART_SIZE: f32 = 220.0;
/// 歌词窗：离当前行超过这么多行就彻底淡掉（再远的本来也在视口外）
const LYRIC_FADE_ROWS: usize = 6;
/// 歌词窗里一行占多高。定死高度，换行时整块才不会一抖一抖
const LYRIC_ROW_H: f32 = 26.0;
/// 歌词当前行的字号，以及离它一行之外的字号。
/// 中间是**连续插值**的：某一行离正中间越近字越大，换句时一行行变大的过程
/// 跟着整列的弹簧一起走（AMLL 里那行放大也是按距离算的，不是「到点啪一下变大」）。
/// 这两个是**默认值**，Ctrl + 滚轮调完字号之后由 `App::lyric_size` 缩放。
const LYRIC_SIZE_ACTIVE: f32 = 17.0;
const LYRIC_SIZE_IDLE: f32 = 13.5;
/// 歌词字号能调到的范围（常态行的字号，单位 pt）。再小看不清，再大一行就放不下几个字了。
const LYRIC_SIZE_MIN: f32 = 10.5;
const LYRIC_SIZE_MAX: f32 = 21.0;
/// Ctrl + 滚轮一格调多少字号
const LYRIC_SIZE_STEP: f32 = 0.5;
/// 滚轮一格滚几行歌词
const LYRIC_WHEEL_ROWS: f32 = 2.0;
/// 手动滚完停这么久，自动滑回「正在唱的那一句」。
///
/// 不自动回的话，用户滚过去看一眼后面的词，就再也回不到正在唱的位置（除非正好
/// 又滚回来）—— 一个只有七行的歌词窗，这太容易迷路了。四秒够看完后面几句。
const LYRIC_BROWSE_HOLD: Duration = Duration::from_secs(4);
/// 目标一下跳得比这还远（拖进度条、刚换歌）就别弹了，直接落位。
///
/// 一句一句往上唱着走，弹簧那点过冲是味道；可要是从第 2 句直接跳到第 40 句，
/// 让弹簧去追就是「整列歌词糊成一片往上飞」，看着像卡了。六行已经是半个视口，
/// 中间那段本来也看不清 —— 直接落位反而干净。
const LYRIC_JUMP_ROWS: f32 = 6.0;
/// 歌词的「当前行 / 相邻行」两档亮度，中间的由 `lyric_alpha` 连续插值。
/// 当前行 1.0（纯白），相邻行 0.60，再往外按几何级数衰减到 0。
const LYRIC_ALPHA_ACTIVE: f32 = 1.0;
const LYRIC_ALPHA_NEAR: f32 = 0.60;
/// 光晕呼吸一圈要几秒
const AUDIO_PULSE: f32 = 4.2;
/// 音频舞台的底色：比纯黑亮一档的冷灰，中间那块「唱片」才浮得起来
const AUDIO_BG: Color = Color::from_rgb(0.043, 0.047, 0.055);
/// 收起/展开的过渡时间常数（约 0.09s 走完 63%，体感 250ms 左右）
const ANIM_TAU: f32 = 0.09;

/// 开源地址。「关于」页里显示这一行，点它用系统浏览器打开。
pub const REPO_URL: &str = "https://github.com/xarzhi/Maneki";
/// 换片后等这么久还没出画面，才把转圈画出来。
///
/// 本地小文件常常 50~80ms 就出首帧了，门一开到就画圈会在每次切集时闪一下，
/// 反而更难看。超过这个时长说明确实在等（大文件、机械盘、网络盘），才值得提示。
const SPIN_DELAY: Duration = Duration::from_millis(80);
/// 换片时**手上还顶着上一帧**的情况（见 `App::stale_frame`）要等更久才画圈。
///
/// mpv 换片时本来就会把上一条继续渲染到新文件接手为止（实测交接大约 100ms），
/// 画面是连着的。这时候要是按 `SPIN_DELAY` 就把圈和暗色压上去，用户看到的是
/// 「画面暗一下又亮回来」，等于自己造了一次闪。拖到这个时长还没新帧，才是真的卡住了。
const STALE_SPIN_DELAY: Duration = Duration::from_millis(400);
/// 转圈从出现到完全不透明的淡入时长（秒）
const SPIN_FADE: f32 = 0.16;
/// 窗口四边/四角的缩放热区厚度
const EDGE: f32 = 6.0;
/// 顶底栏底色的不透明度：越小越透（0.0 = 完全透明，1.0 = 纯黑）
const BAR_ALPHA: f32 = 0.34;
/// 窗口最小尺寸（逻辑像素）
const MIN_W: f32 = 560.0;
const MIN_H: f32 = 400.0;

/// 底栏图标按钮的边长（和 `icon_button` 一致，音量 popover 靠它对位）
const ICON_BTN: f32 = 34.0;
/// 底栏图标按钮之间的间距
const ICON_GAP: f32 = 6.0;
/// 底栏内容的左右内边距（和 bottom_bar 里的 `padding([10, 14])` 一致）
const BAR_PAD_X: f32 = 14.0;
/// 左下角「播放速度」那颗胶囊按钮的宽度（显示 "1.25×" 这种标签）
const SPEED_W: f32 = 46.0;
/// 音量 popover 面板宽度
const POP_W: f32 = 48.0;
/// 速度 popover 面板宽度
const SPEED_POP_W: f32 = 208.0;
/// 音量 popover 底边与底栏顶边的间隙
const POP_GAP: f32 = 8.0;
/// 音量 popover 里竖滑块的高度
const POP_SLIDER_H: f32 = 104.0;
/// 右键菜单面板宽度
const MENU_W: f32 = 196.0;
/// 速度预设（点一下就设这个值）
const SPEEDS: [f64; 9] = [0.5, 0.7, 1.0, 1.25, 1.5, 1.75, 2.0, 2.5, 3.0];
/// 自定义调速滑块的范围
const SPEED_MIN: f64 = 0.5;
const SPEED_MAX: f64 = 8.0;
/// 抽屉滑入 / 滑出的位移余量：多出来的这几像素保证阴影也一起滑出去，不会留一条边
const DRAWER_LEAD: f32 = 16.0;

// ---------------- 单例 ----------------
//
// `iced::application` 的构造函数是 `fn() -> (App, Task)`，没法传参数进去，
// 所以启动阶段拿到的单例句柄和命令行路径先放进静态里，`App` 再通过
// `pending_open()` / `STARTUP_PATHS` 取用。
//
// 句柄用 `Mutex` 包一层是为了满足 `static` 的 `Sync` 要求：
// `Receiver` 是 `Send` 但不是 `Sync`。锁只在 `poll()` 那一瞬持有，没有争用。

/// 主实例才有（抢不到时进程在 main 里就退出了）
static INSTANCE: OnceLock<std::sync::Mutex<Option<single::Instance>>> = OnceLock::new();
/// 启动时要打开的文件 / 文件夹
static STARTUP_PATHS: OnceLock<Vec<PathBuf>> = OnceLock::new();

/// 别的实例有没有转交过来一批路径（没有就是 None）。非阻塞，每个 tick 都可调。
fn pending_open() -> Option<Vec<PathBuf>> {
    INSTANCE.get()?.lock().ok()?.as_ref()?.poll()
}

// ---------------- 品牌 logo ----------------
//
// PNG 在编译期就转成原始 RGBA 存好（assets/*.rgba），运行时不用解码，
// 也就省掉了 iced 的 png 解码特性，启动时间不受影响。
//
// 顶栏那一颗**按显示尺寸出图**（24×24 显示 24px）：iced 的图片采样是双线性，
// 只要缩放比不是 1.0 就会重新采样一遍，96→22 这种非整数缩小会把笔画边缘糊掉。
// 空白页 / 任务栏图标仍是 256×256 —— 那里要的显示尺寸大，留余量才是对的。

/// 顶栏用：24×24 原始 RGBA（**按 1:1 显示**，让 iced 不要去缩它）
///
/// 之前是 96×96 缩到 22px，双线性缩小的次数多、每次都在丢细节，越缩越软。
/// 现在源图就是显示尺寸，采样几乎是「点对点」，笔画边缘干净得多。
const LOGO_RGBA: &[u8] = include_bytes!("../assets/logo_bar.rgba");
const LOGO_SRC: u32 = 24;
/// 窗口图标 / 空白页用：256×256 原始 RGBA
const LOGO_BIG_RGBA: &[u8] = include_bytes!("../assets/logo_icon.rgba");
const LOGO_BIG_SRC: u32 = 256;
/// 顶栏里 logo 的显示边长。必须**等于** `LOGO_SRC`，缩放比 1.0 才不重新采样。
const LOGO_BAR: f32 = 24.0;
/// 空白页中央 logo 的显示边长
const LOGO_HERO: f32 = 80.0;

fn main() -> iced::Result {
    // 命令行开关：注册 / 注销文件关联，做完就退出，不启界面
    match std::env::args().nth(1).as_deref() {
        Some("--register") => {
            report(assoc::register(), "已注册为视频 / 音频文件的打开方式。");
            return Ok(());
        }
        Some("--unregister") => {
            report(assoc::unregister(), "已取消注册。");
            return Ok(());
        }
        _ => {}
    }

    // 单例：第二次启动（双击 exe、或者右键把视频「打开方式」选成 Maneki）
    // 不应该再开一个新窗口 —— 把路径转交给已经在跑的那个实例，然后直接退出。
    // 退出要发生在建窗口之前，所以这一段必须在 `iced::application(...)` 上面。
    //
    // `QP_MULTI=1` 是调试开关：跳过单例，允许同时开多个窗口（截图脚本对样式时用，
    // 也方便自己开两份对比；不设这个变量就完全是正常的单例行为）。
    let paths: Vec<PathBuf> = std::env::args().skip(1).map(PathBuf::from).collect();
    let _ = STARTUP_PATHS.set(paths.clone());
    if std::env::var_os("QP_MULTI").is_none() {
        match single::Instance::acquire(paths) {
            Ok(inst) => {
                let _ = INSTANCE.set(std::sync::Mutex::new(Some(inst)));
            }
            // `Err` 表示路径已经交给主实例了，本进程什么都不用做
            Err(_) => return Ok(()),
        }
    }

    // 首次启动（或 exe 被挪了位置）时把关联补上，写注册表只要几百微秒，不影响秒开
    assoc::ensure();

    iced::application(App::new, App::update, App::view)
        .title(|_: &App| String::from("Maneki"))
        .theme(|_: &App| Theme::Dark)
        .subscription(App::subscription)
        .window(iced::window::Settings {
            size: Size::new(1180.0, 720.0),
            decorations: false,
            // 任务栏 / Alt-Tab 的图标
            icon: iced::window::icon::from_rgba(
                LOGO_BIG_RGBA.to_vec(),
                LOGO_BIG_SRC,
                LOGO_BIG_SRC,
            )
            .ok(),
            ..Default::default()
        })
        .antialiasing(true)
        // 图标字体：assets/iconfont/iconfont.ttf，族名是 iconfont
        .font(include_bytes!("../assets/iconfont/iconfont.ttf").as_slice())
        .run()
}

fn report(result: Result<(), String>, ok_msg: &str) {
    match result {
        Ok(()) => println!("{ok_msg}"),
        Err(e) => println!("{e}"),
    }
}

#[derive(Debug, Clone)]
enum Message {
    Tick,
    OpenDialog,
    /// 打开「设置 → 默认应用」里的 Maneki 页
    OpenDefaultApps,
    Picked(Option<PathBuf>),
    /// 拖进来的东西（可能是文件，也可能是文件夹）。
    /// 一次拖多个文件时 winit 会连着发好几条，所以先攒着，见 `flush_drops`
    Dropped(PathBuf),
    /// 选了一个文件夹
    PickedFolder(Option<PathBuf>),
    TogglePlay,
    Seek(f64),
    /// 方向键 ← / →：快退 / 快进，正数前进
    Nudge(f64),
    /// 方向键 ↑ / ↓：调音量，正数增大（0~100 的刻度）
    NudgeVolume(f64),
    Volume(f64),
    /// 上一集（没有播放列表时退化成后退 10 秒）
    Prev,
    /// 下一集（没有播放列表时退化成前进 10 秒）
    Next,
    ToggleMute,
    /// 点音量键：弹出 / 收起音量 popover
    ToggleVolumePopover,
    /// 点左上角 logo：窗口置顶 / 取消置顶
    ToggleTopmost,
    ToggleFullscreen,
    /// Esc：只有在全屏时才有动作
    Escape,
    GotWindow(Option<iced::window::Id>),
    GotHwnd(Option<isize>),
    /// 按住窗口边缘开始拖拽缩放
    ResizeStart(Direction),
    WinDrag,
    WinMinimize,
    WinMaximize,
    WinClose,
    MouseMoved(Point),
    MouseEntered,
    MouseLeft,
    Resized(Size),
    /// 在歌词区滚了滚轮，正数 = 往下滚一行（已经换算成"行"了，见 `wheel_rows`）
    LyricWheel(f32),
    /// 点了第几行歌词：跳到那一句
    LyricSeek(usize),
    /// 点了抽屉外那层透明 cover。若那一下正好落在歌词面板里，就顺手当成点歌词 ——
    /// 抽屉开着时 cover 会挡住歌词，不能让「点歌词跳转」平白失效。
    CoverClick,
    /// 修饰键变了（只关心 Ctrl 按没按着 —— 它决定滚轮是调字号还是滚歌词）
    Modifiers(bool),
    /// 在画面上按下左键（只有真的按在画面上才会来，按在顶底栏会被它们先吃掉）
    StagePress,
    /// 在画面上松开左键
    StageRelease,

    // ---- 设置弹窗 ----
    /// 点底栏的齿轮
    OpenSettings,
    CloseSettings,
    SettingsTab(Tab),
    SetFit(FitMode),
    SetAspect(Aspect),
    SetLook(Field, f32),
    ResetLook,
    /// 设置里的「自动连播」勾选
    SetAutoplay(bool),
    /// 「关于」页里点开源地址：用系统浏览器打开
    OpenRepo,

    // ---- 字幕 popover ----
    /// 点底栏的字幕键：展开 / 收起字幕 popover
    ToggleSubtitlePopover,
    /// 重新读一遍轨道表（字幕）
    RefreshTracks,
    /// 选择字幕轨；None = 不使用字幕
    SelectSub(Option<i64>),
    PickSubtitle,
    PickedSubtitle(Option<PathBuf>),

    // ---- 播放列表 ----
    /// 点底栏的播放列表键：展开 / 收起右侧抽屉
    TogglePlaylist,
    /// 点抽屉里的某一条
    PlayIndex(usize),
    ClearPlaylist,
    /// 选一个文件夹，把里面的视频和音频全部装进列表
    PickFolder,

    // ---- 播放速度 ----
    /// 点左下角的速度键：展开 / 收起速度 popover
    ToggleSpeedPopover,
    /// 设一个新的倍速（预设按钮和自定义滑块都走这里）
    SetSpeed(f64),

    // ---- 右键菜单 ----
    /// 在画面上按了右键
    ContextMenu,
    /// 右键菜单 / 详情抽屉：在资源管理器里定位这个文件
    RevealInExplorer,
    /// 右键菜单：打开「文件详情」抽屉
    OpenInfo,
    CloseInfo,

    /// 什么都不做。用来「吃掉」事件 —— 比如抽屉上那层 button、cover 上的右键。
    Noop,
}

struct App {
    player: Option<Player>,
    window: Option<iced::window::Id>,
    hwnd: Option<isize>,
    /// 正在拖边缘缩放时的起始快照
    drag: Option<DragResize>,
    /// 正在按住顶栏挪窗口时的起始快照
    moving: Option<DragMove>,
    /// 当前视频的显示尺寸（dwidth/dheight）
    video_size: Option<(u32, u32)>,
    /// 这个文件是不是已经按视频尺寸调过窗口了（只调一次，别跟用户手动改尺寸打架）
    sized_for_video: bool,

    error: Option<String>,
    title: String,
    /// 当前文件的路径（文件对话框默认从这里打开）
    current: Option<PathBuf>,
    frame: Option<video::VideoFrame>,
    /// 手上这一帧是不是**上一条**片子留下的。
    ///
    /// 换片时不再立刻把 `frame` 清成 `None`（那样画面会先黑一下，用户看到的就是
    /// 「闪一下」），而是留着上一帧继续画，等新片的首帧到了再换。这个标记用来
    /// 区分「这帧是当前片子的」还是「只是个占位」，加载拖长时才好决定要不要
    /// 压暗加转圈。
    stale_frame: bool,
    /// 换片的代次号。每发一次 `Cmd::Load` / `Cmd::Stop` 就 +1，
    /// 只采信带着这个号的快照 —— 见 `Cmd::Load` 的说明。
    load_gen: u64,
    /// 这一条片子是什么时候开始加载的（`play()` 里记，出首帧时清）。
    /// 换片那一小段空档靠它判断要不要画转圈，以及画了多久。
    loading_since: Option<Instant>,
    /// 转圈动画的相位，0..1 一整圈
    spin: f32,
    /// 音频舞台那圈光晕的相位，0..1 一整圈（播放时慢慢呼吸）
    pulse: f32,
    /// 音频的曲名 / 歌手 / 封面 / 歌词，后台线程读完丢进来
    tags: meta::Meta,
    /// 封面位图。`tags.cover` 里那份原始像素换成了 iced 的 `Handle` 存这儿 ——
    /// 每帧 `image()` 都要拿一个 Handle，从 RGBA 现造等于每帧重传一遍像素。
    cover: Option<iced::widget::image::Handle>,
    /// 同步歌词当前唱到第几行（没歌词 / 纯文本歌词时是 `None`）
    lyric_line: Option<usize>,
    /// 歌词**画在第几行的高度**，可以是小数，由一条弹簧推着走。
    ///
    /// 换句时它不是"逼近"到新行，而是**冲过头再弹回来**（见 `spring` 模块）——
    /// AMLL 那种手感。界面按它算整块歌词往上挪多少，每行离它多远决定字号和亮度。
    ///
    /// 用弹簧而不是指数逼近还有一个好处：连唱两句间隔很短时，弹簧带着速度
    /// 直接接上，整列是"一路滑过去"，不会每句都停一下再起步。
    lyric_scroll: spring::Spring,
    /// 歌词常态行的字号（pt）。Ctrl + 滚轮在 `LYRIC_SIZE_MIN..=LYRIC_SIZE_MAX` 之间调。
    lyric_size: f32,
    /// 手动滚动的**额外偏移**（单位"行"，0 = 正好跟着当前句）。
    ///
    /// 显示位置 = `lyric_scroll + lyric_browse`：`lyric_scroll` 一直在跟着唱到哪句走，
    /// 滚轮只叠一个偏移上去，所以滚完四秒（`LYRIC_BROWSE_HOLD`）自动滑回 0 就
    /// 又跟上了，不需要记"刚才滚到第几行"。
    lyric_browse: spring::Spring,
    /// 最后一次手动滚歌词是什么时候（超时就归零，见 `animate_with`）
    lyric_browse_at: Option<Instant>,
    /// 手动浏览时**钉住**的显示位置（第几行在中间）。
    ///
    /// 只靠 `lyric_browse` 那个偏移是不够的：播放会继续往前唱，`lyric_scroll` 一走，
    /// 视图就跟着漂 —— 用户滚上去看第一句，两秒后它自己变成第二句。
    /// 所以浏览期间每帧用这个值反算偏移（`pin - scroll`），把画面钉在原地。
    lyric_pin: Option<f32>,
    /// Ctrl 现在按着没有。有它滚轮是「调字号」，没有才是「滚歌词」。
    ctrl: bool,
    /// 后台读标签的线程（第一次播音频时才创建，不拖慢启动）
    tag_loader: Option<meta::Loader>,
    paused: bool,
    /// 最近一次「播放 / 暂停」是什么时候发给 mpv 的。
    ///
    /// 播放线程的快照永远比命令慢半拍：点下暂停的那一瞬间，队列里还压着一两条
    /// 「pause=no」的旧快照，照抄回来就会把按钮先翻成「暂停」再翻回「播放」——
    /// 用户看到的就是按钮闪一下。所以记下发送时刻，等快照追上（或者等超时）之后
    /// 才重新采信 `snap.paused`。
    pause_sent: Option<Instant>,
    duration: f64,
    position: f64,
    volume: f64,
    open_ms: Option<f64>,
    fps: f32,
    seek_at: Option<Instant>,

    /// 1.0 = 完全露出，0.0 = 完全收起。用指数平滑逼近目标值做过渡动画
    bars: f32,
    last_active: Instant,
    /// 上一帧动画是什么时候推的。动画按**真实经过的时间**推进，帧率抖、定时器
    /// 早到/晚到都不影响走完的时间（见 `animate_with`）。
    anim_last: Option<Instant>,
    cursor: Option<Point>,
    win_size: Size,
    muted_before: f64,
    fullscreen: bool,
    /// 音量 popover 是否展开
    volume_popover: bool,
    /// 字幕 popover 是否展开
    sub_popover: bool,
    /// 窗口是否置顶（点左上角 logo 切换）
    topmost: bool,
    /// 设置抽屉的状态（填充 / 比例 / 色彩 / 自动连播 / 当前标签页）
    settings: Settings,
    /// 设置抽屉的滑入滑出进度（1 = 完全展开，0 = 完全收在右边外面）
    settings_anim: f32,
    /// 播放列表抽屉的滑入滑出进度
    playlist_anim: f32,
    /// 这一次按下是从画面上开始的吗？
    ///
    /// iced 的 `mouse_area.on_release` 只判断「松手时鼠标在不在范围内」，
    /// 而画面那层铺满整个窗口 —— 于是点顶底栏、点弹窗空白处松手时，
    /// 画面也会收到 release、把播放暂停掉。加这个标志后只有「按下也在画面上」
    /// 才算数，被上面各层吃掉的按下不会把画面暂停。
    stage_press: bool,
    /// 当前文件的全部轨道（只挑字幕用）
    tracks: Vec<mpv::Track>,
    /// 有没有内嵌字幕（外挂的不算）
    embedded_subs: bool,
    /// 当前选中的字幕轨
    sid: Option<i64>,
    /// 字幕页面上的一行临时提示（加载成功/失败）
    sub_hint: Option<String>,
    /// 播放列表（拖文件夹 / 选文件夹时装载）
    playlist: Playlist,
    /// 攒着还没处理的拖放路径（一次拖多个文件时会连着来好几条）
    pending_drops: Vec<PathBuf>,
    /// 最后一条拖放的时间，用来判断「这一批拖完了」
    drop_at: Option<Instant>,
    /// 上一帧的 eof 状态，只在 false → true 的那一下接下一集
    eof_prev: bool,
    /// 后台时长探测（第一次装列表时才创建，不拖慢启动）
    prober: Option<probe::Prober>,
    /// 单例句柄留在 `INSTANCE` 静态里（互斥体要活到进程结束），
    /// 这里只需要「文件详情」抽屉的状态
    /// 「文件详情」抽屉是否打开
    info_open: bool,
    /// 详情抽屉的滑入滑出进度
    info_anim: f32,
    /// 当前文件的元信息（首帧出来后由播放线程填好）
    info: Option<mpv::MediaInfo>,
    /// 播放速度倍率（1.0 = 原速）
    speed: f64,
    /// 速度 popover 是否展开
    speed_popover: bool,
    /// 右键菜单的位置（窗口内的逻辑坐标）；None = 没展开
    context: Option<Point>,
}

/// 拖边缘缩放时的起始快照，全部是物理像素（屏幕坐标）
#[derive(Debug, Clone, Copy)]
struct DragResize {
    direction: Direction,
    /// 按下那一刻的鼠标位置
    start_cursor: (i32, i32),
    /// 按下那一刻的窗口 (x, y, w, h)
    start_rect: (i32, i32, i32, i32),
    /// 上一次真正下发下去的 (x, y, w, h)，避免每 tick 无意义地重复 SetWindowPos
    applied: (i32, i32, i32, i32),
}

/// 按住顶栏挪窗口时的起始快照，全部是物理像素（屏幕坐标）。
///
/// **不能用 `iced::window::drag`**：它最后调的是 winit 的 `drag_window()`，
/// 也就是给窗口发 `WM_NCLBUTTONDOWN(HTCAPTION)`，交给 `DefWindowProc` 去跑
/// Windows 的**模态移动循环**。那个循环会把整个 UI 线程占住 —— 音频在 mpv 的
/// 线程里照放，画面却一帧都不再画，松开鼠标才恢复（用户看到的就是「按住就
/// 暂停、松手才继续」）。这里改成和拖边缘缩放同一套做法：只记下起点，
/// 每个 tick 根据鼠标位移自己 `SetWindowPos`。
#[derive(Debug, Clone, Copy)]
struct DragMove {
    /// 按下那一刻的鼠标位置
    start_cursor: (i32, i32),
    /// 按下那一刻的窗口左上角
    start_pos: (i32, i32),
    /// 窗口尺寸（挪动过程中不变，但 `set_window_rect` 两个都得给）
    size: (i32, i32),
    /// 上一次真正下发下去的位置，避免每 tick 重复 SetWindowPos
    applied: (i32, i32),
}

impl App {
    fn new() -> (Self, Task<Message>) {
        let (player, error) = match Player::spawn() {
            Ok(p) => (Some(p), None),
            Err(e) => (None, Some(e)),
        };

        let mut app = Self {
            player,
            window: None,
            hwnd: None,
            drag: None,
            moving: None,
            video_size: None,
            sized_for_video: false,
            error,
            title: String::from("Maneki"),
            current: None,
            frame: None,
            stale_frame: false,
            load_gen: 0,
            loading_since: None,
            spin: 0.0,
            pulse: 0.0,
            tags: meta::Meta::default(),
            cover: None,
            lyric_line: None,
            lyric_scroll: spring::Spring::bouncy(0.0),
            lyric_size: LYRIC_SIZE_IDLE,
            lyric_browse: spring::Spring::smooth(0.0),
            lyric_browse_at: None,
            lyric_pin: None,
            ctrl: false,
            tag_loader: None,
            paused: true,
            pause_sent: None,
            duration: 0.0,
            position: 0.0,
            volume: 80.0,
            open_ms: None,
            fps: 0.0,
            seek_at: None,
            bars: 1.0,
            last_active: Instant::now(),
            anim_last: None,
            cursor: None,
            win_size: Size::new(1180.0, 720.0),
            muted_before: 80.0,
            fullscreen: false,
            volume_popover: false,
            sub_popover: false,
            topmost: false,
            settings: Settings::default(),
            settings_anim: 0.0,
            playlist_anim: 0.0,
            stage_press: false,
            tracks: Vec::new(),
            embedded_subs: false,
            sid: None,
            sub_hint: None,
            playlist: Playlist::default(),
            pending_drops: Vec::new(),
            drop_at: None,
            eof_prev: false,
            prober: None,
            info_open: false,
            info_anim: 0.0,
            info: None,
            speed: 1.0,
            speed_popover: false,
            context: None,
        };

        if let Some(p) = &app.player {
            p.send(Cmd::Volume(app.volume));
            p.send(Cmd::Speed(app.speed));
        }

        // 启动时带的路径（双击关联文件 / 命令行 / 拖到 exe 图标上）
        if let Some(paths) = STARTUP_PATHS.get()
            && !paths.is_empty()
        {
            app.open_paths(paths.clone());
        }

        // 调试入口：`QP_DEMO=settings|subtitle|playlist` 启动时直接展开对应浮层，
        // 方便脚本截图核对样式（不设这个变量就走正常流程，和 QP_FONT_TEST 一个性质）。
        match std::env::var("QP_DEMO").as_deref() {
            Ok("settings") => {
                app.playlist.open = false;
                app.settings.open = true;
            }
            Ok("color") => {
                app.playlist.open = false;
                app.settings.open = true;
                app.settings.tab = Tab::Color;
            }
            Ok("about") => {
                app.playlist.open = false;
                app.settings.open = true;
                app.settings.tab = Tab::About;
            }
            Ok("subtitle") => {
                app.playlist.open = false;
                app.sub_popover = true;
            }
            Ok("playlist") => app.playlist.open = true,
            Ok("volume") => app.volume_popover = true,
            Ok("speed") => {
                app.playlist.open = false;
                app.speed_popover = true;
            }
            Ok("info") => {
                app.playlist.open = false;
                app.info_open = true;
            }
            Ok("context") => {
                app.playlist.open = false;
                app.context = Some(Point::new(420.0, 300.0));
            }
            _ => {}
        }

        // 拿到窗口 id 才能做拖动 / 最小化 / 最大化 / 关闭
        (app, iced::window::latest().map(Message::GotWindow))
    }

    fn play(&mut self, path: PathBuf) {
        let Some(player) = &self.player else {
            self.error = Some(String::from("播放引擎没加载起来，无法播放。"));
            return;
        };

        self.title = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.to_string_lossy().into_owned());
        self.current = Some(path.clone());
        // 列表里如果有这一条，顺手把高亮对上（用户点了别的、或播完自动接续都靠这个）
        self.playlist.select = self.playlist.index_of(&path);
        self.eof_prev = false;
        // **不**清 `frame`：留着上一帧继续画。
        // 清成 None 的话，从点下去到新片出首帧这段（几十毫秒到几百毫秒）画面会是
        // 一整块黑 —— 用户看到的就是「切视频时闪一下」。这里只打个标记，等新片
        // 首帧到了（`tick()` 里）再换掉；真加载得久，`spinner_opacity()` 会把旧画面
        // 压暗并叠一个转圈，也不会突兀地黑屏。
        self.stale_frame = self.frame.is_some();
        // 从这一刻起算「在加载」，出首帧时（tick 里）清掉
        self.loading_since = Some(Instant::now());
        self.spin = 0.0;
        self.open_ms = None;
        self.position = 0.0;
        self.duration = 0.0;
        self.paused = false;
        // 换片子也是一次「命令在路上」：上一条如果是暂停状态，队列里还压着
        // 「pause=yes」的快照，别让它把新片子的按钮画成暂停。
        self.pause_sent = Some(Instant::now());
        self.error = None;
        self.bars = 1.0;
        self.last_active = Instant::now();
        self.video_size = None;
        // 注意：这里**不动** `sized_for_video`。换集（点下一集、连播、点列表某一行）
        // 都不该重新摆窗口 —— 用户把窗口拉成什么形状是他自己的事，换个片子就被
        // 拽回去很讨厌。只有「重新打开一批文件」（拖进来 / 文件对话框 / 命令行）
        // 才在 `open_paths` 里把它复位，让窗口按新视频的尺寸重新摆一次。
        self.tracks.clear();
        self.embedded_subs = false;
        self.sid = None;
        self.sub_hint = None;

        // 封面 / 歌词先清空，别让上一首歌的封面在新歌上挂一会儿。
        // 只有音频才去读：视频的标签用不上，白读一遍还多开一条线程。
        self.tags = meta::Meta::default();
        self.cover = None;
        self.lyric_line = None;
        self.lyric_scroll.snap(0.0);
        self.lyric_browse.snap(0.0);
        self.lyric_browse_at = None;
        self.lyric_pin = None;
        if audio::is_audio(&path) {
            if self.tag_loader.is_none() {
                self.tag_loader = meta::Loader::spawn();
            }
            if let Some(loader) = &self.tag_loader {
                loader.request(path.clone());
            }
        }

        self.load_gen = self.load_gen.wrapping_add(1);
        player.send(Cmd::Load(path, self.load_gen));
    }

    /// 把当前这条卸掉，回到「列表里有东西、但一条都没在放」的状态。
    ///
    /// 打开文件夹后停在这里，等用户自己点一条。和 `play()` 的区别是：
    /// `play()` 是换一条接着放，这里是一条都不放 —— 所以除了清界面状态，
    /// 还要给引擎发 `Cmd::Stop`，否则上一条片子还在解码，下一帧就冒出来了。
    fn unload(&mut self) {
        self.current = None;
        self.title = String::from("Maneki");
        self.frame = None;
        self.stale_frame = false;
        self.loading_since = None;
        self.duration = 0.0;
        self.position = 0.0;
        self.paused = false;
        // 卸载不涉及暂停，把「命令在路上」的标记清掉，
        // 免得下一次 play 的头几帧被它挡着不认快照
        self.pause_sent = None;
        self.seek_at = None;
        self.video_size = None;
        self.open_ms = None;
        self.fps = 0.0;
        self.tracks.clear();
        self.embedded_subs = false;
        self.sid = None;
        self.sub_hint = None;
        self.info = None;
        self.eof_prev = false;
        self.error = None;
        // 封面 / 歌词跟着一起清：界面回到「从列表里挑一个」的空白页
        self.tags = meta::Meta::default();
        self.cover = None;
        self.lyric_line = None;
        self.lyric_scroll.snap(0.0);
        self.lyric_browse.snap(0.0);
        self.lyric_browse_at = None;
        self.lyric_pin = None;
        // 代次也往前推一格：在途的旧快照（还是上一条片子的画面/进度）一律不认，
        // 否则刚清空的界面会被它们重新填回旧数字。
        self.load_gen = self.load_gen.wrapping_add(1);
        self.send(Cmd::Stop);
    }

    /// 播列表里的第几条
    fn play_index(&mut self, index: usize) {
        let Some(item) = self.playlist.items.get(index) else {
            return;
        };
        let path = item.path.clone();
        self.play_index_path(index, path);
    }

    fn play_index_path(&mut self, index: usize, path: PathBuf) {
        self.playlist.select = Some(index);
        self.play(path);
    }

    /// 切到列表里的上 / 下**一条视频**。
    ///
    /// 没有上一条（已经在第一条）或者压根没有列表（只有一个文件）时什么也不做 ——
    /// 上一版在这里退化成了「快退 / 快进 10 秒」，用户要的不是那个：上一集 / 下一集
    /// 就是换视频，换不了就应该是**没反应**，不然会被误当成进度跳转。
    fn step_item(&mut self, delta: isize) {
        let Some(cur) = self.playlist.select else {
            return;
        };
        let last = self.playlist.items.len().saturating_sub(1);
        let next = cur as isize + delta;
        // 越界就什么都不做（不夹到 0 / last，避免「在第一条点上一集又重播一遍」）
        if next < 0 || next > last as isize {
            return;
        }
        self.play_index(next as usize);
    }

    /// 打开一批路径（文件、文件夹混着都行）。
    ///
    /// - 单个文件：直接播它，列表里就这一条
    /// - 文件夹 / 一次拖进来好几个：把里面的视频和音频按自然顺序全装进列表，
    ///   **一条都不自动播**，把右边的抽屉拉开，等用户点哪条放哪条
    fn open_paths(&mut self, paths: Vec<PathBuf>) {
        let scan = playlist::scan(&paths);

        if scan.files.is_empty() {
            self.playlist.set(None, Vec::new());
            self.playlist.note = Some(if paths.iter().any(|p| p.is_dir()) {
                String::from("这个文件夹里没有找到视频或音频")
            } else if scan.ignored > 0 {
                String::from("拖进来的不是视频或音频文件")
            } else {
                String::from("这里面没有可以播放的文件")
            });
            self.playlist.open = true;
            // 注意：这里**不动**正在播的那条 —— 拖进来一堆认不出的文件，
            // 不该把用户正在看的片子顺手停掉
            return;
        }

        let single_file = paths.len() == 1 && paths[0].is_file();

        // 新打开一批文件 = 新的一次「打开」，窗口可以按视频尺寸重新摆一次。
        // 换集（上一集 / 下一集 / 连播 / 点列表某一行）不走这里，所以不会动窗口。
        self.sized_for_video = false;

        if single_file {
            self.playlist.set(None, scan.files);
            self.play_index(0);
            self.start_probe();
            return;
        }

        // 抽屉头里显示的来源目录：批里有文件夹就用它，否则用第一个文件的父目录
        let root = paths
            .iter()
            .find(|p| p.is_dir())
            .cloned()
            .or_else(|| paths[0].parent().map(|p| p.to_path_buf()))
            .unwrap_or_else(|| paths[0].clone());

        self.playlist.set(Some(&root), scan.files);
        self.playlist.open = true;
        // 抽屉只能开一条：设置 / 字幕浮层这时候都该让位
        self.settings.close();
        self.sub_popover = false;
        // **不自动播第一个**。原来的做法是 `play_index(0)`，可文件夹往往是几百集的
        // 剧或者一整张专辑，替用户决定「先放第一个」经常不是他想要的（尤其是
        // 想从中间接着看的时候）。现在把引擎卸掉，界面停在「从列表里挑一个」。
        self.unload();
        self.start_probe();
    }

    /// 把列表里的文件排进后台探测队列，让每一行后面那句时长逐条补上。
    ///
    /// 探测用的是一个**独立的 mpv 实例**（见 `probe.rs`），第一次需要时才创建：
    /// 一个只有 1 条单文件的列表，没必要为了一个时间戳多起一套 mpv。
    fn start_probe(&mut self) {
        if self.playlist.items.is_empty() {
            return;
        }
        if self.prober.is_none() {
            self.prober = probe::Prober::spawn().ok();
        }
        if let Some(p) = &self.prober {
            p.queue(self.playlist.items.iter().map(|i| i.path.clone()).collect());
        }
    }

    /// 拖放会连着来好几条（一次拖 5 个文件就是 5 条消息），
    /// 攒到「安静了 120ms」再当成一批处理，不然每次拖多个都会被最后一条覆盖。
    fn flush_drops(&mut self) {
        let Some(at) = self.drop_at else { return };
        if at.elapsed() < DROP_SETTLE {
            return;
        }
        let paths = std::mem::take(&mut self.pending_drops);
        self.drop_at = None;
        if !paths.is_empty() {
            self.open_paths(paths);
        }
    }

    /// 把窗口调成和视频一样的比例，这样画面铺满、不留黑边。
    ///
    /// 全用物理像素算：视频 1 像素 = 屏幕 1 像素最清晰，
    /// 也省掉了 DPI 换算（SetWindowPos 本来就是物理像素）。
    fn fit_window_to_video(&mut self, vw: u32, vh: u32) {
        if self.fullscreen {
            return;
        }
        let Some(hwnd) = self.hwnd else { return };
        if win32::is_maximized(hwnd) {
            return;
        }
        if vw < 2 || vh < 2 {
            return;
        }

        let scale = win32::dpi_scale(hwnd) as f64;
        let min_w = (MIN_W as f64 * scale).round();
        let min_h = (MIN_H as f64 * scale).round();

        let mut w = vw as f64;
        let mut h = vh as f64;

        // 太小的视频别开成指甲盖（同时保证顶底栏不会被挤爆）
        if w < min_w || h < min_h {
            let k = (min_w / w).max(min_h / h);
            w *= k;
            h *= k;
        }

        let Some((ax, ay, aw, ah)) = win32::work_area(hwnd) else {
            return;
        };

        // 再大的也收进工作区（留 4% 边距，别顶到任务栏和屏幕边）
        let max_w = aw as f64 * 0.96;
        let max_h = ah as f64 * 0.96;
        if w > max_w || h > max_h {
            let k = (max_w / w).min(max_h / h);
            w *= k;
            h *= k;
        }

        let (w, h) = (w.round() as i32, h.round() as i32);

        // 窗口中心保持不动，只改大小和比例
        let (cx, cy, cw, ch) = win32::window_rect(hwnd).unwrap_or((ax, ay, w, h));
        let center = (cx + cw / 2, cy + ch / 2);
        let x = (center.0 - w / 2).clamp(ax, (ax + aw - w).max(ax));
        let y = (center.1 - h / 2).clamp(ay, (ay + ah - h).max(ay));

        win32::set_window_rect(hwnd, x, y, w, h);
    }

    // ---------------- 顶底栏的显隐逻辑 ----------------

    /// 鼠标是不是停在某条栏上（停上面时不应该收起，否则点不到按钮）
    fn cursor_over_bars(&self) -> bool {
        let Some(p) = self.cursor else { return false };
        p.y <= TOP_H + 8.0 || p.y >= self.win_size.height - BOTTOM_H - 8.0
    }

    fn want_bars(&self) -> bool {
        // 设置 / 详情抽屉开着的时候，栏必须留着（抽屉的上下边界就是顶底栏）
        if self.settings.open || self.info_open {
            return true;
        }
        // 抽屉还在滑出动画里也一样：栏先收了会露出一条缝
        if self.settings_anim > 0.01 || self.playlist_anim > 0.01 || self.info_anim > 0.01 {
            return true;
        }
        // 播放列表抽屉的高度是按「顶底栏都在」算出来的，栏收起来会露出缝隙
        if self.playlist.open {
            return true;
        }
        // 一条片子都没有（启动后的空白页 / 换片空档）时栏留着：那时候无所谓
        // 「让画面干净点」，而且栏上的打开、设置这些按钮还得能点。
        //
        // 音频要排除在外：它**永远**没有画面（`frame` 恒为 None），要是照这条
        // 判，放歌时两条栏就永远收不掉了。
        //
        // 暂停**不再**特殊对待：以前这里写的是 `!self.playing()`，于是只要暂停，
        // 两条栏就一直挂在上面，鼠标移开窗口也不会收。现在统一成一条规则 ——
        // 静置 2 秒就滑走，暂停也一样。
        if self.frame.is_none() && !self.is_audio() {
            return true;
        }
        // popover / 右键菜单展开时底栏必须留着，否则滑块会跟着一起收走
        self.volume_popover
            || self.sub_popover
            || self.speed_popover
            || self.context.is_some()
            || self.last_active.elapsed() < HIDE_AFTER
            || self.cursor_over_bars()
    }

    /// 有没有抽屉（播放列表 / 设置 / 详情）正开着或在动画里 —— 决定要不要铺那层透明 cover
    fn any_drawer(&self) -> bool {
        self.playlist.open
            || self.settings.open
            || self.info_open
            || self.playlist_anim > 0.002
            || self.settings_anim > 0.002
            || self.info_anim > 0.002
    }

    /// 按**名义上的** 16ms 推一帧。只有单测走这条路 —— 它们不关心真实时间，
    /// 要的是「第 N 帧状态可复现」。运行时走 `animate_with`，用真实帧间隔。
    #[cfg(test)]
    fn animate(&mut self) {
        self.animate_with(1.0 / 60.0);
    }

    /// 推进一帧动画。`dt` 是**这一帧真实过了多久**（秒）。
    ///
    /// 指数逼近（`1-exp(-dt/tau)`）和弹簧积分都按真实 `dt` 走，所以掉帧、或者定时器
    /// 早到/晚到，走完的时间都一样 —— 画出来的轨迹不会因为帧率抖而抖。
    fn animate_with(&mut self, dt: f32) {
        // 夹一下：窗口被挂起、隔几秒才回来时，一帧别把动画推到天上去
        let dt = dt.clamp(0.001, 0.05);
        let k = 1.0 - (-dt / ANIM_TAU).exp();

        let bars = if self.want_bars() { 1.0 } else { 0.0 };
        self.bars = approach(self.bars, bars, k);

        // 三条抽屉用同一套平滑：0 = 完全收在窗口右边外面，1 = 完全展开。
        // 用指数逼近而不是线性插值，走完的时间不依赖帧率，掉帧时也不会「跳」。
        let set = if self.settings.open { 1.0 } else { 0.0 };
        self.settings_anim = approach(self.settings_anim, set, k);

        let list = if self.playlist.open { 1.0 } else { 0.0 };
        self.playlist_anim = approach(self.playlist_anim, list, k);

        let detail = if self.info_open { 1.0 } else { 0.0 };
        self.info_anim = approach(self.info_anim, detail, k);

        // 换片加载中：转圈每帧往前走一点（一整圈 `spinner::TURN` 秒）。
        // `QP_SPIN=1` 是调试开关，让圈常驻，方便对样式（见 `spin_preview`）。
        if self.loading_since.is_some() || spin_preview() {
            self.spin = (self.spin + dt / spinner::TURN) % 1.0;
        }

        // 音频舞台的光晕：只有真的在放的时候才呼吸。暂停 / 没歌时相位不动，
        // 那圈光停在半亮的位置，看着是「静」，不是「死了」。
        if self.is_audio() && !self.paused {
            self.pulse = (self.pulse + dt / AUDIO_PULSE) % 1.0;
        }

        // 歌词：目标就是当前该唱的那一行（一句都没有时回顶上），由弹簧推过去。
        //
        // 这里**只换目标、不重置速度**（`set_target` 而不是 `snap`）：一句还没落定
        // 下一句就来了（快歌、或者用户连按几下方向键），整列带着动量接着滑，
        // 看着是"一路滚过去"；每句都清零的话会一顿一顿。
        let target = self.lyric_line.unwrap_or(0) as f32;
        if (target - self.lyric_scroll.value()).abs() > LYRIC_JUMP_ROWS {
            // 跳得太远（拖进度条 / 刚换歌）：直接落位，别让它飞
            self.lyric_scroll.snap(target);
        } else {
            self.lyric_scroll.set_target(target);
        }
        self.lyric_scroll.tick(dt);

        // 手动滚过去看后面的词：每帧把画面**钉住**（播放继续往前唱，视图不动），
        // 四秒不再滚就滑回正在唱的那一句。
        if let Some(pin) = self.lyric_pin {
            self.lyric_browse.set_target(pin - self.lyric_scroll.value());
        }
        if self
            .lyric_browse_at
            .is_some_and(|at| at.elapsed() > LYRIC_BROWSE_HOLD)
        {
            self.lyric_browse_at = None;
            self.lyric_pin = None;
            self.lyric_browse.set_target(0.0);
        }
        self.lyric_browse.tick(dt);
    }

    /// 换片转圈现在的不透明度（0 = 不画，1 = 完全显示）。
    ///
    /// 三件事叠在一起：
    /// - 有片子、还没出首帧、没报错 —— 否则不画；
    /// - 等过 `SPIN_DELAY` 才开始出现。本地小文件往往 60ms 就出画面了，门一开
    ///   就画会在每次切集时闪一下；
    /// - 出现之后再淡入 `SPIN_FADE` 秒。这样「刚好有点慢」的片子看到的是一圈
    ///   若有若无的弧，不会硬闪；真慢的时候才完整亮起来。
    fn spinner_opacity(&self) -> f32 {
        self.spinner_opacity_at(Instant::now())
    }

    /// 同上，但把「现在」当参数传进来 —— 单测里要给一个假的时刻才能验证
    /// 延迟和淡入。
    fn spinner_opacity_at(&self, now: Instant) -> f32 {
        if spin_preview() {
            return 1.0;
        }
        // 手上这一帧得是**当前这条片子**的才算「有画面」。换片空档里留着的是上一条
        // 的最后一帧（见 `play()` 里的 `stale_frame`），它只是在顶着不黑屏，
        // 不能让转圈永远不出现 —— 真加载得久了，还是要转起来告诉用户「在读」。
        if (self.frame.is_some() && !self.stale_frame)
            || self.is_audio()
            || self.error.is_some()
            || self.current.is_none()
        {
            return 0.0;
        }
        let Some(since) = self.loading_since else {
            return 0.0;
        };
        // 手上还顶着上一帧时，等得更久（原因见 `STALE_SPIN_DELAY`）
        let delay = if self.frame.is_some() {
            STALE_SPIN_DELAY
        } else {
            SPIN_DELAY
        };
        let waited = now.saturating_duration_since(since).saturating_sub(delay).as_secs_f32();
        (waited / SPIN_FADE).min(1.0)
    }

    /// 播放线程喊「现在是暂停/播放状态」，该不该采信。
    ///
    /// 返回 `None` = 这条快照的 paused 不能用（命令还在路上）。
    ///
    /// 背景：点和画面到 mpv 真的停住之间隔着一条队列，`self.paused` 已经在本地
    /// 翻过去了，队列里那几条旧快照还写着老值。照抄回去就是「按钮先翻回暂停、
    /// 下一帧再翻回来」——用户看到的就是闪一下。所以刚发过命令的那一小段时间
    /// 里，只认跟本地一致的值；等快照追上、或者超过 `PAUSE_ECHO` 兜底，恢复同步。
    fn resolve_paused(&mut self, snap_paused: bool) -> Option<bool> {
        let Some(sent_at) = self.pause_sent else {
            return Some(snap_paused);
        };
        // 快照追上了（哪怕只是「mpv 现在就是你说得那样」），从此不再拦
        if snap_paused == self.paused || sent_at.elapsed() >= PAUSE_ECHO {
            self.pause_sent = None;
            return Some(snap_paused);
        }
        None
    }

    /// 收下后台探测出来的时长，按路径填回列表。
    fn drain_probe(&mut self) {
        let got = match &self.prober {
            Some(p) => p.drain(),
            None => return,
        };
        for (path, secs) in got {
            self.playlist.fill_duration(&path, secs);
        }
    }

    /// 收后台线程读到的标签（封面 / 歌词 / 曲名）。
    fn drain_tags(&mut self) {
        let got = match &self.tag_loader {
            Some(l) => l.drain(),
            None => return,
        };
        for (path, mut tags) in got {
            // 只认当前这条：连点几首歌时，前一首的标签可能晚到，
            // 直接收下会把新歌的封面盖成上一首的。
            if self.current.as_deref() != Some(path.as_path()) {
                continue;
            }
            self.cover = tags
                .cover
                .take()
                .map(|c| iced::widget::image::Handle::from_rgba(c.width, c.height, c.rgba));
            self.tags = tags;
            // 新歌词就从当前播放位置重新对一次行，不然要等下一次 tick 才亮。
            // 位置也直接定位过去：不然第一句会从顶上滑下来，像放错了歌。
            self.lyric_line = self.tags.lyrics.as_ref().and_then(|l| l.index_at(self.position));
            self.lyric_scroll
                .snap(self.lyric_line.unwrap_or(0) as f32);
        }
    }

    /// 按当前播放位置更新「唱到第几行」。
    ///
    /// 放音频时每帧调一次；纯文本歌词 / 没有歌词时这里一直是 `None`。
    fn step_lyrics(&mut self) {
        let Some(lyrics) = &self.tags.lyrics else {
            self.lyric_line = None;
            return;
        };
        self.lyric_line = lyrics.index_at(self.position);
    }

    /// 滚轮滚歌词：在「跟着当前句」的偏移上叠一行数，两头都夹住。
    ///
    /// 允许的范围就是「第 0 行能滚到中线」到「最后一行能滚到中线」——
    /// 再往外滚就是一片空白，没意义。
    fn scroll_lyrics(&mut self, rows: f32) {
        let Some(meta::Lyrics::Synced(lines)) = &self.tags.lyrics else {
            return;
        };
        if lines.is_empty() || rows == 0.0 {
            return;
        }
        let scroll = self.lyric_scroll.value();
        let last = (lines.len() - 1) as f32;
        // 夹的是**显示位置**（scroll + 偏移），不是偏移本身：夹偏移的话，
        // 顶到边界之后播放再往前走一句，显示位置就又"漂"出边界了。
        let shown = self.lyric_pin.unwrap_or(scroll + self.lyric_browse.value());
        let next = (shown + rows).clamp(0.0, last);
        self.lyric_pin = Some(next);
        self.lyric_browse.set_target(next - scroll);
        self.lyric_browse_at = Some(Instant::now());
        self.touch();
    }

    /// 点了某一行歌词：跳到那一句唱的地方。
    ///
    /// 歌词位置直接 `snap` 过去 —— 用户点这句就是想看这句，让它从上一句慢慢滑过来
    /// 反而莫名其妙（而且可能跨十几行）。
    fn seek_to_lyric(&mut self, index: usize) {
        let Some(meta::Lyrics::Synced(lines)) = &self.tags.lyrics else {
            return;
        };
        let Some(line) = lines.get(index) else {
            return;
        };
        let at = line.at;
        self.touch();
        self.position = at;
        // 和方向键快进同一个套路：先记时刻，免得下一帧被还没跟上来的旧快照拽回去
        self.seek_at = Some(Instant::now());
        self.send(Cmd::Seek(at));
        self.lyric_line = Some(index);
        self.lyric_scroll.snap(index as f32);
        self.lyric_browse.snap(0.0);
        self.lyric_browse_at = None;
        self.lyric_pin = None;
    }

    fn touch(&mut self) {
        self.last_active = Instant::now();
    }

    /// 客户区坐标 `p` 落在歌词面板的哪一行上（不在面板里就是 `None`）。
    ///
    /// 几何必须和 `audio_stage` 摆歌词的那几行一致：左边 45% 给封面，歌词只占
    /// 右栏的一半宽度，面板高 52% 夹在 240~420，整个面板在窗口里垂直居中
    /// （`row![left, right].align_y(Center)` 套一层 `.center(Fill)`，两级居中
    /// 叠起来就等于窗口居中）。
    ///
    /// 只给「抽屉开着、歌词被 cover 盖住」那条路用，但它本身跟 UI 无关，
    /// 所以顺手也拿来做单测。
    fn lyric_line_at(&self, p: Point) -> Option<usize> {
        if !self.is_audio() {
            return None;
        }
        let Some(meta::Lyrics::Synced(lines)) = &self.tags.lyrics else {
            return None;
        };
        let (w, h) = (self.win_size.width, self.win_size.height);
        let right_w = w * 0.55;
        let text_w = right_w * 0.5;
        let panel_h = (h * 0.52).clamp(240.0, 420.0);
        let x0 = w * 0.45 + (right_w - text_w) * 0.5;
        let y0 = h * 0.5 - panel_h * 0.5;
        if p.x < x0 || p.x > x0 + text_w || p.y < y0 || p.y > y0 + panel_h {
            return None;
        }

        let row_h = LYRIC_ROW_H * (self.lyric_size / LYRIC_SIZE_IDLE);
        if row_h <= 0.0 {
            return None;
        }
        let scroll = self.lyric_scroll.value() + self.lyric_browse.value();
        let (first, pad) = lyric_layout(scroll, panel_h, row_h);
        let rel = p.y - y0 - pad;
        if rel < 0.0 {
            return None;
        }
        let index = first + (rel / row_h) as usize;
        (index < lines.len()).then_some(index)
    }

    /// 播放 / 暂停。键盘空格、点画面、点播放键都走这里。
    fn toggle_play(&mut self) {
        self.touch();
        // 还没开始放（打开文件夹之后停在这里）：按播放就从列表第一条起播。
        // 不这么做的话，这颗按钮在「从列表里挑一个」的界面上是颗死按钮，
        // 用户第一反应就是「坏了」。
        //
        // 换片空档（手上顶着上一帧的占位画面）也当「还没开始放」处理：
        // 那时候去翻 `paused` 只会给一个正在加载的 mpv 发个没意义的暂停命令。
        if (self.frame.is_none() || self.stale_frame) && !self.is_audio() {
            if self.current.is_none() && !self.playlist.items.is_empty() {
                self.play_index(0);
            }
            return;
        }
        self.paused = !self.paused;
        // 先记时刻再发命令：勾上之后 `tick()` 才不会拿旧快照把按钮翻回去
        self.pause_sent = Some(Instant::now());
        self.send(Cmd::Pause(self.paused));
    }

    /// 当前这条是不是纯音频（界面画法、加载判断都看它）。
    fn is_audio(&self) -> bool {
        self.current.as_ref().is_some_and(|p| audio::is_audio(p))
    }

    /// ← / → 快退 / 快进。
    ///
    /// 时长还没解出来（0）时直接不理：那时候 `position + delta` 夹到哪里都是错的，
    /// 硬跳会把播放头的解析工作打断，用户还看不到任何反馈。
    fn nudge(&mut self, delta: f64) {
        if self.current.is_none() || self.duration <= 0.0 {
            return;
        }
        // 节流：按住 → 时 winit 每 ~30ms 发一次 repeat，全发下去会把
        // mpv 的 seek 请求堆满（每次都从关键帧重新解码），画面会一顿一顿的。
        // 60ms 认一次，体感上就是「匀速滑动」。
        if self
            .seek_at
            .is_some_and(|t| t.elapsed() < Duration::from_millis(60))
        {
            return;
        }
        self.touch();
        let t = (self.position + delta).clamp(0.0, self.duration);
        self.position = t;
        self.seek_at = Some(Instant::now());
        self.send(Cmd::Seek(t));
    }

    /// ↑ / ↓ 调音量，一次 `VOL_STEP`，夹在 0~100。
    ///
    /// 已经到头（0 或 100）就不再下发命令 —— 连续按 ↑ 时不该每一下都往
    /// 播放线程塞一条没用的 `Volume`。
    fn nudge_volume(&mut self, delta: f64) {
        self.touch();
        let v = (self.volume + delta).clamp(0.0, 100.0);
        if (v - self.volume).abs() < f64::EPSILON {
            return;
        }
        self.volume = v;
        self.send(Cmd::Volume(v));
    }

    // ---------------- 状态同步 ----------------

    fn tick(&mut self) {
        self.step_resize();
        self.step_move();
        self.flush_drops();
        self.drain_probe();
        self.drain_tags();


        // 别的实例把路径转交过来了（又双击了一次 exe / 把视频「打开方式」选成本程序）：
        // 走和拖放完全一样的 open_paths，所以「第二次打开」的效果和拖进来一致。
        if let Some(paths) = pending_open() {
            self.open_paths(paths);
        }

        let mut new_video_size = None;
        // 播完了要不要接下一集（在下面统一处理，避免和 `player` 的可变借用打架）
        let mut advance_to = None;

        if let Some(player) = &self.player {
            // 只认自己这一代的快照。刚发完 Load / Stop 时，队列里躺着的那份
            // 还是上一条片子的（画面、进度、时长都是旧的），采信它就会
            // 「先黑一下、旧画面回来、再跳到新片」—— 就是切换时那一下闪。
            if let Some(snap) = player.poll().filter(|s| s.load_id == self.load_gen) {
                if let Some(f) = snap.frame {
                    self.frame = Some(f);
                    // 新片的首帧到了，手上这帧不再是「占位」
                    self.stale_frame = false;
                    // 首帧到手，转圈可以收了
                    self.loading_since = None;
                }
                if snap.audio {
                    // 音频没有「首帧」可等：引擎打开就代表加载完了。
                    // 不在这里清掉，转圈会一直挂在那块唱片上面。
                    self.loading_since = None;
                    // 从视频切到音频时，上一帧还挂在手上 —— 界面已经按 `is_audio()`
                    // 走到唱片那一支了，这帧没人画，白占着一块显存，扔掉。
                    self.frame = None;
                    self.stale_frame = false;
                }
                // 时长：
                // - 视频：mpv 说了算，报 0 时（还在解析）先用列表里探测到的值顶着
                // - 音频：**不能用 rodio 报的**。symphonia 对 mp3 的时长是估算的，
                //   实测一个 42.03 秒的 mp3 报成 46.29 秒（偏长 10%），进度条会
                //   停在 91% 就结束。探测线程那边是 mpv 读出来的真实时长，
                //   只要探到了就以它为准；没探到之前先用 rodio 的值顶一下。
                let probed = self
                    .playlist
                    .select
                    .and_then(|i| self.playlist.items.get(i))
                    .and_then(|it| it.duration)
                    .filter(|d| *d > 0.0);
                self.duration = if snap.audio {
                    probed.unwrap_or(snap.duration)
                } else if snap.duration > 0.0 {
                    snap.duration
                } else {
                    probed.unwrap_or(0.0)
                };
                // 正在放的这条自己也知道时长，顺手填回列表。
                // 音频不填：上面说了 rodio 的值偏长，别把列表里的准确值覆盖掉，
                // 等探测线程来写。
                if !snap.audio
                    && snap.duration > 0.0
                    && let Some(i) = self.playlist.select
                {
                    self.playlist.set_duration(i, Some(snap.duration));
                }
                // 没画面（空白页 / 还没出首帧）时不要跟着播放器走：
                // 空闲的 mpv 报告 pause=no，照抄会让空界面上的按钮显示成「暂停」。
                // 音频没有画面，但它的 paused 是准的（rodio 的 Sink 说了算），照收。
                if (self.frame.is_some() || snap.audio)
                    && let Some(p) = self.resolve_paused(snap.paused)
                {
                    self.paused = p;
                }
                self.open_ms = snap.open_ms;
                self.fps = snap.fps;

                let dragging = self
                    .seek_at
                    .map(|t| t.elapsed() < Duration::from_millis(600))
                    .unwrap_or(false);
                if !dragging && snap.position >= 0.0 {
                    self.position = snap.position;
                }

                if let Some(e) = snap.error {
                    self.error = Some(e);
                }

                new_video_size = snap.video_size;

                // 轨道表只增不减地跟着走：弹窗开着时字幕列表要实时反映
                self.tracks = snap.tracks;
                self.embedded_subs = snap.embedded_subs;
                self.sid = snap.sid;

                // 元信息：播放线程在首帧时读一次；换文件时它会被清成 None，
                // 详情抽屉于是回到「正在读取…」，不会残留上一个文件的数字。
                self.info = snap.media_info;

                // 这一集放完了：**用户勾了自动连播**且列表里还有下一条才接着放。
                // 只在 false → true 的那一下触发，之后 eof 一直是 true 也不会重复跳。
                let just_ended = snap.eof && !self.eof_prev;
                self.eof_prev = snap.eof;
                if just_ended
                    && self.settings.autoplay
                    && let Some(next) = self.playlist.select.map(|i| i + 1)
                    && next < self.playlist.items.len()
                {
                    advance_to = Some(next);
                }
            }
        }

        if let Some(next) = advance_to {
            self.play_index(next);
        }

        // 歌词跟着播放位置走（视频没有歌词，直接跳过）
        if self.is_audio() {
            self.step_lyrics();
        }

        // 拿到视频真实尺寸后按它的比例摆一次窗口（每个文件只摆一次）
        if let Some(size) = new_video_size {
            self.video_size = Some(size);
            if !self.sized_for_video {
                self.sized_for_video = true;
                self.fit_window_to_video(size.0, size.1);
            }
        }

        // 动画按真实帧间隔推进：定时器偶尔早到/晚到、或者掉一帧，动画速度都不变
        let dt = self
            .anim_last
            .map(|last| last.elapsed().as_secs_f32())
            .unwrap_or(1.0 / 60.0);
        self.anim_last = Some(Instant::now());
        self.animate_with(dt);
    }

    fn send(&self, cmd: Cmd) {
        if let Some(p) = &self.player {
            p.send(cmd);
        }
    }

    /// 拖边缘缩放：每个 tick 按鼠标位移重算窗口矩形，直接 SetWindowPos。
    ///
    /// 不走 `window::drag_resize`，因为它会进 Windows 的模态缩放循环，
    /// 把 UI 线程整个占住 —— 音频在放、画面却冻住。
    fn step_resize(&mut self) {
        let Some(drag) = self.drag else { return };
        let Some(hwnd) = self.hwnd else {
            self.drag = None;
            return;
        };

        // 全屏时窗口必须钉死在屏幕上：真进到缩放循环里，哪怕只动 1px，
        // 边缘也会立刻露出底下的桌面。进全屏之前已经按下的那次也算数，这里直接掐掉。
        if self.fullscreen {
            self.drag = None;
            return;
        }

        // 松手就结束（轮询按键状态，比等 ButtonReleased 稳）
        if !win32::left_down() {
            self.drag = None;
            return;
        }

        let (cx, cy) = win32::cursor_pos();
        let dx = cx - drag.start_cursor.0;
        let dy = cy - drag.start_cursor.1;
        let (sx, sy, sw, sh) = drag.start_rect;
        let dir = drag.direction;

        let west = matches!(
            dir,
            Direction::West | Direction::NorthWest | Direction::SouthWest
        );
        let east = matches!(
            dir,
            Direction::East | Direction::NorthEast | Direction::SouthEast
        );
        let north = matches!(
            dir,
            Direction::North | Direction::NorthEast | Direction::NorthWest
        );
        let south = matches!(
            dir,
            Direction::South | Direction::SouthEast | Direction::SouthWest
        );

        let mut x = sx;
        let mut y = sy;
        let mut w = sw;
        let mut h = sh;

        if west {
            x = sx + dx;
            w = sw - dx;
        }
        if east {
            w = sw + dx;
        }
        if north {
            y = sy + dy;
            h = sh - dy;
        }
        if south {
            h = sh + dy;
        }

        // 最小尺寸按物理像素卡（这里绕过了系统的 WM_GETMINMAXINFO）
        let scale = win32::dpi_scale(hwnd);
        let min_w = (MIN_W * scale).round() as i32;
        let min_h = (MIN_H * scale).round() as i32;

        if w < min_w {
            if west {
                x = sx + sw - min_w;
            }
            w = min_w;
        }
        if h < min_h {
            if north {
                y = sy + sh - min_h;
            }
            h = min_h;
        }

        let next = (x, y, w, h);
        if next != drag.applied {
            win32::set_window_rect(hwnd, x, y, w, h);
            self.drag = Some(DragResize {
                applied: next,
                ..drag
            });
        }
    }

    /// 按住顶栏挪窗口：每个 tick 按鼠标位移重算窗口位置，直接 SetWindowPos。
    ///
    /// 和 `step_resize` 同一套思路 —— 绝不能交给 `window::drag`，那个会进
    /// Windows 的模态移动循环，把 UI 线程占住，画面就停了（声音照旧）。
    fn step_move(&mut self) {
        let Some(m) = self.moving else { return };
        let Some(hwnd) = self.hwnd else {
            self.moving = None;
            return;
        };

        // 全屏时不许挪：挪一下屏幕就先空出一条桌面，跟「全屏」自相矛盾
        if self.fullscreen {
            self.moving = None;
            return;
        }

        // 松手就结束（轮询按键状态，比等 ButtonReleased 稳：
        // 鼠标移出窗口、或者被别的窗口抢走时也能正常收尾）
        if !win32::left_down() {
            self.moving = None;
            return;
        }

        let (cx, cy) = win32::cursor_pos();
        let x = m.start_pos.0 + (cx - m.start_cursor.0);
        let y = m.start_pos.1 + (cy - m.start_cursor.1);

        if (x, y) != m.applied {
            win32::set_window_rect(hwnd, x, y, m.size.0, m.size.1);
            self.moving = Some(DragMove {
                applied: (x, y),
                ..m
            });
            // 拖动过程中别让顶底栏因为「2 秒没动」收起来
            self.touch();
        }
    }

    fn update(&mut self, message: Message) -> Task<Message> {
        // 右键菜单弹着的时候点左键：只收起菜单，**不要**顺手把播放暂停掉
        let context_was_open = self.context.is_some();

        // 音量 popover 是浮层，点到别处就该收起。
        // 这几个消息要么来自 popover 自己（音量/开合），要么只是纯状态同步（tick、鼠标位置），
        // 其余一律视为「点了别处」。
        if self.volume_popover
            && !matches!(
                message,
                Message::Volume(_)
                    | Message::ToggleVolumePopover
                    | Message::ToggleMute
                    | Message::Tick
                    // 方向键是「用户在播放器上调」，不算点了别处：
                    // 尤其 ↑ / ↓ 调音量时，音量 popover 得开着才能看到数字在变
                    | Message::Nudge(_)
                    | Message::NudgeVolume(_)
                    // Esc 有自己的一套优先级，交给下面的 match 处理
                    | Message::Escape
                    | Message::MouseMoved(_)
                    | Message::MouseEntered
                    | Message::Resized(_)
                    // 启动阶段的自带消息，不是「点了别处」
                    | Message::GotWindow(_)
                    | Message::GotHwnd(_)
                    | Message::Noop
            )
        {
            self.volume_popover = false;
        }

        // 字幕 popover 同理：只有它自己的消息不算「点了别处」
        if self.sub_popover
            && !matches!(
                message,
                Message::ToggleSubtitlePopover
                    | Message::SelectSub(_)
                    | Message::PickSubtitle
                    | Message::PickedSubtitle(_)
                    | Message::RefreshTracks
                    | Message::Tick
                    // 方向键是「用户在播放器上调」，不算点了别处：
                    // 尤其 ↑ / ↓ 调音量时，音量 popover 得开着才能看到数字在变
                    | Message::Nudge(_)
                    | Message::NudgeVolume(_)
                    // Esc 有自己的一套优先级，交给下面的 match 处理
                    | Message::Escape
                    | Message::MouseMoved(_)
                    | Message::MouseEntered
                    | Message::Resized(_)
                    | Message::GotWindow(_)
                    | Message::GotHwnd(_)
                    | Message::Noop
            )
        {
            self.sub_popover = false;
        }

        // 速度 popover：同一套规则
        if self.speed_popover
            && !matches!(
                message,
                Message::ToggleSpeedPopover
                    | Message::SetSpeed(_)
                    | Message::Tick
                    // 方向键是「用户在播放器上调」，不算点了别处：
                    // 尤其 ↑ / ↓ 调音量时，音量 popover 得开着才能看到数字在变
                    | Message::Nudge(_)
                    | Message::NudgeVolume(_)
                    // Esc 有自己的一套优先级，交给下面的 match 处理
                    | Message::Escape
                    | Message::MouseMoved(_)
                    | Message::MouseEntered
                    | Message::Resized(_)
                    | Message::GotWindow(_)
                    | Message::GotHwnd(_)
                    | Message::Noop
            )
        {
            self.speed_popover = false;
        }

        // 右键菜单：除了菜单自己的动作，点哪儿都收起
        if self.context.is_some()
            && !matches!(
                message,
                Message::ContextMenu
                    | Message::RevealInExplorer
                    | Message::OpenInfo
                    | Message::Tick
                    // 方向键是「用户在播放器上调」，不算点了别处：
                    // 尤其 ↑ / ↓ 调音量时，音量 popover 得开着才能看到数字在变
                    | Message::Nudge(_)
                    | Message::NudgeVolume(_)
                    // Esc 有自己的一套优先级，交给下面的 match 处理
                    | Message::Escape
                    | Message::MouseMoved(_)
                    | Message::MouseEntered
                    | Message::Resized(_)
                    | Message::GotWindow(_)
                    | Message::GotHwnd(_)
                    | Message::Noop
            )
        {
            self.context = None;
        }

        match message {
            Message::Tick => self.tick(),

            Message::OpenDialog => {
                self.touch();
                // 从当前视频所在目录开始，比每次从「视频」库重新找一遍省事
                let start = self.current.as_ref().and_then(|p| p.parent()).map(|d| d.to_path_buf());
                return Task::perform(
                    async move {
                        let mut dialog = rfd::AsyncFileDialog::new();
                        if let Some(dir) = start {
                            dialog = dialog.set_directory(dir);
                        }
                        dialog
                            .add_filter("视频", &playlist::VIDEO_EXTS)
                            .add_filter("音频", &audio::AUDIO_EXTS)
                            .pick_file()
                            .await
                            .map(|f| f.path().to_path_buf())
                    },
                    Message::Picked,
                );
            }
            Message::Picked(Some(path)) => self.open_paths(vec![path]),
            Message::Picked(None) => {}
            Message::Dropped(path) => {
                self.pending_drops.push(path);
                self.drop_at = Some(Instant::now());
            }
            Message::PickedFolder(Some(dir)) => self.open_paths(vec![dir]),
            Message::PickedFolder(None) => {}
            Message::OpenDefaultApps => {
                self.touch();
                assoc::open_default_apps();
            }

            Message::TogglePlay => self.toggle_play(),
            Message::Nudge(delta) => self.nudge(delta),
            Message::NudgeVolume(delta) => self.nudge_volume(delta),
            Message::Seek(secs) => {
                self.position = secs;
                self.seek_at = Some(Instant::now());
                self.send(Cmd::Seek(secs));
            }
            Message::Volume(vol) => {
                self.volume = vol;
                self.send(Cmd::Volume(vol));
            }
            Message::Prev => {
                self.touch();
                // 只切上一条视频。没有上一条就什么都不做（不再退化成快退 10 秒）
                self.step_item(-1);
            }
            Message::Next => {
                self.touch();
                // 只切下一条视频。没有下一条就什么都不做（不再退化成快进 10 秒）
                self.step_item(1);
            }
            Message::ToggleMute => {
                self.touch();
                if self.volume > 0.0 {
                    self.muted_before = self.volume;
                    self.volume = 0.0;
                } else {
                    self.volume = if self.muted_before > 0.0 {
                        self.muted_before
                    } else {
                        80.0
                    };
                }
                self.send(Cmd::Volume(self.volume));
            }
            Message::ToggleVolumePopover => {
                self.touch();
                self.volume_popover = !self.volume_popover;
                self.speed_popover = false;
                self.context = None;
            }
            Message::ToggleTopmost => {
                self.touch();
                self.topmost = !self.topmost;
                if let Some(hwnd) = self.hwnd {
                    win32::set_topmost(hwnd, self.topmost);
                }
            }

            Message::ToggleFullscreen => {
                self.touch();
                self.fullscreen = !self.fullscreen;
                if let Some(id) = self.window {
                    let mode = if self.fullscreen {
                        iced::window::Mode::Fullscreen
                    } else {
                        iced::window::Mode::Windowed
                    };
                    return iced::window::set_mode(id, mode);
                }
            }
            Message::Escape => {
                // 浮层优先：右键菜单 → 各 popover → 各抽屉 → 退出全屏
                if self.context.is_some() {
                    self.context = None;
                } else if self.speed_popover {
                    self.speed_popover = false;
                } else if self.sub_popover {
                    self.sub_popover = false;
                } else if self.volume_popover {
                    self.volume_popover = false;
                } else if self.settings.open {
                    self.settings.close();
                    self.touch();
                } else if self.info_open {
                    self.info_open = false;
                    self.touch();
                } else if self.playlist.open {
                    self.touch();
                    self.playlist.open = false;
                } else if self.fullscreen {
                    self.touch();
                    self.fullscreen = false;
                    if let Some(id) = self.window {
                        return iced::window::set_mode(id, iced::window::Mode::Windowed);
                    }
                }
            }

            Message::GotWindow(id) => {
                self.window = id;
                if let Some(id) = id {
                    return Task::batch([
                        // 再小顶底栏就叠一起了
                        iced::window::set_min_size(id, Some(Size::new(560.0, 400.0))),
                        // 拿到 HWND 才能改原生窗口样式（圆角 / 可缩放边框）
                        iced::window::run(id, |w| {
                            use iced::window::raw_window_handle::RawWindowHandle;
                            match w.window_handle().map(|h| h.as_raw()) {
                                Ok(RawWindowHandle::Win32(x)) => Some(x.hwnd.get()),
                                _ => None,
                            }
                        })
                        .map(Message::GotHwnd),
                    ]);
                }
            }
            Message::GotHwnd(hwnd) => {
                self.hwnd = hwnd;
                if let Some(h) = hwnd {
                    win32::polish(h);
                    // 以窗口真实状态为准（比如被别的程序置顶过）
                    self.topmost = win32::is_topmost(h);
                }
            }

            Message::ResizeStart(direction) => {
                self.touch();
                // 全屏时窗口铺满屏幕，边缘就是屏幕边缘，不该拿来缩放
                if !self.fullscreen
                    && let Some(hwnd) = self.hwnd
                {
                    // 最大化状态下拖边缘：先还原，再从还原后的位置开始算
                    if win32::is_maximized(hwnd) {
                        win32::restore(hwnd);
                    }
                    if let Some(rect) = win32::window_rect(hwnd) {
                        self.drag = Some(DragResize {
                            direction,
                            start_cursor: win32::cursor_pos(),
                            start_rect: rect,
                            applied: rect,
                        });
                    }
                }
            }

            Message::WinDrag => {
                // 只记起点，剩下的交给 `step_move` 每个 tick 自己挪
                // （见 `DragMove` 的注释：`window::drag` 会卡住整个 UI 线程）
                //
                // 全屏时顶栏还能被唤出来，但它**不是**抓手：按住就别动窗口。
                if !self.fullscreen
                    && let Some(hwnd) = self.hwnd
                {
                    self.touch();
                    // 最大化时按原生拖动的手感先还原，再从还原后的位置开始挪
                    if win32::is_maximized(hwnd) {
                        win32::restore(hwnd);
                    }
                    if let Some((x, y, w, h)) = win32::window_rect(hwnd) {
                        self.moving = Some(DragMove {
                            start_cursor: win32::cursor_pos(),
                            start_pos: (x, y),
                            size: (w, h),
                            applied: (x, y),
                        });
                    }
                }
            }
            Message::WinMinimize => {
                if let Some(id) = self.window {
                    return iced::window::minimize(id, true);
                }
            }
            Message::WinMaximize => {
                // 全屏时窗口已经铺满整个屏幕，再叠一个「最大化」只会把两套状态
                // 搅在一起（退出全屏时还原成什么尺寸就说不准了），直接不理。
                if !self.fullscreen
                    && let Some(id) = self.window
                {
                    return iced::window::toggle_maximize(id);
                }
            }
            Message::WinClose => {
                if let Some(id) = self.window {
                    return iced::window::close(id);
                }
            }

            Message::MouseMoved(p) => {
                self.cursor = Some(p);
                self.touch();
            }
            Message::MouseEntered => {
                self.cursor = Some(Point::new(0.0, 0.0));
                self.touch();
            }
            Message::MouseLeft => {
                self.cursor = None;
                // 鼠标出去了，这次按下就不算数了（免得在外面松手又切一次暂停）
                self.stage_press = false;
                // 从「离开窗口这一刻」开始重新计时：2 秒后收起，
                // 而不是接着上一次鼠标移动的时间算（那样刚移出去就立刻收起）。
                self.touch();
            }

            // ---- 画面上的一次「按下-松手」 ----
            // 只有按下时鼠标确实在画面上（不是被顶底栏或弹窗吃掉的），松手才切播放。
            Message::StagePress => {
                if context_was_open {
                    // 这一下只是「点掉菜单」
                    self.stage_press = false;
                    self.touch();
                    return Task::none();
                }
                self.stage_press = true;
                self.touch();
            }
            Message::StageRelease => {
                let armed = self.stage_press;
                self.stage_press = false;
                // 松手时已经在栏上了（按住拖动到栏上再松手）也不算。
                //
                // 音频再挡一道：`view()` 那边音频分支压根没挂按下/松手，正常情况
                // 这里收不到 —— 但挡住它比「以后谁手滑又给挂回去、点一下歌就停了」
                // 便宜得多。歌不是画面，点它不该当播放开关。
                if armed && !self.is_audio() && !self.cursor_over_bars() {
                    self.toggle_play();
                }
            }

            // ---- 设置抽屉 ----
            Message::OpenSettings => {
                self.touch();
                // 再点一次同一颗齿轮就把抽屉收回去，和播放列表的行为一致
                self.settings.open = !self.settings.open;
                self.volume_popover = false;
                self.sub_popover = false;
                self.speed_popover = false;
                self.context = None;
                self.playlist.open = false;
                self.info_open = false;
            }
            Message::CloseSettings => {
                self.touch();
                self.settings.close();
            }
            Message::SettingsTab(tab) => {
                self.touch();
                self.settings.tab = tab;
            }
            Message::SetAutoplay(on) => {
                self.settings.autoplay = on;
            }
            Message::OpenRepo => {
                self.touch();
                assoc::open_url(REPO_URL);
            }
            Message::SetFit(fit) => {
                self.touch();
                self.settings.fit = fit;
            }
            Message::SetAspect(aspect) => {
                self.touch();
                self.settings.aspect = aspect;
            }
            Message::SetLook(field, value) => {
                self.settings.set_look(field, value);
            }
            Message::ResetLook => {
                self.settings.look = video::Look::default();
            }
            Message::ToggleSubtitlePopover => {
                self.touch();
                self.sub_popover = !self.sub_popover;
                self.speed_popover = false;
                self.context = None;
                if self.sub_popover {
                    // 四个浮层互斥：同屏只留一个，不然右边会叠成一摞
                    self.volume_popover = false;
                    self.settings.close();
                    self.playlist.open = false;
                    self.info_open = false;
                    self.sub_hint = None;
                    // 展开时重扫一次轨道表：可能刚换过文件，也可能是上次 sub-add 之后
                    self.send(Cmd::RefreshTracks);
                }
            }
            Message::RefreshTracks => {
                self.sub_hint = None;
                self.send(Cmd::RefreshTracks);
            }
            Message::SelectSub(id) => {
                self.sid = id;
                self.sub_hint = None;
                self.send(Cmd::SetSid(id));
            }
            Message::PickSubtitle => {
                self.touch();
                // 从视频所在目录起步：字幕通常就和视频放在一起，
                // 而且从「视频」库这类虚拟目录起步时，系统对话框不接受
                // 直接输入完整路径（会报「文件名无效」）。
                let start = self
                    .current
                    .as_ref()
                    .and_then(|p| p.parent())
                    .map(|d| d.to_path_buf());
                return Task::perform(
                    async move {
                        let mut dialog = rfd::AsyncFileDialog::new();
                        if let Some(dir) = start {
                            dialog = dialog.set_directory(dir);
                        }
                        dialog
                            .add_filter("字幕", &["srt", "ass", "ssa", "vtt", "sub", "idx", "sup"])
                            .pick_file()
                            .await
                            .map(|f| f.path().to_path_buf())
                    },
                    Message::PickedSubtitle,
                );
            }
            Message::PickedSubtitle(Some(path)) => {
                let name = path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| path.to_string_lossy().into_owned());
                self.sub_hint = Some(format!("已加载：{name}"));
                self.send(Cmd::SubAdd(path));
            }
            Message::PickedSubtitle(None) => {}

            // ---- 播放列表 ----
            Message::TogglePlaylist => {
                self.touch();
                self.playlist.open = !self.playlist.open;
                // 几个浮层同时开着会叠在一起，谁最后开听谁的
                self.volume_popover = false;
                self.sub_popover = false;
                self.speed_popover = false;
                self.context = None;
                self.settings.close();
                self.info_open = false;
            }
            Message::PlayIndex(i) => {
                self.touch();
                self.play_index(i);
            }
            Message::ClearPlaylist => {
                self.touch();
                // 只是把列表清空，正在放的视频不受影响
                self.playlist.clear();
                self.playlist.open = true;
                self.playlist.note = Some(String::from("列表已清空"));
                // 探测队列也一起丢掉，否则它还在后台读已经不在列表里的文件
                if let Some(p) = &self.prober {
                    p.queue(Vec::new());
                }
            }
            Message::PickFolder => {
                self.touch();
                let start = self
                    .current
                    .as_ref()
                    .and_then(|p| p.parent())
                    .map(|d| d.to_path_buf());
                return Task::perform(
                    async move {
                        let mut dialog = rfd::AsyncFileDialog::new();
                        if let Some(dir) = start {
                            dialog = dialog.set_directory(dir);
                        }
                        dialog.pick_folder().await.map(|f| f.path().to_path_buf())
                    },
                    Message::PickedFolder,
                );
            }

            // ---- 播放速度 ----
            Message::ToggleSpeedPopover => {
                self.touch();
                self.speed_popover = !self.speed_popover;
                if self.speed_popover {
                    // 同屏只留一个浮层
                    self.volume_popover = false;
                    self.sub_popover = false;
                    self.context = None;
                    self.settings.close();
                    self.playlist.open = false;
                    self.info_open = false;
                }
            }
            Message::SetSpeed(v) => {
                // 0.5–8.0，留两位小数就够（滑块的 step 是 0.1，预设也都是整齐的值）
                let v = v.clamp(SPEED_MIN, SPEED_MAX);
                self.speed = (v * 100.0).round() / 100.0;
                // 只下发一次：播放线程自己记着这个倍率，换集时会重新应用一遍
                self.send(Cmd::Speed(self.speed));
            }

            // ---- 右键菜单 ----
            Message::ContextMenu => {
                // 菜单弹在按下的那一点上（iced 的 on_right_press 不带坐标，
                // 用 CursorMoved 记下来的位置，误差一帧以内）
                self.context = Some(self.cursor.unwrap_or(Point::new(0.0, 0.0)));
                self.volume_popover = false;
                self.sub_popover = false;
                self.speed_popover = false;
                self.touch();
            }
            Message::RevealInExplorer => {
                self.context = None;
                // 抽屉里也有这颗按钮，但那里不关抽屉（用户可能还想接着看信息）
                if let Some(path) = &self.current
                    && let Err(e) = win32::reveal_in_explorer(path)
                {
                    self.error = Some(e);
                }
            }
            Message::OpenInfo => {
                self.touch();
                self.context = None;
                self.info_open = true;
                // 只留一条抽屉
                self.volume_popover = false;
                self.sub_popover = false;
                self.speed_popover = false;
                self.settings.close();
                self.playlist.open = false;
                // 时间/码率这类属性在播放过程中会更准，展开时重读一遍
                self.send(Cmd::RefreshInfo);
            }
            Message::CloseInfo => {
                self.touch();
                self.info_open = false;
            }

            Message::CoverClick => {
                // 点了抽屉外面那层透明 cover。
                //
                // 为什么不干脆把歌词层摆到 cover 上面：歌词面板是靠
                // `row![left, right].align_y(Center)` 两级居中摆出来的，改成绝对
                // 定位就得在渲染时自己算 y，字号一变、窗口一缩就容易对不齐。
                // 与其挪布局，不如在这里按同一套几何反算一次点击落在哪一行。
                let hit = self.cursor.and_then(|p| self.lyric_line_at(p));
                if let Some(index) = hit {
                    self.seek_to_lyric(index);
                }
                self.touch();
                self.playlist.open = false;
                self.settings.close();
                self.info_open = false;
                self.volume_popover = false;
                self.sub_popover = false;
                self.speed_popover = false;
                self.context = None;
            }

            Message::Resized(s) => {
                self.win_size = s;
                self.touch();
            }
            Message::Modifiers(ctrl) => self.ctrl = ctrl,
            Message::LyricWheel(rows) => {
                if self.ctrl {
                    // 按着 Ctrl：调字号。向上滚放大（和浏览器一致），一格 0.5pt，
                    // 两头都夹住 —— 再小看不清，再大一行放不下几个字。
                    // 注意 `rows` 是"显示位置增量"，向上滚是负的，所以这里取反。
                    self.lyric_size = (self.lyric_size - rows * LYRIC_SIZE_STEP)
                        .clamp(LYRIC_SIZE_MIN, LYRIC_SIZE_MAX);
                } else {
                    self.scroll_lyrics(rows);
                }
            }
            Message::LyricSeek(index) => self.seek_to_lyric(index),
            // 栏 / 覆盖层故意「吃掉」按下事件用的占位消息，到这里什么也不做
            Message::Noop => {}
        }
        Task::none()
    }

    fn subscription(&self) -> Subscription<Message> {
        Subscription::batch([
            iced::time::every(Duration::from_millis(16)).map(|_| Message::Tick),
            iced::event::listen_with(|event, _status, _id| match event {
                iced::Event::Window(iced::window::Event::FileDropped(p)) => {
                    Some(Message::Dropped(p))
                }
                iced::Event::Window(iced::window::Event::Resized(s)) => Some(Message::Resized(s)),
                iced::Event::Mouse(iced::mouse::Event::CursorMoved { position }) => {
                    Some(Message::MouseMoved(position))
                }
                iced::Event::Mouse(iced::mouse::Event::CursorEntered) => {
                    Some(Message::MouseEntered)
                }
                iced::Event::Mouse(iced::mouse::Event::CursorLeft) => Some(Message::MouseLeft),
                // 只关心 Ctrl：它决定歌词区的滚轮是「调字号」还是「滚歌词」
                iced::Event::Keyboard(iced::keyboard::Event::ModifiersChanged(m)) => {
                    Some(Message::Modifiers(m.control()))
                }
                iced::Event::Keyboard(iced::keyboard::Event::KeyPressed { key, repeat, .. }) => {
                    match key {
                        // 空格是「切一下」，按住不放会疯狂切，所以只认第一次
                        keyboard::Key::Named(keyboard::key::Named::Space) if !repeat => {
                            Some(Message::TogglePlay)
                        }
                        keyboard::Key::Named(keyboard::key::Named::Escape) => Some(Message::Escape),
                        // 方向键：左右跳进度，上下调音量。
                        // 这几条**故意**连 repeat 一起收：按住 → 应该一路快进，
                        // 按住 ↑ 应该匀速升音量，这才是播放器该有的手感。
                        // 进度那边有 60ms 的节流（见 `nudge`），不会把 seek 请求堆满。
                        keyboard::Key::Named(keyboard::key::Named::ArrowRight) => {
                            Some(Message::Nudge(SEEK_STEP))
                        }
                        keyboard::Key::Named(keyboard::key::Named::ArrowLeft) => {
                            Some(Message::Nudge(-SEEK_STEP))
                        }
                        keyboard::Key::Named(keyboard::key::Named::ArrowUp) => {
                            Some(Message::NudgeVolume(VOL_STEP))
                        }
                        keyboard::Key::Named(keyboard::key::Named::ArrowDown) => {
                            Some(Message::NudgeVolume(-VOL_STEP))
                        }
                        _ => None,
                    }
                }
                _ => None,
            }),
        ])
    }

    // ---------------- 视图 ----------------

    fn view(&self) -> Element<'_, Message> {
        // 字体排查用：QP_FONT_TEST=1 启动时显示一张字样对照表（见 font_specimen）
        if std::env::var_os("QP_FONT_TEST").is_some() {
            return font_specimen();
        }

        let stage: Element<'_, Message> = if let Some(err) = &self.error {
            self.error_panel(err)
        } else if let Some(f) = &self.frame {
            let picture: Element<'_, Message> = iced::widget::shader(VideoProgram {
                frame: Some(f.clone()),
                fit: self.settings.fit,
                aspect: self.settings.aspect,
                look: self.settings.look,
            })
            .width(Fill)
            .height(Fill)
            .into();

            // 换片空档：这里是上一条的最后一帧在顶着（`stale_frame`）。
            // 真加载得久的话，转圈会从 0 淡进来，同时压一层暗色 ——
            // 不然「一动不动的高亮画面 + 一个白圈」看着像卡死了。
            let o = self.spinner_opacity();
            let body: Element<'_, Message> = if o > 0.0 {
                stack![
                    picture,
                    container(spinner::view(self.spin, o))
                        .center(Fill)
                        .style(move |_| container::Style {
                            background: Some(Background::Color(Color::from_rgba(
                                0.0,
                                0.0,
                                0.0,
                                0.55 * o,
                            ))),
                            ..container::Style::default()
                        }),
                ]
                .into()
            } else {
                picture
            };

            // 点画面 = 暂停/继续。必须 on_press + on_release 成对使用：
            // 只挂 on_release 的话，点顶底栏（它们的按下会被栏吃掉、松手却没有）
            // 松手时画面这层照样会收到 release，于是「点一下栏就暂停」。
            mouse_area(body)
                .interaction(mouse::Interaction::Idle)
                .on_press(Message::StagePress)
                .on_release(Message::StageRelease)
                // 右键菜单：给了 `on_right_press` 之后这一层才会捕获右键，
                // 而左键的按下/松手完全不受影响（pause 照旧走 StagePress/StageRelease）。
                .on_right_press(Message::ContextMenu)
                .into()
        } else if self.is_audio() {
            // 纯音频：没有画面可画，给它一块自己的静物界面。
            //
            // **故意不挂 on_press / on_release**：这里不是画面，点空白处不该
            // 把歌暂停掉 —— 想拖动歌词看一眼、点在封面上，都不该打断播放。
            // 暂停有底栏那颗按钮（还有空格键），不用再拿整块舞台当开关。
            mouse_area(self.audio_stage())
                .interaction(mouse::Interaction::Idle)
                .on_right_press(Message::ContextMenu)
                .into()
        } else if self.current.is_some() || spin_preview() {
            // 换片空档：这一条已经在开了，但首帧还没到。
            // 圈出现之前先留一块纯黑 —— 不然会闪一下下面的「把视频拖进来」空白页，
            // 看着像程序把片子丢了。
            let opacity = self.spinner_opacity();
            let center: Element<'_, Message> = if opacity > 0.0 {
                spinner::view(self.spin, opacity)
            } else {
                container(space()).into()
            };

            container(center)
                .center(Fill)
                .style(|_| container::Style {
                    background: Some(Background::Color(Color::BLACK)),
                    ..container::Style::default()
                })
                .into()
        } else if !self.playlist.items.is_empty() {
            // 列表已经装好了，但一条都还没开始放（打开文件夹后的默认状态）。
            // 这里不用再把「拖文件进来」摆一遍 —— 用户刚拖完，那样等于当没收到。
            self.pick_stage()
        } else {
            container(
                column![
                    iced::widget::image(iced::widget::image::Handle::from_rgba(
                        LOGO_BIG_SRC,
                        LOGO_BIG_SRC,
                        LOGO_BIG_RGBA,
                    ))
                    .width(Length::Fixed(LOGO_HERO))
                    .height(Length::Fixed(LOGO_HERO)),
                    text("把视频、音频或文件夹拖进来")
                        .font(icon::UI_FONT)
                        .size(20)
                        .color(icon::TEXT_DIM),
                    text("拖文件夹会把里面的视频和音频全部排进播放列表")
                        .font(icon::UI_FONT)
                        .size(14)
                        .color(icon::TEXT_FAINT),
                    row![
                        pill_button("打开文件", Message::OpenDialog),
                        pill_button("打开文件夹", Message::PickFolder),
                        pill_button("设为默认播放器", Message::OpenDefaultApps),
                    ]
                    .spacing(10)
                    .align_y(Alignment::Center),
                ]
                .spacing(12)
                .align_x(Alignment::Center),
            )
            .center(Fill)
            .style(|_| container::Style {
                background: Some(Background::Color(Color::BLACK)),
                ..container::Style::default()
            })
            .into()
        };

        // 收起进度：0 = 完全露出，1 = 完全移出画面
        let off = 1.0 - self.bars;
        let top_shift = -(TOP_H + 6.0) * off;
        let bottom_shift = (BOTTOM_H + 6.0) * off;

        let mut layers = stack![stage];

        // 全屏透明 cover：只吃点击，不画任何东西。点它就收起抽屉 / 浮层。
        if self.any_drawer() {
            layers = layers.push(self.cover_layer());
        }

        // 底栏压在 cover **上面**。
        //
        // 以前它铺在 cover 底下（想着「抽屉开着时点底栏那一带先关抽屉」），结果
        // 抽屉一开，进度条、播放键、歌词点击**全被 cover 吃掉** —— 用户看到的就是
        // 「播放时点进度条没反应、点歌词也不跳」。现在反过来：播放控制任何时候
        // 都能直接按，点别处的空白照样关抽屉。
        //
        // 抽屉本身上下让出了 `BOTTOM_H`，跟底栏不会重叠，所以顺序换成这样也不会
        // 挡住抽屉底部的按钮。
        layers = layers.push(
            Float::new(self.bottom_bar())
                .translate(move |_: Rectangle, _: Rectangle| Vector::new(0.0, bottom_shift)),
        );

        // 三条右侧抽屉：夹在 cover 和顶栏之间。
        // 上下让出 TOP_H / BOTTOM_H，抽屉不会盖住窗口按钮和播放控制；
        // 抽屉开着时 `want_bars()` 恒为 true，所以这两块留白永远对得上。
        //
        // 位置动画走 `Float::translate`：0 → 1 的进度乘上面板宽度就是「收在右边外面」
        // 到「贴边」的位移。用 Float 而不是给容器加 padding，是因为它只影响绘制和
        // 命中区域，不参与布局，动画中间不会把下面的控件挤来挤去。

        if self.playlist_anim > 0.002 {
            let shift = (playlist::W + DRAWER_LEAD) * (1.0 - self.playlist_anim);
            layers = layers.push(
                Float::new(self.drawer_layer(playlist::panel(&self.playlist), playlist::W))
                    .translate(move |_, _| Vector::new(shift, 0.0)),
            );
        }

        if self.settings_anim > 0.002 {
            let shift = (settings::W + DRAWER_LEAD) * (1.0 - self.settings_anim);
            layers = layers.push(
                Float::new(self.drawer_layer(settings::drawer(&self.settings), settings::W))
                    .translate(move |_, _| Vector::new(shift, 0.0)),
            );
        }

        if self.info_anim > 0.002 {
            let shift = (info::W + DRAWER_LEAD) * (1.0 - self.info_anim);
            let panel = info::drawer(self.info.as_ref(), &self.tracks, &self.title);
            layers = layers.push(
                Float::new(self.drawer_layer(panel, info::W))
                    .translate(move |_, _| Vector::new(shift, 0.0)),
            );
        }

        // 顶栏最后压在 cover 上面：抽屉开着也能拖动窗口、点最小化/关闭
        layers = layers.push(
            Float::new(self.top_bar())
                .translate(move |_: Rectangle, _: Rectangle| Vector::new(0.0, top_shift)),
        );

        // popover / 右键菜单压在底栏上面（但仍在缩放热区下面），
        // 底栏正在收起动画里就不画，免得浮层留在半空中
        if self.volume_popover && self.bars > 0.05 {
            layers = layers.push(self.volume_popover_panel());
        }
        if self.sub_popover && self.bars > 0.05 {
            layers = layers.push(self.subtitle_popover_panel());
        }
        if self.speed_popover && self.bars > 0.05 {
            layers = layers.push(self.speed_popover_panel());
        }
        if let Some(at) = self.context {
            layers = layers.push(self.context_menu(at));
        }

        // 缩放热区必须压在最后一层，否则会被顶底栏的拖拽热区吃掉。
        // 顺序：四条边 → 四个角（角放最后，优先级更高）。
        //
        // 全屏时这整组都不挂：那时窗口就是整个屏幕，没有「边缘」可拉，
        // 留着只会让光标贴到屏幕边上变成双向箭头，还什么都拉不动。
        let layers = if self.fullscreen {
            layers
        } else {
            layers
            .push(edge_zone(
                Direction::North,
                Horizontal::Center,
                Vertical::Top,
                [0, EDGE as u16],
                Fill,
                Length::Fixed(EDGE),
            ))
            .push(edge_zone(
                Direction::South,
                Horizontal::Center,
                Vertical::Bottom,
                [0, EDGE as u16],
                Fill,
                Length::Fixed(EDGE),
            ))
            .push(edge_zone(
                Direction::West,
                Horizontal::Left,
                Vertical::Center,
                [EDGE as u16, 0],
                Length::Fixed(EDGE),
                Fill,
            ))
            .push(edge_zone(
                Direction::East,
                Horizontal::Right,
                Vertical::Center,
                [EDGE as u16, 0],
                Length::Fixed(EDGE),
                Fill,
            ))
            .push(edge_zone(
                Direction::NorthWest,
                Horizontal::Left,
                Vertical::Top,
                [0, 0],
                Length::Fixed(EDGE),
                Length::Fixed(EDGE),
            ))
            .push(edge_zone(
                Direction::NorthEast,
                Horizontal::Right,
                Vertical::Top,
                [0, 0],
                Length::Fixed(EDGE),
                Length::Fixed(EDGE),
            ))
            .push(edge_zone(
                Direction::SouthWest,
                Horizontal::Left,
                Vertical::Bottom,
                [0, 0],
                Length::Fixed(EDGE),
                Length::Fixed(EDGE),
            ))
            .push(edge_zone(
                Direction::SouthEast,
                Horizontal::Right,
                Vertical::Bottom,
                [0, 0],
                Length::Fixed(EDGE),
                Length::Fixed(EDGE),
            ))
        };

        let layers: Element<'_, Message> = layers.width(Fill).height(Fill).into();

        // 纯黑兜底：视频没铺满、或者还没加载出画面时，露出来的就是真黑
        container(layers)
            .width(Fill)
            .height(Fill)
            .style(black_style)
            .into()
    }

    /// 「列表里已经有东西了、但一条都还没开始放」的舞台。
    ///
    /// 打开文件夹后停在这里：右边抽屉里是刚扫出来的列表，用户点哪条就放哪条。
    /// 不能沿用下面那张「把文件拖进来」的空白页 —— 用户刚拖完，那样等于当没收到。
    fn pick_stage(&self) -> Element<'_, Message> {
        let n = self.playlist.items.len();
        container(
            column![
                iced::widget::image(iced::widget::image::Handle::from_rgba(
                    LOGO_BIG_SRC,
                    LOGO_BIG_SRC,
                    LOGO_BIG_RGBA,
                ))
                .width(Length::Fixed(LOGO_HERO))
                .height(Length::Fixed(LOGO_HERO)),
                text("从列表里挑一个开始播放")
                    .font(icon::UI_FONT)
                    .size(20)
                    .color(icon::TEXT_DIM),
                text(format!("已经装进来 {n} 项 · 在右侧列表里点一条就播"))
                    .font(icon::UI_FONT)
                    .size(14)
                    .color(icon::TEXT_FAINT),
            ]
            .spacing(12)
            .align_x(Alignment::Center),
        )
        .center(Fill)
        .style(|_| container::Style {
            background: Some(Background::Color(Color::BLACK)),
            ..container::Style::default()
        })
        .into()
    }

    /// 纯音频的舞台：左边封面，右边歌词。
    ///
    /// 音频没有画面，走不到上面那条 shader 分支；这里给一块安静的界面顶上。
    /// 播放时封面外面那圈光会缓慢呼吸（相位在 `animate` 里推进），暂停就停住 ——
    /// 「在放」和「停着」不用看按钮也能一眼分出来。
    fn audio_stage(&self) -> Element<'_, Message> {
        // 0..1 之间正弦来回走。暂停时固定在 0.5，留一点微光，别整个死掉
        let breath = if self.paused {
            0.5
        } else {
            0.5 + 0.5 * (self.pulse * std::f32::consts::TAU).sin()
        };
        let glow = 0.04 + 0.09 * breath;

        // 舞台横向一刀切：封面那块占 45%，歌词容器占 55%。
        let left_w = self.win_size.width * 0.45;
        let right_w = self.win_size.width * 0.55;

        // 封面在它那 45% 里再缩一圈（留白），同时受高度限制：
        // 下面还要放曲名和歌手，别把两行字挤出屏幕。
        let room = (self.win_size.height - TOP_H - BOTTOM_H - 110.0).max(140.0);
        let side = (left_w * 0.78).clamp(120.0, ART_SIZE).min(room);

        // 歌词容器比封面高不少：上下多出来的那截正好给歌词做渐变留白。
        let panel_h = (self.win_size.height * 0.52).clamp(240.0, 420.0);
        // 歌词正文只占容器宽度的一半，居中 —— 一行太长反而不好跟
        let text_w = right_w * 0.5;

        // 有内嵌封面就贴封面；没有还是原来那只猫
        let art: Element<'_, Message> = match &self.cover {
            Some(handle) => iced::widget::image(handle.clone())
                .width(Length::Fixed(side))
                .height(Length::Fixed(side))
                .content_fit(iced::ContentFit::Cover)
                .border_radius(16.0)
                .into(),
            None => container(
                iced::widget::image(iced::widget::image::Handle::from_rgba(
                    LOGO_BIG_SRC,
                    LOGO_BIG_SRC,
                    LOGO_BIG_RGBA,
                ))
                .width(Length::Fixed(side * 0.46))
                .height(Length::Fixed(side * 0.46)),
            )
            .width(Length::Fixed(side))
            .height(Length::Fixed(side))
            .align_x(Alignment::Center)
            .align_y(Alignment::Center)
            .style(|_| container::Style {
                background: Some(Background::Color(Color::from_rgb(0.10, 0.11, 0.13))),
                border: Border {
                    color: Color::from_rgba(1.0, 1.0, 1.0, 0.07),
                    width: 1.0,
                    radius: 16.0.into(),
                },
                ..container::Style::default()
            })
            .into(),
        };

        // 光晕套在外面一层：有没有真封面都要呼吸
        let art = container(art)
            .width(Length::Fixed(side))
            .height(Length::Fixed(side))
            .style(move |_| container::Style {
                // 用「模糊的白影」当光晕：iced 没有径向渐变，这层柔光是代价最小
                // 又最接近的做法，呼吸起来也不突兀。
                shadow: Shadow {
                    color: Color::from_rgba(1.0, 1.0, 1.0, glow),
                    offset: Vector::ZERO,
                    blur_radius: 46.0,
                },
                ..container::Style::default()
            });

        // 标签里有曲名就用它，没有退回文件名（`self.title` 一直是文件名）
        let name = self.tags.title.as_deref().unwrap_or(&self.title);
        let ext = self
            .current
            .as_ref()
            .and_then(|p| p.extension())
            .map(|e| e.to_string_lossy().to_uppercase())
            .unwrap_or_else(|| String::from("AUDIO"));
        let state = if self.paused { "已暂停" } else { "正在播放" };
        let sub = match &self.tags.artist {
            Some(artist) => format!("{artist} · {ext} · {state}"),
            None => format!("{ext} · {state}"),
        };

        let info = column![
            text(ui::elide(name, side + 20.0, 17.0))
                .font(icon::UI_BOLD)
                .size(17.0)
                .color(icon::TEXT),
            text(ui::elide(&sub, side + 20.0, 12.5))
                .font(icon::UI_FONT)
                .size(12.5)
                .color(icon::TEXT_FAINT),
        ]
        .spacing(6)
        .align_x(Alignment::Center);

        let left = column![art, info]
            .spacing(20)
            .width(Length::Fixed(left_w))
            .align_x(Alignment::Center);

        let right = container(lyrics_view(
            self.tags.lyrics.as_ref(),
            self.lyric_line,
            // 显示位置 = 跟随当前句的弹簧 + 用户滚轮叠的偏移
            self.lyric_scroll.value() + self.lyric_browse.value(),
            text_w,
            panel_h,
            self.lyric_size,
        ))
        .width(Length::Fixed(right_w))
        .height(Length::Fixed(panel_h))
        .align_x(Horizontal::Center)
        .align_y(Vertical::Center);

        container(row![left, right].align_y(Alignment::Center))
            .center(Fill)
            .style(|_| container::Style {
                background: Some(Background::Color(AUDIO_BG)),
                ..container::Style::default()
            })
            .into()
    }

    fn error_panel<'a>(&'a self, err: &'a str) -> Element<'a, Message> {
        container(
            column![
                text("播放失败")
                    .font(icon::UI_FONT)
                    .size(17)
                    .color(icon::TEXT),
                text(err).font(icon::UI_FONT).size(13).color(icon::TEXT_DIM),
                button(text("选择文件").font(icon::UI_FONT).size(13))
                    .on_press(Message::OpenDialog),
            ]
            .spacing(14)
            .align_x(Alignment::Center)
            .max_width(720.0),
        )
        .center(Fill)
        .padding(40)
        .style(|_| container::Style {
            background: Some(Background::Color(Color::BLACK)),
            ..container::Style::default()
        })
        .into()
    }

    fn top_bar(&self) -> Element<'_, Message> {
        // 左：logo + 程序名（整块可点，点一下切换窗口置顶）
        //
        // logo 这个字形在字体 em 框里本身偏上（实测比文字中心高 2px），
        // 加 4px 上 padding 让它在居中后补回来，跟文字对齐。
        //
        // 用 button 包起来：按钮会吃掉 ButtonPressed，外层顶栏那层
        // `mouse_area.on_press(WinDrag)` 就收不到事件，点 logo 不会变成拖窗口。
        let pinned = self.topmost;

        let mut brand = row![
            // logo 用真实的 PNG（编译期转成 RGBA 内嵌），不再是图标字体里的字形
            iced::widget::image(iced::widget::image::Handle::from_rgba(
                LOGO_SRC,
                LOGO_SRC,
                LOGO_RGBA,
            ))
            .width(Length::Fixed(LOGO_BAR))
            .height(Length::Fixed(LOGO_BAR)),
            text("Maneki")
                .font(icon::UI_BOLD)
                .size(13)
                .color(icon::TEXT),
        ]
        .spacing(7)
        .align_y(Alignment::Center);

        // 置顶时名字后面点一颗小圆点，作为常驻状态提示
        if pinned {
            brand = brand.push(
                container(space())
                    .width(Length::Fixed(5.0))
                    .height(Length::Fixed(5.0))
                    .style(|_| container::Style {
                        background: Some(Background::Color(icon::ACCENT)),
                        border: Border {
                            radius: 3.0.into(),
                            ..Border::default()
                        },
                        ..container::Style::default()
                    }),
            );
        }

        let brand = button(container(brand).padding(Padding {
            top: 3.0,
            right: 9.0,
            bottom: 3.0,
            left: 4.0,
        }))
        .padding(0)
        .on_press(Message::ToggleTopmost)
        .style(move |t, status| brand_button(t, status, pinned));

        let left = container(brand)
            .width(Length::Fixed(150.0))
            .align_x(Horizontal::Left);

        // 中：文件名
        let middle = container(
            text(&self.title)
                .font(icon::UI_FONT)
                .size(14.5)
                .color(icon::TEXT),
        )
        .width(Fill)
        .align_x(Horizontal::Center);

        // 右：窗口控制
        let right = container(
            row![
                window_button(icon::MINIMIZE, Message::WinMinimize, false),
                window_button(icon::MAXIMIZE, Message::WinMaximize, false),
                window_button(icon::CLOSE, Message::WinClose, true),
            ]
            .spacing(2)
            .align_y(Alignment::Center),
        )
        .width(Length::Fixed(150.0))
        .align_x(Horizontal::Right);

        // 整条顶栏（含上下留白）都要能吃下按下事件：
        // 一是到处都能拖动窗口，二是不让点击穿透到下面的画面去暂停播放。
        // 里面的按钮自己会先捕获事件，所以点最小化/最大化/关闭仍然只做那件事。
        let strip = container(
            row![left, middle, right]
                .align_y(Alignment::Center)
                .padding([0, 12]),
        )
        .width(Fill)
        .height(TOP_H)
        .align_y(Vertical::Center)
        .style(bar_style);

        container(
            mouse_area(strip)
                .on_press(Message::WinDrag)
                .on_double_click(Message::WinMaximize),
        )
        .width(Fill)
        .height(Fill)
        .align_top(Fill)
        .into()
    }

    fn bottom_bar(&self) -> Element<'_, Message> {
        let total = if self.duration > 0.0 {
            self.duration
        } else {
            1.0
        };

        // 进度行
        //
        // 小圆圈（slider 手柄）默认不画：`radius: 0` 时手柄宽高都是 0，
        // 轨道本身照常可见。鼠标移到进度条上（Status::Hovered）或正在拖
        // （Dragged）才把手柄放出来，同时轨道略微加粗一点作为「可以拖」的反馈。
        let progress = row![
            text(fmt_time(self.position))
                .font(icon::UI_FONT)
                .size(13)
                .color(icon::TEXT_DIM),
            slider(0.0..=total, self.position, Message::Seek)
                .width(Fill)
                // 高度只决定**命中区**：iced 的 slider 把轨道画在 bounds 的正中，
                // 手柄也跟着居中，所以 26 跟 18 看起来一模一样，但可点范围从
                // 上下各 9px 变成各 13px —— 手一抖就点空的情况少很多。
                .height(26.0)
                .style(progress_slider_style),
            text(fmt_time(self.duration))
                .font(icon::UI_FONT)
                .size(13)
                .color(icon::TEXT_DIM),
        ]
        .spacing(10)
        .align_y(Alignment::Center);

        // 中间：上一集 / 播放暂停 / 下一集
        // 一条都没加载时（打开文件夹后的「挑一个」界面）画的必须是播放三角，
        // 暂停图标会让人以为正放着。
        let idle = self.current.is_none();
        let center = row![
            icon_button(icon::PREV, 20.0, Message::Prev),
            play_button(self.paused || idle),
            icon_button(icon::NEXT, 20.0, Message::Next),
        ]
        .spacing(14)
        .align_y(Alignment::Center);

        // 左侧按钮组（从左到右）：播放速度、音量、字幕。
        //
        // 播放速度不再是第一颗：音量在最左下角，速度紧挨着它。
        // 点速度键弹出「预设 + 自定义滑块」的 popover。音量和字幕的 popover
        // 也都从这里往左上展开 —— 它们都属于「播放中随手调」的东西。
        let volume_color = if self.volume <= 0.0 {
            icon::TEXT_FAINT
        } else {
            icon::TEXT
        };
        // 字幕键平时是白的；当前选了某条字幕轨就亮成主题色，一眼能看出「有字幕在跑」
        let sub_color = if self.sid.is_some() {
            icon::ACCENT
        } else {
            icon::TEXT
        };

        let left_group = row![
            icon_button_tinted(
                icon::VOLUME,
                17.0,
                Message::ToggleVolumePopover,
                volume_color,
                self.volume_popover,
            ),
            speed_button(self.speed, self.speed_popover),
            icon_button_tinted(
                icon::SUBTITLE,
                17.0,
                Message::ToggleSubtitlePopover,
                sub_color,
                self.sub_popover,
            ),
        ]
        .spacing(ICON_GAP)
        .align_y(Alignment::Center);

        // 右侧按钮组，从右到左：设置、播放列表、全屏
        let right_group = row![
            icon_button(icon::FULLSCREEN, 17.0, Message::ToggleFullscreen),
            icon_button_tinted(
                icon::PLAYLIST,
                17.0,
                Message::TogglePlaylist,
                icon::TEXT,
                self.playlist.open,
            ),
            icon_button_tinted(
                icon::SETTING,
                17.0,
                Message::OpenSettings,
                icon::TEXT,
                self.settings.open,
            ),
        ]
        .spacing(ICON_GAP)
        .align_y(Alignment::Center);

        let buttons = row![
            container(left_group).width(Fill).align_x(Horizontal::Left),
            center,
            container(right_group).width(Fill).align_x(Horizontal::Right),
        ]
        .align_y(Alignment::Center);

        let inner = column![progress, buttons]
            .spacing(6)
            .padding([10, BAR_PAD_X as u16])
            .width(Fill);

        // 整条底栏都先吃下「按下」事件：不然点栏上的空白处会穿透到画面，
        // 松开时被画面那层当成「点画面」而暂停。内容（按钮、滑块）先处理事件，
        // 它们自己捕获了的话这层就不再重复处理。
        let bar = mouse_area(
            container(inner)
                .width(Fill)
                .height(BOTTOM_H)
                .align_y(Vertical::Center)
                .style(bar_style),
        )
        .on_press(Message::Noop);

        container(bar)
            .width(Fill)
            .height(Fill)
            .align_bottom(Fill)
            .into()
    }

    /// 音量 popover：贴在音量键正上方的一块浮层（竖滑块 + 数值 + 静音）。
    ///
    /// 位置不写死像素，而是用 `Float::translate` 按视口尺寸算：
    /// 横向上让面板中心对准音量键中心（音量键在右侧组里是最左一个，
    /// 所以离右边缘的距离 = 内边距 + 3 个按钮 + 半个按钮），
    /// 纵向上让面板底边压在底栏顶边上方 `POP_GAP`。
    fn volume_popover_panel(&self) -> Element<'_, Message> {
        let muted = self.volume <= 0.0;

        let mute_btn = button(
            container(
                text(icon::VOLUME)
                    .font(icon::ICON_FONT)
                    .size(13)
                    .color(icon::TEXT_FAINT),
            )
            .center(Length::Fixed(24.0)),
        )
        .padding(0)
        .on_press(Message::ToggleMute)
        .style(ghost_button);

        let panel = container(
            column![
                text(if muted {
                    String::from("静音")
                } else {
                    format!("{:.0}", self.volume)
                })
                .font(icon::UI_FONT)
                .size(13)
                .color(icon::TEXT_DIM),
                container(
                    vertical_slider(0.0..=100.0, self.volume, Message::Volume)
                        .width(POP_W - 20.0)
                        .height(Length::Fixed(POP_SLIDER_H))
                        .style(volume_slider_style),
                )
                .center_x(Fill),
                mute_btn,
            ]
            .spacing(5)
            .align_x(Alignment::Center),
        )
        .width(Length::Fixed(POP_W))
        .padding(Padding {
            top: 8.0,
            right: 6.0,
            bottom: 6.0,
            left: 6.0,
        })
        .style(popover_style);

        // 音量键中心离窗口**左**边缘的距离。
        // 它现在是左侧按钮组的**第一颗**，也就是底栏左下角，后面跟速度胶囊和字幕键。
        let anchor_x = BAR_PAD_X + ICON_BTN / 2.0;

        // 面板本身也吃下按下事件，点它的空白处不该穿透到画面
        Float::new(swallow(panel.into()))
            .translate(move |bounds: Rectangle, viewport: Rectangle| {
                Vector::new(
                    // 面板中心对齐锚点
                    anchor_x - bounds.width / 2.0 - bounds.x,
                    viewport.height - BOTTOM_H - POP_GAP - bounds.y - bounds.height,
                )
            })
            .into()
    }

    /// 字幕 popover：贴在字幕键正上方，左边缘和底栏左内边距对齐
    /// （面板 300px 宽，居中对齐会顶出窗口左边）。
    fn subtitle_popover_panel(&self) -> Element<'_, Message> {
        let panel = subtitle::popover(&self.tracks, self.sid, self.sub_hint.as_deref());

        Float::new(swallow(panel))
            .translate(move |bounds: Rectangle, _viewport: Rectangle| {
                Vector::new(
                    BAR_PAD_X - bounds.x,
                    _viewport.height - BOTTOM_H - POP_GAP - bounds.y - bounds.height,
                )
            })
            .into()
    }

    /// 速度 popover：贴在左下角速度键的正上方，左边缘和底栏左内边距对齐。
    fn speed_popover_panel(&self) -> Element<'_, Message> {
        let accent = (self.speed - 1.0).abs() > 0.001;

        let mut grid = column![].spacing(6).width(Fill);
        for chunk in SPEEDS.chunks(3) {
            let mut r = row![].spacing(6).width(Fill);
            for s in chunk {
                r = r.push(speed_chip(*s, (self.speed - *s).abs() < 0.005));
            }
            grid = grid.push(r);
        }

        let panel = container(
            column![
                row![
                    text(icon::SPEED)
                        .font(icon::ICON_FONT)
                        .size(13)
                        .color(icon::TEXT_FAINT),
                    text("播放速度")
                        .font(icon::UI_FONT)
                        .size(13)
                        .color(icon::TEXT_DIM),
                    container(space()).width(Fill),
                    text(fmt_speed(self.speed))
                        .font(icon::UI_FONT)
                        .size(13)
                        .color(if accent { icon::ACCENT } else { icon::TEXT_DIM }),
                ]
                .spacing(6)
                .align_y(Alignment::Center),
                grid,
                slider(SPEED_MIN..=SPEED_MAX, self.speed, Message::SetSpeed)
                    .step(0.1_f64)
                    .width(Fill)
                    .height(18.0)
                    .style(volume_slider_style),
            ]
            .spacing(10)
            .width(Fill),
        )
        .width(Length::Fixed(SPEED_POP_W))
        .padding(Padding {
            top: 10.0,
            right: 10.0,
            bottom: 8.0,
            left: 10.0,
        })
        .style(popover_style);

        // 速度键现在是左侧组的第二颗（前面是音量键），面板左边缘就对齐它的左边缘，
        // 视觉上像是从这颗按钮往上长出来的。
        let anchor_x = BAR_PAD_X + ICON_BTN + ICON_GAP;

        Float::new(swallow(panel.into()))
            .translate(move |bounds: Rectangle, viewport: Rectangle| {
                Vector::new(
                    anchor_x - bounds.x,
                    viewport.height - BOTTOM_H - POP_GAP - bounds.y - bounds.height,
                )
            })
            .into()
    }

    /// 画面右键菜单：在光标位置弹出，靠近窗口边缘时自动往里收。
    fn context_menu(&self, at: Point) -> Element<'_, Message> {
        let panel = container(
            column![
                menu_item(icon::FOLDER, "在资源管理器中打开", Message::RevealInExplorer),
                menu_item(icon::INFO, "查看视频文件详情", Message::OpenInfo),
            ]
            .spacing(2)
            .width(Fill),
        )
        .width(Length::Fixed(MENU_W))
        .padding(5)
        .style(popover_style);

        Float::new(swallow(panel.into()))
            .translate(move |bounds: Rectangle, viewport: Rectangle| {
                let x = at
                    .x
                    .min(viewport.width - bounds.width - 6.0)
                    .max(6.0);
                let y = at
                    .y
                    .min(viewport.height - bounds.height - 6.0)
                    .max(6.0);
                Vector::new(x - bounds.x, y - bounds.y)
            })
            .into()
    }

    /// 抽屉打开时铺满全屏的**透明** cover：只负责吃点击，点一下就收起抽屉。
    ///
    /// 它夹在底栏之上、顶栏之下（见 `view` 里的 push 顺序），所以：
    /// 点画面 / 点底栏 → 关抽屉；点顶栏 → 照常拖窗口、最小化、关闭。
    fn cover_layer(&self) -> Element<'_, Message> {
        mouse_area(container(space()).width(Fill).height(Fill))
            // `CoverClick` 而不是原来那个直接关抽屉的消息：它还多看一眼
            // 「这一下是不是点在歌词那一片」，是的话先跳过去再关抽屉。
            .on_press(Message::CoverClick)
            // 右键也吃掉：否则抽屉开着时右击画面会在抽屉上面再叠一个右键菜单。
            // 这里刻意用 Noop —— 抽屉开着时右键不该有任何动作。
            .on_right_press(Message::Noop)
            .into()
    }

    /// 把一条抽屉摆到右边、上下让出顶底栏。
    ///
    /// 外面这层 `button` 不是为了点击（点是 Noop），而是为了**捕获按下事件**：
    /// 不捕获的话，按在抽屉上会同时算作「按在画面上」，松手就顺带把播放暂停了。
    fn drawer_layer<'a>(&self, panel: Element<'a, Message>, width: f32) -> Element<'a, Message> {
        let shell = button(panel)
            .width(Length::Fixed(width))
            .height(Fill)
            .padding(0)
            .on_press(Message::Noop)
            .style(bare_button);

        container(shell)
            .width(Fill)
            .height(Fill)
            .padding(Padding {
                top: TOP_H,
                right: 0.0,
                bottom: BOTTOM_H,
                left: 0.0,
            })
            .align_x(Horizontal::Right)
            .align_y(Vertical::Top)
            .into()
    }
}

/// 歌词面板：三种情况各画各的。
///
/// - **同步歌词**：整列每行 `row_h` 高的文字，外面套一个 `clip(true)` 的
///   固定尺寸容器，靠**顶部留白**把当前行顶到容器中线上。留白是连续值
///   （`scroll` 可以是小数），所以换句时整块是平滑往上滑，不是一格一格跳。
///   （iced 没有「滚到某一行」的直给接口，自己算留白反而最省事，也不会冒出
///   滚动条、惯性这些东西抢戏。）
///   这一支上还挂了两个鼠标交互：滚轮滚歌词、点某一行跳过去播放。
/// - **纯文本歌词**：没有时间轴对不了行，就给一个能自己滚的列表。
/// - **没有歌词**：说一句，别留一块空白让人以为界面挂了。
///
/// `scroll` 是**显示位置**（第几行贴在中线上）：自动跟随的那条弹簧加上用户
/// 滚轮叠的偏移，见 `App::lyric_browse`。`size` 是常态行字号（Ctrl + 滚轮调）。
fn lyrics_view<'a>(
    lyrics: Option<&'a meta::Lyrics>,
    current: Option<usize>,
    scroll: f32,
    width: f32,
    height: f32,
    size: f32,
) -> Element<'a, Message> {
    // 一行歌词最多这么宽：留 8px 给右边，免得贴着栏边
    let budget = (width - 8.0).max(40.0);
    // 行高跟着字号一起缩放，不然调大字号之后行会挤在一起（默认 26/13.5 ≈ 1.93 倍）
    let row_h = LYRIC_ROW_H * (size / LYRIC_SIZE_IDLE);

    let Some(lyrics) = lyrics else {
        return container(lyrics_hint(
            "没有歌词",
            Some("把同名 .lrc 放在音乐旁边，或写进标签里"),
        ))
        .width(Length::Fixed(width))
        .height(Length::Fixed(height))
        .into();
    };
    if lyrics.len() == 0 {
        return container(lyrics_hint("歌词是空的", None))
            .width(Length::Fixed(width))
            .height(Length::Fixed(height))
            .into();
    }

    match lyrics {
        meta::Lyrics::Plain(lines) => {
            let body = column(
                lines
                    .iter()
                    .map(|line| {
                        text(ui::elide(line, budget, 13.5))
                            .font(icon::UI_FONT)
                            .size(13.5)
                            .color(Color::from_rgba(1.0, 1.0, 1.0, 0.62))
                            .width(Fill)
                            .into()
                    })
                    .collect::<Vec<Element<'a, Message>>>(),
            )
            .spacing(7)
            .width(Fill);

            container(iced::widget::scrollable(body).height(Fill).width(Fill))
                .width(Length::Fixed(width))
                .height(Length::Fixed(height))
                .into()
        }
        meta::Lyrics::Synced(lines) => {
            // 前奏（一句都还没到）时也把第 0 行摆在正中间，只是不点亮：
            // 第一句真的唱出来时只有颜色变化，整块歌词不会先跳一格子。
            let lit = current.is_some();

            let (first, pad) = lyric_layout(scroll, height, row_h);
            // 容器里放得下几行，多给两行当过渡用
            let count = (height / row_h).ceil() as usize + 2;

            let mut rows: Vec<Element<'a, Message>> = Vec::with_capacity(count + 1);
            rows.push(space().height(Length::Fixed(pad)).into());

            for i in first..lines.len().min(first + count) {
                // 离正中间有多远（**连续值**：`scroll` 是弹簧推着的小数）。
                // 字号和亮度都按它算，所以换句那一小段时间里，行是一路变大变亮、
                // 而不是"到点啪一下"跳档 —— AMLL 也是按距离插值的。
                let d = (i as f32 - scroll).abs();
                let alpha = lyric_alpha(d, lit);
                let px = lyric_size(d, size);

                let row = container(
                    text(ui::elide(&lines[i].text, budget, px))
                        .font(if d < 0.5 {
                            icon::UI_BOLD
                        } else {
                            icon::UI_FONT
                        })
                        .size(px)
                        .color(Color::from_rgba(1.0, 1.0, 1.0, alpha))
                        .width(Fill)
                        .align_x(Horizontal::Center),
                )
                .height(Length::Fixed(row_h))
                .width(Fill)
                .align_y(Vertical::Center);

                // 点这一行就跳到这句唱的地方。手型光标提示「这里能点」——
                // 不然谁知道歌词还能点。
                rows.push(
                    mouse_area(row)
                        .interaction(mouse::Interaction::Pointer)
                        .on_press(Message::LyricSeek(i))
                        .into(),
                );
            }

            // clip：滑到一半时，上下溢出的行不能画到容器外面去。
            // 外面再套一层 mouse_area 专门接滚轮（行的 on_press 在内层，互不影响）。
            mouse_area(
                container(column(rows).width(Fill))
                    .width(Length::Fixed(width))
                    .height(Length::Fixed(height))
                    .clip(true),
            )
            .on_scroll(|delta| Message::LyricWheel(wheel_rows(delta)))
            .into()
        }
    }
}

/// 离正中间 `d` 行的那一行该多亮。
///
/// `d` 是**连续值**（`scroll` 由弹簧推着，可以是 3.42 这种）。规则是：
///
/// - 正中间那行纯白（`LYRIC_ALPHA_ACTIVE`）；
/// - 相邻行 `LYRIC_ALPHA_NEAR`（0.60），往外每多一行乘 0.68 —— 到第 6 行正好
///   淡到 0.05 左右，肉眼看不见了；
/// - 最后一行（`LYRIC_FADE_ROWS` 附近）再乘一个线性收尾，避免视口边缘突然
///   冒出一行灰字。
///
/// `lit = false`（前奏，一句都还没唱到）时整列都压到"相邻行"的亮度：
/// 这时候没有"当前句"，不该有一行是白的。
fn lyric_alpha(d: f32, lit: bool) -> f32 {
    let d = d.max(0.0);
    let base = if d <= 1.0 {
        LYRIC_ALPHA_ACTIVE + (LYRIC_ALPHA_NEAR - LYRIC_ALPHA_ACTIVE) * d
    } else {
        LYRIC_ALPHA_NEAR * 0.68f32.powf(d - 1.0)
    };
    let tail = (LYRIC_FADE_ROWS as f32 - d).clamp(0.0, 1.0);
    let alpha = base * tail;
    if lit {
        alpha
    } else {
        alpha.min(LYRIC_ALPHA_NEAR)
    }
}

/// 离正中间 `d` 行的那一行用多大字号。
///
/// 正中间是常态字号 `base` 的 `LYRIC_SIZE_ACTIVE / LYRIC_SIZE_IDLE` 倍（默认 1.26），
/// 一行开外就回到 `base`，中间线性插值。换句时旧行缩回、新行长大是跟着弹簧一起走的，
/// 不是分档跳。`base` 由 Ctrl + 滚轮调（`App::lyric_size`）。
fn lyric_size(d: f32, base: f32) -> f32 {
    let t = d.clamp(0.0, 1.0);
    let active = base * (LYRIC_SIZE_ACTIVE / LYRIC_SIZE_IDLE);
    base + (active - base) * (1.0 - t)
}

/// 滚轮一格算滚几行。
///
/// winit 在触摸板 / 高精度滚轮上发的是 `Pixels`（连续），鼠标滚轮发 `Lines`
/// （一格一格）。两种都换算成"行"，下面 `scroll_lyrics` 只认行。
fn wheel_rows(delta: mouse::ScrollDelta) -> f32 {
    match delta {
        mouse::ScrollDelta::Lines { y, .. } => -y * LYRIC_WHEEL_ROWS,
        // 40 像素 ≈ 鼠标滚轮一格（winit 自己也按这个量级折算）
        mouse::ScrollDelta::Pixels { y, .. } => -y / 40.0 * LYRIC_WHEEL_ROWS,
    }
}

/// 同步歌词要「从第几行开始画」和「顶部留白多高」。
///
/// 留白是连续的：换句时它一点点变小，整块歌词就平滑地往上滑一行，而不是一格
/// 一格地跳。唯一要小心的约束是**留白不能为负**（iced 没有负 padding），所以
/// 起始行要跟着 `scroll` 一起往前挪 —— `scroll` 走一行，`first` 也走一行。
///
/// 返回值保证：`pad >= 0`，且第 `scroll` 行的顶边正好在 `height/2 - 行高/2`。
///
/// `row_h` 是**当前字号下一个行占多高**（Ctrl + 滚轮调字号时会变，见 `lyrics_view`）。
fn lyric_layout(scroll: f32, height: f32, row_h: f32) -> (usize, f32) {
    let half = height * 0.5 - row_h * 0.5;
    let first = (scroll - half / row_h).ceil().max(0.0) as usize;
    let pad = (half - (scroll - first as f32) * row_h).max(0.0);
    (first, pad)
}

/// 歌词那块的一块「没什么可看」的占位。
fn lyrics_hint(title: &'static str, hint: Option<&'static str>) -> Element<'static, Message> {
    let mut col = column![text(title)
        .font(icon::UI_FONT)
        .size(14.0)
        .color(Color::from_rgba(1.0, 1.0, 1.0, 0.42))]
    .spacing(8)
    .align_x(Alignment::Center);

    if let Some(hint) = hint {
        col = col.push(
            text(hint)
                .font(icon::UI_FONT)
                .size(12.0)
                .color(Color::from_rgba(1.0, 1.0, 1.0, 0.26)),
        );
    }

    container(col).center(Fill).into()
}

/// 包一层「只吃事件、不画任何东西」的按钮：浮层 / 抽屉用它挡住穿透
fn swallow<'a>(inner: Element<'a, Message>) -> Element<'a, Message> {
    button(inner)
        .padding(0)
        .on_press(Message::Noop)
        .style(bare_button)
        .into()
}

fn bare_button(_t: &Theme, _s: button::Status) -> button::Style {
    button::Style {
        background: None,
        text_color: icon::TEXT,
        border: Border::default(),
        ..button::Style::default()
    }
}

// ---------------- 组件小工具 ----------------

/// 字体对照表：只给排查用（`QP_FONT_TEST=1`）。
///
/// iced 走 grayscale 抗锯齿、不做 TrueType hinting，12~13px 的中文本来就比
/// Windows 原生（ClearType + hinting）软。把可能的组合排在一起看出哪种最清楚：
/// 关键是**别让 cosmic-text 去合成字重** —— 系统里没有的 Semibold 会被插值模拟，
/// 笔画边缘全是灰过渡，反而更糊。
fn font_specimen<'a>() -> Element<'a, Message> {
    use iced::font::{Family, Weight};

    const SAMPLE: &str = "把视频或文件夹拖进来 Maneki 00:12";
    const LABEL: &str = "把视频或文件夹拖进来";

    let specimen = |name: &'static str, font: iced::Font, size: f32| -> Element<'a, Message> {
        row![
            container(
                text(name)
                    .font(icon::UI_FONT)
                    .size(11)
                    .color(icon::TEXT_FAINT)
            )
            .width(Length::Fixed(210.0)),
            text(SAMPLE).font(font).size(size).color(icon::TEXT),
        ]
        .spacing(12)
        .align_y(Alignment::Center)
        .into()
    };

    let f = |family: Family, weight: Weight| iced::Font {
        family,
        weight,
        ..iced::Font::default()
    };

    let rows = column![
        specimen("SansSerif Normal 12", f(Family::SansSerif, Weight::Normal), 12.0),
        specimen("SansSerif Semibold 12（当前）", f(Family::SansSerif, Weight::Semibold), 12.0),
        specimen("SansSerif Normal 13", f(Family::SansSerif, Weight::Normal), 13.0),
        specimen("SansSerif Semibold 13", f(Family::SansSerif, Weight::Semibold), 13.0),
        specimen("SansSerif Bold 13", f(Family::SansSerif, Weight::Bold), 13.0),
        specimen("YaHei UI Normal 13", f(Family::Name("Microsoft YaHei UI"), Weight::Normal), 13.0),
        specimen("YaHei UI Semibold 13", f(Family::Name("Microsoft YaHei UI"), Weight::Semibold), 13.0),
        specimen("YaHei UI Bold 13", f(Family::Name("Microsoft YaHei UI"), Weight::Bold), 13.0),
        specimen("YaHei Normal 13", f(Family::Name("Microsoft YaHei"), Weight::Normal), 13.0),
        specimen("Segoe UI Semibold 13", f(Family::Name("Segoe UI"), Weight::Semibold), 13.0),
        specimen("YaHei UI Normal 14", f(Family::Name("Microsoft YaHei UI"), Weight::Normal), 14.0),
        specimen("YaHei UI Normal 12", f(Family::Name("Microsoft YaHei UI"), Weight::Normal), 12.0),
        container(space()).height(Length::Fixed(10.0)),
        specimen("当前标题字号 18", f(Family::SansSerif, Weight::Normal), 18.0),
        specimen("当前标题字号 18 加粗", f(Family::SansSerif, Weight::Semibold), 18.0),
        text(LABEL).size(18).font(icon::UI_FONT).color(icon::TEXT_DIM),
    ]
    .spacing(9)
    .width(Length::Fixed(760.0));

    container(container(rows).padding(24).style(|_| container::Style {
        background: Some(Background::Color(Color::from_rgb(0.09, 0.09, 0.10))),
        ..container::Style::default()
    }))
    .width(Fill)
    .height(Fill)
    .center(Fill)
    .style(black_style)
    .into()
}

/// 窗口边缘的一条缩放热区。
///
/// 无边框窗口整窗都是 `HTCLIENT`，Windows 不会给任何边缘缩放机会，
/// 所以只能自己在四个边 / 四个角叠一层透明的 `mouse_area`：
/// 光标形状由 `interaction` 提供，按下后交给 `step_resize` 每个 tick 拖着走。
fn edge_zone<'a>(
    direction: Direction,
    ax: Horizontal,
    ay: Vertical,
    pad: [u16; 2],
    width: Length,
    height: Length,
) -> Element<'a, Message> {
    let interaction = match direction {
        Direction::North | Direction::South => mouse::Interaction::ResizingVertically,
        Direction::East | Direction::West => mouse::Interaction::ResizingHorizontally,
        Direction::NorthEast | Direction::SouthWest => mouse::Interaction::ResizingDiagonallyUp,
        Direction::NorthWest | Direction::SouthEast => {
            mouse::Interaction::ResizingDiagonallyDown
        }
    };

    let zone = mouse_area(container(space()).width(width).height(height))
        .interaction(interaction)
        .on_press(Message::ResizeStart(direction));

    container(zone)
        .width(Fill)
        .height(Fill)
        .padding(pad)
        .align_x(ax)
        .align_y(ay)
        .into()
}

/// 顶底栏底色：**纯色**（不是渐变），一层半透明黑，直接压在画面上。
///
/// 想更透/更实，改 `BAR_ALPHA` 就行。
fn bar_style(_t: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(Color::from_rgba(
            0.0,
            0.0,
            0.0,
            BAR_ALPHA,
        ))),
        ..container::Style::default()
    }
}

/// 播放器底层：纯黑，视频没铺满时露出来的是真黑
fn black_style(_t: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(Color::BLACK)),
        ..container::Style::default()
    }
}

/// 音量 popover 的面板底：深色圆角 + 一圈极淡的描边 + 投影，浮在画面上也看得清
fn popover_style(_t: &Theme) -> container::Style {
    container::Style {
        // 和抽屉 / 字幕面板同一个黑底（黑色半透明，不是纯黑）
        background: Some(Background::Color(icon::PANEL_BG)),
        border: Border {
            radius: 12.0.into(),
            width: 1.0,
            color: icon::PANEL_LINE,
        },
        shadow: Shadow {
            color: Color::from_rgba(0.0, 0.0, 0.0, 0.45),
            offset: Vector::new(0.0, 4.0),
            blur_radius: 16.0,
        },
        ..container::Style::default()
    }
}

/// 调试开关：`QP_SPIN=1` 让换片转圈一直显示。
///
/// 本地小文件几乎秒出画，那个空档短到根本看不见，想对样式（颜色、粗细、转速）
/// 就得等一块慢盘。跟 `QP_DEMO` / `QP_MULTI` 一样，只是给自己看效果用的，
/// 不设这个环境变量时程序行为完全正常。
fn spin_preview() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("QP_SPIN").is_some())
}

/// 指数平滑地逼近目标值。
///
/// `k` 由 `1 - exp(-dt / tau)` 算出来，和帧率无关：掉帧的时候一步迈得大一点，
/// 走完的总时间不会变（线性插值在掉帧时会明显变慢）。
fn approach(cur: f32, target: f32, k: f32) -> f32 {
    let next = cur + (target - cur) * k;
    if (target - next).abs() < 0.002 {
        target
    } else {
        next
    }
}

/// 进度条：**平时没有小圆圈**，鼠标移上去（或正在拖）才出现。
///
/// `HandleShape::Circle { radius: 0.0 }` 会让手柄宽高都算成 0，画出来看不见；
/// 轨道本身照常绘制，所以不碰鼠标时只有一条干净的进度线。
fn progress_slider_style(_t: &Theme, status: slider::Status) -> slider::Style {
    let (radius, rail_w) = match status {
        slider::Status::Dragged => (6.5, 5.0),
        slider::Status::Hovered => (5.5, 5.0),
        slider::Status::Active => (0.0, 3.0),
    };

    let filled = match status {
        slider::Status::Dragged => Color::from_rgb(0.55, 0.83, 1.0),
        _ => icon::ACCENT,
    };

    slider::Style {
        rail: slider::Rail {
            // .0 是手柄左边（已播过的一段），.1 是右边
            backgrounds: (
                Background::Color(filled),
                Background::Color(Color::from_rgba(1.0, 1.0, 1.0, 0.18)),
            ),
            width: rail_w,
            border: Border {
                radius: 3.0.into(),
                ..Border::default()
            },
        },
        handle: slider::Handle {
            shape: slider::HandleShape::Circle { radius },
            background: Background::Color(Color::WHITE),
            border_width: 0.0,
            border_color: Color::TRANSPARENT,
        },
    }
}

/// popover 里的竖滑块：轨道细一点，手柄是白色圆点，已填充的一段用主题蓝
fn volume_slider_style(_t: &Theme, status: slider::Status) -> slider::Style {
    let filled = match status {
        slider::Status::Dragged => Color::from_rgb(0.55, 0.83, 1.0),
        _ => icon::ACCENT,
    };

    slider::Style {
        rail: slider::Rail {
            // .0 是手柄**下方**（已经调上去的音量），.1 是手柄**上方**
            backgrounds: (
                Background::Color(filled),
                Background::Color(Color::from_rgba(1.0, 1.0, 1.0, 0.16)),
            ),
            width: 4.0,
            border: Border {
                radius: 2.0.into(),
                ..Border::default()
            },
        },
        handle: slider::Handle {
            shape: slider::HandleShape::Circle { radius: 6.5 },
            background: Background::Color(icon::TEXT),
            border_width: 0.0,
            border_color: Color::TRANSPARENT,
        },
    }
}

fn icon_button<'a>(glyph: char, size: f32, msg: Message) -> Element<'a, Message> {
    icon_button_tinted(glyph, size, msg, icon::TEXT, false)
}

/// 空白页上的文字按钮（描边胶囊），比图标按钮更适合放中文标签
fn pill_button<'a>(label: &'a str, msg: Message) -> Element<'a, Message> {
    button(text(label).font(icon::UI_FONT).size(14).color(icon::TEXT_DIM))
        .padding(Padding {
            top: 7.0,
            right: 14.0,
            bottom: 7.0,
            left: 14.0,
        })
        .on_press(msg)
        .style(pill_style)
        .into()
}

fn pill_style(_t: &Theme, status: button::Status) -> button::Style {
    use button::Status;
    let bg = match status {
        Status::Hovered => Color::from_rgba(1.0, 1.0, 1.0, 0.14),
        Status::Pressed => Color::from_rgba(1.0, 1.0, 1.0, 0.22),
        _ => Color::from_rgba(1.0, 1.0, 1.0, 0.06),
    };
    button::Style {
        background: Some(Background::Color(bg)),
        text_color: icon::TEXT,
        border: Border {
            radius: 8.0.into(),
            width: 1.0,
            color: Color::from_rgba(1.0, 1.0, 1.0, 0.12),
        },
        ..button::Style::default()
    }
}

/// `active` 为真时按钮带一块底色，用来表示「这块 popover 是它弹出来的」
fn icon_button_tinted<'a>(
    glyph: char,
    size: f32,
    msg: Message,
    color: Color,
    active: bool,
) -> Element<'a, Message> {
    button(
        container(text(glyph).font(icon::ICON_FONT).size(size).color(color))
            .center(Length::Fixed(ICON_BTN)),
    )
    .padding(0)
    .on_press(msg)
    .style(if active { solid_button } else { ghost_button })
    .into()
}

/// 底栏左下角的播放速度键：一颗显示当前倍速的小胶囊。
///
/// 不用图标：`1.25×` 这种信息量图标表达不了，用户得能一眼看出现在是不是倍速。
/// 偏离 1.0 时文字转成主题色，扫一眼就知道「忘记调回来了」。
fn speed_button<'a>(speed: f64, active: bool) -> Element<'a, Message> {
    let normal = (speed - 1.0).abs() < 0.001;
    let color = if normal { icon::TEXT_DIM } else { icon::ACCENT };

    button(
        container(
            text(fmt_speed(speed))
                .font(icon::UI_FONT)
                .size(12.5)
                .color(color),
        )
        .width(Length::Fixed(SPEED_W))
        .height(Length::Fixed(ICON_BTN))
        // 中文字体在行框里天生偏下一点，「1.0×」看着像坐在地板上。
        // 底下垫 padding 会让垂直居中把文字整体往上顶 **(padding / 2)** 像素。
        // 三次实测校准过：0 → 偏下；6（顶上去 3px）→ 偏上；2（顶上去 1px）正好。
        // 别再来回抖了，就 2。
        .padding(Padding {
            top: 0.0,
            right: 0.0,
            bottom: 2.0,
            left: 0.0,
        })
        .align_x(Horizontal::Center)
        .align_y(Vertical::Center),
    )
    .padding(0)
    .on_press(Message::ToggleSpeedPopover)
    .style(if active { solid_button } else { ghost_button })
    .into()
}

/// 速度 popover 里的一个预设小格子
fn speed_chip<'a>(value: f64, active: bool) -> Element<'a, Message> {
    button(
        container(
            text(fmt_speed(value))
                .font(icon::UI_FONT)
                .size(12.5)
                .color(if active {
                    Color::from_rgb(0.05, 0.06, 0.08)
                } else {
                    icon::TEXT_DIM
                }),
        )
        .center_x(Fill)
        .center_y(Length::Fixed(26.0)),
    )
    .width(Fill)
    .padding(0)
    .on_press(Message::SetSpeed(value))
    .style(move |_t, status| {
        use button::Status;
        let bg = match status {
            _ if active => Some(Background::Color(icon::ACCENT)),
            Status::Hovered => Some(Background::Color(Color::from_rgba(1.0, 1.0, 1.0, 0.16))),
            Status::Pressed => Some(Background::Color(Color::from_rgba(1.0, 1.0, 1.0, 0.22))),
            _ => Some(Background::Color(Color::from_rgba(1.0, 1.0, 1.0, 0.06))),
        };
        button::Style {
            background: bg,
            text_color: icon::TEXT,
            border: Border {
                radius: 6.0.into(),
                width: if active { 0.0 } else { 1.0 },
                color: Color::from_rgba(1.0, 1.0, 1.0, 0.10),
            },
            ..button::Style::default()
        }
    })
    .into()
}

/// 右键菜单里的一行：图标 + 文字
fn menu_item<'a>(glyph: char, label: &'a str, msg: Message) -> Element<'a, Message> {
    button(
        row![
            text(glyph)
                .font(icon::ICON_FONT)
                .size(13)
                .color(icon::TEXT_FAINT),
            text(label).font(icon::UI_FONT).size(13).color(icon::TEXT_DIM),
        ]
        .spacing(9)
        .align_y(Alignment::Center)
        .padding(Padding {
            top: 0.0,
            right: 0.0,
            bottom: 0.0,
            left: 2.0,
        }),
    )
    .width(Fill)
    .padding(Padding {
        top: 7.0,
        right: 8.0,
        bottom: 7.0,
        left: 8.0,
    })
    .on_press(msg)
    .style(ghost_button)
    .into()
}

/// 倍速的显示文本：`1.0×` / `1.25×` / `8.0×`（末尾的 0 去掉，但至少留一位小数）
fn fmt_speed(speed: f64) -> String {
    let r = (speed * 100.0).round() / 100.0;
    let mut t = format!("{r:.2}");
    while t.ends_with('0') && !t.ends_with(".0") {
        t.pop();
    }
    format!("{t}×")
}

/// 左上角「logo + 程序名」按钮：置顶时常亮一块底色，平时只有 hover / 按下才亮
fn brand_button(_t: &Theme, status: button::Status, pinned: bool) -> button::Style {    use button::Status;

    let bg = match status {
        Status::Pressed => Some(Background::Color(Color::from_rgba(1.0, 1.0, 1.0, 0.20))),
        Status::Hovered => Some(Background::Color(icon::HOVER_BG)),
        _ if pinned => Some(Background::Color(icon::HOVER_BG)),
        _ => None,
    };

    button::Style {
        background: bg,
        text_color: icon::TEXT,
        border: Border {
            radius: 6.0.into(),
            ..Border::default()
        },
        ..button::Style::default()
    }
}

/// 常亮底色的图标按钮（popover 展开时给音量键用）
fn solid_button(_t: &Theme, _status: button::Status) -> button::Style {
    button::Style {
        background: Some(Background::Color(icon::HOVER_BG)),
        text_color: icon::TEXT,
        border: Border {
            radius: 6.0.into(),
            ..Border::default()
        },
        ..button::Style::default()
    }
}

fn window_button<'a>(glyph: char, msg: Message, danger: bool) -> Element<'a, Message> {
    button(
        container(text(glyph).font(icon::ICON_FONT).size(13).color(icon::TEXT_DIM))
            .center(Length::Fixed(30.0)),
    )
    .padding(0)
    .on_press(msg)
    .style(if danger {
        close_button
    } else {
        ghost_button
    })
    .into()
}

fn ghost_button(_t: &Theme, status: button::Status) -> button::Style {
    use button::Status;
    let bg = match status {
        Status::Hovered => Some(Background::Color(icon::HOVER_BG)),
        Status::Pressed => Some(Background::Color(Color::from_rgba(1.0, 1.0, 1.0, 0.2))),
        _ => None,
    };
    button::Style {
        background: bg,
        text_color: icon::TEXT,
        border: Border {
            radius: 6.0.into(),
            ..Border::default()
        },
        ..button::Style::default()
    }
}

fn close_button(_t: &Theme, status: button::Status) -> button::Style {
    use button::Status;
    let bg = match status {
        Status::Hovered => Some(Background::Color(icon::CLOSE_HOVER)),
        Status::Pressed => Some(Background::Color(Color::from_rgb(0.7, 0.15, 0.15))),
        _ => None,
    };
    button::Style {
        background: bg,
        text_color: icon::TEXT,
        border: Border {
            radius: 6.0.into(),
            ..Border::default()
        },
        ..button::Style::default()
    }
}

fn play_button<'a>(paused: bool) -> Element<'a, Message> {
    let glyph: Element<'a, Message> = if paused {
        text(icon::PLAY)
            .font(icon::ICON_FONT)
            .size(24)
            .color(Color::WHITE)
            .into()
    } else {
        // 字体里没有「暂停」字形，用两根圆角竖条画
        row![
            pause_bar(),
            pause_bar(),
        ]
        .spacing(5)
        .align_y(Alignment::Center)
        .into()
    };

    button(container(glyph).center(Length::Fixed(46.0)))
        .padding(0)
        .on_press(Message::TogglePlay)
        .style(|_t, status| {
            use button::Status;
            let a = match status {
                Status::Hovered => 0.30,
                Status::Pressed => 0.42,
                _ => 0.20,
            };
            button::Style {
                background: Some(Background::Color(Color::from_rgba(1.0, 1.0, 1.0, a))),
                text_color: Color::WHITE,
                border: Border {
                    radius: 23.0.into(),
                    ..Border::default()
                },
                ..button::Style::default()
            }
        })
        .into()
}

fn pause_bar<'a>() -> Element<'a, Message> {
    container(space())
        .width(Length::Fixed(4.0))
        .height(Length::Fixed(15.0))
        .style(|_| container::Style {
            background: Some(Background::Color(Color::WHITE)),
            border: Border {
                radius: 1.5.into(),
                ..Border::default()
            },
            ..container::Style::default()
        })
        .into()
}

fn fmt_time(t: f64) -> String {
    if !t.is_finite() || t < 0.0 {
        return String::from("00:00");
    }
    let s = t as u64;
    let (h, m, sec) = (s / 3600, (s % 3600) / 60, s % 60);
    if h > 0 {
        format!("{h}:{m:02}:{sec:02}")
    } else {
        format!("{m:02}:{sec:02}")
    }
}

/// 拖放这一路的端到端测试。
///
/// 为什么要用「喂消息」而不是真的拖：在用户桌面上模拟 OLE 拖拽既不可靠
/// （窗口位置、DPI、虚拟桌面都会变），也有误移动用户文件的风险。
/// 这里直接走 `Message::Dropped` → 攒批 → `flush_drops` → `open_paths`，
/// 把「一批路径怎么变成播放列表」这段逻辑锁死。
/// 视频文件用空文件即可 —— 扫描只看扩展名，不解码。
#[cfg(test)]
mod tests {
    use super::*;

    fn make_dir(name: &str, files: &[&str]) -> PathBuf {
        let root = std::env::temp_dir().join(name);
        let _ = std::fs::remove_dir_all(&root);
        for f in files {
            let p = root.join(f);
            if let Some(parent) = p.parent() {
                std::fs::create_dir_all(parent).unwrap();
            }
            std::fs::write(&p, b"").unwrap();
        }
        root
    }

    /// `App::new()` 会读命令行第一个参数（正常启动是待打开的文件），
    /// 测试里那个参数是 cargo 的过滤词，所以建完先把列表状态清干净。
    fn app() -> App {
        let (mut app, _task) = App::new();
        app.playlist = Playlist::default();
        app
    }

    fn drop_now(app: &mut App) {
        // 把「最后一条拖放」的时间戳推到 120ms 之前，让 tick 里的攒批逻辑立刻结算
        app.drop_at = Some(Instant::now() - DROP_SETTLE - Duration::from_millis(1));
        app.flush_drops();
    }

    /// 一张 2x2 的假画面：只为了满足 `frame.is_some()` 这类判断，内容无所谓
    fn screen() -> crate::video::VideoFrame {
        crate::video::VideoFrame {
            data: std::sync::Arc::new(vec![0; 16]),
            width: 2,
            height: 2,
        }
    }

    #[test]
    fn one_batch_of_drops_becomes_one_playlist() {
        let root = make_dir(
            "qp_test_drop_folder",
            &[
                "第1集.mp4",
                "第2集.mp4",
                "第10集.mp4",
                "readme.txt",
                "Season 1\\S01E2.mp4",
            ],
        );

        let mut app = app();
        // 一次拖进来两样东西（winit 会连发两条消息）：文件夹 + 其中一个文件，
        // 文件重复了要能去重。
        let _ = app.update(Message::Dropped(root.clone()));
        let _ = app.update(Message::Dropped(root.join("第1集.mp4")));

        // 还没到 120ms，不能抢跑
        assert!(app.playlist.items.is_empty());
        drop_now(&mut app);

        let names: Vec<&str> = app.playlist.items.iter().map(|i| i.name.as_str()).collect();
        assert_eq!(names, ["S01E2.mp4", "第1集.mp4", "第2集.mp4", "第10集.mp4"]);
        // 一整批（文件夹）装进来**一条都不自动播**，抽屉拉开等用户点
        assert_eq!(app.playlist.select, None);
        assert!(app.playlist.open);
        // 界面要停在「挑一个」的状态：没有当前文件、也没有画面
        assert!(app.current.is_none());
        assert!(app.frame.is_none());
        assert_eq!(app.duration, 0.0);
        assert_eq!(app.position, 0.0);
        // 嵌套子目录要标出来源
        assert_eq!(app.playlist.items[0].group.as_deref(), Some("Season 1"));
        // 根目录下的文件不该有 group
        assert!(app.playlist.items[1].group.is_none());

        let _ = std::fs::remove_dir_all(&root);
    }

    /// 正在放片子的时候再扔一个文件夹进来：不能接着放新列表的第一条，
    /// 而且上一部片子的画面、进度、时长都要清干净（不然界面会挂着旧数字）
    #[test]
    fn opening_a_folder_stops_whatever_was_playing() {
        let root = make_dir("qp_test_drop_folder_idle", &["b1.mp4", "b2.mp4"]);

        let mut app = playing_at(30.0);
        app.frame = Some(screen());

        let _ = app.update(Message::Dropped(root.clone()));
        drop_now(&mut app);

        assert_eq!(app.playlist.items.len(), 2);
        assert_eq!(app.playlist.select, None, "不该替用户选第一条");
        assert!(app.current.is_none(), "要停在「挑一个」，而不是接着放旧的");
        assert!(app.frame.is_none());
        assert!(!app.stale_frame, "占位帧的标记也要一起清掉");
        assert_eq!(app.position, 0.0);
        assert_eq!(app.duration, 0.0);
        assert!(!app.paused, "还没开始放，不该显示暂停态");

        let _ = std::fs::remove_dir_all(&root);
    }

    /// 「挑一个」界面上按播放键 / 空格：从列表第一条起播，而不是没反应。
    /// 列表是空的时候仍然什么都不做（不能凭空造一条出来）。
    #[test]
    fn play_from_idle_starts_the_first_item() {
        let root = make_dir("qp_test_play_idle", &["c1.mp4", "c2.mp4"]);

        // 空列表时按播放：什么都不做（不能凭空造一条出来）
        let mut blank = app();
        let _ = blank.update(Message::TogglePlay);
        assert!(blank.current.is_none());
        assert_eq!(blank.playlist.select, None);

        let mut app = app();
        let _ = app.update(Message::Dropped(root.clone()));
        drop_now(&mut app);
        assert_eq!(app.playlist.select, None);

        let _ = app.update(Message::TogglePlay);
        assert_eq!(app.playlist.select, Some(0), "按播放应该从第一条开始");
        assert!(app.current.is_some());
        assert!(!app.paused);

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn one_file_drop_does_not_open_the_drawer() {
        let root = make_dir("qp_test_drop_file", &["only.mkv"]);

        let mut app = app();
        let _ = app.update(Message::Dropped(root.join("only.mkv")));
        drop_now(&mut app);

        assert_eq!(app.playlist.items.len(), 1);
        assert_eq!(app.playlist.select, Some(0));
        assert!(!app.playlist.open, "单个文件不该弹播放列表");

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn dropping_a_folder_without_videos_says_so() {
        let root = make_dir("qp_test_drop_empty", &["a.txt", "b.jpg"]);

        let mut app = app();
        let _ = app.update(Message::Dropped(root.clone()));
        drop_now(&mut app);

        assert!(app.playlist.items.is_empty());
        assert!(app.playlist.open, "什么都没找到也要把抽屉打开说明原因");
        assert_eq!(
            app.playlist.note.as_deref(),
            Some("这个文件夹里没有找到视频或音频")
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn next_and_prev_walk_the_playlist() {
        let root = make_dir(
            "qp_test_drop_walk",
            &["a1.mp4", "a2.mp4", "a3.mp4"],
        );

        let mut app = app();
        let _ = app.update(Message::Dropped(root.clone()));
        drop_now(&mut app);
        assert_eq!(app.playlist.items.len(), 3);
        // 扔文件夹不再自动播第一条，先手动点一条，后面的上/下一集才有起点
        let _ = app.update(Message::PlayIndex(0));

        // 列表里前进 / 后退，而不是快进 10 秒
        let _ = app.update(Message::Next);
        assert_eq!(app.playlist.select, Some(1));
        let _ = app.update(Message::Next);
        assert_eq!(app.playlist.select, Some(2));

        // 已经在最后一条：不动，也不要跳到 0 秒、更不要快进 10 秒
        app.position = 5.0;
        let _ = app.update(Message::Next);
        assert_eq!(app.playlist.select, Some(2));
        assert_eq!(app.position, 5.0, "没有下一条时不该变成进度跳转");

        let _ = app.update(Message::Prev);
        assert_eq!(app.playlist.select, Some(1));

        // 回到第一条再按上一集：什么都不该发生（连进度也不许动）
        let _ = app.update(Message::Prev);
        assert_eq!(app.playlist.select, Some(0));
        app.position = 7.0;
        let _ = app.update(Message::Prev);
        assert_eq!(app.playlist.select, Some(0));
        assert_eq!(app.position, 7.0, "没有上一条时不该变成快退 10 秒");

        let _ = std::fs::remove_dir_all(&root);
    }

    /// 只有一个视频（或者压根没列表）时，上一集 / 下一集必须**没反应**：
    /// 不换片、不动进度。以前这里会退化成 ±10 秒，用户点「下一集」会看到进度条乱跳。
    #[test]
    fn prev_next_do_nothing_without_neighbours() {
        let root = make_dir("qp_test_single_neighbour", &["only.mp4"]);

        let mut app = app();
        let _ = app.update(Message::Dropped(root.join("only.mp4")));
        drop_now(&mut app);
        assert_eq!(app.playlist.items.len(), 1);
        assert_eq!(app.playlist.select, Some(0));

        app.position = 4.0;
        let _ = app.update(Message::Next);
        assert_eq!(app.playlist.select, Some(0));
        assert_eq!(app.position, 4.0);

        let _ = app.update(Message::Prev);
        assert_eq!(app.playlist.select, Some(0));
        assert_eq!(app.position, 4.0);

        // 列表被清空之后也一样：两个键都只是「没反应」
        app.playlist.select = None;
        let _ = app.update(Message::Next);
        let _ = app.update(Message::Prev);
        assert_eq!(app.playlist.select, None);
        assert_eq!(app.position, 4.0);

        let _ = std::fs::remove_dir_all(&root);
    }

    /// 抽屉的滑入滑出：`animate()` 每帧把进度往目标推，
    /// 15 帧（约 250ms）之内应该基本到位，关掉之后要能退回 0。
    #[test]
    fn drawers_slide_in_and_out() {
        let mut app = app();
        assert_eq!(app.settings_anim, 0.0);

        let _ = app.update(Message::OpenSettings);
        for _ in 0..15 {
            app.animate();
        }
        assert!(
            app.settings_anim > 0.9,
            "展开太慢：250ms 才走到 {}",
            app.settings_anim
        );

        let _ = app.update(Message::CloseSettings);
        for _ in 0..15 {
            app.animate();
        }
        assert!(
            app.settings_anim < 0.1,
            "收起太慢：250ms 还剩 {}",
            app.settings_anim
        );
    }

    /// 右侧只有一个位置：设置抽屉、播放列表、字幕浮层互斥
    #[test]
    fn only_one_panel_at_a_time() {
        let mut app = app();

        let _ = app.update(Message::OpenSettings);
        assert!(app.settings.open);

        let _ = app.update(Message::TogglePlaylist);
        assert!(app.playlist.open);
        assert!(!app.settings.open, "开播放列表要把设置收起来");

        let _ = app.update(Message::ToggleSubtitlePopover);
        assert!(app.sub_popover);
        assert!(!app.playlist.open, "开字幕要把播放列表收起来");

        let _ = app.update(Message::OpenSettings);
        assert!(app.settings.open);
        assert!(!app.sub_popover, "开设置要把字幕浮层收起来");

        // 拖进来一个文件夹也要把设置让开
        let root = make_dir("qp_test_panel_switch", &["a.mp4", "b.mp4"]);
        let _ = app.update(Message::Dropped(root.clone()));
        drop_now(&mut app);
        assert!(app.playlist.open);
        assert!(!app.settings.open);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 自动连播默认关：用户没勾就不该替他把下一集放出来
    #[test]
    fn autoplay_is_off_by_default() {
        let app = app();
        assert!(!app.settings.autoplay);

        // 打开设置抽屉不会顺手把它勾上
        let mut app = app;
        let _ = app.update(Message::OpenSettings);
        assert!(!app.settings.autoplay);

        let _ = app.update(Message::SetAutoplay(true));
        assert!(app.settings.autoplay);
    }
    /// 倍速文案：整数倍也要带一位小数（`1.0×` 而不是 `1×`），1.25 / 1.75 不能丢位
    #[test]
    fn speed_label_is_readable() {
        assert_eq!(fmt_speed(1.0), "1.0×");
        assert_eq!(fmt_speed(0.5), "0.5×");
        assert_eq!(fmt_speed(0.7), "0.7×");
        assert_eq!(fmt_speed(1.25), "1.25×");
        assert_eq!(fmt_speed(1.75), "1.75×");
        assert_eq!(fmt_speed(2.0), "2.0×");
        assert_eq!(fmt_speed(8.0), "8.0×");
    }

    /// 预设覆盖用户点名的九档，并且全都落在自定义滑块的范围内
    #[test]
    fn speed_presets_match_the_range() {
        assert_eq!(SPEEDS, [0.5, 0.7, 1.0, 1.25, 1.5, 1.75, 2.0, 2.5, 3.0]);
        for s in SPEEDS {
            assert!(s >= SPEED_MIN && s <= SPEED_MAX, "{s} 超出滑块范围");
        }
        assert_eq!((SPEED_MIN, SPEED_MAX), (0.5, 8.0));
    }

    /// 调速会被夹回 0.5–8.0：滑块拖不出界，但消息是可以被构造的，兜一层更稳
    #[test]
    fn set_speed_clamps_and_rounds() {
        let mut app = app();
        assert_eq!(app.speed, 1.0);

        let _ = app.update(Message::SetSpeed(12.0));
        assert_eq!(app.speed, SPEED_MAX);

        let _ = app.update(Message::SetSpeed(0.01));
        assert_eq!(app.speed, SPEED_MIN);

        // 浮点误差不该让标签变成 "1.2500000000000002×"
        let _ = app.update(Message::SetSpeed(1.234567));
        assert_eq!(fmt_speed(app.speed), "1.23×");
    }

    /// 三条抽屉互相排斥；右键菜单打开时点画面只收菜单，不切播放
    #[test]
    fn info_and_context_stay_exclusive() {
        let mut app = app();

        let _ = app.update(Message::OpenInfo);
        assert!(app.info_open);
        assert!(app.any_drawer());

        let _ = app.update(Message::TogglePlaylist);
        assert!(app.playlist.open);
        assert!(!app.info_open, "开播放列表要把详情抽屉收起来");

        // 点透明 cover 全关
        let _ = app.update(Message::CoverClick);
        assert!(!app.playlist.open);
        assert!(!app.any_drawer());

        // 右键菜单
        app.cursor = Some(Point::new(300.0, 200.0));
        let _ = app.update(Message::ContextMenu);
        assert_eq!(app.context, Some(Point::new(300.0, 200.0)));

        // 这时点画面：菜单收起，而且不该触发播放
        let was = app.paused;
        let _ = app.update(Message::StagePress);
        assert!(app.context.is_none());
        let _ = app.update(Message::StageRelease);
        assert_eq!(app.paused, was, "收菜单的那一下不该切播放");
    }

    /// 换片转圈的时间线：先空一段时间（免得秒开的小文件闪一下），
    /// 再淡入，首帧一到立刻收掉。
    #[test]
    fn spinner_waits_then_fades_in_and_disappears_on_first_frame() {
        let mut app = app();
        // 假装刚点完「下一集」：文件名有了、画面还没来
        app.current = Some(std::path::PathBuf::from("x.mp4"));
        app.frame = None;
        let t0 = Instant::now();
        app.loading_since = Some(t0);

        // 用宏而不是闭包：闭包会把 `app` 借住，后面就没法改 `app.frame` 了
        macro_rules! at {
            ($ms:expr) => {
                app.spinner_opacity_at(t0 + Duration::from_millis($ms))
            };
        }

        // 门还没开
        assert_eq!(at!(0), 0.0, "刚换片就画圈，秒开的文件会闪一下");
        assert_eq!(at!(79), 0.0);

        // 门开了，正在淡入
        let mid = at!(80 + (SPIN_FADE * 1000.0) as u64 / 2);
        assert!(mid > 0.3 && mid < 0.7, "淡入到一半应该是半透明，实际 {mid}");

        // 真慢的时候完整亮起来，但不会超过 1
        assert_eq!(at!(80 + 2000), 1.0);
        assert_eq!(at!(80 + 60_000), 1.0);

        // 首帧到手 —— 不管之前算了多少，立刻归零
        app.frame = Some(screen());
        assert_eq!(at!(80 + 2000), 0.0, "有画面了还画圈就盖住视频了");

        // 报错了也不该转圈（错误面板自己会说清楚）
        app.frame = None;
        app.error = Some(String::from("打不开"));
        assert_eq!(at!(80 + 2000), 0.0);

        // 什么都没打开时（启动后的空白页）也不画
        app.error = None;
        app.current = None;
        assert_eq!(at!(80 + 2000), 0.0);
    }

    /// 换片时手上顶着上一帧（`stale_frame`）：画面是连着的，转圈要等更久才出现 ——
    /// 按 `SPIN_DELAY` 就画，会在每次切集时「暗一下又亮回来」，等于自己造一次闪。
    #[test]
    fn a_leftover_frame_delays_the_spinner() {
        let mut app = app();
        app.current = Some(std::path::PathBuf::from("next.mp4"));
        app.frame = Some(screen());
        app.stale_frame = true;
        let t0 = Instant::now();
        app.loading_since = Some(t0);

        macro_rules! at {
            ($ms:expr) => {
                app.spinner_opacity_at(t0 + Duration::from_millis($ms))
            };
        }

        // 平时的门（80ms）已经开了，但有旧画面顶着，这时候还不该画
        assert_eq!(at!(80 + 200), 0.0, "画面还连着的时候就画圈，会暗一下再亮回来");
        assert_eq!(at!(399), 0.0);

        // 真的卡住了（超过 STALE_SPIN_DELAY）才转起来
        assert_eq!(
            at!(STALE_SPIN_DELAY.as_millis() as u64 + 2000),
            1.0,
            "加载真的卡住了要给出反馈"
        );

        // 新片首帧到了：标记清掉，圈立刻收
        app.stale_frame = false;
        assert_eq!(at!(STALE_SPIN_DELAY.as_millis() as u64 + 2000), 0.0);
    }

    /// 歌词窗的留白必须「不为负 + 连续 + 当前行贴着中线」。
    ///
    /// 这三条一破，换句时的「往上滑」就会变成跳一格、或者整块卡住不动。
    #[test]
    fn lyric_window_slides_continuously_and_keeps_the_line_centered() {
        for height in [240.0f32, 300.0, 374.0, 420.0] {
            let half = height * 0.5 - LYRIC_ROW_H * 0.5;
            let mut last_y: Option<f32> = None;

            // 0 → 30 行，每帧推进 0.1 行（比 60fps 下的真实速度还慢，够密了）
            for step in 0..=300 {
                let scroll = step as f32 * 0.1;
                let (first, pad) = lyric_layout(scroll, height, LYRIC_ROW_H);

                assert!(pad >= 0.0, "留白不能是负的（iced 没有负 padding）");

                // 当前行（第 scroll 行）的顶边要一直贴在中线上
                let top = pad + (scroll - first as f32) * LYRIC_ROW_H;
                assert!(
                    (top - half).abs() < 0.51,
                    "当前行要贴在中线：{top} vs {half}（height={height}）"
                );

                // 截取第 10 行：只要它还在渲染范围里，纵向位置就该连续地往上走
                let i = 10usize;
                if i >= first {
                    let y = pad + (i - first) as f32 * LYRIC_ROW_H;
                    if let Some(prev) = last_y {
                        assert!(
                            (y - prev).abs() <= 0.1 * LYRIC_ROW_H + 0.05,
                            "整块要连续滑动，不能跳：{prev} → {y}"
                        );
                    }
                    last_y = Some(y);
                }
            }
        }
    }

    /// 放音频时点界面不该暂停：歌词区要能点着看，封面也不是开关。
    ///
    /// `view()` 里音频那一支已经不挂按下/松手了，这里测的是 `update()` 里的保险：
    /// 万一哪天手滑又挂回去，也不该把正在放的歌停掉。
    #[test]
    fn clicking_the_audio_stage_does_not_pause() {
        let mut app = app();
        app.current = Some(PathBuf::from("song.mp3"));
        app.paused = false;

        let _ = app.update(Message::StagePress);
        let _ = app.update(Message::StageRelease);

        assert!(!app.paused, "点音频界面不该把正在放的歌暂停");
        assert!(!app.stage_press, "按下标志要收干净，别留给下一条视频");

        // 视频那条路照旧：手上真有画面时点一下还是要暂停
        app.current = Some(PathBuf::from("movie.mp4"));
        app.frame = Some(video::VideoFrame {
            data: std::sync::Arc::new(vec![0; 4]),
            width: 1,
            height: 1,
        });
        let _ = app.update(Message::StagePress);
        let _ = app.update(Message::StageRelease);
        assert!(app.paused, "视频画面点一下还是暂停");
    }

    /// 换句必须走「弹簧」：先冲过目标一点，再弹回来落定。
    ///
    /// 这条盯的是 `animate_with` 里那一步。原来的写法是指数逼近
    /// （`approach`），它**永远不会越过目标**——数值超不过 1.0，「弹」就没了，
    /// 只剩「滑」。所以这里直接断言峰值 > 1.05。
    #[test]
    fn lyric_scroll_overshoots_the_next_line_and_settles() {
        let mut app = app();
        app.tags.lyrics = Some(meta::Lyrics::Synced(
            (0..4)
                .map(|i| meta::Line {
                    at: i as f64 * 4.0,
                    text: format!("第 {} 句", i + 1),
                })
                .collect(),
        ));
        app.lyric_line = Some(0);
        app.lyric_scroll.snap(0.0);

        // 唱到第二句
        app.lyric_line = Some(1);
        let mut peak = 0.0f32;
        let mut frames = 0;
        while frames < 180 {
            app.animate();
            peak = peak.max(app.lyric_scroll.value());
            frames += 1;
        }

        assert!(
            peak > 1.05,
            "滑过目标才叫弹簧，峰值只有 {peak}（是不是又退回成缓动了）"
        );
        assert!(peak < 1.4, "冲得太狠了，整列会像被甩出去：{peak}");
        assert!(
            (app.lyric_scroll.value() - 1.0).abs() < 0.01,
            "三秒之后还没落回第二句：{}",
            app.lyric_scroll.value()
        );
    }

    /// 拖进度条 / 刚换歌那种「一下跳到很远」的情况不该走弹簧：
    /// 整列飞过去看着像卡了，直接落位才对。
    #[test]
    fn a_far_seek_drops_the_lyrics_instead_of_flying() {
        let mut app = app();
        app.tags.lyrics = Some(meta::Lyrics::Synced(
            (0..40)
                .map(|i| meta::Line {
                    at: i as f64,
                    text: format!("第 {} 句", i + 1),
                })
                .collect(),
        ));
        app.lyric_line = Some(0);
        app.lyric_scroll.snap(0.0);

        app.lyric_line = Some(30);
        app.animate();
        assert_eq!(
            app.lyric_scroll.value(),
            30.0,
            "跳这么远应该直接落位，不该一帧一帧飞过去"
        );
    }

    /// 手动滚上去看词的时候，视图要**钉住**：播放继续往前唱，画面不能自己往下漂。
    /// 四秒没人滚，再滑回正在唱的那一句。
    #[test]
    fn browsing_pins_the_view_and_then_comes_back() {
        let mut app = app();
        app.tags.lyrics = Some(meta::Lyrics::Synced(
            (0..10)
                .map(|i| meta::Line {
                    at: i as f64 * 5.0,
                    text: format!("第 {} 句", i + 1),
                })
                .collect(),
        ));
        app.lyric_line = Some(2);
        app.lyric_scroll.snap(2.0);

        // 往上滚一句，停在第 0 句
        app.scroll_lyrics(-2.0);
        for _ in 0..90 {
            app.animate();
        }
        let shown = |a: &App| a.lyric_scroll.value() + a.lyric_browse.value();
        assert!((shown(&app) - 0.0).abs() < 0.05, "没滚到顶：{}", shown(&app));

        // 播放往前唱了两句：视图要还在原处
        app.lyric_line = Some(4);
        for _ in 0..90 {
            app.animate();
        }
        assert!(
            (shown(&app) - 0.0).abs() < 0.05,
            "浏览的时候被播放拖着走了：{}",
            shown(&app)
        );

        // 四秒没人动它，自己滑回当前句
        app.lyric_browse_at = Some(Instant::now() - LYRIC_BROWSE_HOLD - Duration::from_millis(20));
        for _ in 0..120 {
            app.animate();
        }
        assert!(
            (shown(&app) - 4.0).abs() < 0.1,
            "超时之后应该回到正在唱的那句：{}",
            shown(&app)
        );
    }

    /// 滚轮滚歌词：滚到第一句 / 最后一句就顶住，不能滚出一片空白。
    ///
    /// 「显示位置」= 跟随当前句的 `lyric_scroll` + 滚轮叠的 `lyric_browse`，
    /// 允许的范围是 `0 ..= 最后一行`。
    #[test]
    fn scrolling_lyrics_clamps_at_both_ends() {
        let mut app = app();
        app.tags.lyrics = Some(meta::Lyrics::Synced(
            (0..10)
                .map(|i| meta::Line {
                    at: i as f64 * 5.0,
                    text: format!("第 {} 句", i + 1),
                })
                .collect(),
        ));
        app.lyric_line = Some(4);
        app.lyric_scroll.snap(4.0);

        let shown = |app: &App| app.lyric_scroll.value() + app.lyric_browse.target();

        // 一个劲儿往下滚：最后一行贴中线就该停住
        for _ in 0..20 {
            app.scroll_lyrics(5.0);
        }
        assert_eq!(shown(&app), 9.0, "滚过头了，最后一行之后不该还有内容");

        // 再往上滚到头：第一行贴中线
        for _ in 0..40 {
            app.scroll_lyrics(-5.0);
        }
        assert_eq!(shown(&app), 0.0, "滚到顶之后不该继续往负数走");
    }

    /// Ctrl + 滚轮调字号，两头都有硬限制（再小看不清、再大放不下）。
    #[test]
    fn ctrl_wheel_resizes_lyrics_within_limits() {
        let mut app = app();
        assert_eq!(app.lyric_size, LYRIC_SIZE_IDLE);

        app.ctrl = true;
        // 向上滚（`LyricWheel` 里是负的行数）放大
        for _ in 0..80 {
            let _ = app.update(Message::LyricWheel(-1.0));
        }
        assert_eq!(app.lyric_size, LYRIC_SIZE_MAX, "字号没有上限");

        for _ in 0..200 {
            let _ = app.update(Message::LyricWheel(1.0));
        }
        assert_eq!(app.lyric_size, LYRIC_SIZE_MIN, "字号没有下限");
    }

    /// 不按 Ctrl 的滚轮只滚歌词，不许偷偷把字号改了。
    #[test]
    fn plain_wheel_scrolls_lyrics_and_leaves_the_size_alone() {
        let mut app = app();
        app.tags.lyrics = Some(meta::Lyrics::Synced(
            (0..10)
                .map(|i| meta::Line {
                    at: i as f64 * 5.0,
                    text: format!("第 {} 句", i + 1),
                })
                .collect(),
        ));
        app.lyric_line = Some(4);
        app.lyric_scroll.snap(4.0);
        app.ctrl = false;

        let _ = app.update(Message::LyricWheel(-2.0));
        assert_eq!(app.lyric_size, LYRIC_SIZE_IDLE, "滚歌词不该动字号");
        assert!(
            app.lyric_browse.target() < 0.0,
            "往上滚应该看到前面的词（偏移为负）"
        );
    }

    /// 点某一行歌词：播放跳过去，歌词也当场对到那一句（不用弹簧慢慢滑）。
    #[test]
    fn clicking_a_line_seeks_playback_there() {
        let mut app = app();
        app.current = Some(PathBuf::from("song.mp3"));
        app.duration = 100.0;
        app.position = 3.0;
        app.tags.lyrics = Some(meta::Lyrics::Synced(
            (0..4)
                .map(|i| meta::Line {
                    at: i as f64 * 10.0,
                    text: format!("第 {} 句", i + 1),
                })
                .collect(),
        ));
        app.lyric_line = Some(0);
        app.lyric_scroll.snap(0.0);
        // 假装用户刚滚到别处看过
        app.lyric_browse.set_target(2.0);

        let _ = app.update(Message::LyricSeek(3));

        assert_eq!(app.position, 30.0, "要跳到第三句开始唱的地方");
        assert_eq!(app.lyric_line, Some(3));
        assert_eq!(app.lyric_scroll.value(), 3.0, "歌词要当场落位");
        assert_eq!(app.lyric_browse.value(), 0.0, "跳转之后滚动偏移要清零");
    }

    /// 抽屉开着的时候，那层透明 cover 会盖住歌词。点下去不能白费：
    /// 既要跳到点中的那一句，也要把抽屉关掉。
    #[test]
    fn a_cover_click_over_the_lyrics_still_seeks() {
        let mut app = app();
        app.current = Some(PathBuf::from("song.mp3"));
        app.duration = 100.0;
        app.position = 0.0;
        app.tags.lyrics = Some(meta::Lyrics::Synced(
            (0..6)
                .map(|i| meta::Line {
                    at: i as f64 * 10.0,
                    text: format!("第 {} 句", i + 1),
                })
                .collect(),
        ));
        app.lyric_line = Some(2);
        app.lyric_scroll.snap(2.0);
        app.playlist.open = true;

        // 歌词面板正中：面板在窗口里垂直居中，横向占右栏的中间一半
        let (w, h) = (app.win_size.width, app.win_size.height);
        let text_w = w * 0.55 * 0.5;
        let cx = w * 0.45 + (w * 0.55 - text_w) * 0.5 + text_w * 0.5;
        app.cursor = Some(Point::new(cx, h * 0.5));

        let _ = app.update(Message::CoverClick);

        assert_eq!(app.position, 20.0, "正中那行是第 2 句，要跳到它唱的地方");
        assert!(!app.playlist.open, "顺手还得把抽屉关掉");
    }

    /// 同一层 cover，点在歌词面板外面（封面那边）：只关抽屉，不动播放位置。
    #[test]
    fn a_cover_click_outside_the_lyrics_just_closes_the_drawer() {
        let mut app = app();
        app.current = Some(PathBuf::from("song.mp3"));
        app.duration = 100.0;
        app.position = 42.0;
        app.tags.lyrics = Some(meta::Lyrics::Synced(
            (0..6)
                .map(|i| meta::Line {
                    at: i as f64 * 10.0,
                    text: format!("第 {} 句", i + 1),
                })
                .collect(),
        ));
        app.lyric_line = Some(2);
        app.lyric_scroll.snap(2.0);
        app.playlist.open = true;

        // 左边 45% 是封面区，不该算点歌词
        let (w, h) = (app.win_size.width, app.win_size.height);
        app.cursor = Some(Point::new(w * 0.2, h * 0.5));

        let _ = app.update(Message::CoverClick);

        assert_eq!(app.position, 42.0, "点封面不该动播放位置");
        assert!(!app.playlist.open, "抽屉照样关掉");
    }

    /// 视频没有歌词面板，那套几何不生效 —— 点哪儿都只关抽屉。
    #[test]
    fn the_lyrics_hit_box_only_exists_in_audio_mode() {
        let mut app = app();
        app.current = Some(PathBuf::from("movie.mp4"));
        app.tags.lyrics = Some(meta::Lyrics::Synced(
            (0..6)
                .map(|i| meta::Line {
                    at: i as f64 * 10.0,
                    text: format!("第 {} 句", i + 1),
                })
                .collect(),
        ));

        let (w, h) = (app.win_size.width, app.win_size.height);
        assert!(
            app.lyric_line_at(Point::new(w * 0.75, h * 0.5)).is_none(),
            "视频模式下不该有歌词命中区"
        );
    }

    /// 滚轮量：鼠标一格（Lines）和触摸板（Pixels）都要换算成内向的行数。
    #[test]
    fn wheel_delta_turns_into_rows() {
        // 往上滚（y 为正）= 看前面的词 = 负的行数
        let up = wheel_rows(mouse::ScrollDelta::Lines { x: 0.0, y: 1.0 });
        let down = wheel_rows(mouse::ScrollDelta::Lines { x: 0.0, y: -1.0 });
        assert_eq!(up, -LYRIC_WHEEL_ROWS);
        assert_eq!(down, LYRIC_WHEEL_ROWS);

        // 触摸板的像素量：连续滚动，方向一致、量级和「行」对得上
        let px = wheel_rows(mouse::ScrollDelta::Pixels { x: 0.0, y: -40.0 });
        assert!((px - LYRIC_WHEEL_ROWS).abs() < 0.01);
    }

    /// 亮度和字号按「离正中间几行」连续算：正中 1.0 / 一行开外 0.60，
    /// 一路单调变淡，到 `LYRIC_FADE_ROWS` 行外正好看不见。
    ///
    /// 连续是关键 —— 分档跳的话，弹簧滑到一半时那一行会突然变亮变白。
    #[test]
    fn lyric_alpha_and_size_fall_off_smoothly() {
        assert_eq!(lyric_alpha(0.0, true), LYRIC_ALPHA_ACTIVE);
        assert!((lyric_alpha(1.0, true) - LYRIC_ALPHA_NEAR).abs() < 1e-5);
        assert_eq!(lyric_alpha(LYRIC_FADE_ROWS as f32, true), 0.0);

        let mut prev = lyric_alpha(0.0, true);
        let mut d = 0.0;
        while d < LYRIC_FADE_ROWS as f32 {
            d += 0.125;
            let now = lyric_alpha(d, true);
            assert!(now <= prev + 1e-6, "亮度要一路变淡，{d} 行反而更亮了");
            prev = now;
        }

        // 前奏（还没唱到第一句）不该有一行是纯白的
        assert!(lyric_alpha(0.0, false) <= LYRIC_ALPHA_NEAR + 1e-6);

        // 字号：正中最大，一行开外回到常态，中间单调
        assert_eq!(lyric_size(0.0, LYRIC_SIZE_IDLE), LYRIC_SIZE_ACTIVE);
        assert_eq!(lyric_size(1.0, LYRIC_SIZE_IDLE), LYRIC_SIZE_IDLE);
        assert!(lyric_size(0.5, LYRIC_SIZE_IDLE) > lyric_size(0.75, LYRIC_SIZE_IDLE));
    }

    /// 换片空档里按空格：手上那帧只是占位的，不该被当成「正在播」去翻暂停，
    /// 更不该给正在加载的 mpv 塞一条暂停命令。
    #[test]
    fn space_during_a_switch_does_not_pause_the_incoming_file() {
        let mut app = playing_at(10.0);
        app.frame = Some(screen());
        app.stale_frame = true;

        let _ = app.update(Message::TogglePlay);
        assert!(!app.paused, "换片空档不该把新片按成暂停");
    }

    /// 点完播放/暂停的那一下，按钮必须**立刻**变成新状态，并且不被队列里的
    /// 旧快照翻回去 —— 这就是「暂停时按钮闪一下」的根因。
    #[test]
    fn pause_toggle_ignores_stale_snapshots_until_echo_arrives() {
        let mut app = app();
        app.frame = Some(screen());
        app.paused = false;

        // 点一下暂停：本地立刻是 paused，同时记下发送时刻
        app.toggle_play();
        assert!(app.paused, "按钮要立刻变成「播放」");
        assert!(app.pause_sent.is_some());

        // 队列里那条旧快照（还写着「在放」）不能把它翻回去
        assert_eq!(app.resolve_paused(false), None, "旧快照不该采信");
        assert!(app.paused, "按钮不能闪回暂停图标");

        // 真正的回声到了：采信，并且以后不再拦
        assert_eq!(app.resolve_paused(true), Some(true));
        assert!(app.pause_sent.is_none(), "追上之后要停止拦截");
        assert_eq!(app.resolve_paused(false), Some(false), "之后的快照照常同步");
    }

    /// 兜底：mpv 万一不认这条命令，也不能让按钮永远停在用户点出来的状态上
    #[test]
    fn pause_toggle_gives_up_after_timeout() {
        let mut app = app();
        app.frame = Some(screen());
        app.paused = true;
        // 假装那一下是 300ms 之前发的（已经超过 PAUSE_ECHO）
        app.pause_sent = Some(Instant::now() - PAUSE_ECHO);

        // 快照跟本地不一致，但也不能再拦了
        assert_eq!(app.resolve_paused(false), Some(false));
        assert!(app.pause_sent.is_none());
    }

    /// 暂停不再是「栏永远挂着」的理由：鼠标静置够久，两条栏照样收起来
    #[test]
    fn bars_hide_while_paused_too() {
        let mut app = app();
        app.frame = Some(screen());
        app.paused = true;
        app.cursor = Some(Point::new(400.0, 300.0)); // 停在画面中间，不在栏上
        app.last_active = Instant::now() - HIDE_AFTER - Duration::from_millis(50);
        assert!(!app.want_bars(), "暂停 + 鼠标闲着，栏应该收起来");

        // 鼠标移到顶栏上：不能收，不然点不到按钮
        app.cursor = Some(Point::new(400.0, 10.0));
        assert!(app.want_bars(), "鼠标停在栏上时必须留着");

        // 刚动过鼠标：留着
        app.cursor = Some(Point::new(400.0, 300.0));
        app.last_active = Instant::now();
        assert!(app.want_bars(), "刚动过鼠标时不能马上收");

        // 压根没片子（启动后的空白页）：栏也要一直留着
        app.frame = None;
        app.last_active = Instant::now() - HIDE_AFTER - Duration::from_millis(50);
        assert!(app.want_bars(), "空白页上栏不能收");
    }

    /// 转圈的相位只在换片期间往前走，转满一圈要回到 0（不然浮点数会越加越大）
    #[test]
    fn spinner_phase_wraps_once_per_turn() {
        let mut app = app();
        app.current = Some(std::path::PathBuf::from("x.mp4"));
        app.loading_since = Some(Instant::now());
        app.spin = 0.0;

        let steps = (spinner::TURN / (1.0 / 60.0)).ceil() as usize + 2;
        for _ in 0..steps {
            app.animate();
            assert!(
                (0.0..1.0).contains(&app.spin),
                "相位跑出 0..1 了：{}",
                app.spin
            );
        }
        // 每帧 1/60 秒，转一圈 0.9 秒 = 54 帧，走 56 帧应该已经绕回来了
        assert!(app.spin < 0.2, "绕一圈之后没有回卷：{}", app.spin);
    }

    // ---------------- 方向键 ----------------

    /// 造一个「正在放 60 秒的片子、停在 10 秒」的 App
    fn playing_at(secs: f64) -> App {
        let mut app = app();
        app.current = Some(PathBuf::from(r"C:\a\b\movie.mkv"));
        app.duration = 60.0;
        app.position = secs;
        app.paused = false;
        app
    }

    #[test]
    fn arrow_keys_seek_five_seconds_and_stop_at_the_ends() {
        let mut app = playing_at(10.0);

        let _ = app.update(Message::Nudge(SEEK_STEP));
        assert_eq!(app.position, 15.0, "→ 应该快进 5 秒");

        // 一路快进到结尾：夹在时长上，不越界
        app.seek_at = None;
        let _ = app.update(Message::Nudge(SEEK_STEP * 100.0));
        assert_eq!(app.position, 60.0, "快进不能超过总时长");

        app.seek_at = None;
        let _ = app.update(Message::Nudge(-SEEK_STEP * 100.0));
        assert_eq!(app.position, 0.0, "后退不能变成负数");
    }

    #[test]
    fn holding_an_arrow_is_throttled() {
        let mut app = playing_at(10.0);

        let _ = app.update(Message::Nudge(SEEK_STEP));
        assert_eq!(app.position, 15.0);

        // 60ms 之内的 repeat（按住不放时 winit 每 ~30ms 发一次）必须丢掉
        let _ = app.update(Message::Nudge(SEEK_STEP));
        assert_eq!(app.position, 15.0, "60ms 内的连发要被节流掉");

        // 过了节流窗口就正常认
        app.seek_at = Some(Instant::now() - Duration::from_millis(80));
        let _ = app.update(Message::Nudge(SEEK_STEP));
        assert_eq!(app.position, 20.0);
    }

    #[test]
    fn arrow_keys_move_volume_by_five() {
        let mut app = app();
        app.volume = 50.0;

        let _ = app.update(Message::NudgeVolume(VOL_STEP));
        assert_eq!(app.volume, 55.0, "↑ 应该加 5");

        let _ = app.update(Message::NudgeVolume(-VOL_STEP));
        assert_eq!(app.volume, 50.0, "↓ 应该减 5");

        let _ = app.update(Message::NudgeVolume(-VOL_STEP * 100.0));
        assert_eq!(app.volume, 0.0, "音量不能低于 0");

        let _ = app.update(Message::NudgeVolume(VOL_STEP * 100.0));
        assert_eq!(app.volume, 100.0, "音量不能高于 100");
    }

    // ---------------- 音频 ----------------

    #[test]
    fn bars_hide_for_audio_even_though_there_is_no_frame() {
        let mut app = app();
        app.current = Some(PathBuf::from(r"C:\a\b\song.mp3"));
        app.frame = None;
        assert!(app.is_audio(), "mp3 应该被认成音频");

        app.cursor = Some(Point::new(400.0, 300.0));
        app.last_active = Instant::now() - HIDE_AFTER - Duration::from_millis(50);
        assert!(!app.want_bars(), "放音频时静置也要收栏");
    }

    #[test]
    fn space_toggles_audio_without_a_picture() {
        let mut app = app();
        app.current = Some(PathBuf::from(r"C:\a\b\song.mp3"));
        app.frame = None;
        app.paused = false;

        let _ = app.update(Message::TogglePlay);
        assert!(app.paused, "音频没有 frame 也要能切暂停");

        let _ = app.update(Message::TogglePlay);
        assert!(!app.paused, "再按一次要能恢复");
    }
}
