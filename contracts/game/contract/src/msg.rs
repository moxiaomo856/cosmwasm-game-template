// ============================================================================
// 三国卡牌 V2 消息层（裁定书第 4 章入口清单，2026-10-13）
//
// Execute 对账（仅 Sanguo 域，通用域不动）：
//   现役 48
//   − 删 7：SanguoProposeCard / SanguoVoteCard / SanguoExecuteProposal /
//           SanguoCancelProposal（投票全禁 T24）、SanguoBuyFragments /
//           SanguoSetFragmentPrice（四桶碎片废除）、SanguoSetGiftBan
//           （泛化为 SanguoBanPlayer，删+增各 1）
//   − 合并 10：PVP 4 + 混战 4 + SanguoAiBattle + SanguoRankedBattle
//              （裁定 4.3 表实际列 10 个；4.1 公式写"11"系笔误，已对账）
//   + 合并产出 2：SanguoBattlePvp / SanguoBattleAi
//   + 新增 12（4.4 表列 A2/B6/C4）
//   + 新增 1：SanguoMigrateFragments（裁定 8.2 M6 分批迁移入口，
//             4.4 表漏列，附录证据索引已承认此缺口）
//   + 保留入口 1：SanguoSetProjectWallets（C5 废弃清值但入口保留，
//                4.5 保留清单未列出，按 4.2 注"不进删 7 账"计入）
//   = 46 ✅
//
// Query 对账（4.6）：删 3（Proposals/Proposal/ProposalVotes）
//   + 增 3（SanguoShopItems / SanguoEquippedCosmetics / SanguoCollection）= 33 持平。
//   两处有意偏差（留专家复核）：
//   ① SanguoFragmentPrices 保留入口但语义改指指定卡碎片价 card_frag_price[4]
//     （原四桶碎片价表随 SanguoSetFragmentPrice 删除，T15）；
//   ② SanguoCompliance 移除 compliance_mode 布尔（C1 总闸已删，恒真无意义），
//     改返回 params 访问器解析后的 operators + 两项日限。
//
// ⚠️ 4.3 的 PvpAction 枚举原文漏列 Join（Royale 入场），本文件补入——
//    专家审计时请复核该补充是否符合裁定意图。
// ============================================================================

use cosmwasm_schema::schemars::JsonSchema;
use cosmwasm_schema::{cw_serde, QueryResponses};
use serde::{Deserialize, Serialize};
use cosmwasm_std::Uint128;
use crate::state::{
    GameConfig, GameEngine, GameLimit, GameParams, GameEngineConfig,
    GuessInput, RouletteInput, DiceInput, CardInput, ScoreInput,
};
use crate::sanguo::state::{
    CardTemplate, SanguoParams, CardInfo, Fragments, StageConfig, SkinInfo, SeasonConfig, ShopItem,
};

// ============================================================================
// Migrate 入参（方案 Z，裁定 8.1~8.2）
//
// migrate 本体只做原子配置级动作：
//   M1 版本校验放宽（0.1.1 → 0.2.0）；
//   M2 合规总闸硬化（删 compliance_mode 键、SanguoConfig 三字段清空）；
//   M3 PROPOSAL_DEPOSITS → PENDING_REFUNDS（PAXI 键记账转帐）；
//   M4 清空 PLAYER_BATTLE_ORDER（旧编队 cost 校验失效）；
//   CardTemplate 60 张 × 11 字段覆盖写（serde(default) 直读旧 JSON）+ 断言 7 张修正值；
//   写 MIGRATION_V2_DONE = true + 版本号 0.2.0（防重入）。
//
// 玩家侧数据（M5 超限编队补偿 / M6 四桶碎片 → 统一碎片）由管理员反复调
// ExecuteMsg::SanguoMigrateFragments { start_after, limit≤50 } 分批完成（M7 防 OOG）。
// ============================================================================
#[cw_serde]
pub struct MigrateMsg {
    /// 运营白名单（GrantFragmentsBatch 允许的地址），None = 不修改
    pub operators: Option<Vec<String>>,
    /// 遗留 TKCC 对局未结清时默认拒绝 migrate；传 true 强制跳过守卫
    pub ignore_legacy_battles: Option<bool>,
}

/// 运营批量发碎片的单条授权
#[cw_serde]
pub struct FragmentGrant {
    pub player: String,
    pub fragments: Fragments,
}

#[cw_serde]
pub struct InstantiateMsg {
    pub admins: Vec<String>,
    pub prc20_whitelist: Vec<String>,
    pub initial_burn_rate: Option<u64>,
    pub swap_module: Option<String>,
    /// 多个抽水地址（按区块高度+时间确定性轮转选择，非随机）
    pub treasury_list: Option<Vec<String>>,
    pub treasury_share: Option<u64>,
}

// ============================================================================
// V2 战斗入口的嵌套 enum（裁定 4.3）
// ============================================================================

