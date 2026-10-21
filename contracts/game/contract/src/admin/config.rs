use cosmwasm_std::{
    Addr, BankMsg, Coin, CosmosMsg, Deps, DepsMut, Env, MessageInfo, Order, Response, Uint128,
    WasmMsg, to_json_binary,
};
use crate::error::ContractError;
use crate::state::{
    ADMINS, BURN_RATE, GAME_CONFIGS, GAME_LIMITS, GAME_PARAMS, HOUSE_BANKROLL, PAUSED,
    PRC20_WHITELIST, ROUND_TIMEOUT, SWAP_MODULE, TREASURY_LIST,
    TREASURY_SHARE, TOTAL_BURNED, TOTAL_TREASURY, USER_TOTALS, empty_addr,
    MAX_ROUND_TIMEOUT_SECS, MIN_ROUND_TIMEOUT_SECS,
    GameConfig, GameLimit, GameParams,
};

// ========== 管理员检查 ==========

pub fn is_admin(deps: Deps, sender: &Addr) -> Result<bool, ContractError> {
    Ok(ADMINS.has(deps.storage, sender))
}

fn only_admin(deps: Deps, info: &MessageInfo) -> Result<(), ContractError> {
    if !is_admin(deps, &info.sender)? {
        Err(ContractError::Unauthorized)
    } else {
        Ok(())
    }
}

fn admins_count(deps: Deps) -> u64 {
    ADMINS.keys(deps.storage, None, None, Order::Ascending).count() as u64
}

// ========== 添加 / 移除管理员 ==========

pub fn add_admin(deps: DepsMut, info: MessageInfo, new_admin: String) -> Result<Response, ContractError> {
    only_admin(deps.as_ref(), &info)?;
    let addr = deps.api.addr_validate(&new_admin)?;
    if ADMINS.has(deps.storage, &addr) {
        return Err(ContractError::AdminExists);
    }
    ADMINS.save(deps.storage, &addr, &true)?;
    Ok(Response::new()
        .add_attribute("action", "add_admin")
        .add_attribute("admin", addr.to_string())
        .add_attribute("admins_count", admins_count(deps.as_ref()).to_string()))
}

pub fn remove_admin(deps: DepsMut, info: MessageInfo, target: String) -> Result<Response, ContractError> {
    only_admin(deps.as_ref(), &info)?;
    let ta = deps.api.addr_validate(&target)?;
    if ta == info.sender {
        return Err(ContractError::Unauthorized);
    }
    if !ADMINS.has(deps.storage, &ta) {
        return Err(ContractError::Unauthorized);
    }
    if admins_count(deps.as_ref()) <= 1 {
        return Err(ContractError::LastAdmin);
    }
    ADMINS.remove(deps.storage, &ta);
    Ok(Response::new()
        .add_attribute("action", "remove_admin")
        .add_attribute("admin", ta.to_string())
        .add_attribute("admins_count", admins_count(deps.as_ref()).to_string()))
}

pub fn list_admins(deps: Deps) -> Result<Vec<String>, ContractError> {
    let a: Result<Vec<String>, _> = ADMINS
        .keys(deps.storage, None, None, Order::Ascending)
        .map(|k| k.map(|x| x.to_string()))
        .collect();
    Ok(a?)
}

// ========== PRC-20 白名单 ==========

/// 🟢 第五轮新增：列出白名单里的 PRC-20 合约地址。
/// 前端充值页靠这个渲染代币选择器；没有它前端只能靠硬编码，管理员 AddPrc20 后前端看不到。
pub fn list_prc20(deps: Deps) -> Result<Vec<String>, ContractError> {
    let a: Result<Vec<String>, _> = PRC20_WHITELIST
        .keys(deps.storage, None, None, Order::Ascending)
        .map(|k| k.map(|x| x.to_string()))
        .collect();
    Ok(a?)
}

pub fn add_prc20(deps: DepsMut, info: MessageInfo, contract: String) -> Result<Response, ContractError> {
    only_admin(deps.as_ref(), &info)?;
    let addr = deps.api.addr_validate(&contract)?;
    PRC20_WHITELIST.save(deps.storage, &addr, &true)?;
    Ok(Response::new().add_attribute("action", "add_prc20").add_attribute("contract", addr.to_string()))
}

