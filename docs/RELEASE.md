# 开源发布、版本与许可证

[文档中心](README.md) · **[源码专用仓库与附件隔离](GITHUB_SOURCE_ONLY.md)** · [源码编译](BUILD.md) · [GitHub 更新](UPDATES.md) · [部署文档](DEPLOYMENT.md) · [验证记录](VALIDATION.md)

> 当前项目自身 `LICENSE` 尚未由所有者选择并添加。公开可读不等于已授予完整复制、修改和再分发许可；Tabler 的 MIT 许可证不能自动变成整个项目的许可证。

## 1. 发布前先补许可证

1. 由项目所有者选择许可证并添加根目录 `LICENSE` 与必要版权信息。
2. 保留 `src/vendor/tabler/LICENSE`。
3. 审查 Tauri、Rust crates 及其他实际分发内容的许可证和 NOTICE 要求。
4. 发布二进制时按许可证要求附带第三方声明。
5. 在根许可证补齐前，不在 README、Release 或软件元数据中直接声明“整个项目采用 MIT”。

## 2. 版本号与 GitHub Release

应用内更新检查要求公开仓库的 latest stable Release：

- 标签使用 `v1.2.3` 或 `1.2.3`。
- Release 不是草稿或预发布。
- `Cargo.toml` 与 `tauri.conf.json` 的版本保持一致，并随发布递增。
- 当前版本由构建时 `CARGO_PKG_VERSION` 提供，不在前端另行写死。
- latest Release 的版本高于用户当前版本时才显示升级横幅。
- 发布页应包含对应平台的实际产物、安装说明、架构与未覆盖边界。

应用只提醒并打开发布页，不自动下载、替换或安装。仓库配置和忽略版本规则见 [UPDATES.md](UPDATES.md)。

## 3. 发布产物

按实际构建结果提供，不要为了凑清单发布未构建架构：

| 平台 | 产物 | 发布前必须说明 |
| --- | --- | --- |
| macOS Apple Silicon | 完整 `.app` / DMG | ad hoc 或 Developer ID、是否公证、首次启动与授权步骤 |
| Windows ARM64 | NSIS 与可选独立 Release | 当前用户安装、WebView2、SmartScreen、是否 Authenticode |
| 其他架构 | 仅实际构建后提供 | 不得把 ARM64 标为 Universal 或 x64 |

产物文件名应包含版本与架构。发布页写明文件来源、安装方式、系统要求、签名状态和未覆盖边界，不要求用户进行额外的文件摘要比对。

## 4. 发布前排除敏感内容

不得提交或上传：

- `~/.claude`、`~/.codex` 的真实复制目录。
- 真实 JSONL、rollout、`state_5.sqlite`、`session_index.jsonl`。
- 真实 `favorites.json`、认证文件、Token、Cookie、私钥或证书导出。
- 未脱敏日志、崩溃报告、Terminal 截图和含真实路径的测试输出。
- 临时 Chrome profile、下载目录、测试导出或合成 daemon 秘密。
- Apple 公证凭据、CI secret、本地签名配置和更新设置中的个人仓库偏好。
- 包含真实用户目录或旧危险自动化逻辑的备份包。

回归数据应制作全新的最小化脱敏 fixture。若敏感内容曾进入 Git 历史，仅删除最新文件并不足够。

## 5. 源码仓库与附件强制隔离

本目录是源码专用 Git 根目录。实际 `.gitignore`、`.gitattributes`、`.githooks/`、`scripts/check-source-only.sh` 和 `source-only.yml` 共同执行以下规则：

- Git 只包含源码、锁文件、测试、文档和必要源码资源。
- DMG、App、EXE、MSI、安装包、压缩包、构建目录、发布校验清单和真实会话数据不得进入 Git。
- pre-commit 检查暂存树，pre-push 检查 HEAD；GitHub CI 再检查远端提交内容。
- 不使用 Git LFS、子模块或重命名绕过附件限制，不执行 `git add -f`。
- Windows CI 只运行源码测试，不构建或上传 NSIS，不使用 `actions/upload-artifact`。
- 二进制附件以后由项目所有者在 GitHub Release 中单独上传，不能先提交进源码仓库。

