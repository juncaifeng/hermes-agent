# Hermes Desktop 技能目录管理 UI 提案：外部 skills 目录的可视化接入

> 日期：2026-08-07 ｜ 状态：**已采纳（同日实施，按 §9 决策修订）**
> 背景：桌面端"技能与工具"页只能浏览/开关 `~/.hermes/skills/` 与 hub 安装的技能。后端其实**早已支持** `skills.external_dirs` 外部目录（config.yaml 配置项已发布、发现链路全覆盖），但桌面端没有任何 UI 可以管理它——用户只能手工编辑 config.yaml。本提案给出该功能的 UI 落地方案。
>
> **已拍板的决策（2026-08-07）**：
> 1. 管理界面放在 **/settings 通用设置页**；同时 **/skills 列表页增加按目录（来源空间）筛选**。
> 2. 来源徽标**后补**；但 `/skills` 的目录筛选需要每个技能的来源目录——后端列表加 `source_dir` 字段**纳入本次范围**（仅用于筛选，不展示徽标）。
> 3. 失效目录：**警示 + 提供"清理失效目录"按钮**。
> 4. 不提供"打开 config.yaml"逃生入口。

---

## 1. 问题陈述

- 用户在别处（Claude Code 的 `~/.agents/skills`、团队共享目录、另一个 agent 框架的技能库）已有一批技能目录。
- Hermes 后端可以读它们，但配置入口只有手改 `config.yaml`：
  ```yaml
  skills:
    external_dirs:
      - ~/.agents/skills
  ```
- 桌面端无任何可视化入口；大多数用户不知道这个能力存在。

**目标**：在桌面端提供一个"附加技能目录"管理界面，把上述 YAML 编辑变成三次点击。

## 2. 可行性核查（已确认，均为现状资产）

| 前提 | 状态 | 证据 |
|---|---|---|
| 配置项存在并生效 | ✅ | `hermes_cli/config_defaults.py` `skills.external_dirs`，默认 `[]` |
| 后端发现链路覆盖外部目录 | ✅ | `agent/skill_utils.py::get_all_skills_dirs()`；`tools/skills_tool.py::_find_all_skills()` 显式扫描 local+external；gateway 与 CLI slash 命令扫描同样走这条路（含 external 前缀白名单） |
| 桌面 `/api/skills` 列表自动包含外部技能 | ✅ | 列表 handler 走 `_find_all_skills()`，配好即列出，零改动 |
| 配置读写 API 现成 | ✅ | `GET/POST /api/config`、`/api/config/schema`（桌面设置页已在用） |
| 原生目录选择器桥现成 | ✅ | `selectPaths`（Tauri 迁移已实现并验证） |
| 技能启停、用量、来源（provenance）标记已有 | ✅ | `/api/skills` 已带 `enabled`/`usage`/`provenance`（hub/bundled/agent） |

**结论：无需动后端核心，改动面 = 一个 UI 区块 + 现有 API 接线。**

## 3. 提案方案（UI 设计）

**落点**：`/skills` 页 "技能" 页签内顶部，加一个紧凑的 **"技能目录"管理条**（默认折叠为一行摘要，展开后管理）。不新设 tab——目录管理是低频设置，不该占据与 skills/toolsets/hub/mcp 平级的一级导航。

折叠态（一行）：
```
技能目录  ~/.hermes/skills（内置） + 2 个附加目录      [管理 ▸]
```

展开态：

```
技能目录
├ 内置（可写）        C:\Users\admin\AppData\Local\hermes\skills     ← 固定首行，不可删
├ ~/.agents/skills                                    55 个技能 [打开] [移除]
├ D:\team\shared-skills                               12 个技能 [打开] [移除]
└ [ + 添加目录… ]   ← 调原生目录选择器（selectPaths 桥）

  · 附加目录只读：agent 新建技能始终写入内置目录
  · 变更对“新会话”生效（进行中的会话不受影响）
  · 添加目录即信任其内容为可信技能来源
```

交互细节：

1. **添加**：原生目录选择器 → 校验（存在、是目录、非重复、不解析到内置目录）→ 追加进 `skills.external_dirs` → `POST /api/config` 保存 → 技能列表 invalidate 重取。
2. **移除**：点"移除"即从数组删除并保存（不删磁盘任何东西，纯配置摘除）。
3. **目录状态**：列出时每行做一次轻量校验（exist 检查走桥 `readDir` 或后端列表时附带），失效目录显示"目录缺失"警示条但**不自动移除**（可能是可移动磁盘/网络盘掉线）。
4. **打开**：复用现有 `openDir`/`revealPath` 桥在资源管理器中打开。
5. **技能计数**：直接从当前 `/api/skills` 列表按来源统计（见 §4 的可选后端小改），或第一版先不显示计数。

