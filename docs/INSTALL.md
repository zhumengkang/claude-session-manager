# 二进制安装与授权

[文档中心](README.md) · [返回首页](../README.md) · 下一步：[使用指南](USAGE.md) · [GitHub 更新](UPDATES.md) · 后续：[部署/更新/卸载](DEPLOYMENT.md) · 开发者：[源码编译](BUILD.md)

> 安装顺序：**取得安装文件 → 安装 → 完成系统授权 → 用可信测试会话验证。** 使用二进制不需要 Rust、Cargo 或 Tauri CLI。

## 1. 安装前确认

当前版本：`0.1.0`；Bundle ID：`com.claude.sessionmanager`。

| 平台 | 安装文件 | 当前验收状态 |
| --- | --- | --- |
| macOS Apple Silicon | `Claude Session Manager_0.1.0_aarch64.dmg` | `/Applications` 严格签名、DMG 和安装版合成恢复已有验收记录 |
| macOS Apple Silicon | 完整 `Claude Session Manager.app` | 必须保留整个 `.app`，不要只复制 `Contents/MacOS/csm-app` |
| Windows 11 ARM64 | `Claude Session Manager_0.1.0_arm64-setup.exe` | 当前用户 NSIS 更新、卸载、干净重装及安装路径 GUI 已有验收记录 |
| Windows 11 ARM64 | `csm-app-windows-arm64.exe` | 独立可执行文件，不含安装器和卸载器 |

当前没有已验收的 macOS Intel/Universal 或 Windows x64 安装包。不要把 ARM64 文件当作通用包。

只从项目所有者的 GitHub Release 或明确交付目录取得文件，确认平台、架构、版本和文件名。不要运行网盘转载、来源不明的“修复版”、证书修复包或他人转发的旧备份。

### 运行条件

- Claude Code 和/或 Codex CLI 已按各自官方流程安装、初始化并登录。
- macOS：使用系统自带 Terminal。
- Windows：需要 Windows Terminal、WebView2 Runtime 和 Windows PowerShell `powershell.exe`。
- CLI 依赖 Node 时，应用会尝试自动发现；自定义位置可用 `CSM_NODE_CLI` 指定绝对路径。
- 二进制使用不需要单独安装 Rust；Codex SQLite 读取不需要安装 `sqlite3` 命令。
- 使用与 CLI 相同的用户账户运行管理器，不需要 root、`sudo` 或管理员 PowerShell。

应用不写死用户名或某台机器的 CLI 路径。自动发现和覆盖规则见 [CONFIGURATION.md](CONFIGURATION.md)。

---

## 2. macOS 安装 DMG

### 2.1 放入应用程序目录

1. 完全退出旧版 Claude Session Manager。
2. 双击 DMG 挂载。
3. 将整个 **Claude Session Manager.app** 拖入 `Applications`。
4. 如果提示替换，先确认旧管理器已经退出，再替换完整应用包。
5. 推出 DMG，不要长期从 DMG 内运行。
6. 在 Finder 中打开 `/Applications`，找到 **Claude Session Manager**。

如果拿到的是完整 `.app`，同样复制整个应用包。不要只复制其中的 Unix 可执行文件，也不要运行历史备份里的 App。

可在 Terminal 检查安装副本的结构和签名完整性：

```bash
codesign --verify --deep --strict --verbose=2 \
  "/Applications/Claude Session Manager.app"
```

无错误输出且退出码为 0，表示应用包内部签名结构完整。它**不等于 Apple 已公证，也不等于 Gatekeeper 一定接受**。

### 2.2 当前签名与“信任开发者”的区别

当前构建使用 **ad hoc 签名**，不是 Developer ID，也没有 Apple 公证。因此需要区分三件事：

