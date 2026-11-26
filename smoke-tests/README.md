# 测试与验证入口

[项目首页](../README.md) · [文档中心](../docs/README.md) · [验证记录](../docs/VALIDATION.md) · [Windows 原生测试](windows/README.md)

> 普通 Rust/前端测试与真实 Terminal/安装版验收是不同层级。实际测试输入、产物版本、架构和证据范围以当次交付记录为准。

## 1. 普通 Rust 测试与静态检查

在项目根目录：

```bash
cargo fmt --manifest-path src-tauri/Cargo.toml --check
cargo test --manifest-path src-tauri/Cargo.toml
cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings
```

2026-11-26 当前源码 debug 与 `--release` 结果均为：**112 项通过、0 项失败、13 项显式 ignored**，严格 Clippy 退出码 0。ignored 不计入通过数。

普通测试覆盖解析、缓存、固定快照、导出、命令构造、Provider 身份、启动稳定门，以及 GitHub 更新的仓库/版本/URL 校验、忽略版本和设置持久化。它不访问真实 GitHub，也不证明真实 Terminal GUI 成功。

**不要直接对所有 ignored 测试使用 `--include-ignored`。** 其中包含真实 Terminal、真实存储、真实 CLI 和大型文件测试，必须按各自条件单独选择。

### Git 源码专用检查

提交和推送前还必须检查 Git 实际内容，不允许上传安装包、压缩附件、构建产物或真实会话：

```bash
./scripts/check-source-only.sh --staged
./scripts/check-source-only.sh --tracked
```

首次提交前没有 HEAD，只运行 `--staged`；`--tracked` 用于已有提交的源码树。本地 hooks 与 GitHub `source-only` workflow 使用同一检查器。当前 67 个暂存文件通过，伪造 DMG、无扩展名 Mach-O 和临时 HEAD 中的二进制均在独立临时仓库中被预期拒绝。完整规则见 [../docs/GITHUB_SOURCE_ONLY.md](../docs/GITHUB_SOURCE_ONLY.md)。

## 2. 完整前端 mock 冒烟

依赖本机 Node 和 Chrome，默认 Chrome 位于 `/Applications/Google Chrome.app`：

```bash
./smoke-tests/run-frontend-smoke.sh
```

无执行权限时：

```bash
zsh smoke-tests/run-frontend-smoke.sh
```

可通过 `CSM_CHROME_PATH` 指定 Chrome，通过 `CSM_CDP_PORT` 指定调试端口；未指定端口时自动选择空闲端口。

运行器加载真实 `src/index.html`，注入模拟 Tauri IPC，覆盖：

- Claude/Codex 切换、搜索、过滤、目录分组和选择集。
- 最新消息、固定快照反向分页、Markdown、HTML 转义。
- 收藏、单开参数、单窗多标签和跨 Provider 独立批次。
- 短/长/并发导出、清理警告和路径操作。
- 自启动失败回读与 Windows UA 快捷键分支。
- GitHub 更新设置、高版本横幅、纯文本 Release 名称、精确发布页 IPC。
- 忽略版本后后台不重复提示、手动重新查看、同版本无横幅。
- 后台更新失败静默、空仓库提示与手动错误显示。
- 桌面和 900px 布局、无 JavaScript 异常、无外部 HTTP 资源。

成功标志：`FRONTEND_SMOKE_OK`，退出码 0。当前完整冒烟已通过，0 JavaScript 异常、0 外部 HTTP 资源。

测试不读取真实会话、收藏、权限或 GitHub，不打开 Terminal。运行器只结束自己捕获的 Chrome PID，并删除自己的临时 Chrome profile 和下载目录。

### 截图

成功运行写入 `/tmp`：

```text
csm-ui-claude.png
csm-ui-detail.png
csm-ui-selection.png
csm-ui-context-menu.png
csm-ui-update.png
csm-ui-update-banner.png
csm-ui-codex.png
csm-ui-900.png
```

失败时额外写入 `csm-ui-failure.png`。图片只含 `/mock/...` 路径、模拟消息和合成更新结果；更新图中的仓库与 `9.9.9` 不是真实 Release。复制到文档前必须逐张查看，不能仅凭截图文件存在就宣称视觉通过。