## 4. 数据流与 API 面

```
[选择目录] selectPaths (桥, 已有)
     ↓
[保存]  POST /api/config  { skills: { external_dirs: [...] } }  (已有)
     ↓
[生效]  get_external_skills_dirs() 按 config.yaml mtime 缓存, 保存即失效重建 (已有)
     ↓
[展示]  GET /api/skills 自动包含外部目录技能 (已有)
```

**唯一可能的后端小改（可选，非阻塞）**：列表项目前只返回 `name/description/category(+enabled/usage/provenance)`，不含源路径。若要做"本地/外部目录X"来源徽标或按目录计数，给 `_find_all_skills()` 的字典加一个 `source_dir` 字段即可（一行级改动）。第一版可不做徽标。

## 5. 边界与约束（后端既有语义，UI 如实呈现）

- **只读**：外部目录永不写入；agent 自建技能固定落内置目录。UI 文案需明确，避免用户期待"往共享目录里存技能"。
- **生效时机**：技能注入发生在会话构建期（prompt caching 铁律），新目录对**新会话**生效；进行中的会话不受影响。这与 `/skills install --now` 的既有惯例一致，保存后提示"新会话生效"即可。
- **同名遮蔽**：内置目录优先于外部目录，同名技能以本地为准。UI 不额外处理，但可在技能行 tooltip 标注"被内置同名技能遮蔽"（可选增强，依赖 §4 的 source_dir）。
- **信任边界**：加入目录 = 把该目录纳入可信 skill 根（slash 命令扫描会信任其中技能）。展开区放一行警示文案即可，不做二次确认弹窗（低频操作，目标用户是高级用户）。
- **profile 作用域**：配置按 profile 隔离（桌面已有 profile 切换时全局 invalidate 的机制），本功能自动继承。

## 6. 为什么不做成别的样子

- **新 tab（目录）**：低频设置不配占一级导航，且"技能目录"语义服务于技能列表，就近放最直观。
- **放进 /settings 设置页**：那里是通用配置；技能目录与技能列表的关联性（改完立刻看列表变化）在 /skills 页内更强。可选后续在设置页加镜像入口。
- **后端 REST 管理端点（POST /api/skills/dirs 等）**：不必要——`/api/config` 就是配置写入通道，再开专用端点是重复造轮子。

## 7. 测试计划

- **渲染端 vitest**：添加/移除/去重/缺失目录警示 的组件级测试；保存调用断言（`skills.external_dirs` 正确序列化进 config payload）。
- **端到端（CDP 探针，`e2e/check-*.mjs` 模式）**：加一个外部目录 → `/api/skills` 返回包含其中技能 → 移除后消失。
- **既有后端测试**:`get_external_skills_dirs` 已有测试钩子（缓存清空单测存在于 agent/skill_utils），本次不动后端逻辑，无需新增。

## 8. 工作量拆解

| 项 | 内容 | 量级 |
|---|---|---|
| 1 | "技能目录"折叠区块组件（列表/添加/移除/缺失态/打开） | ~200 行 TSX |
| 2 | config 读写接线 + 列表 invalidate | ~50 行 |
| 3 | i18n 文案（中/英） | 小 |
| 4 | vitest 组件测试 + CDP e2e 探针 | ~150 行 |
| 5 |（可选）`source_dir` 字段 + 来源徽标 | 后端 ~10 行 + 前端 ~40 行 |

合计约半天到一天（含 typecheck/vitest/版本号/MSI 例行流程）。

## 9. 决策点（需你拍板）

1. **落点**：/skills 页签内折叠区块（推荐）vs 独立 tab vs 设置页？
2. **来源徽标**：第一版就要（依赖后端加 `source_dir` 字段）还是后补？
3. **缺失目录处理**：仅警示（推荐）vs 提供"清理失效目录"按钮？
4. 是否需要"打开 config.yaml 直接编辑"的逃生入口链接？（一行代码的事，建议带）

---

*提案完毕，未实施。若批准，按 §8 顺序实施，完成后走例行验证（typecheck + vitest + cargo check + MSI 出包）。*
