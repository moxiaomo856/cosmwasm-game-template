//! paxi-lottery-contract-simple —— 入口

use cosmwasm_std::{
    entry_point, to_json_binary, Binary, Deps, DepsMut, Env, MessageInfo, Order, Response,
    StdError, StdResult,
};
use cw2::set_contract_version;

use paxi_common::{AdminSet, BalanceLedger, TkccError};

use crate::error::ContractError;
use crate::helpers::{
    assert_admin, config_response, exec_admin_direct, native_key, tkcc_response,
};
use crate::msg::{
    ActivePoolEntry, ActivePoolOfTemplatesResponse, AdminsResponse, BalanceResponse,
    BalancesResponse, ExecuteMsg, InstantiateMsg, LotteriesResponse, MigrateMsg,
    ParticipantsResponse, PoolTemplatesResponse, QueryMsg, ReceiveMsg, ReceiveHookMsg,
};
use crate::state::{
    Config, ACTIVE_POOL_OF_TEMPLATE, CLAIMED, CONFIG, LOTTERIES, LOTTERY_CONFIG, PARTICIPANTS,
    POOL_TEMPLATES, TKCC_DECIMALS, CONTRACT_NAME, CONTRACT_VERSION,
};

const MAX_PAGE_LIMIT: u32 = 100;
const DEFAULT_PAGE_LIMIT: u32 = 30;

#[cfg_attr(not(feature = "library"), entry_point)]
pub fn instantiate(
    mut deps: DepsMut,
    _env: Env,
    _info: MessageInfo,
    msg: InstantiateMsg,
) -> Result<Response, ContractError> {
    set_contract_version(deps.storage, CONTRACT_NAME, CONTRACT_VERSION)?;

    let treasury = deps.api.addr_validate(&msg.treasury)?;
    CONFIG.save(deps.storage, &Config { treasury, paused: false })?;
    // 传入的 lottery_config 也要过一遍校验：非法配置宁可部署失败，
    // 也不要带着"奖池永远开不了奖"的配置上线。
    let lottery_config = msg.lottery_config.unwrap_or_default();
    crate::helpers::validate_lottery_config(&lottery_config)?;
    LOTTERY_CONFIG.save(deps.storage, &lottery_config)?;

    let mut admins = Vec::with_capacity(msg.admins.len());
    for a in &msg.admins {
        admins.push(deps.api.addr_validate(a)?);
    }
    AdminSet::init(deps.storage, &admins, msg.multisig_threshold)?;

    // TKCC：现阶段传 null
    if let Some(token) = &msg.tkcc_token {
        let addr = deps.api.addr_validate(token)?;
        // P2-1 修复：探测失败 → 部署直接失败，不写猜测的 decimals。
        // 与 SetTkccToken 同一策略（见 helpers.rs 注释）。
        let info = paxi_common::assert_is_prc20(deps.as_ref(), &addr)
            .map_err(|_| ContractError::TkccProbeFailed(addr.to_string()))?;
        paxi_common::set_tkcc_token(deps.storage, &addr)?;
        TKCC_DECIMALS.save(deps.storage, &info.decimals)?;
    }
    if let Some(burn) = &msg.tkcc_burn_address {
        let addr = deps.api.addr_validate(burn)?;
        paxi_common::set_tkcc_burn_address(deps.storage, &addr)?;
        // N1：**显式**写入销毁方式，不依赖 `BurnMode::default()`（默认是 BlackHole，
        // 但显式书写语义明确，将来 Default 变更也不会造成行为漂移）。
        paxi_common::TKCC_BURN_MODE.save(deps.storage, &paxi_common::BurnMode::BlackHole)?;
    } else {
        // M1：未配置黑洞地址时，默认销毁方式退化为 Skip（转入金库），
        // 避免管理员只调 SetTkccToken 就让开奖在 burn 阶段永久卡死。
        // 生产环境应改用 SetTkccBurnAddress + SetTkccBurnMode(BlackHole/Burn)。
        paxi_common::TKCC_BURN_MODE.save(deps.storage, &paxi_common::BurnMode::Skip)?;
    }

    Ok(Response::new()
        .add_attribute("action", "instantiate")
        .add_attribute("contract", CONTRACT_NAME)
        .add_attribute("tkcc_configured", msg.tkcc_token.is_some().to_string()))
}

