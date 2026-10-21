//! 抽奖：创建 / 参与 / 开奖 / 领取 / 退款
//!
//! 包含两种模式（共用同一张 `LOTTERIES` 表与同一套分配逻辑）：
//! * **A 模式（玩家自建池）**：建池者付 50 PAXI + 60 万 TKCC，自定义参与费与人数
//! * **B2 模式（平台模板池）**：管理员预设模板，见 [crate::template]
//!
//! * 参与：1–10 PAXI + 1 万–10 万 TKCC，地址去重
//! * 人数：5–50，满员后开奖
//! * 分配：TKCC 38 / 28（二等奖 2 人共）/ 建池者 14 / 运营 14 / 销毁 6；
//!         PAXI 40 / 30 / 建池者 15 / 运营 15（不销毁）。
//!         平台池无建池者，其分成归运营（运营 28% / 30%）。
//!         **余数归一等奖，总额守恒**

use cosmwasm_std::{Addr, Deps, DepsMut, Env, Order, Response, StdResult, Uint128};

use paxi_common::{
    draw_distinct_indices, fallback_seed, mix_seed, BalanceLedger, RandomSource,
};

use crate::error::ContractError;
use crate::helpers::{
    build_burn_msgs, escrow_key, escrow_shortfall, native_key, note_burned, note_ops, require_tkcc,
    to_tkcc_units,
};
use crate::msg::UnclaimedResponse;
use crate::state::{
    Lottery, LotteryStatus, Payout, Winners, ACTIVE_POOL_OF_TEMPLATE, CLAIMED, LOTTERIES,
    LOTTERY_CONFIG, LOTTERY_SEQ, LOTTERY_STATUS_INDEX, PAXI_CREATOR_BPS, PAXI_FIRST_BPS,
    PAXI_OPS_BPS, PAXI_SECOND_TOTAL_BPS, PARTICIPANT_JOIN_TIME, PARTICIPANTS, REFUND_STARTED,
    TKCC_BURN_BPS, TKCC_CREATOR_BPS, TKCC_FIRST_BPS, TKCC_OPS_BPS, TKCC_SECOND_TOTAL_BPS,
    TEMPLATE_SECRETS,
};

/// 链上熵源：CosmWasm 2.0.4 的 BlockInfo 只有 chain_id / height / time
fn block_entropy(env: &Env) -> String {
    format!("{}:{}:{}", env.block.chain_id, env.block.height, env.block.time.seconds())
}

/// 按万分比取整计算（向下取整）。
///
/// 溢出兜底刻意返回 **0 而不是 `total`**：返回 `total` 会让 `spent` 超过奖池，
/// `pool.saturating_sub(spent)` 归零，一等奖拿到整池 → 严重超发。
/// 返回 0 时 `spent` 只会偏小，多出来的部分仍然通过"余数归一等奖"回到一等奖，
/// **总额守恒依然成立**（最坏退化为"几乎全给一等奖"，而不会透支托管）。
fn bps(total: Uint128, bps: u64) -> Uint128 {
    total
        .checked_mul(Uint128::new(bps as u128))
        .unwrap_or(Uint128::zero())
        .checked_div(Uint128::new(10_000u128))
        .unwrap_or(Uint128::zero())
}

/// 分配计算：**余数一律归一等奖**，保证
/// `first + second_each*2 + creator + ops + burn == pool` 恒等。
///
/// `is_template_pool`：
/// * `false`（A 模式玩家池）—— 建池者 14%（TKCC）/15%（PAXI），运营 14%/15%
/// * `true` （B2 模式平台池）—— 无建池者，建池者那部分**归运营** → 运营 28%/30%
///
/// 两种模式的一等奖、二等奖、销毁比例完全一致。
pub fn compute_payout(
    pool_tkcc: Uint128,
    pool_paxi: Uint128,
    second_count: u64,
    is_template_pool: bool,
) -> Payout {
    // ---- TKCC ----
    let p_first = bps(pool_tkcc, TKCC_FIRST_BPS);
    let p_second_total = bps(pool_tkcc, TKCC_SECOND_TOTAL_BPS);
    let p_burn = bps(pool_tkcc, TKCC_BURN_BPS);
    // 平台池：建池者分成归运营
    let (p_creator, p_ops) = if is_template_pool {
        (Uint128::zero(), bps(pool_tkcc, TKCC_CREATOR_BPS + TKCC_OPS_BPS))
    } else {
        (bps(pool_tkcc, TKCC_CREATOR_BPS), bps(pool_tkcc, TKCC_OPS_BPS))
    };

    // ---- PAXI ----
    let a_first = bps(pool_paxi, PAXI_FIRST_BPS);
    let a_second_total = bps(pool_paxi, PAXI_SECOND_TOTAL_BPS);
    let (a_creator, a_ops) = if is_template_pool {
        (Uint128::zero(), bps(pool_paxi, PAXI_CREATOR_BPS + PAXI_OPS_BPS))
    } else {
        (bps(pool_paxi, PAXI_CREATOR_BPS), bps(pool_paxi, PAXI_OPS_BPS))
    };

    let n = if second_count == 0 { 1 } else { second_count };
    let p_second_each = p_second_total
        .checked_div(Uint128::new(n as u128))
        .unwrap_or(Uint128::zero());
    // 二等奖按人数均分。`余数`（百分比取整 + 二等奖均分除不尽的零头）**全部归一等奖**，
    // 从而保证 `first + second_each*2 + creator + ops + burn == pool` 严格相等。
    //
    // 注意：这里 `p_spent` 用的是**实发** `p_second_paid`（= second_each*n），
    // 差集 `pool - p_spent` 已天然包含"二等奖除不尽的零头"，故传入一等奖时
    // **不能**再额外加一次零头，否则会多发（历史 bug，见 tests 守恒用例）。
    let p_second_paid = p_second_each * Uint128::new(n as u128);
    let p_spent = p_first + p_second_paid + p_creator + p_ops + p_burn;
    let p_remainder = pool_tkcc.saturating_sub(p_spent);
    let first_tkcc = p_first + p_remainder;

    let a_second_each = a_second_total
        .checked_div(Uint128::new(n as u128))
        .unwrap_or(Uint128::zero());
    let a_second_paid = a_second_each * Uint128::new(n as u128);
    let a_spent = a_first + a_second_paid + a_creator + a_ops;
    let a_remainder = pool_paxi.saturating_sub(a_spent);
    let first_paxi = a_first + a_remainder;

    Payout {
        first_tkcc,
        second_each_tkcc: p_second_each,
        first_paxi,
        second_each_paxi: a_second_each,
        creator_tkcc: p_creator,
        creator_paxi: a_creator,
        ops_tkcc: p_ops,
        ops_paxi: a_ops,
        burn_tkcc: p_burn,
    }
}

