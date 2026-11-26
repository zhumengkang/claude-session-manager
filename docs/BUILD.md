# 源码编译与构建验证

[文档中心](README.md) · 普通用户：[二进制安装与授权](INSTALL.md) · [使用指南](USAGE.md) · [GitHub 更新](UPDATES.md) · [部署文档](DEPLOYMENT.md) · [测试说明](../smoke-tests/README.md)

> **只想使用应用时无需阅读本文。** 直接使用已发布的 DMG 或 NSIS 即可。本文面向需要修改源码、自行签名或重新发布的人。

## 1. 构建前说明

- 后端：Rust 2021 + Tauri 2。
- 前端：`src/index.html` 静态 HTML/CSS/JavaScript，**没有前端打包步骤**。
- UI 资源：本地 Tabler 1.4.0，不需要运行时 CDN。
- Codex SQLite：`rusqlite` bundled SQLite，不需要外置 `sqlite3`。
- 当前版本：`0.1.0`。
- 当前 macOS 配置使用 ad hoc 签名；重新编译通常会改变 CDHash，旧 TCC 授权可能不再匹配。
- 当前实际构建证据对应 macOS d、Windows e 和测试 f 输入。最新文档包只是文档刷新，**不等于重新编译这些二进制**。
- 当前源码新增 `update_checker.rs`，使用 `reqwest` blocking + `native-tls` 读取公开 GitHub Release，`semver` 比较版本，`open` 打开已验证发布页。
- 更新请求连接超时 5 秒、总超时 12 秒、最多 3 次重定向、仅 HTTPS、响应最大 1 MiB，并通过 `spawn_blocking` 避免阻塞 GUI。
- 加入更新功能后的 macOS App/DMG 已实际构建，strict codesign 与 `hdiutil verify` 通过；构建目录中的 App 已在隔离空数据环境启动并确认“更新”入口。它尚未覆盖 `/Applications`，真实 App 更新面板点击未完成。Windows 新构建尚未执行，旧二进制验收不能冒充该功能的安装版验收。

更新功能、Release 标签和真实仓库验证边界见 [UPDATES.md](UPDATES.md)。

## 2. 获取源码

克隆实际发布的仓库，或使用发布者提供的确定性源码包：

```bash
git clone https://github.com/<OWNER>/<REPO>.git
cd <REPO>
```

将占位符替换为真实地址。项目根目录应包含 `src/`、`src-tauri/`、`docs/` 和 `smoke-tests/`。不要运行备份目录中来源不明的旧 App 或旧自动化脚本。

## macOS 构建

### 1. 环境要求

- macOS 与 Xcode Command Line Tools。
- stable Rust，包含 `rustc`、`cargo`，使用 rustup 管理工具链。
- Tauri CLI 2。
- 运行和恢复测试需要系统 Terminal、Claude Code / Codex CLI。
- **仅前端 mock 冒烟测试**额外需要 Node.js 和 Chrome；正常前端构建不需要 Node。

```bash
xcode-select --install
rustc --version
cargo --version
rustup show active-toolchain
```

安装 Tauri CLI 2：

```bash
cargo install tauri-cli --version "^2.0.0" --locked
cargo tauri --version
```

本次开发曾使用 Rust/Cargo 1.94.0、Tauri CLI 2.10.1、Node.js 26.13.1；这是环境记录，不是强制版本下限。

### 2. 开发运行与静态检查

在项目根目录：

```bash
cargo tauri dev
```

仅检查编译：

```bash
cargo check --manifest-path src-tauri/Cargo.toml
```

普通测试和 lint：

```bash
cargo fmt --manifest-path src-tauri/Cargo.toml --check
cargo test --manifest-path src-tauri/Cargo.toml
cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings
```

前端 mock 冒烟：

```bash
./smoke-tests/run-frontend-smoke.sh
./smoke-tests/run-frontend-smoke.sh ./frontend-review-regression.mjs
```

如脚本没有执行权限：

```bash
zsh smoke-tests/run-frontend-smoke.sh
```

这些前端脚本只启动自己的 headless Chrome，注入模拟 Tauri IPC；不读取真实会话、不打开 Terminal、不修改真实权限。

### 3. 构建 App 与 DMG

```bash
cargo tauri build
```

当前 `bundle.targets` 为 `app` 和 `dmg`，主要输出：

