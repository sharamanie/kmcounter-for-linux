# Windows 侧排查结论（托盘点了呼不出窗口）

写给 Linux 侧（合并源码用）和用户。全部结论都在 **Windows 真机**（Windows 11 26200，NVIDIA，2560×1600 @150%）上复现/验证过，
测试对象是 `D:\kmcounter\kmcounter.exe`（用户实际运行的位置）与最新构建。

---

## 0. TL;DR

1. **用户机器上跑的是旧构建**（`kmcounter.exe` 时间戳 18:01，早于本轮改动），
   它没有 Win32 直接显示/隐藏那套代码 —— 收起后再点托盘必然无效（已实测复现）。
   换成 20:09 的构建后，托盘呼出恢复正常。
2. **20:09 的源码里还有一个真 bug**：`gui.rs` 的 `win32_window::main_hwnd()` 用
   “EnumWindows 遇到的第一个本进程顶层窗口”当主窗口，会选中 **OpenGL 驱动建的假窗口**
   （NVIDIA：`NVOpenGLPbuffer` / `__wglDummyWindowFodder`）或 winit 的消息窗口。
   一旦选中，之后每次“显示窗口”都作用在这些看不见的窗口上：**日志照样写“已通过 Win32 直接显示窗口”，
   真正的主窗口却永远回不来**——托盘彻底失效（本次已在真机复现，见 §2）。
3. 已修复（`src/gui.rs` 的 `win32_window` + `src/tray.rs` 的 NIN_SELECT），见 §3。
   **本机已编译 + 真机验证（4 轮真实点击「收起→呼出」全通过）+ 已部署**：见 §4。

---

## 1. 复现方法与实测结果

