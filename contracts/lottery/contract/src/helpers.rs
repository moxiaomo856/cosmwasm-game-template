//! paxi-lottery-contract-simple —— 公共辅助

use cosmwasm_std::{
    coins, to_json_binary, Addr, BankMsg, CosmosMsg, Deps, DepsMut, Env, Response, StdResult,
    Storage, Uint128,
};
use cw_storage_plus::Item;

use paxi_common::{
    assert_is_prc20, AdminAction, AdminSet, BalanceLedger, BurnMode, TkccError,
    TKCC_BURN_ADDRESS, TKCC_BURN_MODE,
};

use crate::error::ContractError;
use crate::msg::{ConfigResponse, LotteryAdminAction, TkccResponse};
use crate::state::{
    Config, CONFIG, ESCROW_NS, LOTTERY_CONFIG, TKCC_DECIMALS, TOTAL_BURNED, TOTAL_OPS,
};

pub const NATIVE_DENOM: &str = "upaxi";
pub const DEFAULT_TKCC_DECIMALS: u8 = 6;

// ========================================================================
// 基础
// ========================================================================

pub fn native_key() -> Addr {
    paxi_common::native_key()
}

pub fn assert_not_paused(deps: Deps) -> Result<(), ContractError> {
    if CONFIG.load(deps.storage)?.paused {
        return Err(ContractError::Paused);
    }
    Ok(())
}

pub fn assert_admin(deps: Deps, who: &Addr) -> Result<(), ContractError> {
    AdminSet::assert_admin(deps.storage, who)?;
    Ok(())
}

pub fn tkcc_decimals(storage: &dyn Storage) -> u8 {
    TKCC_DECIMALS.may_load(storage).ok().flatten().unwrap_or(DEFAULT_TKCC_DECIMALS)
}

/// "TKCC 个数" -> 最小单位
///
/// M3：decimals 异常或 amount 溢出时返回 `ContractError::Overflow`，
/// 不再静默回落（原来会导致扣费偏小）。
pub fn to_tkcc_units(storage: &dyn Storage, count: Uint128) -> Result<Uint128, ContractError> {
    let d = tkcc_decimals(storage);
    if d > 30 {
        return Err(ContractError::Overflow);
    }
    let factor = 10u128.checked_pow(d as u32).ok_or(ContractError::Overflow)?;
    count
        .checked_mul(Uint128::new(factor))
        .map_err(|_| ContractError::Overflow)
}

pub fn require_tkcc(storage: &dyn Storage) -> Result<Addr, ContractError> {
    Ok(paxi_common::load_tkcc(storage)?)
}

/// M-2：抽奖合约 SetBurnMode 两条路径共用的前置校验。
/// 抽成函数避免 `AdminAction::SetTkccBurnMode` 与 `LotteryAdminAction::SetBurnMode`
/// 两份 copy 将来漂移（历史教训：之前 AdminCustom 有一份独立 copy 漏了黑洞校验）。
///
/// 抽奖合约的 6% TKCC 销毁**永远**走销毁路径（不依赖 fee_to_treasury 开关），
/// 所以校验是无条件的：只要 mode == BlackHole 就必须保证黑洞地址可用。
fn assert_burn_mode_ready(deps: Deps, mode: BurnMode) -> Result<(), ContractError> {
    if mode == BurnMode::BlackHole && TKCC_BURN_ADDRESS.may_load(deps.storage)?.is_none() {
        return Err(ContractError::Tkcc(paxi_common::TkccError::BurnAddressNotConfigured));
    }
    Ok(())
}