| 提示或操作 | 当前构建如何处理 |
| --- | --- |
| “无法验证开发者” | 这是 Gatekeeper 首次启动提示，不是 Terminal 权限问题 |
| “Apple 无法检查是否包含恶意软件” | 先完成下方单次 App 启动确认，不要添加证书信任 |
| “仍要打开” / “Open Anyway” | 针对这一份 App 的启动确认，不是信任所有未签名应用 |
| “信任此开发者”或钥匙串证书信任 | **当前 ad hoc 构建不需要，也不应通过添加证书信任解决** |
| Developer ID + Apple 公证 | 属于另一发布方式；当前构建不能声称已经具备 |

Developer ID 签名和公证由发布者在构建、签名和发布阶段完成，不是用户在钥匙串里手动打开“始终信任”。企业内部证书或描述文件又是另一套流程，不适用于这份 ad hoc 构建。

### 2.3 首次启动：完成单次 Gatekeeper 确认

现代 macOS 的主要操作如下，菜单文字可能随系统语言略有不同：

1. **先正常启动一次 App。** 如果系统拦截并显示提示，先关闭提示窗口。
2. 打开：

   ```text
   Apple 菜单  → 系统设置 → 隐私与安全性
   ```

3. 向下滚动到 **“安全性”** 区域，查找类似以下文字：

   ```text
   “Claude Session Manager”已被阻止，以保护你的 Mac。
   ```

4. 确认提示对象确实是 `/Applications/Claude Session Manager.app`，并且安装文件来源明确。
5. 点击 **“仍要打开”** 或 **“Open Anyway”**。系统要求时，使用 Touch ID 或当前用户密码确认。
6. 在后续确认窗口中点击 **“打开”**。
7. 回到 `/Applications` 再次启动 App。

如果“安全性”区域没有该条目：

- 在 Finder 中按住 Control 点击 App，选择“打开”，阅读实际提示，然后再检查“隐私与安全性”。
- 只有系统明确提供“打开”选项、且你已经确认来源时才继续；不同 macOS 版本提供的入口可能不同。
- 确认操作的是 `/Applications` 中的新副本，不是 DMG、桌面副本或旧备份。
- 如果提示“已损坏”、应用包结构不完整或 `codesign --verify` 失败，停止启动，重新取得完整 App 或从当前源码构建。

**不要执行以下操作：**

- 不要全局关闭 Gatekeeper，不要使用 `spctl --master-disable`。
- 不要关闭 SIP，不要开启“任何来源”作为常规解决方案。
- 不要在钥匙串中把不明证书改为“始终信任”。
- 不要为了绕过提示而删除整个应用的安全属性或运行他人提供的清理脚本。

### 2.4 权限一：Terminal 自动化 / Apple Events

点击“打开会话”或“单窗多标签”时，应用需要让 Terminal 创建本次窗口和标签。

首次操作时，macOS 可能显示：

```text
“Claude Session Manager”想要控制“Terminal”。
```

确认是本次恢复操作触发后，点击 **“允许”**。手动检查位置：

```text
系统设置 → 隐私与安全性 → 自动化 → Claude Session Manager → Terminal
```

处理规则：

- Terminal 开关应允许，授权对象是 **Claude Session Manager 控制 Terminal**。
- 如果之前点过“不允许”，在这里打开 Terminal 开关，然后完全退出并重开管理器。
- 不要授予其他来源不明的 App 相同权限。
- 此权限不代表允许管理器关闭你的其他窗口；生产代码仍不得批量关闭 Terminal。

### 2.5 权限二：辅助功能 / Accessibility

读取原生标签结构和完成批量恢复需要辅助功能权限。

1. 打开：

   ```text
   系统设置 → 隐私与安全性 → 辅助功能
   ```

2. 解锁设置（如果系统要求）。
3. 如果列表已有 **Claude Session Manager**，确认它指向当前 `/Applications` 副本并打开开关。
4. 如果没有条目，点击 **“+”**，从 `/Applications` 选择 **Claude Session Manager.app**，再打开开关。
5. 完全退出并重新打开管理器。