/// 收集参与者（按地址升序，保证随机可复现）。
///
/// M3 修复：使用 `prefix(id)` 只扫本抽奖的参与者，避免随平台抽奖数量增长而
/// 退化成 O(N) 全表扫描。
pub fn load_participants(deps: &DepsMut, id: u64) -> Result<Vec<Addr>, ContractError> {
    let mut list: Vec<Addr> = PARTICIPANTS
        .prefix(id)
        .keys(deps.storage, None, None, Order::Ascending)
        .collect::<StdResult<Vec<_>>>()?;
    list.sort();
    Ok(list)
}

// ========================================================================
// 创建
// ========================================================================

pub fn execute_create_lottery(
    mut deps: DepsMut,
    env: &Env,
    sender: &Addr,
    tier: u8,
    commit_hash: Option<String>,
) -> Result<Response, ContractError> {
    crate::helpers::assert_not_paused(deps.as_ref())?;
    let cfg = LOTTERY_CONFIG.load(deps.storage)?;

    // simple 版：人数、参与费、建池费**全部**来自档位表，不接受任何自由参数。
    let spec = crate::state::tier_spec(tier).ok_or(ContractError::InvalidTier(tier))?;

    if let Some(h) = &commit_hash {
        if h.len() != 64 || hex::decode(h).is_err() {
            return Err(ContractError::InvalidCommitHash);
        }
        // M4：拒绝 sha256("") —— secret="" 任何人都能算出，commit-reveal 形同虚设。
        // 硬编码已知值，不做完整重复检测（那需要存所有 commit_hash，gas 贵）。
        if h == "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855" {
            return Err(ContractError::InvalidCommitHash);
        }
    }

    // 建池费：PAXI（raw）+ TKCC（个数 -> raw）
    let create_paxi = spec.create_fee_paxi;
    let create_tkcc = to_tkcc_units(deps.storage, spec.create_fee_tkcc)?;

    // 依赖 TKCC：未配置 -> TkccNotConfigured。
    let tkcc = require_tkcc(deps.storage)?;

    // 🟢 建池者即**交易发送方**（`info.sender`）。链上原生鉴权，无需任何附加签名。
    let user = sender.clone();

    let id = {
        let seq = LOTTERY_SEQ.may_load(deps.storage)?.unwrap_or_default() + 1;
        LOTTERY_SEQ.save(deps.storage, &seq)?;
        seq
    };

    let escrow = escrow_key(id);
    if !create_paxi.is_zero() {
        BalanceLedger::debit(deps.storage, &user, &native_key(), create_paxi)?;
        BalanceLedger::credit(deps.storage, &escrow, &native_key(), create_paxi)?;
    }
    if !create_tkcc.is_zero() {
        BalanceLedger::debit(deps.storage, &user, &tkcc, create_tkcc)?;
        BalanceLedger::credit(deps.storage, &escrow, &tkcc, create_tkcc)?;
    }

    let now = env.block.time.seconds();
    let join_tkcc_units = to_tkcc_units(deps.storage, spec.join_tkcc)?;
    LOTTERIES.save(
        deps.storage,
        id,
        &Lottery {
            id,
            creator: user.clone(),
            // 审计用；金额与人数以下方快照为准
            tier,
            join_paxi: spec.join_paxi,
            join_tkcc: join_tkcc_units,
            max_people: spec.max_people,
            pool_paxi: create_paxi,
            pool_tkcc: create_tkcc,
            settled_pool_paxi: Uint128::zero(),
            settled_pool_tkcc: Uint128::zero(),
            participant_count: 0,
            creator_refunded: false,
            // 建池费实付快照：退款时用它，绝不用当前的全局配置
            create_fee_paid_paxi: create_paxi,
            create_fee_paid_tkcc: create_tkcc,
            status: LotteryStatus::Open,
            created_at: now,
            // 审计：`now + duration_secs` 在 release 下会静默回绕；改用 checked_add。
            expires_at: now
                .checked_add(cfg.duration_secs)
                .ok_or(ContractError::Overflow)?,
            commit_hash,
            revealed: None,
            winners: None,
            payout: None,
            seed: None,
            random_source: None,
            // A 模式：玩家自建池
            template_id: None,
            is_template_pool: false,
        },
    )?;
    // 状态索引（P2-6）：新建池 → open
    LOTTERY_STATUS_INDEX.save(deps.storage, (status_str(&LotteryStatus::Open), id), &())?;

    Ok(Response::new()
        .add_attribute("action", "create_lottery")
        .add_attribute("lottery_id", id.to_string())
        .add_attribute("creator", user)
        .add_attribute("tier", tier.to_string())
        .add_attribute("pool_paxi", create_paxi)
        .add_attribute("pool_tkcc", create_tkcc)
        .add_attribute("max_people", spec.max_people.to_string()))
}

