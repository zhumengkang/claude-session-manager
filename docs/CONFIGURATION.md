# 配置、数据目录、自动发现与隐私

[文档中心](README.md) · [安装与授权](INSTALL.md) · [使用指南](USAGE.md) · [GitHub 更新](UPDATES.md) · [故障排查](TROUBLESHOOTING.md)

> 默认情况下无需配置。应用按当前操作系统用户解析目录，不写死用户名；只有自定义 CLI 数据目录、CLI/Node 位置或诊断输出时才需要环境变量。

## 1. 默认读取的数据

| 数据 | 默认位置 |
| --- | --- |
| Claude 会话主记录 | `~/.claude/projects/<project-slug>/<session-uuid>.jsonl` |
| Claude 自定义名称与运行记录 | `~/.claude/sessions/*.json` |
| Codex rollout | `~/.codex/sessions/YYYY/MM/DD/rollout-...-<uuid>.jsonl` |
| Codex SQLite 状态库 | `~/.codex/state_5.sqlite` |
| Codex 会话索引 | `~/.codex/session_index.jsonl` |

- `~` 由应用按当前用户动态解析；Windows 对应 `%USERPROFILE%`。
- 应用扫描已发现的所有项目目录，不局限于管理器启动目录。
- `CLAUDE_CONFIG_DIR` 和 `CODEX_HOME` 优先于默认位置。
- 管理器不会直接改写 Claude/Codex 的 JSONL 主记录；恢复后由对应 CLI 按自己的行为更新。
- CLI 存储格式可能随版本变化，缺字段、移动文件或新格式可能影响列表与详情。

Claude `projects` 的直接子目录符号链接可作为显式项目入口；列表、详情、导出和打开使用同一规范化根边界。项目内部文件链接、嵌套目录链接或根目录之外的路径不能借此逃逸。

当前字符串 IPC 不能无损表达任意非 UTF-8 Unix 路径。此类记录会被列表与统计一致跳过，不会显示成“看得见但无法打开”的条目；这不等于已经支持任意字节路径。

## 2. 应用自己写入的位置

| 内容 | macOS | Windows |
| --- | --- | --- |
| 收藏 | `<CLAUDE_CONFIG_DIR>/favorites.json`，默认 `~/.claude/favorites.json` | 同一规则，默认 `%USERPROFILE%\.claude\favorites.json` |
| 元数据缓存 | `~/Library/Caches/com.claude.sessionmanager/session-metadata-v1.json` | `%LOCALAPPDATA%\com.claude.sessionmanager\session-metadata-v1.json` |
| 更新设置 | `~/Library/Application Support/com.claude.sessionmanager/update-settings.json` | `%APPDATA%\com.claude.sessionmanager\update-settings.json` |
| 诊断日志 | `~/Library/Logs/Claude Session Manager/launch.log` | `%LOCALAPPDATA%\Claude Session Manager\Logs\launch.log` |
| 登录自启动 | `~/Library/LaunchAgents/com.claude.sessionmanager.plist` | 当前用户 HKCU Run，不是文件 |
| Markdown 导出 | 系统“文稿”下 `Claude Session Manager Exports` | 系统 Documents 下同名目录 |

Claude 与 Codex 收藏保存在同一文件中；Codex 键带 `codex:` 前缀，避免跨 Provider 的 ID 冲突。更新设置只保存公开仓库名、自动检查开关和忽略版本，不保存 GitHub Token、Cookie 或会话正文。

## 3. 环境变量

| 变量 | 用途 | 规则 |
| --- | --- | --- |
| `CLAUDE_CONFIG_DIR` | Claude 数据根目录，也决定收藏位置 | 默认 `~/.claude` |
| `CODEX_HOME` | Codex 数据根目录 | 默认 `~/.codex` |
| `CSM_CLAUDE_CLI` | 显式指定 Claude CLI | 必须是可执行文件的绝对路径；未设置时自动发现 |
| `CSM_CODEX_CLI` | 显式指定 Codex CLI | 同上，不会与 Claude 候选互换 |
| `CSM_NODE_CLI` | 指定恢复子进程使用的 Node | 必须是绝对路径；自动发现时会保留原 PATH |
| `CSM_CACHE_DIR` | 覆盖缓存**目录** | 应用在其中创建 `session-metadata-v1.json` |
| `CSM_DEBUG_LOG` | 覆盖诊断日志的**完整文件路径** | 不是日志目录 |
| `CSM_CHROME_PATH` | 仅前端 mock 测试 | Chrome 可执行文件路径 |
| `CSM_CDP_PORT` | 仅前端 mock 测试 | 未指定时自动选择空闲 CDP 端口 |

自定义配置目录若使用相对路径，会先基于管理器启动时的当前目录转为绝对路径，再同时用于列表和恢复。非 Unicode 的 CLI、配置目录或恢复 PATH 会在启动前明确报错，不使用有损转换继续执行。

### 双击 App 与 shell 环境不同

