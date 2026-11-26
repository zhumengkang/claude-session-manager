# 故障排查与已知边界

[文档中心](README.md) · [安装与授权](INSTALL.md) · [使用指南](USAGE.md) · [GitHub 更新](UPDATES.md) · [配置与隐私](CONFIGURATION.md)

> 先按具体症状定位，不要通过关闭全部 Terminal、结束所有 Claude/Codex 进程、删除真实会话目录或全局关闭系统安全功能来“修复”。

## 1. 会话列表为空或数量偏少

按顺序确认：

1. 顶部是否选择了正确的 Claude / Codex Provider。
2. 搜索、收藏、最近 30、主会话等过滤是否缩小了结果。
3. 默认目录是否存在：

   ```bash
   test -d "$HOME/.claude/projects" && echo "Claude projects exists"
   test -d "$HOME/.codex/sessions" && echo "Codex sessions exists"
   test -f "$HOME/.codex/state_5.sqlite" && echo "Codex sqlite exists"
   ```

4. 是否设置了指向其他位置的 `CLAUDE_CONFIG_DIR` / `CODEX_HOME`。
5. 双击 App 与 shell 启动是否使用不同环境。
6. 会话主 JSONL / rollout 是否仍存在；只有索引不等于可恢复。
7. 文件权限是否允许当前用户读取。
8. CLI 是否升级到当前解析器尚未支持的新存储格式。

存储读取错误不能被当成“零条会话”。保留脱敏错误信息，不要为排查数量公开上传完整会话目录。配置规则见 [CONFIGURATION.md](CONFIGURATION.md)。

## 2. 点击打开后只有 shell，没有恢复 CLI

先在 Terminal 人工检查：

```bash
command -v claude
claude --version
claude --help

command -v codex
codex --version
codex --help
codex resume --help
```

然后核对：

- App 是否能找到 CLI；终端可用但 App 找不到时，常是 GUI PATH 与 shell PATH 不同。
- 需要时用 `CSM_CLAUDE_CLI` / `CSM_CODEX_CLI` 指定可执行文件绝对路径。
- CLI 当前版本是否支持文档中的恢复参数。
- CLI 是否已按官方流程安装、初始化并登录。
- 原项目目录是否存在且仍是目录；无效目录不会静默退回用户主目录。
- 对应 JSONL / rollout 是否仍存在。
- 会话是否已经在运行；重复会话可能被跳过，Windows 当前不能聚焦原标签。
- 是否卡在 Trust、首次 bypass 或其他 CLI 交互提示。

错误若显示“只核实到 0/N”，表示新 TTY 上没有同时匹配 Provider 身份、完整恢复参数和 session ID。成功启动要求完整集合连续稳定匹配 1000 ms；立即退出或 Trust 未确认不能算成功。生产代码不会因此自动关闭已创建对象。

## 3. macOS 报 Terminal 自动化失败或 `-1719`

`-1719` 曾与辅助功能拒绝相关。依次检查：

1. 是否授权 **Claude Session Manager 本身**，不是只授权 `osascript`。
2. “自动化 → Terminal”和“辅助功能”是否分别允许。
3. 替换 App 后 CDHash 是否改变，旧 TCC 记录是否仍绑定旧构建。
4. 授权后是否完全退出并重开 App。
5. 查看脱敏后的日志：

   ```bash
   tail -n 100 "$HOME/Library/Logs/Claude Session Manager/launch.log"
   ```