```text
src-tauri/target/release/bundle/macos/Claude Session Manager.app
src-tauri/target/release/bundle/dmg/Claude Session Manager_<version>_<arch>.dmg
```

当前版本 Apple Silicon 示例：

```text
src-tauri/target/release/bundle/dmg/Claude Session Manager_0.1.0_aarch64.dmg
```

构建后检查：

```bash
codesign --verify --deep --strict --verbose=2 \
  "src-tauri/target/release/bundle/macos/Claude Session Manager.app"

hdiutil verify \
  "src-tauri/target/release/bundle/dmg/Claude Session Manager_0.1.0_aarch64.dmg"
```

安装、替换和授权按 [INSTALL.md](INSTALL.md) 执行。不要只复制 App 内的单个 Unix 可执行文件作为 macOS 分发包。

### 4. 架构目标

原生构建最简单：在对应架构 Mac 上运行 `cargo tauri build`。

Apple Silicon：

```bash
rustup target add aarch64-apple-darwin
cargo tauri build --target aarch64-apple-darwin
```

Intel：

```bash
rustup target add x86_64-apple-darwin
cargo tauri build --target x86_64-apple-darwin
```

Universal 候选路线：

```bash
rustup target add aarch64-apple-darwin x86_64-apple-darwin
cargo tauri build --target universal-apple-darwin
```

**当前只记录 Apple Silicon 原生构建验收。** Intel 与 Universal 是构建路线，不是已实测发布产物。正式提供前需要分别检查启动、`lipo -info`、会话列表/详情/恢复、签名、公证、TCC 和原生 Terminal 标签。

### 5. Developer ID 与公证

当前 `src-tauri/tauri.conf.json`：

```json
"macOS": {
  "signingIdentity": "-"
}
```

这明确表示 ad hoc。公开发行建议单独建立流程：

1. 准备有效的 `Developer ID Application` 证书。
2. 用 `security find-identity -v -p codesigning` 检查身份。
3. 在受控发布配置中使用正确签名身份，不继续固定为 `-`。
4. 按 Apple 当前要求签名完整 App、嵌套内容和 DMG。
5. 用 `xcrun notarytool` 提交公证，用 `xcrun stapler` 验证/附加票据。
6. 在干净用户环境验证下载首次启动、TCC 和 Terminal 自动化。
7. 不提交私钥、Apple ID 密码、App 专用密码、API Key 或 notary keychain profile。

当前项目没有完成该正式发布流程，不能描述为“已 Developer ID 签名并公证”。

## Windows 构建

### 1. 环境要求

建议使用受支持的 64 位 Windows 10/11，并准备：

1. stable Rust MSVC 工具链。
2. Visual Studio Build Tools 的 **Desktop development with C++ / 使用 C++ 的桌面开发** 工作负载。
3. 可用 Windows SDK；`rusqlite` bundled 需要 C 编译器和 SDK，仅安装 Rust target 不够。
4. Tauri CLI 2。
5. WebView2 Runtime。
6. Windows Terminal，确认 `wt.exe` 应用执行别名可用。
7. Windows PowerShell `powershell.exe`；当前后端不是自动改用 `pwsh`。
8. 运行恢复测试时需要原生 Windows Claude/Codex CLI。

PowerShell 检查：

```powershell
rustc --version
cargo --version
rustup show
where.exe claude
where.exe codex
where.exe wt
where.exe powershell
claude --version
codex --version
```

`where.exe` 只用于人工诊断，应用内部直接遍历 PATH/PATHEXT，避免控制台代码页破坏 Unicode 路径。

### 2. 开发、测试与 NSIS

```powershell
cargo fmt --manifest-path src-tauri/Cargo.toml --check
cargo test --manifest-path src-tauri/Cargo.toml
cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings
cargo tauri dev
```

仅检查编译：

```powershell
cargo check --manifest-path src-tauri/Cargo.toml
```

构建当前用户 NSIS：

```powershell
cargo tauri build --bundles nsis
```

预期输出：

```text
src-tauri/target/release/csm-app.exe
src-tauri/target/release/bundle/nsis/Claude Session Manager_<version>_<arch>-setup.exe
```

当前 ARM64 示例：

```text
src-tauri/target/release/bundle/nsis/Claude Session Manager_0.1.0_arm64-setup.exe
```

