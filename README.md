# PyMan

一个用 Rust (egui) 编写的桌面窗口程序，用来管理 Python 脚本的运行：

- **添加脚本和参数**：填写名称、脚本路径和命令行参数，点击“添加并启动”。
- **查看当前运行的脚本**：左侧列表显示每个条目及其状态（运行中 / 完成 / 失败 / 已停止 / 未运行）。
- **查看脚本日志**：选中某个条目即可实时查看它的 stdout / stderr 输出。
- **进程隔离**：每个脚本运行在**独立的 pyo3 进程**里 —— 一个脚本崩溃或卡死，不会拖垮管理器 UI。
- **单文件分发**：GUI 和脚本执行进程合并在**同一个 `pyman` 二进制**里，发布/分发只需一个 exe。
- **历史记录持久化**：添加过的脚本会自动保存，下次启动 PyMan 时自动加载到列表里（不用重新填写）。
- **自启动选项**：每个条目可单独勾选“自启”——勾选后，下次启动 PyMan 时该脚本会**自动运行**；未勾选的只加载到列表，不执行。

## 架构

```
┌──────────────┐  re-exec self with --worker   ┌──────────────────┐
│   pyman      │ ────────────────────────────► │  pyman --worker  │
│  (egui GUI)  │   (1 child per script)        │  (pyo3 + CPython)│
│  supervisor  │ ◄──────────────────────────── │  runs the .py    │
└──────────────┘        capture stdout/stderr  └──────────────────┘
   同一个二进制                                  同一个二进制
```

PyMan 是**单二进制、双角色**的程序：默认以 GUI 模式启动；当被以 `pyman --worker <script> [args...]`（或被改名为 `pyman-worker` 运行）调用时，进入 **worker 模式**，用 pyo3 嵌入 CPython 执行单个脚本后退出。supervisor 给每个脚本 spawn 一个 `--worker` 子进程（即重新执行自身），所以一次发布只需一个 `pyman[.exe]`，但仍然保留真正的进程隔离——脚本崩溃不会拖垮 GUI。

这是一个 Cargo workspace，包含一个 crate：

| crate | 作用 |
|-------|------|
| `pyman` | 既是 lib 也是 bin 的单一 crate。`main.rs` 根据 argv 决定角色（`--worker` / 改名 `pyman-worker` → worker；否则 → GUI）。`app` 模块渲染界面并管理条目列表；`supervisor` 启动 `--worker` 子进程、按行读取它们的 stdout/stderr、轮询退出状态；`history` 负责把脚本列表持久化到磁盘并在启动时加载；`worker` 模块用 `pyo3`（`auto-initialize` 特性）嵌入 CPython，把脚本作为 `__main__` 执行并正确设置 `sys.argv`。**GUI 进程自身绝不初始化 Python。** |

**数据模型**：`app` 维护一个条目列表，每个条目是 `Entry { 名称, 路径, 参数, autostart, task: Option<ScriptTask> }`。`task` 为 `None` 表示该脚本是“已加载但未运行”的历史项；点“▶ 运行”才会 spawn worker 填上 `task`。这个单一结构同时承载了“有哪些脚本”和“哪些在跑”。

**持久化**：条目列表以 JSON 存到系统配置目录：
- Windows: `%APPDATA%\pyman\pyman_history.json`
- macOS: `~/Library/Application Support/pyman/pyman_history.json`
- Linux: `$XDG_CONFIG_HOME/pyman/pyman_history.json`（一般为 `~/.config/...`）

保存时机：添加、移除、切换自启时；写入是原子写临时文件 + rename。启动时加载：`autostart=true` 的条目立刻 spawn，其它条目以“未运行”状态进入列表。读取/写入失败只记日志、不崩溃（损坏的文件会被当作空列表忽略）。

supervisor 通过 `std::env::current_exe()` 拿到自身路径并带上 `--worker` 重新执行，因此发布时只需把**一个** exe 放到目标目录即可（`current_exe()` 不可用时回退到 PATH 中的 `pyman`）。

## 依赖要求

- **Rust** 工具链（已用 1.96 测试）。
- **Python**：worker 通过 pyo3 的 `auto-initialize` 在运行机器上加载已安装的 CPython。
  - **构建期**：采用 `abi3-py38` 稳定 ABI + `generate-import-lib`，pyo3 会自动合成 `python3.dll` 导入库，**构建机无需安装 Python**。
  - **运行期**：目标机器只需装有 **Python 3.8+** 的任意次版本（官方安装包自带 `python3.dll`），一个 build 即可跑 3.8 / 3.9 / 3.10 / 3.11 / 3.12 / 3.13 / 3.14+，不再绑定具体版本。Python 需在 `PATH` 中，或通过 `PYO3_PYTHON` 环境变量指向 `python.exe`。

## 构建

```bash
cargo build --release
```

产物：

- `target/release/pyman`（**唯一**可执行文件，同时充当 GUI 和脚本执行进程）

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

worker 模式也可以脱离 GUI 直接运行，便于调试。两种等价的写法：

```bash
# 方式一：--worker 标志（supervisor 内部用的就是这种）
./target/release/pyman --worker path/to/script.py arg1 arg2

# 方式二：把 exe 改名为 pyman-worker（旧用法仍兼容，方便习惯）
cp target/release/pyman target/release/pyman-worker
./target/release/pyman-worker path/to/script.py arg1 arg2
```

约定：worker 的 stdout/stderr 完全留给脚本输出（GUI 会原样捕获并展示）。worker 自身只在启动时往 **stderr** 打印一行 `{"kind":"started",...}` JSON 作为存活信号。

## 目录结构

```
pyman/
├─ Cargo.toml                 # workspace 根（仅一个 member crate）
├─ crates/
│  └─ pyman/                  # 单一二进制：GUI + worker 双角色
│     ├─ Cargo.toml
│     ├─ build.rs
│     └─ src/{main,app,supervisor,worker,history,font,icon}.rs
└─ examples/                  # 示例 Python 脚本
   ├─ hello.py
   ├─ crash.py
   └─ loop.py
```
