# Security Notes

> 本文件是本模板的**安全边界与已知风险**说明，供集成方在生产环境部署前评估。
> 本文件不构成安全审计报告——**本模板没有经过第三方专业安全审计**（原因见文末）。

---

## 1. 审计状态（先说清楚）

| 项目 | 状态 |
|------|------|
| 第三方专业安全审计 | ❌ **未进行** |
| 内部代码走查 | ✅ 有（见 `../docs/maintenance.md` 的历史记录） |
| 单元测试 | ✅ 抽奖合约 11 个用例；游戏合约以 `cargo test` 为准 |
| 形式化验证 / 模糊测试 | ❌ 未进行 |

**为什么没有审计**：资助提案的「维护范围」一节明确把 security audit 列为
**排除项**（Excluded: custom development, security audit, operation of existing
production contracts, 24/7 support）。审计费用不在获批预算内。

**这意味着**：任何集成方在把本模板用于**有真实资金价值**的奖池之前，
应自行安排审计。文档里把已知的风险点写清楚，是为了让你能评估风险，
而不是替你免除审计义务。

---

## 2. 随机数

### 2.1 三级优先级

`contracts/common/src/random.rs` 定义了 `RandomSource`：

| 级别 | 机制 | 状态 |
|------|------|------|
| 1 | `Vrf` — 链上 VRF（Paxi 链若提供 VRF 预编译/enclave） | **接口已预留，本模板未接入**（依赖链端支持） |
| 2 | `CommitReveal` — 建池者提交 `hash(secret)`，开奖阶段揭示 secret | ✅ 已实现 |
| 3 | `BlockHashFallback` — `sha256(参与者列表 + lottery_id + block_hash + tx_index + nonce)` | ✅ 已实现，**仅适用于小奖池** |

实际开奖走哪条路径取决于奖池类型：平台模板池使用 commit-reveal（哈希链揭示 + 轮换），
玩家自建池在条件不满足时回落到 block-hash。

### 2.2 实现要点（可审计）

- **长度前缀哈希**：`mix_seed` 对每个分段先写入 `len` 再写内容，
  避免 `"ab"+"c"` 与 `"a"+"bc"` 这类拼接歧义。
- **无模偏抽样**：`next_u64` 使用 rejection sampling（`v < limit` 判据），
  不是简单的 `v % upper`，因此不存在取模偏置。
- **不放回抽样**：`draw_distinct_indices` 是部分 Fisher-Yates，
  保证中奖者不重复，且 `count <= total`。
- **链下可复现**：种子构造完全确定，链下 indexer 可以**一字不差地复现**
  同一 seed 并离线核对开奖结果。参数顺序在源码中有固定契约，请勿随意调整。

### 2.3 残余风险（必须知情）

1. **Last-actor 攻击（仅影响 block-hash 兜底路径）**
   若参与者列表在开奖前可被操纵（例如攻击者是最后一个加入者），
   攻击者可以通过「加入 / 不加入」改变结果。commit-reveal 路径可抵御此攻击。

2. **出块者影响力（仅影响 block-hash 兜底路径）**
   `block_hash` 由验证者在出块后确定，最后出块的验证者理论上对结果有影响力。

3. **兜底路径的适用范围**
   源码注释给出的结论是：兜底方案**仅适用于奖池较小**的场景。
   **主网大额奖池必须启用 commit-reveal 或 VRF。**
   集成时请根据奖池规模选择 `RandomSource`，不要图省事一律用兜底。

4. **VRF 尚未接入**
   `RandomSource::Vrf` 目前只是一个枚举分支，没有任何链上实现。
   在 Paxi 链提供 VRF 能力前，请使用 commit-reveal。

### 2.4 集成方检查清单

- [ ] 按奖池规模选择 `RandomSource`，大额奖池禁用 `BlockHashFallback`
- [ ] commit-reveal 的 secret 由**高熵随机源**生成，且在建池后、开奖前保密
- [ ] 在前端对参与者可见的随机源标注清楚（不要宣称「绝对公平」）
- [ ] 保留 commit-reveal 的揭示记录，链下可复现核对

