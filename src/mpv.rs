//! libmpv 的最小 FFI 封装。
//!
//! 只暴露播放器真正需要的东西：加载文件、读取属性、软件渲染出 RGBA 帧。
//! 常量全部取自 mpv 官方头文件 include/mpv/{client.h,render.h}。

use libloading::Library;
use std::ffi::{CStr, CString, c_char, c_int, c_void};
use std::path::PathBuf;
use std::ptr;
use std::time::{Duration, Instant};

const FORMAT_STRING: c_int = 1;
const FORMAT_INT64: c_int = 4;
const FORMAT_DOUBLE: c_int = 5;

// mpv_render_param_type（见 render.h）
const PARAM_API_TYPE: c_int = 1;
const PARAM_SW_SIZE: c_int = 17;
const PARAM_SW_FORMAT: c_int = 18;
const PARAM_SW_STRIDE: c_int = 19;
const PARAM_SW_POINTER: c_int = 20;

/// mpv_render_context_update() 的返回值，置位表示有新帧可用。
pub const RENDER_UPDATE_FRAME: u64 = 1 << 0;

#[repr(C)]
struct RenderParam {
    type_: c_int,
    data: *mut c_void,
}

type FnCreate = unsafe extern "C" fn() -> *mut c_void;
type FnInitialize = unsafe extern "C" fn(*mut c_void) -> c_int;
type FnTerminateDestroy = unsafe extern "C" fn(*mut c_void);
type FnSetOptionString = unsafe extern "C" fn(*mut c_void, *const c_char, *const c_char) -> c_int;
type FnProperty = unsafe extern "C" fn(*mut c_void, *const c_char, c_int, *mut c_void) -> c_int;
type FnSetPropString = unsafe extern "C" fn(*mut c_void, *const c_char, *const c_char) -> c_int;
type FnCommand = unsafe extern "C" fn(*mut c_void, *mut *const c_char) -> c_int;
type FnFree = unsafe extern "C" fn(*mut c_void);
type FnWaitEvent = unsafe extern "C" fn(*mut c_void, f64) -> *const c_void;
type FnRenderCreate = unsafe extern "C" fn(*mut *mut c_void, *mut c_void, *mut RenderParam) -> c_int;
type FnRenderRender = unsafe extern "C" fn(*mut c_void, *mut RenderParam) -> c_int;
type FnRenderUpdate = unsafe extern "C" fn(*mut c_void) -> u64;
type FnRenderFree = unsafe extern "C" fn(*mut c_void);

type FnErrorString = unsafe extern "C" fn(c_int) -> *const c_char;

#[derive(Default)]
pub struct PumpOutcome {
    pub ended: bool,
    pub error: Option<String>,
}

/// 一条轨道（`track-list` 里的一项）。
///
/// 只需要字幕用得上的字段：`id` 用于 `sid` 选择，`external` 区分「内嵌」和
/// 「我们 sub-add 进去的本地文件」，标题/语言用来在设置面板里给人看。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Track {
    pub id: i64,
    /// "video" | "audio" | "sub"
    pub kind: String,
    pub title: Option<String>,
    pub lang: Option<String>,
    pub external: bool,
}

impl Track {
    /// 面板上显示的名字：优先标题，退而求其次用语言，都没有就叫「字幕 N」
    pub fn label(&self) -> String {
        let mut s = self
            .title
            .clone()
            .or_else(|| self.lang.clone())
            .unwrap_or_else(|| format!("字幕 {}", self.id));
        if let Some(lang) = &self.lang
            && self.title.is_some()
        {
            s = format!("{s}（{lang}）");
        }
        s
    }
}

/// 一个文件的元信息，「文件详情」抽屉用。
///
/// 全部来自 mpv 的属性表 —— 容器解析交给 ffmpeg，我们不去啃 MP4/MKV 的 box。
/// 每个字段都是 `Option`：直播流没有时长、纯音频没有分辨率、部分封装没有码率，
/// 拿不到就让界面显示「—」，不要编一个 0 出来。
#[derive(Debug, Clone, Default)]
pub struct MediaInfo {
    /// mpv 报告的当前文件路径
    pub path: String,
    /// 容器 / 封装，如 "Matroska"
    pub format: Option<String>,
    /// 字节数
    pub size: Option<f64>,
    pub width: Option<i64>,
    pub height: Option<i64>,
    /// 形如 "h264 (High)"
    pub video_codec: Option<String>,
    /// bit/s
    pub video_bitrate: Option<f64>,
    pub fps: Option<f64>,
    pub audio_codec: Option<String>,
    pub audio_channels: Option<i64>,
    pub sample_rate: Option<i64>,
    pub audio_bitrate: Option<f64>,
    pub duration: Option<f64>,
}


