//! paxi-lottery-contract-simple —— 状态定义

use cosmwasm_std::{Addr, Uint128};
use cosmwasm_schema::cw_serde;
use cw_storage_plus::{Item, Map};

pub const CONTRACT_NAME: &str = "crates.io:paxi-lottery-contract-simple";
pub const CONTRACT_VERSION: &str = env!("CARGO_PKG_VERSION");

/// 抽奖资金托管命名空间
pub const ESCROW_NS: &str = "lottery";

// ========================================================================
// 配置
// ========================================================================

#[cw_serde]
pub struct Config {
    pub treasury: Addr,
    pub paused: bool,
}

pub const CONFIG: Item<Config> = Item::new("config");

/// TKCC decimals（`SetTkccToken` 时探测，失败回落 6）
pub const TKCC_DECIMALS: Item<u8> = Item::new("tkcc_decimals");

/// 抽奖规则（**simple 版：奖池大小与费用全部由三档 tier 静态决定**）。
///
/// 对比完整版删除了 create_fee_* / min|max_join_* / min|max_people 八个自由参数：
/// 所有金额与人数都来自 `tier_spec()` 查表，管理员只保留开奖人数与时长两个旋钮。
#[cw_serde]
pub struct LotteryConfig {
    /// 一等奖人数。**必须恰好为 1**（`validate_lottery_config` 强制）：
    /// 一等奖不做均分，`compute_payout` 的 `first_*` 是整份奖金，
    /// 而 `execute_claim` 会把它发给 `winners.first` 里的**每一个**地址。
    /// 多于 1 人时第二位得主会因托管余额不足而永远领不到（资金锁死）。
    pub first_prize_count: u64,
    /// 二等奖人数。**必须 >= 1**（`validate_lottery_config` 强制）：
    /// 二等奖是"共 N%"按人数均分，`=0` 会导致 `second_each` 等于整份二等奖
    /// 但 `winners.second` 是空数组 → 该份额永远卡在托管。
    pub second_prize_count: u64,
    /// 抽奖持续时间（秒），超时未满员可退款
    pub duration_secs: u64,
    /// 超时退款时是否退还建池者的建池费
    pub refund_creator_on_timeout: bool,
}

impl Default for LotteryConfig {
    fn default() -> Self {
        Self {
            first_prize_count: 1,
            second_prize_count: 2,
            duration_secs: 24 * 3600,
            refund_creator_on_timeout: true,
        }
    }
}

pub const LOTTERY_CONFIG: Item<LotteryConfig> = Item::new("lottery_config");

// ========================================================================
// 五档 tier（v2：在原三档基础上新增两档，原三档一行不动）
// ========================================================================

pub const TIER_SMALL: u8 = 0; // 5 人档
pub const TIER_MEDIUM: u8 = 1; // 20 人档
pub const TIER_LARGE: u8 = 2; // 50 人档
pub const TIER_SMALL_HIGH: u8 = 3; // 5 人高档（v2 新增）
pub const TIER_LARGE_PAXI: u8 = 4; // 50 人纯 PAXI 档（v2 新增，无 TKCC）

/// 单档的静态规格。
///
/// * `join_tkcc` / `create_fee_tkcc` 单位是 **TKCC 个数**（不是 raw），
///   与完整版同口径；合约扣款前用 `to_tkcc_units()` 转 raw。
/// * `join_paxi` / `create_fee_paxi` 单位是 **upaxi**（raw）。
///
/// 分配比例（38/28/14/14/6 等）对所有档位完全一致，见下方 bps 常量。
#[cw_serde]
pub struct TierSpec {
    pub max_people: u64,
    /// 每人参与费：PAXI（upaxi）
    pub join_paxi: Uint128,
    /// 每人参与费：TKCC 个数
    pub join_tkcc: Uint128,
    /// 建池费：PAXI（upaxi）
    pub create_fee_paxi: Uint128,
    /// 建池费：TKCC 个数
    pub create_fee_tkcc: Uint128,
}