// ========================================================================
// 参与
// ========================================================================

/// **参与池子的唯一实现**（A 模式 `JoinLottery` 与 B2 模式 `ActivateTemplate` 共用）。
///
/// 金额一律从链上 `Lottery` 对象读取，调用方**不传金额**；参与者即**交易发送方**
/// （`info.sender`）。`action` 只用于响应里的 `action` 属性：
/// * `JoinLottery`      → action=`join_lottery`
/// * `ActivateTemplate` → action=`activate_template`
///
/// 【关键】这里对第一个加入者（`participant_count == 0`，即模板池的"触发者"）
/// **不做任何特殊处理**：不发额外奖励、不给分成、不记特殊标记。
/// 触发者与第 50 个参与者走的是完全相同的代码路径与扣款逻辑。
pub fn join_pool_internal(
    mut deps: DepsMut,
    env: &Env,
    sender: &Addr,
    pool_id: u64,
    action: &str,
) -> Result<Response, ContractError> {
    crate::helpers::assert_not_paused(deps.as_ref())?;

    let mut lottery = LOTTERIES.may_load(deps.storage, pool_id)?.ok_or(ContractError::LotteryNotFound(pool_id))?;
    if !matches!(lottery.status, LotteryStatus::Open) {
        return Err(ContractError::LotteryNotOpen(pool_id));
    }
    // N1：时间边界统一为 now >= expires_at 已到期
    if env.block.time.seconds() >= lottery.expires_at {
        return Err(ContractError::Expired(pool_id, lottery.expires_at));
    }

    let join_tkcc = lottery.join_tkcc;
    // 与建池同理：无副作用的 TKCC 配置校验先做，再动业务状态。
    let tkcc = require_tkcc(deps.storage)?;

    // 🟢 参与者即**交易发送方**（`info.sender`）。
    let user = sender.clone();

    // P1-2：建池者不得参与自己的池。
    //
    // 若建池者填了 commit_hash，他就掌握 secret；又因为他能自由选择何时加入，
    // 于是可以先算出中奖结果、不中奖就不加入 —— 这是"看菜下饭"的免费期权，
    // 跟开奖用什么熵源无关，只能从参与资格上堵。
    // 建池者本来就拿 14%（TKCC）/ 15%（PAXI）的建池者分成，不参与抽奖完全合理。
    //
    // 模板池不受影响：它的 creator 是**合约自身地址**，调用者不可能是它。
    if !lottery.is_template_pool && user == lottery.creator {
        return Err(ContractError::CreatorCannotJoin(pool_id));
    }

    if PARTICIPANTS.has(deps.storage, (pool_id, &user)) {
        return Err(ContractError::AlreadyJoined(user.clone(), pool_id));
    }

    let escrow = escrow_key(pool_id);
    if !lottery.join_paxi.is_zero() {
        BalanceLedger::debit(deps.storage, &user, &native_key(), lottery.join_paxi)?;
        BalanceLedger::credit(deps.storage, &escrow, &native_key(), lottery.join_paxi)?;
        lottery.pool_paxi += lottery.join_paxi;
    }
    if !join_tkcc.is_zero() {
        BalanceLedger::debit(deps.storage, &user, &tkcc, join_tkcc)?;
        BalanceLedger::credit(deps.storage, &escrow, &tkcc, join_tkcc)?;
        lottery.pool_tkcc += join_tkcc;
    }

    PARTICIPANTS.save(deps.storage, (pool_id, &user), &true)?;
    // C+：记录加入时间，作为开奖种子的盐。
    PARTICIPANT_JOIN_TIME.save(deps.storage, (pool_id, &user), &env.block.time.seconds())?;
    lottery.participant_count += 1;

    // 满员：只改状态，**保留**模板指针。
    //
    // 【死锁修复】满员就清指针的话，`active_pool_of_templates` 查询会拿不到
    // active_pool_id → 前端"揭示秘密"的前置校验（必须有活跃池）把管理员拦住，
    // 满员池永远等不到揭示 → 又一个死锁。指针保留到此池开奖 / 全退时才清
    // （见 draw / refund 的"指针仍指向自己才清"逻辑）；
    // 下一次 ActivateTemplate 靠 `participant_count >= max_people` 判断开新池
    // 并覆盖指针，与保留指针不冲突。
    if lottery.participant_count >= lottery.max_people {
        lottery.status = LotteryStatus::Full;
        // 状态索引（P2-6）：open → full
        LOTTERY_STATUS_INDEX.remove(deps.storage, (status_str(&LotteryStatus::Open), pool_id));
        LOTTERY_STATUS_INDEX.save(deps.storage, (status_str(&LotteryStatus::Full), pool_id), &())?;
    }
    LOTTERIES.save(deps.storage, pool_id, &lottery)?;

    Ok(Response::new()
        .add_attribute("action", action)
        .add_attribute("lottery_id", pool_id.to_string())
        .add_attribute("participant", user)
        .add_attribute("count", lottery.participant_count.to_string())
        .add_attribute("status", status_str(&lottery.status)))
}

