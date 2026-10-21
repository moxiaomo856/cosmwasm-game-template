//! paxi-lottery-contract-simple —— B2 模板池（simple 版：模板只选档位）
//!
//! # 双模式
//! 同一个合约、同一张 `LOTTERIES` 表，靠 `Lottery.is_template_pool` 区分：
//! * A 模式（玩家自建池）：`is_template_pool = false`，建池者拿 14%/15%
//! * B2 模式（平台模板池）：`is_template_pool = true`，无建池者，其分成归运营（28%/30%）
//!
//! # 【重要】触发者无奖励
//! 触发者（第一个激活某模板、该模板当时无活跃池的用户）与普通参与者**完全一致**：
//! 扣款金额相同、分成比例相同、没有任何额外奖金/分成/特权。
//! 合约中不存在任何针对触发者的字段或资金路径。
//!
//! # simple 版差异
//! 模板不再携带自由参数（join_paxi/join_tkcc/min_people/max_people），
//! 只保存 `tier`（0/1/2），参与费与人数全部从 `tier_spec()` 查出。

use cosmwasm_std::{Addr, DepsMut, Env, MessageInfo, Response};
use paxi_common::AdminSet;

use crate::error::ContractError;
use crate::helpers::next_id;
use crate::lottery::join_pool_internal;
use crate::state::{
    tier_spec, Lottery, LotteryStatus, PoolTemplate, ACTIVE_POOL_OF_TEMPLATE, LOTTERIES,
    LOTTERY_CONFIG, LOTTERY_SEQ, LOTTERY_STATUS_INDEX, POOL_TEMPLATES, TEMPLATE_COMMIT,
    TEMPLATE_REVEALED, TEMPLATE_SECRETS, TEMPLATE_SEQ,
};

/// sha256("") 的 hex —— 拒绝空 secret 的承诺。
const EMPTY_SHA256_HEX: &str =
    "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

/// 规范化并校验 commit_hash：必须是 sha256 的 64 位 hex，且不得是 sha256("")。
fn validate_commit(commit_hash: Option<String>) -> Result<Option<String>, ContractError> {
    match commit_hash {
        Some(h) => {
            let h = h.trim().to_ascii_lowercase();
            if h.len() != 64 || !h.chars().all(|c| c.is_ascii_hexdigit()) {
                return Err(ContractError::InvalidCommitHash);
            }
            // secret="" 任何人都能算出，commit-reveal 形同虚设。
            if h == EMPTY_SHA256_HEX {
                return Err(ContractError::InvalidCommitHash);
            }
            Ok(Some(h))
        }
        None => Ok(None),
    }
}

/// 管理员：创建模板（simple 版：只选档位）
///
/// `commit_hash`：平台托管 secret 的**链头承诺**（sha256 的 64 位 hex，可选）。
/// 不提交承诺的模板，其池子永远无法开奖（只能退款）——
/// 官方奖池金额大、可被出块者反复攻击，宁可不开也不开可研磨的池。
pub fn execute_create_pool_template(
    mut deps: DepsMut,
    info: &MessageInfo,
    name: String,
    tier: u8,
    commit_hash: Option<String>,
) -> Result<Response, ContractError> {
    AdminSet::assert_admin(deps.storage, &info.sender)?;

    let name = name.trim().to_string();
    if name.is_empty() || name.chars().count() > 64 {
        return Err(ContractError::InvalidParams { reason: "name 长度非法".to_string() });
    }
    // 档位必须合法：tier 是模板唯一的参数来源
    tier_spec(tier).ok_or(ContractError::InvalidTier(tier))?;

    let commit_hash = validate_commit(commit_hash)?;

    let id = next_id(deps.storage, TEMPLATE_SEQ)?;
    if let Some(h) = &commit_hash {
        TEMPLATE_COMMIT.save(deps.storage, id, h)?;
    }
    POOL_TEMPLATES.save(
        deps.storage,
        id,
        &PoolTemplate {
            id,
            name: name.clone(),
            tier,
            active: true,
            // 前端靠它区分"等待揭示"与"无承诺只能退款"两种满员状态
            has_commit: commit_hash.is_some(),
        },
    )?;

    Ok(Response::new()
        .add_attribute("action", "create_pool_template")
        .add_attribute("template_id", id.to_string())
        .add_attribute("name", name)
        .add_attribute("tier", tier.to_string())
        .add_attribute("commit_hash", commit_hash.unwrap_or_default()))
}

/// 管理员：启停模板
pub fn execute_update_pool_template(
    deps: DepsMut,
    info: &MessageInfo,
    id: u64,
    active: bool,
) -> Result<Response, ContractError> {
    AdminSet::assert_admin(deps.storage, &info.sender)?;
    let mut tpl = POOL_TEMPLATES
        .may_load(deps.storage, id)?
        .ok_or(ContractError::TemplateNotFound(id))?;
    tpl.active = active;
    POOL_TEMPLATES.save(deps.storage, id, &tpl)?;
    Ok(Response::new()
        .add_attribute("action", "update_pool_template")
        .add_attribute("template_id", id.to_string())
        .add_attribute("active", active.to_string()))
}

