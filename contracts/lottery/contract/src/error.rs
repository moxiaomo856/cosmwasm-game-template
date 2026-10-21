//! paxi-lottery-contract-simple —— 错误定义

use cosmwasm_std::{Addr, StdError, Uint128};
use thiserror::Error;

use paxi_common::{AdminError, BalanceError, TkccError};

#[derive(Error, Debug)]
pub enum ContractError {
    #[error(transparent)]
    Std(#[from] StdError),

    #[error(transparent)]
    Tkcc(#[from] TkccError),

    #[error(transparent)]
    Balance(#[from] BalanceError),

    #[error(transparent)]
    Admin(#[from] AdminError),

    #[error("Contract is paused")]
    Paused,

    #[error("Amount must be greater than zero")]
    ZeroAmount,

    #[error("Unauthorized")]
    Unauthorized,

    #[error("Multisig required: threshold is {threshold}, use ProposeAdminAction")]
    MultisigRequired { threshold: u32 },

    #[error("Only upaxi is accepted as native denom, got {0}")]
    InvalidDenom(String),

    #[error("Arithmetic overflow")]
    Overflow,

    // ---------------- 抽奖 ----------------
    #[error("Lottery {0} not found")]
    LotteryNotFound(u64),

    #[error("Lottery {0} is not open for joining")]
    LotteryNotOpen(u64),

    #[error("Lottery {0} has not been drawn yet")]
    LotteryNotDrawn(u64),

    #[error("{0} already joined lottery {1}")]
    AlreadyJoined(Addr, u64),

    /// 建池者不能参与自己建的池。
    ///
    /// 建池者掌握 commit-reveal 的 secret（如果他填了），又能在最后时刻才决定
    /// 要不要加入 —— 他可以先算出结果，不中奖就不加入。这个漏洞跟用什么熵源
    /// 无关，只能靠"不让建池者下场"来堵。建池者本就拿 14%/15% 的建池者分成，
    /// 不参与抽奖完全合理。
    #[error("Creator cannot join own lottery {0}")]
    CreatorCannotJoin(u64),

    /// 官方模板池没有平台托管的 secret 承诺，禁止开奖。
    ///
    /// 模板池没有真实建池者，`commit_hash` 只能来自平台（`TEMPLATE_COMMIT`）。
    /// 没有承诺的模板池走 `BlockHashFallback` 会被出块者研磨 —— 官方奖池
    /// 金额大、可持续攻击，绝不允许这条路径存在。只能等管理员补承诺，
    /// 或走退款。
    #[error("Template pool {0} has no committed secret; reveal first or refund")]
    TemplateCommitRequired(u64),

    #[error("{0} is not a participant of lottery {1}")]
    NotParticipant(Addr, u64),

    #[error("Lottery {0} is not full yet: {1}/{2}")]
    LotteryNotFull(u64, u64, u64),

    #[error("Lottery {0} already drawn")]
    AlreadyDrawn(u64),

    #[error("Lottery {0} already refunded")]
    AlreadyRefunded(u64),

    #[error("Lottery {0} already has refunds: drawing is permanently disabled, participants should refund")]
    RefundStarted(u64),

    #[error("Lottery {0} has not expired yet")]
    NotExpired(u64),

    #[error("Lottery {0} already expired at {1}")]
    Expired(u64, u64),

    #[error("{0} is not a winner of lottery {1}")]
    NotWinner(Addr, u64),

    #[error("{0} already claimed lottery {1}")]
    AlreadyClaimed(Addr, u64),

    #[error("Nothing to claim")]
    NothingToClaim,

    #[error("Invalid params: {reason}")]
    InvalidParams { reason: String },

    /// simple 版：非法档位。合法值只有 0（5 人）/ 1（20 人）/ 2（50 人）。
    #[error("Invalid tier {0}: must be 0 (5 people) / 1 (20 people) / 2 (50 people)")]
    InvalidTier(u8),

    /// TKCC 探测失败：`assert_is_prc20` 无法确认目标合约是 PRC-20。
    /// decimals 一旦写入会影响所有 TKCC 费用换算，一次性配置宁可失败，
    /// 也不要把猜测值永久写入存储（旧实现静默回退 6，属"巧合正确"）。
    #[error("TKCC token probe failed: {0} is not a valid PRC-20 contract; set aborted (decimals NOT guessed)")]
    TkccProbeFailed(String),

    #[error("Invalid commit hash: expected 64 hex chars")]
    InvalidCommitHash,

    #[error("Secret does not match the committed hash")]
    CommitMismatch,

    #[error("Secret already revealed")]
    AlreadyRevealed,

    #[error("Not enough participants to draw: {0} < {1}")]
    NotEnoughParticipants(u64, u64),

    #[error("Lottery {0}: creator committed a secret but never revealed it before expiry; refund instead of drawing")]
    CommitNotRevealed(u64),

    // ---------------- 平台模板池（B2）----------------
    #[error("Pool template {0} not found")]
    TemplateNotFound(u64),

    #[error("Pool template {0} is inactive")]
    TemplateInactive(u64),

    #[error("Template {0} has an active pool, cannot modify params")]
    TemplateHasActivePool(u64),

    /// 会计恒等式被破坏：托管账户余额 < 记录中的奖池。
    /// 开奖前自检用，宁可拒绝开奖也不要按虚高的奖池分配（会透支托管）。
    #[error(
        "Lottery {id}: escrow holds less than the recorded pool \
         (paxi {escrow_paxi} < {pool_paxi}, tkcc {escrow_tkcc} < {pool_tkcc})"
    )]
    EscrowMismatch {
        id: u64,
        escrow_paxi: Uint128,
        pool_paxi: Uint128,
        escrow_tkcc: Uint128,
        pool_tkcc: Uint128,
    },
}

impl ContractError {
    pub fn tkcc_not_configured() -> Self {
        ContractError::Tkcc(TkccError::TkccNotConfigured)
    }
}

pub type ContractResult<T> = Result<T, ContractError>;
