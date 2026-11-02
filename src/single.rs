//! 单例：第二次双击 exe（或把视频拖到 exe 图标上）不再起一个新窗口，
//! 而是把要打开的文件**转交给已经在跑的那个实例**。
//!
//! 做法：
//! 1. 启动时先抢一个命名互斥体（`Local\Maneki.SingleInstance`）。
//!    抢到了就是主实例；`ERROR_ALREADY_EXISTS` 说明已经有一个在跑。
//! 2. 主实例开一条命名管道（`\\.\pipe\maneki-ipc`）监听，收到一行行路径就丢进队列，
//!    UI 在 `tick()` 里取走并走正常的 `open_paths()`。
//! 3. 后来者把命令行里的路径写进管道，然后直接退出（不建窗口，秒退）。
//!
//! 为什么用管道而不是 `WM_COPYDATA`：那个得有一个我们自己的窗口过程来收消息，
//! 而窗口过程在 winit 手里，插不进去；管道只要一个后台线程就行。

use std::path::PathBuf;
use std::sync::mpsc::{Receiver, TryRecvError, channel};

// ---------------- Win32 ----------------

type Handle = isize;

const ERROR_ALREADY_EXISTS: i32 = 183;
const PIPE_ACCESS_INBOUND: u32 = 0x0000_0001;
const PIPE_TYPE_BYTE: u32 = 0x0000_0000;
const PIPE_READMODE_BYTE: u32 = 0x0000_0000;
const PIPE_WAIT: u32 = 0x0000_0000;
/// 客户端连接超时（毫秒）。后来者不该在这儿干等。
const CONNECT_WAIT_MS: u32 = 500;
const GENERIC_WRITE: u32 = 0x4000_0000;
const OPEN_EXISTING: u32 = 3;

#[link(name = "kernel32")]
unsafe extern "system" {
    fn CreateMutexW(attrs: *const u16, owner: i32, name: *const u16) -> Handle;
    fn CreateNamedPipeW(
        name: *const u16,
        open_mode: u32,
        pipe_mode: u32,
        max_instances: u32,
        out_buf: u32,
        in_buf: u32,
        timeout: u32,
        security: *const u16,
    ) -> Handle;
    fn ConnectNamedPipe(pipe: Handle, overlapped: *mut u16) -> i32;
    fn DisconnectNamedPipe(pipe: Handle) -> i32;
    fn ReadFile(pipe: Handle, buf: *mut u8, len: u32, read: *mut u32, ov: *mut u16) -> i32;
    fn WriteFile(pipe: Handle, buf: *const u8, len: u32, written: *mut u32, ov: *mut u16) -> i32;
    fn WaitNamedPipeW(name: *const u16, timeout: u32) -> i32;
    fn CreateFileW(
        name: *const u16,
        access: u32,
        share: u32,
        security: *const u16,
        disposition: u32,
        flags: u32,
        template: Handle,
    ) -> Handle;
    fn CloseHandle(h: Handle) -> i32;
}

const INVALID_HANDLE: Handle = -1;

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

const MUTEX_NAME: &str = "Local\\Maneki.SingleInstance";
const PIPE_NAME: &str = r"\\.\pipe\maneki-ipc";

// ---------------- 对外接口 ----------------

pub struct Instance {
    /// 互斥体句柄：**必须一直活着**，进程退出才释放
    mutex: Handle,
    /// 主实例才有：管道线程转过来的路径
    rx: Receiver<Vec<PathBuf>>,
}

