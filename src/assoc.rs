//! 把 Maneki 注册成视频文件的「打开方式」。
//!
//! 全部写在 `HKEY_CURRENT_USER` 下，不需要管理员权限。要写三套东西，缺一不可：
//!
//! 1. `Software\Classes\Maneki.Video` —— ProgID：文件类型名、图标、打开命令；
//! 2. 每个后缀的 `OpenWithProgids` —— 让资源管理器右键「打开方式」里出现 Maneki；
//! 3. `Software\RegisteredApplications` + `Software\Maneki\Capabilities` ——
//!    让「设置 → 默认应用」的列表里出现 Maneki，用户可以在那儿一键设为默认。
//!
//! 注意：Windows 8 以后不允许程序静默改写 `UserChoice`（有哈希校验，写进去也会被
//! 系统判定为「无效选择」并重置）。所以「真正变成默认」必须由用户点一下确认，
//! 我们能做的是：把候选注册好、在用户没做过选择的后缀上直接抢占默认、
//! 以及在需要时把用户送到系统那个设置页面（`open_default_apps`）。

use std::ffi::c_void;

#[link(name = "advapi32")]
unsafe extern "system" {
    fn RegCreateKeyExW(
        hkey: isize,
        sub_key: *const u16,
        reserved: u32,
        class: *const u16,
        options: u32,
        sam: u32,
        security: *const c_void,
        result: *mut isize,
        disposition: *mut u32,
    ) -> i32;
    fn RegSetValueExW(
        hkey: isize,
        name: *const u16,
        reserved: u32,
        ty: u32,
        data: *const u8,
        len: u32,
    ) -> i32;
    fn RegQueryValueExW(
        hkey: isize,
        name: *const u16,
        reserved: *mut u32,
        ty: *mut u32,
        data: *mut u8,
        len: *mut u32,
    ) -> i32;
    fn RegOpenKeyExW(
        hkey: isize,
        sub_key: *const u16,
        options: u32,
        sam: u32,
        result: *mut isize,
    ) -> i32;
    fn RegDeleteTreeW(hkey: isize, sub_key: *const u16) -> i32;
    fn RegDeleteValueW(hkey: isize, name: *const u16) -> i32;
    fn RegCloseKey(hkey: isize) -> i32;
}

#[link(name = "shell32")]
unsafe extern "system" {
    fn ShellExecuteW(
        hwnd: isize,
        operation: *const u16,
        file: *const u16,
        parameters: *const u16,
        directory: *const u16,
        show: i32,
    ) -> isize;
    fn SHChangeNotify(event: i32, flags: u32, item1: *const c_void, item2: *const c_void);
}

/// HKEY_CURRENT_USER = ((HKEY)(ULONG_PTR)(LONG)0x80000001)
const HKEY_CURRENT_USER: isize = 0xffff_ffff_8000_0001u64 as isize;

const KEY_READ: u32 = 0x2_0019;
const KEY_WRITE: u32 = 0x2_0006;

const REG_SZ: u32 = 1;
const REG_NONE: u32 = 0;

/// 文件关联用的 ProgID
const PROG_ID: &str = "Maneki.Video";
/// 程序在「默认应用」里的标识（也是 RegisteredApplications 的值名）
const APP_ID: &str = "Maneki";

/// 支持的后缀（跟打开对话框里的过滤器保持一致）
pub const EXTS: &[&str] = &[
    ".mp4", ".mkv", ".avi", ".mov", ".flv", ".wmv", ".webm", ".ts", ".m2ts", ".mts", ".rmvb",
    ".rm", ".m4v", ".mpg", ".mpeg", ".ogv", ".3gp", ".vob", ".asf",
];

// ---------------- 注册表小工具 ----------------

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// 一个打开着的键，析构时自动 RegCloseKey
struct Key(isize);

impl Drop for Key {
    fn drop(&mut self) {
        if self.0 != 0 {
            unsafe {
                RegCloseKey(self.0);
            }
        }
    }
}

impl Key {
    fn create(path: &str) -> Option<Key> {
        let sub = wide(path);
        let mut handle: isize = 0;
        let mut disposition: u32 = 0;
        let ok = unsafe {
            RegCreateKeyExW(
                HKEY_CURRENT_USER,
                sub.as_ptr(),
                0,
                std::ptr::null(),
                0,
                KEY_WRITE,
                std::ptr::null(),
                &mut handle,
                &mut disposition,
            )
        };
        (ok == 0 && handle != 0).then_some(Key(handle))
    }

