//! Windows 原生窗口微调。
//!
//! iced / winit 用 `decorations(false)` 建出来的窗口是纯 `WS_POPUP`：
//!
//! 1. DWM 只给「带可缩放边框（WS_THICKFRAME）的窗口」做圆角，
//!    纯 POPUP 窗口会被直接排除，所以 `DWMWA_WINDOW_CORNER_PREFERENCE`
//!    设了也不生效 —— 必须先把 WS_THICKFRAME 补回去。
//! 2. 补上 WS_THICKFRAME 之后，DWM 会顺手画一圈浅色边框，
//!    用 `DWMWA_BORDER_COLOR = DWMWA_COLOR_NONE` 把它抹掉，
//!    只留圆角。

use std::ffi::c_void;

#[link(name = "dwmapi")]
unsafe extern "system" {
    fn DwmSetWindowAttribute(hwnd: isize, attr: u32, value: *const c_void, size: u32) -> i32;
}

#[link(name = "user32")]
unsafe extern "system" {
    fn GetWindowLongW(hwnd: isize, index: i32) -> i32;
    fn SetWindowLongW(hwnd: isize, index: i32, value: i32) -> i32;
    fn SetWindowPos(
        hwnd: isize,
        insert_after: isize,
        x: i32,
        y: i32,
        cx: i32,
        cy: i32,
        flags: u32,
    ) -> i32;
    fn GetWindowRect(hwnd: isize, rect: *mut Rect) -> i32;
    fn GetCursorPos(point: *mut Point) -> i32;
    fn GetAsyncKeyState(vkey: i32) -> i16;
    fn GetDpiForWindow(hwnd: isize) -> u32;
    fn IsZoomed(hwnd: isize) -> i32;
    fn ShowWindow(hwnd: isize, cmd: i32) -> i32;
    fn MonitorFromWindow(hwnd: isize, flags: u32) -> isize;
    fn GetMonitorInfoW(monitor: isize, info: *mut c_void) -> i32;
    fn GetLocalTime(time: *mut SystemTime) -> ();
}

/// `SYSTEMTIME`。字段顺序不能改，它是 ABI。
#[repr(C)]
struct SystemTime {
    year: u16,
    month: u16,
    day_of_week: u16,
    day: u16,
    hour: u16,
    minute: u16,
    second: u16,
    millis: u16,
}

#[repr(C)]
struct Rect {
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
}

#[repr(C)]
struct Point {
    x: i32,
    y: i32,
}

const GWL_STYLE: i32 = -16;
const GWL_EXSTYLE: i32 = -20;

/// WS_EX_TOPMOST
const WS_EX_TOPMOST_BIT: i32 = 0x0000_0008;

/// `SetWindowPos` 的 hWndInsertAfter 特殊值
const HWND_TOPMOST: isize = -1;
const HWND_NOTOPMOST: isize = -2;

const WS_CAPTION_BITS: u32 = 0x00c0_0000; // WS_BORDER | WS_DLGFRAME
const WS_THICKFRAME_BITS: u32 = 0x0004_0000;

const SWP_NOSIZE: u32 = 0x0001;
const SWP_NOMOVE: u32 = 0x0002;
const SWP_NOZORDER: u32 = 0x0004;
const SWP_NOACTIVATE: u32 = 0x0010;
const SWP_FRAMECHANGED: u32 = 0x0020;

/// DWMWINDOWATTRIBUTE
const DWMWA_WINDOW_CORNER_PREFERENCE: u32 = 33;
const DWMWA_BORDER_COLOR: u32 = 34;
/// DWM_WINDOW_CORNER_PREFERENCE
const DWMWCP_ROUND: i32 = 2;
/// DWMWA_COLOR_NONE
const DWMWA_COLOR_NONE: u32 = 0xffff_fffe;

/// 给窗口补上可缩放边框 + DWM 圆角。
pub fn polish(hwnd: isize) {
    if hwnd == 0 {
        return;
    }

    unsafe {
        let before = GetWindowLongW(hwnd, GWL_STYLE) as u32;

        // 保留 WS_POPUP（不要标题栏），只把 WS_THICKFRAME 加回来。
        let after = (before | WS_THICKFRAME_BITS) & !WS_CAPTION_BITS;

        if after != before {
            SetWindowLongW(hwnd, GWL_STYLE, after as i32);
            SetWindowPos(
                hwnd,
                0,
                0,
                0,
                0,
                0,
                SWP_FRAMECHANGED | SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
            );
        }

        let pref = DWMWCP_ROUND;
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_WINDOW_CORNER_PREFERENCE,
            &pref as *const i32 as *const c_void,
            size_of::<i32>() as u32,
        );

        let none = DWMWA_COLOR_NONE;
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_BORDER_COLOR,
            &none as *const u32 as *const c_void,
            size_of::<u32>() as u32,
        );
    }
}

// ---------------- 自己驱动的窗口缩放 ----------------
//
// 为什么要自己算：`iced::window::drag_resize` 走的是 Windows 的**模态缩放循环**
// （`SendMessage(WM_NCLBUTTONDOWN, HTxxx)`），那个循环期间 UI 线程被完全占住，
// 于是音频照放、画面冻住，松开鼠标才恢复重绘。
// 改成每个 tick 用 `SetWindowPos` 把窗口摆到位，事件循环一直转，画面就不会停。

/// 窗口在屏幕上的位置和大小（物理像素）
pub fn window_rect(hwnd: isize) -> Option<(i32, i32, i32, i32)> {
    let mut r = Rect {
        left: 0,
        top: 0,
        right: 0,
        bottom: 0,
    };
    let ok = unsafe { GetWindowRect(hwnd, &mut r) };
    if ok == 0 {
        return None;
    }
    Some((r.left, r.top, r.right - r.left, r.bottom - r.top))
}