/// PVP 战场类型：单挑（原 CreatePvp/AcceptPvp/CancelPvp/ClaimPvpReward）/ 混战（原 Royale 4 入口）
#[cw_serde]
pub enum PvpMode {
    Single,
    Royale,
}

/// PVP 战场动作。mode×action 非法组合（如 Royale+Accept）在 exec 分发处
/// 编译期穷尽匹配直接报错（裁定 4.3 分发约束）。
/// ⚠️ Join 为本文件补入（4.3 枚举原文漏列 Royale 入场动作）。
#[cw_serde]
pub enum PvpAction {
    Create,
    Accept,
    Join,
    Cancel,
    Claim,
    Settle,
}

/// AI 战场类型：休闲（原 SanguoAiBattle，level=难度 1..5）/ 天梯（原 SanguoRankedBattle，level=段位 0..4）
#[cw_serde]
pub enum AiMode {
    Casual,
    Ranked,
}

#[cw_serde]
pub enum ExecuteMsg {
    // ========== 管理员（通用域，原样保留） ==========
    AddAdmin { admin: String },
    RemoveAdmin { admin: String },
    AddPrc20 { contract: String },
    RemovePrc20 { contract: String },
    SetPaused { paused: bool },
    UpdateGameParams { params: GameParams },
    SetBurnRate { rate: u64 },
    SetGameConfig { game_id: String, config: crate::state::GameConfig },
    BurnPaxi { amount: Uint128 },
    SetSwapModule { contract: String },
    SetTreasuryList { treasuries: Vec<String> },
    SetTreasuryShare { share: u64 },
    SetGameLimit { game_id: String, limit: GameLimit },
    RemoveGameLimit { game_id: String },
    SetRoundTimeout { secs: u64 },
    SetGameEngineConfig { game_id: String, config: GameEngineConfig },
    RemoveGameEngineConfig { game_id: String },
    ForceExpireRound { player: String, game_id: String, round_id: String },
    /// token: None = 原生 PAXI（info.funds），Some(addr) = PRC-20（transfer_from）
    FundBankroll { token: Option<String>, amount: Option<Uint128> },
    /// 升级前清扫：amount ≤ 合约余额 − USER_TOTALS 硬护栏，玩家资产不可能被扫走
    AdminSweep { token: Option<String>, recipient: String, amount: Option<Uint128> },
    AddGame { game_id: String, limit: GameLimit, engine_config: GameEngineConfig },

    // ========== 资产 ==========
    Deposit { token: Option<String>, amount: Option<Uint128> },
    Withdraw { token: Option<String>, amount: Uint128 },

    // ========== 通用游戏一步式 Play（防作弊：随机数合约内生成） ==========
    Play {
        game_id: String,
        round_id: String,
        token: Option<String>,
        bet: Uint128,
        guess_input: Option<GuessInput>,
        roulette_input: Option<RouletteInput>,
        dice_input: Option<DiceInput>,
        card_input: Option<CardInput>,
        score_input: Option<ScoreInput>,
    },

    // ========== Swap（T23：通用资产层保留，零 TKCC 红线不涉及；PAXI↔PRC-20 兑换） ==========
    Swap {
        prc20: String,
        offer_denom: String,
        offer_amount: Uint128,
        min_receive: Uint128,
    },

    // ====================================================================
    // 🀄 三国卡牌（46 入口，裁定第 4 章）
    //    🟢 玩家身份一律由交易发送方 info.sender 决定：
    //    所有玩家入口都不再携带会话地址 / 随机数 / 签名字段，
    //    前端只需用玩家钱包直接发交易即可。
    // ====================================================================

    // ---- 管理员：配置（26 原样区）----
    /// C3：三字段全部废弃（migrate 清值），exec 存而不读；签名保留待专家复核是否硬化拒非空
    SanguoSetConfig {
        tkcc_token: String,
        burn_address: String,
        tap_addresses: Vec<String>,
    },
    SanguoSetParams { params: SanguoParams },
    SanguoAddTemplates { templates: Vec<CardTemplate> },
    /// C 档新增：60 张 × 11 字段回填直喂（裁定 8.1 覆盖写用；AddTemplates 仍可加新卡）
    SanguoSetCardsBatch { cards: Vec<CardTemplate> },

    // ---- 抽卡（免费 + 碎片加购）----
    /// tap_index @deprecated C3（生态抽水地址废弃），保留字段仅为签名兼容，exec 忽略
    SanguoDraw {
        tap_index: u8,
        pack3: bool,
        round_id: String,
    },

