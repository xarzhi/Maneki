# Maneki

**用 Rust + iced 写的 Windows 视频播放器**，解码交给 libmpv。

就冲两件事去的：**几十 GB 的片子也秒开**，以及拖窗口、拉缩放的时候画面不停。

名字取自招财猫（maneki-neko）——举爪那一下，正好是「点一下播放」。
和 [Meow Player](https://github.com/xarzhi/meow-player)（本地音乐播放器）是一对：一个是猫叫，一个是猫的样子。

![界面](assets/screenshot.png)

*截图里放的是 FFmpeg 的彩条测试片（`assets/sample.mp4`），右侧是播放列表抽屉。*

## 特性

**播放**

- 打开时窗口按视频比例摆好，画面铺满、不留黑边；换集只换片子，不动窗口尺寸
- 顶栏和底栏在鼠标静置 2 秒后收起，鼠标一动就滑回来
- 换片空档在画面中央转一圈，首帧到手立刻收（秒开的小文件不会闪）
- 播放速度 0.5×–8.0×，预设 + 自定义滑杆，连播时保持不变
- 音量、字幕、速度三块浮层都从底栏上方弹出

**画面**

- 填充方式：默认（保持比例）/ 铺满 / 拉伸
- 画面比例：自动 / 16:9 / 4:3 / 5:4 / 21:9
- 色彩调整：亮度、对比度、饱和度、色调、锐度

**文件**

- 打开方式：拖文件、拖文件夹、命令行参数、文件关联，四种都行
- 播放列表带时长（后台线程探测，不挡打开）
- 单例：再次双击 exe 不会开第二个窗口，路径转交给已经在跑的那个实例
- 右键菜单：在资源管理器中打开 / 查看视频文件详情（容器、编码、码率、轨道…）
- 全屏时窗口是钉死的，拖不动也拉不动

## 为什么能秒开

不是靠「预读更多」，恰恰相反——`src/mpv.rs` 里把这几个值全调低了：

| 选项 | 这里的值 | libmpv 默认 |
| --- | --- | --- |
| `cache` | `no` | `auto` |
| `demuxer-lavf-probesize` | `32768`（32 KiB） | 5 MiB |
| `demuxer-lavf-analyzeduration` | `1`（秒） | 5 秒 |
| `demuxer-readahead-secs` | `0.5` | 1.0 |
| `demuxer-max-bytes` | `64MiB` | 150 MiB |

内存占用和文件大小无关：不 cache、缓冲上限固定，开 50 GB 的片子和开 50 MB 的占一样多。

另外 `config=no`、`load-scripts=no`、`osc=no`、`idle=yes` —— 不读用户配置、不跑 Lua 脚本、不带内置 OSD，启动时少干一大堆活。

## 为什么拖窗口不卡

播放线程独占 `Mpv`，解码和取帧都跑在那条线程里，UI 线程每帧只收最新一帧（一个 `Arc<Vec<u8>>`）。

窗口拖动和边缘缩放都是自己算好位置调 `SetWindowPos`，**不走** win32 的 `WM_NCLBUTTONDOWN(HTCAPTION)`——那个会进系统的模态移动循环，把 UI 线程整个占住，表现就是「声音还在放、画面冻住，松手才恢复」。

## 快捷键

| 键 | 作用 |
| --- | --- |
| `Space` | 播放 / 暂停 |
| `Esc` | 逐层收起：右键菜单 → 浮层 → 抽屉 → 退出全屏 |

## 支持的格式

`mp4` `mkv` `avi` `mov` `flv` `wmv` `webm` `ts` `m2ts` `mts` `rmvb` `rm` `m4v` `mpg` `mpeg` `ogv` `3gp` `vob` `asf`

实际能不能放，最终取决于你的 libmpv 里那份 FFmpeg 构建。

## 环境

- Windows 10 / 11（x64）
- Rust 1.85+（用的是 edition 2024）
- `libmpv-2.dll` —— **不在仓库里**，117 MB，超过 GitHub 单文件 100 MB 的限制

## 构建

```powershell
git clone https://github.com/xarzhi/Maneki.git
cd Maneki

# 把 libmpv-2.dll 放到 vendor\ 下，文件名必须就是 libmpv-2.dll
# 程序会从 exe 所在目录逐级往上找 vendor\libmpv-2.dll

cargo build --release
.\target\release\maneki.exe "D:\某个视频.mp4"
```

`cargo run --release -- "D:\某个视频.mp4"` 也能跑，DLL 会从仓库根的 `vendor\` 里找到。

## 打包安装包

```powershell
powershell -ExecutionPolicy Bypass -File .\package.ps1 0.1.0
```

产物在 `dist\Maneki_0.1.0_x64-setup.exe`。几个细节：

- 版本号唯一来源是 `Cargo.toml` 的 `version`：脚本改完会让 `build.rs` 重新渲染 `assets/app.rc`，再从产物的版本资源里读回来校验，对不上就报错；
- 安装向导里选完目录，程序会再套一层 `Maneki` 子目录（`...\Tools` → `...\Tools\Maneki`），不会把 exe 和 uninstall.exe 直接撒在父目录里；
- `vendor\libmpv-2.dll` 会一起打进安装包，装完直接能播。

## 代码结构

| 文件 | 职责 |
| --- | --- |
| `main.rs` | 应用状态、消息、视图与布局 |
| `mpv.rs` | libmpv 的最小 FFI 封装，以及上面那批启动选项 |
| `player.rs` | 播放线程：解码、取帧、回报状态快照 |
| `video.rs` | 上屏：把一帧 RGBA 当纹理采样画出来，附带填充 / 比例 / 调色 |
| `playlist.rs` | 目录扫描、自然排序、播放列表抽屉 |
| `settings.rs` | 设置抽屉 |
| `info.rs` | 文件详情抽屉 |
| `subtitle.rs` | 字幕浮层 |
| `spinner.rs` | 换片转圈 |
| `assoc.rs` | 文件关联（全部写在 HKCU，不需要管理员权限） |
| `single.rs` | 单例互斥体 + 命名管道 |
| `win32.rs` | Windows 原生窗口微调：无边框、圆角、拖动、缩放、置顶 |
| `ui.rs` / `icon.rs` | 跨面板的小工具 / 图标字体接入 |

## 已知限制

- 只有 Windows：`win32.rs` 里全是原生调用，没有做跨平台抽象
- 字幕只走 libmpv 自己的渲染，样式不可调
- **不能切音轨**：多音轨的片子目前只能用 libmpv 挑的第一条（字幕可以选，音轨还没做）

## 许可证

[MIT](LICENSE) © 2026 xarzhi