impl Instance {
    /// 抢单例。
    ///
    /// - `Ok(instance)`：自己就是主实例，正常起窗口
    /// - `Err(paths)`：已经有一个实例在跑，`paths` 已经交给它了，本进程直接退出
    pub fn acquire(args: Vec<PathBuf>) -> Result<Self, Vec<PathBuf>> {
        let name = wide(MUTEX_NAME);
        let mutex = unsafe { CreateMutexW(std::ptr::null(), 0, name.as_ptr()) };
        let already = std::io::Error::last_os_error().raw_os_error() == Some(ERROR_ALREADY_EXISTS);

        if already {
            // 交给已有实例，交不交得出去都不再起新窗口
            forward(&args);
            if mutex != 0 && mutex != INVALID_HANDLE {
                unsafe { CloseHandle(mutex) };
            }
            return Err(args);
        }

        let (tx, rx) = channel::<Vec<PathBuf>>();
        std::thread::Builder::new()
            .name(String::from("maneki-ipc"))
            .spawn(move || serve(tx))
            .ok();

        Ok(Self { mutex, rx })
    }

    /// 别的实例转过来的路径（没有就是 None）。非阻塞。
    pub fn poll(&self) -> Option<Vec<PathBuf>> {
        match self.rx.try_recv() {
            Ok(paths) if !paths.is_empty() => Some(paths),
            Ok(_) => None,
            Err(TryRecvError::Empty | TryRecvError::Disconnected) => None,
        }
    }
}

impl Drop for Instance {
    fn drop(&mut self) {
        if self.mutex != 0 && self.mutex != INVALID_HANDLE {
            unsafe { CloseHandle(self.mutex) };
        }
    }
}

// ---------------- 主实例：管道服务 ----------------

fn serve(tx: std::sync::mpsc::Sender<Vec<PathBuf>>) {
    let name = wide(PIPE_NAME);

    loop {
        let pipe = unsafe {
            CreateNamedPipeW(
                name.as_ptr(),
                PIPE_ACCESS_INBOUND,
                PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT,
                1,
                0,
                4096,
                0,
                std::ptr::null(),
            )
        };
        if pipe == INVALID_HANDLE || pipe == 0 {
            // 建不出来就别空转
            std::thread::sleep(std::time::Duration::from_secs(2));
            continue;
        }

        // 阻塞等一个客户端连上来
        unsafe { ConnectNamedPipe(pipe, std::ptr::null_mut()) };

        let mut buf = [0u8; 8192];
        let mut read: u32 = 0;
        let ok = unsafe {
            ReadFile(
                pipe,
                buf.as_mut_ptr(),
                buf.len() as u32,
                &mut read,
                std::ptr::null_mut(),
            )
        };
        if ok != 0 && read > 0 {
            let text = String::from_utf8_lossy(&buf[..read as usize]).into_owned();
            // 第一行是协议头，后面一行一个路径
            let paths: Vec<PathBuf> = text
                .lines()
                .skip(1)
                .filter(|l| !l.trim().is_empty())
                .map(|l| PathBuf::from(l.trim_end_matches('\r')))
                .collect();
            if !paths.is_empty() {
                let _ = tx.send(paths);
            }
        }

        unsafe {
            DisconnectNamedPipe(pipe);
            CloseHandle(pipe);
        }
    }
}

// ---------------- 后来者：把路径写进管道 ----------------

fn forward(args: &[PathBuf]) {
    if args.is_empty() {
        // 光双击了一个快捷方式：没什么要转交的，唤起已有窗口就够了
        return;
    }

    let name = wide(PIPE_NAME);
    // 等一下主实例把管道建出来（它可能刚启动）
    unsafe { WaitNamedPipeW(name.as_ptr(), CONNECT_WAIT_MS) };

    let pipe = unsafe {
        CreateFileW(
            name.as_ptr(),
            GENERIC_WRITE,
            0,
            std::ptr::null(),
            OPEN_EXISTING,
            0,
            0,
        )
    };
    if pipe == INVALID_HANDLE || pipe == 0 {
        return;
    }

    let mut payload = String::from("OPEN\n");
    for p in args {
        payload.push_str(&p.to_string_lossy());
        payload.push('\n');
    }

    let mut written: u32 = 0;
    unsafe {
        WriteFile(
            pipe,
            payload.as_ptr(),
            payload.len() as u32,
            &mut written,
            std::ptr::null_mut(),
        );
        CloseHandle(pipe);
    }
}
