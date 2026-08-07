# Hermes Desktop 后端分发方案提案：uv tools（PyPI 实时安装）vs PyInstaller 预打包

> 日期：2026-08-06 ｜ 状态：**已采纳为默认方案（同日实施）**；PyInstaller 保留为离线 `--bundled` 变体
> 背景：MSI 后端分发当前用 PyInstaller 预打包（`scripts/bundle-backend.sh`），体积 66.7MB、构建链复杂（provider extras / `--copy-metadata` / 模型种子 / cygpath 路径坑），且 sealed 产物里 hermes 的 lazy-deps（运行时 `pip install` provider SDK）失效，需要逐个预打包 provider。本提案评估改用 **uv tools 实时安装**（后端发布到 PyPI，安装时 `uv tool install` 即装即用）是否更优。

---

## 1. 提案方案

**不再把 Python 后端打进 MSI**。改为：

1. **后端发布到 PyPI**（`hermes-agent`，release 流程已存在——本机 `uv tool install hermes-agent` 装的就是 PyPI 上的 v0.20.0）。
2. **MSI 只含前端** + 一个轻量引导（`uv.exe`，官方 standalone ~20MB，或要求目标机预装 uv）。
3. **首次启动**：`gateway.rs` 检测 `%USERPROFILE%\.local\bin\hermes.exe`（uv tool 安装位置）——不存在则**引导安装**（`uv tool install hermes-agent`，带进度 UI / 失败提示），然后照常 spawn `hermes serve`。
4. **日常运行**：与现状回退路径一致（`gateway.rs` 现有优先级已含 `%USERPROFILE%\.local\bin\hermes.exe` → PATH），仅新增"不存在时先安装"一步。

## 2. 可行性核查（已确认）

| 前提 | 状态 | 证据 |
|---|---|---|
| `hermes-agent` 已在 PyPI | ✅ | 本机 `uv tool list`：`hermes-agent v0.20.0`（PyPI 安装，非本地 editable） |
| uv tool 环境可 lazy-install provider | ✅ | uv tool venv 有 pip/ensurepip，`tools/lazy_deps.py` 运行时安装正常（开发环境同机制） |
| gateway.rs 已支持 uv tool 路径 | ✅ | `resolve_hermes_binary` 优先级 2 = `%USERPROFILE%\.local\bin\hermes.exe` |
| 后端版本随 PyPI 更新 | ✅ | `uv tool upgrade hermes-agent`（或 hermes 自带 update 子命令） |

> 注：pyproject 的 "Building wheels or sdists is not supported" 是**本地源码构建拦截**（Nix/uv2nix 场景防护），不阻止 PyPI 正式发布物——发布走现有 CI 流程即可，无需改 pyproject。

## 3. 与现状（PyInstaller）对比

| 维度 | PyInstaller 预打包（现状） | uv tools 实时安装（提案） |
|---|---|---|
| MSI 体积 | 66.7MB（含后端 90MB 源） | ~20MB（前端 + uv.exe）或更小 |
| 离线安装（无 PyPI 访问） | ✅ 开箱即用 | ❌ 首次必须联网（PyPI） |
| 首次启动到可用 | 秒级（本地 spawn） | 1-3 分钟（下载 + 安装 hermes 依赖） |
| provider 依赖（anthropic 等） | 需预打包 extras + copy-metadata（6 个已做；bedrock/vertex 等未做仍报错） | ✅ lazy-deps 自然工作，任何 provider 按需安装 |
| 模型目录种子（离线） | 需打包种子（已做） | 首次联网安装后目录自动下载；**离线仍无** |
| 后端版本 | 随 MSI 冻结（需重打包发布） | 随 PyPI（`uv tool upgrade`） |
| 构建复杂度 | PyInstaller 链（extras/copy-metadata/种子/cygpath 坑/重打包慢） | 极低（无后端打包） |
| 磁盘占用（目标机） | 后端 ~90MB 随 MSI | hermes venv ~200-400MB（含完整依赖树） |

## 4. 风险与缓解

| 风险 | 影响 | 缓解 |
|---|---|---|
| **目标机无法访问 PyPI**（内网/离线/被墙） | 后端装不上，应用不可用 | ① 混合方案（见 §6）；② 企业可自建 PyPI 镜像/私有源（`uv tool install --index-url`）；③ 引导期明确报错 + 手动安装指引 |
| 首次启动等待 1-3 分钟 | 体验下降 | 安装进度 UI（复用 boot 进度事件）；后台安装不阻塞界面；安装完成后自动 continue boot |
| uv 未安装 | 目标机无 uv | MSI 内置 uv.exe（~20MB）或首次引导下载（需网络） |
| 版本漂移（后端新于前端） | 偶发兼容问题 | 前端与后端版本协商（gateway 检测版本，不匹配提示升级）；发布节奏对齐 |
| 供应链（PyPI 投毒） | 与 pip 生态同风险 | 复用 hermes 现有 exact-pin + uv.lock；`uv tool install` 哈希校验（uv.lock 已含） |
| 后端依赖解析失败（新机器缺编译工具链） | 安装失败 | hermes 主依赖全 wheel（无编译需求）；重型 provider 走 lazy-install（首次使用才装） |

## 5. 构建/运行时差异

```bash
# 现状构建
bash apps/desktop/scripts/bundle-backend.sh   # PyInstaller ~25min
npx vite build && npx tauri build --bundles msi  # 66.7MB

# 提案构建（无后端打包）
npx vite build && npx tauri build --bundles msi  # ~20MB，快很多
```

```rust
// gateway.rs 新增逻辑（提案）
fn ensure_backend_installed(app) -> Option<PathBuf> {
    if let Some(bin) = resolve_existing_hermes_binary() { return Some(bin) }
    // 引导: uv tool install hermes-agent (带进度事件, 失败给指引)
    run_uv_tool_install()?;                       // 或打开安装向导
    resolve_existing_hermes_binary()
}
```

## 6. 建议路线（推荐：混合方案）

**两个构建变体，同一代码**：

- **默认（联网优先）**：MSI 轻量 + uv 引导。目标机可访问 PyPI → 开箱即用、provider 全支持、后端永远新。这是提案主体。
- **`--bundled` 变体（离线兜底）**：保留现有 PyInstaller 构建脚本作为可选参数，为离线/内网机器产出 66.7MB 自包含 MSI。构建成本已付（脚本现成），只是默认不跑。

**推荐默认切换为 uv 方案**，理由：解决 provider 全支持（anthropic/bedrock/vertex 全活）、构建链大幅简化、后端随 PyPI 更新；代价（首次联网 + 1-3 分钟）对绝大多数场景可接受，离线场景由 `--bundled` 变体兜底。

## 7. 决策点（需你拍板）

1. **目标机是否保证可访问 PyPI**？（决定能否完全弃 PyInstaller，还是走 §6 混合）
2. **uv.exe 进 MSI**（~20MB，目标机零依赖）还是首次引导下载 / 要求预装？
3. **首次安装交互**：静默后台装（装完自动 continue boot）vs 显式安装向导？
4. **版本策略**：后端随 PyPI 自由升级，还是与前端版本绑定（安装指定版本 `hermes-agent==<前端版本>`）？
5. 若不弃 PyInstaller：`--bundled` 变体是否需要继续维护（离线分发是硬需求？）

---

*提案完毕，未实施。若批准路线，后续实施项：`ensure_backend_installed` 引导逻辑 + 进度事件、uv.exe 打包、CI 双变体构建、版本协商。*