/// A 模式入口：参与玩家自建池（薄封装）
pub fn execute_join_lottery(
    deps: DepsMut,
    env: &Env,
    sender: &Addr,
    id: u64,
) -> Result<Response, ContractError> {
    join_pool_internal(deps, env, sender, id, "join_lottery")
}

// ========================================================================
// commit-reveal
// ========================================================================

pub fn execute_reveal_secret(
    deps: DepsMut,
    env: &Env,
    sender: &Addr,
    id: u64,
    secret: String,
) -> Result<Response, ContractError> {
    crate::helpers::assert_not_paused(deps.as_ref())?;
    let mut lottery = LOTTERIES.may_load(deps.storage, id)?.ok_or(ContractError::LotteryNotFound(id))?;
    if lottery.creator != *sender {
        return Err(ContractError::Unauthorized);
    }
    if lottery.revealed.is_some() {
        return Err(ContractError::AlreadyRevealed);
    }
    // H-1：到期后禁止 reveal —— 否则创建者可以观察参与者名单再决定是否揭示，
    // 等价于一次免费的"看菜下饭"期权。必须在到期前锁定选择。
    if env.block.time.seconds() >= lottery.expires_at {
        return Err(ContractError::Expired(id, lottery.expires_at));
    }
    let commit = lottery.commit_hash.clone().ok_or(ContractError::InvalidCommitHash)?;

    use sha2::{Digest as _, Sha256};
    let digest = Sha256::digest(secret.as_bytes());
    let hex_digest = hex::encode(digest);
    if hex_digest != commit {
        return Err(ContractError::CommitMismatch);
    }

    lottery.revealed = Some(secret);
    LOTTERIES.save(deps.storage, id, &lottery)?;

    Ok(Response::new()
        .add_attribute("action", "reveal_secret")
        .add_attribute("lottery_id", id.to_string()))
}

// ========================================================================
// 开奖
// ========================================================================

