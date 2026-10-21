//! TKCC 集成。
//!
//! # 关键设计：TKCC 是外部 PRC-20，地址先空着
//!
//! * TKCC **不是**本项目的合约，它是 Paxi 链上已经存在（或未来发币）的一个 PRC-20 合约。
//! * 两个业务合约各自维护 `TKCC_TOKEN: Item<Addr>`，未配置时 `may_load()` 返回 `None`。
//! * 未配置时，所有依赖 TKCC 的操作返回 `TkccNotConfigured`，
//!   **不 panic、不跳过扣费**（即不允许"免费"执行付费操作）。
//! * 发币后由管理员调用 `SetTkccToken { token }` 一次性启用，无需迁移。
//!
//! # 销毁策略
//!
//! TKCC 销毁 6%（抽奖分配）有两种实现：
//! 1. `BurnMode::Burn` —— 直接调用 PRC-20 的 `burn`（要求 TKCC 合约支持且本合约有权限）。
//! 2. `BurnMode::BlackHole` —— 转到管理员配置的黑洞地址（默认，最稳妥）。
//! 3. `BurnMode::Skip` —— 不销毁，归入运营金库（仅测试网使用，主网禁止）。

use cosmwasm_std::{Addr, StdResult, Storage};
use cosmwasm_schema::cw_serde;
use cw_storage_plus::Item;

use crate::error::TkccError;

/// TKCC 的 PRC-20 合约地址。`None` 表示尚未配置。
pub const TKCC_TOKEN: Item<Addr> = Item::new("tkcc_token");

/// TKCC 销毁黑洞地址（可选）。
pub const TKCC_BURN_ADDRESS: Item<Addr> = Item::new("tkcc_burn_address");

/// TKCC 销毁方式。
pub const TKCC_BURN_MODE: Item<BurnMode> = Item::new("tkcc_burn_mode");

#[cw_serde]
pub enum BurnMode {
    /// 调用 TKCC 合约的 burn 接口
    Burn,
    /// 转到黑洞地址（默认）
    BlackHole,
    /// 不销毁，转入金库（仅测试网）
    Skip,
}

impl Default for BurnMode {
    fn default() -> Self {
        BurnMode::BlackHole
    }
}

/// 读取 TKCC 地址；未配置则返回 `TkccNotConfigured`。
///
/// 这是所有依赖 TKCC 的业务的**唯一入口**。
pub fn load_tkcc(store: &dyn Storage) -> Result<Addr, TkccError> {
    TKCC_TOKEN.may_load(store)?.ok_or(TkccError::TkccNotConfigured)
}

/// 读取 TKCC 地址，允许未配置（用于查询 / 展示）。
pub fn may_load_tkcc(store: &dyn Storage) -> StdResult<Option<Addr>> {
    TKCC_TOKEN.may_load(store)
}

/// 管理员设置 TKCC 地址。调用方需先做 `addr_validate` 与权限校验。
pub fn set_tkcc_token(store: &mut dyn Storage, token: &Addr) -> StdResult<()> {
    TKCC_TOKEN.save(store, token)
}

/// 管理员设置销毁黑洞地址。
pub fn set_tkcc_burn_address(store: &mut dyn Storage, burn_address: &Addr) -> StdResult<()> {
    TKCC_BURN_ADDRESS.save(store, burn_address)
}

/// 读取销毁方式，未配置时使用默认的 `BlackHole`。
pub fn load_burn_mode(store: &dyn Storage) -> StdResult<BurnMode> {
    Ok(TKCC_BURN_MODE.may_load(store)?.unwrap_or_default())
}

/// 生成 TKCC 销毁子消息所需的目标信息。
///
/// 返回 `(mode, Option<blackhole_addr>)`，由业务合约自行构造消息。
pub fn burn_target(store: &dyn Storage) -> Result<(BurnMode, Option<Addr>), TkccError> {
    let mode = TKCC_BURN_MODE.may_load(store).map_err(|e| TkccError::InvalidTkccAddress(e.to_string()))?.unwrap_or_default();
    let addr = TKCC_BURN_ADDRESS.may_load(store).map_err(|e| TkccError::InvalidTkccAddress(e.to_string()))?;
    if mode == BurnMode::BlackHole && addr.is_none() {
        return Err(TkccError::BurnAddressNotConfigured);
    }
    Ok((mode, addr))
}

/// 内部余额账本中表示"原生 PAXI"的空地址键。
///
/// `BALANCES: Map<(&Addr, &Addr), Uint128>` —— 第二项为空地址表示 PAXI，
/// 非空则表示对应 PRC-20 合约地址。
pub fn native_key() -> Addr {
    Addr::unchecked("")
}

#[cfg(test)]
mod tests {
    use super::*;
    use cosmwasm_std::testing::MockStorage;

    #[test]
    fn tkcc_unconfigured() {
        let store = MockStorage::new();
        assert_eq!(load_tkcc(&store), Err(TkccError::TkccNotConfigured));
        assert_eq!(may_load_tkcc(&store).unwrap(), None);
    }

    #[test]
    fn tkcc_configured() {
        let mut store = MockStorage::new();
        let addr = Addr::unchecked("paxi1tkccxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx");
        set_tkcc_token(&mut store, &addr).unwrap();
        assert_eq!(load_tkcc(&store).unwrap(), addr);
    }

    #[test]
    fn blackhole_requires_address() {
        let mut store = MockStorage::new();
        set_tkcc_token(&mut store, &Addr::unchecked("tkcc")).unwrap();
        // 默认 BlackHole 但未配置黑洞地址 -> 报错
        assert_eq!(burn_target(&store), Err(TkccError::BurnAddressNotConfigured));
        set_tkcc_burn_address(&mut store, &Addr::unchecked("blackhole")).unwrap();
        let (mode, addr) = burn_target(&store).unwrap();
        assert_eq!(mode, BurnMode::BlackHole);
        assert_eq!(addr.unwrap().as_str(), "blackhole");
    }
}
