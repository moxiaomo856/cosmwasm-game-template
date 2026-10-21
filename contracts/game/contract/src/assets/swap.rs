// 本轮改动（第十二轮）：swap.rs:142 CosmosMsg::Stargate → CosmosMsg::Any（cosmwasm-std 2.0.4 中 Stargate 已 deprecated，仅编译期警告，不影响功能但应按 2.0 规范修正）。
use cosmwasm_std::{
    Addr, AnyMsg, Binary, CosmosMsg, DepsMut, Env, MessageInfo, ReplyOn, Response, SubMsg,
    Uint128, WasmMsg, to_json_binary,
};
use serde_json::json;
use crate::error::ContractError;
use crate::state::{BALANCES, PRC20_WHITELIST, SWAP_MODULE, empty_addr};
use crate::helpers::validate_paxi_address;
use crate::contract::update_user_totals;

// ========================================================================
// H7 + H9 FIX: Swap follows official guide + internal ledger complete
//
// Official facts (DApp 3.8 + CLI 1.13/1.16):
//   - Swap is native Cosmos SDK module, NOT a Wasm contract
//   - Module address: 见 crate::state::DEFAULT_SWAP_MODULE（链级基础设施地址，
//     非部署者资产；部署到其它链时用 InstantiateMsg.swap_module 覆盖）
//   - typeUrl: "/x.swap.types.MsgSwap"
//   - MsgSwap protobuf: creator(1), prc20(2), offer_denom(3), offer_amount(4), min_receive(5)
//
// Native PAXI path:
//   MsgSwap deducts upaxi from creator(=contract) bank via SubMsg.funds
//
// PRC-20 path:
//   1. increase_allowance(swap_module, offer_amount) -- native module deducts from allowance
//   2. MsgSwap native module performs the swap
//
// H9 FIX: Complete internal ledger on BOTH sides:
//   Before MsgSwap: BALANCES[offer_denom] -= offer_amount, USER_TOTALS[offer_denom] -= offer_amount
//   After  MsgSwap (reply id=2): BALANCES[prc20] += actual_receive, USER_TOTALS[prc20] += actual_receive
//   On MsgSwap error: ROLLBACK offer_denom BALANCES + USER_TOTALS
//
// NOTE: CosmWasm 2.0 uses CosmosMsg::Any (NOT deprecated CosmosMsg::Stargate).
//
// 🟢 第二轮修复：`deps` 参数少了 `mut`。
//   下面第 111 行要调 `update_user_totals(deps.branch(), ...)`，
//   而 `DepsMut::branch(&mut self)` 需要可变借用 —— 非 mut 绑定会直接编译失败：
//   "cannot borrow `deps` as mutable, as it is not declared as mutable"。
// ========================================================================

/// Manual protobuf encode for MsgSwap (5 string fields, all length-delimited wire type = 2)
///
/// ⚠️ L4 审计标注（2026-10-10）：隐含假设 —— 字段号 1..=15。
///    `(field << 3 | 2) as u8` 只有在 field ≤ 15 时才能装进单字节 tag
///    （field ≥ 16 需要 2 字节 varint tag，u8 截断会编码出垃圾 tx）。
///    当前 MsgSwap 恰好只有 5 个字段；将来若 protobuf schema 扩到 ≥16，
///    debug_assert 会在测试期立刻暴露（release 下 assert 被裁掉，零体积开销）。
fn encode_msg_swap(creator: &str, prc20: &str, offer_denom: &str, offer_amount: &str, min_receive: &str) -> Vec<u8> {
    let mut out = Vec::new();
    let write_string = |out: &mut Vec<u8>, field: u32, s: &str| {
        debug_assert!(field >= 1 && field < 16, "pb tag requires single-byte field number < 16");
        out.push((field << 3 | 2) as u8);  // tag = (field << 3) | wire_type(2)
        write_varint(out, s.len() as u64);  // length varint
        out.extend_from_slice(s.as_bytes());  // UTF-8 bytes
    };
    write_string(&mut out, 1, creator);
    write_string(&mut out, 2, prc20);
    write_string(&mut out, 3, offer_denom);
    write_string(&mut out, 4, offer_amount);
    write_string(&mut out, 5, min_receive);
    out
}

fn write_varint(out: &mut Vec<u8>, mut v: u64) {
    while v >= 0x80 {
        out.push(((v & 0x7F) | 0x80) as u8);
        v >>= 7;
    }
    out.push(v as u8);
}

/// SubMsg id for swap MsgSwap (used in reply for ledger settlement)
pub const SWAP_SUBMSG_ID: u64 = 2;