pub fn execute_draw_lottery(mut deps: DepsMut, env: &Env, id: u64) -> Result<Response, ContractError> {
    crate::helpers::assert_not_paused(deps.as_ref())?;
    let mut lottery = LOTTERIES.may_load(deps.storage, id)?.ok_or(ContractError::LotteryNotFound(id))?;
    if matches!(lottery.status, LotteryStatus::Drawn) {
        return Err(ContractError::AlreadyDrawn(id));
    }
    if matches!(lottery.status, LotteryStatus::Refunded) {
        return Err(ContractError::AlreadyRefunded(id));
    }

    // M1：开奖前先确认销毁路径可用。若未配置黑洞地址却用 BlackHole 模式，
    // 这里给出清晰错误，而不是在扣完款后才在 burn 阶段失败导致奖池永久卡死。
    crate::helpers::assert_burn_config_ok(deps.as_ref())?;

    let cfg = LOTTERY_CONFIG.load(deps.storage)?;
    let participants = load_participants(&deps, id)?;
    let now = env.block.time.seconds();

    // P1：退款 / 开奖抢跑防护（双保险）。
    // simple 版开奖**只看满员**：退款一次 participant_count 必然跌破 max_people，
    // 抢跑在结构上已不可能；本标记保留，防未来有人放开开奖条件时漏掉这层闸。
    if REFUND_STARTED.may_load(deps.storage, id)?.unwrap_or(false) {
        return Err(ContractError::RefundStarted(id));
    }

    // simple 版：**只有满员才能开奖**（无 min_people 分支）；
    // 超时未满员一律走 Refund。
    let can_draw = lottery.participant_count >= lottery.max_people;
    if !can_draw {
        return Err(ContractError::LotteryNotFull(
            id,
            lottery.participant_count,
            lottery.max_people,
        ));
    }
    if (participants.len() as u64) < cfg.first_prize_count + cfg.second_prize_count {
        return Err(ContractError::NotEnoughParticipants(
            participants.len() as u64,
            cfg.first_prize_count + cfg.second_prize_count,
        ));
    }

    // ---- 随机源 ----
    //
    // C+：把每个参与者的**加入时间**一起混入种子，作为"出块者不可控"的熵源。
    // 用 for 循环（而非闭包 map）是为了让 `deps.storage` 借用检查更直观。
    let mut parts: Vec<String> = Vec::with_capacity(participants.len());
    for a in &participants {
        // 旧池（本字段引入前加入的）读不到时回落 0；只影响历史池，新池必有值。
        let jt = PARTICIPANT_JOIN_TIME
            .may_load(deps.storage, (id, a))
            .ok()
            .flatten()
            .unwrap_or(0u64);
        parts.push(format!("{}@{}", a, jt));
    }
    let (seed, source) = if lottery.is_template_pool {
        // ---- 模板池：随机源走"平台托管 secret"查表 ----
        //
        // 池内 `revealed` 快照恒为 None（揭示不回写池子），不能直接读；
        // 正确的 key 是池子创建时快照的 `commit_hash`（= 那时模板的当前承诺），
        // 到 `TEMPLATE_SECRETS[(template_id, commit)]` 查对应的揭示值。
        // 这样满员**之后**的揭示也能被本池用到，且每个承诺只对应一次揭示。
        let tid = lottery
            .template_id
            .ok_or(ContractError::TemplateCommitRequired(id))?;
        let commit = lottery
            .commit_hash
            .clone()
            .ok_or(ContractError::TemplateCommitRequired(id))?;
        match TEMPLATE_SECRETS.may_load(deps.storage, (tid, commit.as_str()))? {
            Some(secret) => (
                fallback_seed(
                    &parts,
                    id,
                    &block_entropy(env),
                    env.block.height,
                    0,
                    Some(secret.as_bytes()),
                ),
                RandomSource::CommitReveal,
            ),
            // 管理员还没揭示（或揭示流程中断）→ 等待揭示；过期未揭示则只能退款。
            None => return Err(ContractError::CommitNotRevealed(id)),
        }
    } else {
        // ---- A 模式玩家池：commit-reveal 快照 + fallback（原逻辑不变）----
        match (&lottery.commit_hash, &lottery.revealed) {
            (Some(_), Some(secret)) => (
                fallback_seed(
                    &parts,
                    id,
                    &block_entropy(env),
                    env.block.height,
                    0,
                    Some(secret.as_bytes()),
                ),
                RandomSource::CommitReveal,
            ),
            (Some(_), None) if now < lottery.expires_at => {
                // 承诺了但还没揭示，且未到期 -> 不允许开奖，避免创建者挑选时机
                return Err(ContractError::CommitMismatch);
            }
            (Some(_), None) => {
                // M5：承诺过但到期仍未揭示 —— 创建者违约，禁止 fallback 开奖，
                // 改为让参与者走退款流程（见 execute_refund）。
                return Err(ContractError::CommitNotRevealed(id));
            }
            (None, _) => (
                // 玩家池未提交承诺：允许 fallback（参与者加入时间已加盐）。
                // 模板池走不到这里 —— 上面已按 TemplateCommitRequired 拒绝无承诺池。
                fallback_seed(&parts, id, &block_entropy(env), env.block.height, 0, None),
                RandomSource::BlockHashFallback,
            ),
        }
    };

    let need = (cfg.first_prize_count + cfg.second_prize_count) as u64;
    let indices = draw_distinct_indices(seed, participants.len() as u64, need);
    let first: Vec<Addr> = indices
        .iter()
        .take(cfg.first_prize_count as usize)
        .map(|i| participants[*i as usize].clone())
        .collect();
    let second: Vec<Addr> = indices
        .iter()
        .skip(cfg.first_prize_count as usize)
        .take(cfg.second_prize_count as usize)
        .map(|i| participants[*i as usize].clone())
        .collect();

    // ---- 分配 ----
    // 平台池：建池者分成自动归运营（compute_payout 内部分支）
    let payout = compute_payout(
        lottery.pool_tkcc,
        lottery.pool_paxi,
        cfg.second_prize_count,
        lottery.is_template_pool,
    );

    let escrow = escrow_key(id);
    let tkcc = require_tkcc(deps.storage)?;
    let treasury = crate::state::CONFIG.load(deps.storage)?.treasury;

    // 会计恒等式自检：退款会同步扣减 pool_*，所以正常情况下
    // escrow 余额 >= pool_*。若这里不成立，说明存在未预期的资金路径，
    // 此时按 pool_* 分配会透支托管 → 后位领取者永远拿不到钱。
    // 宁可拒绝开奖（资金仍可通过 Refund 取回），也不要造成永久锁死。
    if let Some(err) = escrow_shortfall(deps.as_ref(), id, &lottery, &escrow, &tkcc)? {
        return Err(err);
    }

    // 建池者分成 + 运营分成 -> 内部余额
    BalanceLedger::debit(deps.storage, &escrow, &native_key(), payout.creator_paxi + payout.ops_paxi)?;
    BalanceLedger::credit(deps.storage, &lottery.creator, &native_key(), payout.creator_paxi)?;
    BalanceLedger::credit(deps.storage, &treasury, &native_key(), payout.ops_paxi)?;
    note_ops(deps.branch(), &native_key(), payout.ops_paxi)?;

    BalanceLedger::debit(deps.storage, &escrow, &tkcc, payout.creator_tkcc + payout.ops_tkcc)?;
    BalanceLedger::credit(deps.storage, &lottery.creator, &tkcc, payout.creator_tkcc)?;
    BalanceLedger::credit(deps.storage, &treasury, &tkcc, payout.ops_tkcc)?;
    note_ops(deps.branch(), &tkcc, payout.ops_tkcc)?;

    // TKCC 销毁 6%：离开合约（burn 或转黑洞）
    let burn_msgs = if !payout.burn_tkcc.is_zero() {
        BalanceLedger::debit(deps.storage, &escrow, &tkcc, payout.burn_tkcc)?;
        // 只有"真离开流通"的两种模式才计入销毁统计；
        // `Skip` 实际是转入金库，计成销毁会让统计失真。
        if !matches!(paxi_common::load_burn_mode(deps.storage)?, paxi_common::BurnMode::Skip) {
            note_burned(deps.branch(), payout.burn_tkcc)?;
        }
        build_burn_msgs(&deps, &tkcc, payout.burn_tkcc)?
    } else {
        vec![]
    };

    // M2：开奖并扣走 建池者/运营/销毁 之后，托管里**只应剩**"待中奖者领取"的部分。
    // 原先是 `debug_assert_eq!`（release 构建被优化掉），现在改成显式检查，
    // 否则"compute_payout 新增分配项却忘了同步扣款"这类代际回归在链上会被静默放过，
    // 导致后位中奖者永远领不到钱（资金永久锁死）。
    {
        let expected_paxi = payout.first_paxi
            + payout.second_each_paxi * Uint128::new(cfg.second_prize_count as u128);
        let actual_paxi = BalanceLedger::balance_of(deps.storage, &escrow, &native_key());
        let expected_tkcc = payout.first_tkcc
            + payout.second_each_tkcc * Uint128::new(cfg.second_prize_count as u128);
        let actual_tkcc = BalanceLedger::balance_of(deps.storage, &escrow, &tkcc);
        if actual_paxi != expected_paxi || actual_tkcc != expected_tkcc {
            return Err(ContractError::EscrowMismatch {
                id,
                escrow_paxi: actual_paxi,
                pool_paxi: expected_paxi,
                escrow_tkcc: actual_tkcc,
                pool_tkcc: expected_tkcc,
            });
        }
    }

    let source_label = format!("{:?}", source);
    // 状态索引（P2-6）：old（simple 版必然是 full）→ drawn
    let old_status = status_str(&lottery.status);
    lottery.status = LotteryStatus::Drawn;
    LOTTERY_STATUS_INDEX.remove(deps.storage, (old_status, id));
    LOTTERY_STATUS_INDEX.save(deps.storage, (status_str(&LotteryStatus::Drawn), id), &())?;
    // 模板池开奖后清指针（只有指针**当前仍指向自己**时才清）。
    // 指针生命周期：open_new_pool 时 save（覆盖旧值）→ 满员**保留**（前端揭示
    // 校验依赖它）→ 开奖或全退时 remove。
    // 场景：老池 P1 满员后用户激活开了新池 P2（指针 T → P2），随后 P1 才被开奖；
    // 若无条件清，就会误删 P2 的指针，P2 立刻变成孤儿池。
    if let Some(tid) = lottery.template_id {
        if ACTIVE_POOL_OF_TEMPLATE.may_load(deps.storage, tid)? == Some(id) {
            ACTIVE_POOL_OF_TEMPLATE.remove(deps.storage, tid);
        }
    }
    lottery.winners = Some(Winners { first: first.clone(), second: second.clone() });
    lottery.payout = Some(payout.clone());
    lottery.seed = Some(hex::encode(seed));
    lottery.random_source = Some(source);
    // M2：开奖成功后把奖池余额清零，并将原始奖池快照保留到 settled_pool_*，
    // 供前端 / 审计展示历史奖池，避免列表里一直显示"奖池 = 原值"误导用户。
    lottery.settled_pool_paxi = lottery.pool_paxi;
    lottery.settled_pool_tkcc = lottery.pool_tkcc;
    lottery.pool_paxi = Uint128::zero();
    lottery.pool_tkcc = Uint128::zero();
    LOTTERIES.save(deps.storage, id, &lottery)?;

    let mut resp = Response::new()
        .add_attribute("action", "draw_lottery")
        .add_attribute("lottery_id", id.to_string())
        .add_attribute("seed", hex::encode(seed))
        .add_attribute("random_source", source_label)
        .add_attribute("first", first.iter().map(|a| a.to_string()).collect::<Vec<_>>().join(","))
        .add_attribute("second", second.iter().map(|a| a.to_string()).collect::<Vec<_>>().join(","))
        .add_attribute("burn_tkcc", payout.burn_tkcc);
    if !burn_msgs.is_empty() {
        resp = resp.add_messages(burn_msgs);
    }
    Ok(resp)
}