pub fn remove_prc20(deps: DepsMut, info: MessageInfo, contract: String) -> Result<Response, ContractError> {
    only_admin(deps.as_ref(), &info)?;
    let addr = deps.api.addr_validate(&contract)?;
    PRC20_WHITELIST.remove(deps.storage, &addr);
    Ok(Response::new().add_attribute("action", "remove_prc20").add_attribute("contract", addr.to_string()))
}

// ========== 暂停 ==========

pub fn set_paused(deps: DepsMut, info: MessageInfo, paused: bool) -> Result<Response, ContractError> {
    only_admin(deps.as_ref(), &info)?;
    PAUSED.save(deps.storage, &paused)?;
    Ok(Response::new().add_attribute("action", "set_paused").add_attribute("paused", paused.to_string()))
}

// ========== 游戏参数（全局 house_edge 等，可选） ==========

pub fn update_game_params(deps: DepsMut, info: MessageInfo, params: GameParams) -> Result<Response, ContractError> {
    only_admin(deps.as_ref(), &info)?;
    // 🟢 外部审计 #6：min/max bet 必须为正（0 会让该游戏永远无法下注或无上限）
    if params.min_bet.is_zero() || params.max_bet.is_zero() {
        return Err(ContractError::InvalidAmount);
    }
    if params.min_bet > params.max_bet {
        return Err(ContractError::InvalidAmount);
    }
    if params.house_edge > 10000 {
        return Err(ContractError::InvalidAmount);
    }
    GAME_PARAMS.save(deps.storage, &params)?;
    Ok(Response::new()
        .add_attribute("action", "update_game_params")
        .add_attribute("min_bet", params.min_bet)
        .add_attribute("max_bet", params.max_bet)
        .add_attribute("house_edge", params.house_edge.to_string()))
}

// ========== 燃烧配置 ==========

pub fn set_burn_rate(deps: DepsMut, info: MessageInfo, rate: u64) -> Result<Response, ContractError> {
    only_admin(deps.as_ref(), &info)?;
    if rate > 10000 {
        return Err(ContractError::InvalidAmount);
    }
    let ts = TREASURY_SHARE.load(deps.storage).unwrap_or(0u64);
    validate_split(rate, ts)?;
    BURN_RATE.save(deps.storage, &rate)?;
    Ok(Response::new().add_attribute("action", "set_burn_rate").add_attribute("rate", rate.to_string()))
}

pub fn set_game_config(deps: DepsMut, info: MessageInfo, game_id: String, config: GameConfig) -> Result<Response, ContractError> {
    only_admin(deps.as_ref(), &info)?;
    if let Some(rate) = config.burn_rate_override {
        if rate > 10000 {
            return Err(ContractError::InvalidAmount);
        }
    }
    if let Some(ts) = config.treasury_share_override {
        if ts > 10000 {
            return Err(ContractError::InvalidAmount);
        }
    }
    // 🟢 外部审计 #4 修复：交叉校验按「本次写入后的生效值组合」进行——
    //    override 缺省的一侧取全局现值，杜绝「只改一边、与另一边全局值
    //    相加超 10000」的绕过（原实现 burn=Some 且 treasury=None 时不校验）。
    let eff_br = config
        .burn_rate_override
        .unwrap_or_else(|| BURN_RATE.load(deps.storage).unwrap_or(0u64));
    let eff_ts = config
        .treasury_share_override
        .unwrap_or_else(|| TREASURY_SHARE.load(deps.storage).unwrap_or(0u64));
    validate_split(eff_br, eff_ts)?;
    GAME_CONFIGS.save(deps.storage, &game_id, &config)?;
    Ok(Response::new().add_attribute("action", "set_game_config").add_attribute("game_id", game_id))
}

// ========== 🟢 新增：游戏限额配置（加游戏 = 调这个接口） ==========