    fn open(path: &str) -> Option<Key> {
        let sub = wide(path);
        let mut handle: isize = 0;
        let ok = unsafe {
            RegOpenKeyExW(
                HKEY_CURRENT_USER,
                sub.as_ptr(),
                0,
                KEY_READ,
                &mut handle,
            )
        };
        (ok == 0 && handle != 0).then_some(Key(handle))
    }

    /// 写字符串值（`name` 传空串就是默认值）
    fn set_str(&self, name: &str, value: &str) -> bool {
        let key = wide(name);
        // REG_SZ 的字节要带上结尾的 0
        let mut data = Vec::with_capacity((value.len() + 1) * 2);
        for unit in value.encode_utf16().chain(std::iter::once(0)) {
            data.extend_from_slice(&unit.to_le_bytes());
        }
        let ok = unsafe {
            RegSetValueExW(
                self.0,
                key.as_ptr(),
                0,
                REG_SZ,
                data.as_ptr(),
                data.len() as u32,
            )
        };
        ok == 0
    }

    /// 写一个 REG_NONE 空值：只用来占个名字（OpenWithProgids / SupportedTypes）
    fn set_none(&self, name: &str) -> bool {
        let key = wide(name);
        let ok = unsafe {
            RegSetValueExW(self.0, key.as_ptr(), 0, REG_NONE, std::ptr::null(), 0)
        };
        ok == 0
    }

    fn get_str(&self, name: &str) -> Option<String> {
        let key = wide(name);
        let mut len: u32 = 0;
        let mut ty: u32 = 0;
        let rc = unsafe {
            RegQueryValueExW(
                self.0,
                key.as_ptr(),
                std::ptr::null_mut(),
                &mut ty,
                std::ptr::null_mut(),
                &mut len,
            )
        };
        if rc != 0 || len == 0 {
            return None;
        }
        let mut buf = vec![0u8; len as usize];
        let rc = unsafe {
            RegQueryValueExW(
                self.0,
                key.as_ptr(),
                std::ptr::null_mut(),
                &mut ty,
                buf.as_mut_ptr(),
                &mut len,
            )
        };
        if rc != 0 {
            return None;
        }
        let units: Vec<u16> = buf
            .chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .take_while(|u| *u != 0)
            .collect();
        Some(String::from_utf16_lossy(&units))
    }

    fn delete_value(&self, name: &str) {
        let key = wide(name);
        unsafe {
            RegDeleteValueW(self.0, key.as_ptr());
        }
    }
}

fn delete_tree(path: &str) {
    let sub = wide(path);
    unsafe {
        RegDeleteTreeW(HKEY_CURRENT_USER, sub.as_ptr());
    }
}

fn get_str(path: &str, name: &str) -> Option<String> {
    Key::open(path).and_then(|k| k.get_str(name))
}

/// 用 `path\name` = value 写一个字符串值（键不存在就建）
fn put(path: &str, name: &str, value: &str) {
    if let Some(key) = Key::create(path) {
        key.set_str(name, value);
    }
}

fn put_none(path: &str, name: &str) {
    if let Some(key) = Key::create(path) {
        key.set_none(name);
    }
}

// ---------------- 对外接口 ----------------

fn exe_path() -> String {
    std::env::current_exe()
        .ok()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|| String::from("maneki.exe"))
}

/// 当前 exe 的文件名。
///
/// `Software\Classes\Applications\<exe 文件名>` 这个位置必须和真实文件名一模一样，
/// 资源管理器才会把「打开方式 → 更多应用」里那一条认成同一个程序 —— 名字对不上就会
/// 多出一个没有图标的空条目。所以这里不写字面量，直接问系统。
fn exe_name() -> String {
    std::env::current_exe()
        .ok()
        .and_then(|p| {
            p.file_name()
                .map(|n| n.to_string_lossy().into_owned())
        })
        .unwrap_or_else(|| String::from("maneki.exe"))
}

fn open_command() -> String {
    format!("\"{}\" \"%1\"", exe_path())
}

/// 已经注册过、而且指向的还是当前这个 exe
pub fn is_registered() -> bool {
    get_str(
        &format!("Software\\Classes\\{PROG_ID}\\shell\\open\\command"),
        "",
    )
    .map(|v| v == open_command())
    .unwrap_or(false)
}

/// 启动时调一下：没注册或 exe 挪了位置就重新注册
pub fn ensure() {
    if !is_registered() {
        let _ = register();
    }
}