#[cfg_attr(not(feature = "library"), entry_point)]
pub fn execute(deps: DepsMut, env: Env, info: MessageInfo, msg: ExecuteMsg) -> Result<Response, ContractError> {
    let paused = CONFIG.load(deps.storage)?.paused;
    if paused {
        let is_admin = AdminSet::is_admin(deps.storage, &info.sender);
        // 暂停期间，用户仍可**存取自己的内部余额、领取中奖、超时退款**
        // （钱本就是他的，不应被事故期锁死）。
        // 注意 `Refund` 只退**调用者本人**的参与费 / 建池者实付建池费，
        // 且仅限已过期池，因此不会成为事故期的新攻击面。
        let user_fund_op = matches!(
            msg,
            ExecuteMsg::Deposit {}
                | ExecuteMsg::Receive(_)
                | ExecuteMsg::Withdraw { .. }
                | ExecuteMsg::Claim { .. }
                | ExecuteMsg::Refund { .. }
        );
        // 仅管理员可执行管理操作；其余池生命周期操作（建池 / 参与 /
        // 开奖 / 揭示）一律冻结，防止事故期被利用。
        // O1：模板管理也纳入 —— 事故期管理员可能需要紧急停用/调整模板。
        // ⚠️ ActivateTemplate **不在**任何白名单：它就是普通用户的参与操作
        //（触发者与普通参与者完全一致），且 execute_activate_template /
        // join_pool_internal 内部都有 assert_not_paused —— 事故期冻结参与是
        // 设计意图，若放进白名单就成了永远走不到的死分支。
        let admin_op = matches!(
            msg,
            ExecuteMsg::Admin(_)
                | ExecuteMsg::AdminCustom(_)
                | ExecuteMsg::ProposeAdminAction { .. }
                | ExecuteMsg::ConfirmAdminAction { .. }
                | ExecuteMsg::CreatePoolTemplate { .. }
                | ExecuteMsg::UpdatePoolTemplate { .. }
        );
        if !user_fund_op && !(admin_op && is_admin) {
            return Err(ContractError::Paused);
        }
    }

    match msg {
        ExecuteMsg::Deposit {} => execute_deposit_native(deps, &info),
        ExecuteMsg::Receive(wrapper) => execute_receive(deps, &info, wrapper),
        ExecuteMsg::Withdraw { token, amount } => execute_withdraw(deps, &info, token, amount),

        ExecuteMsg::CreateLottery { tier, commit_hash } => {
            crate::lottery::execute_create_lottery(deps, &env, &info.sender, tier, commit_hash)
        }
        ExecuteMsg::JoinLottery { id } => {
            crate::lottery::execute_join_lottery(deps, &env, &info.sender, id)
        }
        ExecuteMsg::ActivateTemplate { template_id } => {
            crate::template::execute_activate_template(deps, &env, &info.sender, template_id)
        }
        ExecuteMsg::RevealSecret { id, secret } => {
            crate::lottery::execute_reveal_secret(deps, &env, &info.sender, id, secret)
        }
        ExecuteMsg::DrawLottery { id } => crate::lottery::execute_draw_lottery(deps, &env, id),
        ExecuteMsg::Claim { id } => crate::lottery::execute_claim(deps, &env, &info.sender, id),
        ExecuteMsg::Refund { id } => crate::lottery::execute_refund(deps, &env, &info.sender, id),

        // ---- 管理员：模板管理（B2）----
        ExecuteMsg::CreatePoolTemplate { name, tier, commit_hash } => {
            crate::template::execute_create_pool_template(deps, &info, name, tier, commit_hash)
        }
        ExecuteMsg::UpdatePoolTemplate { id, active } => {
            crate::template::execute_update_pool_template(deps, &info, id, active)
        }

        ExecuteMsg::Admin(action) => exec_admin_direct(deps, &env, &info.sender, action),
        ExecuteMsg::AdminCustom(lottery_action) => {
            assert_admin(deps.as_ref(), &info.sender)?;
            let threshold = AdminSet::threshold(deps.storage);
            if threshold > 1 {
                return Err(ContractError::MultisigRequired { threshold });
            }
            // 统一委托给 `apply_lottery_admin_action`（`helpers.rs`），
            // 该函数是 `LotteryAdminAction` 的**唯一**实现位置。
            // 历史教训：曾经 AdminCustom 里还复制了一份 match，
            // 导致 `SetBurnMode` 的黑洞地址前置校验只在多签路径生效；
            // 收敛后新增任何变体只需改一处。
            crate::helpers::apply_lottery_admin_action(deps, &env, lottery_action)
        }
        ExecuteMsg::ProposeAdminAction { action } => {
            assert_admin(deps.as_ref(), &info.sender)?;
            let id = AdminSet::propose(deps.storage, env.block.time, &info.sender, action)?;
            Ok(Response::new()
                .add_attribute("action", "propose_admin_action")
                .add_attribute("proposal_id", id.to_string()))
        }
        ExecuteMsg::ConfirmAdminAction { id } => {
            let (reached, action) =
                AdminSet::confirm(deps.storage, env.block.time, &info.sender, id)?;
            if !reached {
                return Ok(Response::new()
                    .add_attribute("action", "confirm_admin_action")
                    .add_attribute("proposal_id", id.to_string())
                    .add_attribute("executed", "false"));
            }
            Ok(crate::helpers::apply_admin_action(deps, &env, action)?
                .add_attributes([("proposal_id", id.to_string()), ("executed", "true".to_string())]))
        }
    }
}