用脚本模拟/真实鼠标点击（脚本都留在 `D:\kmcounter\`）：

| 脚本 | 用途 |
| --- | --- |
| `winscan.ps1 -TargetPid <pid>` | 列出该进程所有顶层窗口：类名/标题/可见/最小化/矩形 |
| `probe.ps1 -TargetPid <pid> -Action scan\|click\|close` | 发 `WM_APP_TRAY`(0x0202) 模拟左键、发 `WM_CLOSE` 模拟点 × |
| `realmouse.ps1 -X <px> -Y <py> [-NoClick]` | 真实鼠标移动/点击（DPI 感知，物理像素） |
| `trayrect.ps1 -TargetPid <pid>` | `Shell_NotifyIconGetRect` 问 shell 要图标位置（注意：图标在溢出里时给的是“显示隐藏的图标”按钮的位置） |
| `cycle.ps1 -TargetPid <pid> -Rounds N` | 自动跑 N 轮「点 × → 打开溢出 → 点图标」，每步打印主窗口可见性 |

### 1.1 旧构建（18:01）——用户当时运行的那个

```
[12:46:59] INFO kmcounter::gui            窗口已隐藏到托盘          ← 老构建的收起路径（没有 Win32 行）
[12:47:00] INFO kmcounter::controller     托盘：显示窗口            ← 托盘命令收到了
[12:47:01] INFO kmcounter::controller     托盘：显示窗口
[12:47:03] INFO kmcounter::controller     托盘：显示窗口
主窗口状态：[after-summon] main visible=False（三次都是 False）
```

结论：命令到达控制线程，但 `Visible(true)` 这类 egui 命令永远不会被处理——
窗口被 `Visible(false)` 隐藏后渲染循环停摆（这正是交接文档里推测的机制）。
用户看到的“第一次能呼出”是因为第一次点托盘时窗口还没真正收起/渲染循环还在跑；
一旦收起，后面就再也叫不回来。

### 1.2 最新构建（20:09）——有 Win32 直接显示，但会选错窗口

同一台机器、同一套流程，进程 PID 5112：

```
[12:48:08] INFO kmcounter::gui::win32_window  已通过 Win32 直接隐藏窗口
[12:48:09] INFO kmcounter::controller         托盘：显示窗口
[12:48:09] INFO kmcounter::gui::win32_window  已通过 Win32 直接显示窗口   ← 日志说显示了
...（又两次，同样“成功”）
主窗口状态：main visible=False  ← 实际一次都没显示出来
前台窗口    ：0x120B90 = NVOpenGLPbuffer / __wglDummyWindowFodder（本进程自己的窗口！）
```

进程内窗口清单（`winscan.ps1`）说明候选有多脏：

```
0x120B90 NVOpenGLPbuffer   __wglDummyWindowFodder   Visible=False   ← 被当成主窗口
0x40B6C  NVOpenGLPbuffer   NVOGLDC invisible        Visible=False
0x50C0A  Window Class      KMCounter-rs v0.1.0 | …  Visible=False   ← 真正的主窗口
0x80B68  Winit Thread Event Target                   Visible=True
0x60B5A  KMCounterTrayWnd  KMCounter                 Visible=False
```

`SetForegroundWindow` 全进程只有 `win32_window::show()` 在调，而它把 `NVOpenGLPbuffer` 顶成了前台窗口
——这就是“选错窗口”的铁证：`main_hwnd()` 缓存了错误的句柄，且**只要句柄没被销毁就一直用下去**。
（另一次启动 PID 27772 时 Z 序恰好让主窗口排第一，于是连续 5 轮真实点击都正常 ——
这就是“有时好使、有时彻底不灵”的来源。）

### 1.3 真实鼠标点击会发哪些回调（v3，Windows 11 26200）

在托盘溢出面板里真实点击图标，`tray.rs` 记到：

```
lparam=0x0201 (WM_LBUTTONDOWN) wparam=1
lparam=0x0202 (WM_LBUTTONUP)   wparam=1   → 旧代码就认这个，能呼出
lparam=0x0400 (NIN_SELECT)     wparam=1   → 旧代码忽略（无害，但换成键盘选中时可能就只有它）
```

---

## 2. 根因

`src/gui.rs` → `mod win32_window` → `main_hwnd()`（旧实现）：

```rust
unsafe extern "system" fn enum_proc(hwnd: HWND, lparam: LPARAM) -> BOOL {
    ...GetWindowThreadProcessId(hwnd, &mut owner);
    if owner != pid { return TRUE; }
    ...GetClassNameW(...); if class.contains("KMCounterTrayWnd") { return TRUE; }
    MAIN_HWND.store(hwnd as usize, Ordering::Relaxed);   // ← 只要不是托盘窗口就认
    FALSE                                                  // ← 第一个就收工
}
```

- 判据只有“是本进程的窗口且不是托盘窗口”，**返回顺序完全由 Z 序决定**；
- 进程里还有 OpenGL 驱动的像素格式假窗口（`NVOpenGLPbuffer`，选完像素格式后一直留在窗口列表里）、
  winit 的线程消息窗口 `Winit Thread Event Target`、搜狗/TSF 注入的输入法窗口；
- 隐藏窗口会被系统排到 Z 序底部，所以“窗口已经收起”的状态下重新枚举几乎必然先撞上这些辅助窗口；
- 缓存只在 `IsWindow()` 失败时重找，所以**错一次=永久错**，而日志仍然报“已显示窗口”，
  从外面看就是“托盘点了没反应”。

## 3. 改动

### 3.1 `src/gui.rs`（`mod win32_window`，整体重写）

- 认窗口改用确定特征，二选一（`is_main_window`）：
  1. 窗口类名是 winit 主窗口类 `Window Class`；
  2. 标题以程序标题 `KMCounter-rs` 开头（egui 每帧用 `ViewportCommand::Title` 设置）。
- 枚举时按优先级收集候选（`by_class` > `by_title` > `fallback`），并跳过辅助窗口
  （`KMCounterTrayWnd` / `*Pbuffer*` / `Winit Thread Event Target` / `*IME*` / `Sogou*` / `SoPY*`）。
- **缓存要复核**：只有 `IsWindow()==0`（原来就有的判断）之外再加 `is_main_window()` 通过才复用；
  退化的 fallback 猜测不写缓存，每次都重新找。
- **用完自检**：`show()` 之后若 `IsWindowVisible()==0`（说明又找错了），
  或者 `hide()` 之后窗口居然还可见，就丢弃缓存并在日志里警告，下次重新定位——
  即使将来又混进别的辅助窗口，也不会再“永久失效”。
- 日志带上句柄：`已通过 Win32 直接显示窗口（0x50C0A）`，以后一眼能看出打的是哪个窗口。

### 3.2 `src/tray.rs`

左键分支同时认 `NIN_SELECT(0x0400)` / `NIN_KEYSELECT(0x0401)`：
鼠标点击实测会先发 0x0201/0x0202 再补一个 0x0400，重复发 `Show` 是无害的（幂等）；
键盘激活或某些系统/位置只发 NIN_*，只认 0x0202 就会漏。

> 仍未处理 `WM_CONTEXTMENU` 之外的 `NIN_POPUPOPEN/CLOSE`（悬停通知）——不需要，忽略即可。

## 4. 验证（本机实测）

修复后的 exe（本机用 README 的 GNU + llvm-mingw 路线编译，SHA256 前 16 位 `F07EC5928E314F51`）
已部署到本机的运行目录（`D:\kmcounter\kmcounter.exe`）。旧 exe 备份为 `kmcounter.exe.old-HHMM.bak`（18:01 / 20:09 各一份）。

### 4.1 真实鼠标点击（4 轮「点 × → 托盘呼出」）

`cycle.ps1 -TargetPid 29092 -Rounds 4`（打开溢出面板 → 真点图标 → 发 WM_CLOSE 模拟 ×）：

```
round 1: after-close main visible=False → after-summon main visible=True  前台=主窗口
round 2: after-close main visible=False → after-summon main visible=True  前台=主窗口
round 3: after-close main visible=False → after-summon main visible=True  前台=主窗口
round 4: after-close main visible=False → after-summon main visible=True  前台=主窗口
```

日志（`D:\kmcounter\run-fixed.err.log`）里每次都是同一个句柄，且与 `simfind.ps1` 按新判据选出的窗口一致：

```
[15:23:08] 已通过 Win32 直接隐藏窗口（0x90C06）
[15:23:10] 托盘回调 lparam=0x0202 … → 托盘：显示窗口 → 已通过 Win32 直接显示窗口（0x90C06）
[15:23:10] 托盘回调 lparam=0x0400 … → 托盘：显示窗口 → 已通过 Win32 直接显示窗口（0x90C06）
…（round 2/3/4 同样）
simfind.ps1: fixed criteria pick: 0x90C06 (by_class)      ← 与日志里的句柄同一个
```

无 `仍不可见/仍可见`、无 `没找到 winit 主窗口` 告警。

### 4.2 二次启动（`show.request`）路径

窗口收起后运行第二个实例 → 主实例日志 `收到显示窗口请求（另一实例启动）` →
`已通过 Win32 直接显示窗口（0x90C06）`，`winscan` 确认主窗口 `Visible=True` ✓

### 4.3 构建产物自检

- `llvm-objdump -p` 的导入表与原版 exe 完全一致（只有 kernel32/user32/gdi32/opengl32/… 与 api-ms-win-crt-*），
  **没有 libunwind.dll 之类的额外运行库**（用 gnullvm 目标直接编会带 `libunwind.dll` 导入，不能这么交付，已改走 GNU 目标静态链接）。
- 构建 0 警告；`cargo test`：见 4.4。

### 4.4 cargo test

- 原有 1 项失败是 **Windows 平台的测试写法问题**（`config::tests::datadir_marker_parsing` 里写了 Unix 绝对路径
  `/mnt/item/KMCounter/`，在 Windows 上 `is_absolute()` 为假，会走「相对程序目录」分支）。
  产品的 Windows 行为是对的（用户真实 `datadir.txt` 写 `E:\项目\run\KMCounter` 工作正常）。
  已把该用例改成按平台给出绝对路径（`#[cfg(unix)]/[cfg(windows)]` 各一份断言）。
