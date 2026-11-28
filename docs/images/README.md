# 文档示例图

本目录提供 README 使用的本地图片，不依赖外部图床。

## 图片清单

| 文件 | 内容 | 性质 |
| --- | --- | --- |
| `csm-ui-claude.png` | Claude 会话列表、搜索过滤、收藏与底部操作区 | 真实前端渲染 + 模拟 IPC |
| `csm-ui-detail.png` | 最新消息详情、元数据卡片、Markdown 和代码块 | 真实前端渲染 + 模拟 IPC |
| `csm-ui-selection.png` | 全选、多选和“单窗多标签”按钮 | 真实前端渲染 + 模拟 IPC |
| `csm-ui-codex.png` | Codex 主会话、子线程、消息计数与详情 | 真实前端渲染 + 模拟 IPC |
| `csm-ui-context-menu.png` | 打开、导出、复制路径和文件管理器菜单 | 真实前端渲染 + 模拟 IPC |
| `csm-ui-update.png` | GitHub 仓库设置、当前版本和检查结果 | 真实前端渲染 + 模拟 IPC |
| `csm-ui-update-banner.png` | 更高稳定版本的升级横幅与操作按钮 | 真实前端渲染 + 模拟高版本 |
| `csm-ui-900.png` | 900 × 700 最小宽度附近布局 | 真实前端渲染 + 模拟 IPC |
| `csm-macos-live-ui.png` | macOS 安装版主界面：目录分组、搜索、收藏与底部操作区 | 作者本机 macOS 安装版实机截图 |
| `csm-macos-live-update.png` | macOS 安装版的更新面板与同版本检查结果 | 作者本机 macOS 安装版实机截图 |
| `csm-macos-live-banner.png` | macOS 0.1.0 检查到 `v0.1.1` 的升级横幅 | 作者本机 macOS 安装版实机截图 |
| `csm-windows-live-ui.png` | Windows 11 ARM64 安装版主界面与会话详情 | 作者本机 Windows 11 ARM64 安装版实机截图 |
| `csm-windows-live-update.png` | Windows 安装版的更新面板与检查结果 | 作者本机 Windows 11 ARM64 安装版实机截图 |
| `install-macos-dmg.png` | DMG 窗口：把 App 拖到 Applications，含红色箭头标注 | macOS 实机窗口截图 + 标注 |
| `install-windows-setup-welcome.png` | Windows NSIS 安装向导欢迎页 | Windows 11 ARM64 实机安装窗口 |
| `install-windows-setup-maintenance.png` | 已安装同版本时的维护页 | Windows 11 ARM64 实机安装窗口 |
| `install-windows-setup-location.png` | 安装位置页，默认 LOCALAPPDATA 路径 | Windows 11 ARM64 实机安装窗口；用户名已打码 |
| `install-windows-setup-complete.png` | 安装完成页，绿色进度条 | Windows 11 ARM64 实机安装窗口 |
| `install-windows-setup-finish.png` | 完成页：运行应用与创建桌面快捷方式 | Windows 11 ARM64 实机安装窗口 |
| `csm-windows-webview2.png` | Windows 白色 UI、205 条分页后的详情、Markdown 和代码块 | Windows 11 ARM64 真实 Release App + WebView2 + 合成夹具 |
| `csm-windows-terminal-tabs.png` | Claude 收藏批开创建的 Windows Terminal 顶部两个标签 | 已验证自有 HWND 的 `PrintWindow` + 合成夹具 |
| `csm-windows-codex-terminal-tab.png` | Claude 已运行后，Codex 首次批开在另一原生窗口创建的一个标签 | 已验证不同自有 HWND 的 `PrintWindow` + 合成夹具 |
| `architecture.svg` | UI、共享 Rust、Provider 数据、内嵌 SQLite 与 macOS/Windows 集成 | 架构示意图 |
| `install-flow.svg` | macOS 安装、权限与重编译授权流程 | 流程示意图 |
| `terminal-tabs.svg` | 一个 Terminal 原生窗口中的多个顶部标签 | 结构示意图，不是 Terminal 截图 |

## 数据与验证边界

- 八张 `csm-ui-*.png` 由 `smoke-tests/frontend-smoke.mjs` 驱动真实 `src/index.html` 在 headless Chrome 中生成，使用模拟 IPC 和 `/mock/...` 路径。更新图片使用 `example-owner/session-manager` 与 `9.9.9` 合成高版本，不是真实 GitHub 发布证据。
- 五张 `csm-macos-live-*.png` / `csm-windows-live-*.png` 是作者本机安装版的窗口截图，不是 mock。只包含会话标题、项目分组和版本号，不含消息正文、凭据或完整用户路径；更新面板与横幅中的仓库是项目公开仓库，版本号是对应发布时的真实 latest。
- 六张 `install-*.png` 来自真实安装窗口。Windows 向导截图取自 0.1.1 同版本维护流程；首次安装会跳过维护页，其余页面与按钮相同。`install-windows-setup-location.png` 中的用户名已打码。`install-macos-dmg.png` 的红色箭头是后加标注。
- `csm-windows-webview2.png` 来自真实 Windows 11 ARM64 Release App 的 WebView2 CDP；两张 Terminal PNG 分别只对 UIA 已验证属于本次运行的 Claude/Codex Terminal HWND 调用 `PrintWindow`，不是全屏截图。
- Chrome mock 与 Windows 隔离夹具图片中的会话、消息、路径和 Session ID 都是模拟数据；实机截图只显示本机会话标题。
- 实机截图不在自动脚本中生成；界面或版本变化后应重新截取、逐张审阅，再替换本目录文件。
- Chrome mock 测试不打开 Terminal；Windows 隔离集成测试会创建并逐项清理自己的 Terminal 标签，但不操作已有标签。
- PNG 能证明各自记录的渲染与结构，不能替代官方 CLI 的真实账号恢复验收。`terminal-tabs.svg` 仍只是结构示意图，真实验证记录见 `smoke-tests/README.md`。
- Windows 图片来自对应历史构建的通过轮次，复制前已逐张人工查看；其具体版本、架构和证据范围以当次交付记录为准。

## 重新生成 Chrome 前端 PNG

在项目根目录运行：

```bash
./smoke-tests/run-frontend-smoke.sh
```

成功后，脚本会在 `/tmp` 生成八张图。检查 `FRONTEND_SMOKE_OK` 并逐张审阅后，可复制到本目录：

```bash
cp \
  /tmp/csm-ui-claude.png \
  /tmp/csm-ui-detail.png \
  /tmp/csm-ui-selection.png \
  /tmp/csm-ui-context-menu.png \
  /tmp/csm-ui-update.png \
  /tmp/csm-ui-update-banner.png \
  /tmp/csm-ui-codex.png \
  /tmp/csm-ui-900.png \
  docs/images/
```

如果 UI 或模拟 fixture 改变，应同步检查图片内容、README 说明和测试断言。不要未经验证就把旧截图当作新版本界面。

## 重新生成 Windows 实机 PNG

按 `smoke-tests/windows/README.md` 在已登录的 Windows 桌面中准备全新隔离夹具并运行 `windows-gui-smoke.mjs`。只有退出码为 0、UIA 分别确认 Claude 一个窗口两个标签与随后 Codex 另一窗口一个标签、受限清理完成且逐张审阅图片后，才可复制 `windows-app.png` / `terminal-tabs-claude.png` / `terminal-tabs-codex.png`。不要对混合归属窗口或包含真实凭据、真实会话的屏幕截图。