pub fn swap(
    mut deps: DepsMut,
    env: Env,
    info: MessageInfo,
    prc20: String,
    offer_denom: String,
    offer_amount: Uint128,
    min_receive: Uint128,
) -> Result<Response, ContractError> {
    let sender = info.sender.clone();

    // 1. Load swap_module address (native module address)
    let swap_module_addr = SWAP_MODULE
        .load(deps.storage)
        .map_err(|_| ContractError::InvalidAmount)?;
    let swap_module = validate_paxi_address(deps.api, &swap_module_addr)?;

    // 2. Whitelist check -- both target prc20 and offer_denom
    let prc20_addr = validate_paxi_address(deps.api, &prc20)?;
    if !PRC20_WHITELIST.has(deps.storage, &prc20_addr) {
        return Err(ContractError::ContractNotWhitelisted);
    }
    if offer_amount.is_zero() {
        return Err(ContractError::InvalidAmount);
    }

    // Build token refs for internal ledger
    let (offer_token_addr, offer_is_native) = if offer_denom == "upaxi" {
        (empty_addr(), true)
    } else {
        let t = validate_paxi_address(deps.api, &offer_denom)?;
        // 🟢 审计修复：不再校验 offer 侧的白名单（理由同 withdraw.rs）。
        // 用户手里的币本来就是经白名单充值 / Swap 换入 / 游戏赢得的，
        // 若因管理员把该币移出白名单就禁止换回，等于冻结用户资产。
        // 白名单只用于约束「目标 prc20」（即换入什么币），见上方 prc20_addr 的校验。
        (t, false)
    };

    // H9 FIX: Deduct offer_denom from internal ledger (BALANCES + USER_TOTALS)
    {
        let key = (&sender, &offer_token_addr);
        let mut balance = BALANCES.may_load(deps.storage, key)?.unwrap_or(Uint128::zero());
        if balance < offer_amount {
            return Err(ContractError::InsufficientBalance);
        }
        balance = balance.checked_sub(offer_amount).map_err(|_| ContractError::Overflow)?;
        BALANCES.save(deps.storage, key, &balance)?;

        // H9: Also deduct USER_TOTALS to maintain sum(BALANCES) == USER_TOTALS invariant
        update_user_totals(deps.branch(), &offer_token_addr, offer_amount, false)?;
    }

    // 5. Build MsgSwap protobuf
    let creator = env.contract.address.to_string();
    let msg_swap_bytes = encode_msg_swap(
        &creator,
        &prc20,
        &offer_denom,
        &offer_amount.to_string(),
        &min_receive.to_string(),
    );

    // Payload for reply: all context needed to settle ledger on swap completion
    let payload = Binary::from(json!({
        "player": sender.to_string(),
        "offer_denom": offer_denom,
        "offer_amount": offer_amount.to_string(),
        "prc20": prc20,
        "min_receive": min_receive.to_string(),
        "offer_is_native": offer_is_native,
    }).to_string().as_bytes().to_vec());

    let mut resp = Response::new();

    // 6. Construct MsgSwap (CosmosMsg::Any) wrapped in SubMsg for reply callback
    //    注意：cosmwasm-std 2.0.4 中 CosmosMsg::Any 是元组变体 Any(AnyMsg)，
    //    需用 CosmosMsg::Any(AnyMsg { type_url, value }) 构造（不同于旧版 Stargate 的结构体语法）。
    let any_msg: CosmosMsg = CosmosMsg::Any(AnyMsg {
        type_url: "/x.swap.types.MsgSwap".to_string(),
        value: Binary::from(msg_swap_bytes),
    });

    if offer_is_native {
        // === Native PAXI ===
        // MsgSwap native module deducts upaxi DIRECTLY from creator(=contract) bank balance.
        // Contract bank has upaxi from user deposits, so no extra funds mechanism needed.
        // SubMsg has NO funds field in CosmWasm 2.0 -- module reads creator bank directly.
        let sub = SubMsg {
            id: SWAP_SUBMSG_ID,
            msg: any_msg,
            gas_limit: None,
            reply_on: ReplyOn::Always,
            payload,
        };
        resp = resp.add_submessage(sub);
        resp = resp.add_attribute("action", "swap_upaxi_stargate");
    } else {
        // === PRC-20 ===
        // Step 1: increase_allowance so native module can deduct PRC-20 from creator allowance
        let increase_allowance = WasmMsg::Execute {
            contract_addr: offer_denom.clone(),
            msg: to_json_binary(&json!({
                "increase_allowance": {
                    "spender": swap_module.to_string(),
                    "amount": offer_amount.to_string(),
                }
            }))?,
            funds: vec![],
        };
        resp = resp.add_message(increase_allowance);

        // Step 2: MsgSwap via SubMsg for reply -- no funds needed (module uses allowance)
        let sub = SubMsg {
            id: SWAP_SUBMSG_ID,
            msg: any_msg,
            gas_limit: None,
            reply_on: ReplyOn::Always,
            payload,
        };
        resp = resp.add_submessage(sub);
        resp = resp.add_attribute("action", "swap_prc20_stargate");
        resp = resp.add_attribute("swap_module", swap_module);
    }

    resp = resp.add_attribute("offer_denom", offer_denom);
    resp = resp.add_attribute("offer_amount", offer_amount);
    resp = resp.add_attribute("min_receive", min_receive);
    resp = resp.add_attribute("prc20_target", prc20);
    resp = resp.add_attribute("h9", "ledger_both_sides_settle_in_reply");

    Ok(resp)
}
