# Hermes Desktop 快速截图提案：进程窗口截图 → 标注 → 插入会话

> 日期：2026-08-07 ｜ 状态：**已采纳（同日实施，按 §7 决策修订）**
> 背景：用户在与 agent 对话时经常需要给它看"另一个程序现在长什么样"（报错弹窗、UI 状态、对比截图）。现在只能手动系统截图 → 保存文件 → 拖进 composer。本提案在 composer 右侧功能区加一个快速截图按钮：点击 → 选择当前正在运行的进程窗口 → 截图 → 标注 → 作为图片附件插入会话。
>
> **已拍板的决策（2026-08-07）**：
> 1. v1 截图引擎用 **PrintWindow**（轻量、无新依赖；硬件加速/最小化窗口取空时明确报错）。
> 2. 标注采用 **PDF 批注式**：框选/箭头/画笔工具 + 每个标注自动编号（①②③ 画在图上），描述文字在**侧边栏**直接填写（不设独立文字工具）；发送时图片带编号标记，对应描述文字按编号插入 composer 草稿。
> 3. 选择器用**弹层列表**（进程名+窗口标题）。
> 4. 选择器内含"**整个屏幕**"条目（全屏截图）。

---

## 1. 核心结论：不需要动 Python 后端

**整条链路都在桌面壳内闭环**，后端只会在最终收到一张普通的图片附件（走既有上传通道，后端零感知）：

```
[composer 按钮] → [Rust: 枚举可见窗口] → [渲染端: 进程/窗口选择器]
      → [Rust: 按窗口截图(PNG)] → [渲染端: 标注编辑器(canvas)]
      → [既有 attachImageBlob 插入会话附件]   ← 到这里为止全是既有通道
```

理由：
- 截图目标是**用户本机正在运行的窗口**——即使后端是远程的，该截的也是桌面本机的窗口，所以捕获逻辑天然属于 Tauri/Rust 层。
- WebView2 没有 Electron `desktopCapturer` 的等价物，捕获必须落在 Rust。
- 插入会话走 `use-composer-actions.ts` 已有的 `attachImageBlob`（粘贴/拖放图片就走它），零改动。
- 已确认 Electron 时代**没有**截图功能（electron/ 里只有 mic/camera 权限处理）——这是全新功能，不是迁移补齐。

## 2. 技术方案

### 2.1 Rust 侧（新增两个桥命令，`desktop_misc.rs` 或新 `capture.rs`）