// ========================================================================
// 领取
// ========================================================================

pub fn execute_claim(
    mut deps: DepsMut,
    _env: &Env,
    sender: &Addr,
    id: u64,
) -> Result<Response, ContractError> {
    // H-2 修复的反例：claim 在 contract.rs 的暂停白名单里（user_fund_op），
    // 用户自己的钱不应被暂停锁死，所以**不能**加 assert_not_paused。
    let lottery = LOTTERIES.may_load(deps.storage, id)?.ok_or(ContractError::LotteryNotFound(id))?;
    // N3：按状态细分错误，避免"已退款"也被报成"尚未开奖"而误导用户。
    match lottery.status {
        LotteryStatus::Drawn => {}
        LotteryStatus::Refunded => return Err(ContractError::AlreadyRefunded(id)),
        _ => return Err(ContractError::LotteryNotDrawn(id)),
    }
    let winners = lottery.winners.clone().ok_or(ContractError::NothingToClaim)?;
    let payout = lottery.payout.clone().ok_or(ContractError::NothingToClaim)?;

    let (tkcc_amount, paxi_amount) = if winners.first.contains(sender) {
        (payout.first_tkcc, payout.first_paxi)
    } else if winners.second.contains(sender) {
        (payout.second_each_tkcc, payout.second_each_paxi)
    } else {
        return Err(ContractError::NotWinner(sender.clone(), id));
    };

    if CLAIMED.has(deps.storage, (id, sender)) {
        return Err(ContractError::AlreadyClaimed(sender.clone(), id));
    }
    CLAIMED.save(deps.storage, (id, sender), &true)?;

    let escrow = escrow_key(id);
    let tkcc = require_tkcc(deps.storage)?;

    if !paxi_amount.is_zero() {
        BalanceLedger::debit(deps.storage, &escrow, &native_key(), paxi_amount)?;
        BalanceLedger::credit(deps.storage, sender, &native_key(), paxi_amount)?;
    }
    if !tkcc_amount.is_zero() {
        BalanceLedger::debit(deps.storage, &escrow, &tkcc, tkcc_amount)?;
        BalanceLedger::credit(deps.storage, sender, &tkcc, tkcc_amount)?;
    }

    Ok(Response::new()
        .add_attribute("action", "claim")
        .add_attribute("lottery_id", id.to_string())
        .add_attribute("winner", sender)
        .add_attribute("paxi", paxi_amount)
        .add_attribute("tkcc", tkcc_amount))
}