/// 鼠标在屏幕上的位置（物理像素）
pub fn cursor_pos() -> (i32, i32) {
    let mut p = Point { x: 0, y: 0 };
    unsafe {
        GetCursorPos(&mut p);
    }
    (p.x, p.y)
}

/// 左键是不是还按着（拖缩放时用来判断松手，比等事件可靠）
pub fn left_down() -> bool {
    const VK_LBUTTON: i32 = 0x01;
    unsafe { GetAsyncKeyState(VK_LBUTTON) < 0 }
}

/// 窗口所在显示器的 DPI 缩放比（96 = 1.0）
pub fn dpi_scale(hwnd: isize) -> f32 {
    let dpi = unsafe { GetDpiForWindow(hwnd) };
    if dpi == 0 { 1.0 } else { dpi as f32 / 96.0 }
}

/// 把窗口摆到指定位置和大小（物理像素）
pub fn set_window_rect(hwnd: isize, x: i32, y: i32, w: i32, h: i32) {
    unsafe {
        SetWindowPos(
            hwnd,
            0,
            x,
            y,
            w,
            h,
            SWP_NOZORDER | SWP_NOACTIVATE,
        );
    }
}

/// 是否处于最大化状态
pub fn is_maximized(hwnd: isize) -> bool {
    unsafe { IsZoomed(hwnd) != 0 }
}

/// 窗口所在显示器的工作区（去掉任务栏），物理像素：(x, y, w, h)
pub fn work_area(hwnd: isize) -> Option<(i32, i32, i32, i32)> {
    #[repr(C)]
    struct MonitorInfo {
        cb_size: u32,
        rc_monitor: Rect,
        rc_work: Rect,
        flags: u32,
    }

    const MONITOR_DEFAULTTONEAREST: u32 = 2;

    let mut info = MonitorInfo {
        cb_size: size_of::<MonitorInfo>() as u32,
        rc_monitor: Rect {
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
        },
        rc_work: Rect {
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
        },
        flags: 0,
    };

    let monitor = unsafe { MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST) };
    if monitor == 0 {
        return None;
    }
    let ok = unsafe { GetMonitorInfoW(monitor, &mut info as *mut MonitorInfo as *mut c_void) };
    if ok == 0 {
        return None;
    }

    let r = info.rc_work;
    Some((r.left, r.top, r.right - r.left, r.bottom - r.top))
}

/// 最大化 → 还原（拖边缘缩放前先还原，和系统行为一致）
pub fn restore(hwnd: isize) {
    const SW_RESTORE: i32 = 9;
    unsafe {
        ShowWindow(hwnd, SW_RESTORE);
    }
}

// ---------------- 资源管理器 ----------------

/// 打开资源管理器并**选中**这个文件（右键菜单「在资源管理器中打开」）。
///
/// 用 `raw_arg` 自己写引号：`explorer.exe` 的 `/select,` 参数对引号很挑剔，
/// 路径里有空格时如果让标准库自动拼接整段加引号，部分 Windows 版本会把它
/// 当成一个文件名去找。手写成 `/select,"C:\a b\x.mp4"` 一定对。
///
/// 只**新开**一个资源管理器窗口，绝不去碰已经在跑的 explorer 进程。
pub fn reveal_in_explorer(path: &std::path::Path) -> Result<(), String> {
    use std::os::windows::process::CommandExt;
    if !path.exists() {
        return Err(format!("文件已经不在了：{}", path.display()));
    }
    std::process::Command::new("explorer.exe")
        .raw_arg(format!("/select,\"{}\"", path.display()))
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("无法打开资源管理器：{e}"))
}

// ---------------- 窗口置顶 ----------------
//
// 走 `SetWindowPos(HWND_TOPMOST / HWND_NOTOPMOST)` 而不是改 WS_EX_TOPMOST 扩展样式位：
// 前者会把窗口真正移到 topmost 段（Z 序）里，后者只改标志位、窗口位置不动，
// 表现上就是「标志位改了但没浮上去」。

/// 窗口当前是不是置顶状态
pub fn is_topmost(hwnd: isize) -> bool {
    if hwnd == 0 {
        return false;
    }
    unsafe { (GetWindowLongW(hwnd, GWL_EXSTYLE) & WS_EX_TOPMOST_BIT) != 0 }
}

/// 置顶 / 取消置顶（不动位置、不动大小、不抢焦点）
pub fn set_topmost(hwnd: isize, on: bool) {
    if hwnd == 0 {
        return;
    }
    unsafe {
        SetWindowPos(
            hwnd,
            if on { HWND_TOPMOST } else { HWND_NOTOPMOST },
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
        );
    }
}

/// 本地时间的紧凑写法 `20261103-190500`，截图文件名用。
///
/// 不用 `SystemTime` 转：那个得到的是 UTC，中国用户看到的文件名会差 8 小时。
/// `GetLocalTime` 直接给本地时间，也不用为了时区去拉一个日期库。
pub fn local_stamp() -> String {
    let mut t = SystemTime {
        year: 0,
        month: 0,
        day_of_week: 0,
        day: 0,
        hour: 0,
        minute: 0,
        second: 0,
        millis: 0,
    };
    unsafe { GetLocalTime(&mut t) };
    format!(
        "{:04}{:02}{:02}-{:02}{:02}{:02}",
        t.year, t.month, t.day, t.hour, t.minute, t.second
    )
}