| 命令 | 返回 | 实现 |
|---|---|---|
| `list_windows` | `[{ id, title, process, pid }]` | `windows` crate:`EnumWindows` + `IsWindowVisible` + `GetWindowTextW` + `GetWindowThreadProcessId`；过滤空标题/工具窗口/**排除自家 hermes-desktop 窗口**；按进程名排序 |
| `capture_window(id)` | PNG bytes（base64 或写临时文件返回路径） | v1 用 `PrintWindow(hwnd, PW_RENDERFULLCONTENT)` —— 轻量、同步、无会话管理；对硬件加速窗口个别失效的边角在 v1 接受并文档化，后续可升级 Windows Graphics Capture(`windows-capture` crate) |

两个命令都要进 `permissions/loopback-commands.json`（remote ACL 要求，前面踩过的坑）。

### 2.2 渲染端（新组件 + composer 按钮）

1. **按钮**:composer 右侧功能区（附件/图片按钮组附近）加"截图"图标按钮。
2. **窗口选择器**：弹层列出 `list_windows` 结果（进程名 + 窗口标题；可选：进程图标）。搜索过滤非必需，列表按进程分组即可。
3. **标注编辑器**：截图拿到后进入轻量 canvas 标注——矩形/箭头/文字/自由画笔 + 撤销 + 完成/取消。自绘 canvas（~300 行），不引第三方标注库（依赖纪律）。
4. **插入**：标注结果 `canvas.toBlob()` → `attachImageBlob(blob)`（既有函数，内部已处理保存/上传/附件 pill)。

### 2.3 数据流

```
list_windows() ──► 选择器 UI ──► capture_window(hwnd) ──► base64 PNG
                                                              │
标注编辑器 ◄──────────────────────────────────────────────────┘
   │ 完成
   ▼
canvas.toBlob() → attachImageBlob() → 附件 pill → 随消息发送(既有上传链路)
```

## 3. 边界与约束

- **权限**:Windows 截其他应用窗口无需系统授权（与 macOS 的屏幕录制权限不同），无额外引导。
- **自家窗口排除**:`list_windows` 必须过滤 hermes-desktop 自身（否则用户能选到 app 自己/弹窗，套娃截图没意义还可能有副作用）。
- **最小化/遮挡窗口**:`PrintWindow` 对最小化窗口可能返回空——v1 对捕获失败返回明确错误（"窗口可能已最小化，请恢复后重试")，不做强行还原。
- **远程后端**：功能截的是本机窗口，与后端位置无关；`attachImageBlob` 的保存/上传链路已处理远程情形。
- **安全面**:`capture_window` 只接受 `list_windows` 枚举出的 hwnd 类标识；命令只出不进（返回图片），不写文件系统（或只写临时目录）。
- **多显示器/DPI**:`PrintWindow` 按窗口实际尺寸取图，无需额外处理；标注编辑器按 DPR 缩放显示。

## 4. 为什么不做成别的样子

- **不动 Python 后端**：见 §1——目标窗口在本机，后端可能在远端，把截图放后端是错的。
- **不引 `windows-capture`(WGC)做 v1**:WGC 能处理遮挡/硬件加速边角但引入会话管理与更大依赖；`PrintWindow` 覆盖 80% 场景，v1 先轻。留升级路径。
- **不引第三方标注库**（fabric/markerjs)：需求只有矩形/箭头/文字/画笔，自绘 canvas 可控且无依赖膨胀。
- **不做"区域截图"**：那是另一套交互（屏幕覆盖层选区），可作为后续迭代；窗口截图已覆盖用户诉求（"选择正在运行的进程")。

## 5. 测试计划

- **Rust**:cargo test 暂不适用（EnumWindows/PrintWindow 需真实窗口）；以手测+CDP 验证代替——构造：打开记事本 → `list_windows` 应含 notepad → `capture_window` 返回非空 PNG(base64 可解码、尺寸>0)。
- **渲染端 vitest**：选择器过滤逻辑（空标题/自家窗口排除）、标注编辑器的撤销栈逻辑、插入调用 attachImageBlob 的断言。
- **CDP e2e**：桥调用两命令 + 附件 pill 出现。

## 6. 工作量拆解

| 项 | 内容 | 量级 |
|---|---|---|
| 1 | Rust `list_windows` + `capture_window` + 权限登记 | ~150 行 Rust |
| 2 | 窗口选择器弹层 | ~120 行 TSX |
| 3 | 标注编辑器（canvas,4 工具+撤销） | ~300 行 TSX |
| 4 | composer 按钮 + 流程串联 + i18n | ~80 行 |
| 5 | vitest + CDP 验证 | ~120 行 |

合计约 1 天（含例行 typecheck/vitest/版本号/MSI)。

## 7. 决策点（需你拍板）

1. **v1 截图引擎**:`PrintWindow`（推荐，轻）还是直接上 Windows Graphics Capture（覆盖边角但重）？
2. **标注工具集**：矩形/箭头/文字/画笔四件套（推荐）还是更精简（仅矩形+文字）？
3. **选择器形态**：弹层列表（推荐）还是系统级"点选窗口"模式（移动十字光标选窗，交互更酷但实现复杂）?
4. 是否需要同时支持"全屏截图"选项（加一个"整个屏幕"条目）?

---

*提案完毕，未实施。批准后按 §6 顺序实施，走例行验证 + MSI 出包。*