    // ---- 战斗（合并产出 2，裁定 4.3）----
    /// 单挑 + 混战统一入口。
    /// Single: Create(opponent=Some 指定 / None 公开, public, card_ids) /
    ///         Accept / Cancel / Claim(match_id)；
    /// Royale: Create(size=4..6, card_ids) / Join / Settle / Claim(royale_id)。
    /// 创建/接受/入场只校验卡牌归属 + 编队合法性（validate_battle_team），
    /// 不算战力（算力推迟到 Settle/Claim，与 L10 一致）。
    SanguoBattlePvp {
        mode: PvpMode,
        action: PvpAction,
        match_id: Option<String>,
        opponent: Option<String>,
        public: Option<bool>,
        royale_id: Option<String>,
        size: Option<u8>,
        card_ids: Vec<String>,
        round_id: String,
    },
    /// AI 休闲（level=难度 1..5）+ 天梯排位（level=段位 0..4）统一入口。
    /// 胜负由 compute_battle 引擎复算（E1~E3），奖励只发统一碎片。
    SanguoBattleAi {
        mode: AiMode,
        level: u8,
        card_ids: Vec<String>,
        round_id: String,
    },
    /// AI/关卡胜利奖励领取（原 SanguoClaimReward，原样保留）
    SanguoClaimReward {
        battle_id: String,
        round_id: String,
    },

    // ---- 编队 / 养成 ----
    SanguoSetBattleOrder {
        order: Vec<String>,
        round_id: String,
    },
    SanguoStarUp {
        card_id: String,
        use_fragments: Option<bool>,
        round_id: String,
    },
    SanguoDecompose {
        card_id: String,
        round_id: String,
    },
    /// B 档新增：批量分解（单笔事务省 gas；命中锁卡整批拒绝）
    SanguoDecomposeBatch {
        card_ids: Vec<String>,
        round_id: String,
    },
    SanguoUpgrade {
        card_id: String,
        round_id: String,
    },
    SanguoCraft {
        rarity: String,
        round_id: String,
    },

    // ---- 老三国合约资产迁移（卡牌 + 碎片，每钱包一次）----
    SanguoAddMigrationTarget { old_contract: String },
    SanguoMigrateFromOld {
        old_contract: String,
        round_id: String,
    },

    // ---- 合规配置 / 关卡 / 运营 ----
    /// C1 硬化：compliance_mode 字段删除（合规是唯一路径）。
    /// 保留 operators / daily limits 的设置能力（daily limits 实际写入 params）。
    SanguoSetComplianceConfig {
        operators: Option<Vec<String>>,
        daily_draw_limit: Option<u64>,
        daily_fragment_cap: Option<u64>,
    },
    /// 运营/管理员：整表写入关卡配置（覆盖式，校验模板存在；enemy_stars 可空=全 1★）
    SanguoSetStages { stages: Vec<StageConfig> },
    /// 运营/管理员：批量发碎片（发入 FRAGMENT_BALANCE，受 params.daily_fragment_cap 硬顶）
    SanguoGrantFragmentsBatch { grants: Vec<FragmentGrant> },
    /// 玩家：批量提交关卡进度（链下打、链上复算；每关卡每玩家仅一次）
    SanguoSubmitStageBatch {
        stage_id: u64,
        batch_id: String,
        card_ids: Vec<String>,
        round_id: String,
    },

    // ---- 商店（A 档 + 委托改造 4，裁定 2.1 / 9.1）----
    /// 统一购买分发：item_type ∈ skin/avatar/cardback/title/card/stamina/draw/reset。
    /// target = 类型内目标 id（皮肤 id / 模板 id / 关卡 id；draw/stamina 可空）。
    /// pay_with_fragment = true 走碎片币（FRAGMENT_BALANCE + 日限），false 走 PAXI。
    /// 原 SanguoResetStage / SanguoBuyDrawCount 收编为本入口分发臂（11.1 拍板）。
    SanguoBuyShopItem {
        item_id: String,
        target: Option<String>,
        pay_with_fragment: bool,
        round_id: String,
    },
    /// admin 批量 upsert 商品表（未来加商品只改这张表，零 migrate）
    SanguoSetShopItems { items: Vec<ShopItem> },
    /// 委托改造：签名不变，内部改调统一购买分发（9.1）
    SanguoBuyCard {
        card_name: String,
        round_id: String,
    },
    /// 委托改造：签名不变，内部改调统一购买分发
    SanguoBuySkin {
        skin_id: String,
        round_id: String,
    },
    /// 委托改造：签名不变，内部改调统一购买分发
    SanguoBuyStamina {
        pack_count: u32,
        round_id: String,
    },
    /// 委托改造：签名不变，内部改调统一购买分发
    SanguoBuySeasonPass {
        round_id: String,
    },
    /// 管理员：设置指定卡 PAXI 价（碎片价走 params.card_frag_price，T15）
    SanguoSetCardPrice { card_name: String, price: Uint128 },
    /// C5：废弃清值但入口保留（写码对账注记见文件头）
    SanguoSetProjectWallets { wallets: Vec<String> },