pub struct Mpv {
    // Library 必须比所有函数指针活得久
    _lib: Library,
    ctx: *mut c_void,
    render: *mut c_void,
    api_sw: CString,
    fmt_rgb0: CString,
    set_property_string: FnSetPropString,
    set_property: FnProperty,
    get_property: FnProperty,
    command: FnCommand,
    free: FnFree,
    wait_event: FnWaitEvent,
    render_frame: FnRenderRender,
    render_update: FnRenderUpdate,
    render_free: FnRenderFree,
    terminate_destroy: FnTerminateDestroy,
    error_string: FnErrorString,
}

unsafe fn sym<T: Copy>(lib: &Library, name: &str) -> Result<T, String> {
    let cname = CString::new(name).map_err(|e| e.to_string())?;
    unsafe {
        let s = lib
            .get::<T>(cname.to_bytes_with_nul())
            .map_err(|e| format!("找不到符号 {name}: {e}"))?;
        Ok(*s)
    }
}

/// 找出 libmpv-2.dll 的绝对路径。
///
/// **不能依赖相对路径**：双击 exe 或在资源管理器里把视频拖到 exe 图标上时，
/// 进程的当前工作目录是 exe 所在目录（`target\release`），而不是项目根目录，
/// `vendor/libmpv-2.dll` 这种相对路径必然找不到。
fn libmpv_candidates() -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    let dll = "libmpv-2.dll";

    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            // 1) exe 同目录（发布时把 DLL 摆这里最稳，也是安装后的形态）
            out.push(dir.join(dll));
            // 2) exe 同目录下的 vendor\（安装包把 DLL 放在 resources\ 或 vendor\ 都能兜住）
            out.push(dir.join("vendor").join(dll));
            // 3) 从 exe 目录逐级向上找 vendor/libmpv-2.dll
            //    target/release -> target -> 项目根，开发时走这条
            let mut up = dir.parent();
            for _ in 0..6 {
                let Some(d) = up else { break };
                out.push(d.join("vendor").join(dll));
                up = d.parent();
            }
        }
    }

    // 3) 兜底：当前工作目录
    out.push(PathBuf::from(dll));
    out.push(PathBuf::from("vendor").join(dll));
    out
}

fn load_libmpv() -> Result<Library, String> {
    let mut tried: Vec<String> = Vec::new();
    for p in libmpv_candidates() {
        if !p.is_file() {
            continue;
        }
        match unsafe { Library::new(&p) } {
            Ok(l) => return Ok(l),
            Err(e) => tried.push(format!("{}: {e}", p.display())),
        }
    }
    if tried.is_empty() {
        let listed = libmpv_candidates()
            .iter()
            .map(|p| p.display().to_string())
            .collect::<Vec<_>>()
            .join("\n  ");
        return Err(format!(
            "找不到 libmpv-2.dll，已查找以下位置：\n  {listed}\n\
             请把该文件放到 exe 同目录，或放回项目的 vendor\\ 目录。"
        ));
    }
    Err(format!("libmpv-2.dll 加载失败：\n  {}", tried.join("\n  ")))
}

/// 一个 mpv 实例是拿来干什么的。三种用途的选项差别不小，所以做成显式模式，
/// 而不是一堆布尔开关。
#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    /// 真正放片子：渲染上下文 + 正常解码
    Play,
    /// 只读时长：不建渲染上下文，安静到极点
    Probe,
    /// 抽帧（进度条悬停预览）：要渲染上下文，但不出声、不往后播、只用软解
    Frames,
}

impl Mpv {
    /// 正常播放用的实例：软件渲染上下文 + 一整套「几十 GB 也能秒开」的选项
    pub fn new() -> Result<Self, String> {
        Self::build(Mode::Play)
    }

