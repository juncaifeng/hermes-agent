# Hermes Desktop：Electron → Tauri v2 迁移映射表

> 目标：把 `apps/desktop`（Electron 40 + React 19）迁移到 Tauri v2（Rust 主进程 + WebView2）。
> 本文档是迁移的**起步清单**，逐项给出 `window.hermesDesktop` 桥（`apps/desktop/electron/preload.ts`）126 个可调用项的迁移方案。
> 生成日期：2026-08-05。

> **迁移状态（2026-08-06 更新）：** 126 个可调用项已全部迁移完毕。桥层 `apps/desktop/src/desktop-bridge.ts` 的 `notMigrated` 哨兵计数 **62 → 0**。处理方式分三类：(1) **Rust 命令真实实现**——后端连接/引导/终端/连接配置/剪贴板(arboard)/文件对话框(tauri-plugin-dialog,返回形状对齐 `HermesSelectPathsOptions` 契约)/fs 操作(trash+open,rename 含安全校验)/日志 tail/链接标题(scheme 白名单+禁 redirect+私网段拒绝+512KiB 上限)/插件根/默认项目目录；(2) **Tauri 等价物**——`on*` 事件走 `listen()`、`api` 代理走 `invoke('api')`；(3) **安全降级 no-op**——纯桌面窗口专属能力(petOverlay 桌面宠物、quickEntry 快速输入、updates/uninstall 升级卸载、主题市场、wakeIndicator、zoom、findInPage、麦克风预检等)在 WebView2 场景无对应物，桥层返回不 reject 的安全形状，渲染端零改动。验证：`cargo check`/`cargo build --release` 干净通过、`tsc --noEmit` 通过、release 构建 CDP 实测 14 项代表性方法工作；review 驱动的 4 项返回形状修正(selectPaths→string[]、selectSavePath→null|string、renamePath→{path}、setDefaultProjectDir→{dir})已并入并在 review 复审通过。未实测项：`fetchLinkTitle`(依赖外网)、`selectPaths`/`selectSavePath`(阻塞对话框,无法无头验证)。

> **后端打包（2026-08-06）：** MSI 通过 `bundle.resources` 内置 PyInstaller 打包的 `hermes-backend`（`scripts/bundle-backend.sh`，uv venv py3.13 + hermes 主依赖 editable 安装）。`gateway.rs::resolve_hermes_binary` 优先级：**bundled `<resource_dir>/hermes-backend/hermes-backend.exe`** → `HERMES_DESKTOP_HERMES` → `%USERPROFILE%\.local\bin\hermes.exe` → PATH。目标机零依赖离线可用（已验证：裸 exe 带 resource 启动后后端进程为 `hermes-backend.exe`，WS accepted）。产物约 83MB（exe 26.6MB + _internal），MSI 总计 ~66MB。重建后端：`bash apps/desktop/scripts/bundle-backend.sh`（hermes 禁 wheel 构建，必须 `-e` editable 安装；PyInstaller 用 wrapper `entry.py`，6.x 无 `--entry-point`）。**信任链注意：`hermes-backend.exe` 与 MSI 当前未签名**，用户机 SmartScreen/Defender 可能告警/拦截，发布前需对两者做代码签名。

**发布前安全清单（security_review 2026-08-06，2 HIGH 待发布前处理）：**
1. **代码签名**：对 `hermes-backend.exe` 与 MSI 做 Authenticode 签名（未签名二进制 + per-user 安装 → 资源目录对同用户可写，恶意替换后应用每次启动执行任意代码）。
2. **spawn 前校验**：可选——`gateway.rs` spawn bundled exe 前做 WinVerifyTrust 签名校验，或改为 `perMachine` 安装 + 资源目录 ACL 只读。
3. **供应链已收紧**：`bundle-backend.sh` 改用 `uv sync --frozen`（消费 `uv.lock`，`[tool.uv]` 的 override-dependencies/exclude-newer 生效），pyinstaller 固定 `==6.14.2`；构建机应隔离并校验锁文件。