    // ---- 化妆品 / 卡锁 / 预设（B 档）----
    /// 装备化妆品（slot ∈ skin/avatar/cardback/title，校验已拥有）
    SanguoEquipCosmetic {
        slot: String,
        item_id: String,
        round_id: String,
    },
    SanguoUnequipCosmetic {
        slot: String,
        round_id: String,
    },
    /// 锁卡（分解/赠送/预设写入前必查）
    SanguoLockCard {
        card_id: String,
        locked: bool,
        round_id: String,
    },
    /// 写入队伍预设（preset_id ∈ 0..2，写入过 cost≤cap 校验）
    SanguoSetTeamPreset {
        preset_id: u8,
        card_ids: Vec<String>,
        name: String,
        round_id: String,
    },
    /// 一键切 PLAYER_BATTLE_ORDER
    SanguoUseTeamPreset {
        preset_id: u8,
        round_id: String,
    },

    // ---- 赛季 / 每日任务 ----
    SanguoClaimSeasonTier {
        tier_index: u64,
        round_id: String,
    },
    SanguoClaimDailyTask {
        task_id: u64,
        round_id: String,
    },

    // ---- 管理员：新玩法配置 ----
    SanguoSetSkins { skins: Vec<SkinInfo> },
    SanguoSetSeasonConfig { config: SeasonConfig },
    /// C 档：紧急暂停单玩法（game_id 如 "sanguo_pvp" / "sanguo_ai" / "sanguo_stage"）
    SanguoPauseGame { game_id: String, paused: bool },

    // ---- 赠送（冷静期体系保留，碎片走 FRAGMENT_BALANCE）----
    SanguoGiftCard {
        card_id: String,
        to: String,
        round_id: String,
    },
    SanguoGiftFragments {
        rarity: String,
        amount: u64,
        to: String,
        round_id: String,
    },
    SanguoGiftSkin {
        skin_id: String,
        to: String,
        round_id: String,
    },

    // ---- 管理员：经济护栏 / 支付代币 / 封禁 / 退款 ----
    SanguoSetGlobalDailyPayoutCap { cap: Uint128 },
    /// 支付代币（C6 已放开，2026-10-14 用户拍板）：空串 = 原生 PAXI；
    /// 非空 = 白名单内 PRC-20（须先 AddPrc20），收款走合约内部余额通道。
    SanguoSetPaymentToken { token: String },
    /// C 档：全局封禁（泛化原 GIFT_BANNED，存储 key "sg_gban" 不变零迁移；
    /// 所有玩法入口前置检查）
    SanguoBanPlayer { player: String, banned: bool },
    /// C 档：玩家领取提案押金退款（PAXI 键；来源 PENDING_REFUNDS，M3 记入）
    SanguoRefundProposalDeposit {
        round_id: String,
    },

    // ---- M5/M6：V2 玩家数据分批迁移（admin 反复调用，limit ≤ 50，M7）----
    /// 每批：四桶碎片 → 统一碎片（common×1/rare×5/epic×20/legend×80）；
    /// 单账户折算 ≤2 倍且 ≤50000，超额入 PENDING_FRAGMENT_REVIEW（值+游标）；
    /// 转换后旧桶 remove。同批顺带执行 M5：按 PLAYER_BATTLE_ORDER 镜像
    /// resolve_player_battle_cards，Σcost>30 发 500 统一碎片（每玩家仅一次）。
    /// 游标 = 上批最后一个 player 地址（首页传 None）。
    SanguoMigrateFragments {
        start_after: Option<String>,
        limit: Option<u32>,
    },
}

#[cw_serde]
#[derive(QueryResponses)]
pub enum QueryMsg {
    // ========== 通用域（原样保留） ==========
    #[returns(BalanceResponse)]
    Balance { address: String, token: Option<String> },
    #[returns(BurnConfigResponse)]
    BurnConfig {},
    #[returns(TreasuryConfigResponse)]
    TreasuryConfig {},
    #[returns(GameConfig)]
    GameConfig { game_id: String },
    #[returns(AdminsResponse)]
    ListAdmins {},
    #[returns(AllowanceResponse)]
    Allowance { owner: String, spender: String, token: String },
    #[returns(GameLimitResponse)]
    GameLimit { game_id: String },
    #[returns(ListGamesResponse)]
    ListGames {},
    #[returns(RoundStatusResponse)]
    RoundStatus { player: String, game_id: String, round_id: String },
    #[returns(DailyStatsResponse)]
    DailyStats { player: String, date: Option<u64>, game_id: Option<String> },
    #[returns(GameEngineConfigResponse)]
    GameEngineConfigQuery { game_id: String },
    #[returns(Prc20ListResponse)]
    ListPrc20 {},