    /// 只用来读时长的实例：不建渲染上下文、不接声卡、不开画面。
    ///
    /// 播放列表要显示每条的时长，而扫目录时不能真的去解一遍文件（几百个文件
    /// 会把 UI 卡住）。所以另开一个 mpv，一条条 `loadfile` 读 `duration` ——
    /// 它只解容器头，不解码，几十毫秒一条。用独立实例是因为主播放器正在放片子，
    /// 不能拿它去开别的文件。
    pub fn probe() -> Result<Self, String> {
        Self::build(Mode::Probe)
    }

    /// 抽帧用的实例：要渲染上下文（不然拿不到画面），但要**安静** ——
    /// 不出声、光有画面也不往后放、软解不用显卡。
    ///
    /// 进度条悬停缩略图就靠它：它是独立实例，所以再怎么 seek 都不会打断
    /// 正在播放的那一个。
    pub fn frames() -> Result<Self, String> {
        Self::build(Mode::Frames)
    }

    fn build(mode: Mode) -> Result<Self, String> {
        let with_render = mode != Mode::Probe;
        let lib = load_libmpv()?;

        unsafe {
            let create: FnCreate = sym(&lib, "mpv_create")?;
            let initialize: FnInitialize = sym(&lib, "mpv_initialize")?;
            let set_option: FnSetOptionString = sym(&lib, "mpv_set_option_string")?;
            let set_property_string: FnSetPropString = sym(&lib, "mpv_set_property_string")?;
            let set_property: FnProperty = sym(&lib, "mpv_set_property")?;
            let get_property: FnProperty = sym(&lib, "mpv_get_property")?;
            let command: FnCommand = sym(&lib, "mpv_command")?;
            let free: FnFree = sym(&lib, "mpv_free")?;
            let wait_event: FnWaitEvent = sym(&lib, "mpv_wait_event")?;
            let render_create: FnRenderCreate = sym(&lib, "mpv_render_context_create")?;
            let render_frame: FnRenderRender = sym(&lib, "mpv_render_context_render")?;
            let render_update: FnRenderUpdate = sym(&lib, "mpv_render_context_update")?;
            let render_free: FnRenderFree = sym(&lib, "mpv_render_context_free")?;
            let terminate_destroy: FnTerminateDestroy = sym(&lib, "mpv_terminate_destroy")?;
            let error_string: FnErrorString = sym(&lib, "mpv_error_string")?;

            let ctx = create();
            if ctx.is_null() {
                return Err("mpv_create 失败".into());
            }

            // 允许用环境变量覆盖（便于对比不同硬解策略的启动开销）
            let hwdec = std::env::var("QP_HWDEC").unwrap_or_else(|_| String::from("auto-safe"));
            // 抽帧实例一律软解：缩略图本来就是 CPU 缩放出来的，走硬解只会多一次
            // 「解到显存再拷回来」，还容易和主播放器抢同一个解码器会话。
            let hwdec = if mode == Mode::Frames {
                String::from("no")
            } else {
                hwdec
            };

            // ---- 这些选项决定了「几十 GB 也能秒开」 ----
            // 不读用户配置、不跑脚本、不要内置 OSD；空闲待命而不是直接退出
            let vo = if with_render { "libmpv" } else { "null" };
            let mut options: Vec<(&str, String)> = vec![
                ("config", "no".into()),
                ("load-scripts", "no".into()),
                ("osc", "no".into()),
                ("osd-level", "0".into()),
                ("terminal", "no".into()),
                ("idle", "yes".into()),
                ("keep-open", "yes".into()),
                // 用渲染 API 出帧，而不是让 mpv 自己开窗口
                ("vo", vo.into()),
                ("hwdec", hwdec),
                // 关键：本地文件不预读、缓冲上限固定，内存占用与文件大小无关
                ("cache", "no".into()),
                ("demuxer-max-bytes", "64MiB".into()),
                ("demuxer-max-back-bytes", "16MiB".into()),
                // ---- 打开阶段的三把刀，全是为了首帧更快 ----
                // 流分析时长上限 1 秒（默认会读到 5 秒）
                ("demuxer-lavf-analyzeduration", "1".into()),
                // 探测只读 32 KiB（ffmpeg 默认 5 MiB，网络盘/机械盘上差别很大）
                ("demuxer-lavf-probesize", "32768".into()),
                // 不预读 1 秒，够出一帧就交出来（默认 1.0）
                ("demuxer-readahead-secs", "0.5".into()),
                // 拖动进度条时精确定位
                ("hr-seek", "yes".into()),
                ("input-default-bindings", "no".into()),
                ("input-vo-keyboard", "no".into()),
                ("msg-level", "all=no".into()),
                ("volume-max", "200".into()),
                ("save-position-on-quit", "no".into()),
                ("audio-client-name", "Maneki".into()),
            ];

            if !with_render {
                // 探测要「安静」：不出声、不动画面、解出头部就够。
                //
                // 这里**不能**再关掉音频（`audio=no`）：纯音频文件本来就只有音轨，
                // 一关掉 mpv 就既不建轨道也不报 duration，列表里那首歌的时长会
                // 一直是 `--:--`，而播放这边又拿不到可靠时长。`ao=null` 已经保证
                // 不出声了，`pause=yes` 也保证它不会真的往后解码。
                options.push(("ao", "null".into()));
                options.push(("pause", "yes".into()));
            } else if mode == Mode::Frames {
                // 抽帧也要安静，但它得有画面：出帧靠渲染 API，声音和「往后放」
                // 都不需要。`pause=yes` 是关键 —— 不这样的话 loadfile 之后它
                // 会自己往后播，我们 seek 过去的那一帧还没画出来就被冲掉了。
                options.push(("ao", "null".into()));
                options.push(("aid", "no".into()));
                options.push(("pause", "yes".into()));
                // 抽帧要快，因为它每次都要从关键帧解到目标帧。4K HDR 的片子上
                // 单线程解码一次要 1.4 秒（实测），多开几条能把等待压下来，
                // 而它只在鼠标停在进度条上时才动，平时是闲着的。
                options.push(("vd-lavc-threads", "0".into()));
                // 缩略图只有 160 宽，去块滤波的细节根本看不出来，跳过它省一大截
                // 解码时间（H.264/HEVC 上很可观）。
                options.push(("vd-lavc-skiploopfilter", "all".into()));
                // 缩放用双线性就够了：3840→160 之后三次插值和双线性看不出差别，
                // 但双线性快得多。
                options.push(("scale", "bilinear".into()));
                options.push(("dscale", "bilinear".into()));
            }

            for (k, v) in &options {
                let ck = CString::new(*k).unwrap();
                let cv = CString::new(v.as_str()).unwrap();
                let _ = set_option(ctx, ck.as_ptr(), cv.as_ptr());
            }

            let rc = initialize(ctx);
            if rc < 0 {
                terminate_destroy(ctx);
                return Err(format!("mpv_initialize 失败（{rc}）"));
            }

            // 软件渲染上下文：mpv 解码后直接交给我们一块 RGBA 内存
            let api_sw = CString::new("sw").unwrap();
            let fmt_rgb0 = CString::new("rgb0").unwrap();
            let mut render: *mut c_void = ptr::null_mut();

            if with_render {
                let mut params = [RenderParam {
                    type_: PARAM_API_TYPE,
                    data: api_sw.as_ptr() as *mut c_void,
                }];
                let rc = render_create(&mut render, ctx, params.as_mut_ptr());
                if rc < 0 {
                    terminate_destroy(ctx);
                    return Err(format!("mpv_render_context_create 失败（{rc}）"));
                }
            }

            Ok(Self {
                _lib: lib,
                ctx,
                render,
                api_sw,
                fmt_rgb0,
                set_property_string,
                set_property,
                get_property,
                command,
                free,
                wait_event,
                render_frame,
                render_update,
                render_free,
                terminate_destroy,
                error_string,
            })
        }
    }