/// 开奖前确认销毁路径可用。
///
/// 默认 `BurnMode::BlackHole` 但管理员只调了 `SetTkccToken` 没调
/// `SetTkccBurnAddress` 时，开奖会在 burn 阶段失败且每次都失败，奖池永久卡死。
/// 这里在扣款前先给出清晰错误，并配合 `instantiate` 时"无黑洞地址则默认 Skip"
/// 的双保险，保证开箱即用。
/// O2：直接复用 assert_burn_mode_ready（逻辑完全相同），避免两份 copy 漂移。
pub fn assert_burn_config_ok(deps: Deps) -> Result<(), ContractError> {
    let mode = paxi_common::load_burn_mode(deps.storage)?;
    assert_burn_mode_ready(deps, mode)
}

/// 自增序列：读取 + 1 并写回（抽奖 id / 模板 id 共用）
pub fn next_id(storage: &mut dyn Storage, seq: Item<u64>) -> Result<u64, ContractError> {
    let id = seq.may_load(storage)?.unwrap_or_default() + 1;
    seq.save(storage, &id)?;
    Ok(id)
}

/// 奖池托管账户（保证 `Σ 余额 == 合约实际持有量`）
pub fn escrow_key(id: u64) -> Addr {
    Addr::unchecked(format!("{}:{}", ESCROW_NS, id))
}

// ========================================================================
// 配置校验
// ========================================================================

/// 配置合法性校验（simple 版）。
///
/// simple 版的金额 / 人数全部来自 `tier_spec()` 静态表，这里只需校验
/// 与档位无关的几项：中奖人数约束与时长。中奖人数上限以**最小档 5 人**为准。
pub fn validate_lottery_config(cfg: &crate::state::LotteryConfig) -> Result<(), ContractError> {
    // 中奖人数必须 <= 最小档人数（5），否则 5 人档的池子连中奖者都凑不齐
    let winners = cfg
        .first_prize_count
        .checked_add(cfg.second_prize_count)
        .ok_or(ContractError::Overflow)?;
    // 一等奖**不做均分**：`compute_payout` 的 `first_tkcc` / `first_paxi` 是"一等奖池总额"，
    // 而 `execute_claim` 会把该整份发给**每个**一等奖得主。因此一等奖必须**恰好 1 人**——
    // 多于 1 人时第二位得主会因托管余额不足而永远领不到（资金锁死）。
    // （若将来要支持多一等奖，需把 first 也改成 per-winner 均分，而不是放宽这里的校验。）
    if cfg.first_prize_count != 1 {
        return Err(ContractError::InvalidParams {
            reason: format!(
                "first_prize_count must be exactly 1 (got {}): 一等奖不做均分，多于 1 人会导致后位无法领取",
                cfg.first_prize_count
            ),
        });
    }
    // 二等奖是"共 N%"按人数均分，必须至少 1 人。否则 `compute_payout` 里 `n` 会被兜成 1、
    // `second_each` 等于整份二等奖，而 `winners.second` 是空数组 → 28% TKCC / 30% PAXI
    // 没人能领，永久卡在托管。
    if cfg.second_prize_count == 0 {
        return Err(ContractError::InvalidParams {
            reason: "second_prize_count must be >= 1".into(),
        });
    }
    if winners > crate::state::tier_spec(crate::state::TIER_SMALL)
        .expect("TIER_SMALL must exist")
        .max_people
    {
        return Err(ContractError::InvalidParams {
            reason: format!(
                "prize slots {} (first {} + second {}) exceed the smallest tier (5 people)",
                winners, cfg.first_prize_count, cfg.second_prize_count
            ),
        });
    }

    // 时长：0 会让奖池创建即过期
    if cfg.duration_secs == 0 {
        return Err(ContractError::InvalidParams {
            reason: "duration_secs must be > 0".into(),
        });
    }

    Ok(())
}

// ========================================================================
// 资金
// ========================================================================

pub fn build_payout_msgs(
    to: &Addr,
    token: &Addr,
    amount: Uint128,
) -> Result<Vec<CosmosMsg>, ContractError> {
    if amount.is_zero() {
        return Ok(vec![]);
    }
    if *token == native_key() {
        Ok(vec![CosmosMsg::Bank(BankMsg::Send {
            to_address: to.to_string(),
            amount: coins(amount.u128(), NATIVE_DENOM),
        })])
    } else {
        Ok(vec![paxi_common::transfer_msg(token, to, amount)?])
    }
}