// ========================================================================
// 退款
// ========================================================================

pub fn execute_refund(
    mut deps: DepsMut,
    env: &Env,
    sender: &Addr,
    id: u64,
) -> Result<Response, ContractError> {
    // H-2 修复的反例：refund 在 contract.rs 的暂停白名单里（user_fund_op），
    // 用户自己的钱不应被暂停锁死，所以**不能**加 assert_not_paused。
    let mut lottery = LOTTERIES.may_load(deps.storage, id)?.ok_or(ContractError::LotteryNotFound(id))?;
    if matches!(lottery.status, LotteryStatus::Drawn) {
        return Err(ContractError::AlreadyDrawn(id));
    }
    if matches!(lottery.status, LotteryStatus::Refunded) {
        return Err(ContractError::AlreadyRefunded(id));
    }
    // N1：时间边界统一为 now < expires_at 未到期（才允许退款）
    if env.block.time.seconds() < lottery.expires_at {
        return Err(ContractError::NotExpired(id));
    }

    let cfg = LOTTERY_CONFIG.load(deps.storage)?;
    let tkcc = require_tkcc(deps.storage)?;
    let escrow = escrow_key(id);

    let is_participant = PARTICIPANTS.has(deps.storage, (id, sender));
    let is_creator = lottery.creator == *sender;
    if !is_participant && !is_creator {
        return Err(ContractError::NotParticipant(sender.clone(), id));
    }

    let mut refunded_paxi = Uint128::zero();
    let mut refunded_tkcc = Uint128::zero();

    // 参与者：退回自己的参与费（只退调用者本人，避免一次交易遍历所有人爆 gas）
    if is_participant {
        refunded_paxi = lottery.join_paxi;
        refunded_tkcc = lottery.join_tkcc;
        if !refunded_paxi.is_zero() {
            BalanceLedger::debit(deps.storage, &escrow, &native_key(), refunded_paxi)?;
            BalanceLedger::credit(deps.storage, sender, &native_key(), refunded_paxi)?;
        }
        if !refunded_tkcc.is_zero() {
            BalanceLedger::debit(deps.storage, &escrow, &tkcc, refunded_tkcc)?;
            BalanceLedger::credit(deps.storage, sender, &tkcc, refunded_tkcc)?;
        }
        PARTICIPANTS.remove(deps.storage, (id, sender));
        // C+：一并清掉 join_time（池子一旦发生退款就永久禁止开奖，该字段已无用）。
        PARTICIPANT_JOIN_TIME.remove(deps.storage, (id, sender));
        lottery.participant_count = lottery.participant_count.saturating_sub(1);
        // P1：标记「本池已发生退款」→ 永久禁止开奖（见 execute_draw_lottery 的
        // RefundStarted 检查）。只标参与者退款：建池者退的是自己的建池费，
        // 不减少参与者人数，满员开奖仍然合法，不该被这里挡掉。
        REFUND_STARTED.save(deps.storage, id, &true)?;
        // 【关键】奖池必须同步扣减，否则「过期后有人退款 → 仍满足 min_people → 还能开奖」
        // 时，开奖会按虚高的 pool_* 分配，超出 escrow 实际余额，后位领取者报 Insolvency
        // （Drawn 后又不允许 Refund），资金永久锁死。
        lottery.pool_paxi = lottery.pool_paxi.saturating_sub(refunded_paxi);
        lottery.pool_tkcc = lottery.pool_tkcc.saturating_sub(refunded_tkcc);
    }

    // 建池者：仅退还"建池费"（配置项），**绝不动用参与者的钱**。
    // BUG 1 修复：旧实现把 `pool_paxi/pool_tkcc`（含所有参与费）当建池费退，
    // 导致建池者抢跑拿走整个奖池。这里改为退**建池时实付的建池费**，并用
    // `creator_refunded` 防重复退款，彻底消除抢跑风险。
    //
    // H3 显式守卫：模板池（`is_template_pool = true`）的 creator 是合约自身占位，
    // 没有任何"建池者费用"需要退还；模板池的 timeout 退款只给参与者。
    // 旧代码虽然因为 `creator_refunded` 默认 true 而恰好安全，但那是巧合；
    // 这里显式排除，把意图写到条件里。
    if is_creator && !lottery.is_template_pool && cfg.refund_creator_on_timeout && !lottery.creator_refunded {
        // 用建池时的实付快照，不用当前的全局 cfg：
        // 否则管理员调高 `create_fee_*` 后，建池者能按住新费率退走参与者的参与费。
        let creator_paxi = lottery.create_fee_paid_paxi;
        let creator_tkcc = lottery.create_fee_paid_tkcc;
        let held_paxi = BalanceLedger::balance_of(deps.storage, &escrow, &native_key());
        let held_tkcc = BalanceLedger::balance_of(deps.storage, &escrow, &tkcc);
        let back_paxi = held_paxi.min(creator_paxi);
        let back_tkcc = held_tkcc.min(creator_tkcc);
        if !back_paxi.is_zero() {
            BalanceLedger::debit(deps.storage, &escrow, &native_key(), back_paxi)?;
            BalanceLedger::credit(deps.storage, &lottery.creator, &native_key(), back_paxi)?;
            refunded_paxi += back_paxi;
        }
        if !back_tkcc.is_zero() {
            BalanceLedger::debit(deps.storage, &escrow, &tkcc, back_tkcc)?;
            BalanceLedger::credit(deps.storage, &lottery.creator, &tkcc, back_tkcc)?;
            refunded_tkcc += back_tkcc;
        }
        lottery.creator_refunded = true;
        // 双保险：simple 版开奖只看满员，创建者退款不减人数、无实际漏洞；
        // 但万一将来放开开奖条件，这道闸能防"建池者退款后仍开奖拿分成"回归。
        REFUND_STARTED.save(deps.storage, id, &true)?;
        // 同步扣减奖池（与参与者退款同理）
        lottery.pool_paxi = lottery.pool_paxi.saturating_sub(back_paxi);
        lottery.pool_tkcc = lottery.pool_tkcc.saturating_sub(back_tkcc);
    }

    // 全部退完则标记为 Refunded
    let held_paxi = BalanceLedger::balance_of(deps.storage, &escrow, &native_key());
    let held_tkcc = BalanceLedger::balance_of(deps.storage, &escrow, &tkcc);
    if held_paxi.is_zero() && held_tkcc.is_zero() {
        // 状态索引（P2-6）：old（open 或 full）→ refunded
        let old_status = status_str(&lottery.status);
        lottery.status = LotteryStatus::Refunded;
        LOTTERY_STATUS_INDEX.remove(deps.storage, (old_status, id));
        LOTTERY_STATUS_INDEX.save(deps.storage, (status_str(&LotteryStatus::Refunded), id), &())?;
        // 与开奖同理：只有指针仍指向自己时才清。
        // 场景：老池 P1 过期后新池 P2 被开（指针 T → P2），随后 P1 被全退；
        // 若无条件清，就会误删 P2 的指针。
        if let Some(tid) = lottery.template_id {
            if ACTIVE_POOL_OF_TEMPLATE.may_load(deps.storage, tid)? == Some(id) {
                ACTIVE_POOL_OF_TEMPLATE.remove(deps.storage, tid);
            }
        }
    }
    LOTTERIES.save(deps.storage, id, &lottery)?;

    Ok(Response::new()
        .add_attribute("action", "refund")
        .add_attribute("lottery_id", id.to_string())
        .add_attribute("to", sender)
        .add_attribute("paxi", refunded_paxi)
        .add_attribute("tkcc", refunded_tkcc))
}