只有确认旧授权失效后，才由用户按 [安装与授权](INSTALL.md#重编译或替换-app-后权限失效) 决定是否定向重置本 App 的权限。不要直接编辑 TCC 数据库，不要全局重置权限。

## 4. macOS 提示“不受信任”“无法打开”或“已损坏”

这发生在 App 启动前，与 Terminal 自动化权限不同：

- 确认文件来自项目所有者的 Release 或明确交付目录，并核对版本、架构和 strict codesign。
- 当前是 ad hoc、未公证构建，严格完整性通过不等于 Gatekeeper 必然接受。
- 先正常启动一次，再检查“系统设置 → 隐私与安全性 → 安全性”是否出现本 App 的“仍要打开”。
- 当前构建不需要在钥匙串中添加“信任开发者”或证书信任。
- 来源不明、提示已损坏或签名结构验证失败时，不要运行。
- 不要全局关闭 Gatekeeper、SIP 或 SmartScreen，不要添加不明证书。
- 完整处理步骤见 [INSTALL.md](INSTALL.md#23-首次启动完成单次-gatekeeper-确认)。

## 5. 批量只看到部分标签或中途失败

- 读取错误中的 `stage`，区分预检、创建窗口、创建第 N 个标签和启动第 N 个会话。
- macOS 使用 Cmd+T 创建后续标签；快捷键冲突、系统 UI 变化和未覆盖的 locale 仍可能影响结果。
- Windows 通过 `wt.exe -w new` 和多个 `new-tab` 创建新窗口；确认 `wt.exe` 应用执行别名可用。
- 生产路径不包含失败自动清理。先辨认本次新对象，再决定如何处理。
- 不要通过“关闭所有 Terminal 窗口”补救，也不要按进程名批量结束 CLI。
- macOS AppleScript 的多个 window ID 不一定等于多个可见原生窗口；严格结构需看 `AXTabGroup` / `AXRadioButton`。

## 6. Claude 已打开后 Codex 被误判为已开启

预期行为是不同 Provider、不同 session 独立判断：

- Claude 运行时，停止的 Codex 应能在自己的新窗口启动。
- 收藏批量只查询当前 Provider，不应混入另一类收藏。
- 不相交批次应继续创建新窗口，不限制每个 Provider 只能有一个窗口。

如果仍出现跨 Provider 阻塞，记录 Provider、所选数量、返回的 `opened/already_running` 和脱敏日志，不要用关闭另一 Provider 的方式绕过。

## 7. Codex 无标题、无消息数量或状态不完整

检查：

```text
~/.codex/state_5.sqlite
~/.codex/session_index.jsonl
~/.codex/sessions/YYYY/MM/DD/rollout-...-<uuid>.jsonl
```

应用使用内嵌 SQLite 只读打开状态库，不需要安装或配置 `sqlite3` 命令。数据库结构变化或权限错误应保留错误日志并添加脱敏 fixture，不能把读取失败当成零消息。

## 8. Codex 标签已关闭，却仍提示“已经开启”

Windows 有两层检查：

1. 优先检查真正的交互恢复进程，要求 Provider、完整参数和 session ID 匹配。
2. 再通过 Restart Manager 只读查询 rollout 持有者。

只有 `threads.source=cli` 且持有者 exe 路径含精确 `app-server-daemon` 组件时，该后台句柄才不会单独阻止重开。`source=vscode`、未知来源、普通 Codex exe 或 exe 路径不可得时仍保守拦截。

不要删除进程检测或 rollout 保护来绕过提示。若仍复现，记录线程来源、持有者路径和是否仍有精确恢复进程，不公开真实正文。

## 9. 更新面板没有提示或检查失败

| 现象 | 检查与处理 |
| --- | --- |
| 点击检查提示先填写仓库 | 输入公开仓库 `owner/repository`，不要粘贴 URL 或凭据 |
| 返回 404 / 未找到稳定 Release | 确认仓库公开，已有非草稿、非预发布的 latest Release |
| 标签不是语义化版本 | 使用 `v1.2.3` 或 `1.2.3`，不要使用只有日期或 `stable` 的标签 |
| 已是最新版本 | Release 版本小于或等于当前运行版本，不显示升级横幅 |
| 忽略后不再弹出 | 这是预期；手动检查仍可查看，更高版本仍会提醒 |
| GitHub 限流或拒绝 | 稍后重试；当前不支持通过面板加入 Token |
| 网络超时 | 检查网络后手动重试；后台失败不影响会话功能 |
| 打开发布页失败 | 核对 Release 是否属于当前仓库；不要手工粘贴外部下载地址到应用 |

当前只检查、提醒并打开发布页，不会自动下载或替换应用。完整规则见 [UPDATES.md](UPDATES.md)。

## 10. 缓存疑似过期

先完全退出 App，再只处理本应用缓存文件：

```text
macOS：~/Library/Caches/com.claude.sessionmanager/session-metadata-v1.json
Windows：%LOCALAPPDATA%\com.claude.sessionmanager\session-metadata-v1.json
```

也可用 `CSM_CACHE_DIR` 指向新的隔离缓存目录做对照。不要删除整个 Claude/Codex 数据目录。缓存可重建，但大型文件第一次重新解析会有 I/O 成本。

## 11. 导出失败或内容与预期不同

- 导出是建立时固定快照中可解析消息的 Markdown，不是原始 JSONL 的逐字节副本。
- 后续新消息不属于已建立的旧导出快照。
- 长导出会完整扫描源文件，耗时与文件大小、磁盘速度有关。
- 检查 Documents 下 `Claude Session Manager Exports` 是否可写。
- 文件名重名会追加序号，不直接覆盖。
- 若返回 `cleanup_warning`，表示导出结果与临时清理状态需要分别查看，不能当作完全成功。

## 12. 当前仍未覆盖或明确限制

- 官方账号的完整 Claude/Codex TUI 历史与正文恢复未被合成 Provider 测试替代。
- macOS 6.5 秒负向错误 toast 未完成视觉验收。
- macOS 桌面/iCloud 副本存在 strict-codesign detritus；从 `/Applications` 使用。
- macOS 恢复 IPC 仍为同步主线程路径，自动化期间可能阻塞 UI。
- Windows x64、MSI、Explorer、自启动真实切换、原生标签聚焦和真实 `.ps1` Provider 完整恢复未覆盖。
- Windows 已运行会话会阻止重复，但不能聚焦原标签。
- 当前没有 Developer ID / Apple 公证或 Authenticode 发布流程。
- GitHub 更新仍需真实公开仓库的只读验收；mock 不能证明某个未来仓库配置正确。
- 首轮源码 CI 已通过，但后续提交仍需查看对应 Actions 结果，CI 不代替真实 GUI/Terminal 验收。
- 项目自身 LICENSE 尚未补齐。
- 任何测试通过都不等于“已经没有 bug”。