授权对象必须是 **App 本身**，不是只授权 `osascript`、Terminal、Claude Code 或 Codex。当前版本会在创建 Terminal 对象前进行 Accessibility 预检；未授权时应直接报错，而不是先创建空白窗口。

### 2.6 文件与文件夹访问

如果项目位于桌面、文稿、下载或外置磁盘，macOS 可能另外询问文件夹访问权限。只开放实际项目所需的位置。

- 不需要为了读取所有会话直接开启“完全磁盘访问”。
- 不要为了安装本应用修改 `~/.claude` 或 `~/.codex` 的所有者。
- 不要复制其他机器的认证目录来代替本机登录。

### 重编译或替换 App 后权限失效

重新编译、替换或移动 App 后，签名身份可能变化。系统设置里的开关仍打开，不代表旧授权一定认可新副本。

按顺序处理：

1. 完全退出管理器。
2. 确认 `/Applications` 中只有当前要使用的完整 App，旧备份不要继续运行。
3. 在“辅助功能”中选择旧条目，点击 **“−”** 移除，再用 **“+”** 添加当前 App 并开启。
4. 在“自动化 → Claude Session Manager”中检查 Terminal；条目异常时，重新触发一次恢复并按新提示授权。
5. 完全退出并重开管理器，再用隔离测试会话验证。

只有界面操作后仍明确出现本 App 的权限拒绝时，才由你决定是否单独重置该 Bundle ID 的对应权限：

```bash
tccutil reset Accessibility com.claude.sessionmanager
```

Terminal 自动化也需要重置时，单独执行：

```bash
tccutil reset AppleEvents com.claude.sessionmanager
```

这两条命令会撤销该 Bundle ID 已有的对应授权，下次操作需要重新批准。它们不能解决 Gatekeeper 启动提示，也不应作为每次启动前的固定步骤。不要直接编辑 TCC 数据库，不要做全局权限重置。

![macOS 安装、权限与重编译授权流程](images/install-flow.svg)

---

## 3. Windows 11 ARM64 安装与首次启动

### 3.1 检查运行组件

先确认：

- Windows Terminal 已安装，`wt.exe` 应用执行别名可用。
- Microsoft Edge WebView2 Runtime 已安装。
- Windows PowerShell `powershell.exe` 可用；当前实现不是自动改用 `pwsh`。
- 原生 Windows Claude Code / Codex CLI 已在同一用户环境中安装并登录。

在普通 PowerShell 中进行只读检查：

```powershell
where.exe wt
where.exe powershell
where.exe claude
where.exe codex
where.exe node
claude --version
codex --version
```

缺少组件时：

- Windows Terminal：按照 Microsoft 官方说明安装，并在“设置 → 应用 → 高级应用设置 → 应用执行别名”中确认 `wt.exe` 已启用。官方入口：<https://learn.microsoft.com/windows/terminal/install>。
- WebView2：从 Microsoft 官方页面安装 Evergreen Runtime，选择与系统匹配的原生 ARM64 版本。安装后完全退出并重开管理器。官方入口：<https://developer.microsoft.com/microsoft-edge/webview2/>。
- CLI：按 Claude Code / Codex 各自官方流程安装和登录，不要把 WSL 中的 CLI 当作原生 Windows CLI。

### 3.2 使用 NSIS 安装

1. 从项目所有者的 Release 或明确交付目录取得 `Claude Session Manager_0.1.0_arm64-setup.exe`。
2. 关闭旧管理器窗口。不要顺带结束 Claude、Codex、Windows Terminal 或其他无关进程。
3. 使用安装旧版的同一 Windows 用户运行安装器。当前 NSIS 是 `currentUser` 安装，不需要管理员 PowerShell。
4. 安装完成后，从开始菜单打开 **Claude Session Manager**。
5. 默认安装位置：

   ```text
   %LOCALAPPDATA%\Claude Session Manager\csm-app.exe
   ```

6. 可在 PowerShell 确认实际安装路径和文件信息：

   ```powershell
   $app = "$env:LOCALAPPDATA\Claude Session Manager\csm-app.exe"
   Get-Item $app | Select-Object FullName, Length, LastWriteTime
   ```