## 3. 独立前端审查回归

```bash
./smoke-tests/run-frontend-smoke.sh ./frontend-review-regression.mjs
CSM_REVIEW_FILTER=duplicate-session-identity ./smoke-tests/run-frontend-smoke.sh ./frontend-review-regression.mjs
```

当前结果：

```text
REVIEW_REGRESSION_SUMMARY total=27 passed=27 failed=0
```

覆盖关键搜索/分页/收藏竞态、同会话失败重试、存储错误保留旧统计，以及 `update-panel-empty-startup-and-manual-failure`。更新 mock 会持久保存自己的模拟设置，确保保存后读取与真实后端契约一致。

## 4. macOS 无害 Terminal 原生标签测试

只有在明确授权操作 Terminal 后，才在 `src-tauri` 目录单独运行：

```bash
cargo test real_terminal_batch_smoke -- --ignored --nocapture
```

该测试创建自己的新窗口与 3 个原生标签，每页运行无害 `/bin/sleep 10`，不读取或恢复真实会话。成功标志包括：

```text
REAL_TERMINAL_SMOKE_OK
TERMINAL_SMOKE_VERIFIED 3 NATIVE_TABS 3
TEST_CLEANUP_CONFIRMED
```

它通过返回的对象 ID + TTY 核对测试对象，并用只读 AX 验证 `AXTabGroup` / `AXRadioButton`。AX 不可用、锁屏或身份不一致时必须失败，不能把 AppleScript 对象数冒充原生结构。

## 5. 隔离 Provider 启动确认与失败路径

明确授权后，在 `src-tauri` 目录：

```bash
CSM_ISOLATED_TERMINAL_SMOKE=1 \
cargo test tests::real_terminal_provider_confirmation_and_failures_smoke \
  -- --ignored --exact --nocapture
```

使用真实 Terminal 和合成 Node Provider，不调用官方 CLI、不读取真实会话、不发送模型提示。验证：

- 单开和混合批量在新 TTY 上匹配 Provider 角色、精确恢复 argv 和 session ID。
- Claude 已运行时，未打开的 Codex 仍可启动。
- 缺失 CLI、立即退出、Trust 未启动均返回 `0/1`，不能报告 opened。
- 完整启动集合连续稳定匹配 1000 ms。
- 只清理匹配本次夹具、PID、TTY、argv 和对象 ID 的测试对象。

成功标志：

```text
REAL_TERMINAL_PROVIDER_CONFIRMATION_OK single=1 batch=2 negative=3 cleanup=true
OWNED_PROVIDER_PROCESSES_REMAINING=0
OWNED_TERMINALS_REMAINING=0
```

## 6. 真实 Claude + Codex 混合恢复测试

这是更敏感的 opt-in 测试，只有明确授权并能选定停止会话时才运行：

```bash
CSM_REAL_TERMINAL_SMOKE=1 \
CSM_PROTECT_CLAUDE_ID=<current-session-id> \
cargo test real_claude_codex_native_tabs_smoke -- --ignored --nocapture --test-threads=1
```

约束：

- `CSM_PROTECT_CLAUDE_ID` 用于排除当前对话，不得选择当前会话。
- 选择已停止、目录有效且已接受 Trust 的测试目标。
- 不修改 Trust、真实权限或认证配置。
- 不发送模型提示，不读取或输出真实会话正文。
- 只按本次新 TTY + session ID + 精确 argv 识别和清理 Provider。
- 只关闭本次返回且 ID + TTY 匹配的 Terminal 对象。

成功标志：

```text
REAL_PROVIDER_MIXED_SMOKE_OK exact_flags=true native_tabs=2 claude_registered=true codex_registered=true cleanup=true
```

即使通过，也只证明恢复进程与原生标签，不等于完整历史正文已验收。

## 7. CLI 自动发现与版本

### 只解析，不执行 Provider

macOS 示例：