    // ====================================================================
    // 🀄 三国卡牌查询（33，裁定 4.6：删 3 增 3 持平）
    // ====================================================================
    #[returns(SanguoConfigResponse)]
    SanguoConfig {},
    #[returns(SanguoParamsResponse)]
    SanguoParams {},
    #[returns(SanguoCardsResponse)]
    SanguoPlayerCards { address: String },
    #[returns(CardInfo)]
    SanguoCard { card_id: String },
    #[returns(Vec<CardTemplate>)]
    SanguoTemplates { start_after: Option<u32>, limit: Option<u32> },
    #[returns(SanguoRarityCountResponse)]
    SanguoRarityCount {},
    #[returns(SanguoAiStatsResponse)]
    SanguoAiStats { address: String },
    #[returns(SanguoBattleOrderResponse)]
    SanguoBattleOrder { player: String },
    #[returns(SanguoPendingResponse)]
    SanguoPending { address: String },
    #[returns(SanguoFragmentsResponse)]
    SanguoFragments { address: String },
    // （SanguoProposals / SanguoProposal / SanguoProposalVotes 已删——投票全禁 T24）
    #[returns(SanguoPvpResponse)]
    SanguoPvp { match_id: String },
    #[returns(SanguoPvpListResponse)]
    SanguoPvpList { player: Option<String>, status: Option<String>, limit: Option<u32> },
    #[returns(SanguoRoyaleResponse)]
    SanguoRoyale { royale_id: String },
    #[returns(SanguoRoyaleListResponse)]
    SanguoRoyaleList { status: Option<String>, limit: Option<u32> },
    #[returns(SanguoMigrationWhitelistResponse)]
    SanguoMigrationWhitelist {},
    /// C1：compliance_mode 字段已移除（总闸恒真）；返回 operators + params 解析日限
    #[returns(SanguoComplianceResponse)]
    SanguoCompliance {},
    #[returns(SanguoStagesResponse)]
    SanguoStages {},
    // ---- 商店 / 化妆品 / 图鉴（增 3）----
    #[returns(SanguoShopItemsResponse)]
    SanguoShopItems {},
    #[returns(SanguoEquippedCosmeticsResponse)]
    SanguoEquippedCosmetics { address: String },
    /// 图鉴：collected = 已收集模板 id（曾拥有），total_templates = 全卡池模板数
    #[returns(SanguoCollectionResponse)]
    SanguoCollection { address: String },
    // ---- 付费 / 新玩法 ----
    #[returns(SanguoCardPricesResponse)]
    SanguoCardPrices {},
    /// 语义变更（T15）：返回指定卡碎片价 card_frag_price[4]（原四桶碎片价表已废）
    #[returns(SanguoFragmentPricesResponse)]
    SanguoFragmentPrices {},
    #[returns(SanguoProjectWalletsResponse)]
    SanguoProjectWallets {},
    #[returns(SanguoStaminaResponse)]
    SanguoStamina { address: String },
    #[returns(SanguoSkinsResponse)]
    SanguoSkins {},
    #[returns(SanguoPlayerSkinsResponse)]
    SanguoPlayerSkins { address: String },
    #[returns(SanguoSeasonResponse)]
    SanguoSeason {},
    #[returns(SanguoPlayerSeasonResponse)]
    SanguoPlayerSeason { address: String },
    #[returns(SanguoRankResponse)]
    SanguoRank { address: String },
    #[returns(SanguoDailyTaskResponse)]
    SanguoDailyTask { address: String },
    /// 升级前存量结算进度（migrate 方案 Z 前置核对用）
    #[returns(SanguoLegacyStatusResponse)]
    SanguoLegacyStatus {},
    // ---- 赠送 ----
    #[returns(SanguoGiftStatusResponse)]
    SanguoGiftStatus { player: String, with: Option<String> },
    #[returns(SanguoGiftCandidatesResponse)]
    SanguoGiftCandidates { player: String, limit: Option<u32> },
}

// ===== 响应结构体（通用域，原样保留） =====

#[cw_serde]
pub struct BalanceResponse {
    pub amount: Uint128,
}

#[cw_serde]
pub struct BurnConfigResponse {
    pub burn_rate: u64,
    pub total_burned: Uint128,
}

#[cw_serde]
pub struct AdminsResponse {
    pub admins: Vec<String>,
}

#[cw_serde]
pub struct AllowanceResponse {
    pub allowance: Uint128,
}

#[cw_serde]
pub struct TreasuryConfigResponse {
    pub treasury_list: Vec<String>,
    pub treasury_share: u64,
    pub total_treasury: Uint128,
}

#[cw_serde]
pub struct GameLimitResponse {
    pub limit: Option<GameLimit>,
}

#[cw_serde]
pub struct ListGamesResponse {
    pub games: Vec<GameEntry>,
}

#[cw_serde]
pub struct GameEntry {
    pub game_id: String,
    pub enabled: bool,
    pub min_bet: Uint128,
    pub max_bet: Uint128,
    pub max_payout_multiplier: u64,
    pub engine: Option<GameEngine>,
    pub max_daily_bet: Uint128,
    pub max_daily_rounds: u64,
}