完整操作和恢复说明见 **[GITHUB_SOURCE_ONLY.md](GITHUB_SOURCE_ONLY.md)**。

Rust 应用应提交 `Cargo.lock` 固定构建依赖，不要按库项目习惯随意忽略。

## 6. 发布检查清单

### 源码与文档

- [ ] `scripts/check-source-only.sh --staged` 通过，暂存树只有源码及必要源码资源。
- [ ] `git diff --cached --name-only` 已人工复核，无 DMG/App/EXE/MSI、压缩包、构建产物、真实会话或个人路径。
- [ ] 根 `LICENSE` 与第三方声明已补齐。
- [ ] 文档链接、锚点和图片路径检查通过。
- [ ] 首页优先展示二进制安装与使用，编译位于后面。
- [ ] 安装包名、版本、架构与真实产物一致。
- [ ] 文档中的版本、架构、安装步骤和签名状态与实际产物一致。
- [ ] 没有把历史安装版结果冒充新源码或新二进制结果。
- [ ] 已知限制与未测试平台仍保留。

### 普通验证

- [ ] `cargo fmt --check`、`cargo test`、严格 Clippy 通过。
- [ ] 完整前端冒烟和独立审查回归通过。
- [ ] 0 JavaScript 异常、0 意外外部 HTTP 资源。
- [ ] 更新面板 mock 覆盖新版本、同版本、忽略、失败和纯文本渲染。
- [ ] 使用真实公开仓库分别完成同版本与更高版本的只读检查。
- [ ] 真实 Terminal / Provider 测试按平台单独记录，不以 mock 代替。

### 构建与安装

- [ ] 从干净克隆构建，记录实际源码输入。
- [ ] 生成当次产物清单，并在实际交付副本上复核。
- [ ] macOS 检查 strict codesign、DMG 与当前构建授权。
- [ ] Windows 分别核对独立 Release、NSIS 内层载荷和最终安装文件。
- [ ] 更新、卸载、干净重装分别执行并记录结果。
- [ ] 新构建重新做 GUI 截图复核，不沿用旧图冒充新功能。
- [ ] CI 已实际运行才引用绿色结果与 Actions URL。

## 7. 发布 GitHub Release 的建议顺序

1. 递增版本并完成上述检查。
2. 提交实际源码与锁文件；提交前运行 `scripts/check-source-only.sh --staged`，是否提交/推送由项目所有者决定。
3. 推送前运行 `scripts/check-source-only.sh --tracked` 并检查 `git ls-files`，确保没有附件或本地数据。
4. 在仓库外构建并验证对应平台产物；附件由项目所有者以后在 GitHub Release 中单独上传，不进入 Git。
5. 在 Release 说明中写明安装、授权、升级方式、安全提示和已知边界。
6. 先以草稿核对内容，再发布为非预发布的 latest stable Release。
7. 在已配置 `owner/repository` 的应用中手动检查：
   - 同版本安装不应显示升级横幅。
   - 旧版本安装应显示新版本横幅。
   - “查看发布页升级”必须打开该仓库对应 Release。
8. 不需要为了验证提醒而自动下载或替换当前安装。

不要编造仓库、Release、版本检查响应或 Actions 结果。

## 8. 贡献与问题报告

贡献 PR 建议包含：

1. 问题与修改范围。
2. 测试命令和真实输出摘要。
3. 未验证的平台与边界。
4. Terminal 自动化、权限、更新网络或数据格式变化说明。
5. 不含真实会话正文的脱敏复现材料。

报告安全问题时，不要公开真实认证信息、完整会话或用户路径；先提供最小化复现与受影响版本。

## 9. 独立项目声明

本项目是独立本地工具，不代表 Anthropic、OpenAI、Apple、Microsoft 或 GitHub 的官方产品。Claude/Codex 恢复后的行为由对应 CLI 与用户自己的配置决定。

## 参考资料

- Tauri 2：<https://tauri.app/>
- Rust / rustup：<https://www.rust-lang.org/tools/install>
- GitHub Releases：<https://docs.github.com/repositories/releasing-projects-on-github>
- Windows Terminal 命令行：<https://learn.microsoft.com/windows/terminal/command-line-arguments>
- 测试说明：[../smoke-tests/README.md](../smoke-tests/README.md)
