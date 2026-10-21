use cosmwasm_std::{Addr, Api, StdResult};

pub fn validate_paxi_address(api: &dyn Api, addr: &str) -> StdResult<Addr> {
    api.addr_validate(addr)
}

// 🟢 第二轮清理：删除了 to_base64 / to_hex / from_hex —— 三个函数在 src/ 下零调用。
// 其中 to_hex 还和 engines.rs 里 Card::to_hex 同名，纯属误导。
// 注：hex / base64 依赖本身在 src/ 下已零直接引用（无感验签机制移除后），
// Cargo.toml 中保留仅为不动依赖清单；如需精简可由 owner 自行删除。
//
// 注：时间相关功能统一使用 env.block.time.seconds()，
// 不再提供辅助函数避免误导。
