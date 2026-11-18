# CSTool 死亡切屏

一个只有约 300 KB 的 Windows 托盘小工具：在 `Counter-Strike 2` 中死亡后自动切到你选定的窗口、网页或程序，新回合开始时自动切回游戏。

A ~300 KB Windows tray tool: when you die in `Counter-Strike 2` it switches to a window, web page or program you chose, and switches back to the game when the next round starts.

## 来源与致谢

本项目把 [CS2Toolkit](https://github.com/thok404/CS2Toolkit) 中的“死亡一键切屏”功能单独移植为 Rust 程序，切屏逻辑移植自该项目的 `app/logic/visual_handler.py`、`application_windows.py`、`input_release.py` 和 `cs2_window.py`。

- 原作者：Moon4Quartz（[clover-233/CS2Toolkit](https://github.com/clover-233/CS2Toolkit)）
- 修复版维护者：[thok404/CS2Toolkit](https://github.com/thok404/CS2Toolkit)

本项目是独立的衍生作品，不是 CS2Toolkit 的官方版本；使用中遇到的问题请不要反馈给上述作者。

## 功能

- 三种切换目标：当前已打开的应用窗口、网页、本地程序
- “延迟切出”和“延迟切回”，各 0～10 秒
- 附加动作：切回时关闭目标，或者切出时播放媒体、切回时暂停（直接控制目标程序的媒体，不影响其他正在播放的程序）
- 切屏模式可选：死亡自动切出并切回、死亡不会自动切出且仅自动切回、停用
- 可选：切屏前松开按住的键盘键和鼠标键，避免按键卡住
- 半场过场不会提前切回；观战队友不会触发或打断切屏
- 托盘常驻，右键菜单可快速启用/停用
- 启动时检查是否有新版本，有则提示并提供下载入口

## 使用

1. 运行 `cstool.exe`。首次运行会自动查找 CS2，并在其 `game\csgo\cfg` 文件夹中写入 `gamestate_integration_cstool.cfg`。
2. **首次写入配置后需要重启一次 CS2**，游戏只在启动时读取这个文件。
3. 在设置窗口中选择死亡后要切到的目标。
4. 关闭设置窗口后程序留在系统托盘；用 `cstool.exe --tray` 启动可以不显示设置窗口。

设置保存在 `%LOCALAPPDATA%\CSTool\config.json`。默认监听 `127.0.0.1:3210`，端口被占用时会改用后面的端口并同步更新游戏里的配置。

## 工作原理

程序通过 Valve 官方的 Game State Integration（GSI）接收游戏状态：CS2 把你自己的血量、回合阶段等信息以 HTTP POST 发到本机端口。程序只请求 `provider`、`map`、`round`、`player_id`、`player_state` 五类数据，不读取游戏内存，也不注入游戏进程。

程序唯一的联网行为是启动时向 `api.github.com` 查询一次本项目的最新版本号；它不上传任何数据，也不会自动下载或安装。可以在托盘右键菜单中取消勾选“启动时检查更新”来关闭。

由于只能在收到消息后响应，切屏会比死亡略晚一点，具体取决于游戏发送消息和 Windows 切换窗口的速度。

## 反馈问题

如果切屏或媒体控制不符合预期，可以用 `cstool.exe --log` 启动程序，复现一次问题，然后把 `%LOCALAPPDATA%\CSTool\debug.log` 附在 Issue 里。日志只记录切屏步骤、前台程序名和各程序的音频状态，不记录按键内容和窗口内容。不加 `--log` 时不会写日志。

## 注意

- “切回时强制关闭所有浏览器进程”和“切回时强制关闭该程序的进程”会直接结束进程，未保存的内容会丢失。
- “松开按住的按键”只在确有按键被按住时短暂安装输入钩子（最长 5 秒），不记录任何键鼠内容。自动切出前如果模拟松键失败，程序会等你实际松手后再切出。等待期间复活会取消这次切出。
- 修改本地游戏文件夹、使用第三方工具的风险由使用者自行承担。

## 构建

需要 Rust（MSVC 工具链）和 Windows SDK。

```bash
cargo build --release
```

```bash
cargo test
```

图标由 `tools/make_icon.py` 生成（仅用 Python 标准库）。

## 跟进上游改动

切屏逻辑移植自 CS2Toolkit 的四个文件。仓库里的 GitHub Actions 每天检查这些文件在上游是否有变化；如果有，会自动开一个带 `upstream-update` 标签的 Issue。移植是手动完成的：处理完之后，把 `.github/upstream-baseline` 改为新的上游提交并关闭 Issue。

也可以在本地运行同一个检查（需要 GitHub CLI）：

```bash
DRY_RUN=1 bash tools/check_upstream.sh
```

## 源码结构

| 文件 | 内容 |
|---|---|
| `src/engine.rs` | 切屏状态机，不含任何 Windows 调用，带单元测试 |
| `src/gsi.rs` | GSI 接收服务、消息解析、游戏配置文件 |
| `src/win.rs` | 窗口查找、切换、最小化、进程处理 |
| `src/input_release.rs` | 切屏前释放按住的键鼠 |
| `src/media.rs` | 通过 Windows 媒体会话接口直接播放/暂停目标程序 |
| `src/audio.rs` | 检测浏览器或目标程序是否正在出声（媒体键后备方案使用） |
| `src/log.rs` | 可选的诊断日志（`--log`） |
| `src/update.rs` | 启动时检查 GitHub 上是否有新版本 |
| `src/ui.rs` | 设置窗口（原生 Windows 控件） |
| `src/main.rs` | 入口、托盘图标、定时器 |

## 许可证

本项目以 `GPL-3.0-only` 发布，详见 [LICENSE](./LICENSE)。CS2Toolkit 同样以 GPL-3.0 发布。

本项目不是 Valve 官方产品，也不隶属于 Valve。`Counter-Strike`、`CS2` 等名称和商标归其各自权利人所有。
