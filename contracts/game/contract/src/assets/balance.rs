use cosmwasm_std::{Deps, StdResult, Uint128};
use crate::msg::BalanceResponse;
// 🟠 修复：导入 EMPTY_ADDR
use crate::state::{BALANCES, empty_addr};
use crate::helpers::validate_paxi_address;

pub fn query_balance(
    deps: Deps,
    address: String,
    token: Option<String>,
) -> StdResult<BalanceResponse> {
    let addr = validate_paxi_address(deps.api, &address)?;

    let amount = if let Some(t) = token {
        // PRC-20: 用真实 token 地址
        let token_addr = validate_paxi_address(deps.api, &t)?;
        let key = (&addr, &token_addr);
        BALANCES.may_load(deps.storage, key)?.unwrap_or(Uint128::zero())
    } else {
        // 原生 PAXI: 用 empty_addr() 函数
        let key = (&addr, &empty_addr());
        BALANCES.may_load(deps.storage, key)?.unwrap_or(Uint128::zero())
    };

    Ok(BalanceResponse { amount })
}