macOS 双击启动不一定读取 `.zshrc`。确需覆盖时，可完全退出 App 后显式启动其可执行文件：

```bash
CLAUDE_CONFIG_DIR="$HOME/.config/claude" \
CODEX_HOME="$HOME/.config/codex" \
CSM_CACHE_DIR="$HOME/Library/Caches/com.claude.sessionmanager" \
CSM_DEBUG_LOG="$HOME/Library/Logs/Claude Session Manager/launch.log" \
"/Applications/Claude Session Manager.app/Contents/MacOS/csm-app"
```

以上目录只是示例。无需自定义时不要设置；不需要把示例路径原样复制到其他机器。

Windows 应在启动管理器的同一用户环境中设置变量，修改后重启旧管理器。恢复标签通过 `--inheritEnvironment` 继承管理器进程环境。

## 4. CLI 与 Node 自动发现

未提供 `CSM_*_CLI` 时，应用会：

1. 按当前 PATH 搜索对应 Provider。
2. 检查当前用户的常见安装位置与版本管理器目录。
3. macOS 检查文件执行权限；Windows 按 PATHEXT 识别 `.exe`、`.cmd`、`.bat`、`.ps1` 等形态。
4. 保留包含空格和 Unicode 的路径，不依赖控制台文本输出解析路径。
5. 为需要 Node 的 CLI 准备运行 PATH，不无理由丢弃原 PATH。

终端中的 shell 别名、函数或仅写在交互式 shell 配置里的 PATH，不一定对 GUI 进程可见。找不到时使用相应 `CSM_*_CLI` 绝对路径覆盖，不要填仓库地址、命令文本或第三方 API 基址。

恢复时会把非默认 `CLAUDE_CONFIG_DIR` / `CODEX_HOME` 传给 Terminal 中的 CLI，避免列表读取一套配置而恢复使用另一套。原项目目录仍会单独校验。

## 5. 缓存与固定快照

- 缓存结合纳秒修改时间、长度、平台变更标识和完整内容 BLAKE3；元数据相同也不能单独证明内容未变。
- 文件增长时先校验旧前缀；只有旧前缀一致才增量解析后缀，否则从头重建。
- 缓存保存使用唯一临时文件、独占创建、完整写入与原子替换，避免并发写入共用临时路径。
- 缓存保存元数据，不持久化消息正文。
- 详情首页固定源文件长度与完整内容指纹；向上翻页继续携带同一快照令牌。
- 后端反向分块读取，每块 256 KiB；单页最多 1000 条消息，页内按从旧到新显示。
- 后续追加不会移动旧分页基准；中部改写会拒绝复用旧快照或缓存。

完整校验提高了一致性，但大型活跃文件仍有明显 I/O 成本。详细验证边界见 [VALIDATION.md](VALIDATION.md)。

## 6. 更新设置

左侧“更新”面板可配置公开 GitHub 仓库和自动检查。无仓库配置时不会发起更新请求；更换仓库会清除旧仓库的忽略版本。损坏或格式不受支持的设置文件会返回错误，不会被静默覆盖。

检查地址固定为 GitHub 官方 API，发布页也限定为当前仓库的 `github.com/.../releases/tag/` 地址。配置项不包含 Token、私有仓库凭据或自定义 Enterprise 基址。

操作与故障排查见 [UPDATES.md](UPDATES.md)。

## 7. 网络与隐私边界

- 除用户配置的 GitHub 更新检查外，管理器没有远程服务后端；UI 不加载运行时 CDN。
- GitHub 更新只读取公开 Release 元数据，不上传会话、收藏、项目路径、Token 或本机身份信息；GitHub 仍可按其服务策略看到常规 HTTPS 请求与源 IP。
- 恢复后的 Claude Code / Codex 按各自配置联网，管理器不改变它们的模型、认证或网络行为。
- 收藏和缓存可能含 Session ID、标题、项目路径；日志还可能含错误文本、Terminal 对象 ID 或 TTY。分享前必须脱敏。
- macOS 单开会在本次新 Terminal 标签中识别固定 Trust 提示文本，因此不能描述为“从不读取 Terminal 内容”。这不等于读取所有既有终端内容。
- 不要公开提交真实 JSONL、rollout、`state_5.sqlite`、认证文件、令牌、Cookie 或完整会话正文。
- 当前 Tauri 配置为 `csp: null`，并启用 `dangerousDisableAssetCspModification`；开源发布前应收紧 CSP 并复测，不能宣称已有严格 CSP。

## 8. 修改配置后的最小检查

1. 完全退出并重新打开管理器。
2. 核对两个 Provider 的列表数量与项目路径。
3. 打开一条详情，确认不是空数据被误当成功。
4. 用专门准备的可信测试会话单开，再验证双标签批量。
5. 如修改更新仓库，点击“保存并检查更新”，核对当前仓库与 Release 地址。
6. 失败时只分享脱敏日志和复现步骤，不上传整个配置或会话目录。