/// 这个后缀是不是已经被用户（或别的程序）明确指定过默认程序
fn has_user_choice(ext: &str) -> bool {
    Key::open(&format!(
        "Software\\Microsoft\\Windows\\CurrentVersion\\Explorer\\FileExts\\{ext}\\UserChoice"
    ))
    .is_some()
}

/// 注册成视频文件的打开方式。可重复调用。
pub fn register() -> Result<(), String> {
    let exe = exe_path();
    let cmd = open_command();

    // 1. ProgID：类型名 + 图标 + 打开命令
    let prog_path = format!("Software\\Classes\\{PROG_ID}");
    put(&prog_path, "", "Maneki 视频文件");
    put(&prog_path, "DefaultIcon", &format!("\"{exe}\",0"));
    put(&format!("{prog_path}\\shell"), "", "open");
    put(&format!("{prog_path}\\shell\\open\\command"), "", &cmd);

    // 2. 每个后缀挂到 ProgID 上，并进「打开方式」候选
    for ext in EXTS {
        put_none(&format!("Software\\Classes\\{ext}\\OpenWithProgids"), PROG_ID);

        // 用户没明确选过默认程序的后缀，直接把这个后缀的默认值指过来。
        // 已经有 UserChoice 的别碰 —— 系统会校验哈希，硬改只会被重置。
        if !has_user_choice(ext) {
            put(&format!("Software\\Classes\\{ext}"), "", PROG_ID);
        }
    }

    // 3. 让「设置 → 默认应用」里出现 Maneki
    let cap_path = format!("Software\\{APP_ID}\\Capabilities");
    put(&cap_path, "ApplicationName", "Maneki");
    put(
        &cap_path,
        "ApplicationDescription",
        "轻量级 Rust 视频播放器，秒开、全格式",
    );
    put(&cap_path, "ApplicationIcon", &format!("\"{exe}\",0"));
    for ext in EXTS {
        put(&format!("{cap_path}\\FileAssociations"), ext, PROG_ID);
    }
    put(
        "Software\\RegisteredApplications",
        APP_ID,
        &format!("Software\\{APP_ID}\\Capabilities"),
    );

    // 4. 顺带支持「拖到 exe 上」「打开方式 → 更多应用」这条路径
    let app_path = format!("Software\\Classes\\Applications\\{}", exe_name());
    put(&format!("{app_path}\\shell\\open\\command"), "", &cmd);
    put(&format!("{app_path}\\shell"), "", "open");
    for ext in EXTS {
        put_none(&format!("{app_path}\\SupportedTypes"), ext);
    }

    notify_shell();
    Ok(())
}

/// 注销：把我们写过的东西清干净（不碰用户自己的关联）
pub fn unregister() -> Result<(), String> {
    delete_tree(&format!("Software\\Classes\\{PROG_ID}"));
    delete_tree(&format!("Software\\{APP_ID}"));
    delete_tree(&format!("Software\\Classes\\Applications\\{}", exe_name()));

    if let Some(key) = Key::create("Software\\RegisteredApplications") {
        key.delete_value(APP_ID);
    }

    for ext in EXTS {
        let open_with = format!("Software\\Classes\\{ext}\\OpenWithProgids");
        if let Some(key) = Key::create(&open_with) {
            key.delete_value(PROG_ID);
        }

        // 只有默认值还指向我们时才清掉
        let ext_path = format!("Software\\Classes\\{ext}");
        if get_str(&ext_path, "").as_deref() == Some(PROG_ID) {
            if let Some(key) = Key::create(&ext_path) {
                key.delete_value("");
            }
        }
    }

    notify_shell();
    Ok(())
}

/// 告诉资源管理器「关联变了」，立刻刷新图标和打开方式菜单
fn notify_shell() {
    const SHCNE_ASSOCCHANGED: i32 = 0x0800_0000;
    const SHCNF_IDLIST: u32 = 0;
    unsafe {
        SHChangeNotify(
            SHCNE_ASSOCCHANGED,
            SHCNF_IDLIST,
            std::ptr::null(),
            std::ptr::null(),
        );
    }
}

/// 打开「设置 → 默认应用」里 Maneki 那一页，让用户自己点「设为默认」。
pub fn open_default_apps() {
    let op = wide("open");
    let uri = wide(&format!("ms-settings:defaultapps?registeredAppUser={APP_ID}"));
    const SW_SHOWNORMAL: i32 = 1;
    unsafe {
        ShellExecuteW(
            0,
            op.as_ptr(),
            uri.as_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            SW_SHOWNORMAL,
        );
    }
}