/// 🟢 审计新增：GameLimit 的合法性校验
///
/// 抽出来是为了让 `set_game_limit` 和 `add_game` 共用同一套规则 ——
/// 原实现里 `add_game` 完全没做校验，管理员可以通过 AddGame 写入
/// min_bet > max_bet、max_payout_multiplier = 0、max_daily_rounds = 0 这类非法配置，
/// 导致该游戏要么永远无法下注，要么派彩倍数校验被绕过。
pub fn validate_game_limit(limit: &GameLimit) -> Result<(), ContractError> {
    if limit.min_bet.is_zero() {
        return Err(ContractError::InvalidAmount);
    }
    if limit.min_bet > limit.max_bet {
        return Err(ContractError::InvalidAmount);
    }
    if limit.max_payout_multiplier == 0 || limit.max_payout_multiplier > 10000 {
        return Err(ContractError::InvalidAmount);
    }
    if limit.max_daily_rounds == 0 {
        return Err(ContractError::InvalidAmount);
    }
    if limit.max_daily_bet.is_zero() || limit.max_daily_bet < limit.max_bet {
        // 单局上限超过每日上限是配置矛盾，会让「每日限额」形同虚设
        return Err(ContractError::InvalidAmount);
    }
    Ok(())
}

pub fn set_game_limit(deps: DepsMut, info: MessageInfo, game_id: String, limit: GameLimit) -> Result<Response, ContractError> {
    only_admin(deps.as_ref(), &info)?;

    validate_game_limit(&limit)?;

    GAME_LIMITS.save(deps.storage, &game_id, &limit)?;

    Ok(Response::new()
        .add_attribute("action", "set_game_limit")
        .add_attribute("game_id", &game_id)
        .add_attribute("enabled", limit.enabled.to_string())
        .add_attribute("min_bet", limit.min_bet)
        .add_attribute("max_bet", limit.max_bet)
        .add_attribute("max_payout_multiplier", limit.max_payout_multiplier.to_string())
        .add_attribute("max_daily_bet", limit.max_daily_bet)
        .add_attribute("max_daily_rounds", limit.max_daily_rounds.to_string()))
}

pub fn remove_game_limit(deps: DepsMut, info: MessageInfo, game_id: String) -> Result<Response, ContractError> {
    only_admin(deps.as_ref(), &info)?;
    GAME_LIMITS.remove(deps.storage, &game_id);
    Ok(Response::new()
        .add_attribute("action", "remove_game_limit")
        .add_attribute("game_id", game_id))
}

// ========== 🟢 第二轮新增：Deduct 超时时长（可配置） ==========

/// 设置「Deduct 后多久可以按输局强制结算」的超时时长
///
/// 原来这个值写死为 `const ROUND_TIMEOUT_SECS = 3600`。不同玩法单局时长差异极大：
/// 骰子可能 3 秒结束，而两段式 Deduct/Settle 的牌类游戏可能要几分钟。
/// 写死 1 小时的问题在于：设短了会误伤正常对局（管理员能在玩家还在玩的时候
/// 强行判负），设长了资金又悬空太久。交给管理员按游戏类型调整最合适。
pub fn set_round_timeout(deps: DepsMut, info: MessageInfo, secs: u64) -> Result<Response, ContractError> {
    only_admin(deps.as_ref(), &info)?;
    if secs < MIN_ROUND_TIMEOUT_SECS || secs > MAX_ROUND_TIMEOUT_SECS {
        // 下限是硬性安全约束：设成 0 秒等于管理员可以在 Deduct 的同一个区块里
        // 立刻把玩家判负，那就是明抢。
        return Err(ContractError::InvalidRoundTimeout {
            secs,
            min: MIN_ROUND_TIMEOUT_SECS,
            max: MAX_ROUND_TIMEOUT_SECS,
        });
    }
    ROUND_TIMEOUT.save(deps.storage, &secs)?;
    Ok(Response::new()
        .add_attribute("action", "set_round_timeout")
        .add_attribute("secs", secs.to_string()))
}

// ========== Swap 模块地址 ==========

pub fn set_swap_module(deps: DepsMut, info: MessageInfo, contract: String) -> Result<Response, ContractError> {
    only_admin(deps.as_ref(), &info)?;
    deps.api.addr_validate(&contract)?;
    SWAP_MODULE.save(deps.storage, &contract)?;
    Ok(Response::new().add_attribute("action", "set_swap_module").add_attribute("contract", contract))
}

// ========== H1: 手动燃烧 ==========

