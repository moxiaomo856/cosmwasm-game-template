use cosmwasm_std::{
    Addr, Coin, DepsMut, Env, MessageInfo, Response, Uint128, WasmMsg, to_json_binary,
};
use crate::error::ContractError;
use crate::state::{BALANCES, PRC20_WHITELIST, empty_addr};
use crate::helpers::validate_paxi_address;
use crate::contract::update_user_totals;
use serde_json::json;

pub fn deposit(
    mut deps: DepsMut,
    env: Env,
    info: MessageInfo,
    token: Option<String>,
    amount_param: Option<Uint128>,
) -> Result<Response, ContractError> {
    let sender = info.sender.clone();

    if let Some(addr) = token {
        // ============ H3: PRC-20 主动拉取模式 ============
        let token_addr = validate_paxi_address(deps.api, &addr)?;
        if !PRC20_WHITELIST.has(deps.storage, &token_addr) {
            return Err(ContractError::ContractNotWhitelisted);
        }
        let expected = amount_param.ok_or(ContractError::InvalidAmount)?;
        if expected.is_zero() {
            return Err(ContractError::InvalidAmount);
        }

        // 🟢 外部审计 #9：查询失败与授权不足分开报——查询失败原来是 Std 裸传，
        //    前端会误显示成「授权不足」，用户反复 approve 也无效。
        let allowance = query_prc20_allowance(deps.as_ref(), &token_addr, &sender, &env.contract.address)
            .map_err(|e| ContractError::Prc20AllowanceQueryFailed(e.to_string()))?;
        if allowance < expected {
            return Err(ContractError::InsufficientAllowance);
        }

        let transfer_msg = WasmMsg::Execute {
            contract_addr: token_addr.to_string(),
            msg: to_json_binary(&json!({
                "transfer_from": {
                    "owner": sender.to_string(),
                    "recipient": env.contract.address.to_string(),
                    "amount": expected.to_string(),
                }
            }))?,
            funds: vec![],
        };

        let key = (&sender, &token_addr);
        let mut balance = BALANCES
            .may_load(deps.storage, key)?
            .unwrap_or(Uint128::zero());
        balance = balance
            .checked_add(expected)
            .map_err(|_| ContractError::Overflow)?;
        BALANCES.save(deps.storage, key, &balance)?;

        // 所有币种都更新 USER_TOTALS
        update_user_totals(deps.branch(), &token_addr, expected, true)?;

        Ok(Response::new()
            .add_message(transfer_msg)
            .add_attribute("action", "prc20_deposit")
            .add_attribute("token", token_addr.to_string())
            .add_attribute("amount", expected)
            .add_attribute("method", "transfer_from"))
    } else {
        // ============ 原生 PAXI ============
        let amount = info
            .funds
            .iter()
            .find(|c| c.denom == "upaxi")
            .map(|c| c.amount)
            .unwrap_or(Uint128::zero());
        if amount.is_zero() {
            return Err(ContractError::InvalidAmount);
        }

        let empty = empty_addr();
        let key = (&sender, &empty);
        let mut balance = BALANCES
            .may_load(deps.storage, key)?
            .unwrap_or(Uint128::zero());
        balance = balance
            .checked_add(amount)
            .map_err(|_| ContractError::Overflow)?;
        BALANCES.save(deps.storage, key, &balance)?;

        // 所有币种都更新 USER_TOTALS
        update_user_totals(deps.branch(), &empty, amount, true)?;

        Ok(Response::new()
            .add_attribute("action", "native_deposit")
            .add_attribute("amount", amount))
    }
}

/// 查询 PRC-20 allowance（owner → spender）
///
/// 🟢 改为 pub：`admin::config::fund_bankroll` 需要复用这个查询。
pub fn query_prc20_allowance(
    deps: cosmwasm_std::Deps,
    token: &Addr,
    owner: &Addr,
    spender: &Addr,
) -> Result<Uint128, ContractError> {
    let query = to_json_binary(&json!({
        "allowance": {
            "owner": owner.to_string(),
            "spender": spender.to_string()
        }
    }))?;

    let raw: serde_json::Value = deps.querier.query(&cosmwasm_std::QueryRequest::Wasm(
        cosmwasm_std::WasmQuery::Smart {
            contract_addr: token.to_string(),
            msg: query,
        },
    ))?;

    let amount = raw
        .as_str()
        .and_then(|s| s.parse::<u128>().ok())
        .or_else(|| raw.get("allowance").and_then(|v| v.as_str()).and_then(|s| s.parse().ok()))
        .or_else(|| raw.get("amount").and_then(|v| v.as_str()).and_then(|s| s.parse().ok()))
        .or_else(|| raw.get("data")
            .and_then(|d| d.get("allowance").and_then(|v| v.as_str()).and_then(|s| s.parse().ok())))
        .or_else(|| raw.get("data")
            .and_then(|d| d.get("amount").and_then(|v| v.as_str()).and_then(|s| s.parse().ok())))
        .unwrap_or(0);

    Ok(Uint128::from(amount))
}
