# PyMan

一个用 Rust (egui) 编写的桌面窗口程序，用来管理 Python 脚本的运行：

- **添加脚本和参数**：填写名称、脚本路径和命令行参数，点击“添加并启动”。
- **查看当前运行的脚本**：左侧列表显示每个条目及其状态（运行中 / 完成 / 失败 / 已停止 / 未运行）。
- **查看脚本日志**：选中某个条目即可实时查看它的 stdout / stderr 输出。
- **进程隔离**：每个脚本运行在**独立的 pyo3 进程**里 —— 一个脚本崩溃或卡死，不会拖垮管理器 UI。
- **单文件分发**：worker 二进制在构建期被打包进 GUI exe，运行时解压到用户数据目录再执行。发布/分发仍是**一个 exe**，且 **GUI 不依赖 Python 也能启动**（Python 仅在真正跑脚本时才需要）。
- **历史记录持久化**：添加过的脚本会自动保存，下次启动 PyMan 时自动加载到列表里（不用重新填写）。
- **自启动选项**：每个条目可单独勾选“自启”——勾选后，下次启动 PyMan 时该脚本会**自动运行**；未勾选的只加载到列表，不执行。

## 架构

```
┌──────────────┐   embed::ensure_worker()      ┌──────────────────┐
│   pyman      │ ────────────────────────────► │  pyman-worker    │
│  (egui GUI)  │   解压内嵌 worker 并 spawn     │  (pyo3 + CPython)│
│  supervisor  │   (1 child per script)        │  runs the .py    │
│              │ ◄──────────────────────────── │                  │
└──────────────┘        capture stdout/stderr  └──────────────────┘
   GUI 二进制             worker 是独立二进制，
   (不链接 pyo3)          构建时被嵌进 GUI exe
```

PyMan 是**双 crate、单下载文件**的程序：