#[cw_serde]
pub struct RoundStatusResponse {
    pub status: Option<String>,
    pub bet: Option<Uint128>,
    pub payout: Option<Uint128>,
}

/// 单个游戏的每日限额快照
#[cw_serde]
pub struct GameDailyLimit {
    pub game_id: String,
    pub enabled: bool,
    pub max_daily_bet: Uint128,
    pub max_daily_rounds: u64,
}

#[cw_serde]
pub struct DailyStatsResponse {
    pub total_bet: Uint128,
    pub rounds_played: u64,
    pub max_daily_bet: Uint128,
    pub max_daily_rounds: u64,
    pub game_id: Option<String>,
    pub game_limits: Vec<GameDailyLimit>,
}

#[cw_serde]
pub struct GameEngineConfigResponse {
    pub config: Option<GameEngineConfig>,
}

#[cw_serde]
pub struct Prc20ListResponse {
    pub tokens: Vec<String>,
}

// ====================================================================
// 🀄 三国卡牌响应结构体
//    ⚠️ 字段集合必须与 query.rs 构造处逐字段一致，同时是前端 sanguo.js
//    直接消费的 JSON 形状——改任何一处都要三处对齐（msg / query / 前端）。
// ====================================================================

#[cw_serde]
pub struct SanguoConfigResponse {
    /// C3：三字段 deprecated，migrate 后恒为空串/空列表，前端可停止展示
    pub tkcc_token: Option<String>,
    pub burn_address: Option<String>,
    pub tap_addresses: Vec<String>,
}

/// V2 参数响应：全部为**访问器解析后的生效值**（0/全零已回退默认），
/// 前端拿到的就是合约将实际使用的数字，无需再做 0 值兜底。
/// deprecated 字段（C4）不再暴露——它们恒 0 且 exec 不读。
#[cw_serde]
pub struct SanguoParamsResponse {
    // ---- 抽卡 ----
    pub daily_draw_limit: u64,
    pub daily_draw_purchase_cap: u32,
    /// 保底阈值（0 = 无保底）
    pub pity_threshold: u32,
    // ---- 碎片 ----
    pub daily_fragment_cap: u64,
    // ---- 体力 ----
    pub stamina_max: u64,
    pub stamina_regen_secs: u64,
    pub stamina_pack_amount: u64,
    /// 体力包单价（upaxi 字符串）
    pub stamina_pack_price: String,
    // ---- 持有 ----
    pub max_player_cards: u32,
    pub max_cards: u32,
    pub rarity_max: [u64; 4],
    // ---- 合成 / 分解 / 升星 ----
    pub craft_cost: [u64; 4],
    pub fragment_from_duplicate: [u64; 4],
    pub star_up_fragment_cost: [u64; 4],
    pub upgrade_atk_boost: [u32; 4],
    pub upgrade_def_boost: [u32; 4],
    pub upgrade_frag_atk: u32,
    pub upgrade_frag_def: u32,
    pub upgrade_frag_max_level: u8,
    // ---- AI 难度 ----
    pub diff_boost_bp: [u64; 5],
    pub ai_floor_bp: [u64; 5],
    pub daily_ai_limit: u64,
    // ---- 天梯排位 ----
    pub rank_boost_bp: [u64; 5],
    pub rank_win_points: i64,
    pub rank_lose_points: i64,
    pub rank_tier_up_points: i64,
    pub rank_stamina_cost: u64,
    pub rank_win_season_points: u64,
    pub rank_rewards: [Fragments; 5],
    // ---- 每日任务 ----
    pub daily_task_count: u32,
    pub daily_task_required: [u64; 4],
    pub daily_task_reward: [Fragments; 4],
    pub daily_task_season_points: u64,
    // ---- 赠送冷却（9 项）----
    pub gift_holding_secs: u64,
    pub gift_legend_holding_secs: u64,
    pub gift_pair_age_secs: u64,
    pub gift_acct_age_secs: u64,
    pub gift_pair_idle_expire_secs: u64,
    pub gift_pair_refresh_interval: u64,
    pub gift_daily_sent: u32,
    pub gift_daily_recv: u32,
    pub gift_frag_cap: u64,
    // ---- 战斗奖励（统一碎片四桶表展示；入账折算 to_unified）----
    pub ai_frag_reward: [Fragments; 5],
    pub pvp_frag_reward: Fragments,
    pub royale_frag_reward: Fragments,
    // ---- 指定卡碎片价 / 编队 ----
    pub card_frag_price: [u64; 4],
    pub team_cost_cap: u32,
    // ---- 战斗系数（12 项，裁定 6.2；访问器解析后的生效值）----
    pub faction_counter_bp: u64,
    pub faction_countered_bp: u64,
    pub faction_neutral_bp: u64,
    pub troop_counter_bp: u64,
    pub troop_countered_bp: u64,
    pub troop_neutral_bp: u64,
    pub rarity_legend_bp: u64,
    pub rarity_epic_bp: u64,
    pub rarity_rare_bp: u64,
    pub rarity_common_bp: u64,
    pub power_flat_cap_bp: u64,
    pub eff_lower_bound_bp: u64,
    /// 🟢 参数收敛：技能强度全局系数（bp，10000=默认；已解析回退值）
    pub skill_strength_bp: u64,
    /// 🟢 参数收敛：连携强度全局系数（bp，10000=默认；已解析回退值）
    pub bond_strength_bp: u64,
    /// 当前支付代币。空串 = 原生 PAXI（C6 已放开：管理员可切白名单内 PRC-20）
    pub pay_token: String,
}

