# Hermes Desktop 技能进化可见化提案：自动沉淀技能的审批与管理界面

> 日期：2026-08-07 ｜ 状态：**已采纳（同日实施一期，按 §8 决策）**
> 背景：Hermes 的"自动进化"机制（后台评审 nudge → `_SKILL_REVIEW_PROMPT` → `skill_manage` 写入）目前对用户**完全不可见**——技能悄悄被创建/修改，用户既不知道发生了什么，也无法干预。用户要求：进化过程可见、可管理（审批、启停、选择保存作用域：当前项目 vs 全局长期）。本提案给出落地方案。
>
> **已拍板的决策（2026-08-07）**：
> 1. 一期交付 **审批 + 学习动态 feed 一起**。
> 2. 门控**默认关**（`skills.write_approval: false`，不改变存量行为，用户主动开启）。
> 3. 二期（项目 vs 全局作用域）**一期完成后排期**，目录约定届时再定。
> 4. 通知形态：**toast + 技能页红点**。

---

## 1. 问题陈述

后台评审机制运转良好，但存在三个体验断层：

1. **不可见**：技能被创建/修补时用户无感知，事后在技能页才发现"多了东西"。
2. **无干预点**：写入直接生效，用户想要"先审一遍再决定"没有入口（CLI 的审批门控存在但默认关闭且深埋 slash 命令）。
3. **无作用域选择**：所有自动沉淀固定写全局 `~/.hermes/skills/`，无法表达"这个技能只对当前项目有意义"。

## 2. 可行性核查（已确认，均为现状资产）

| 前提 | 状态 | 证据 |
|---|---|---|
| 审批门控后端完整 | ✅ | `tools/write_approval.py`：`skills.write_approval` 配置键（默认关）；门控矩阵中技能写入一律 stage；`stage_write()` 落盘到 `~/.hermes/pending/skills/`（带 gist 摘要 + 完整 payload + origin） |
| 审批重放机制 | ✅ | `skill_manager_tool.py::apply_skill_pending()` —— 批准后 bypass 门控重放真实写入；CLI `/skills pending\|approve\|reject\|diff` 已在用 |
| 后台评审写入过门控 | ✅ | 评审 fork 调 `skill_manage` → `_apply_skill_write_gate`；门控开启时后台来源同样 stage |
| 启用/停用 | ✅ | `/api/skills/toggle` + 桌面技能页开关，已在线 |
| 回滚安全网 | ✅ | `agent/curator_backup.py` 快照 + curator archive（只归档不删除） |
| 桌面配置写通道 | ✅ | `POST /api/config`（技能目录功能已使用） |
| **缺口：REST 审批端点** | ❌ 需新建 | `web_routers/` 无 pending/approve 端点（现为 CLI 独占） |
| **缺口：项目级技能作用域** | ❌ 新概念 | `skill_manage` 写死全局；`get_all_skills_dirs` 不感知 cwd |

**结论：一期（审批可见化）基本是"已有机制的 UI 化"；二期（作用域）是概念性新增。**

## 3. 一期方案：审批与可见化（推荐先做）

### 3.1 UX 流程

```
后台评审产生技能写入(创建/修补)
   ↓(skills.write_approval: on)
写入被 stage 到 ~/.hermes/pending/skills/,技能暂不生效
   ↓
桌面端:技能页出现"待审批"红点 + 通知 toast("2 项技能更新待审批")
   ↓
用户进入 /skills 页"待审批"区:
   · 每条:类型徽标(新建/修补/删文件)、技能名、gist 摘要、产生时间、来源(后台评审/前台)
   · 展开看 diff(新建=全文;修补=old/new 对照)
   · [批准] [拒绝] [批准后停用] 三态
   ↓
批准 → apply_skill_pending 重放 → 技能生效(新会话可见)
拒绝 → 丢弃 pending 记录,技能库不变
```

### 3.2 需要新建的薄 REST 端点（`web_routers/skills.py`）

| 端点 | 作用 | 实现 |
|---|---|---|
| `GET /api/skills/pending` | 列出待审项 | 直接读 `~/.hermes/pending/skills/` 记录（write_approval 已有读取函数） |
| `POST /api/skills/pending/<id>/approve` | 批准 | 调 `apply_skill_pending(payload)` |
| `POST /api/skills/pending/<id>/reject` | 拒绝 | 删除 pending 记录文件 |
| `GET /api/skills/pending/<id>/diff` | diff 预览 | gist + payload 的 old/new 渲染素材 |

