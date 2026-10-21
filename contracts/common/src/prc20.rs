//! PRC-20（Paxi 上的 CW20 兼容代币标准）消息结构与调用助手。
//!
//! Paxi 的 PRC-20 与 CW20 接口一致，因此这里直接按 CW20 的 JSON 形态定义：
//! `{"transfer":{"recipient":"paxi1...","amount":"1000"}}`。
//!
//! TKCC 就是这样的一个外部 PRC-20 合约，本项目**不发行、不部署**它。

use cosmwasm_std::{
    to_json_binary, Addr, CosmosMsg, Deps, QueryRequest, StdResult, Uint128, WasmMsg, WasmQuery,
    Binary,
};
use cosmwasm_schema::cw_serde;

/// PRC-20 / CW20 执行消息（本项目会用到的子集）。
#[cw_serde]
pub enum Prc20ExecuteMsg {
    /// 转账。用于 Withdraw、金库分成、黑洞销毁。
    Transfer { recipient: String, amount: Uint128 },
    /// 转账并通知接收合约（携带 msg）。用于 Deposit（Send + Receive hook）。
    Send { contract: String, amount: Uint128, msg: Binary },
    /// 销毁（要求 TKCC 合约支持）。
    Burn { amount: Uint128 },
    /// 授权额度（用于 TransferFrom 模式）。
    IncreaseAllowance { spender: String, amount: Uint128 },
    /// 从授权额度中划转。
    TransferFrom { owner: String, recipient: String, amount: Uint128 },
}

/// PRC-20 / CW20 查询消息。
#[cw_serde]
pub enum Prc20QueryMsg {
    Balance { address: String },
    TokenInfo {},
    Allowance { owner: String, spender: String },
}

#[cw_serde]
pub struct BalanceResponse {
    pub balance: Uint128,
}

#[cw_serde]
pub struct TokenInfoResponse {
    pub name: String,
    pub symbol: String,
    pub decimals: u8,
    pub total_supply: Uint128,
}

/// 构造一条 PRC-20 transfer 子消息。
pub fn transfer_msg(token: &Addr, recipient: &Addr, amount: Uint128) -> StdResult<CosmosMsg> {
    Ok(CosmosMsg::Wasm(WasmMsg::Execute {
        contract_addr: token.to_string(),
        msg: to_json_binary(&Prc20ExecuteMsg::Transfer {
            recipient: recipient.to_string(),
            amount,
        })?,
        funds: vec![],
    }))
}

/// 构造一条 PRC-20 burn 子消息。
pub fn burn_msg(token: &Addr, amount: Uint128) -> StdResult<CosmosMsg> {
    Ok(CosmosMsg::Wasm(WasmMsg::Execute {
        contract_addr: token.to_string(),
        msg: to_json_binary(&Prc20ExecuteMsg::Burn { amount })?,
        funds: vec![],
    }))
}

/// 构造一条 PRC-20 transfer_from 子消息（需要用户先 increase_allowance）。
pub fn transfer_from_msg(
    token: &Addr,
    owner: &Addr,
    recipient: &Addr,
    amount: Uint128,
) -> StdResult<CosmosMsg> {
    Ok(CosmosMsg::Wasm(WasmMsg::Execute {
        contract_addr: token.to_string(),
        msg: to_json_binary(&Prc20ExecuteMsg::TransferFrom {
            owner: owner.to_string(),
            recipient: recipient.to_string(),
            amount,
        })?,
        funds: vec![],
    }))
}

/// 查询某地址的 PRC-20 余额。
pub fn query_balance(deps: Deps, token: &Addr, who: &Addr) -> StdResult<Uint128> {
    let res: BalanceResponse = deps.querier.query(&QueryRequest::Wasm(WasmQuery::Smart {
        contract_addr: token.to_string(),
        msg: to_json_binary(&Prc20QueryMsg::Balance { address: who.to_string() })?,
    }))?;
    Ok(res.balance)
}

/// 查询 PRC-20 的 token_info（用于校验某地址确实是 PRC-20 合约）。
pub fn query_token_info(deps: Deps, token: &Addr) -> StdResult<TokenInfoResponse> {
    deps.querier.query(&QueryRequest::Wasm(WasmQuery::Smart {
        contract_addr: token.to_string(),
        msg: to_json_binary(&Prc20QueryMsg::TokenInfo {})?,
    }))
}

/// 校验某地址确实是一个 PRC-20 合约（`SetTkccToken` 时的可选强校验）。
pub fn assert_is_prc20(deps: Deps, token: &Addr) -> Result<TokenInfoResponse, crate::error::TkccError> {
    query_token_info(deps, token).map_err(|e| crate::error::TkccError::NotPrc20(e.to_string()))
}