#[cw_serde]
pub struct SanguoCardsResponse {
    pub cards: Vec<CardInfo>,
}

#[cw_serde]
pub struct SanguoRarityCountResponse {
    pub common: u64,
    pub rare: u64,
    pub epic: u64,
    pub legend: u64,
    pub total: u64,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq, JsonSchema)]
pub struct SanguoAiStatsResponse {
    pub total: u64,
    pub wins: u64,
    /// 胜率百分比字符串（如 "42.86"），避免浮点精度在 JSON 里漂移
    pub win_rate: String,
    pub today_count: u64,
    /// AI 每日限次（语义 = params.ai_daily_limit()；字段名保留 V1 兼容）
    pub daily_limit: u64,
    /// V2.1：分模式计数（total/wins 以上仍为 AI 专属，语义不变）
    pub pvp_total: u64,
    pub pvp_wins: u64,
    pub royale_total: u64,
    pub royale_wins: u64,
    pub stage_total: u64,
    pub stage_wins: u64,
    /// 全模式合计（首页「总对战/胜场」取这两个）
    pub all_total: u64,
    pub all_wins: u64,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq, JsonSchema)]
pub struct SanguoBattleOrderResponse {
    pub order: Vec<String>,
    /// V2 附加：该玩家的队伍预设（preset_id, name, card_ids），前端一次拉取；
    /// 未设预设时为空数组（增量字段，旧前端可忽略）
    pub presets: Vec<SanguoTeamPresetResponse>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq, JsonSchema)]
pub struct SanguoTeamPresetResponse {
    pub preset_id: u8,
    pub name: String,
    pub card_ids: Vec<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq, JsonSchema)]
pub struct SanguoPendingResponse {
    /// 统一碎片余额（FRAGMENT_BALANCE 字符串）——V2 起待领奖励即时入账，
    /// 本字段即玩家当前碎片；battle_ids 保留兼容（V2 恒空）
    pub total_rewards: String,
    pub battle_ids: Vec<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq, JsonSchema)]
pub struct SanguoFragmentsResponse {
    pub common: u64,
    pub rare: u64,
    pub epic: u64,
    pub legend: u64,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq, JsonSchema)]
pub struct SanguoPvpResponse {
    pub match_id: String,
    pub challenger: String,
    pub opponent: String,
    pub is_public: bool,
    pub challenger_order: Vec<String>,
    pub opponent_order: Vec<String>,
    pub winner: Option<String>,
    pub status: String,
    pub created_at: u64,
    pub finished_at: Option<u64>,
    pub reward_claimed: bool,
    /// 🟢 Q3 亮牌：逐组胜负（A=挑战者、B=应战者；元素=该方该组赢下的局数 0~3）
    pub group_scores_a: [u8; 3],
    pub group_scores_b: [u8; 3],
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq, JsonSchema)]
pub struct SanguoPvpListResponse {
    pub matches: Vec<SanguoPvpResponse>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq, JsonSchema)]
pub struct SanguoRoyaleResponse {
    pub royale_id: String,
    pub players: Vec<String>,
    pub player_orders: Vec<Vec<String>>,
    pub winner: Option<String>,
    pub status: String,
    pub created_at: u64,
    pub finished_at: Option<u64>,
    pub size: u8,
    pub reward_claimed: bool,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq, JsonSchema)]
pub struct SanguoRoyaleListResponse {
    pub royales: Vec<SanguoRoyaleResponse>,
}

/// 老三国合约迁移白名单查询结果
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq, JsonSchema)]
pub struct SanguoMigrationWhitelistResponse {
    pub contracts: Vec<String>,
}

/// C1：compliance_mode 已删（恒真无意义），返回 operators + params 解析日限
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq, JsonSchema)]
pub struct SanguoComplianceResponse {
    pub operators: Vec<String>,
    pub daily_draw_limit: u64,
    pub daily_fragment_cap: u64,
}