- 修复后：**44 passed; 0 failed**（`x86_64-pc-windows-gnu` 目标，Windows 主机上运行）。

## 5. 本次改动清单（diff 摘要）

| 文件 | 改动 |
| --- | --- |
| `src/gui.rs` | `mod win32_window` 重写：按 `Window Class` / 标题前缀认主窗口、跳过辅助窗口、缓存复核、显示/隐藏后自检并丢弃错误句柄、日志带句柄 |
| `src/tray.rs` | 左键分支增加 `NIN_SELECT(0x0400)` / `NIN_KEYSELECT(0x0401)` |
| `src/config.rs` | 仅测试：`datadir_marker_parsing` 的绝对路径用例按平台分支（Windows 用 `E:\...`） |

## 6. 在 Windows 上构建（本机已装好工具链）

本机原来没有任何 Rust 工具链（exe 一直是 Linux 侧交叉编译产物）。为真机验证，我装了：

```
%USERPROFILE%\.cargo\bin             rustup（minimal，未改 PATH）
%USERPROFILE%\.rustup                stable-x86_64-pc-windows-msvc（默认）+
                                      stable-x86_64-pc-windows-gnullvm（主机工具链，自带 rust-lld）
D:\rustsetup\llvm-mingw-extract\...   llvm-mingw 20260922（ucrt，Windows 版）
D:\rustsetup\mingw-compat             libgcc.a ← libclang_rt.builtins-x86_64.a
                                      libgcc_eh.a ← x86_64-w64-mingw32/lib/libunwind.a
D:\rustsetup\kmcounter-target         构建输出（CARGO_TARGET_DIR）
```

