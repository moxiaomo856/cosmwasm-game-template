# Scope Clarification — Invisible-UX Module
# 交付范围澄清：关于「无感」模块

> **Status / 状态**: published alongside the public repository, 2026-10-21
> **Related proposal / 关联提案**: Paxi CosmWasm Game & Lottery Open-Source Template
> + Invisible-UX Starter (188,000 PAXI) — passed, funds disbursed.

---

## 中文

### 一、结论先说

提案已于 2026-10-21 通过，188,000 PAXI 已到账，模板仓库正在按承诺建设。

**仓库落位**：`github.com/moxiaomo856/cosmwasm-game-template`
（提案原文写的是「Paxi Ecosystem org」；提案人没有该 org 的建仓权限，
故落在其个人账号下，此处如实记录。项目与任何 Paxi 官方组织无隶属或背书关系。）

在开工前的范围复核中，我们**主动收窄了一项交付内容**：

> **公开模板将不包含 session-key 无感签名实现。玩家操作改为由交易发送方直接签名。**

提案 §3 交付物第 5 项与 §7 验收标准中提到「Invisible-UX starter」与
「without manual per-transaction signing」。**本次交付不满足该条验收标准。**
链上提案的 summary 中含「invisible-UX starter」字样（链上记录不可修改）。

我们选择**在仓库发布的第一时间主动披露**这一偏差，而不是让社区自行发现。

### 二、为什么收窄

1. **无感的价值在链下，不在链上。**
   session-key 机制解决的是「玩家不必逐笔弹窗签名」。但真正决定体验的是链下那一层：
   session key 的生成与保管、feegrant/relayer 代付 gas、钱包抽象、
   多设备会话恢复、跨应用会话编排。这些属于运营资产，公开即失去差异化；
   而链上的验签协议本身是公开可复制的，公开它并不构成竞争优势。

2. **上游实现与生产运营中台深度耦合。**
   生产环境中的无感链路绑定了具体的代付策略、风控、频控与用户体系。
   脱离上下文单独开源既不完整，也不安全——反而会让集成方误以为可以直接用于生产。

3. **模板的定位是「让新开发者跑起来」。**
   本模板要解决的是「Paxi 上没有一个可复用的 CosmWasm 游戏 / 抽奖骨架」。
   直签模型是最容易理解、最容易审计的起点，适合作为教学与二次开发的基线。

### 三、不变的是什么

- **资金用途不变**：188,000 PAXI 全部用于模板交付、文档、测试网部署与 3 个月维护。
- **交付物其余部分全部保留**：两份合约（游戏 + 抽奖）、参数化、cw721-base 适配、
  React 参考前端、完整文档、测试网部署与 2 周监控、测试网体验活动、
  每周公开 build log、3 个月维护与月度报告。
- **时间表不变**：仓库在提案通过后 7 天内创建于约定位置，其余里程碑按原计划推进。
- **不缩水的是「可验证性」**：结算结果、奖池、卡牌、余额全部链上可验证。

### 四、多出来的东西

收窄无感的同时，我们补上了一个提案里没细写、但对开源项目很关键的东西：

- **明确的信任边界文档**（`docs/invisible-ux-boundaries.md`）
- **如实的安全说明**（`docs/security-notes.md`），含随机数三��机制、
  last-actor 攻击等残余风险，以及**明确声明本模板未做第三方安全审计**
  （审计不在获批预算内，属提案维护范围排除项）
- **接入自有鉴权层的扩展点说明**：游戏合约的 `sanguo_auth()`、
  抽奖合约的 `sender` 参数，集成方可自行实现 session 层或用 feegrant 代付

### 五、我们仍然会做的

- 每周公开 build log（进度、测试网地址、截图）
- 月度维护报告
- 测试网抽奖体验活动（目标 100 次交互；测试网代币无价值，非博彩非投资）
- 社区 PR 首审 7 天内响应，模板 issue 48 小时内首响

---

## English

### Summary

The DAO proposal (188,000 PAXI) passed on 2026-10-21 and the funds have been
disbursed. The template repository is being built as promised.

**Repository location**: `github.com/moxiaomo856/cosmwasm-game-template`.
The proposal named "the Paxi Ecosystem org"; the proposer does not have
repository-creation rights in that org, so the template is published under
their personal account instead. This deviation is recorded here. The project
is not affiliated with, endorsed by, or operated on behalf of any official
Paxi organisation.

During a scope review before starting, we **narrowed one deliverable
deliberately**:

> **The public template will NOT ship the session-key invisible-UX
> implementation. Player actions are authenticated by the transaction sender
> (direct signing).**

The proposal's deliverable #5 and acceptance criteria mention an
"Invisible-UX starter" and "without manual per-transaction signing".
**This delivery does not meet that acceptance criterion.** The on-chain
proposal summary contains the words "invisible-UX starter" and cannot be
amended. We are disclosing this proactively at repository publication rather
than letting the community find it.

**Why:**
1. The value of invisible UX lives off-chain (key custody, feegrant/relayer,
   wallet abstraction, session recovery, cross-app session orchestration).
   The on-chain verification protocol is public and replicable; the off-chain
   orchestration is the actual differentiator, and shipping it would give away
   the moat while adding little for a template user.
2. The upstream implementation is tightly coupled to a production relayer /
   risk-control stack. Open-sourcing it out of context would be incomplete and
   potentially misleading for integrators.
3. The template's purpose is to give new developers a working, understandable
   starting point. Direct signing is the most auditable and easiest to reason
   about.

**Unchanged:** the full budget usage, all other deliverables (both contracts,
parameterization, cw721-base traits, React demo, full docs, testnet deployment
with 2-week monitoring, testnet experience event, weekly public build log,
3 months of maintenance with monthly reports), and the original timeline.

**Added beyond the proposal:** an explicit trust-boundary document
(`docs/invisible-ux-boundaries.md`), an honest security note
(`docs/security-notes.md`) that states plainly that the template has **not**
undergone a third-party security audit (auditing was an explicit exclusion in
the approved maintenance scope), and documented extension points for anyone who
wants to plug in their own session layer or a feegrant-based relayer.

Full disclosure, including the reasoning and the mapping to the original
acceptance criteria, is in `docs/scope-clarification.md` in the repository.