/// 只读视图：已开奖但**尚未被领取**的中奖金额（治理 / 审计用）。
///
/// 用途：中奖者丢失私钥 / 长期不来领取时，管理员据此评估是否有资金长久滞留托管。
/// 纯读、不动状态，因此不进暂停白名单也不受影响。
pub fn query_unclaimed(deps: Deps, id: u64) -> StdResult<UnclaimedResponse> {
    let empty = || UnclaimedResponse {
        drawn: false,
        unclaimed_paxi: Uint128::zero(),
        unclaimed_tkcc: Uint128::zero(),
        pending: vec![],
    };

    let Some(lottery) = LOTTERIES.may_load(deps.storage, id)? else {
        return Ok(empty());
    };
    if !matches!(lottery.status, LotteryStatus::Drawn) {
        return Ok(empty());
    }
    let (Some(winners), Some(payout)) = (lottery.winners.as_ref(), lottery.payout.as_ref()) else {
        return Ok(empty());
    };

    let mut unclaimed_paxi = Uint128::zero();
    let mut unclaimed_tkcc = Uint128::zero();
    let mut pending: Vec<String> = vec![];

    // 一等奖得主领 first_*（整份），二等奖得主领 second_each_*。
    // 前提：`validate_lottery_config` 已把 `first_prize_count` 限制为**恰好 1**，
    // 因此 `winners.first` 至多 1 个地址；若将来放开多一等奖，本函数与 `execute_claim`
    // 都必须改成"按人头均分"，否则会把整份奖金重复计入。
    let tiers = winners
        .first
        .iter()
        .map(|a| (a, payout.first_paxi, payout.first_tkcc))
        .chain(
            winners
                .second
                .iter()
                .map(|a| (a, payout.second_each_paxi, payout.second_each_tkcc)),
        );

    for (addr, paxi, tkcc) in tiers {
        if CLAIMED.has(deps.storage, (id, addr)) {
            continue;
        }
        unclaimed_paxi += paxi;
        unclaimed_tkcc += tkcc;
        pending.push(addr.to_string());
    }

    Ok(UnclaimedResponse { drawn: true, unclaimed_paxi, unclaimed_tkcc, pending })
}

pub fn status_str(s: &LotteryStatus) -> &'static str {
    match s {
        LotteryStatus::Open => "open",
        LotteryStatus::Full => "full",
        LotteryStatus::Drawn => "drawn",
        LotteryStatus::Refunded => "refunded",
    }
}

/// 供测试使用：构造兜底种子
pub fn debug_seed(parts: &[String], id: u64, entropy: &str, height: u64) -> [u8; 32] {
    fallback_seed(parts, id, entropy, height, 0, None)
}

/// 未使用但保留：sha256 混合（构建更平滑的熵）
#[allow(dead_code)]
pub fn mix(a: &[u8], b: &[u8]) -> [u8; 32] {
    mix_seed(&[a, b])
}
