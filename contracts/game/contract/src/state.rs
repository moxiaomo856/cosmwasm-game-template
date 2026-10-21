use cosmwasm_std::{Addr, Storage, Uint128};
use cw_storage_plus::{Item, Map};
use cosmwasm_schema::cw_serde;

// ===== 🟢 游戏引擎枚举 =====
#[cw_serde]
pub enum GameEngine {
    /// 猜数字类：猜大小、猜数字、猜奇偶
    Guess,
    /// 轮盘类：俄罗斯轮盘、幸运大转盘
    Roulette,
    /// 骰子类：大小单双、骰宝
    Dice,
    /// 卡牌类：21点、比大小、炸金花
    Card,
    /// 休闲计分类：贪吃蛇、跑酷、打砖块（前端算分，合约按分数结算）
    Score,
}

// ===== cw2 常量 =====
pub const CONTRACT_NAME: &str = "crates.io:paxi-game-contract";
pub const CONTRACT_VERSION: &str = env!("CARGO_PKG_VERSION");

// ===== 管理员 =====
pub const ADMINS: Map<&Addr, bool> = Map::new("admins");

// ===== 暂停 =====
pub const PAUSED: Item<bool> = Item::new("paused");

// ===== PRC-20 白名单 =====
pub const PRC20_WHITELIST: Map<&Addr, bool> = Map::new("prc20_whitelist");

// ===== 燃烧 =====
pub const BURN_RATE: Item<u64> = Item::new("burn_rate");
pub const GAME_CONFIGS: Map<&str, GameConfig> = Map::new("game_configs");
pub const GAME_PARAMS: Item<GameParams> = Item::new("game_params");

// ===== Swap 模块 =====
pub const SWAP_MODULE: Item<String> = Item::new("swap_module");

// ===== 用户余额（所有代币类型） =====
//   (&player, &EMPTY_ADDR)  → 原生 PAXI
//   (&player, &token_addr)  → PRC-20 代币
pub const BALANCES: Map<(&Addr, &Addr), Uint128> = Map::new("balances");

// ===== H1: 资金池会计（多币种） =====
pub const HOUSE_BANKROLL: Map<&Addr, Uint128> = Map::new("house_bankroll");
pub const USER_TOTALS: Map<&Addr, Uint128> = Map::new("user_totals");
pub const TOTAL_BURNED: Map<&Addr, Uint128> = Map::new("total_burned");
pub const TOTAL_TREASURY: Map<&Addr, Uint128> = Map::new("total_treasury");

// 🟢 M5 审计修复（2026-10-10）：assert_solvency 在 PRC-20 balance 查询失败时
//    会放行（不拦提现，避免查询故障卡死用户）。该「跳过」分支此前完全不可观测 ——
//    若白名单被污染进一个 balance 查询必然失败的代币，偿付能力检查就被整体绕过。
//    这里累计跳过次数，供链下监控/治理排查（异常升高 = 白名单可能有问题）。
pub const SOLVENCY_SKIP_COUNT: Item<u64> = Item::new("solvency_skip_count");

// ===== 运营配置 =====
// 多个抽水地址，按区块高度+时间确定性轮转选择（防止单点被封 + 分散资金；非随机）
pub const TREASURY_LIST: Item<Vec<String>> = Item::new("treasury_list");
pub const TREASURY_SHARE: Item<u64> = Item::new("treasury_share");

// ============================================================================
// 🆕 付费系统：买卡 + 买碎片（明码标价、确定性获得，非博弈）
//   - 卡牌价格表：卡牌名 → PAXI 价格（upaxi，1 PAXI = 1_000_000 upaxi）
//   - 碎片价格表：稀有度 → (每包数量, 每包价格 upaxi)
//   - PAXI 收入轮询转入项目方钱包，不在合约停留
// ============================================================================
pub const PROJECT_WALLETS: Item<Vec<Addr>> = Item::new("project_wallets");
pub const WALLET_INDEX: Item<u64> = Item::new("wallet_index");
pub const CARD_PRICES: Map<&str, Uint128> = Map::new("card_prices");

// 🟢 2026-10-21 公开模板化：链级 Swap 模块地址。
//   这是**目标链的基础设施地址**（与 staking / distribution 模块同级），
//   不是部署者的资产地址，因此作为默认值保留在模板里是合理的。
//   部署到其它链时，用 `InstantiateMsg.swap_module` 覆盖即可。
pub const DEFAULT_SWAP_MODULE: &str = "paxi1mfru9azs5nua2wxcd4sq64g5nt7nn4n80r745t";
// 🔴 FRAGMENT_PRICES 已删除（T7/裁定 5.3）：四桶碎片购买废弃，
//    统一碎片按 params.card_frag_price[4] 计价；链上旧键 fragment_prices 成孤儿不再读取。

// ===== 🟢 新增：游戏限制配置（每个 game_id 独立配置） =====
// 管理员可以随时增删改，加游戏只改配置，不重部署合约
pub const GAME_LIMITS: Map<&str, GameLimit> = Map::new("game_limits");