// ========================================================================
// 资金（与社交合约同一套内部余额模型）
// ========================================================================

fn execute_deposit_native(mut deps: DepsMut, info: &MessageInfo) -> Result<Response, ContractError> {
    if info.funds.is_empty() {
        return Err(ContractError::ZeroAmount);
    }
    let mut total = cosmwasm_std::Uint128::zero();
    for coin in &info.funds {
        if coin.denom != crate::helpers::NATIVE_DENOM {
            return Err(ContractError::InvalidDenom(coin.denom.clone()));
        }
        total = total
            .checked_add(coin.amount)
            .map_err(|_| ContractError::Overflow)?;
    }
    if total.is_zero() {
        return Err(ContractError::ZeroAmount);
    }
    BalanceLedger::credit(deps.storage, &info.sender, &native_key(), total)?;
    Ok(Response::new()
        .add_attribute("action", "deposit")
        .add_attribute("from", &info.sender)
        .add_attribute("amount", total))
}

fn execute_receive(
    mut deps: DepsMut,
    info: &MessageInfo,
    wrapper: ReceiveMsg,
) -> Result<Response, ContractError> {
    let tkcc = crate::helpers::require_tkcc(deps.storage)?;
    if info.sender != tkcc {
        return Err(ContractError::InvalidDenom(info.sender.to_string()));
    }
    if wrapper.amount.is_zero() {
        return Err(ContractError::ZeroAmount);
    }
    let hook: ReceiveHookMsg = cosmwasm_std::from_json(&wrapper.msg)?;
    match hook {
        ReceiveHookMsg::Deposit {} => {
            let sender = deps.api.addr_validate(&wrapper.sender)?;
            BalanceLedger::credit(deps.storage, &sender, &tkcc, wrapper.amount)?;
            Ok(Response::new()
                .add_attribute("action", "deposit")
                .add_attribute("from", sender)
                .add_attribute("token", tkcc)
                .add_attribute("amount", wrapper.amount))
        }
    }
}

fn execute_withdraw(
    mut deps: DepsMut,
    info: &MessageInfo,
    token: Option<String>,
    amount: cosmwasm_std::Uint128,
) -> Result<Response, ContractError> {
    if amount.is_zero() {
        return Err(ContractError::ZeroAmount);
    }
    let token_addr = match &token {
        None => native_key(),
        Some(t) => {
            let addr = deps.api.addr_validate(t)?;
            let tkcc = crate::helpers::require_tkcc(deps.storage)?;
            if addr != tkcc {
                return Err(ContractError::InvalidDenom(t.clone()));
            }
            addr
        }
    };
    BalanceLedger::debit(deps.storage, &info.sender, &token_addr, amount)?;
    let msgs = crate::helpers::build_payout_msgs(&info.sender, &token_addr, amount)?;
    Ok(Response::new()
        .add_messages(msgs)
        .add_attribute("action", "withdraw")
        .add_attribute("to", &info.sender)
        .add_attribute("token", token_addr.to_string())
        .add_attribute("amount", amount))
}

// ========================================================================
// Query
// ========================================================================