pub fn burn_paxi(deps: DepsMut, info: MessageInfo, amount: Uint128) -> Result<Response, ContractError> {
    only_admin(deps.as_ref(), &info)?;
    if amount.is_zero() {
        return Err(ContractError::InvalidAmount);
    }
    let native = crate::state::empty_addr();
    let mut hb = HOUSE_BANKROLL.may_load(deps.storage, &native)?.unwrap_or(Uint128::zero());
    if hb < amount {
        return Err(ContractError::InsufficientBankrollForBurn);
    }
    hb = hb.checked_sub(amount).map_err(|_| ContractError::Overflow)?;
    HOUSE_BANKROLL.save(deps.storage, &native, &hb)?;

    let mut total = TOTAL_BURNED.may_load(deps.storage, &native)?.unwrap_or(Uint128::zero());
    total = total.checked_add(amount).map_err(|_| ContractError::Overflow)?;
    TOTAL_BURNED.save(deps.storage, &native, &total)?;

    Ok(Response::new()
        .add_message(BankMsg::Burn {
            amount: vec![Coin { denom: "upaxi".to_string(), amount }],
        })
        .add_attribute("action", "burn_paxi")
        .add_attribute("amount", amount)
        .add_attribute("total_burned", total)
        .add_attribute("house_bankroll_remaining", hb))
}

// ========== 🟢 审计新增：庄家准备金注资 ==========

/// 向 HOUSE_BANKROLL（庄家准备金）注资
///
/// 原合约里 HOUSE_BANKROLL 只能靠「玩家输局」积累，而 `burn_paxi` 只能往外掏。
/// 结果是：合约刚部署、或准备金被打光之后，**任何赢局都会因为
/// `InsufficientBankrollForProfit` 而失败** —— 游戏实际上开不了张，
/// 玩家只能一直输。必须给管理员一个正式的注资入口。
///
/// - token = None：使用 info.funds 里的 upaxi
/// - token = Some(addr)：走 PRC-20 `transfer_from`，管理员需先 approve 给本合约
pub fn fund_bankroll(
    mut deps: DepsMut,
    env: Env,
    info: MessageInfo,
    token: Option<String>,
    amount: Option<Uint128>,
) -> Result<Response, ContractError> {
    only_admin(deps.as_ref(), &info)?;

    if let Some(addr) = token {
        let token_addr = deps.api.addr_validate(&addr)?;
        // 🟢 L5 审计修复（2026-10-10）：移除 PRC-20 白名单门控。
        //    白名单语义是「限制玩家充值」，不应门控「管理员注资」——否则管理员
        //    一旦把某代币移出白名单，此前用该代币注资的 HOUSE_BANKROLL 余额
        //    将永远无法补充（玩家还能赢、还会从 bankroll 扣），最终 bankroll
        //    耗尽 → InsufficientBankrollForProfit → 游戏停摆。
        //    本入口本身 only_admin + transfer_from 授权双保险，风险可控。

        let amount = amount.ok_or(ContractError::InvalidAmount)?;
        if amount.is_zero() {
            return Err(ContractError::InvalidAmount);
        }

        let allowance = crate::assets::deposit::query_prc20_allowance(
            deps.as_ref(),
            &token_addr,
            &info.sender,
            &env.contract.address,
        )?;
        if allowance < amount {
            return Err(ContractError::InsufficientAllowance);
        }

        let mut hb = HOUSE_BANKROLL
            .may_load(deps.storage, &token_addr)?
            .unwrap_or(Uint128::zero());
        hb = hb.checked_add(amount).map_err(|_| ContractError::Overflow)?;
        HOUSE_BANKROLL.save(deps.storage, &token_addr, &hb)?;

        let pull = WasmMsg::Execute {
            contract_addr: token_addr.to_string(),
            msg: to_json_binary(&serde_json::json!({
                "transfer_from": {
                    "owner": info.sender.to_string(),
                    "recipient": env.contract.address.to_string(),
                    "amount": amount.to_string(),
                }
            }))?,
            funds: vec![],
        };

        Ok(Response::new()
            .add_message(pull)
            .add_attribute("action", "fund_bankroll")
            .add_attribute("token", token_addr.to_string())
            .add_attribute("amount", amount)
            .add_attribute("house_bankroll", hb))
    } else {
        let amount = info
            .funds
            .iter()
            .find(|c| c.denom == "upaxi")
            .map(|c| c.amount)
            .unwrap_or(Uint128::zero());
        if amount.is_zero() {
            return Err(ContractError::InvalidAmount);
        }

        let native = crate::state::empty_addr();
        let mut hb = HOUSE_BANKROLL
            .may_load(deps.storage, &native)?
            .unwrap_or(Uint128::zero());
        hb = hb.checked_add(amount).map_err(|_| ContractError::Overflow)?;
        HOUSE_BANKROLL.save(deps.storage, &native, &hb)?;

        Ok(Response::new()
            .add_attribute("action", "fund_bankroll")
            .add_attribute("token", "upaxi")
            .add_attribute("amount", amount)
            .add_attribute("house_bankroll", hb))
    }
}