    /// 打开文件。mpv 只读文件头并按需 seek，不会把整个文件读进内存。
    ///
    /// 返回 Err 时给出人话原因；成功不代表能解码，解码失败走 `pump_events`。
    pub fn load(&self, path: &str) -> Result<(), String> {
        let p = std::path::Path::new(path);
        if !p.exists() {
            return Err(format!("文件不存在：{path}"));
        }
        if p.is_dir() {
            return Err(format!("这是一个文件夹，不是视频文件：{path}"));
        }
        if let Ok(m) = std::fs::metadata(p) {
            if m.len() == 0 {
                return Err(format!("文件是空的（0 字节）：{path}"));
            }
        }
        // 能不能真的读出来（U 盘拔了、权限不够都会在这里暴露）
        if std::fs::File::open(p).is_err() {
            return Err(format!("文件无法读取（权限或磁盘问题）：{path}"));
        }

        let cpath = CString::new(path).map_err(|_| String::from("路径里含有非法字符（NUL）"))?;
        let mut args: [*const c_char; 4] = [
            c"loadfile".as_ptr(),
            cpath.as_ptr(),
            c"replace".as_ptr(),
            ptr::null(),
        ];
        let rc = unsafe { (self.command)(self.ctx, args.as_mut_ptr()) };
        if rc < 0 {
            return Err(self.describe(rc));
        }
        Ok(())
    }

