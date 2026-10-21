# Invisible-UX Boundaries / 无感能力边界

> 本文件说明「无感（invisible UX）」在本模板中**包含什么、不包含什么、为什么**，
> 以及集成方如何自行接入。范围变更的对外说明见
> [`scope-clarification.md`](./scope-clarification.md)。

---

## 1. 什么是「无感」

在本项目的语境里，「无感」指：**玩家只做一次授权，之后的操作不再逐笔弹钱包签名**。

其技术实现是 session-key（会话密钥）模式：

```
玩家首次：主钱包签名 → 链上 register_session（登记公钥）
玩家后续：用派生出的 session key 签名 → 链上校验签名 + 消耗 nonce
         → 合约还原出主钱包地址（资金的实际归属者）
```

链上承载这套逻辑的模块在上游生产合约中叫 `session`
（游戏合约内联实现，抽奖合约通过 `paxi-common` 共享实现）。

---

## 2. 本模板的取舍

**本公开模板不包含 session-key 无感实现。玩家操作由交易发送方（`info.sender`）直接签名。**

### 2.1 为什么不包含

1. **它是链下体验层，不是链上能力。**
   无感的价值 90% 在链下：session key 的生成与保管、feegrant/relayer 代付 gas、
   钱包抽象、多设备会话恢复、跨应用会话编排。这些是**你的运营资产**，
   把它们开源等于把差异化交出去，而链上那部分协议本身是公开可复制的。

2. **上游实现与运营中台耦合。**
   生产环境里的无感链路绑定了具体的代付策略、频控、风控与用户体系，
   抽出来单独开源既不完整也不安全（脱离上下文的安全代码反而误导人）。

3. **模板的定位是「让新开发者跑起来」。**
   本模板要解决的是「Paxi 上没有一个可复用的 CosmWasm 游戏 / 抽奖骨架」。
   直签模型是**最小可理解、最容易审计**的鉴权方式，适合作为教学与起点。

### 2.2 与获批提案的差异（诚实声明）

获批提案 §3 交付物第 5 项写了「Invisible-UX starter kit（optional module）」，
§7 验收标准写了「Invisible-UX demo completes at least one NFT mint and one PvP
settlement **without manual per-transaction signing**」。

**本模板不满足该条验收标准**——因为交付的模板是直签模型。
提案的链上 summary 中含「invisible-UX starter」字样（链上记录不可修改）。

我们选择**如实披露**而不是含糊带过：对社区的正式说明见
[`scope-clarification.md`](./scope-clarification.md)，请在发布仓库时一并放出。

---

## 3. 边界清单

| 能力 | 本模板 | 说明 |
|------|--------|------|
| 交易发送方直签鉴权 | ✅ 包含 | 玩家 = `info.sender`，每笔操作需钱包签名 |
| 链上状态可验证 | ✅ 包含 | 所有余额、卡牌、奖池、结算结果均在链上 |
| session-key 注册 / 撤销 | ❌ 不含 | `RegisterSession` / `RevokeSession` 消息已移除 |
| session 签名校验 / nonce 消耗 | ❌ 不含 | `validate_and_consume_session` 已移除 |
| session 每日额度（`daily_limit`） | ❌ 不含 | 随 session 模块移除 |
| session TTL / 过期清理 | ❌ 不含 | 随 session 模块移除 |
| feegrant / relayer 代付 gas | ❌ 不含 | 属于链下运营层 |
| 钱包抽象层 | ❌ 不含 | 属于链下运营层 |
| session key 托管 / 跨设备恢复 | ❌ 不含 | 属于链下运营层 |
| 跨应用 session 编排 SDK | ❌ 不含 | 属于链下运营层 |
| CommitReveal 随机数 | ✅ 包含 | `contracts/common/src/random.rs` |
| 内部余额账本 / PRC-20 助手 | ✅ 包含 | `contracts/common/src/balance.rs`、`prc20.rs` |
| 管理员多签数据结构 | ✅ 包含（未接线） | `contracts/common/src/admin.rs` 的 `AdminProposal` |

---

## 4. 信任边界

即使本模板不含无感，链上能验证的部分和不能验证的部分仍需说清楚：

| 问题 | 本模板的答案 |
|------|-------------|
| 谁签名？ | 玩家本人（主钱包），即 `info.sender` |
| 谁付 gas？ | 玩家自己的钱包（链上原生 PAXI 手续费），**无代付** |
| 玩家授权了什么？ | 每一笔具体交易（游戏操作 / 抽奖参与），无预授权额度 |
| 合约能做什么？ | 只在玩家主动发起的交易范围内写链上状态；管理员白名单另有特权 |
| 资金归属如何确定？ | 归 `info.sender` 所有人（直签模型） |
| 如何退出？ | 玩家随时停止交互即可，无会话、无锁定、无托管 |

**本模板不声称「完全无链上化」或「无需信任任何第三方」。**
结算结果是链上可验证的；链下前端、索引器、浏览器扩展均不在信任边界内，
它们可能影响体验但不应能改变链上结果。

---

## 5. 集成方如何接入自己的鉴权层

如果你需要 session-key 或其他第三方鉴权方式，有两条路：

### 路线 A：链下代付（最简单，推荐先试）

不动合约，只在前端用 **feegrant** 让第三方 relayer 支付 gas。
玩家仍逐笔签名，但不用自己掏 gas。用户体验已显著改善，且不引入合约改动。

### 路线 B：在合约内自建 session 层

本模板保留了清晰的接入点，你可以自行实现：

**游戏合约**——玩家身份解析集中在 `src/sanguo/helpers.rs` 的 `sanguo_auth()`：

```rust
// 当前实现：直接返回交易发送方
pub fn sanguo_auth(deps: Deps, env: Env, info: &MessageInfo) -> Result<Addr, ContractError> {
    // ... 首次链上活动写入 ACCT_FIRST_SEEN ...
    Ok(info.sender.clone())
}
```

把 `Ok(info.sender.clone())` 换成你自己的解析逻辑
（查 session 表 → 验签 → 消耗 nonce → 返回主钱包地址），
并在 `src/msg.rs` 的 `ExecuteMsg` 变体上补回你需要的签名字段即可。
注意 `sanguo_auth` 目前还负责写入 `ACCT_FIRST_SEEN`（赠送系统的「账号满 30 天」门槛依赖它），
替换实现时请保留这个副作用。

**抽奖合约**——玩家身份在 `src/lottery.rs` 的
`execute_create_lottery` / `join_pool_internal` / `execute_join_lottery` 中
以 `sender: &Addr` 传入。同样替换为你的解析逻辑即可。

**共用 crate**——`contracts/common/` 是 vendor 进来的独立 crate，
你可以直接在自己的副本里加一个 `session` 模块（上游实现是 Apache-2.0，
可自由修改与再分发，保留 NOTICE 即可）。

---

## 6. 不要踩的坑

- **不要**把 session key 存在服务端明文或日志里——它等同于账户权限。
- **不要**用「无感」作为免除链下风控的理由；代付 gas 意味着 relayer 在替你承担
  提交失败与抢跑的成本，需要自己做频率限制与额度控制。
- **不要**在 commit-reveal 的 secret 泄露后仍然继续使用该轮次——secret 一旦公开，
  后续的 join 就失去了约束力。
