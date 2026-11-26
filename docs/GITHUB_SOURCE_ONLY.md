# GitHub 源码专用仓库与附件隔离

[文档中心](README.md) · [项目首页](../README.md) · [开源发布](RELEASE.md) · [源码编译](BUILD.md)

> 本目录是 **源码专用 Git 根目录**。Git 只保存源码、锁文件、测试、文档，以及应用图标和文档图片等必要源码资源。DMG、App、EXE、MSI、压缩包、构建目录和交付清单不得进入 Git。二进制附件由项目所有者以后单独上传到 GitHub Release。

## 1. 仓库边界

**Git 根目录只能是包含本文件所在项目的 `csm-app/`。** 不要把包含 `macOS Apple Silicon/`、`Source/*.tar.gz` 或交付说明的上层交付目录初始化成仓库。

| 内容 | 是否进入 Git | 后续位置 |
| --- | --- | --- |
| Rust、HTML/CSS/JavaScript、配置、能力声明 | 是 | 源码仓库 |
| `Cargo.lock`、测试脚本、最小脱敏测试源码 | 是 | 源码仓库 |
| Markdown 文档、必要的应用图标与文档截图 | 是 | 源码仓库 |
| `.app`、DMG、EXE、MSI、DLL、dylib、构建目录 | **否** | 仓库外；验证后由所有者单独发布 |
| 源码压缩包、交付压缩包、交付清单 | **否** | 仓库外；需要时作为 Release 附件单独上传 |
| 真实 JSONL、SQLite、收藏、更新设置、日志、凭据 | **否** | 留在本机，不上传 |

文档图片和应用图标是界面/文档的组成资源，不是 DMG、安装器或交付压缩包等发布附件。单文件仍受 5 MiB 限制。

## 2. 已设置的三重防线

1. `.gitignore`：默认排除构建产物、安装包、压缩包、真实会话、日志和签名材料。
2. `.githooks/pre-commit` 与 `.githooks/pre-push`：检查 Git 暂存树或 HEAD 的实际内容。
3. `.github/workflows/source-only.yml`：GitHub 对 push、pull request 和手动运行再次检查。

检查器不只依赖扩展名：它还会拒绝非源码 MIME 类型、符号链接、Git LFS 指针、子模块、超过 5 MiB 的文件和常见敏感数据路径。不得用 `git add -f`、重命名或 Git LFS 绕过。

Windows CI 只执行源码测试，不再构建或上传 NSIS，也不使用 `actions/upload-artifact`。本仓库任何 workflow 都不应自动上传二进制附件或创建 Release。

本地 hooks 不是服务器权限，仍可能被 `--no-verify` 绕过。配置真实 GitHub 远端后，所有者还应按仓库权限启用合适的分支保护 / ruleset，并把 `source-only` 设为必需检查、限制未经检查的推送。仓库是 Private 还是 Public 与是否启用保护是两件事；未显式检查和配置前，不能声称服务器端保护已经生效。即使 CI 失败，也不应继续合并或发布包含附件的提交。

## 3. 本地启用与每次检查

首次克隆或hooks未生效时：

```bash
git config core.hooksPath .githooks
```

提交前：

```bash
git status --short
git add --all
./scripts/check-source-only.sh --staged
git diff --cached --name-only
```

已经提交、准备推送前：

```bash
./scripts/check-source-only.sh --tracked
git ls-files
```

`SOURCE_ONLY_OK` 才表示机械边界检查通过。它不能替代人工审查文档中的个人路径、截图内容、Token、真实会话正文或许可证。

## 4. 正确的后续发布方式

1. 在源码仓库提交并推送源码；提交和推送由项目所有者决定。
2. 在仓库外构建 DMG / NSIS，并按当次发布清单验证。
3. 二进制附件以后由项目所有者通过 GitHub Release 单独上传，不执行 `git add`，不放进源码目录。
4. 发布说明应写清版本、架构、安装步骤、系统要求和未覆盖边界，不把交付文件回填进源码目录。
5. 不为上传附件恢复 `actions/upload-artifact`、自动 Release 或 Git LFS，除非项目所有者以后明确改变规则。

## 5. 发现误加入时

- 尚在暂存区：从暂存区移除，并确认 `.gitignore` 已覆盖；不要使用强制添加。
- 已提交但未推送：先停止推送，由所有者选择修正提交的方式。
- 已推送或含敏感信息：仅删除最新文件不够，需要评估 Git 历史与凭据轮换；不要掩盖或假装未发生。

项目自身 `LICENSE` 仍需所有者选择。源码目录整理完成不代表已经获得开源许可、已经推送或已经发布 Release。