/// 用户：激活模板（B2 主入口）
///
/// 【设计说明】触发者（第一个调用此消息且该模板无活跃池的人）：
///   - 不是建池者，不付建池费
///   - 不获得任何分成、奖励、特权
///   - 与第 2 到第 N 个参与者**完全一样**
pub fn execute_activate_template(
    mut deps: DepsMut,
    env: &Env,
    sender: &Addr,
    template_id: u64,
) -> Result<Response, ContractError> {
    crate::helpers::assert_not_paused(deps.as_ref())?;

    let tpl = POOL_TEMPLATES
        .may_load(deps.storage, template_id)?
        .ok_or(ContractError::TemplateNotFound(template_id))?;
    if !tpl.active {
        return Err(ContractError::TemplateInactive(template_id));
    }

    // 找活跃池（若已满员 / 已开奖 / 已退款 / **已过期**，则开新池）
    let pool_id = match ACTIVE_POOL_OF_TEMPLATE.may_load(deps.storage, template_id)? {
        Some(pid) => {
            let l = LOTTERIES
                .may_load(deps.storage, pid)?
                .ok_or(ContractError::LotteryNotFound(pid))?;
            let now = env.block.time.seconds();
            // 过期检查是必须的：join_pool_internal 对过期池直接返回 Expired；
            // 若不在这里开新池，池子过期后整个模板都会被冻死。
            // 注意 `>=`，与 join_pool_internal 的过期判断保持一致。
            // Full 显式列入（虽然 count >= max 也能命中）：防止未来有人改动
            // join_pool_internal 的状态推进导致短路失效。
            let need_new = matches!(
                l.status,
                LotteryStatus::Drawn | LotteryStatus::Refunded | LotteryStatus::Full
            ) || l.participant_count >= l.max_people
                || now >= l.expires_at;
            if need_new {
                open_new_pool_from_template(deps.branch(), env, &tpl)?
            } else {
                pid
            }
        }
        None => open_new_pool_from_template(deps.branch(), env, &tpl)?,
    };

    // 触发者与普通参与者走同一条代码路径。
    join_pool_internal(deps, env, sender, pool_id, "activate_template")
}

/// 从模板创建新池（**不扣建池费，无建池者分成**）
fn open_new_pool_from_template(
    mut deps: DepsMut,
    env: &Env,
    tpl: &PoolTemplate,
) -> Result<u64, ContractError> {
    let id = next_id(deps.storage, LOTTERY_SEQ)?;

    let now = env.block.time.seconds();
    let cfg = LOTTERY_CONFIG.load(deps.storage)?;
    // 审计：`now + duration_secs` 在 release 下会静默回绕；改用 checked_add。
    let expires_at = now
        .checked_add(cfg.duration_secs)
        .ok_or(ContractError::Overflow)?;
    let spec = tier_spec(tpl.tier).ok_or(ContractError::InvalidTier(tpl.tier))?;
    let join_tkcc_units = crate::helpers::to_tkcc_units(deps.storage, spec.join_tkcc)?;

    // 先把 commit 快照读出来：`LOTTERIES.save` 会**可变借用** `deps.storage`，
    // 若在 `Lottery { ... }` 字面量里直接调用 `TEMPLATE_COMMIT.may_load(deps.storage, ...)`
    // 就是同时可变 + 不可变借用同一 storage（E0502）。
    let commit_snapshot = TEMPLATE_COMMIT.may_load(deps.storage, tpl.id)?;
    LOTTERIES.save(
        deps.storage,
        id,
        &Lottery {
            id,
            // 平台池的 creator 用合约自身占位；开奖时 creator 分成 = 0，
            // `credit(creator, 0)` 是安全的空操作。
            creator: env.contract.address.clone(),
            tier: tpl.tier,
            join_paxi: spec.join_paxi,
            join_tkcc: join_tkcc_units,
            max_people: spec.max_people,
            pool_paxi: cosmwasm_std::Uint128::zero(),
            pool_tkcc: cosmwasm_std::Uint128::zero(),
            settled_pool_paxi: cosmwasm_std::Uint128::zero(),
            settled_pool_tkcc: cosmwasm_std::Uint128::zero(),
            participant_count: 0,
            creator_refunded: true, // 平台池无建池费，无需退款
            create_fee_paid_paxi: cosmwasm_std::Uint128::zero(),
            create_fee_paid_tkcc: cosmwasm_std::Uint128::zero(),
            status: LotteryStatus::Open,
            created_at: now,
            expires_at,
            // 平台托管的 secret：
            // * commit_hash —— 快照**本池创建那一刻**模板的当前承诺，
            //   它是开奖时到 `TEMPLATE_SECRETS` 查"本池该用哪个 secret"的 key；
            // * revealed —— 恒为 None：揭示只更新模板级状态，不回写已存在的池子
            //   （否则"满员后揭示 → 池子读不到"会造成开奖死锁）。
            // 没有承诺的模板开出的池子将无法开奖（见 lottery.rs 开奖分支）。
            commit_hash: commit_snapshot,
            revealed: None,
            winners: None,
            payout: None,
            seed: None,
            random_source: None,
            template_id: Some(tpl.id),
            is_template_pool: true,
        },
    )?;

    ACTIVE_POOL_OF_TEMPLATE.save(deps.storage, tpl.id, &id)?;
    // 状态索引（P2-6）：模板开新池 → open
    LOTTERY_STATUS_INDEX.save(
        deps.storage,
        (crate::lottery::status_str(&LotteryStatus::Open), id),
        &(),
    )?;
    Ok(id)
}