**离线模型目录种子（2026-08-06，修复 MSI 后"模型厂商列表/目录为空"）：**
- `hermes_cli/model_catalog.py::_read_bundled_seed` — 冷缓存+网络失败时回退到打包的 `hermes_cli/model-catalog.json`（源：`website/static/api/model-catalog.json`，openrouter/nous 精选目录），并写入磁盘缓存。
- `agent/models_dev.py::_load_disk_cache` — 冷缓存时回退到打包的 `agent/models_dev_cache.json`（源：构建机 `$HERMES_HOME/models_dev_cache.json`，完整 models.dev 能力库 ~3.5MB；构建机无缓存则跳过）。
- 打包脚本用 `--add-data` 注入两个种子（目标为包目录 `hermes_cli`/`agent`，种子文件名与源一致——`hermes_cli/model-catalog.json`、`agent/models_dev_cache.json`；注意 PyInstaller `--add-data SRC;DEST` 的 DEST 是目录，伪文件名目标会生成目录导致 `is_file()` 恒 False，这是初版 blocking 的教训；路径经 `cygpath -w` 转 Windows 形式，否则 MSYS `/e/...` 被解析到错误盘符）。已验证：产物种子为 JSON 文件（6830B + 3562473B），模拟打包布局（种子文件在模块旁）下 `_read_bundled_seed` 返回 2 providers、`_load_disk_cache` 返回 180 providers。
- **厂商列表（`/api/model/options`）显示逻辑**：仅显示已配置凭据的 provider + 虚拟 provider（`include_unconfigured`/`explicit_only` 默认关闭，渲染端既有行为）；全新机器无凭据时只有 moa/copilot 属预期——配置凭据后厂商即出现（打包后端读取 `$HERMES_HOME/.env`/config.yaml 已验证正常）。

**推理 provider 依赖打包（2026-08-06，修复 MSI 后 "agent init failed: The 'anthropic' package is required"）：**
- hermes 的 provider 专属 SDK（anthropic 等）是 **lazy-install**（`tools/lazy_deps.py` 运行时 `pip install`），PyInstaller 密封产物无法运行 pip → 报错。
- 修复：打包 venv `uv sync --frozen --extra anthropic --extra exa --extra firecrawl --extra fal --extra vercel --extra hindsight` 预装推理 provider SDK；且 PyInstaller 加 `--copy-metadata <6 个包>` 复制 dist-info，否则 `lazy_deps._is_satisfied` 的 `importlib.metadata.version()` 查不到版本会误判缺失。
- 已验证：产物 `_internal/` 含 6 个 provider dist-info（anthropic-0.87.0 等）；模拟打包布局（_internal 入 sys.path）下 `metadata.version("anthropic")=0.87.0` 满足 `==0.87.0`，lazy_deps 判定已装不再触发安装。
- 未打包：bedrock（boto3）/vertex（google-auth）/azure-identity 等重 provider extra——用户选用时仍会走 lazy-install 失败，需按需补充（体积大）；messaging/voice/wake 同理。

**后端分发路线切换（2026-08-06，uv 引导方案替代 PyInstaller 默认）：**
- 背景：PyInstaller 打包链复杂（extras/copy-metadata/种子/cygpath 坑）+ provider 支持不全；而 `hermes-agent` **已在 PyPI**（0.20.0，`uv tool list` 验证），发布尝试 403（token 用户无项目权限）反而确认了"用现有官方包"路线。
- 新方案（默认）：**MSI 只打前端（17.4MB）**，`gateway.rs` 首次启动引导——`resolve_hermes_binary` 失败 → `bootstrap_backend`：找 uv（PATH + `~/.local/bin`/`~/.cargo/bin`/WinGet Links）→ `uv tool install --force hermes-agent` → 重新 resolve → spawn；boot progress 新增 `backend.installing` 阶段。
- **镜像 fallback**：PyPI 直连在中国网络常失败（tls handshake eof，本机实测）→ 直连失败后自动用 `UV_DEFAULT_INDEX=https://pypi.tuna.tsinghua.edu.cn/simple` 重试（实测 TUNA 装成功）。
- 已验证：隐藏本机 hermes 后启动，bootstrap 自动安装后端并 `backend ready` + WS accepted；MSI 17.4MB（从 66.7MB 降）。
- PyInstaller 脚本（`scripts/bundle-backend.sh`）**保留**（离线 `--bundled` 变体，tauri.conf.json 加回 resources 即可），暂不删除，后续按需维护。
- 待办：无 uv 时的下载引导（当前报错提示安装 uv）；uv.exe 是否随 MSI（用户决策：先检查系统环境，已实现 resolve_uv 检测系统已有）。