- **`pyman`**（GUI）：egui 管理窗口。**不链接 pyo3**，因此它的 PE 导入表里没有 `python3.dll` —— Windows 加载器启动 GUI 时不需要 Python，机器上没装 Python 也能正常打开界面。`supervisor` 给每个脚本 spawn 一个 worker 子进程；`embed` 模块负责把构建期内嵌进 GUI exe 的 worker 二进制**解压**到用户数据目录（`%LOCALAPPDATA%\pyman\`）并缓存（按内容指纹，不变就不重写）。
- **`pyman-worker`**（脚本执行进程）：**唯一**链接 pyo3 的地方。用 `pyo3`（`auto-initialize`）嵌入 CPython，把脚本作为 `__main__` 执行并正确设置 `sys.argv`。它链接 `python3.dll` 作为**加载期硬依赖**——supervisor 在 spawn 它之前会先扫描 PATH 找到一个真实的 Python 目录（同时含 `python.exe` 与 `python3.dll`），把它**注入到 worker 子进程的 PATH 最前面**，这样 worker 进程启动时加载器才能解析 `python3.dll`。

**为什么拆成两个 crate？** 这是让 GUI 在干净机器上能启动的关键。曾经 worker 和 GUI 合并在同一个二进制里，但 pyo3 的 `abi3 + generate-import-lib` 会把 `python3.dll` 变成该二进制的**加载期硬依赖**，导致**每次启动 GUI**（甚至 `main()` 还没跑）Windows 加载器都要找 `python3.dll`，找不到就弹「找不到 python3.dll」错误窗。`/DELAYLOAD` 也救不了——pyo3 导入了 `PyExc_*` 等**数据符号**，MSVC 无法延迟数据导入（`LNK1194`）。把 worker 拆成独立二进制后，GUI 的导入表里就完全没有 Python 了。

这是一个 Cargo workspace，包含两个 crate：

| crate | 作用 |
|-------|------|
| `pyman` | GUI bin + lib。`main.rs` 是 GUI 入口；`app` 模块渲染界面并管理条目列表；`supervisor` 启动 worker 子进程、按行读取 stdout/stderr、轮询退出状态、注入 Python PATH；`worker` 模块只含**纯 std 的 Python 发现逻辑**（`find_python_on_path` 等，不含 pyo3）；`embed` 模块负责解压内嵌的 worker 二进制；`history` 持久化脚本列表。**GUI 进程自身绝不链接、也不初始化 Python。** |
| `pyman-worker` | 脚本执行进程。**唯一**依赖 pyo3 的 crate。lib 里的 `run()` 用 `pyo3`（`auto-initialize`）嵌入 CPython，执行单个脚本后退出；bin 是一行 `std::process::exit(run())` 包装。它的编译产物在 GUI 的 `build.rs` 里被 `include_bytes!` 进 `pyman.exe`。 |

**数据模型**：`app` 维护一个条目列表，每个条目是 `Entry { 名称, 路径, 参数, autostart, task: Option<ScriptTask> }`。`task` 为 `None` 表示该脚本是“已加载但未运行”的历史项；点“▶ 运行”才会 spawn worker 填上 `task`。这个单一结构同时承载了“有哪些脚本”和“哪些在跑”。

**持久化**：条目列表以 JSON 存到系统配置目录：
- Windows: `%APPDATA%\pyman\pyman_history.json`
- macOS: `~/Library/Application Support/pyman/pyman_history.json`
- Linux: `$XDG_CONFIG_HOME/pyman/pyman_history.json`（一般为 `~/.config/...`）

保存时机：添加、移除、切换自启时；写入是原子写临时文件 + rename。启动时加载：`autostart=true` 的条目立刻 spawn，其它条目以“未运行”状态进入列表。读取/写入失败只记日志、不崩溃（损坏的文件会被当作空列表忽略）。

supervisor 启动 worker 时，不是重新执行 GUI 自身，而是解压构建期内嵌进 `pyman.exe` 的 `pyman-worker` 二进制（见 `embed` 模块），把它写到 `%LOCALAPPDATA%\pyman\pyman-worker.exe`（按内容指纹缓存，不变就不重写），然后 spawn 它。因此发布时只需把**一个** `pyman.exe` 放到目标目录——worker 是嵌在里面的。

## 依赖要求

- **Rust** 工具链（已用 1.96 测试）。
- **Python**：**只有 worker** (`pyman-worker`) 通过 pyo3 的 `auto-initialize` 在运行机器上加载已安装的 CPython；GUI 不碰 Python。
  - **构建期**：采用 `abi3-py38` 稳定 ABI + `generate-import-lib`，pyo3 会自动合成 `python3.dll` 导入库，**构建机无需安装 Python**。（GUI 的 `build.rs` 会 shell out 调 `cargo` 把 worker 编进一个独立 target 目录再 `include_bytes!` 进 GUI exe，见 `crates/pyman/build.rs`。）
  - **运行期**：**启动 GUI 不需要 Python**（GUI 不链接 pyo3）。但**跑脚本时**，worker 需要目标机器装有 **Python 3.8+** 的任意次版本（官方安装包自带 `python3.dll`），一个 build 即可跑 3.8 / 3.9 / 3.10 / 3.11 / 3.12 / 3.13 / 3.14+，不再绑定具体版本。Python 需在 `PATH` 中，或通过 `PYO3_PYTHON` 环境变量指向 `python.exe`。找不到时，GUI 会显示一条友好的中文提示而不是崩溃。

## 构建

```bash
cargo build --release
```

产物：

- `target/release/pyman.exe`（**唯一**对外可执行文件：GUI + 内嵌的 worker。`pyman-worker.exe` 也会单独编译出来，但已被嵌进 `pyman.exe`，不需要随包分发。）

## 运行

```bash
# 启动 GUI（双击或命令行均可）
./target/release/pyman
```

在界面里：

1. 在“名称”填一个易记的名字（可留空，默认用脚本文件名）。
2. 在“脚本路径”填入例如 `examples/hello.py`（可用绝对路径）。
3. 在“参数”里填入空格分隔的参数（会传给脚本的 `sys.argv`）。
4. 可选：勾选“下次启动自动运行”，让这条脚本以后每次开 PyMan 都自动跑起来。
5. 点击“▶ 添加并启动”。左侧出现该条目并立刻运行；选中它即可在右侧看到实时日志。
6. 每个条目旁有：
   - **自启**：切换是否下次启动时自动运行（绿色 = 开）。
   - **⏹ 停止**（运行中时）/ **▶ 运行**（未运行时）：停止或重新启动该脚本。
   - **✕ 移除**：从列表和历史记录里删除（需先停止）。

关闭并重新打开 PyMan，所有条目都会回来；勾了“自启”的会自动开跑。

## 自测（无界面）

GUI 二进制带一个 `--self-test` 模式，它用真实的 supervisor 跑一遍 `examples/hello.py`，校验日志流和退出分类是否正确。无需显示器，适合 CI：

```bash
./target/release/pyman --self-test
# 期望输出: self-test: PASS   (退出码 0)
```

仓库自带的示例脚本：

| 脚本 | 说明 |
|------|------|
| `examples/hello.py` | 打印 argv、交替输出 stdout/stderr、正常结束。 |
| `examples/crash.py` | 抛出未捕获异常，演示“崩溃不影响 GUI”，任务被标记为失败。 |
| `examples/loop.py <count>` | 长时间运行，用来测试“停止”按钮。 |

## 直接调用 worker

worker 是独立二进制，也可以脱离 GUI 直接运行，便于调试（需要目标机器有 Python 3.8+，因为 worker 硬依赖 `python3.dll`）：

```bash
# 直接跑编译出来的 worker（它会去掉可选的 --worker 前缀）
./target/release/pyman-worker path/to/script.py arg1 arg2
```

约定：worker 的 stdout/stderr 完全留给脚本输出（GUI 会原样捕获并展示）。worker 自身只在启动时往 **stderr** 打印一行 `{"kind":"started",...}` JSON 作为存活信号。

## 目录结构

```
pyman/
├─ Cargo.toml                 # workspace 根（两个 member crate）
├─ crates/
│  ├─ pyman/                  # GUI bin（不链接 pyo3）
│  │  ├─ Cargo.toml
│  │  ├─ build.rs             # 构建期：把 worker 二进制 include_bytes! 进 GUI
│  │  └─ src/{main,app,supervisor,worker,embed,history,font,icon}.rs
│  └─ pyman-worker/           # 脚本执行 bin（唯一链接 pyo3 的地方）
│     ├─ Cargo.toml
│     └─ src/{main,lib}.rs    # lib::run() 嵌 CPython 执行脚本
└─ examples/                  # 示例 Python 脚本
   ├─ hello.py
   ├─ crash.py
   └─ loop.py
```