/// 档位查表。**这是本合约唯一的金额/人数来源**，合约不接受任何自由参数。
/// v2 新增档位只追加 match 分支，原三档（0/1/2）的数值保持不变——
/// 老实例 migrate 升级后，已存档位快照的进行中池子按原值结算不受影响。
pub fn tier_spec(id: u8) -> Option<TierSpec> {
    match id {
        TIER_SMALL => Some(TierSpec {
            max_people: 5,
            join_paxi: Uint128::new(1_000_000), // 1 PAXI
            join_tkcc: Uint128::new(10_000),    // 1 万 TKCC
            create_fee_paxi: Uint128::new(1_000_000),   // 1 PAXI
            create_fee_tkcc: Uint128::new(20_000),      // 2 万 TKCC
        }),
        TIER_MEDIUM => Some(TierSpec {
            max_people: 20,
            join_paxi: Uint128::new(2_000_000),     // 2 PAXI
            join_tkcc: Uint128::new(20_000),        // 2 万 TKCC
            create_fee_paxi: Uint128::new(15_000_000), // 15 PAXI
            create_fee_tkcc: Uint128::new(100_000),    // 10 万 TKCC
        }),
        TIER_LARGE => Some(TierSpec {
            max_people: 50,
            join_paxi: Uint128::new(10_000_000),    // 10 PAXI
            join_tkcc: Uint128::new(100_000),       // 10 万 TKCC
            create_fee_paxi: Uint128::new(60_000_000), // 60 PAXI
            create_fee_tkcc: Uint128::new(600_000),    // 60 万 TKCC
        }),
        // v2 新增：5 人高档 —— 参与费 10 倍，奖池更厚，成局速度与小池一致
        TIER_SMALL_HIGH => Some(TierSpec {
            max_people: 5,
            join_paxi: Uint128::new(10_000_000),    // 10 PAXI
            join_tkcc: Uint128::new(100_000),       // 10 万 TKCC
            create_fee_paxi: Uint128::new(15_000_000), // 15 PAXI
            create_fee_tkcc: Uint128::new(120_000),    // 12 万 TKCC
        }),
        // v2 新增：50 人纯 PAXI 档 —— 无 TKCC 参与，适合不持有 TKCC 的用户。
        // 0 值安全：join/create/burn/claim/refund 全链路均有 is_zero() 守卫（见 lottery.rs）。
        TIER_LARGE_PAXI => Some(TierSpec {
            max_people: 50,
            join_paxi: Uint128::new(10_000_000),    // 10 PAXI
            join_tkcc: Uint128::zero(),             // 0 TKCC
            create_fee_paxi: Uint128::new(75_000_000), // 75 PAXI
            create_fee_tkcc: Uint128::zero(),          // 0 TKCC
        }),
        _ => None,
    }
}

// ========================================================================
// 奖池分配（单位：万分之一 bps）
// ========================================================================

/// TKCC：一等奖 38%、二等奖两人共 28%、建池者 14%、运营 14%、销毁 6%
pub const TKCC_FIRST_BPS: u64 = 3800;
pub const TKCC_SECOND_TOTAL_BPS: u64 = 2800;
pub const TKCC_CREATOR_BPS: u64 = 1400;
pub const TKCC_OPS_BPS: u64 = 1400;
pub const TKCC_BURN_BPS: u64 = 600;

/// PAXI：一等奖 40%、二等奖两人共 30%、建池者 15%、运营 15%、不销毁
pub const PAXI_FIRST_BPS: u64 = 4000;
pub const PAXI_SECOND_TOTAL_BPS: u64 = 3000;
pub const PAXI_CREATOR_BPS: u64 = 1500;
pub const PAXI_OPS_BPS: u64 = 1500;

// ========================================================================
// 平台模板池（B2 模式）
// ========================================================================

/// 管理员预设的奖池模板（simple 版：模板只选档位）。
/// 用户调 `ActivateTemplate` 即可参与，无需自己建池、无需付建池费。
/// 参与费与人数从 `tier_spec(tier)` 查出。
#[cw_serde]
pub struct PoolTemplate {
    pub id: u64,
    pub name: String,
    /// 档位之一：TIER_SMALL / TIER_MEDIUM / TIER_LARGE / TIER_SMALL_HIGH / TIER_LARGE_PAXI
    pub tier: u8,
    pub active: bool,
    /// 是否提交过平台托管的 secret 承诺。
    /// 无承诺模板的池子**永远无法开奖**（只能退款）——前端据此置灰揭示按钮、
    /// 给用户准确的"等待退款"提示，而不是误导性的"等待揭示"。
    pub has_commit: bool,
}

pub const POOL_TEMPLATES: Map<u64, PoolTemplate> = Map::new("pool_templates");
pub const TEMPLATE_SEQ: Item<u64> = Item::new("template_seq");
/// 每个模板当前的活跃池 id
pub const ACTIVE_POOL_OF_TEMPLATE: Map<u64, u64> = Map::new("active_pool_of_template");