// ========== 🟢 升级前清扫（2026-10-10） ==========

/// PRC-20 balance 查询返回结构（仅 admin_sweep 本地反序列化用）
#[derive(serde::Deserialize)]
struct SweepPrc20Balance {
    balance: Uint128,
}

/// 管理员把合约残留资金转到指定运营钱包（升级旧合约前的清场入口）
///
/// 处理两类没有玩家出口的资金：
///   1. HOUSE_BANKROLL（庄家准备金 / house 收益）；
///   2. 账外游离资金（如误直转到合约地址的 TKCC，不在任何台账里）。
///
/// 🟢 终审修复（P1，2026-10-10）：链上硬护栏，不再依赖调用方自律——
///   可扫上限 = 合约实际余额 − USER_TOTALS[token]（玩家应付款），超出直接拒绝；
///   转出后同步递减 HOUSE_BANKROLL（saturating，游离部分台账本就没有），
///   保证「余额 == USER_TOTALS + HOUSE_BANKROLL + 账外游离」恒等式在清扫后
///   依然满足 assert_solvency，玩家提现不会被卡死。
///
/// token = None：原生 upaxi，BankMsg::Send 转给 recipient；
/// token = Some(addr)：PRC-20，WasmMsg transfer 转给 recipient（不查白名单——
///   与 Withdraw 同理：白名单管「进」，不管「出」，防止移出白名单后资金锁死）。
/// amount = None：扫出「可扫上限」（余额 − 玩家应付款），不是全额。
pub fn admin_sweep(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
    token: Option<String>,
    recipient: String,
    amount: Option<Uint128>,
) -> Result<Response, ContractError> {
    only_admin(deps.as_ref(), &info)?;

    let recipient_addr = deps.api.addr_validate(&recipient)?;
    if recipient_addr == env.contract.address {
        return Err(ContractError::InvalidAmount);
    }

    // 统一处理两个分支的公共逻辑：算可扫上限 + 扣减台账
    let (token_addr_opt, balance, user_total) = match &token {
        None => {
            let b = deps
                .querier
                .query_balance(env.contract.address.as_str(), "upaxi")?
                .amount;
            let ut = USER_TOTALS
                .may_load(deps.storage, &empty_addr())?
                .unwrap_or(Uint128::zero());
            (None, b, ut)
        }
        Some(addr) => {
            let token_addr = deps.api.addr_validate(addr)?;
            // 终审修复：PRC-20 余额查询失败直接报错，不放行（管理员操作，宁严勿松）
            let b: Uint128 = {
                let query: cosmwasm_std::QueryRequest<cosmwasm_std::Empty> =
                    cosmwasm_std::QueryRequest::Wasm(cosmwasm_std::WasmQuery::Smart {
                        contract_addr: token_addr.to_string(),
                        msg: to_json_binary(&serde_json::json!({
                            "balance": { "address": env.contract.address.to_string() }
                        }))?,
                    });
                deps.querier.query::<SweepPrc20Balance>(&query)?.balance
            };
            let ut = USER_TOTALS
                .may_load(deps.storage, &token_addr)?
                .unwrap_or(Uint128::zero());
            (Some(token_addr), b, ut)
        }
    };

    // 🟢 硬护栏：可扫上限 = 余额 − 玩家应付款
    let sweepable = balance
        .checked_sub(user_total)
        .map_err(|_| ContractError::InsufficientBalance)?; // 余额 < 应付款 = 已经资不抵债，禁止清扫
    let amount = amount.unwrap_or(sweepable);
    if amount.is_zero() {
        return Err(ContractError::InvalidAmount);
    }
    if amount > sweepable {
        return Err(ContractError::InsufficientBalance);
    }

    // 🟢 同步递减 HOUSE_BANKROLL（saturating）：扫的部分若属准备金则台账对齐，
    //    属账外游离/超出准备金的部分台账本就没记，saturating 减到 0 即可。
    let native = empty_addr();
    let token_key = token_addr_opt.as_ref().unwrap_or(&native);
    let mut hb = HOUSE_BANKROLL
        .may_load(deps.storage, token_key)?
        .unwrap_or(Uint128::zero());
    hb = hb.saturating_sub(amount);
    HOUSE_BANKROLL.save(deps.storage, token_key, &hb)?;

    // 出账后偿付能力自检（防御性：上面护栏正常时必然通过）
    crate::contract::assert_solvency(deps.as_ref(), &env.contract.address, token_key, true)?;

    let sweep_msg = match &token_addr_opt {
        None => CosmosMsg::Bank(BankMsg::Send {
            to_address: recipient_addr.to_string(),
            amount: vec![Coin { denom: "upaxi".to_string(), amount }],
        }),
        Some(token_addr) => CosmosMsg::Wasm(WasmMsg::Execute {
            contract_addr: token_addr.to_string(),
            msg: to_json_binary(&serde_json::json!({
                "transfer": {
                    "recipient": recipient_addr.to_string(),
                    "amount": amount.to_string(),
                }
            }))?,
            funds: vec![],
        }),
    };

    Ok(Response::new()
        .add_message(sweep_msg)
        .add_attribute("action", "admin_sweep")
        .add_attribute("token", token_key.to_string())
        .add_attribute("recipient", recipient_addr.to_string())
        .add_attribute("amount", amount)
        .add_attribute("user_total_protected", user_total)
        .add_attribute("house_bankroll_remaining", hb))
}