    /// 停掉当前文件（探测实例每读完一个时长就调一次，好释放句柄）
    pub fn stop(&self) {
        let mut args: [*const c_char; 2] = [c"stop".as_ptr(), ptr::null()];
        unsafe {
            (self.command)(self.ctx, args.as_mut_ptr());
        }
    }

    pub fn set_flag(&self, name: &str, value: bool) {
        let Ok(n) = CString::new(name) else { return };
        let v = CString::new(if value { "yes" } else { "no" }).unwrap();
        unsafe {
            (self.set_property_string)(self.ctx, n.as_ptr(), v.as_ptr());
        }
    }

    pub fn set_double(&self, name: &str, value: f64) {
        let Ok(n) = CString::new(name) else { return };
        let mut v = value;
        unsafe {
            (self.set_property)(self.ctx, n.as_ptr(), FORMAT_DOUBLE, &mut v as *mut f64 as *mut c_void);
        }
    }

    pub fn set_int(&self, name: &str, value: i64) {
        let Ok(n) = CString::new(name) else { return };
        let mut v = value;
        unsafe {
            (self.set_property)(self.ctx, n.as_ptr(), FORMAT_INT64, &mut v as *mut i64 as *mut c_void);
        }
    }

    /// 选字幕轨。`None` 表示关掉字幕（`sid` 设成 "no"）。
    pub fn set_sid(&self, id: Option<i64>) {
        match id {
            Some(id) => self.set_int("sid", id),
            None => {
                let Ok(n) = CString::new("sid") else { return };
                let v = CString::new("no").unwrap();
                unsafe {
                    (self.set_property_string)(self.ctx, n.as_ptr(), v.as_ptr());
                }
            }
        }
    }

    /// 播放速度倍率。mpv 默认 `audio-pitch-correction=yes`，变速不变调。
    ///
    /// 注意这个属性在 `loadfile` 之后会被重置成 1.0（mpv 每个文件重新初始化
    /// 播放器状态），所以自动连播切下一集时必须重新下发一次。
    pub fn set_speed(&self, speed: f64) {
        self.set_double("speed", speed.clamp(0.1, 8.0));
    }

    /// 挂一个本地字幕文件并立刻选中它（mpv 会把它当外挂轨加进 track-list）。
    pub fn sub_add(&self, path: &str) -> Result<(), String> {
        let p = std::path::Path::new(path);
        if !p.is_file() {
            return Err(format!("字幕文件不存在：{path}"));
        }
        let cpath = CString::new(path).map_err(|_| String::from("路径里含有非法字符（NUL）"))?;
        let mut args: [*const c_char; 4] = [
            c"sub-add".as_ptr(),
            cpath.as_ptr(),
            c"select".as_ptr(),
            ptr::null(),
        ];
        let rc = unsafe { (self.command)(self.ctx, args.as_mut_ptr()) };
        if rc < 0 {
            return Err(self.describe(rc));
        }
        Ok(())
    }