/// 模板池的**平台托管 secret 承诺**（sha256 的 64 位 hex）。
///
/// 模板池没有真实建池者（creator 是合约自身地址），没人能给 commit-reveal，
/// 随机源只能由平台提供：管理员创建模板时提交链头承诺，每次开奖前用
/// `RevealTemplateSecret` 揭示下一个 secret（哈希链轮换，见该消息注释）。
/// 没有承诺的模板，其池子**永远无法开奖**（只能退款）——
/// 宁可不开，也不开一个出块者可研磨的官方奖池。
///
/// 用独立 Map 而不是给 `PoolTemplate` / `Lottery` 加字段，不动已有序列化格式。
pub const TEMPLATE_COMMIT: Map<u64, String> = Map::new("template_commit");

/// 模板池最近一次揭示的 secret（明文）。仅供审计 / 前端展示。
/// 哈希链推进规则：**新承诺 = 本次揭示值本身**（下一个揭示值 s' 满足
/// sha256(s') == 本值），见 `TEMPLATE_COMMIT` 与 `template.rs` 的注释。
pub const TEMPLATE_REVEALED: Map<u64, String> = Map::new("template_revealed");

/// (template_id, 该池创建时的承诺) -> 对应的已揭示 secret。
///
/// 【开奖机制的关键】池子创建时把模板**当时的承诺**快照进 `Lottery.commit_hash`；
/// 之后管理员每次揭示只更新模板级 `TEMPLATE_COMMIT / TEMPLATE_REVEALED`，
/// **不会**回写已存在的池子。若开奖时直接读池子的 revealed 快照，
/// 满员之后揭示的值永远进不了池子 → 官方池永远无法开奖（死锁）。
/// 因此模板池开奖改为：拿池子的 commit 快照当 key，到这里查"那个承诺对应的 secret"。
/// 每个承诺只对应一次揭示，哈希链"每个 secret 只用一次"的语义不变。
pub const TEMPLATE_SECRETS: Map<(u64, &str), String> = Map::new("template_secrets");

/// 该奖池是否已经发生过退款。
///
/// simple 版下开奖**只看满员**（不存在"过期但达到 min_people 也能开"的分支），
/// 退款一次 participant_count 必然跌破 max_people，抢跑开奖在结构上已不可能。
/// 此标记保留为双保险：万一未来放开开奖条件，这道闸还在。
pub const REFUND_STARTED: Map<u64, bool> = Map::new("refund_started");

/// (lottery_id, participant) -> 加入时的区块时间（秒）。
///
/// C+：开奖种子加盐。
/// 出块者能控制开奖块的 `block.height` / `block.time`，但控制不了每个参与者在
/// **不同时间点**的加入时间。把 join_time 混入 seed 后，出块者要研磨结果必须
/// **同时**控制所有参与者的加入块，成本从 O(1) 提升到 O(N × 时长)。
///
/// 旧数据（本字段引入前已加入的池子）读不到时回落 0，只影响历史池，新池必有值。
pub const PARTICIPANT_JOIN_TIME: Map<(u64, &Addr), u64> = Map::new("participant_join_time");

// ========================================================================
// 抽奖
// ========================================================================

#[cw_serde]
pub enum LotteryStatus {
    /// 接受报名
    Open,
    /// 已满员，等待开奖
    Full,
    /// 已开奖
    Drawn,
    /// 超时退款结束
    Refunded,
    //
    // 【已删除】`Cancelled`（"已取消"）：合约中**没有任何路径**能进入该状态，
    // 前端 i18n / STATUS_RANK 里的"已取消"因此永远是死文案。留着只会误导
    // 读代码的人以为存在管理员风控取消入口。
    // 风控需求当前由 `SetPaused`（全局冻结）+ 到期 `Refund`（资金原路退回）覆盖；
    // 将来真要加"管理员取消单个池"，届时连同资金退回路径一起新增，不要只加枚举。
}

#[cw_serde]
pub struct Winners {
    /// 一等奖（1 人）
    pub first: Vec<Addr>,
    /// 二等奖（2 人）
    pub second: Vec<Addr>,
}

#[cw_serde]
pub struct Payout {
    /// 一等奖可得（TKCC）
    pub first_tkcc: Uint128,
    /// 二等奖每人可得（TKCC）
    pub second_each_tkcc: Uint128,
    /// 一等奖可得（PAXI）
    pub first_paxi: Uint128,
    /// 二等奖每人可得（PAXI）
    pub second_each_paxi: Uint128,
    /// 建池者分成
    pub creator_tkcc: Uint128,
    pub creator_paxi: Uint128,
    /// 运营分成
    pub ops_tkcc: Uint128,
    pub ops_paxi: Uint128,
    /// 销毁（仅 TKCC）
    pub burn_tkcc: Uint128,
}