#[cfg_attr(not(feature = "library"), entry_point)]
pub fn query(deps: Deps, _env: Env, msg: QueryMsg) -> StdResult<Binary> {
    match msg {
        QueryMsg::Config {} => to_json_binary(&config_response(deps)?),
        QueryMsg::Tkcc {} => to_json_binary(&tkcc_response(deps)?),
        QueryMsg::LotteryConfig {} => to_json_binary(&LOTTERY_CONFIG.load(deps.storage)?),
        QueryMsg::Lottery { id } => to_json_binary(&LOTTERIES.may_load(deps.storage, id)?),
        QueryMsg::Lotteries { status, start_after, limit } => {
            let limit = limit.unwrap_or(DEFAULT_PAGE_LIMIT).min(MAX_PAGE_LIMIT) as usize;
            let mut lotteries = Vec::new();
            match &status {
                Some(s) => {
                    // P2-6：按状态走二级索引（(status, id) 前缀范围扫描），
                    // 把 O(全表) 降到 O(该状态条目数)。每条命中再做一次状态
                    // 复核：索引意外漂移时只是跳过（自愈），不会返回脏数据。
                    for item in crate::state::LOTTERY_STATUS_INDEX
                        .prefix(s.as_str())
                        .range(deps.storage, None, None, Order::Ascending)
                    {
                        let (id, _) = item?;
                        if let Some(sa) = start_after {
                            if id <= sa {
                                continue;
                            }
                        }
                        if let Some(l) = LOTTERIES.may_load(deps.storage, id)? {
                            if crate::lottery::status_str(&l.status) != s.as_str() {
                                continue;
                            }
                            lotteries.push(l);
                            if lotteries.len() >= limit {
                                break;
                            }
                        }
                    }
                }
                None => {
                    // 无状态过滤：直接主表升序扫描（本就高效）。
                    for item in LOTTERIES.range(deps.storage, None, None, Order::Ascending) {
                        let (id, l) = item?;
                        // N9：按 id 递增比较分页，而不是"必须精确命中 start_after"。
                        // 旧写法在 start_after 指向不存在的 id 时会一直 continue → 返回空页，
                        // 前端会误判成"已到末尾"。抽奖 id 单调递增，故该改法等价且更稳。
                        if let Some(s) = start_after {
                            if id <= s {
                                continue;
                            }
                        }
                        lotteries.push(l);
                        if lotteries.len() >= limit {
                            break;
                        }
                    }
                }
            }
            to_json_binary(&LotteriesResponse { lotteries })
        }
        QueryMsg::Participants { id } => {
            // M3：用前缀只扫本抽奖的参与者，避免 O(N) 全表扫描
            let participants = PARTICIPANTS
                .prefix(id)
                .keys(deps.storage, None, None, Order::Ascending)
                .collect::<StdResult<Vec<_>>>()?;
            to_json_binary(&ParticipantsResponse { participants })
        }
        QueryMsg::Winners { id } => to_json_binary(
            &LOTTERIES.may_load(deps.storage, id)?.and_then(|l| l.winners),
        ),
        QueryMsg::Payout { id } => {
            to_json_binary(&LOTTERIES.may_load(deps.storage, id)?.and_then(|l| l.payout))
        }
        QueryMsg::Unclaimed { id } => to_json_binary(&crate::lottery::query_unclaimed(deps, id)?),
        QueryMsg::PoolTemplates {} => {
            let templates: StdResult<Vec<_>> = POOL_TEMPLATES
                .range(deps.storage, None, None, Order::Ascending)
                .map(|r| r.map(|(_, t)| t))
                .collect();
            to_json_binary(&PoolTemplatesResponse { templates: templates? })
        }
        QueryMsg::PoolTemplate { id } => {
            to_json_binary(&POOL_TEMPLATES.may_load(deps.storage, id)?)
        }
        QueryMsg::ActivePoolOfTemplates {} => {
            let mut entries = Vec::new();
            for item in POOL_TEMPLATES.range(deps.storage, None, None, Order::Ascending) {
                let (tid, tpl) = item?;
                if !tpl.active {
                    continue;
                }
                let spec = crate::state::tier_spec(tpl.tier)
                    .ok_or(StdError::generic_err("template has invalid tier"))?;
                let active_pool_id = ACTIVE_POOL_OF_TEMPLATE.may_load(deps.storage, tid)?;
                let count = match active_pool_id {
                    Some(pid) => match LOTTERIES.may_load(deps.storage, pid)? {
                        // Open **和 Full** 都返回真实人数：
                        // Full 归零会让前端"揭示秘密"的前置校验（count >= max）
                        // 把管理员拦死，满员池永远等不到揭示（死锁）。
                        Some(l) if matches!(
                            l.status,
                            crate::state::LotteryStatus::Open | crate::state::LotteryStatus::Full
                        ) =>
                        {
                            l.participant_count
                        }
                        // 已开奖 / 已退款：展示为 0，等下次激活自动开新池
                        _ => 0,
                    },
                    None => 0,
                };
                entries.push(ActivePoolEntry {
                    template_id: tid,
                    template_name: tpl.name.clone(),
                    tier: tpl.tier,
                    active_pool_id,
                    participant_count: count,
                    max_people: spec.max_people,
                    join_paxi: spec.join_paxi,
                    join_tkcc: spec.join_tkcc,
                });
            }
            to_json_binary(&ActivePoolOfTemplatesResponse { entries })
        }
        QueryMsg::Balance { address, token } => {
            let addr = deps.api.addr_validate(&address)?;
            let token_addr = match &token {
                None => native_key(),
                Some(t) => deps.api.addr_validate(t)?,
            };
            to_json_binary(&BalanceResponse {
                address: addr.clone(),
                token,
                balance: BalanceLedger::balance_of(deps.storage, &addr, &token_addr),
            })
        }
        QueryMsg::Balances { address } => {
            let addr = deps.api.addr_validate(&address)?;
            let balances = BalanceLedger::all_balances(deps.storage, &addr)?
                .into_iter()
                .map(|(t, a)| (t.to_string(), a))
                .collect();
            to_json_binary(&BalancesResponse { balances })
        }
        QueryMsg::Admins {} => to_json_binary(&AdminsResponse {
            admins: AdminSet::list(deps.storage)?,
            threshold: AdminSet::threshold(deps.storage),
        }),
        QueryMsg::Proposal { id } => {
            to_json_binary(&paxi_common::admin::PROPOSALS.may_load(deps.storage, id)?)
        }
    }
}