    /// 把当前这一帧存成图片文件（格式看后缀，mpv 自己选编码器）。
    ///
    /// 用 mpv 自己截而不是我们把 `render_into` 那帧拉出来存：
    /// - 它截的是**视频原始分辨率**，不是我们缩到窗口大小的那版；
    /// - 界面上的亮度/对比度是着色器里做的，截图不该把调色腌进去；
    /// - 少一次 8MB 级别的内存拷贝。
    ///
    /// `"video"` 表示不带字幕（默认会带）。截图是留画面，不是留字幕。
    pub fn screenshot_to_file(&self, path: &str) -> Result<(), String> {
        if let Some(dir) = std::path::Path::new(path).parent() {
            std::fs::create_dir_all(dir)
                .map_err(|e| format!("建不了截图目录 {}：{e}", dir.display()))?;
        }
        let cpath = CString::new(path).map_err(|_| String::from("路径里含有非法字符（NUL）"))?;
        let mut args: [*const c_char; 4] = [
            c"screenshot-to-file".as_ptr(),
            cpath.as_ptr(),
            c"video".as_ptr(),
            ptr::null(),
        ];
        let rc = unsafe { (self.command)(self.ctx, args.as_mut_ptr()) };
        if rc < 0 {
            return Err(self.describe(rc));
        }
        Ok(())
    }

    /// 读整张轨道表。文件还没加载出来时返回空表。
    ///
    /// mpv 的属性路径支持带下标（`track-list/0/type`），所以不需要解析数组类型，
    /// 一条条读字符串就够——轨道最多几条，开销可以忽略。
    pub fn tracks(&self) -> Vec<Track> {
        let count = self.get_i64("track-list/count").unwrap_or(0).clamp(0, 64);
        let mut out = Vec::new();
        for i in 0..count {
            let Some(kind) = self.get_string(&format!("track-list/{i}/type")) else {
                continue;
            };
            let Some(id) = self.get_i64(&format!("track-list/{i}/id")) else {
                continue;
            };
            out.push(Track {
                id,
                kind,
                title: self.get_string(&format!("track-list/{i}/title")),
                lang: self.get_string(&format!("track-list/{i}/lang")),
                external: matches!(
                    self.get_string(&format!("track-list/{i}/external")).as_deref(),
                    Some("yes")
                ),
            });
        }
        out
    }

    /// 有没有内嵌字幕（不含我们外挂进去的）。
    #[allow(dead_code)]
    pub fn has_embedded_subs(&self) -> bool {
        self.tracks()
            .iter()
            .any(|t| t.kind == "sub" && !t.external)
    }

    /// 把当前文件的元信息一次性读出来（「文件详情」抽屉用）。
    ///
    /// 全部走 mpv 属性，不碰 ffprobe。文件还没起来时各字段为 `None`，
    /// 界面据此显示「—」。
    ///
    /// **优先读 `track-list/{i}/demux-*`**：那是容器头里直接解析出来的值，
    /// 文件一解出头部就有；而 `video-bitrate` / `audio-params/*` 这类是解码器
    /// 跑起来之后才有的实时属性（探测实例连声卡都不开，一定是 None）。
    /// 两条路都试一遍，拿到哪个算哪个。
    pub fn media_info(&self) -> MediaInfo {
        let mut info = MediaInfo {
            path: self.get_string("path").unwrap_or_default(),
            format: self.get_string("file-format"),
            size: self.get_f64("file-size"),
            duration: self.get_f64("duration"),
            ..Default::default()
        };

        // 主视频/音频轨的下标（没有就是 None）
        let count = self.get_i64("track-list/count").unwrap_or(0).clamp(0, 64);
        let mut v_idx: Option<i64> = None;
        let mut a_idx: Option<i64> = None;
        for i in 0..count {
            match self.get_string(&format!("track-list/{i}/type")).as_deref() {
                Some("video") if v_idx.is_none() => v_idx = Some(i),
                Some("audio") if a_idx.is_none() => a_idx = Some(i),
                _ => {}
            }
        }

        if let Some(i) = v_idx {
            // dwidth/dheight 是「转正 + 按像素长宽比校正」之后的显示尺寸；
            // 拿不到再退回容器里的编码尺寸（竖屏手机视频这里会差一次旋转）
            info.width = self
                .get_i64("dwidth")
                .or_else(|| self.get_i64(&format!("track-list/{i}/demux-w")));
            info.height = self
                .get_i64("dheight")
                .or_else(|| self.get_i64(&format!("track-list/{i}/demux-h")));
            // `video-codec` 会给出 "H.264 / AVC / MPEG-4 AVC / ..." 这种一长串别名，
            // 抽屉里放不下也没必要；短名（h264）更清楚。
            info.video_codec = self
                .get_string(&format!("track-list/{i}/codec"))
                .or_else(|| self.get_string("video-codec"));
            info.video_bitrate = self
                .get_f64(&format!("track-list/{i}/demux-bitrate"))
                .or_else(|| self.get_f64("video-bitrate"));
            info.fps = self
                .get_f64("container-fps")
                .or_else(|| self.get_f64("estimated-vf-fps"));
        }

        if let Some(i) = a_idx {
            info.audio_codec = self
                .get_string(&format!("track-list/{i}/codec"))
                .or_else(|| self.get_string("audio-codec"));
            info.audio_bitrate = self
                .get_f64(&format!("track-list/{i}/demux-bitrate"))
                .or_else(|| self.get_f64("audio-bitrate"));
            info.audio_channels = self
                .get_i64(&format!("track-list/{i}/demux-channel-count"))
                .or_else(|| self.get_i64("audio-params/channel-count"));
            info.sample_rate = self
                .get_i64(&format!("track-list/{i}/demux-samplerate"))
                .or_else(|| self.get_i64("audio-params/sample-rate"));
        }

        info
    }