/// 管理员：揭示模板池的平台托管 secret（开奖前必须调用）。
///
/// # 哈希链轮换
/// 管理员离线生成链：随机 64-hex 的 sN，sᵢ = hex(sha256(sᵢ₊₁))，链头承诺
/// c₀ = sha256(s₁)。每次揭示下一个 sᵢ：
/// 1. 验证 sha256(sᵢ) == 当前承诺；
/// 2. 新承诺 = **sᵢ 本身**（它是 sᵢ₊₁ 承诺的原像，下一轮恰好对上）；
/// 3. 记录 revealed = sᵢ，该模板之后开出的池子在开奖时混入它。
///
/// 每个值揭示即公开、且哈希链保证绝不复用。
///
/// # 时机约束（与 A 模式 H-1 一致）
/// 当前活跃池**已过期**时禁止揭示 —— 否则管理员能看着完整参与者名单
/// 决定揭不揭示，等于一张"看菜下饭"的免费期权。此时池子只能退款。
pub fn execute_reveal_template_secret(
    deps: DepsMut,
    env: &Env,
    template_id: u64,
    secret: &str,
) -> Result<Response, ContractError> {
    POOL_TEMPLATES
        .may_load(deps.storage, template_id)?
        .ok_or(ContractError::TemplateNotFound(template_id))?;

    // 时机约束：**Open 且已过期**时禁止揭示 —— 否则管理员能看着还在变化的
    // 参与者名单决定揭不揭示，等于一张"看菜下饭"的免费期权。
    // ⚠️ Full 状态**不在限制内**：满员池正是"该开奖的"，参与者集合已封盘，
    // 揭示不存在看菜下饭空间；若过期后禁止补揭示，满员池会死锁成只能退款。
    if let Some(pid) = ACTIVE_POOL_OF_TEMPLATE.may_load(deps.storage, template_id)? {
        if let Some(l) = LOTTERIES.may_load(deps.storage, pid)? {
            let now = env.block.time.seconds();
            if matches!(l.status, LotteryStatus::Open) && now >= l.expires_at {
                return Err(ContractError::Expired(pid, l.expires_at));
            }
        }
    }

    let old_commit = TEMPLATE_COMMIT
        .may_load(deps.storage, template_id)?
        .ok_or(ContractError::TemplateCommitRequired(template_id))?;

    use sha2::{Digest as _, Sha256};
    let secret = secret.trim().to_string();
    // 哈希链要求每个揭示值本身是 64 位 hex（它是上一个承诺的原像）。
    // 不强制的话，首轮能过、第二轮起永远验证失败，管理员会被悄悄锁死。
    if secret.len() != 64 || !secret.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(ContractError::InvalidCommitHash);
    }
    // 与 validate_commit 一致：拒绝 sha256("")，虽然哈希链场景下承诺几乎
    // 不可能是它，但保持与建池承诺同一安全基线，成本为零。
    if secret == EMPTY_SHA256_HEX {
        return Err(ContractError::InvalidCommitHash);
    }
    let hex_digest = hex::encode(Sha256::digest(secret.as_bytes()));
    if hex_digest != old_commit {
        return Err(ContractError::CommitMismatch);
    }

    // 哈希链推进：下一个承诺 = 本次揭示值本身。
    // 若把新承诺写成 sha256(s_i)（本次的 hex_digest），第二轮的验证对象就对不上，
    // 链会立刻断掉 —— 这里是 s_i（下一轮 sha256(s_{i+1}) == s_i 恰好成立）。
    TEMPLATE_COMMIT.save(deps.storage, template_id, &secret)?;
    TEMPLATE_REVEALED.save(deps.storage, template_id, &secret)?;
    // 【开奖死锁修复】把"旧承诺 → 本次揭示值"记下来：
    // 以旧承诺为 key 的池子（含已满员、指针已被清掉的池子）开奖时，
    // 用池内的 commit 快照到这里查本池该用的 secret。
    TEMPLATE_SECRETS.save(
        deps.storage,
        (template_id, old_commit.as_str()),
        &secret,
    )?;

    Ok(Response::new()
        .add_attribute("action", "reveal_template_secret")
        .add_attribute("template_id", template_id.to_string())
        .add_attribute("commit", hex_digest))
}
