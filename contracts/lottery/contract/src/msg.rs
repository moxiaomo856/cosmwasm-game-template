//! paxi-lottery-contract-simple —— 消息定义

use cosmwasm_std::{Addr, Binary, Uint128};
use cosmwasm_schema::{cw_serde, QueryResponses};

use paxi_common::{AdminAction, BurnMode};

use crate::state::{Lottery, LotteryConfig, Payout, PoolTemplate, Winners};

#[cw_serde]
pub struct InstantiateMsg {
    pub admins: Vec<String>,
    pub treasury: String,
    /// TKCC（外部 PRC-20）合约地址。**现阶段传 `null`**
    pub tkcc_token: Option<String>,
    pub tkcc_burn_address: Option<String>,
    pub lottery_config: Option<LotteryConfig>,
    pub multisig_threshold: Option<u32>,
}

#[cw_serde]
pub struct ReceiveMsg {
    pub sender: String,
    pub amount: Uint128,
    pub msg: Binary,
}

#[cw_serde]
pub enum ReceiveHookMsg {
    Deposit {},
}

#[cw_serde]
pub enum ExecuteMsg {
    // ---------------- 资金 ----------------
    Deposit {},
    Receive(ReceiveMsg),
    Withdraw { token: Option<String>, amount: Uint128 },

    // ---------------- 抽奖 ----------------
    /// 创建抽奖（simple 版）：建池者按**所选档位**付建池费，参与费与人数同档锁定。
    ///
    /// * `tier`：0=5 人档 / 1=20 人档 / 2=50 人档，费用见 `tier_spec()`
    /// * `commit_hash`：建池者的 `sha256(secret)` hex（可选，commit-reveal）
    ///
    /// 建池者即交易发送方（`info.sender`）。
    CreateLottery {
        tier: u8,
        /// commit-reveal 承诺：`sha256(secret)` hex（可选）
        commit_hash: Option<String>,
    },
    JoinLottery { id: u64 },
    /// **B2 主入口**：激活模板并参与。
    ///
    /// * 模板无活跃池 → 用模板参数开新池，调用者作为**普通参与者**加入
    /// * 模板有未满活跃池 → 直接加入
    /// * 不传任何金额参数，参与费一律从链上 `Lottery` 对象读取
    ///
    /// 【重要】第一个触发者与后续参与者**完全一致**：扣款相同、分成相同、
    /// 没有任何额外奖励或特权。他只是在正确的时间点了一次"参与"。
    ActivateTemplate { template_id: u64 },
    /// 创建者揭示 commit-reveal 的 secret
    RevealSecret { id: u64, secret: String },
    /// 满员后任意人 / keeper 可触发开奖
    DrawLottery { id: u64 },
    /// 中奖者领取（进入内部余额）
    Claim { id: u64 },
    /// 超时未满员，参与者退款
    Refund { id: u64 },

    // ---------------- 管理员：模板管理（B2）----------------
    /// 创建奖池模板（simple 版：只选档位，费用与人数由 tier 决定）
    ///
    /// `commit_hash`：平台托管 secret 的**链头承诺**（sha256 的 64 位 hex，可选）。
    /// 不提交的模板，其池子无法开奖（只能退款）。
    /// 哈希链轮换见 `LotteryAdminAction::RevealTemplateSecret`。
    CreatePoolTemplate {
        name: String,
        tier: u8,
        commit_hash: Option<String>,
    },
    /// 启停模板（停用不影响已有活跃池，让当前池跑完）
    UpdatePoolTemplate { id: u64, active: bool },

    // ---------------- 管理员 ----------------
    Admin(AdminAction),
    AdminCustom(LotteryAdminAction),
    ProposeAdminAction { action: AdminAction },
    ConfirmAdminAction { id: u64 },
}

#[cw_serde]
pub enum LotteryAdminAction {
    SetLotteryConfig { config: LotteryConfig },
    SetBurnMode { mode: BurnMode },
    /// 揭示模板池的平台托管 secret（开奖前必须调用）。
    ///
    /// **哈希链轮换**：管理员创建模板时提交链头承诺 c₀ = hex(sha256(s₁))；
    /// 之后每次揭示下一个 secret sᵢ₊₁，合约验证 sha256(sᵢ₊₁) == 当前承诺后，
    /// 把承诺更新为**本次的哈希值**（即已公开的 sᵢ 的 hex）。
    /// 管理员只需在建模板前一次性离线生成整条链（s₁ = sha256(s₂) = …），
    /// 每次开奖前揭示一个，无需反复提交承诺交易。
    ///
    /// secret 一旦揭示即公开，绝不能复用 —— 哈希链保证每个值只用一次。
    RevealTemplateSecret { template_id: u64, secret: String },
}