均为薄包装，业务逻辑零新增。

### 3.3 门控开关的桌面入口

- 设置 → 技能目录区块旁（或技能页工具区）加"技能写入需审批"开关，写 `skills.write_approval`（`/api/config` 通道）。
- 默认保持关（不改变现有行为）；开启后立即对前台与后台写入同时生效。

### 3.4 不做的事（一期）

- 不做行内审批（inline approve/deny 弹窗）——技能内容太大，既有矩阵本来就对技能一律 stage，保持一致。
- 不改 curator、不动后台评审 prompt。

## 4. 二期方案：技能作用域（项目 vs 全局）

新概念，需单独立项推进，本提案给出方向与待决项：

1. **目录约定**：建议 `<项目>/.hermes/skills/`（与 AGENTS.md 的项目级语义同族）。备选 `.agents/skills/`（与 Claude Code 用户级目录撞名，不推荐默认启用）。
2. **发现扩展**:`get_all_skills_dirs()` 增加"当前会话 cwd 的项目技能目录"。注意它从 profile 级变成 cwd 敏感——技能注入在会话构建期发生，cwd 彼时已知，可行；但所有调用点要审计（CLI/gateway/cron 的 cwd 各不相同，cron 的 `workdir` 语义要对齐）。
3. **写入定向**:`skill_manage` 加 `scope: "global" | "project"`（默认 global，保持现状）；审批 UI 在批准时可选作用域（pending payload 记录 scope，重放时按其路由）。
4. **只读边界不受影响**：项目目录是 Hermes 自有约定（可写），与 `external_dirs`（外部所有、只读）是两个维度，不混淆。
5. **与 curator 的关系**：项目技能同样纳入 `created_by: agent` 生命周期管理；项目删除/归档时技能随目录自然消亡，无需额外 GC。

## 5. 另一种 UX 哲学（备选，可与审批并存）

**自动生效 + 学习动态 feed + 一键回滚**：不开门控，写入照常生效；桌面端提供"学习动态"列表（何时、哪个技能、被怎么改了），每条带 [回滚]（用 curator 快照恢复）/ [停用]。干扰最小，符合 Hermes 自主调性；审批模式适合谨慎用户。两者用 `skills.write_approval` 开关天然切换——**建议一期两者都做**：feed 列表两种模式下都有价值（stage 模式下 feed 就是待审列表）。

## 6. 测试计划

- **后端**：沿用 `scripts/run_tests.sh`；为三个新端点写行为测试（stage 后出现在列表、approve 重放后技能落盘、reject 后记录消失；mock 文件系统到 tmp HERMES_HOME）。
- **渲染端 vitest**：待审区组件（列表/展开/三态操作/空态）、开关写入 config 的断言。
- **CDP e2e 探针**：造一个 staged 写入 → 桌面端出现红点 → approve → 技能出现在 `/api/skills`。

## 7. 工作量拆解

| 期 | 项 | 量级 |
|---|---|---|
| 一 | 4 个薄 REST 端点 + 测试 | 后端 ~150 行 + 测试 |
| 一 | 待审批区 UI + 红点/通知 + 开关 | ~300 行 TSX + 测试 |
| 一 |（建议同做）学习动态 feed（纯列表展示 curator/usage 数据） | ~120 行 |
| 二 | 作用域约定 + 发现扩展 + skill_manage scope + 审批落点选择 | 中期改动，单独立项 |

一期约 1 天（含例行 typecheck/vitest/版本号/MSI）。

## 8. 决策点（需你拍板）

1. **一期范围**：仅审批（开关+待审区），还是审批+学习动态 feed 一起？（建议一起）
2. **门控默认值**：保持默认关（推荐——不改变存量行为，用户主动开启）还是对新装默认开？
3. **二期目录约定**：`<项目>/.hermes/skills/`（推荐）vs 其他？二期是否现在就立项并排在一期之后？
4. **审批通知形态**：toast + 技能页红点（推荐）vs 仅红点（更安静）？

---

*提案完毕，未实施。批准后按"一期 →（可选）二期立项"顺序推进。*
