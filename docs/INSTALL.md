# 二进制安装与授权

[文档中心](README.md) · [返回首页](../README.md) · 下一步：[使用指南](USAGE.md) · [GitHub 更新](UPDATES.md) · 后续：[部署/更新/卸载](DEPLOYMENT.md) · 开发者：[源码编译](BUILD.md)

> 安装顺序：**取得安装文件 → 安装 → 完成系统授权 → 用可信测试会话验证。** 使用二进制不需要 Rust、Cargo 或 Tauri CLI。

## 0. 先选路线（30 秒）

| 你的情况 | 走哪条路 |
| --- | --- |
| 只想使用应用，不写代码 | 留在本文：macOS 看第 2 节，Windows 看第 3 节 |
| 想从源码自己编译 | 打开 [源码编译](BUILD.md)，按「从零开始」照着做 |
| 已装过旧版，只想升级 | 不用重读首次安装，直接看 [已有安装如何更新](DEPLOYMENT.md#已有安装如何更新到新版本) |

安装其实只有四步：**拿对文件 → 放进系统 → 过系统拦截 → 打开验证。** 第一次装请按下面的顺序做，不要跳步。

### 安装前 60 秒检查

- [ ] 系统是 **macOS Apple Silicon** 或 **Windows 11 ARM64**；当前没有已验收的 Intel Mac / Windows x64 安装包。
- [ ] 文件来自项目所有者的 GitHub Release 或明确交付目录，不是网盘转载或“修复版”。
- [ ] 文件名里的版本、架构与你的机器对得上。
- [ ] 旧版已经**完全退出**，不是只关了窗口。
- [ ] 准备一条**专门用来测试、当前已停止**的会话；不要拿正在使用的重要会话试恢复。

> **Release 页只有 `Source code` 压缩包时，说明该版本没有上传安装附件。** 不要把源码压缩包当成安装包，也不要从第三方转载站下载。可以等待附件上传，或按 [源码编译](BUILD.md) 自己构建；当前已验收的安装版本是 `0.1.0`。

![macOS 安装、权限与重编译授权流程](images/install-flow.svg)

## 1. 安装前确认

当前安装包版本：`0.1.0`（源码 main 为 `0.1.1`，仅版本号递增，没有新的安装包）；Bundle ID：`com.claude.sessionmanager`。

已经安装过旧版本？不需要重读本页的首次安装步骤，直接按 [DEPLOYMENT.md · 已有安装如何更新到新版本](DEPLOYMENT.md#已有安装如何更新到新版本) 替换程序。

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

### 2.0 先确认是 Apple Silicon

1. 点屏幕左上角 **Apple 菜单 → 关于本机**。
2. 看“芯片”一行：显示 **Apple M1 / M2 / M3 / M4…** 才是 Apple Silicon。
3. 也可以在 Terminal 执行：

   ```bash
   uname -m
   ```

   输出 `arm64` 表示 Apple Silicon；输出 `x86_64` 表示 Intel Mac，当前安装包不适用于这台机器。

### 2.1 放入应用程序目录

1. 从 Release 或交付目录把 `Claude Session Manager_0.1.0_aarch64.dmg` 下载到“下载”文件夹。不要直接在 DMG 压缩包里双击 App。
2. 双击 DMG，等待 Finder 打开一个带 App 图标和 `Applications` 快捷方式的小窗口。
3. **完全退出旧版**：如果旧版正在运行，按 `Command + Q`，确认菜单栏不再显示 Claude Session Manager。只点窗口左上角红点不算退出。
4. 把 **Claude Session Manager.app** 图标拖到窗口里的 `Applications` 快捷方式上，松手。
5. 如果系统问“是否替换”，先确认旧版已经退出，再选择替换。
6. 在 Finder 左侧点“推出”按钮，或右键桌面上的 DMG 选择“推出”。不要长期从 DMG 里运行 App。
7. 打开 `/Applications`，确认能看到完整的 **Claude Session Manager**。也可以执行：

   ```bash
   ls -ld "/Applications/Claude Session Manager.app"
   ```

   应输出一个目录，而不是“No such file or directory”。

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

第一次启动必须从 `/Applications` 里的 **Claude Session Manager** 启动，不要从 DMG、下载文件夹或旧备份启动。

现代 macOS 的主要操作如下，菜单文字可能随系统语言略有不同：

1. **先正常启动一次 App。** 如果系统拦截并显示“无法验证开发者”或“Apple 无法检查是否包含恶意软件”，先点提示窗口的“好”或“取消”关闭它，不要点“移到废纸篓”。
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
7. 回到 `/Applications` 再次启动 App。能正常看到白色主界面，就说明 Gatekeeper 这一步已完成；以后启动通常不会再拦截同一份 App。

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

先确认机器和组件：

1. 打开 **设置 → 系统 → 系统信息**，看“系统类型”是否为 **基于 ARM64 的处理器**。
2. 打开普通 PowerShell（不需要管理员），执行：

   ```powershell
   $env:PROCESSOR_ARCHITECTURE
   ```

   应输出 `ARM64`。当前安装包不适用于 Windows x64。
3. 逐项检查命令是否存在：

   ```powershell
   where.exe wt
   where.exe powershell
   where.exe claude
   where.exe codex
   where.exe node
   claude --version
   codex --version
   ```

   正常时 `where.exe` 会打印一个完整路径；如果某个命令没有任何输出，说明对应组件还没装好。

还需要：

- Windows Terminal 已安装，`wt.exe` 应用执行别名可用。
- Microsoft Edge WebView2 Runtime 已安装。Windows 11 通常已自带；如果应用启动后白屏或打不开，再补装。
- Windows PowerShell `powershell.exe` 可用；当前实现不是自动改用 `pwsh`。
- 原生 Windows Claude Code / Codex CLI 已在同一用户环境中安装并登录。不要把 WSL 里的 CLI 当成原生 Windows CLI。

缺少组件时：

- Windows Terminal：按照 Microsoft 官方说明安装，并在“设置 → 应用 → 高级应用设置 → 应用执行别名”中确认 `wt.exe` 已启用。官方入口：<https://learn.microsoft.com/windows/terminal/install>。
- WebView2：从 Microsoft 官方页面安装 Evergreen Runtime，选择与系统匹配的原生 ARM64 版本。安装后完全退出并重开管理器。官方入口：<https://developer.microsoft.com/microsoft-edge/webview2/>。
- CLI：按 Claude Code / Codex 各自官方流程安装和登录，不要把 WSL 中的 CLI 当作原生 Windows CLI。

### 3.2 使用 NSIS 安装

1. 从项目所有者的 Release 或明确交付目录取得 `Claude Session Manager_0.1.0_arm64-setup.exe`，放到“下载”文件夹。
2. **关闭旧管理器窗口**。如果右下角通知区域还有旧版图标，也右键退出。不要顺带结束 Claude、Codex、Windows Terminal 或其他无关进程。
3. 双击安装器。当前 NSIS 是 `currentUser` 安装，不需要管理员 PowerShell，也不要右键“以管理员身份运行”。
4. 如果 SmartScreen 出现提示，按 [3.3](#33-smartscreen-与发布者提示) 核对后再继续。
5. 安装向导一路点“下一步 / 安装”，完成后勾选或手动从开始菜单打开 **Claude Session Manager**。
6. 默认安装位置：

   ```text
   %LOCALAPPDATA%\Claude Session Manager\csm-app.exe
   ```

7. 在 PowerShell 确认实际安装路径和文件信息：

   ```powershell
   $app = "$env:LOCALAPPDATA\Claude Session Manager\csm-app.exe"
   Get-Item $app | Select-Object FullName, Length, LastWriteTime
   (Get-Item $app).VersionInfo.FileVersion
   ```

   应看到文件存在，`FileVersion` 为 `0.1.0`。如果提示“找不到路径”，说明安装到了别的用户目录，或安装器没有真正完成。

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

---

## 5. 安装完成后照着点一遍

按顺序做，任何一步失败都先看第 6 节，不要直接重装或删数据。

1. **打开 App**：应看到白色主界面，顶部有 Claude / Codex 切换。
2. **确认版本**：点左下角 **“⇧ 更新”**，面板应显示“当前版本 0.1.0”。如果显示“发现新版本 0.1.1”，是正常的：`v0.1.1` 只发布源码，没有新的安装包，保持当前 0.1.0 即可。
3. **看列表**：左侧应出现“本地会话”和按项目分组的会话。列表为空时先看 [TROUBLESHOOTING.md](TROUBLESHOOTING.md)，不要清空 `~/.claude` 或 `~/.codex`。
4. **看详情**：点一条会话，右侧应出现消息、元数据和“加载更早”。
5. **单开测试**：选一条**已停止**的可信测试会话，点“打开会话”。macOS 首次会弹 Terminal 自动化授权；Windows 应打开 Windows Terminal。
6. **批量测试**：再选两条已停止的会话，点“单窗多标签”。应只新建**一个** Terminal 窗口，里面有两个顶部标签。
7. **导出测试**：右键一条会话 →“导出 Markdown”，确认文件能保存并打开。

### 安装验收清单

- [ ] App 能从开始菜单 / 启动台正常打开
- [ ] 版本显示为 0.1.0
- [ ] Claude 和 Codex 至少一个列表有内容
- [ ] 会话详情和 Markdown 能显示
- [ ] 单条会话能恢复
- [ ] 两条会话能在同一个新窗口的两个标签中打开
- [ ] 导出 Markdown 成功
- [ ] 没有删除或清空任何真实会话目录

---

## 6. 常见卡点速查

| 现象 | 常见原因 | 怎么处理 |
| --- | --- | --- |
| macOS 提示“无法验证开发者” | Gatekeeper 首次拦截 | 按 [2.3](#23-首次启动完成单次-gatekeeper-确认) 到“隐私与安全性”点“仍要打开” |
| macOS 提示“应用已损坏” | 只复制了 App 的一部分，或从 DMG/下载目录直接运行 | 重新拖完整 `.app` 到 `/Applications`，再启动 |
| macOS 点“打开会话”没反应或报权限错误 | 没有 Terminal 自动化 / 辅助功能权限 | 按 [2.4](#24-权限一terminal-自动化--apple-events) 和 [2.5](#25-权限二辅助功能--accessibility) 授权，然后 `Command + Q` 完全退出再开 |
| macOS 替换 App 后权限失效 | 新构建签名变化，旧 TCC 授权不匹配 | 按 [重编译或替换 App 后权限失效](#重编译或替换-app-后权限失效) 移除旧条目再添加 |
| macOS 双击 App 没反应 | 可能仍在运行旧实例，或 App 不完整 | 先 `Command + Q` 退出旧版；再检查 `/Applications` 中是完整 App |
| Windows SmartScreen 提示“未知发布者” | 当前安装器没有 Authenticode 发布者签名 | 按 [3.3](#33-smartscreen-与发布者提示) 核对文件名后单次确认，不要关闭 SmartScreen |
| Windows 安装后打开还是旧版 | 旧进程还在，或开始菜单指向旧快捷方式 | 完全退出旧版，重新运行新 NSIS；用 3.2 的路径命令确认实际文件 |
| Windows 双击没反应或白屏 | WebView2 Runtime 缺失或损坏 | 安装/修复 WebView2 Evergreen Runtime，再重开 App |
| Windows 更新检查失败 | 网络、代理或 DNS 不通 | App 本身可继续使用；需要检查时恢复网络后再点“保存并检查更新” |
| 列表为空或只有 shell 会话 | CLI 还没产生会话，或目录配置不同 | 先按 [TROUBLESHOOTING.md](TROUBLESHOOTING.md) 排查，不要删除会话目录 |
| 批量打开只出一个标签 | 选择里包含已在运行的会话，或权限未完成 | 只选已停止的测试会话；macOS 确认辅助功能权限后重试 |