/// TKCC 销毁：
/// * `Burn` —— 调用 TKCC 合约的 burn（要求 TKCC 支持）
/// * `BlackHole` —— 转到管理员配置的黑洞地址（默认，最稳妥）
/// * `Skip` —— 不销毁，转入金库（仅测试网）
///
/// L1 警告：`Burn` 模式要求 TKCC 合约实现 PRC-20 burn。如果 TKCC 不支持 burn，
/// 开奖时这里会 panic，奖池永久卡死。管理员请在 `SetTkccBurnMode { mode: Burn }`
/// 时确认 TKCC 支持 burn；建议先用 BlackHole 模式测试。
pub fn build_burn_msgs(
    deps: &DepsMut,
    token: &Addr,
    amount: Uint128,
) -> Result<Vec<CosmosMsg>, ContractError> {
    if amount.is_zero() {
        return Ok(vec![]);
    }
    // 与 assert_burn_config_ok / lottery.rs 开奖处保持同一读取路径
    let mode = paxi_common::load_burn_mode(deps.storage)?;
    match mode {
        BurnMode::Burn => Ok(vec![paxi_common::burn_msg(token, amount)?]),
        BurnMode::BlackHole => {
            let hole = TKCC_BURN_ADDRESS
                .may_load(deps.storage)?
                .ok_or(paxi_common::TkccError::BurnAddressNotConfigured)?;
            Ok(vec![paxi_common::transfer_msg(token, &hole, amount)?])
        }
        // M5：Skip 分支直接链上转账到 treasury 地址，不走 BalanceLedger::credit(treasury)。
        // 审计 / 对账时 "TOTAL_OPS 累计" 与 "Balance(treasury) 增量" 在 Skip 路径下会有差异，
        // 差额恰好是 skip 掉的那部分手续费。两条路径最终"钱都进了金库"。
        BurnMode::Skip => {
            let treasury = CONFIG.load(deps.storage)?.treasury;
            Ok(vec![paxi_common::transfer_msg(token, &treasury, amount)?])
        }
    }
}

/// 记录销毁统计
pub fn note_burned(deps: DepsMut, amount: Uint128) -> Result<(), ContractError> {
    let total = TOTAL_BURNED.may_load(deps.storage)?.unwrap_or_default();
    TOTAL_BURNED.save(deps.storage, &(total + amount))?;
    Ok(())
}

/// 记录运营分成
pub fn note_ops(deps: DepsMut, token: &Addr, amount: Uint128) -> Result<(), ContractError> {
    let total = TOTAL_OPS.may_load(deps.storage, token)?.unwrap_or_default();
    TOTAL_OPS.save(deps.storage, token, &(total + amount))?;
    Ok(())
}

/// 开奖前偿付能力自检：比较**托管账户实际余额**与记录中的奖池。
///
/// 退款会同步扣减 `pool_*`，所以正常情况下 escrow 余额 >= pool_*。
/// 若 escrow 余额 < pool_*（说明存在未预期的资金路径，记录虚高），按 pool_*
/// 分配会透支托管，后位领取者永远拿不到钱。此时**拒绝开奖**（资金仍可通过
/// Refund 取回），也不要造成永久锁死。
pub fn escrow_shortfall(
    deps: Deps,
    id: u64,
    lottery: &crate::state::Lottery,
    escrow: &Addr,
    tkcc: &Addr,
) -> Result<Option<ContractError>, ContractError> {
    let held_paxi = BalanceLedger::balance_of(deps.storage, escrow, &native_key());
    let held_tkcc = BalanceLedger::balance_of(deps.storage, escrow, tkcc);
    if held_paxi < lottery.pool_paxi || held_tkcc < lottery.pool_tkcc {
        return Ok(Some(ContractError::EscrowMismatch {
            id,
            escrow_paxi: held_paxi,
            pool_paxi: lottery.pool_paxi,
            escrow_tkcc: held_tkcc,
            pool_tkcc: lottery.pool_tkcc,
        }));
    }
    Ok(None)
}

