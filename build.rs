//! 把图标和版本信息编译进 exe（只有主程序 maneki 需要，probe 不加）。
//!
//! 版本号**不写在 .rc 里**：`assets/app.rc` 是个带占位符的模板，这里用
//! `CARGO_PKG_VERSION` 替换后再交给 rc.exe 编译。好处是打包脚本只改
//! Cargo.toml 一处，exe 的文件属性、安装包文件名、安装后显示的版本号
//! 就全都跟着走，不会出现「安装包是 0.2.0、exe 属性里写着 0.1.0」这种事。
//!
//! rc.exe 由 embed-resource 自己去找（本机 Windows SDK 装在 D:\Windows Kits）。

use std::path::PathBuf;

fn main() {
    println!("cargo:rerun-if-changed=assets/app.rc");
    println!("cargo:rerun-if-changed=assets/logo.ico");
    println!("cargo:rerun-if-env-changed=CARGO_PKG_VERSION");

    let version = std::env::var("CARGO_PKG_VERSION").unwrap_or_else(|_| String::from("0.0.0"));
    let mut parts = version.split('.').map(|p| p.parse::<u16>().unwrap_or(0));
    let major = parts.next().unwrap_or(0);
    let minor = parts.next().unwrap_or(0);
    let patch = parts.next().unwrap_or(0);

    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let icon = manifest.join("assets").join("logo.ico");
    let template = std::fs::read_to_string(manifest.join("assets").join("app.rc"))
        .expect("读不到 assets/app.rc");

    // RC 把反斜杠当转义符：路径里的每个 \ 都要写成 \\
    let icon = icon.display().to_string().replace('\\', "\\\\");
    let rc = template
        .replace("@ICON@", &icon)
        .replace("@VERSION4@", &format!("{major},{minor},{patch},0"))
        .replace("@VERSION@", &version);

    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("app.rc");
    std::fs::write(&out, rc).expect("写不了生成的 .rc");

    embed_resource::compile_for(&out, ["maneki"], embed_resource::NONE)
        .manifest_optional()
        .unwrap();
}