**日志路径不一致修复（2026-08-06）：** Rust 侧 `desktop_misc.rs::hermes_home()` 的默认回退原为 `~/.hermes`，而 Python 后端 `hermes_constants._get_platform_default_hermes_home()` 在 Windows 默认 `%LOCALAPPDATA%\hermes`——导致桌面"Open logs"打开空目录、`get_recent_logs` 读不到日志。已对齐为 Windows `%LOCALAPPDATA%\hermes`（HERMES_HOME 环境变量优先），非 Windows `~/.hermes`。

**前端改走 loopback HTTP 伺服（2026-08-07，替代 tauri:// 自定义协议，解决 release 版"Could not connect to Hermes gateway"）：**
- **根因**：打包后 renderer 从 `tauri://localhost` 加载，WebView2 映射为 `http://tauri.localhost`，WS 握手携带该 Origin；后端 DNS-rebinding 白名单（`_LOOPBACK_HOSTS`）不含它 → 403 origin_mismatch → 前端 boot 失败。dev 模式（vite 在 localhost）天然放行所以全程没暴露；Electron 的 `file://` 走非 web 来源豁免所以也从未触发。仓库 `web_server.py` 的白名单修复（tauri.localhost）虽已提交分支，但**任何已分发/旧版后端都没有它**——在线安装路线不可依赖。
- **方案**：不改后端、不依赖后端版本——Rust 内嵌静态服务器（`src/asset_server.rs`，tiny_http+mime_guess，loopback only、防目录穿越、SPA 回退、按类型加 charset），把 `dist/` 挂在 `http://127.0.0.1:<随机端口>`；窗口 URL 改在代码里创建时指向它（`lib.rs`，dev 仍用 devUrl）。WS Origin 变为 `http://127.0.0.1:<port>`——端口剥离后命中 loopback 白名单，**任何版本后端（含未修复的 0.19.0/0.20.0）都放行**（已实测）。`windows.rs` 的 session/instance 弹窗共用同一 `base_url()`，全部窗口同 origin。附带收益：页面不再是"远程安全源"，WebView2 混合内容自动升级不再把 `ws://` 改写成 `wss://`，`tls_proxy.rs` 在本地后端路径彻底不需要（目前本就是未接线死代码）。
- **ACL 配套（关键坑）**：loopback origin 对 Tauri ACL 是 **remote 上下文**——(1) `capabilities/default.json` 加 `remote.urls: ["http://127.0.0.1:*", "http://localhost:*"]`（URLPattern，端口通配已实测匹配任意端口）；(2) **自定义命令在 remote 上下文默认全拒**（webview/mod.rs：`!is_local && acl.is_none()` → reject），必须建 app 权限清单：`permissions/loopback-commands.json`（68 个桥命令全列，文件结构必须是 `{"permission": [...]}`——顶层单权限对象会被 serde 静默丢弃，是本次最大坑）；(3) capability 里**裸标识符**引用（`"loopback-commands"`，无前缀 = app 清单，`app:` 前缀会落到 core app 插件）；(4) `windows` 列表补 `session-*`/`instance-*` glob（原来只有 main，弹窗 IPC 本就会被拒）。`lib.rs` invoke_handler 上方已加注释：**新命令必须同步进 permissions/loopback-commands.json**。
- **tauri.conf.json**：`app.windows` 置空（窗口代码化创建）；`bundle.resources: ["../dist"]`（MSI 把 dist 铺到安装目录，`resolve_dist_dir` 先查 resource_dir 再回退 exe 同级——裸 exe 测试时把 dist 拷到 exe 旁即可）。
- **已验证**：`cargo check` dev+release 双 profile 干净；release exe 实测——asset server 200/SPA 回退/穿越 403/MIME 正确；CDP 实测页面 href=`http://127.0.0.1:<port>/#/`、composer 可见、无失败指示、真实会话列表加载（`e2e/check-loopback-origin.mjs`，绿灯）。
- **遗留**：frontendDist 内嵌与 resources 双份体积（~6-10MB 压缩）后续可优化掉内嵌；tls_proxy 死代码待清理或留作远程 wss 场景；上游 `web_server.py` tauri.localhost PR 仍应提交（救其他 tauri:// 消费方），但不再是发布阻塞项。

