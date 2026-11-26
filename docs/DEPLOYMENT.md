# 部署、更新、自启动与卸载

[文档中心](README.md) · [二进制安装与授权](INSTALL.md) · [使用指南](USAGE.md) · [GitHub 更新](UPDATES.md) · [源码编译](BUILD.md) · [配置与隐私](CONFIGURATION.md)

> 本文面向已经安装二进制、需要更新/迁移/卸载的用户，以及负责交付产物的人。首次安装优先阅读 [INSTALL.md](INSTALL.md)。

## 1. 部署入口

| 任务 | macOS Apple Silicon | Windows ARM64 |
| --- | --- | --- |
| 推荐安装形态 | 完整 `.app` / DMG | 当前用户 NSIS |
| 推荐位置 | `/Applications/Claude Session Manager.app` | `%LOCALAPPDATA%\Claude Session Manager\csm-app.exe` |
| 更新 | 退出 App 后完整替换应用包 | 退出管理器后运行已核验的新 NSIS |
| 自启动 | 当前用户 LaunchAgent | 当前用户 HKCU Run |
| 卸载 | 移除 App，按需清理自有缓存/日志/权限 | 使用安装器对应卸载入口，按需清理自有缓存/日志 |
| 登录后行为 | 只启动管理器 | 只启动管理器 |

当前不会登录后自动批量恢复收藏，也不会自动创建 Terminal 窗口。

### 应用内 GitHub 更新提醒

左侧“更新”面板用于检查公开 GitHub latest stable Release。配置 `owner/repository` 后可手动检查，也可启用启动时与运行期间每 6 小时的自动检查。更高稳定版本显示横幅，支持打开发布页、忽略版本和稍后提醒。

**提醒与安装是两个阶段：** 当前不会自动下载、替换、安装或重启。看到提示后仍需核验当次发布清单，再按下方 macOS/Windows 步骤更新。无仓库时不发起请求，不保存 GitHub 凭据。详见 [UPDATES.md](UPDATES.md)。

旧交付二进制不包含该新源码功能；只有构建、发布并安装包含该功能的新版本后，更新面板才会出现在实际安装版中。

## 2. macOS 更新

1. 核验新 DMG 或完整 `.app` 的来源与 SHA-256。
2. 完全退出旧管理器。
3. 用 Finder 将新 App 完整替换到 `/Applications`；不要只覆盖 `Contents/MacOS/csm-app`。
4. 如果是刚从源码构建，可用 `ditto` 保留应用包结构：

   ```bash
   ditto \
     "src-tauri/target/release/bundle/macos/Claude Session Manager.app" \
     "/Applications/Claude Session Manager.app"
   ```

5. 验证并启动：

   ```bash
   codesign --verify --deep --strict --verbose=2 \
     "/Applications/Claude Session Manager.app"
   open "/Applications/Claude Session Manager.app"
   ```

