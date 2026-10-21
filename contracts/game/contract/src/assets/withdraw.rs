use cosmwasm_std::{Addr, BankMsg, Coin, DepsMut, Env, MessageInfo, Response, Uint128, WasmMsg, to_json_binary};
use crate::error::ContractError;
use crate::state::{BALANCES, empty_addr};
use crate::helpers::validate_paxi_address;
use crate::contract::update_user_totals;
use serde_json::json;

pub fn withdraw(
    mut deps: DepsMut,
    env: Env,
    info: MessageInfo,
    token: Option<String>,
    amount: Uint128,
) -> Result<Response, ContractError> {
    if amount.is_zero() {
        return Err(ContractError::InvalidAmount);
    }

    let sender = info.sender.clone();

    if let Some(addr) = token {
        // === PRC-20 ===
        let token_addr = validate_paxi_address(deps.api, &addr)?;
        // 🟢 审计修复（高危）：原实现在这里校验 PRC-20 白名单。
        // 但白名单是管理员可随时 RemovePrc20 的 —— 一旦某个代币被移出白名单，
        // 已经充值进来的用户就**永远无法提现**，资金被锁死在合约里。
        // 白名单的正确职责是「限制哪些币能充进来」，不该拦截提现：
        // 用户余额里只可能存在通过白名单充值 / 游戏赢得 / Swap 换入的币，本身就是安全的。
        // （PRC20_WHITELIST 的 import 随之移除）

        let key = (&sender, &token_addr);
        let mut balance = BALANCES
            .may_load(deps.storage, key)?
            .unwrap_or(Uint128::zero());
        if balance < amount {
            return Err(ContractError::InsufficientBalance);
        }
        balance = balance
            .checked_sub(amount)
            .map_err(|_| ContractError::Overflow)?;
        BALANCES.save(deps.storage, key, &balance)?;

        // 所有币种都更新 USER_TOTALS
        update_user_totals(deps.branch(), &token_addr, amount, false)?;

        // 🟢 第二轮新增：提现是资金出口，出账后验一遍偿付能力
        crate::contract::assert_solvency(deps.as_ref(), &env.contract.address, &token_addr, true)?;

        let wasm_msg = WasmMsg::Execute {
            contract_addr: token_addr.to_string(),
            msg: to_json_binary(&json!({
                "transfer": {
                    "recipient": sender.to_string(),
                    "amount": amount.to_string()
                }
            }))?,
            funds: vec![],
        };

        Ok(Response::new()
            .add_message(wasm_msg)
            .add_attribute("action", "prc20_withdraw")
            .add_attribute("token", token_addr.to_string())
            .add_attribute("amount", amount)
            .add_attribute("user", sender))
    } else {
        // === 原生 PAXI ===
        let empty = empty_addr();
        let key = (&sender, &empty);
        let mut balance = BALANCES
            .may_load(deps.storage, key)?
            .unwrap_or(Uint128::zero());
        if balance < amount {
            return Err(ContractError::InsufficientBalance);
        }
        balance = balance
            .checked_sub(amount)
            .map_err(|_| ContractError::Overflow)?;
        BALANCES.save(deps.storage, key, &balance)?;

        // 所有币种都更新 USER_TOTALS
        update_user_totals(deps.branch(), &empty, amount, false)?;

        // 🟢 第二轮新增：同上，原生提现后做偿付能力兜底检查
        crate::contract::assert_solvency(deps.as_ref(), &env.contract.address, &empty, true)?;

        let bank_msg = BankMsg::Send {
            to_address: sender.to_string(),
            amount: vec![Coin {
                denom: "upaxi".to_string(),
                amount,
            }],
        };

        Ok(Response::new()
            .add_message(bank_msg)
            .add_attribute("action", "native_withdraw")
            .add_attribute("amount", amount)
            .add_attribute("user", sender))
    }
}