**MSI 打包修正（2026-08-07，安装版"Could not connect"根因）：** 初版 `"resources": ["../dist"]` 数组格式会把 `../` 映射为安装目录下的 `_up_/dist`，`resolve_dist_dir` 找不到 → 回退 tauri:// → 撞上 wss 混合内容自动升级 + Origin 403 双重墙。修为**对象格式** `"resources": { "../dist": "dist" }`（dist 直接落 `<install>/dist`，wxs 目录树已确认无 `_up_`）；顺带版本号 0.17.0 → 0.18.0（同版本 MSI 无法干净升级，且全机安装需提权——非提权静默装 0.x 同版会 1603/MSI_LUA 拒绝）。0.18.0 MSI 已在多台机器实装验证可用。构建命令固化在 `apps/desktop/Makefile`（本机无 `make`，用 `mingw32-make -C apps/desktop <target>`）。

**渲染端两个 shim/路径修复（2026-08-07，0.18.1/0.18.2）：**
- `normalizePreviewTarget` shim 原样返回输入字符串（truthy 谎言）→ 渲染端跳过 `localPreviewTarget` 回退，裸字符串被当 PreviewTarget → preview 面板 `Cannot read properties of undefined (reading 'split')`。修为返回 `null`（类型本就允许），分类交还渲染端本地回退。教训：no-op shim 的返回形状必须**诚实**——truthy 占位值比显式拒绝更危险。
- `filePathForTarget` 的 `file://` 解码在 Windows 产生 `/E:/...` 前导斜杠路径（URL 语法的一部分，不是路径的一部分）→ Rust `fs::read` 报 `os error 123`（预览不可用）。修为对盘符路径剥前导斜杠（`/^\/[A-Za-z]:[\\/]/`）。该路径来自 url-only 的 preview target（聊天内文件链接/工具结果）；Electron 时代由 normalizePreviewTarget 归一化掩盖了它。
- 另注意：`npm run build` 会把 Electron 专用的 node-pty 塞进 `dist/node_modules`，Tauri 拒绝打包含 node_modules 的 frontendDist——`Makefile msi` 已内置剥除。
- 0.18.3（2026-08-07）：`localPreviewTarget` 补齐 Windows 路径形态——file:// 解码剥盘符前导斜杠；`E:\`/`E:/`/UNC 判定为绝对路径不再误拼 cwd（原先 `E:\x` 被当相对路径拼出 `cwd/E:\x` 垃圾路径，同样 os error 123）。新增 `src/lib/local-preview.test.ts` 六项形态契约测试（vitest 全过）。后续可选：把 `normalizePreviewTarget` 实现为真正的 Rust 命令（含 stat/二进制嗅探），彻底消除对渲染端回退的依赖。
- 0.18.4（2026-08-07）：**冷 resume 不同步工作区 cwd**——`resumeSession` 只在暖缓存路径 `setCurrentCwd(cachedViewState.cwd)`；冷路径（无缓存，典型：app 重启后首次切换到另一项目的会话）只 `patchSessionWorkspace`/`updateSessionState`，从不动 `$currentCwd`，导致文件树/git review 停在旧项目。修为冷路径同步 `runtimeInfo?.cwd || stored?.cwd`（与暖路径同源）。附 `use-session-actions.test.tsx` 两个用例（runtime info 有/无 cwd 两种，42 全绿）。注意：这是 Electron 时代就存在的上游缺口，非 Tauri 回归。
- 0.18.5（2026-08-07）：**子代理弹窗（session/instance 二级窗口）全空白**——`open_session_window`/`open_window` 是 sync command，跑在主线程且正处 WebView2 IPC 消息处理器内，嵌套创建第二个 webview 死锁：`build()` 不返回、invoke 永不 resolve、新窗永远停在 about:blank（连 CDP `Page.navigate` 都无响应）。修为 **async command + `run_on_main_thread` 投递创建**（Tauri 官方对多窗口的推荐姿势，[#13092](https://github.com/tauri-apps/tauri/issues/13092)）；裸 CDP 实测弹窗正确导航并渲染子代理会话内容。顺带修 `on_window_event` 的相邻 bug：原来**任何**窗口 Destroyed 都 `stop_gateway`——关个弹窗会把主窗口还在用的后端杀掉，现限定只有 main 窗口销毁才回收。教训：凡在 command 里建窗口，一律 async + run_on_main_thread；验证弹窗类 bug 时 playwright 的 connectOverCDP 在本机环境不稳，裸 WebSocket CDP（Node 22 原生 WebSocket）是可靠的替代探针。

## 0. 通用范式（先定三条捷径，可省掉大半工作量）

- **桥垫片**：先写一个 `window.hermesDesktop` 适配层，把 `invoke` 一对一映射到 `@tauri-apps/api/core.invoke`，事件映射到 `listen()`。方法名/签名全部保留，`src/` 下 84 处调用点零改动。
- **通用 `api` 代理**：`hermes:api` 是通用 REST 代理（转发到后端 `hermes serve`），用 `tauri-plugin-http` 或原生 `fetch` 即可，是最大的平移面。
- **`on*` 事件**：全部归为 Tauri event，用 `@tauri-apps/api/event.listen` 注册，事件名保持 `hermes-<n>` 风格。

### 术语约定

| Electron | Tauri v2 |
|---|---|
| `ipcRenderer.invoke(channel, ...args)` | `invoke(channel, args)`（`@tauri-apps/api/core`） |
| `ipcRenderer.on(channel, cb)` | `listen(channel, cb)`（`@tauri-apps/api/event`） |
| `ipcRenderer.send(channel, ...)` | `emit(channel, ...)` |
| `contextBridge.exposeInMainWorld` | `#[tauri::command]` + `invoke` |
| `webContents.findInPage` | Rust `webview` find API 或 JS 自实现 |
| `node-pty` | `portable-pty` crate 或 Headless Node sidecar |

---

## 1. 后端进程 / 连接

| IPC 方法 | Tauri 方案 | 备注 |
|---|---|---|
| `getConnection` / `revalidateConnection` / `touchBackend` / `getGatewayWsUrl` | `#[tauri::command] backend_connection` | 迁 `backend-command.ts` |
| `getConnectionConfig` / `saveConnectionConfig` / `applyConnectionConfig` / `testConnectionConfig` / `probeConnectionConfig` | `#[tauri::command] connection_config` + `tauri-plugin-store` | |
| `onConnectionApplied` | Tauri event | 软切换后广播 |
| `api(request)` | `tauri-plugin-http` 或原生 `fetch` | 通用后端 REST 代理，最大平移面 |
| `getVersion` / `getRemoteDisplayReason` | `#[tauri::command]` | 读 build 元数据 |

## 2. 后端进程生命周期（sidecar）

| IPC 方法 | Tauri 方案 | 备注 |
|---|---|---|
| `onBackendExit` | Tauri event（sidecar 退出钩子） | |
| `signalDeepLinkReady` | `tauri-plugin-deep-link` | |

> 后端 `hermes serve` 走 **sidecar**（`tauri.conf.json` externalBin），`tauri-plugin-shell` 管理 spawn/退出。对应要移植：`backend-child.ts`、`primary-backend-startup.ts`、`backend-ready.ts`、`backend-health.ts`、`backend-probes.ts`。

## 3. 窗口 / 多窗口 / Overlay

| IPC 方法 | Tauri 方案 | 备注 |
|---|---|---|
| `openSessionWindow(sessionId, opts)` | `#[tauri::command] open_session_window` | 多 WebviewWindow |
| `openWindow()` | 同上（新建主窗口） | |
| `onClosePreviewRequested` / `onOpenFolderRequested` / `onOpenUpdatesRequested` / `onFocusSession` / `onWindowStateChanged` | Tauri events | |
| `setTitleBarTheme` / `setTranslucency` | `tauri-plugin-window` + `#[tauri::command]` | 迁 `window-state.ts`、`titlebar-overlay-width.ts` |
| `petOverlay.*`（open/close/setBounds/setIgnoreMouse/setFocusable/pushState/control/onState/onControl） | `#[tauri::command] pet_overlay_*` + events | 透明置顶副窗口 + 拖拽 |
| `wakeIndicator.*`（getState/setState/onState） | `#[tauri::command] wake_indicator_*` + event | 迁 `wake-indicator-window.ts` |
| `quickEntry.*`（getSettings/setSettings/submit/dismiss/pushState/onState/onSubmit/onShown） | `#[tauri::command] quick_entry_*` + events | 全局热键副窗口，`tauri-plugin-global-shortcut` 注册热键 |
| `claimAmbientCue` | `#[tauri::command]` | 后台提示去重 |

## 4. 文件系统 / 预览

| IPC 方法 | Tauri 方案 | 备注 |
|---|---|---|
| `readDir` / `readFileText` / `writeTextFile` / `renamePath` / `trashPath` / `readFileDataUrl` / `readFileDataUrlForAttach` | `tauri-plugin-fs`（+ `#[tauri::command]` 包装） | `trash` 用 `tauri-plugin-fs` 或 `trash` crate |
| `gitRoot` / `revealPath` / `openDir` / `desktopPluginsRoot` | `#[tauri::command]` | 迁 `fs-read-dir.ts`、`git-root.ts` |
| `selectPaths` | `tauri-plugin-dialog` | 文件选择 |
| `normalizePreviewTarget` / `sanitizeWorkspaceCwd` | `#[tauri::command]` | 迁 `workspace-cwd.ts` |
| `watchPreviewFile` / `watchDirectory` / `stopPreviewFileWatch` / `onPreviewFileChanged` | `tauri-plugin-fs` watch API + event | |
| `getPathForFile` | `tauri-plugin-dialog`（`webUtils` 等价物） | 拖放文件取路径 |
| `saveImageFromUrl` / `saveImageBuffer` / `saveClipboardImage` | `#[tauri::command]` + `tauri-plugin-dialog` | 迁 `media.ts` |
| `readClipboard` / `writeClipboard` | `tauri-plugin-clipboard-manager` | |
| `dataUrlReadMax.get/set` | `#[tauri::command]` + `tauri-plugin-store` | |
| `fetchLinkTitle` | Rust `reqwest` | 迁 `link-title-window.ts` |

## 5. Git 操作

| IPC 方法 | Tauri 方案 | 备注 |
|---|---|---|
| `git.worktreeList/Add/Remove`、`branchSwitch/List`、`baseBranchList`、`repoStatus`、`fileDiff`、`scanRepos` | `git2` crate 或调用系统 git（`tauri-plugin-shell` `Command`） | 迁 `git-worktree-ops.ts`、`git-repo-scan.ts`、`git-root.ts` |
| `git.review.*`（list/diff/stage/unstage/revert/revParse/commit/commitContext/push/shipInfo/createPr） | `git2` crate / shell `Command` | 迁 `git-review-ops.ts` |

## 6. 终端（node-pty — 最大障碍）

| IPC 方法 | Tauri 方案 | 备注 |
|---|---|---|
| `terminal.start/write/resize/cwd/dispose/onData/onExit` | Rust `portable-pty` 或 Node sidecar | **迁移前必须决策的技术选型**：① `portable-pty` crate 重写（合入主进程，性能好）；② 保留 Headless Node 子进程跑 node-pty，Tauri 通过 sidecar 管理 |

## 7. 系统能力（主题/电源/缩放/通知/麦克风）

| IPC 方法 | Tauri 方案 | 备注 |
|---|---|---|
| `setNativeTheme` | `tauri-plugin-window`（theme API） | |
| `setKeepAwake` | `tauri-plugin-power` 或 Rust `power` crate | 迁 `power-save.ts` |
| `getOnBattery` / `onBatteryChanged` / `onPowerResume` | Rust `battery`/`sysinfo` crate + event | 迁 `power-save.ts` |
| `zoom.get/setPercent/onChanged` | `#[tauri::command] zoom_*` + event | 迁 `zoom.ts` |
| `findInPage` / `stopFindInPage` / `onFoundInPage` | `#[tauri::command]` + Rust `webview` find API 或 JS 自实现 | 迁 `find-in-page.ts` |
| `setActiveWork` | `#[tauri::command]` | 活跃工作区透传 |
| `setPreviewShortcutActive` | `tauri-plugin-global-shortcut` | |

## 8. 通知 / 麦克风 / 日志 / 引导

| IPC 方法 | Tauri 方案 | 备注 |
|---|---|---|
| `notify` / `onNotificationAction` | `tauri-plugin-notification` + event | |
| `requestMicrophoneAccess` | Rust 权限或 WebView 权限请求 | |
| `revealLogs` / `getRecentLogs` | `#[tauri::command]` | 读 `HERMES_HOME/logs` |
| `onBootstrapEvent` / `getBootstrapState` / `continueBootstrapLocal` / `resetBootstrap` / `repairBootstrap` / `cancelBootstrap` / `getBootProgress` / `onBootProgress` | `#[tauri::command] bootstrap_*` + events | 迁 `bootstrap-runner.ts`、`bootstrap-platform.ts`、`first-run-setup-gate.ts` |

## 9. 认证 / OAuth / Cloud

| IPC 方法 | Tauri 方案 | 备注 |
|---|---|---|
| `sshConfigHosts` / `sshResolveHost` | `#[tauri::command]` + 读 `~/.ssh/config` | 迁 `ssh-config.ts` |
| `oauthLoginConnectionConfig` / `oauthLogoutConnectionConfig` | Rust `oauth2` crate + 系统浏览器 | 迁 `native-oauth*.ts`、`oauth-net-request.ts` |
| `cloud.status/login/logout/discover/agentSignIn` | `#[tauri::command] cloud_*` | |
| `profile.get/set` | `#[tauri::command]` + `tauri-plugin-store` | 迁 `profile-session-routing.ts` |
| `native-token-store` / `native-auth-decisions` | Rust `keyring` crate | 敏感令牌安全存储 |

## 10. 更新 / 卸载 / 设置

| IPC 方法 | Tauri 方案 | 备注 |
|---|---|---|
| `updates.check/apply/getBranch/setBranch/onProgress` | `tauri-plugin-updater` + 自定义发布源 | 协议/签名不同，需对接分发服务器 |
| `uninstall.summary/run` | `#[tauri::command]` + `tauri-plugin-process` | 迁 `desktop-uninstall.ts` |
| `settings.get/set/pickDefaultProjectDir` | `#[tauri::command]` + `tauri-plugin-dialog` + store | |
| `themes.fetchMarketplace/searchMarketplace` | `reqwest`（迁 `vscode-marketplace.ts`） | |

## 11. 通用事件订阅（全转 Tauri event）

`onDeepLink`、`onWindowStateChanged`、`onFocusSession`、`onNotificationAction`、`onPreviewFileChanged`、`onBackendExit`、`onConnectionApplied`、`onPowerResume`、`onBatteryChanged`、`onBootProgress`、`onBootstrapEvent`、`onClosePreviewRequested`、`onOpenFolderRequested`、`onOpenUpdatesRequested`

→ 统一 `@tauri-apps/api/event.listen`，事件名保持 `hermes:*` 风格，签名不变。

## 12. 打包 / 构建（非 IPC，但必须改）

- electron-builder → `tauri.conf.json` + WiX（本机已有 WiX Toolset v3.14）
- auto-update → `tauri-plugin-updater`
- `node-pty` 原生依赖 → Rust 依赖
- 需重写的 `scripts/`：`run-electron-builder.mjs`、`before-build/pack/after-pack.mjs`、`notarize*.mjs`、`stage-native-deps.mjs`、`set-exe-identity.mjs`、`rebuild-native.mjs`、`write-build-stamp.mjs`
- 测试：`e2e/*.spec.ts`（Playwright+Electron）→ Tauri WebDriver；vitest electron 工程 → 重写；**ui 工程可复用**

---

## 工作量评估

| 类别 | 项数 | 难度 | 关键点 |
|---|---|---|---|
| 通用代理/事件（0、11） | ~30 | 低 | 平移面大，垫片即可 |
| 文件/Git/设置（4、5、10） | ~40 | 中 | `tauri-plugin-fs/shell/dialog` + `git2` |
| 窗口/Overlay（3） | ~25 | 中 | 多窗口 + 置顶透明窗较繁琐 |
| 认证/Cloud（9） | ~12 | 中 | `keyring` + `oauth2` |
| 引导/更新（8、10） | ~15 | 高 | `bootstrap-runner` 逻辑复杂 |
| **终端 node-pty（6）** | 7 | **极高** | Rust pty vs Node sidecar，**需前置决策** |
| 主进程 main.ts（跨多类） | — | 极高 | ~11000 行拆 Rust 是最大单点 |

## 建议落地顺序

1. 桥垫片（第 0 类）
2. 通用代理 / 事件（0、11）
3. 文件 / Git / 设置（4、5、10）
4. 窗口 / Overlay（3）
5. 认证 / Cloud（9）
6. 引导 / 更新（8、10）
7. **最后单独攻坚终端 node-pty（6）**

---

## 待决策项（前置）

- [ ] 终端方案：`portable-pty` crate vs Headless Node sidecar
- [ ] 更新分发服务器：`tauri-plugin-updater` 发布源对接
- [ ] Rust 侧 Git：`git2` crate vs 调用系统 git
- [ ] 迁移范围：是否保留 `window.hermesDesktop` 垫片长期共存，还是彻底替换