// ========================================================================
// 管理员
// ========================================================================

pub fn exec_admin_direct(
    deps: DepsMut,
    env: &Env,
    sender: &Addr,
    action: AdminAction,
) -> Result<Response, ContractError> {
    assert_admin(deps.as_ref(), sender)?;
    let threshold = AdminSet::threshold(deps.storage);
    if threshold > 1 {
        return Err(ContractError::MultisigRequired { threshold });
    }
    apply_admin_action(deps, env, action)
}

pub fn apply_admin_action(
    deps: DepsMut,
    env: &Env,
    action: AdminAction,
) -> Result<Response, ContractError> {
    let mut resp = Response::new().add_attribute("action", "admin_action");
    match action {
        AdminAction::SetTkccToken { token } => {
            let addr = deps.api.addr_validate(&token)?;
            // P0：TKCC 是**一次性**配置。
            // 内部余额以 (user, token_addr) 为键，一旦把 TKCC 从 A 换成 B，
            // 所有未终态奖池 escrow 在 A 地址下的托管余额就再也取不出来：
            //   · 开奖 → escrow_shortfall 报 EscrowMismatch
            //   · 退款 → debit(escrow, tkcc=B) 报 Insufficient
            // 两条出路同时断掉，且合约没有任何管理员兜底提取路径 = 资金永久锁死。
            // 因此：已配置过且目标地址不同 → 直接拒绝（复用此前定义但从未使用的
            // TkccAlreadySet）；地址相同则幂等放行，方便重复调用/脚本重放。
            if let Some(cur) = paxi_common::may_load_tkcc(deps.as_ref().storage)? {
                if cur != addr {
                    return Err(TkccError::TkccAlreadySet(cur.to_string()).into());
                }
            }
            // P2-1 修复：探测失败直接报错，不再静默回退 DEFAULT_TKCC_DECIMALS。
            // decimals 一旦写入会影响所有 TKCC 费用换算（join/create 费），
            // 宁可配置失败重试，也不要把猜测值永久写入存储。
            let info = assert_is_prc20(deps.as_ref(), &addr)
                .map_err(|_| ContractError::TkccProbeFailed(addr.to_string()))?;
            paxi_common::set_tkcc_token(deps.storage, &addr)?;
            TKCC_DECIMALS.save(deps.storage, &info.decimals)?;
            resp = resp
                .add_attribute("set_tkcc_token", addr.to_string())
                .add_attribute("tkcc_decimals", info.decimals.to_string());
        }
        AdminAction::SetTkccBurnAddress { address } => {
            let addr = deps.api.addr_validate(&address)?;
            paxi_common::set_tkcc_burn_address(deps.storage, &addr)?;
            resp = resp.add_attribute("set_tkcc_burn_address", addr.to_string());
        }
        AdminAction::SetTkccBurnMode { mode } => {
            // M-2：校验抽成 assert_burn_mode_ready，避免两条路径 copy 漂移。
            // 抽奖合约 6% TKCC 销毁永远走销毁路径，无条件要求黑洞地址。
            assert_burn_mode_ready(deps.as_ref(), mode.clone())?;
            TKCC_BURN_MODE.save(deps.storage, &mode)?;
            resp = resp.add_attribute("set_tkcc_burn_mode", format!("{:?}", mode));
        }
        AdminAction::SetTreasury { treasury } => {
            let addr = deps.api.addr_validate(&treasury)?;
            let mut cfg = CONFIG.load(deps.storage)?;
            cfg.treasury = addr.clone();
            CONFIG.save(deps.storage, &cfg)?;
            resp = resp.add_attribute("set_treasury", addr.to_string());
        }
        AdminAction::SetPaused { paused } => {
            let mut cfg = CONFIG.load(deps.storage)?;
            cfg.paused = paused;
            CONFIG.save(deps.storage, &cfg)?;
            resp = resp.add_attribute("set_paused", paused.to_string());
        }
        AdminAction::UpdateAdmins { add, remove, threshold } => {
            for a in add {
                AdminSet::add(deps.storage, &deps.api.addr_validate(&a)?)?;
            }
            for r in remove {
                AdminSet::remove(deps.storage, &deps.api.addr_validate(&r)?)?;
            }
            if let Some(t) = threshold {
                AdminSet::set_threshold(deps.storage, t)?;
            }
            resp = resp.add_attribute("update_admins", "ok");
        }
        AdminAction::Custom { label: _, payload } => {
            let lottery_action: LotteryAdminAction = cosmwasm_std::from_json(&payload)?;
            return Ok(apply_lottery_admin_action(deps, env, lottery_action)?);
        }
    }
    Ok(resp)
}