// ===== 🟢 新增：游戏引擎配置（每个 game_id 绑定一个引擎 + 引擎参数） =====
pub const GAME_ENGINE_CONFIGS: Map<&str, GameEngineConfig> = Map::new("game_engine_configs");

// ===== 🟢 新增：已使用的 round_id（防重放） =====
// key = "{player_addr}:{game_id}:{round_id}" → value = RoundStatus
pub const USED_ROUNDS: Map<&str, RoundStatus> = Map::new("used_rounds");

// ===== 🟢 审计新增：Deduct 超时保护 =====
// 问题：Deduct 已经把玩家余额扣掉了，但如果玩家（或前端）一直不调 Settle，
// 这笔钱既不在任何人余额里，也没进庄家准备金，会永久悬空在合约内。
// 现在 Deduct 时记一个 deadline，超时后管理员可以强制按「输局」结算入账。
//
// 🟢 第二轮改进：超时时长原本是写死的 `const ROUND_TIMEOUT_SECS = 3600`，
// 不同游戏的单局时长差别很大（骰子 3 秒、21 点可能几分钟、两段式 Deduct/Settle
// 甚至可能跨天），硬编码 1 小时要么太短误伤正常局，要么太长资金悬空太久。
// 改为可配置的 Item，管理员用 SetRoundTimeout 调整；读取时统一走
// `round_timeout_secs()`，老合约（migrate 上来没写过这个 Item）自动回落到默认值。
pub const ROUND_TIMEOUT: Item<u64> = Item::new("round_timeout");
pub const DEFAULT_ROUND_TIMEOUT_SECS: u64 = 3600; // 默认 1 小时
pub const MIN_ROUND_TIMEOUT_SECS: u64 = 60;       // 最短 1 分钟（防止管理员设 0 秒抢钱）
pub const MAX_ROUND_TIMEOUT_SECS: u64 = 7 * 24 * 3600; // 最长 7 天

// key 与 USED_ROUNDS 相同："{player_addr}:{game_id}:{round_id}" → 过期时间戳（秒）
pub const ROUND_DEADLINE: Map<&str, u64> = Map::new("round_deadline");

// ===== 🟢 新增：每日统计（按玩家 + 日） =====
// key = "{player_addr}:{date_timestamp}" → DailyCounter
pub const DAILY_TRACKING: Map<&str, DailyCounter> = Map::new("daily_tracking");

// ===== 常量 =====
// （已删除 MAX_PAYOUT_MULTIPLIER：全局派彩上限从未被引用，
//   实际限额走每个游戏独立的 GameLimit::max_payout_multiplier）
pub const DEFAULT_MAX_DAILY_BET: u128 = 100_000_000;  // 默认每日总下注上限（单位：upaxi = 1 PAXI 的 10^-6）
pub const DEFAULT_MAX_DAILY_ROUNDS: u64 = 50;         // 默认每日最大局数

// ===== 结构体 =====

#[cw_serde]
pub struct GameConfig {
    pub burn_enabled: bool,
    pub burn_rate_override: Option<u64>,
    pub treasury_share_override: Option<u64>,
}

impl Default for GameConfig {
    fn default() -> Self {
        Self {
            burn_enabled: true,
            burn_rate_override: None,
            treasury_share_override: None,
        }
    }
}

#[cw_serde]
pub struct GameParams {
    pub min_bet: Uint128,
    pub max_bet: Uint128,
    pub house_edge: u64,
}

impl Default for GameParams {
    fn default() -> Self {
        Self {
            min_bet: Uint128::from(1u128),
            max_bet: Uint128::from(1_000_000u128),
            house_edge: 500,
        }
    }
}

/// 🟢 新增：每个游戏的限额配置
/// 管理员通过 SetGameLimit 设置，加新游戏只需加一条配置
#[cw_serde]
pub struct GameLimit {
    /// 是否启用这个游戏（false 则 Deduct/Settle 拒绝）
    pub enabled: bool,
    /// 单局最小下注
    pub min_bet: Uint128,
    /// 单局最大下注
    pub max_bet: Uint128,
    /// 单局最大派彩倍数（相对 bet），例如 300 = 最多赔 3 倍
    pub max_payout_multiplier: u64,
    /// 每日最大下注额（所有玩家合计？不，按玩家）
    pub max_daily_bet: Uint128,
    /// 每日最大结算次数（单玩家）
    pub max_daily_rounds: u64,
}

impl Default for GameLimit {
    fn default() -> Self {
        Self {
            enabled: true,
            min_bet: Uint128::from(1u128),
            max_bet: Uint128::from(10_000_000u128), // 10 PAXI
            max_payout_multiplier: 300,            // 最多赔 3 倍
            max_daily_bet: Uint128::from(DEFAULT_MAX_DAILY_BET), // 100 PAXI
            max_daily_rounds: DEFAULT_MAX_DAILY_ROUNDS,
        }
    }
}

