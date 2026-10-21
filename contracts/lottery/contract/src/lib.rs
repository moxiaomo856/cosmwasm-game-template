//! paxi-lottery-contract-simple
//!
//! 抽奖（**三档简化版，双模式共存**）：
//! * A 模式玩家建池：选择档位后，建池费 / 参与费 / 人数全部由 `tier_spec()` 决定
//!   - 0 = 5 人档：参与 1 PAXI + 1 万 TKCC，建池 1 PAXI + 2 万 TKCC
//!   - 1 = 20 人档：参与 2 PAXI + 2 万 TKCC，建池 15 PAXI + 10 万 TKCC
//!   - 2 = 50 人档：参与 10 PAXI + 10 万 TKCC，建池 60 PAXI + 60 万 TKCC
//! * B2 模式平台模板池：管理员预设模板（只选档位），用户 `ActivateTemplate` 即参与，无建池费
//!
//! simple 版只看满员开奖（无 min_people 分支）；一等奖 1 人、二等奖 2 人。
//! 分配 TKCC 38/28/14/14/6、PAXI 40/30/15/15；平台池建池者那部分归运营（28%/30%）。
//!
//! * TKCC 是**外部 PRC-20 依赖**，地址可为空，发币后由管理员 `SetTkccToken` 设置。
//! * 未配置 TKCC 时，创建 / 参与 / TKCC 销毁统一返回 `TkccNotConfigured`，
//!   不 panic、不跳过扣费。

pub mod contract;
pub mod error;
pub mod helpers;
pub mod lottery;
pub mod msg;
pub mod state;
pub mod template;

pub use crate::contract::{execute, instantiate, migrate, query};
pub use crate::error::ContractError;
pub use crate::msg::{ExecuteMsg, InstantiateMsg, MigrateMsg, QueryMsg};

#[cfg(test)]
mod tests;
