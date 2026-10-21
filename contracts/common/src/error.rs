//! 跨模块共享的错误类型。
//!
//! 各合约的 `ContractError` 通过 `#[from]` 把这些错误包装进去，
//! 保证两个合约的错误语义一致（尤其是 `TkccNotConfigured`）。

use cosmwasm_std::{Addr, StdError, Uint128};
use thiserror::Error;

/// 原生 PAXI 的最小单位 denom
pub const NATIVE_DENOM: &str = "upaxi";

#[derive(Error, Debug, PartialEq)]
pub enum TkccError {
    /// TKCC 尚未配置。所有依赖 TKCC 的操作都必须返回该错误，
    /// **不得 panic、不得跳过扣费**（即不允许"免费"执行付费操作）。
    #[error("TKCC token is not configured yet")]
    TkccNotConfigured,

    #[error("TKCC token is already configured: {0}")]
    TkccAlreadySet(String),

    #[error("Invalid TKCC contract address: {0}")]
    InvalidTkccAddress(String),

    #[error("TKCC contract does not expose a valid PRC-20 token_info: {0}")]
    NotPrc20(String),

    #[error("TKCC burn address is not configured")]
    BurnAddressNotConfigured,

    #[error(transparent)]
    Std(#[from] StdError),
}

#[derive(Error, Debug, PartialEq)]
pub enum BalanceError {
    #[error("Insufficient internal balance: need {need}, have {have}")]
    Insufficient { need: Uint128, have: Uint128 },

    #[error("Ledger insolvency detected for {token}: liabilities {liabilities} > assets {assets}")]
    Insolvency {
        token: String,
        liabilities: Uint128,
        assets: Uint128,
    },

    #[error("Unexpected funds: {0}")]
    UnexpectedFunds(String),

    #[error("Amount must be greater than zero")]
    ZeroAmount,

    #[error(transparent)]
    Std(#[from] StdError),
}

#[derive(Error, Debug, PartialEq)]
pub enum AdminError {
    #[error("Unauthorized: {0} is not an admin")]
    Unauthorized(Addr),

    #[error("Admin threshold must be >= 1 and <= number of admins")]
    InvalidThreshold,

    #[error("Proposal {0} not found")]
    ProposalNotFound(u64),

    #[error("Proposal {0} already confirmed by {1}")]
    AlreadyConfirmed(u64, Addr),

    #[error("Proposal {0} already executed")]
    AlreadyExecuted(u64),

    #[error("Proposal {0} expired")]
    ProposalExpired(u64),

    #[error("Cannot remove the last admin")]
    LastAdmin,

    #[error(transparent)]
    Std(#[from] StdError),
}