#[cw_serde]
#[derive(QueryResponses)]
pub enum QueryMsg {
    #[returns(ConfigResponse)]
    Config {},

    #[returns(TkccResponse)]
    Tkcc {},

    #[returns(LotteryConfig)]
    LotteryConfig {},

    #[returns(Option<Lottery>)]
    Lottery { id: u64 },

    /// 分页列表。
    ///
    /// * `start_after`：从 **id 严格大于** 该值的记录开始（不是"精确命中"），
    ///   `None` 表示从头开始。
    /// * `status`：可选的 status 过滤，**在分页之后应用**；因此返回条数
    ///   可能小于 `limit`（被过滤掉的部分不补），调用方需用"返回条数 < limit"
    ///   或"最后一条的 id"判断是否还有下一页，**不能**用"返回条数 == limit"。
    #[returns(LotteriesResponse)]
    Lotteries { status: Option<String>, start_after: Option<u64>, limit: Option<u32> },

    #[returns(ParticipantsResponse)]
    Participants { id: u64 },

    #[returns(Option<Winners>)]
    Winners { id: u64 },

    #[returns(Option<Payout>)]
    Payout { id: u64 },

    /// 已开奖但尚未被领取的中奖金额（治理 / 审计视角，只读）
    #[returns(UnclaimedResponse)]
    Unclaimed { id: u64 },

    /// 全部模板
    #[returns(PoolTemplatesResponse)]
    PoolTemplates {},

    /// 单个模板
    #[returns(Option<PoolTemplate>)]
    PoolTemplate { id: u64 },

    /// 每个活跃模板的当前池状态（含人数进度）
    #[returns(ActivePoolOfTemplatesResponse)]
    ActivePoolOfTemplates {},

    #[returns(BalanceResponse)]
    Balance { address: String, token: Option<String> },

    #[returns(BalancesResponse)]
    Balances { address: String },

    #[returns(AdminsResponse)]
    Admins {},

    #[returns(Option<paxi_common::AdminProposal>)]
    Proposal { id: u64 },
}

/// 未领取中奖金额视图（只读，供治理 / 审计判断是否有"中奖者长期不来领"的资金）
#[cw_serde]
pub struct UnclaimedResponse {
    /// 是否已开奖（未开奖时下面字段恒为 0）
    pub drawn: bool,
    pub unclaimed_paxi: Uint128,
    pub unclaimed_tkcc: Uint128,
    /// 尚未领取的中奖者地址列表
    pub pending: Vec<String>,
}

#[cw_serde]
pub struct ConfigResponse {
    pub treasury: Addr,
    pub paused: bool,
    pub contract_version: String,
}

#[cw_serde]
pub struct TkccResponse {
    pub token: Option<Addr>,
    pub configured: bool,
    pub decimals: Option<u8>,
    pub burn_address: Option<Addr>,
    pub burn_mode: Option<BurnMode>,
}

#[cw_serde]
pub struct LotteriesResponse {
    pub lotteries: Vec<Lottery>,
}

#[cw_serde]
pub struct ParticipantsResponse {
    pub participants: Vec<Addr>,
}

#[cw_serde]
pub struct PoolTemplatesResponse {
    pub templates: Vec<PoolTemplate>,
}

#[cw_serde]
pub struct ActivePoolEntry {
    pub template_id: u64,
    pub template_name: String,
    pub tier: u8,
    pub active_pool_id: Option<u64>,
    pub participant_count: u64,
    pub max_people: u64,
    pub join_paxi: Uint128,
    /// TKCC 个数（前端 ×10^decimals 展示）
    pub join_tkcc: Uint128,
}

#[cw_serde]
pub struct ActivePoolOfTemplatesResponse {
    pub entries: Vec<ActivePoolEntry>,
}

#[cw_serde]
pub struct BalanceResponse {
    pub address: Addr,
    pub token: Option<String>,
    pub balance: Uint128,
}

#[cw_serde]
pub struct BalancesResponse {
    pub balances: Vec<(String, Uint128)>,
}

#[cw_serde]
pub struct AdminsResponse {
    pub admins: Vec<Addr>,
    pub threshold: u32,
}

#[cw_serde]
pub struct MigrateMsg {
    pub tkcc_token: Option<String>,
}
