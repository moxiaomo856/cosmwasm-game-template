//! 生成 CosmWasm JSON Schema（链下工具产物，供前端 / 索引器 / 钱包使用）。
//!
//! 用法（**必须带 --lib 或指定 --bin**）：
//! ```bash
//! cargo run --features library --bin schema
//! ```
//!
//! ⚠️ **这个 bin 目标不能编到 wasm32**。原因有两层：
//!   1. cosmwasm-schema 2.x 的 `write_api!` 宏在 wasm32 下会主动
//!      `compile_error!`（"can't compile schema generator for the `wasm32` arch"）；
//!   2. `#[derive(QueryResponses)]` 生成的 impl 带
//!      `#[cfg(not(target_arch = "wasm32"))]`——wasm32 下该 trait 压根不存在，
//!      于是 `write_api!` 报 "the trait `QueryResponses` is not implemented"。
//!   → 编译链上 wasm 时**必须**用 `cargo build --lib --target wasm32-unknown-unknown`。
//!
//! 产物写入当前工作目录下的 `schema/` 目录。
//!
//! 注意：鉴权模型为**交易发送方直签**（`info.sender`）。本模板不含无感（session-key）
//! 签名模块，因此消息结构中不存在 session_addr / nonce / signature 字段。

use cosmwasm_schema::write_api;

// 注意：src/bin/schema.rs 是**独立的 bin crate**，它的 `crate` 根是本文件自己，
// 不包含 lib 的模块。因此这里必须用 **lib crate 名**（package
// `paxi-lottery-contract-simple` → lib 名 `paxi_lottery_contract_simple`）
// 来引用，不能写 `crate::msg`（会报 E0432: could not find `msg` in the crate root）。
use paxi_lottery_contract_simple::msg::{ExecuteMsg, InstantiateMsg, MigrateMsg, QueryMsg};

fn main() {
    write_api! {
        instantiate: InstantiateMsg,
        execute: ExecuteMsg,
        query: QueryMsg,
        migrate: MigrateMsg,
    }
}