---

## 3. 鉴权模型

### 3.1 当前模型：交易发送方直签

本模板的玩家态操作**由交易发送方（`info.sender`）直接签名**，
消息结构中不存在 `session_addr` / `nonce` / `signature` 字段。

这意味着：
- **谁签名，谁就是玩家**。合约从 `info.sender` 取玩家地址。
- 第三方**无法**代某位玩家提交操作（因为必须拥有该地址的私钥）。
- 交易的防重放由**链本身**的 sequence / nonce 机制保证。

### 3.2 与上游版本（session-key 无感鉴权）的差异

上游生产合约使用 session-key 无感签名：玩家首次授权后，派生一个 session key，
后续操作由 session key 签名，链上校验后还原出主钱包地址。

本公开模板**刻意不包含**该实现，理由与影响见
[`invisible-ux-boundaries.md`](./invisible-ux-boundaries.md)。
对安全姿态的实际影响：

| 维度 | 变化 |
|------|------|
| 第三方代提交 | **收紧**（不再存在「principal ≠ sender」状态） |
| 签名重放 | session nonce 机制随之移除；链级防重放仍在 |
| 每日额度防绕过 | session 的 `daily_limit` 机制随之移除；链上额度逻辑本身不变，但不再有签名侧的额���绑定 |
| 交易签名次数 | **增加**（每笔操作需钱包签名）——这是 UX 成本，不是安全成本 |

### 3.3 管理员与运营方权限（信任假设）

模板保留了一些中心化权限，集成方必须知情：

| 权限 | 持有者 | 说明 |
|------|--------|------|
| `ADMIN` 白名单 | 部署者 | 可改参数、加白名单、批量发碎片、扫 treasury |
| `operator` 白名单 | 部署者指定 | 可执行部分运营操作 |
| 暂停名单 `paused` | 部署者 | 可暂停合约 |
| Treasury 归集 | 部署者 | 管理员可将合约内余额归集到 treasury 地址 |

**本模板未实现**多签、时锁、权限分级。集成到主网前，
建议至少把 `ADMIN` 换成多签地址，并考虑给 treasury 归集加时锁。
`contracts/common/src/admin.rs` 里已有多签提案的数据结构
（`AdminProposal` / `PROPOSALS`），可作为扩展起点。

---

## 4. 资金流相关

- **TKCC 是外部 PRC-20 依赖**：地址可为空。**未配置时**，
  创建 / 参与 / TKCC 销毁统一返回 `TkccNotConfigured`——不 panic、不跳过扣费。
  这是一条刻意的安全设计，请勿为了「让流程跑通」而绕过。
- **烧毁模式** `BurnMode::BlackHole` / `BurnMode::Skip`：
  黑洞模式把 TKCC 转到 burn address 并要求该地址已配置；
  Skip 模式跳过销毁。**两者对奖池价值核算影响不同**，切换前请确认。
- **零金额守卫**：所有扣费路径都有 `is_zero()` 检查，零金额不会触发扣费消息。

---

## 5. 已知限制

1. **VRF 未接入**（见 2.3）
2. **无 session-key 无感鉴权**（见 3.2 与 `invisible-ux-boundaries.md`）
3. **管理员单点权限**，未做多签/时锁
4. **未做第三方安全审计**（见第 1 节）
5. **游戏专属硬编码尚未完全参数化**：`SG_SKILLS`(60) / `SG_BONDS`(23) /
   `FRAG_UNIT` / `daily_limit` 等仍在源码中，见 `../PARAMETERIZE.md` 的待办清单
6. **Rust 工具链锁在 1.69.0**：链上 wasm 只接受 1.69.0 产物
   （1.70+ 的预编译 std 带 sign-ext 指令，CosmWasm VM 0.55 不支持）。
   升级工具链前请先在测试网验证。

---

## 6. 报告安全问题

发现安全问题请通过仓库的 Security Advisory 私下报告，不要公开开 issue。
我们会先确认并修复，再协调披露。