#[cfg_attr(not(feature = "library"), entry_point)]
pub fn migrate(mut deps: DepsMut, _env: Env, msg: MigrateMsg) -> Result<Response, ContractError> {
    set_contract_version(deps.storage, CONTRACT_NAME, CONTRACT_VERSION)?;
    let mut resp = Response::new().add_attribute("action", "migrate");
    if let Some(token) = msg.tkcc_token {
        let addr = deps.api.addr_validate(&token)?;
        // 与 AdminAction::SetTkccToken 同一策略：TKCC 一次性。
        // migrate 这条路径会绕过管理员动作，若允许覆写，旧地址下的托管余额
        // （未终态奖池 escrow）将永久取不出来，因此这里同样拒绝换地址。
        if let Some(cur) = paxi_common::may_load_tkcc(deps.as_ref().storage)? {
            if cur != addr {
                return Err(TkccError::TkccAlreadySet(cur.to_string()).into());
            }
        }
        // P2-1 修复：探测失败 → migrate 失败，不写猜测的 decimals。
        let info = paxi_common::assert_is_prc20(deps.as_ref(), &addr)
            .map_err(|_| ContractError::TkccProbeFailed(addr.to_string()))?;
        paxi_common::set_tkcc_token(deps.storage, &addr)?;
        TKCC_DECIMALS.save(deps.storage, &info.decimals)?;
        resp = resp.add_attribute("set_tkcc_token", addr.to_string());
    }
    Ok(resp)
}

/// 未中奖且未退款的托管余额（审计用）
pub fn escrow_balance(deps: Deps, id: u64) -> (cosmwasm_std::Uint128, cosmwasm_std::Uint128) {
    let escrow = crate::helpers::escrow_key(id);
    let paxi = BalanceLedger::balance_of(deps.storage, &escrow, &native_key());
    let tkcc = match paxi_common::may_load_tkcc(deps.storage) {
        Ok(Some(t)) => BalanceLedger::balance_of(deps.storage, &escrow, &t),
        _ => cosmwasm_std::Uint128::zero(),
    };
    (paxi, tkcc)
}

/// 已领取标记（测试与前端展示用）
pub fn is_claimed(deps: Deps, id: u64, who: &cosmwasm_std::Addr) -> bool {
    CLAIMED.has(deps.storage, (id, who))
}
