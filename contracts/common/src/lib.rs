//! paxi-common
//!
//! 游戏合约（game-contract）与抽奖合约（paxi-lottery-contract-simple）共享的底层模块。
//!
//! * [`tkcc`]     — TKCC 外部 PRC-20 依赖的地址配置与 `TkccNotConfigured` 语义
//! * [`balance`]  — 合约内部余额账本（Deposit / Withdraw / 内部划转 / 偿付能力校验）
//! * [`prc20`]    — PRC-20（CW20 兼容）消息结构与调用助手
//! * [`random`]   — 随机源（commit-reveal / 兜底 sha256 混合）与不重复抽样
//! * [`admin`]    — 管理员白名单 + 多签提案
//!
//! # 关于无感签名（session）模块
//!
//! 上游版本另含 `session` 模块（无感签名：`SessionInfo` / `register_session` /
//! `validate_and_consume_session`）。**本公开模板刻意不包含该模块**——
//! 无感签名属于链下体验层能力，公开模板只保留链上可验证的最小鉴权路径，
//! 由使用者自行选择鉴权方案（默认：交易发送方直签）。
//! 若你的应用需要无感签名，可自行实现并通过 `PlayerAuth` 之类的扩展点接入。
//!
//! 详见本目录 `NOTICE`。

pub mod admin;
pub mod balance;
pub mod error;
pub mod prc20;
pub mod tkcc;
pub mod random;

pub use admin::{AdminAction, AdminProposal, AdminSet};
pub use balance::BalanceLedger;
pub use error::{AdminError, BalanceError, TkccError, NATIVE_DENOM};
pub use prc20::{
    assert_is_prc20, burn_msg, query_balance, query_token_info, transfer_from_msg, transfer_msg,
    BalanceResponse, Prc20ExecuteMsg, Prc20QueryMsg, TokenInfoResponse,
};
pub use tkcc::{
    burn_target, load_burn_mode, load_tkcc, may_load_tkcc, native_key, set_tkcc_burn_address,
    set_tkcc_token, BurnMode, TKCC_BURN_ADDRESS, TKCC_BURN_MODE, TKCC_TOKEN,
};
pub use random::{draw_distinct_indices, fallback_seed, mix_seed, RandomSource};
