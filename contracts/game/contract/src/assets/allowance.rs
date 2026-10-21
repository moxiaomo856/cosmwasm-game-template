// ========== PRC-20 Allowance 查询 ==========
use cosmwasm_std::{Deps, QueryRequest, StdResult, Uint128, WasmQuery, to_json_binary};
use crate::msg::AllowanceResponse;
use crate::helpers::validate_paxi_address;

pub fn query_allowance(
    deps: Deps,
    owner: String,
    spender: String,
    token: String,
) -> StdResult<AllowanceResponse> {
    let _owner = validate_paxi_address(deps.api, &owner)?;
    let _spender = validate_paxi_address(deps.api, &spender)?;
    let token_addr = validate_paxi_address(deps.api, &token)?;

    let query = to_json_binary(&serde_json::json!({
        "allowance": { "owner": owner, "spender": spender }
    }))?;

    // 🟢小修：失败时 fallback 返回 0，不要让查询直接报错阻塞前端
    let result: Result<AllowanceResponse, _> = deps.querier.query(&QueryRequest::Wasm(WasmQuery::Smart {
        contract_addr: token_addr.to_string(),
        msg: query,
    }));

    match result {
        Ok(r) => Ok(r),
        Err(_) => Ok(AllowanceResponse { allowance: Uint128::zero() }),
    }
}