```bash
CARGO_BIN="$(command -v cargo)"
RUSTC_BIN="$(command -v rustc)"
env -u CSM_CLAUDE_CLI -u CSM_CODEX_CLI \
  PATH="/usr/bin:/bin" RUSTC="$RUSTC_BIN" CSM_REAL_CLI_DISCOVERY=1 \
  "$CARGO_BIN" test provider_cli::tests::real_provider_cli_auto_discovery_smoke \
  -- --ignored --exact --nocapture
```

Windows PowerShell 示例：

```powershell
$cargo = (Get-Command cargo.exe).Source
$env:PATH = "$env:USERPROFILE\.cargo\bin;$env:SystemRoot\System32"
Remove-Item Env:CSM_CLAUDE_CLI,Env:CSM_CODEX_CLI -ErrorAction SilentlyContinue
$env:CSM_REAL_CLI_DISCOVERY = '1'
& $cargo test provider_cli::tests::real_provider_cli_auto_discovery_smoke -- --ignored --exact --nocapture
```

成功标志：`REAL_PROVIDER_CLI_DISCOVERY_OK claude=1 codex=1 executed=0`。自动 Node PATH 的成功标志为 `REAL_PROVIDER_RUNTIME_PATH_OK prepared=1 executed=0`。

### 只执行 `--version`

设置 `CSM_REAL_CLI_VERSION=1` 后单独运行 `provider_cli::tests::real_provider_cli_version_with_runtime_path_smoke`，使用 `--ignored --exact --nocapture`。成功标志：

```text
REAL_PROVIDER_CLI_VERSION_OK claude=1 codex=1 prompts=0
```

该测试不发送模型提示、不打印认证配置；`--version` 通过不等于完整恢复通过。

## 8. Windows 原生 WebView2 / Terminal 测试

使用实际 Release App、真实 WebView2 和 UIA，不是 Chrome 修改 Windows UA。运行方法和输出见 **[windows/README.md](windows/README.md)**。

核心成功标志为 `WINDOWS_GUI_SMOKE_OK`，并要求：

- 原生窗口和 owned 标签数量符合场景。
- 精确 argv/cwd/环境与 1000 ms 稳定确认。
- 负向路径不误报 opened。
- 0 JavaScript 异常、0 意外外部 HTTP 请求、0 模型提示。
- `OWNED_TERMINAL_TABS_REMAINING=0`、`OWNED_PROVIDER_PROCESSES_REMAINING=0`。

## 9. GitHub 更新的真实验收

普通 Rust 与前端 mock 不访问真实 GitHub。项目有真实公开仓库后，另行进行只读验收：

1. 配置真实 `owner/repository`。
2. 同版本安装手动检查，不应显示升级横幅。
3. 旧版本安装检查更高 latest stable Release，应显示横幅。
4. 核对发布页属于同一仓库，Release 名称只作为文本。
5. 验证忽略、稍后提醒、后台失败和手动重试。
6. 不需要下载或安装新包，不伪造高版本 Release。

当前尚未完成真实公开仓库验证。包含更新功能的 macOS App/DMG 已构建并校验，但尚未安装；旧安装版不包含该功能。

## 10. 不可违反的真实测试清理规则

- 永不关闭当前 Claude Code 对话窗口或用户已有 Terminal。
- 不通过“关闭全部 Terminal 窗口”清理，不重启 Terminal 或电脑。
- 只创建、检查和清理当前测试拥有的对象。
- 不把前后 window ID 差集当作清理所有权。
- macOS 清理匹配当前脚本返回的对象 ID 与唯一 TTY。
- Windows 只 Invoke owned 标签自己的唯一关闭子按钮，不调用父窗口 Close。
- 生产代码不得增加窗口关闭、失败自动清理或 AX 清理。
- 不按进程名批量结束 CLI，不停止真实 Codex daemon 或 WindowsTerminal。
- 不修改真实项目 Trust/权限，不直接编辑 TCC 或重置系统安全设置。
- 不读取/打印真实会话正文作为测试日志，不发送模型提示，不绕过锁屏。
- 测试后及时清理；无法证明所有权时保留并报告，不扩大清理范围。

历史实机范围与未覆盖项见 [../docs/VALIDATION.md](../docs/VALIDATION.md)。测试通过不等于“已经没有 bug”。