具体用户目录由当前账户决定，不要写死其他机器的用户名，也不要为了读取另一个用户的会话而以管理员身份运行。

### 3.3 SmartScreen 与发布者提示

当前安装器没有 Authenticode 发布者签名和信誉记录，SmartScreen 可能显示“Windows 已保护你的电脑”或“未知发布者”。这不等同于系统已经确认文件恶意，也不等同于应该关闭安全功能。

只有安装文件来自上述明确来源、文件名和架构正确时，才由你决定是否进行单次启动确认：

1. 在 SmartScreen 窗口点击 **“更多信息”**。
2. 再次核对要运行的文件名确实是当前 NSIS 安装器。
3. 如果仍决定安装，点击 **“仍要运行”**。
4. 后续如果显示发布者信息，确认提示对象仍是同一个安装器。

注意：

- “仍要运行”是本次文件启动确认，不是信任所有未知发布者。
- 不要关闭 SmartScreen、Microsoft Defender 或防火墙。
- 不要添加 Defender 排除项，不要安装所谓证书修复包。
- 不要通过全局修改 PowerShell 执行策略解决安装问题。
- 来源无法确认、文件被安全软件明确判定为恶意，或安装器行为与说明不符时，停止运行。

需要查看当前安装文件的签名状态时，可在 PowerShell 执行：

```powershell
Get-AuthenticodeSignature $app | Select-Object Status, StatusMessage, SignerCertificate
```

当前未签名结果不能冒充受信任发布者签名。公开发布流程需要另行配置代码签名和时间戳。

### 3.4 Windows 权限与执行策略

Windows 没有 macOS 的“自动化 / 辅助功能”授权步骤。正常运行需要：

- 当前用户能够读取自己的 `%USERPROFILE%\.claude` 和 `%USERPROFILE%\.codex`。
- `wt.exe`、WebView2 和 `powershell.exe` 可用。
- Claude / Codex 能在同一用户环境中独立运行。

`.ps1` Provider 只在本次恢复的 PowerShell 进程内使用 `Process` 作用域执行策略，不需要执行全局 `Set-ExecutionPolicy Unrestricted`。真实 `.ps1` Provider 的完整恢复仍未验收。

### 3.5 独立可执行文件

也可直接运行 `csm-app-windows-arm64.exe`：

- 不创建开始菜单项或卸载注册项。
- 仍需要 WebView2、Windows Terminal、PowerShell 和相应 CLI。
- 仍使用当前用户的会话、收藏、缓存和日志目录。
- 更新时先退出旧进程，再完整替换为来源明确的新文件。
- SmartScreen 规则与安装器相同，不要把它当成绕过安全检查的入口。

### 3.6 更新与卸载

更新前先退出管理器，以同一 Windows 用户运行新 NSIS；不要结束真实 Provider 或终端进程。卸载通过“设置 → 应用 → 已安装的应用 → Claude Session Manager”完成。

完整步骤和收藏、缓存、日志的保留规则见 [DEPLOYMENT.md](DEPLOYMENT.md)。

---

## 4. 安装后的最小验证

1. 打开管理器，切换 Claude / Codex，确认列表数量和目录分组。
2. 打开一条会话详情，确认最新消息、Markdown 和“加载更早”。
3. 选择一个专门准备且可信的停止会话，执行单开。
4. 再选择两个停止会话，点击“单窗多标签”。
5. 确认本次批次只创建一个新 Terminal 窗口，并有所选数量的顶部标签。
6. 重复点击同一批，确认不会重复启动旧快照。

恢复使用的精确参数及安全影响见 [USAGE.md](USAGE.md#2-恢复命令与安全提示)。如果出现错误，按 [TROUBLESHOOTING.md](TROUBLESHOOTING.md) 的症状定位，不要通过关闭全部 Terminal 窗口或删除真实会话目录来“修复”。