/// round_id 状态
#[cw_serde]
pub enum RoundStatus {
    /// 已扣款，等待结算
    Deducted { bet: Uint128, token_addr: String },
    /// 已结算
    Settled { bet: Uint128, payout: Uint128, token_addr: String },
}

/// 每玩家每日统计
#[cw_serde]
pub struct DailyCounter {
    pub total_bet: Uint128,
    pub rounds_played: u64,
}

impl Default for DailyCounter {
    fn default() -> Self {
        Self {
            total_bet: Uint128::zero(),
            rounds_played: 0,
        }
    }
}

/// 返回一个空的 Addr 占位符，用于表示原生 PAXI（非 PRC-20）
pub fn empty_addr() -> Addr {
    Addr::unchecked("")
}

// ============================================================================
// 🟢 7A：支付代币配置（管理员可参数化切换）
//   - empty_addr()  → 原生 PAXI（默认）
//   - 其他 Addr     → PRC-20 合约地址（从合约内部余额扣，保持无感）
// 切换方式：SanguoSetPaymentToken { token }
// ============================================================================
pub const PAYMENT_TOKEN: Item<Addr> = Item::new("sg_payment_token");

/// 根据时间戳获取当日 0 点 UTC 秒数（用于 DAILY_TRACKING 的 key）
pub fn day_key(timestamp: u64) -> u64 {
    // 简单取整：86400 秒一天
    (timestamp / 86400) * 86400
}

/// 读取当前的 round 超时时长（秒）
///
/// 老合约 migrate 上来时 ROUND_TIMEOUT 还没写过，这里回落到 DEFAULT_ROUND_TIMEOUT_SECS，
/// 保证升级后行为不突变。
pub fn round_timeout_secs(storage: &dyn Storage) -> u64 {
    ROUND_TIMEOUT
        .may_load(storage)
        .ok()
        .flatten()
        .unwrap_or(DEFAULT_ROUND_TIMEOUT_SECS)
}

/// 🟢 游戏引擎配置 —— 每个 game_id 绑定一个引擎 + 参数
/// 管理员通过 SetGameEngineConfig 设置，加新游戏只需加一条配置
#[cw_serde]
pub struct GameEngineConfig {
    /// 绑定哪个引擎
    pub engine: GameEngine,
    /// 引擎特定参数（JSON 格式，不同引擎不同字段）
    pub params: GameEngineParams,
}

#[cw_serde]
pub enum GameEngineParams {
    /// 猜数字：{ min, max, multiplier }
    Guess { min: u64, max: u64, multiplier: u64 },
    /// 轮盘：{ slots, payouts } payouts[i] = slot i 的派彩倍数（如 [1,2,0,5,0,3,0,10]）
    Roulette { slots: u64, payouts: Vec<u64> },
    /// 骰子：{ dice_count, sides, bet_types: ["big","small","odd","even","specific"] }
    Dice {
        dice_count: u64,
        sides: u64,
        bet_types: Vec<String>,
    },
    /// 卡牌：{ deck_count, min_cards, max_cards, multiplier_by_hand }
    /// multiplier_by_hand 是手牌等级对应的派彩倍数
    Card {
        deck_count: u64,
        min_cards: u64,
        max_cards: u64,
        /// 按手牌类型索引，multiplier_by_hand[hand_rank] = 倍数
        multiplier_by_hand: Vec<u64>,
    },
    /// 计分：{ max_score, multiplier_per_score, max_payout }
    Score {
        max_score: u64,
        /// 每得 1 分的派彩倍数（相对于 bet）
        multiplier_per_score: u64,
        /// 单局最大派彩（绝对值，防止刷分）
        max_payout: Uint128,
    },
}

// ===== 引擎 Play 接口参数 =====

/// 猜数字引擎输入
#[cw_serde]
pub struct GuessInput {
    /// 玩家猜的数字（必须在 [min, max] 范围内）
    pub guess: u64,
}

/// 轮盘引擎输入
#[cw_serde]
pub struct RouletteInput {
    /// 玩家下注的 slot 编号（0 到 slots-1）
    pub bet_slot: u64,
}

/// 骰子引擎输入
#[cw_serde]
pub struct DiceInput {
    /// 下注类型："big" / "small" / "odd" / "even" / "specific"
    pub bet_type: String,
    /// 当 bet_type = "specific" 时，指定具体点数
    pub specific_value: Option<u64>,
}

/// 卡牌引擎输入
#[cw_serde]
pub struct CardInput {
    /// 玩家要多少张牌（在 [min_cards, max_cards] 范围内）
    pub card_count: u64,
    /// 比较模式："vs_dealer"（跟庄家比）或 "rank_only"（只按手牌等级派彩）
    pub mode: String,
}

/// 计分类引擎输入
#[cw_serde]
pub struct ScoreInput {
    /// 前端算好的最终分数（合约只做范围检查）
    pub final_score: u64,
}