6. 重新编译或替换后 CDHash 通常变化。若 Terminal 权限被拒绝，按 [INSTALL.md](INSTALL.md#重编译或替换-app-后权限失效) 核对当前 CDHash 和 TCC，不要只看开关外观。
7. 如果启用过自启动，确认 LaunchAgent 指向当前 `current_exe`；必要时在新 App 中关闭再开启自启动。

不要对正在运行的应用包做不完整覆盖，也不要运行来源不明的旧 App 备份。

## 3. Windows 更新

1. 核验新 `Claude Session Manager_0.1.0_arm64-setup.exe`。
2. 关闭旧管理器窗口；不要顺带结束真实 Codex daemon、Claude、Windows Terminal 或其他无关进程。
3. 以安装旧版的同一 Windows 用户运行新 NSIS。
4. 安装完成后从开始菜单启动。
5. 在 PowerShell 中核对实际安装文件：

   ```powershell
   $app = "$env:LOCALAPPDATA\Claude Session Manager\csm-app.exe"
   Get-Item $app | Select-Object FullName, Length
   Get-FileHash -Algorithm SHA256 $app
   ```

6. 检查 Provider 列表和详情，再用隔离测试会话验证恢复。

当前 e NSIS 的更新、卸载、干净重装均已实际执行并退出 0，最终安装路径 GUI 也已通过。但这只覆盖 Windows 11 ARM64 当前验收环境，不扩展为 x64、MSI 或所有 Windows 环境的保证。

### Release 与 NSIS 内层载荷

交付中的独立 Release 和 NSIS 内层 `csm-app.exe` 可能不是完全相同的 SHA-256。当前构建只有既定的 `UNK` → `NSS` 三字节包标记差异，规范化后逐字节一致。核对安装文件时应使用 `manifest.json` 的 `installed_payload_sha256` 或对应安装证据，不要错误地要求它等于独立 Release 哈希。

## 4. 登录自启动

### macOS

UI 开关创建：

```text
~/Library/LaunchAgents/com.claude.sessionmanager.plist
```

其中 `ProgramArguments` 应指向当前运行的 App 内可执行文件，并由 `launchctl` 加载。只读检查：

```bash
plutil -p "$HOME/Library/LaunchAgents/com.claude.sessionmanager.plist"
```

移动或替换 App 后，如果仍指向旧路径，在新 App 中关闭再开启自启动。当前自启动真实系统切换未在本轮重新验收，不应把 plist 存在单独当作登录行为已经通过。

### Windows

UI 开关写入：

```text
HKCU\Software\Microsoft\Windows\CurrentVersion\Run
值名：ClaudeSessionManager
```

只读检查：

```powershell
reg query "HKCU\Software\Microsoft\Windows\CurrentVersion\Run" /v ClaudeSessionManager
```

应通过应用开关管理该值。Windows 自启动真实切换尚未实机验收。

## 5. macOS 卸载

1. 退出 App。
2. 优先在 UI 中关闭自启动。
3. 将 `/Applications/Claude Session Manager.app` 移到废纸篓。
4. 按需处理该 App 自己的 LaunchAgent、缓存和日志：

   ```text
   ~/Library/LaunchAgents/com.claude.sessionmanager.plist
   ~/Library/Caches/com.claude.sessionmanager/
   ~/Library/Application Support/com.claude.sessionmanager/update-settings.json
   ~/Library/Logs/Claude Session Manager/
   ```

5. 在系统设置中移除不再需要的 Terminal 自动化和辅助功能授权。
6. 收藏文件如有价值，先备份再决定是否处理。

**不要为了卸载本应用删除整个 `~/.claude` 或 `~/.codex`。** 它们包含 Claude Code / Codex 自己的配置、认证和会话，不是本应用专属数据。

## 6. Windows 卸载

1. 退出管理器。
2. 如需关闭自启动，优先在 UI 中关闭。
3. 通过 Windows“已安装的应用”或该 NSIS 安装对应的卸载入口卸载。
4. 卸载后按需检查：

   ```text
   %LOCALAPPDATA%\Claude Session Manager\
   %LOCALAPPDATA%\com.claude.sessionmanager\
   %APPDATA%\com.claude.sessionmanager\update-settings.json
   HKCU\Software\Microsoft\Windows\CurrentVersion\Run\ClaudeSessionManager
   ```

5. 收藏、导出和 CLI 数据按需单独备份；不要删除整个 `%USERPROFILE%\.claude` 或 `%USERPROFILE%\.codex`。

当前已验收卸载只表示程序文件与卸载注册项按证据消失，不代表应该自动删除用户收藏、真实会话或其他 Provider 进程。

## 7. 数据保留对照

| 内容 | 默认位置 | 卸载时建议 |
| --- | --- | --- |
| Claude/Codex 主会话 | `~/.claude`、`~/.codex`；Windows 为 `%USERPROFILE%` 下对应目录 | **保留，不属于本应用专属数据** |
| 收藏 | `<CLAUDE_CONFIG_DIR>/favorites.json`，默认 `~/.claude/favorites.json` | 有需要先备份 |
| macOS 缓存 | `~/Library/Caches/com.claude.sessionmanager/session-metadata-v1.json` | 可重建，可按需删除 |
| Windows 缓存 | `%LOCALAPPDATA%\com.claude.sessionmanager\session-metadata-v1.json` | 可重建，可按需删除 |
| macOS 更新设置 | `~/Library/Application Support/com.claude.sessionmanager/update-settings.json` | 只含公开仓库与提醒偏好，可按需删除 |
| Windows 更新设置 | `%APPDATA%\com.claude.sessionmanager\update-settings.json` | 只含公开仓库与提醒偏好，可按需删除 |
| macOS 日志 | `~/Library/Logs/Claude Session Manager/launch.log` | 分享前脱敏，可按需删除 |
| Windows 日志 | `%LOCALAPPDATA%\Claude Session Manager\Logs\launch.log` | 分享前脱敏，可按需删除 |
| Markdown 导出 | Documents 下 `Claude Session Manager Exports` | 用户成果，默认保留 |

## 8. 回滚与迁移

- 只回滚到来源明确、哈希可核验且与所需功能兼容的产物。
- 回滚仍需完全退出 App、完整替换并重新检查 CDHash/TCC；不要运行旧危险自动化脚本或不明 App 备份。
- 迁移到另一台机器不等于复制认证目录。Claude/Codex 应按各自官方流程安装和登录。
- 完全自定义数据位置需要显式配置；不要写死旧机器用户名。
- 测试或部署失败时，不通过关闭全部 Terminal、结束所有 Provider 或删除真实会话目录清理。

## 9. 交付部署复核

负责交付时至少核对：

1. 四个主要产品文件的名称、架构、大小和 SHA-256。
2. macOS `/Applications` strict codesign 与 DMG `hdiutil verify`。
3. Windows 安装后文件、当前用户卸载注册项及安装路径进程身份。
4. 文档中的产物名、版本、哈希和已知限制与 `manifest.json` 一致。
5. 文档刷新不改变二进制；重新构建则必须重新生成哈希并重新检查授权。
6. 根级 `SHA256SUMS.txt` 在实际交付副本上重新校验，而不是只验证打包前的源文件。

当前测试证据及未覆盖项见 [VALIDATION.md](VALIDATION.md)。