构建命令（README 的 Linux 配方在 Windows 上的等价写法）：

```cmd
set PATH=%USERPROFILE%\.cargo\bin;D:\rustsetup\llvm-mingw-extract\llvm-mingw-20260922-ucrt-x86_64\bin;%PATH%
set RUSTFLAGS=-L D:\rustsetup\mingw-compat
set CARGO_TARGET_DIR=D:\rustsetup\kmcounter-target
cd /d D:\kmcounter\kmcounter-src
cargo +stable-x86_64-pc-windows-gnullvm build --release --target x86_64-pc-windows-gnu
cargo +stable-x86_64-pc-windows-gnullvm test  --target x86_64-pc-windows-gnu
```

**两个坑**：

1. 本机 cargo 默认会走一个 `127.0.0.1` 代理（环境里带进来的），不覆盖就报
   `Failed to connect to index.crates.io:443 over proxy 127.0.0.1`；直接用空代理覆盖即可：
   `cargo --config "http.proxy=''" fetch`（我这次因为开了代理客户端，通过
   `set HTTPS_PROXY=http://127.0.0.1:65532` 走真代理反而快 17 倍，两种都可用）。
2. 主机工具链必须是 **gnullvm/gnu**（自带/可用 llvm-mingw 的链接器）；用默认的 MSVC 主机工具链
   编 GNU 目标时，build script 需要 `link.exe`，本机没有 Visual Studio，会直接失败。

清理：不再需要时删掉 `%USERPROFILE%\.cargo`、`%USERPROFILE%\.rustup`、`D:\rustsetup` 即可（连带我装在
`D:\kmcounter\` 下的辅助脚本 `winscan.ps1 / probe.ps1 / realmouse.ps1 / trayrect.ps1 / cycle.ps1 / simfind.ps1`）。