/// `LotteryAdminAction` 的**唯一**实现位置。
///
/// `contract.rs::AdminCustom` 与多签路径（`AdminAction::Custom`）都委托到这里，
/// 因此新增变体时只需改这一处。（曾经 AdminCustom 里还有一份拷贝，
/// 结果两边逻辑漂移：AdminCustom 的 SetBurnMode 漏了黑洞地址前置校验。）
pub(crate) fn apply_lottery_admin_action(
    deps: DepsMut,
    env: &Env,
    action: LotteryAdminAction,
) -> Result<Response, ContractError> {
    match action {
        LotteryAdminAction::SetLotteryConfig { config } => {
            validate_lottery_config(&config)?;
            LOTTERY_CONFIG.save(deps.storage, &config)?;
            Ok(Response::new()
                .add_attribute("action", "set_lottery_config")
                .add_attribute("first_prize_count", config.first_prize_count.to_string())
                .add_attribute("second_prize_count", config.second_prize_count.to_string()))
        }
        LotteryAdminAction::SetBurnMode { mode } => {
            // M-2：复用 assert_burn_mode_ready，与 AdminAction::SetTkccBurnMode 保持一致。
            assert_burn_mode_ready(deps.as_ref(), mode.clone())?;
            TKCC_BURN_MODE.save(deps.storage, &mode)?;
            Ok(Response::new()
                .add_attribute("action", "set_burn_mode")
                .add_attribute("mode", format!("{:?}", mode)))
        }
        LotteryAdminAction::RevealTemplateSecret { template_id, secret } => {
            crate::template::execute_reveal_template_secret(deps, env, template_id, &secret)
        }
    }
}

// ========================================================================
// 查询
// ========================================================================

pub fn config_response(deps: Deps) -> StdResult<ConfigResponse> {
    let cfg: Config = CONFIG.load(deps.storage)?;
    Ok(ConfigResponse {
        treasury: cfg.treasury,
        paused: cfg.paused,
        contract_version: crate::state::CONTRACT_VERSION.to_string(),
    })
}

pub fn tkcc_response(deps: Deps) -> StdResult<TkccResponse> {
    let token = paxi_common::may_load_tkcc(deps.storage)?;
    let configured = token.is_some();
    Ok(TkccResponse {
        token,
        configured,
        decimals: if configured { Some(tkcc_decimals(deps.storage)) } else { None },
        burn_address: TKCC_BURN_ADDRESS.may_load(deps.storage)?,
        burn_mode: TKCC_BURN_MODE.may_load(deps.storage)?,
    })
}

pub fn to_json<T: serde::Serialize>(v: &T) -> StdResult<cosmwasm_std::Binary> {
    to_json_binary(v)
}

/// 偿付能力自检（管理员审计 / 测试用）
pub fn solvency_check(deps: Deps) -> Result<(), ContractError> {
    BalanceLedger::solvency_check(deps.storage)?;
    Ok(())
}