    pub fn seek_absolute(&self, seconds: f64) {
        let Ok(n) = CString::new("time-pos") else { return };
        let mut v = seconds;
        unsafe {
            (self.set_property)(self.ctx, n.as_ptr(), FORMAT_DOUBLE, &mut v as *mut f64 as *mut c_void);
        }
    }

    /// 等 mpv 把「寻址完成，可以继续播」那条事件发出来。
    ///
    /// 抽帧必须靠这条事件判断寻址结束，不能靠 `time-pos`：
    /// `time-pos` 是**命令一发出就变**的，画面却还在从关键帧往后解。
    /// 也不要靠轮询渲染上下文 —— `needs_render` 是一次性标志，寻址期间读掉
    /// 就再也等不到了（暂停状态下 mpv 只在寻址完成时再发一次）。
    ///
    /// 返回 false 表示超时或者文件已经结束（调用方自己决定要不要硬画一帧）。
    pub fn wait_seek_done(&self, timeout: Duration) -> bool {
        #[repr(C)]
        struct Ev {
            event_id: c_int,
            _error: c_int,
            _reply: u64,
            _data: *mut c_void,
        }

        let start = Instant::now();
        while start.elapsed() < timeout {
            let ev = unsafe { (self.wait_event)(self.ctx, 0.02) };
            if ev.is_null() {
                continue;
            }
            match unsafe { (*(ev as *const Ev)).event_id } {
                21 => return true,  // MPV_EVENT_PLAYBACK_RESTART
                7 => return false,  // MPV_EVENT_END_FILE
                0 => std::thread::sleep(Duration::from_millis(4)), // MPV_EVENT_NONE
                _ => {}
            }
        }
        false
    }

    pub fn get_f64(&self, name: &str) -> Option<f64> {
        let n = CString::new(name).ok()?;
        let mut v: f64 = 0.0;
        let rc = unsafe {
            (self.get_property)(self.ctx, n.as_ptr(), FORMAT_DOUBLE, &mut v as *mut f64 as *mut c_void)
        };
        (rc >= 0 && v.is_finite()).then_some(v)
    }

    pub fn get_i64(&self, name: &str) -> Option<i64> {
        let n = CString::new(name).ok()?;
        let mut v: i64 = 0;
        let rc = unsafe {
            (self.get_property)(self.ctx, n.as_ptr(), FORMAT_INT64, &mut v as *mut i64 as *mut c_void)
        };
        (rc >= 0).then_some(v)
    }

    pub fn get_string(&self, name: &str) -> Option<String> {
        let n = CString::new(name).ok()?;
        let mut raw: *mut c_char = ptr::null_mut();
        let rc = unsafe {
            (self.get_property)(self.ctx, n.as_ptr(), FORMAT_STRING, &mut raw as *mut *mut c_char as *mut c_void)
        };
        if rc < 0 || raw.is_null() {
            return None;
        }
        let s = unsafe { CStr::from_ptr(raw).to_string_lossy().into_owned() };
        unsafe { (self.free)(raw as *mut c_void) };
        Some(s)
    }

