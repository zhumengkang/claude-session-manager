# 文档中心

[返回项目首页](../README.md) · **[二进制安装与授权](INSTALL.md)** · [使用指南](USAGE.md) · [GitHub 更新](UPDATES.md) · [部署/更新/卸载](DEPLOYMENT.md) · [源码编译](BUILD.md)

> 只想安装和使用：按 **安装与授权 → 使用指南** 阅读，不需要先安装 Rust，也不需要阅读编译文档。

## 按任务快速定位

| 我要做什么 | 直接打开 | 主要内容 |
| --- | --- | --- |
| 安装 macOS DMG | [INSTALL.md](INSTALL.md#macos安装-dmg推荐) | 校验、拖入 `/Applications`、首次启动 |
| 安装 Windows ARM64 | [INSTALL.md](INSTALL.md#windows-arm64安装-nsis推荐) | 当前用户 NSIS 安装、WebView2、Windows Terminal |
| 解决 macOS“不受信任/无法打开” | [INSTALL.md](INSTALL.md#macos首次启动gatekeeper-与签名) | ad hoc、Gatekeeper、完整性检查 |
| 配置 Terminal 自动化和辅助功能 | [INSTALL.md](INSTALL.md#macos必须授予的两项权限) | Apple Events、Accessibility、重新启动 |
| 重编译后权限开关仍在但恢复被拒绝 | [INSTALL.md](INSTALL.md#重编译或替换-app-后权限失效) | CDHash、csreq、定向重置与重新授权 |
| 浏览、搜索、收藏和查看详情 | [USAGE.md](USAGE.md#1-浏览搜索与详情) | Provider、目录分组、反向分页、Markdown |
| 打开一个会话 | [USAGE.md](USAGE.md#3-恢复单个会话) | 精确恢复命令、运行中保护 |
| 一批会话在一个窗口多标签打开 | [USAGE.md](USAGE.md#4-批量恢复一个批次一个新窗口多个顶部标签) | 批次、去重、跨 Provider、新窗口规则 |
| 导出、复制路径、显示文件 | [USAGE.md](USAGE.md#5-导出复制与文件管理器) | Markdown 快照、权限、路径操作 |
| 检查 GitHub 新版本、忽略或重新查看 | [UPDATES.md](UPDATES.md) | 更新面板、自动检查、升级横幅、发布页 |
| 更新、卸载、自启动或回滚 | [DEPLOYMENT.md](DEPLOYMENT.md) | 两平台部署生命周期与数据保留 |
| 从源码编译 macOS App/DMG | [BUILD.md](BUILD.md#macos-构建) | 环境、命令、输出、签名和验证 |
| 从源码编译 Windows NSIS | [BUILD.md](BUILD.md#windows-构建) | MSVC、Windows SDK、NSIS、输出 |
| 自定义 CLI、Node 或会话目录 | [CONFIGURATION.md](CONFIGURATION.md) | 自动发现、绝对路径覆盖、环境变量 |
| 列表为空、只有 shell、权限报错 | [TROUBLESHOOTING.md](TROUBLESHOOTING.md) | 按症状排查及当前限制 |
| 查看功能截图 | [FEATURES.md](FEATURES.md) | 白色 UI、详情、选择、Windows Terminal |
| 查看架构与代码结构 | [ARCHITECTURE.md](ARCHITECTURE.md) | Tauri IPC、Rust 后端、平台集成 |
| 查看 Windows 实现细节 | [WINDOWS.md](WINDOWS.md) | PowerShell、Restart Manager、NSIS、UIA |
| 查看测试与历史验收证据 | [VALIDATION.md](VALIDATION.md) | 当前结果、历史构建、明确未覆盖项 |
| 准备开源发布 | [RELEASE.md](RELEASE.md) | 许可证、第三方声明、发布检查 |
| **配置 GitHub 源码专用仓库、隔离附件** | **[GITHUB_SOURCE_ONLY.md](GITHUB_SOURCE_ONLY.md)** | Git 根目录、忽略规则、hooks、CI 与手动 Release 边界 |
| 运行测试 | [../smoke-tests/README.md](../smoke-tests/README.md) | 前端、Rust、真实 GUI 的边界与命令 |

## 推荐阅读顺序

### 普通使用者

1. [二进制安装与授权](INSTALL.md)
2. [使用指南](USAGE.md)
3. [GitHub 更新检查](UPDATES.md)（需要升级提醒时）
4. [故障排查](TROUBLESHOOTING.md)（只有出现问题时需要）
5. [部署、更新与卸载](DEPLOYMENT.md)（需要更新或卸载时）

### 开发与发布者

1. [GitHub 源码专用仓库与附件隔离](GITHUB_SOURCE_ONLY.md)
2. [源码编译](BUILD.md)
3. [架构与实现](ARCHITECTURE.md)
4. [测试说明](../smoke-tests/README.md)
5. [验证记录](VALIDATION.md)
6. [开源发布清单](RELEASE.md)

## 文档目录

```text
csm-app/                          # 唯一的源码专用 Git 根目录
├── .gitignore / .gitattributes   # 附件排除与文本/二进制换行规则
├── .githooks/                    # 本地提交、推送前检查
├── .github/workflows/            # 源码检查，不上传构建附件
├── scripts/
│   └── check-source-only.sh      # 暂存树 / HEAD 实际内容检查
├── README.md                     # 首页：二进制安装、使用优先，编译在后
├── docs/
│   ├── README.md                 # 本索引：按任务定位
│   ├── INSTALL.md                # 二进制安装、macOS/Windows 授权
│   ├── USAGE.md                  # 浏览、详情、恢复、批量、导出
│   ├── UPDATES.md                # GitHub 更新检查、横幅、忽略与发布页
│   ├── DEPLOYMENT.md             # 更新、卸载、自启动、回滚、数据保留
│   ├── BUILD.md                  # macOS/Windows 源码编译与构建验证
│   ├── CONFIGURATION.md          # 数据路径、CLI/Node 发现、环境变量、隐私
│   ├── TROUBLESHOOTING.md        # 已知限制与故障排查
│   ├── FEATURES.md               # 功能与示例图
│   ├── ARCHITECTURE.md           # 架构与源码结构
│   ├── WINDOWS.md                # Windows 实现与实机细节
│   ├── VALIDATION.md             # 当前与历史测试验收记录
│   ├── RELEASE.md                # 开源发布、许可证与贡献说明
│   ├── GITHUB_SOURCE_ONLY.md     # 源码仓库边界、hooks、CI 与附件隔离
│   └── images/                   # 必要的文档源码资源，无图床/CDN
└── smoke-tests/
    ├── README.md                 # 测试总入口
    └── windows/README.md         # Windows 原生隔离测试
```

## 交付包中的文档位置

如果你拿到的是 `Claude-Session-Manager-20261126-docs` 交付目录：

1. 先读根目录 `README-交付说明.md`，了解产品文件与验收边界。
2. 在 `Source/` 解压最新源码文档包，首页就是 `csm-app/README.md`。
3. 按上表进入 `csm-app/docs/`。
4. 产品哈希以交付根目录 `SHA256SUMS.txt` 为准；证据范围以 `manifest.json` 和 `delivery-verification.json` 为准。

文档包与 d/e/f 实际构建输入分开保存。**文档更新不等于重新编译，也不会改变现有二进制哈希。**