/// 升级前存量结算进度（方案 Z 前置核对）
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq, JsonSchema)]
pub struct SanguoLegacyStatusResponse {
    pub pending_pvp: u64,
    pub pending_royale: u64,
    pub finished_unclaimed_pvp: u64,
    pub finished_unclaimed_royale: u64,
    /// migrate 后恒 0（投票全禁）；字段保留供旧前端兼容
    pub unexecuted_proposals: u64,
    pub contract_paxi: Uint128,
    /// (PRC-20 合约地址, 合约持有余额)
    pub prc20_balances: Vec<(String, String)>,
    /// M3 记入的待退押金（PAXI 键；玩家可 SanguoRefundProposalDeposit 领取）
    pub pending_refunds: Vec<(String, String)>,
    /// M6 待审超额碎片队列（player, 折算值, 游标）
    pub pending_fragment_review: Vec<(String, String, u64)>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, JsonSchema)]
pub struct SanguoStagesResponse {
    pub stages: Vec<StageConfig>,
}

// ===== 商店 / 化妆品 / 图鉴（增 3） =====

#[cw_serde]
pub struct SanguoShopItemsResponse {
    /// 全量商品表（enabled=false 的也返回，前端可灰显"已下架"）
    pub items: Vec<ShopItem>,
}

#[cw_serde]
pub struct SanguoEquippedCosmeticsResponse {
    /// (slot, item_id)；仅返回已装备位
    pub equipped: Vec<(String, String)>,
}

#[cw_serde]
pub struct SanguoCollectionResponse {
    /// 已收集模板 id（曾拥有即计入，卖掉/分解不掉图鉴）
    pub collected: Vec<String>,
    /// 全卡池模板总数（收集进度分母）
    pub total_templates: u64,
}

// ===== 付费 / 新玩法响应 =====

#[cw_serde]
pub struct SanguoCardPricesResponse {
    /// (卡牌名, 价格 upaxi)
    pub prices: Vec<(String, String)>,
}

/// 语义变更（T15）：指定卡碎片价，按稀有度 common/rare/epic/legend
#[cw_serde]
pub struct SanguoFragmentPricesResponse {
    pub card_frag_price: [u64; 4],
}

#[cw_serde]
pub struct SanguoProjectWalletsResponse {
    pub wallets: Vec<String>,
    pub next_index: u64,
}

#[cw_serde]
pub struct SanguoStaminaResponse {
    pub current: u64,
    pub max: u64,
    pub last_update: u64,
    pub regen_per_secs: u64,
}

#[cw_serde]
pub struct SanguoSkinsResponse {
    pub skins: Vec<SkinInfo>,
}

#[cw_serde]
pub struct SanguoPlayerSkinsResponse {
    pub skins: Vec<String>,
}

#[cw_serde]
pub struct SanguoSeasonResponse {
    pub season_id: u64,
    pub active: bool,
    pub start_time: u64,
    pub end_time: u64,
    pub pass_price_paxi: String,
    pub tiers: Vec<SanguoSeasonTierResponse>,
}

#[cw_serde]
pub struct SanguoSeasonTierResponse {
    pub points_req: u64,
    pub reward: SanguoFragmentsResponse,
}

#[cw_serde]
pub struct SanguoPlayerSeasonResponse {
    pub pass_owned: bool,
    pub points: u64,
    pub claimed: Vec<u64>,
}

#[cw_serde]
pub struct SanguoRankResponse {
    pub tier: u8,
    pub points: i64,
}

#[cw_serde]
pub struct SanguoDailyTaskResponse {
    pub day: u64,
    pub tasks: Vec<SanguoDailyTaskItem>,
}

#[cw_serde]
pub struct SanguoDailyTaskItem {
    pub task_id: u64,
    pub progress: u64,
    pub required: u64,
    pub claimed: bool,
    pub reward: SanguoFragmentsResponse,
}

// ===== 🎁 赠送系统：响应结构体 =====

#[cw_serde]
pub struct SanguoGiftStatusResponse {
    pub banned: bool,
    pub account_first_seen: u64,
    pub account_age_secs: u64,
    pub sent_today: u32,
    pub recv_today: u32,
    pub daily_sent_limit: u32,
    pub daily_recv_limit: u32,
    pub pair_first_at: u64,
    pub pair_last_at: u64,
    pub pair_ok: bool,
    pub pair_age_needed_secs: u64,
    pub pair_idle_expire_secs: u64,
    pub holding_secs: u64,
    pub legend_holding_secs: u64,
    pub fragment_cap: u64,
}

#[cw_serde]
pub struct GiftCandidate {
    pub player: String,
    pub first_at: u64,
    pub last_at: u64,
    pub age_secs: u64,
    pub idle_secs: u64,
    pub eligible: bool,
}

#[cw_serde]
pub struct SanguoGiftCandidatesResponse {
    pub candidates: Vec<GiftCandidate>,
}
