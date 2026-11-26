# Windows 实现与验收边界

[文档中心](README.md) · [Windows 编译](BUILD.md#windows-构建) · [安装与授权](INSTALL.md#windows-arm64安装-nsis推荐) · [使用指南](USAGE.md) · [验证记录](VALIDATION.md)

> 当前已有 Windows 11 ARM64 的真实 Release、NSIS、WebView2 与原生 Terminal 隔离验收，但这不等于所有 Windows 架构、真实账号或最新更新功能均已完成验证。实现针对原生 Windows，不把 WSL 文件系统和进程混作原生 Windows 会话。

## 1. 当前实现范围

| 模块 | 实现 | 已有验证与缺口 |
| --- | --- | --- |
| 会话列表与详情 | 共享 JSONL、收藏、搜索、目录分组和固定快照分页 | 真实 WebView2 + 合成数据通过；完整官方 TUI 恢复未完全验收 |
| Codex SQLite | `rusqlite` bundled，只读 `state_5.sqlite` | 内嵌 SQLite 与合成数据库通过，不需要外置 sqlite3 |
| CLI 查找 | 显式绝对路径、PATH/PATHEXT、当前用户安装目录 | 特殊目录 `.cmd` 和不在 PATH 的用户安装目录已实测 |
| 单会话恢复 | 一个新 Terminal 标签，精确 argv 与稳定进程确认 | 停止态单开和同 UUID 并发保护已测；原生聚焦未实现 |
| 批量恢复 | 每次 `wt.exe -w new`，多个 `new-tab` | 真实 UIA 已核实一个窗口多个顶部标签 |
| 工作目录 | PowerShell `Set-Location -LiteralPath` + EncodedCommand | 中文、空格、单引号、`&`、分号路径已测 |
| 已运行会话 | Claude 记录/PID、精确 argv；Codex 另有 Restart Manager | daemon 与 VS Code 来源对照已测，未知来源仍保守拦截 |
| 文件管理器 | `explorer.exe /select,<canonical-path>` | 尚未实机验收 |
| 登录自启动 | 当前用户 HKCU Run | 尚未实机切换验收，只启动管理器 |
| 安装包 | 当前用户 NSIS | ARM64 更新、卸载、干净重装和安装路径 GUI 已有记录 |
| GitHub 更新 | 共享 Rust 更新模块 + 前端面板 | 新源码仅完成 Rust/前端 mock；尚未做新 Windows 构建或真实 GitHub 验收 |

已有安装版结果属于先前构建，不能自动继承给加入更新功能后的未来二进制。

## 2. 环境与运行时

需要：

1. stable Rust MSVC（仅源码构建需要）。
2. Visual Studio C++ 工作负载与 Windows SDK（仅源码构建需要）。
3. WebView2 Runtime。
4. Windows Terminal，`wt.exe` 应用执行别名可用。
5. Windows PowerShell `powershell.exe`，当前不是自动改用 `pwsh`。
6. 原生 Windows Claude Code / Codex CLI，按官方流程安装和登录。
7. CLI 需要时自动发现 Node；可用 `CSM_NODE_CLI` 指定绝对路径。

人工诊断：

```powershell
where.exe claude
where.exe codex
where.exe wt
where.exe powershell
claude --version
codex --version
```

应用内部不解析 `where.exe` 文本，而是直接遍历 PATH/PATHEXT 和用户安装目录，避免控制台代码页破坏 Unicode 路径。PATH 之外还会检查常见 Node、npm、Volta、Bun、Cargo、asdf、mise、pnpm、WinGet Links 等当前用户位置。

支持 `.exe`、`.cmd`、`.bat`、`.ps1` 等可执行形态。`.ps1` 只在本次恢复 PowerShell 进程内使用 `Process` 作用域执行策略；真实 `.ps1` Provider 的完整恢复仍未验收。shell 函数或别名不等于应用可定位的可执行文件。

## 3. 数据、缓存、更新与日志

```text
Claude 配置：%USERPROFILE%\.claude\
Codex 配置：%USERPROFILE%\.codex\
收藏：%CLAUDE_CONFIG_DIR%\favorites.json（未设置时使用默认 Claude 目录）
缓存：%LOCALAPPDATA%\com.claude.sessionmanager\session-metadata-v1.json
更新设置：%APPDATA%\com.claude.sessionmanager\update-settings.json
日志：%LOCALAPPDATA%\Claude Session Manager\Logs\launch.log
```

仍支持 `CLAUDE_CONFIG_DIR`、`CODEX_HOME`、`CSM_CLAUDE_CLI`、`CSM_CODEX_CLI`、`CSM_NODE_CLI`、`CSM_CACHE_DIR` 和 `CSM_DEBUG_LOG`。路径按实际用户环境解析，不写死某个验收机器用户名。

更新面板只保存公开仓库和检查偏好，不需要 GitHub Token。更新检查尚未在该平台的新构建中实测；不能由 macOS 前端 mock 推导为 Windows 安装版已通过。

## 4. Windows Terminal 单窗多标签

逻辑结构：

```text
wt.exe -w new
  new-tab --title <title-A>
          --suppressApplicationTitle --inheritEnvironment
          powershell.exe -NoLogo -NoExit -EncodedCommand <command-A>
  ;
  new-tab --title <title-B>
          --suppressApplicationTitle --inheritEnvironment
          powershell.exe -NoLogo -NoExit -EncodedCommand <command-B>
```

真实代码通过 `std::process::Command` 逐项传 argv，分号也是独立参数，不用 `cmd /c` 拼接整段命令。`-w new` 是刻意选择，不复用 `0`、`last`、固定窗口名或用户已有数字 ID。

工作目录不交给 `wt new-tab --startingDirectory`，因为含分号目录会影响 Windows Terminal 子命令解析。改为在编码后的 PowerShell 中：

```powershell
$env:CLAUDE_CODE_CHILD_SESSION = $null
$env:CLAUDECODE = $null
Set-Location -LiteralPath '<validated-cwd>' -ErrorAction Stop
& '<resolved-cli>' '<provider-flags...>' '<session-id>'
```

单引号按 PowerShell 规则转为 `''`，完整脚本编码为 UTF-16LE Base64。标签标题移除控制字符，ASCII `;` 替换为全角 `；`，避免子命令分隔冲突。

Provider 参数保持：

```text
Claude: --dangerously-skip-permissions -r <session-id>
Codex:  --dangerously-bypass-approvals-and-sandbox resume <session-id>
```

应用不传字体、字号、配色或布局覆盖参数，沿用 Windows Terminal 默认 profile。两个启动 IPC 使用后台阻塞任务，避免稳定核实期间冻结 WebView2；macOS 的同步主线程实现不能套用到 Windows。

Microsoft 命令行参考：<https://learn.microsoft.com/en-us/windows/terminal/command-line-arguments>。

## 5. 已运行会话与 Codex daemon

判断顺序：

1. Claude 读取会话运行记录中的 Session ID / PID，并核实进程属于 Claude。
2. 扫描 sysinfo 进程 argv，要求 Provider 标记、独立且精确的 Session ID 参数和完整恢复 flags。
3. Codex argv 不含可识别 Session ID 时，用 Restart Manager 只读查询 rollout 文件占用，再核实 PID 属于 Codex。

Restart Manager 只调用 `RmStartSession`、`RmRegisterResources`、`RmGetList`、`RmEndSession`，不调用 `RmShutdown`、`RmRestart`，也不按进程名终止。

CLI 来源 rollout 被精确 `app-server-daemon` 路径组件持有时，该后台句柄不单独阻止重开；真正的交互恢复进程仍优先阻止重复。`source=vscode`、未知来源、普通 Codex exe 或路径不可得的持有者仍保守拦截。

启动前记录已有 `(PID, start_time)`，启动后只把新身份计入结果。完整集合连续匹配 1000 ms 才算成功。进程核实不等于窗口结构证明，窗口/标签由独立只读 UIA 验收。

## 6. 失败与重复操作行为

- 失效的非空 cwd 在创建终端前报错。
- Provider 立即退出或 Trust 未启动不能误报 opened。
- 相同 Provider/session 已运行时，Windows 返回不能聚焦原标签并阻止重复，不静默再开一份。
- 不相交批次继续调用 `wt -w new`，不限制 Provider 窗口总数。
- 部分会话已运行时，只为剩余停止会话创建新批次。
- 生产路径不关闭窗口、不自动清理失败对象、不终止既有 CLI。

## 7. 自启动、安装与卸载

自启动位置：

```text
HKCU\Software\Microsoft\Windows\CurrentVersion\Run
值名：ClaudeSessionManager
```

只读检查：

```powershell
reg query "HKCU\Software\Microsoft\Windows\CurrentVersion\Run" /v ClaudeSessionManager
```

登录只启动管理器，不自动批量恢复收藏。自启动真实系统切换仍未验收。

NSIS 为 `installMode = currentUser`，默认安装到 `%LOCALAPPDATA%\Claude Session Manager\csm-app.exe`。独立 Release、NSIS 内层载荷与最终安装文件应按当次交付证据分别核对，不假设它们总是同一个文件哈希。

当前没有 Authenticode 发布者签名。不要通过关闭 SmartScreen 解决来源不明问题。安装、更新、卸载与数据保留见 [DEPLOYMENT.md](DEPLOYMENT.md)。

## 8. 真实测试的 owned 规则

Windows 隔离 GUI 测试必须：

- 使用全新的随机 run ID 与合成 Provider，不连接模型服务、不发送提示。
- 不读取真实会话正文，不修改真实 Trust、认证或权限。
- 只读 UIA 核对完整唯一标题、ControlType、Runtime ID、进程和 HWND。
- 只 Invoke 已确认所有权标签自己的唯一关闭子按钮。
- 不调用父窗口 `WindowPattern.Close`，不关闭混合归属窗口。
- 不按进程名结束 Claude/Codex，不停止真实 Codex daemon 或 WindowsTerminal。
- 测试结束后确认 owned 标签、Provider、测试 App 与临时 root 按证据清理。
- 不把前后窗口差集当作所有权，不用全屏截图暴露真实窗口。

运行入口见 [../smoke-tests/windows/README.md](../smoke-tests/windows/README.md)。

## 9. 扩大支持声明前的门槛

1. 对加入更新功能的新 Windows 构建重新运行普通测试、NSIS 与真实 WebView2/Terminal 验收。
2. 用真实公开 GitHub 仓库分别验证同版本和更高版本，只读检查，不必下载或安装。
3. 用明确选择的真实会话验证官方 CLI 完整历史和正文恢复，不复制 macOS 认证秘密。
4. 分别验证 Explorer、自启动和运行中标签聚焦；未实现的功能不能写成已支持。
5. 在 Windows x64 和其他受支持版本上重复构建与原生结构测试；MSI 单独验收。
6. 实际运行 Windows 源码 CI，保存 Actions URL 与测试日志；CI 文件存在不是绿色证据。源码 CI 不构建或上传安装附件，Release 附件由所有者以后单独发布。
