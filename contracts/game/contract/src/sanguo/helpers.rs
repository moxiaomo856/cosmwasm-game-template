use cosmwasm_std::{
    Addr, BankMsg, Coin, DepsMut, Env, MessageInfo, Uint128, WasmMsg, to_json_binary,
};
use serde_json::json;

use crate::error::ContractError;
use crate::state::{BALANCES, HOUSE_BANKROLL, USER_TOTALS, empty_addr};
use crate::contract::update_user_totals;
use crate::sanguo::state::*;

// ============================================================================
// 通用 helper
// ============================================================================

/// 读取三国配置（tkcc / burn / tap）。未设置时返回错误，前端应先提示管理员配置。
pub fn load_config(deps: &dyn cosmwasm_std::Storage) -> Result<SanguoConfig, ContractError> {
    SANGUO_CONFIG
        .may_load(deps)?
        .ok_or_else(|| ContractError::SanguoNotConfigured)
}

/// 读取三国参数（未设置时回落到默认，方便部署后先跑通）
pub fn load_params(deps: &dyn cosmwasm_std::Storage) -> Result<SanguoParams, ContractError> {
    Ok(SANGUO_PARAMS.may_load(deps)?.unwrap_or_else(SanguoParams::default_sanguo))
}

// ===== 余额（复用通用合约 BALANCES）=====

pub fn player_balance(
    deps: &dyn cosmwasm_std::Storage,
    player: &Addr,
    token: &Addr,
) -> Uint128 {
    BALANCES.may_load(deps, (player, token)).ok().flatten().unwrap_or(Uint128::zero())
}

/// 从玩家余额扣除（BALANCES -，USER_TOTALS -），不足报错。
/// 扣下的代币留在合约内，由调用方贷记 HOUSE_BANKROLL（视为庄家资金）。
pub fn consume_balance(
    deps: &mut DepsMut,
    player: &Addr,
    token: &Addr,
    amount: Uint128,
) -> Result<(), ContractError> {
    let mut bal = player_balance(deps.storage, player, token);
    if bal < amount {
        return Err(ContractError::InsufficientBalance);
    }
    bal = bal.checked_sub(amount).map_err(|_| ContractError::Overflow)?;
    BALANCES.save(deps.storage, (player, token), &bal)?;
    update_user_totals(deps.branch(), token, amount, false)?;
    Ok(())
}

/// 给玩家余额加（BALANCES +，USER_TOTALS +）
pub fn credit_balance(
    deps: &mut DepsMut,
    player: &Addr,
    token: &Addr,
    amount: Uint128,
) -> Result<(), ContractError> {
    let mut bal = player_balance(deps.storage, player, token);
    bal = bal.checked_add(amount).map_err(|_| ContractError::Overflow)?;
    BALANCES.save(deps.storage, (player, token), &bal)?;
    update_user_totals(deps.branch(), token, amount, true)?;
    Ok(())
}

// ===== 庄家准备金（复用通用合约 HOUSE_BANKROLL）=====

pub fn add_house(deps: &mut DepsMut, token: &Addr, amount: Uint128) -> Result<(), ContractError> {
    if amount.is_zero() {
        return Ok(());
    }
    let mut hb = HOUSE_BANKROLL
        .may_load(deps.storage, token)?
        .unwrap_or(Uint128::zero());
    hb = hb.checked_add(amount).map_err(|_| ContractError::Overflow)?;
    HOUSE_BANKROLL.save(deps.storage, token, &hb)?;
    Ok(())
}

pub fn sub_house(deps: &mut DepsMut, token: &Addr, amount: Uint128) -> Result<(), ContractError> {
    if amount.is_zero() {
        return Ok(());
    }
    let mut hb = HOUSE_BANKROLL
        .may_load(deps.storage, token)?
        .unwrap_or(Uint128::zero());
    if hb < amount {
        return Err(ContractError::SanguoInsufficientVault);
    }
    hb = hb.checked_sub(amount).map_err(|_| ContractError::Overflow)?;
    HOUSE_BANKROLL.save(deps.storage, token, &hb)?;
    Ok(())
}

// ============================================================================
// PRC-20 转账（通用 WasmMsg，与新合约 calc_house_edge_split 同款）
// ============================================================================

/// 🟢 审计修复：返回 Result 而非 panic
pub fn build_prc20_transfer(token: &Addr, recipient: &Addr, amount: Uint128) -> Result<WasmMsg, ContractError> {
    Ok(WasmMsg::Execute {
        contract_addr: token.to_string(),
        msg: to_json_binary(&json!({
            "transfer": {
                "recipient": recipient.to_string(),
                "amount": amount.to_string()
            }
        }))?,
        funds: vec![],
    })
}

/// 🟢 审计修复：返回 Result 而非 panic
pub fn build_prc20_burn(token: &Addr, amount: Uint128) -> Result<WasmMsg, ContractError> {
    Ok(WasmMsg::Execute {
        contract_addr: token.to_string(),
        msg: to_json_binary(&json!({
            "burn": { "amount": amount.to_string() }
        }))?,
        funds: vec![],
    })
}

/// 原生 PAXI 转给抽水地址（抽卡费）
pub fn build_paxi_send(recipient: &str, amount: Uint128) -> BankMsg {
    BankMsg::Send {
        to_address: recipient.to_string(),
        amount: vec![Coin {
            denom: "upaxi".to_string(),
            amount,
        }],
    }
}

// ============================================================================
// 确定性随机种子（与参考合约同款，足够防按秒级预测）
// ============================================================================

pub fn mix_seed(a: u64, b: u64, c: u64) -> u64 {
    let mut x = a
        .wrapping_mul(6364136223846793005)
        .wrapping_add(b.wrapping_mul(1442695040888963407))
        .wrapping_add(c);
    x ^= x >> 33;
    x = x.wrapping_mul(0xff51afd7ed558ccd);
    x ^= x >> 33;
    x = x.wrapping_mul(0xc4ceb9fe1a85ec53);
    x ^= x >> 33;
    x
}

// ============================================================================
// 玩家身份解析：交易发送方 info.sender
//
// 🟢 本模板不做会话密钥 / 无感签名——玩家身份完全由链上交易签名背书，
//    取 MessageInfo.sender 即为最终玩家地址。
// ============================================================================

/// 解析玩家地址并顺带记录账号诞生时间。
///
/// 返回值即 `info.sender`；本函数保留下来是为了集中承载
/// 「首次链上活动即记录 ACCT_FIRST_SEEN」这一副作用，供所有玩家入口复用。
pub fn sanguo_auth(
    deps: &mut DepsMut,
    env: &Env,
    info: &MessageInfo,
) -> Result<Addr, ContractError> {
    let player = info.sender.clone();
    // 🎁 优化：首次链上活动即记录账号诞生时间，使赠送系统的"账号≥30天"门槛
    //    反映真实游戏龄（否则老玩家首次赠送会被误判为账号过新而拒绝）。
    let now = env.block.time.seconds();
    if ACCT_FIRST_SEEN.may_load(deps.storage, &player)?.is_none() {
        ACCT_FIRST_SEEN.save(deps.storage, &player, &now)?;
    }
    Ok(player)
}