#[cw_serde]
pub struct Lottery {
    pub id: u64,
    pub creator: Addr,
    /// 创建时选择的档位（0/1/2，审计与前端展示用；金额以本结构快照为准）
    pub tier: u8,
    /// 参与费（PAXI，upaxi）——从 tier_spec 快照
    pub join_paxi: Uint128,
    /// 参与费（TKCC 最小单位）——从 tier_spec 快照（个数 × 10^decimals）
    pub join_tkcc: Uint128,
    pub max_people: u64,
    /// 奖池累计
    pub pool_paxi: Uint128,
    pub pool_tkcc: Uint128,
    /// 开奖时的奖池快照（开奖后 `pool_*` 归零，这里保留历史值供前端/审计展示）
    #[serde(default)]
    pub settled_pool_paxi: Uint128,
    #[serde(default)]
    pub settled_pool_tkcc: Uint128,
    pub participant_count: u64,
    /// 建池者的建池费是否已退还（防重复退款，也防"抢跑"）
    #[serde(default)]
    pub creator_refunded: bool,
    /// 建池时**实付**的建池费（PAXI，upaxi）。
    ///
    /// 退款一律用这个快照值，而**不是**当前的全局 `cfg.create_fee_*`：
    /// 否则管理员调整费率后，建池者可以按新费率退走包含参与者参与费的托管余额。
    /// 模板池无建池费，恒为 0。
    #[serde(default)]
    pub create_fee_paid_paxi: Uint128,
    /// 建池时**实付**的建池费（TKCC 最小单位）
    #[serde(default)]
    pub create_fee_paid_tkcc: Uint128,
    pub status: LotteryStatus,
    pub created_at: u64,
    pub expires_at: u64,
    /// A 模式：创建者提交的 `sha256(secret)` hex。
    /// 模板池：**创建池子那一刻**模板的当前承诺（作为 `TEMPLATE_SECRETS` 的查询 key）。
    #[serde(default)]
    pub commit_hash: Option<String>,
    /// A 模式：揭示的 secret（开奖前由创建者提交）。
    /// 模板池恒为 None（揭示值通过 `TEMPLATE_SECRETS` 查表，不回写池子）。
    #[serde(default)]
    pub revealed: Option<String>,
    pub winners: Option<Winners>,
    pub payout: Option<Payout>,
    /// 开奖时的随机种子（hex），供链下复算审计
    pub seed: Option<String>,
    /// 使用的随机源
    pub random_source: Option<paxi_common::RandomSource>,
    /// 平台模板池：`Some(template_id)`；玩家自建池：`None`
    #[serde(default)]
    pub template_id: Option<u64>,
    /// 是否平台模板池。`true` 时建池者分成归运营（28% / 30%），且无建池费。
    #[serde(default)]
    pub is_template_pool: bool,
}

pub const LOTTERIES: Map<u64, Lottery> = Map::new("lotteries");
pub const LOTTERY_SEQ: Item<u64> = Item::new("lottery_seq");
/// (lottery_id, participant) -> 是否已参与（去重）
pub const PARTICIPANTS: Map<(u64, &Addr), bool> = Map::new("participants");
/// (lottery_id, winner) -> 是否已领取
pub const CLAIMED: Map<(u64, &Addr), bool> = Map::new("claimed");

// ========================================================================
// 统计
// ========================================================================

pub const TOTAL_BURNED: Item<Uint128> = Item::new("total_burned");
pub const TOTAL_OPS: Map<&Addr, Uint128> = Map::new("total_ops");

// ========================================================================
// 抽奖状态二级索引（P2-6）
// ========================================================================

/// key = (status_str, id)。`Lotteries { status }` 查询用它把
/// O(全表扫描) 降到 O(该状态条目数)。
///
/// 写入点（与状态迁移严格同步）：
/// * 创建 / 模板开新池        → ("open", id)
/// * join 满员                → ("full", id)（同时移除 open）
/// * 开奖                     → ("drawn", id)（同时移除旧状态）
/// * 托管清空标记 Refunded    → ("refunded", id)（同时移除旧状态）
///
/// 查询端对每条命中再做一次 `status_str` 复核：即使索引意外漂移，
/// 也只是跳过该条（自愈），绝不会返回状态不符的数据。
pub const LOTTERY_STATUS_INDEX: Map<(&str, u64), ()> = Map::new("lottery_status_idx");