// ========== 公共辅助 ==========

pub fn effective_burn_rate(deps: &Deps, game_id: &str) -> Result<Option<u64>, ContractError> {
    let global_rate = BURN_RATE.load(deps.storage).unwrap_or(0u64);
    if global_rate == 0 {
        return Ok(None);
    }
    if let Some(gc) = GAME_CONFIGS.may_load(deps.storage, game_id)? {
        if !gc.burn_enabled {
            return Ok(None);
        }
        if let Some(o) = gc.burn_rate_override {
            return Ok(Some(o));
        }
    }
    Ok(Some(global_rate))
}

pub fn effective_treasury_share(deps: &Deps, game_id: &str) -> Result<u64, ContractError> {
    if let Some(gc) = GAME_CONFIGS.may_load(deps.storage, game_id)? {
        if let Some(o) = gc.treasury_share_override {
            return Ok(o);
        }
    }
    Ok(TREASURY_SHARE.load(deps.storage).unwrap_or(0u64))
}

pub fn validate_split(burn_rate: u64, treasury_share: u64) -> Result<(), ContractError> {
    if burn_rate.checked_add(treasury_share).unwrap_or(10001) > 10000 {
        return Err(ContractError::SplitExceedsHundredPercent);
    }
    Ok(())
}

// ========== house_edge 三分：运营账户管理 ==========

pub fn set_treasury_list(deps: DepsMut, info: MessageInfo, treasuries: Vec<String>) -> Result<Response, ContractError> {
    only_admin(deps.as_ref(), &info)?;
    for t in &treasuries {
        if !t.is_empty() {
            deps.api.addr_validate(t)?;
        }
    }
    TREASURY_LIST.save(deps.storage, &treasuries)?;
    Ok(Response::new()
        .add_attribute("action", "set_treasury_list")
        .add_attribute("count", treasuries.len().to_string()))
}

pub fn set_treasury_share(deps: DepsMut, info: MessageInfo, share: u64) -> Result<Response, ContractError> {
    only_admin(deps.as_ref(), &info)?;
    if share > 10000 {
        return Err(ContractError::InvalidAmount);
    }
    let burn_rate = BURN_RATE.load(deps.storage).unwrap_or(0u64);
    validate_split(burn_rate, share)?;
    TREASURY_SHARE.save(deps.storage, &share)?;
    Ok(Response::new()
        .add_attribute("action", "set_treasury_share")
        .add_attribute("share", share.to_string()))
}