    /// 有新帧需要绘制吗？（探测实例没有渲染上下文，永远 false）
    pub fn needs_render(&self) -> bool {
        if self.render.is_null() {
            return false;
        }
        unsafe { (self.render_update)(self.render) & RENDER_UPDATE_FRAME != 0 }
    }

    /// 把当前帧渲染到 `buf`（RGBA，`w*h*4` 字节，alpha 由调用方补齐）。
    pub fn render_into(&self, buf: &mut Vec<u8>, w: u32, h: u32) -> bool {
        if self.render.is_null() {
            return false;
        }
        let needed = w as usize * h as usize * 4;
        if needed == 0 {
            return false;
        }
        buf.resize(needed, 0);

        let mut size = [w as i32, h as i32];
        let mut stride = w as usize * 4;
        let mut params = [
            RenderParam {
                type_: PARAM_API_TYPE,
                data: self.api_sw.as_ptr() as *mut c_void,
            },
            RenderParam {
                type_: PARAM_SW_SIZE,
                data: size.as_mut_ptr() as *mut c_void,
            },
            RenderParam {
                type_: PARAM_SW_FORMAT,
                data: self.fmt_rgb0.as_ptr() as *mut c_void,
            },
            RenderParam {
                type_: PARAM_SW_STRIDE,
                data: &mut stride as *mut usize as *mut c_void,
            },
            RenderParam {
                type_: PARAM_SW_POINTER,
                data: buf.as_mut_ptr() as *mut c_void,
            },
            RenderParam {
                type_: 0,
                data: ptr::null_mut(),
            },
        ];
        unsafe { (self.render_frame)(self.render, params.as_mut_ptr()) == 0 }
    }

    /// 清空事件队列，顺带把加载/播放错误捞出来。
    ///
    /// 以前这里只判断「是否播放结束」，`loadfile` 失败（路径写错、格式不支持、
    /// 权限不足）时的错误码被直接丢掉，界面表现就是永远黑屏、无声、无提示。
    pub fn pump_events(&self) -> PumpOutcome {
        let mut out = PumpOutcome::default();

        // mpv_event_end_file（END_FILE 事件的 data 指向它）
        #[repr(C)]
        struct EndFile {
            reason: c_int,
            error: c_int,
            _entry_id: i64,
            _insert_id: i64,
            _insert_num: c_int,
        }

        #[repr(C)]
        struct Ev {
            event_id: c_int,
            _error: c_int,
            _reply: u64,
            data: *mut c_void,
        }

        loop {
            let ev = unsafe { (self.wait_event)(self.ctx, 0.0) };
            if ev.is_null() {
                break;
            }
            let id = unsafe { (*(ev as *const Ev)).event_id };
            if id == 0 {
                break; // MPV_EVENT_NONE
            }
            if id == 7 {
                // MPV_EVENT_END_FILE
                out.ended = true;
                let data = unsafe { (*(ev as *const Ev)).data };
                if !data.is_null() {
                    let ef = unsafe { &*(data as *const EndFile) };
                    // reason == 4 是 MPV_END_FILE_REASON_ERROR
                    if ef.reason == 4 && ef.error != 0 {
                        out.error = Some(self.describe(ef.error));
                    }
                }
            }
        }
        out
    }

    fn describe(&self, code: c_int) -> String {
        let raw = unsafe { (self.error_string)(code) };
        if raw.is_null() {
            return format!("mpv 错误码 {code}");
        }
        let s = unsafe { CStr::from_ptr(raw) }.to_string_lossy().into_owned();
        format!("{s}（{code}）")
    }
}

impl Drop for Mpv {
    fn drop(&mut self) {
        unsafe {
            if !self.render.is_null() {
                (self.render_free)(self.render);
            }
            (self.terminate_destroy)(self.ctx);
        }
    }
}

// `Mpv` 内部是裸指针，默认不是 Send。这层断言是安全的：
// 一个 ctx 只由持有它的那个线程使用（我们把它整体移进渲染线程，之后不再跨线程共享），
// 而 mpv 自身的 client API 就是为「每线程独立 ctx」设计的。
unsafe impl Send for Mpv {}
