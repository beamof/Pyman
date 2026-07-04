# PyMan

一个用 Rust (egui) 编写的桌面窗口程序，用来管理 Python 脚本的运行：

- **添加脚本和参数**：填写名称、脚本路径和命令行参数，点击“添加并启动”。
- **直接运行 python 命令**：脚本路径**留空**时，「参数」框会被当作 `python` 的命令行参数，例如填 `-m http.server` 即运行 `python -m http.server`。支持 `-m 模块名`、`-c "代码"` 等任意写法（参数里用双引号 `"` 可分组带空格的值）。列表里这种条目自动命名为 `python <参数>`。
- **查看当前运行的脚本**：左侧列表显示每个条目及其状态（运行中 / 完成 / 失败 / 已停止 / 未运行）。
- **查看脚本日志**：选中某个条目即可实时查看它的 stdout / stderr 输出。
- **进程隔离**：每个脚本运行在**独立的 `python` 子进程**里 —— 一个脚本崩溃或卡死，不会拖垮管理器 UI。
- **单文件分发**：PyMan 就是一个 `pyman.exe`，不嵌任何额外二进制，也不链 `python3.dll` —— **GUI 不依赖 Python 也能启动**，Python 仅在真正跑脚本时才需要。
- **历史记录持久化**：添加过的脚本会自动保存，下次启动 PyMan 时自动加载到列表里（不用重新填写）。
- **自启动选项**：每个条目可单独勾选“自启”——勾选后，下次启动 PyMan 时该脚本会**自动运行**；未勾选的只加载到列表，不执行。

## 架构

```
┌──────────────┐   spawn python.exe <script> <args>   ┌────────────────┐
│   pyman      │ ───────────────────────────────────► │  python        │
│  (egui GUI)  │   (1 child per script)               │  解释器子进程   │
│  supervisor  │ ◄─────────────────────────────────── │  runs the .py  │
└──────────────┘        capture stdout/stderr         └────────────────┘
   GUI 二进制           就用系统里现成的 python，
   (不链接 pyo3)        不嵌任何 CPython
```

PyMan 是**单 crate、单 exe** 的程序。GUI（`pyman`）用 egui 渲染管理窗口，**不链接 pyo3、也不嵌入 CPython**，因此它的 PE 导入表里没有 `python3.dll` —— Windows 加载器启动 GUI 时不需要 Python，机器上没装 Python 也能正常打开界面。

跑脚本时，`supervisor` 直接 spawn 系统里现成的 `python` 解释器作为子进程，两种模式都一样：

- **脚本模式**（默认）：`python <脚本路径> <参数>`。
- **CLI 模式**（脚本路径留空）：`python <参数>`，例如 `python -m http.server`。

每个脚本一个独立 `python` 子进程，stdout/stderr 被按行捕获到日志缓冲、退出码被轮询分类。脚本崩溃/卡死只影响它自己的子进程，**不会拖垮 GUI**。spawn 之前，supervisor 会用 `worker` 模块里的发现逻辑（`find_python_on_path`）在 PATH 上找一个**真实的** Python 目录（同时含 `python.exe` 与 `python3.dll`，借此过滤掉 Windows Store 的 App Execution Alias 占位），取到解释器可执行文件路径（`find_python_exe`）后启动它，并把该 Python 目录 prepend 到子进程的 PATH，让脚本里的 `import` / 再 spawn 的子进程也能找到 Python。

> 历史背景：早期版本曾把 CPython 用 pyo3 **嵌入**到一个单独的 `pyman-worker` 二进制里，构建时 `include_bytes!` 嵌进 GUI exe、运行时解压再执行，并靠"GUI 与 worker 拆成两个二进制"来避免 `python3.dll` 污染 GUI 的加载期依赖。改成直接 spawn `python.exe` 后，GUI 不再碰 CPython，这整套 worker/embed/双 crate 机制都随之移除——**既统一了执行流程，又天然保持一个 exe、干净机器也能启动**。

这是一个单 crate 的 Cargo workspace：

| 模块 | 作用 |
|-------|------|
| `main.rs` | GUI 入口；`--self-test` 无界面自测。 |
| `app` | egui 界面 + 条目列表管理。 |
| `supervisor` | spawn `python` 子进程、按行读取 stdout/stderr、轮询退出状态、注入 Python PATH。 |
| `worker` | 纯 std 的 Python 发现逻辑（`find_python_on_path` / `find_python_exe`），不含 pyo3。 |
| `history` | 持久化脚本列表。 |
| `font` / `icon` | CJK 字体加载 / 应用图标。 |

**数据模型**：`app` 维护一个条目列表，每个条目是 `Entry { 名称, 路径, 参数, autostart, task: Option<ScriptTask> }`。`task` 为 `None` 表示该脚本是“已加载但未运行”的历史项；点“▶ 运行”才会 spawn `python` 子进程填上 `task`。这个单一结构同时承载了“有哪些脚本”和“哪些在跑”。

**持久化**：条目列表以 JSON 存到系统配置目录：
- Windows: `%APPDATA%\pyman\pyman_history.json`
- macOS: `~/Library/Application Support/pyman/pyman_history.json`
- Linux: `$XDG_CONFIG_HOME/pyman/pyman_history.json`（一般为 `~/.config/...`）

保存时机：添加、移除、切换自启时；写入是原子写临时文件 + rename。启动时加载：`autostart=true` 的条目立刻 spawn，其它条目以“未运行”状态进入列表。读取/写入失败只记日志、不崩溃（损坏的文件会被当作空列表忽略）。

## 依赖要求

- **Rust** 工具链（已用 1.96 测试）。
- **Python**：PyMan **不嵌入** CPython，直接调用系统里的解释器。
  - **构建期**：GUI 不依赖 pyo3 / abi3 / `python3.dll`，**构建机无需安装 Python**，`cargo build` 自带全部所需。
  - **运行期**：**启动 GUI 不需要 Python**（GUI 不链任何 Python 符号）。**跑脚本时**需要目标机器装有任意 Python 3（官方安装包自带 `python3.dll`）。Python 需在 `PATH` 中，或通过 `PYO3_PYTHON` 环境变量指向 `python.exe` 的完整路径。找不到时，GUI 会显示一条友好的中文提示而不是崩溃。

## 构建

```bash
cargo build --release
```

产物：

- `target/release/pyman.exe`（**唯一**对外可执行文件：整个程序就是这一个 exe，不嵌也不依赖任何额外二进制。）

## 运行

```bash
# 启动 GUI（双击或命令行均可）
./target/release/pyman
```

在界面里：

1. 在“名称”填一个易记的名字（可留空，默认用脚本文件名）。
2. 在“脚本路径”填入例如 `examples/hello.py`（可用绝对路径）。**留空**则进入「直接运行 python」模式：此时「参数」框作为 `python` 的命令行参数（例如 `-m http.server` 或 `-c "print(1)"`），等价于在终端敲 `python <参数>`。
3. 在“参数”里填入空格分隔的参数（脚本模式下会传给脚本的 `sys.argv`；python 模式下就是 python 自己的参数，支持双引号分组）。
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

## 目录结构

```
pyman/
├─ Cargo.toml                 # workspace 根（单个 member crate）
├─ crates/
│  └─ pyman/                  # GUI bin + lib（不链接 pyo3）
│     ├─ Cargo.toml
│     ├─ build.rs             # 仅设置 Windows 子系统链接参数
│     └─ src/{main,app,supervisor,worker,history,font,icon}.rs
└─ examples/                  # 示例 Python 脚本
   ├─ hello.py
   ├─ crash.py
   └─ loop.py
```
```