NSIS 配置为 `installMode = currentUser`。安装与 SmartScreen 边界见 [INSTALL.md](INSTALL.md#3-windows-11-arm64-安装与首次启动)。

### 3. MSI 与架构边界

MSI 候选命令：

```powershell
cargo tauri build --bundles msi
```

预期目录：

```text
src-tauri/target/release/bundle/msi/
```

**MSI 命令未在当前验收中执行，不能标为已验证。** 当前只完成 Windows 11 ARM64 Release/NSIS 和最终安装路径 GUI；Windows x64、Explorer、自启动真实切换、原生标签聚焦和真实 `.ps1` Provider 完整恢复仍有独立缺口。

全局 `bundle.targets` 保持 macOS 的 `app` / `dmg`，Windows 通过 `--bundles` 显式选择，避免 macOS 默认构建强制引入 Windows bundler。

## 3. 真实测试不是普通单元测试的替代品

带 `--ignored` 的真实 Terminal 测试不会随普通 `cargo test` 自动执行。不要为了“跑全测试”直接对所有 ignored 测试使用 `--include-ignored`。

测试层级和边界：

| 测试 | 能证明 | 不能证明 |
| --- | --- | --- |
| Rust 普通测试 | 解析、缓存、命令构造、身份与结果逻辑 | 真实 Terminal GUI 成功 |
| Clippy/fmt | 静态质量与格式 | 运行时行为 |
| 前端 CDP mock | 真实前端交互与模拟 IPC 契约 | 真实存储、CLI、TCC、Terminal |
| 隔离 Provider GUI | 真实 App、WebView2/Terminal、精确 argv、原生标签 | 官方账号完整 TUI 恢复 |
| 已安装 App GUI | 当前二进制的安装、授权和端到端点击路径 | 其他系统、架构或未来构建 |

完整命令和严格 owned 清理规则见：

- [../smoke-tests/README.md](../smoke-tests/README.md)
- [../smoke-tests/windows/README.md](../smoke-tests/windows/README.md)
- [VALIDATION.md](VALIDATION.md)

任何真实 Terminal 测试只能操作本次创建且可证明所有权的对象；不关闭当前对话或已有终端，不按进程名批量结束，不把窗口差集当作清理凭据，不发送模型提示。

## 4. 构建后交付检查

1. 记录实际源码输入、版本和构建环境。
2. 记录 Release、DMG、NSIS 的文件名、大小和架构。
3. 从实际交付副本再次检查文件完整性和安装行为，而不只检查构建目录。
4. macOS 检查 strict codesign、DMG、Gatekeeper 首次启动和当前系统授权。
5. Windows 分别核对独立 Release、NSIS 和最终安装路径，确认当前用户安装与 SmartScreen 提示。
6. 更新/卸载/干净重装分别记录安装器退出码和独立状态证据。
7. 普通测试、前端 mock 和真实 GUI 分开报告。
8. 保留未覆盖项，不写“已经没有 bug”。
9. 文档更新无需伪装成新二进制构建；源码或测试脚本改变时应明确区分输入。

## 5. GitHub CI 与附件边界

仓库包含两个源码检查 workflow：

- [source-only.yml](../.github/workflows/source-only.yml)：对 push、pull request 和手动运行检查 Git 实际内容，拒绝安装包、压缩附件、二进制载荷、真实会话、凭据和超大文件。
- [windows-source-check.yml](../.github/workflows/windows-source-check.yml)：在相关源码变化或手动运行时，使用 `windows-latest` 与 stable Rust 执行普通 Rust 测试。

Windows workflow **不再安装 Tauri CLI、不构建 NSIS、不上传 artifact**；仓库不使用 `actions/upload-artifact`，也不自动创建 Release。DMG、NSIS 等附件应在仓库外单独构建和验证，以后由项目所有者通过 GitHub Release 单独上传。完整规则见 [GitHub 源码专用仓库与附件隔离](GITHUB_SOURCE_ONLY.md)。

2026-11-26 首轮源码导入已实际运行并通过：[source-only](https://github.com/zhumengkang/claude-session-manager/actions/runs/45785523418) 与 [windows-source-check](https://github.com/zhumengkang/claude-session-manager/actions/runs/45785521950) 均为 success。后续提交状态以 GitHub Actions 页面为准；CI 源码检查不能代替真实 Windows Terminal 的隔离 GUI 验收，首轮 Windows 运行还记录了 `actions/checkout@v4` 的 Node.js 20 弃用提示，该提示不是测试失败。
