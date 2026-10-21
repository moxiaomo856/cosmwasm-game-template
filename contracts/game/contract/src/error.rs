use cosmwasm_std::StdError;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum ContractError {
    #[error("{0}")]
    Std(#[from] StdError),

    #[error("Unauthorized")]
    Unauthorized,

    #[error("Paused")]
    Paused,

    #[error("Game not found: {0}")]
    GameNotFound(String),

    #[error("Game disabled: {0}")]
    GameDisabled(String),

    #[error("Insufficient balance")]
    InsufficientBalance,

    #[error("Invalid amount")]
    InvalidAmount,

    #[error("Invalid parameters: {reason}")]
    InvalidParams { reason: String },

    // 🟢 修复（低危 1）：未知 SubMsg reply id 不再静默 Ok，
    //    避免将来新增 SubMsg 却忘记加 handler 时被静默吞掉。
    // 🟢 L6 审计修复：错误信息带上已知 id 列表，排障时不用再翻源码。
    #[error("Unhandled submessage reply id: {id} (known ids: {known:?})")]
    UnhandledReplyId { id: u64, known: Vec<u64> },

    #[error("Contract not in whitelist")]
    ContractNotWhitelisted,

    #[error("Overflow")]
    Overflow,

    // ===== 管理员 =====
    #[error("Admins cannot be empty")]
    EmptyAdmins,

    #[error("Admin already exists")]
    AdminExists,

    #[error("Cannot remove the last admin")]
    LastAdmin,

    // ===== 资金池 =====
    #[error("House bankroll insufficient for burn")]
    InsufficientBankrollForBurn,

    // 🟢 第二轮修复：原来 #[error(...)] 里引用了 {have} / {need}，
    // 但变体本身是单元变体、没有这两个字段 —— thiserror 展开后会去找
    // `self.have`，直接编译失败。这里补上字段，报错时能看到缺口有多大。
    #[error("House bankroll insufficient to cover player profit (have {have}, need {need})")]
    InsufficientBankrollForProfit { have: String, need: String },

    #[error("Bank balance below total user balance; contract is insolvent")]
    Insolvent,

    // ===== PRC-20 =====
    #[error("PRC-20 transferFrom failed")]
    Prc20TransferFromFailed,

    #[error("PRC-20 approve failed")]
    Prc20ApproveFailed,

    #[error("User did not grant sufficient allowance")]
    InsufficientAllowance,

    // 🟢 外部审计 #9：与 InsufficientAllowance 区分——allowance 查询本身失败
    //    （PRC-20 合约无响应/接口不匹配/反序列化失败），前端才能给出正确提示
    #[error("Failed to query PRC-20 allowance: {0}")]
    Prc20AllowanceQueryFailed(String),

    // ===== 版本 =====
    #[error("Contract version mismatch for migrate: expected {expected}, got {got}")]
    VersionMismatch { expected: String, got: String },

    // ===== 运营分 =====
    #[error("Burn rate + Treasury share exceeds 100% (10000 basis points)")]
    SplitExceedsHundredPercent,

    #[error("Treasury address cannot be empty when treasury_share > 0")]
    TreasuryNotSet,

    // ===== 🟢 新增：游戏限额相关 =====
    #[error("Bet amount {bet} below minimum {min}")]
    BetTooLow { bet: String, min: String },

    #[error("Bet amount {bet} above maximum {max}")]
    BetTooHigh { bet: String, max: String },

    #[error("Payout {payout} exceeds max multiplier ({max_mult}x) of bet {bet}")]
    PayoutTooHigh { payout: String, bet: String, max_mult: u64 },

    #[error("Round {round_id} already used (status: {status})")]
    RoundAlreadyUsed { round_id: String, status: String },

    #[error("Round {round_id} not found or not in 'deducted' state")]
    RoundNotFoundOrNotDeducted { round_id: String },

    #[error("Daily bet limit exceeded: {used} / {limit}")]
    GameDailyBetExceeded { used: String, limit: String },

    #[error("Daily rounds limit exceeded: {played} / {limit}")]
    GameDailyRoundsExceeded { played: u64, limit: u64 },

    #[error("Payout cannot be less than bet when won=true")]
    PayoutLessThanBet,

    // ===== 🟢 游戏引擎相关 =====
    #[error("Game engine not configured for game: {0}")]
    GameEngineNotConfigured(String),

    #[error("Game engine config already exists for game: {0}")]
    GameEngineConfigExists(String),

    #[error("Engine input is empty or not matching the expected engine type")]
    EngineInputMismatch,

    #[error("Guess {guess} out of range [{min}, {max}]")]
    GuessOutOfRange { guess: u64, min: u64, max: u64 },

    #[error("Roulette slot {slot} out of range (0..{slots})")]
    RouletteSlotOutOfRange { slot: u64, slots: u64 },

    #[error("Invalid dice bet type: {bet_type}")]
    InvalidDiceBetType { bet_type: String },

    #[error("Card count {count} out of range [{min}, {max}]")]
    CardCountOutOfRange { count: u64, min: u64, max: u64 },

    #[error("Invalid card mode: {mode}")]
    InvalidCardMode { mode: String },

    #[error("Score {score} exceeds max {max}")]
    ScoreExceedsMax { score: u64, max: u64 },

    #[error("Unsupported engine type: {engine}")]
    UnsupportedEngine { engine: String },

    // ===== 🟢 审计新增：引擎参数校验 =====
    #[error("Invalid engine params: {0}")]
    InvalidEngineParams(String),

    // ===== 🟢 审计新增：Deduct 超时强制结算 =====
    #[error("Round {round_id} has not expired yet (deadline: {deadline})")]
    RoundNotExpired { round_id: String, deadline: u64 },

    // 🟢 第二轮新增：SetRoundTimeout 参数越界
    #[error("Round timeout {secs}s out of range [{min}, {max}]")]
    InvalidRoundTimeout { secs: u64, min: u64, max: u64 },

    #[error("Score engine is disabled (frontend score manipulation vulnerability). Use Deduct/Settle two-step with strict limits instead")]
    ScoreEngineDisabled,

    // ===== Swap =====
    #[error("Failed to parse actual receive amount from Swap module events; transaction rolled back for safety")]
    SwapReceiveParseFailed,

    // 🟢 M2 审计修复（2026-10-10）：reply payload 由合约自己构造，正常不会解析失败；
    //    一旦失败（版本升级/人为破坏）必须显式报错回滚，而不是静默按 0 处理
    //    （回滚分支按 0 记账 = 用户资产凭空消失；成功分支 min_receive=0 = 滑点保护失效）。
    #[error("Swap reply payload is malformed; transaction rolled back for safety")]
    SwapPayloadMalformed,

    // 🟢 审计新增：滑点保护。旧实现只打一个 attribute 就照常入账，
    // 等于 min_receive 这个参数完全失效（用户可被三明治攻击）。
    #[error("Swap slippage exceeded: expected at least {min_receive}, actually received {actual_receive}")]
    SwapSlippageExceeded { min_receive: String, actual_receive: String },

    // ====================================================================
    // 🀄 三国卡牌模块错误（复用通用合约 BALANCES / HOUSE_BANKROLL）
    //
    // 第九轮审计清理：只保留当前已暴露消息（SanguoSetConfig / SanguoSetParams /
    // SanguoAddTemplates / SanguoDraw）会用到的变体。AI 对战 / 提案 / PVP /
    // 混战 / 养成相关错误变体已移除，完整存档见 docs/dead-code-archive/error.rs.bak，
    // 下一版本开放相关玩法时恢复。
    // ====================================================================
    #[error("Sanguo game not configured (call SetConfig/AddTemplates first)")]
    SanguoNotConfigured,

    #[error("Sanguo eco/tap vault balance insufficient")]
    SanguoInsufficientVault,

    #[error("Message variant not handled by sanguo module")]
    SanguoNotHandled,

    #[error("Card template error: {0}")]
    SanguoTemplateError(String),

    #[error("No card templates configured")]
    SanguoNoTemplates,

    // 🔴 安全重审（2026-10-12）：出金路径 PRC-20 余额查询失败时返回（H-1 防御纵深）
    #[error("Solvency check failed: cannot query balance of {token}")]
    SolvencyQueryFailed { token: String },

    #[error("Invalid eco/tap (vault) address")]
    SanguoInvalidTap,

    #[error("Insufficient PAXI: expected {expected}, got {got}")]
    SanguoInsufficientPaxi { expected: u128, got: u128 },

    #[error("Invalid amount")]
    SanguoInvalidAmount,

    #[error("Card not found: {0}")]
    SanguoCardNotFound(String),

    #[error("Insufficient fragments: needed {needed}, have {have}")]
    SanguoInsufficientFragments { needed: u64, have: u64 },

    #[error("Invalid rarity: {0}")]
    SanguoInvalidRarity(String),

    // ===== 🀄 三国卡牌：玩法错误（AI / PVP / 混战 / 提案 / 养成）=====
    #[error("Invalid difficulty level: {0}")]
    SanguoInvalidDifficulty(u8),

    #[error("Daily AI battle limit reached: {0}")]
    SanguoDailyAiLimit(u64),

    #[error("Need at least 3 cards to battle")]
    SanguoNeed3Cards,

    #[error("Battle record not found: {0}")]
    SanguoBattleNotFound(String),

    #[error("Reward already claimed")]
    SanguoAlreadyClaimed,

    #[error("No reward available to claim")]
    SanguoNoReward,

    #[error("Card already at maximum star")]
    SanguoMaxStar,

    #[error("Card already at maximum level")]
    SanguoMaxLevel,

    // （提案/投票错误变体已删除——投票功能全禁 T24，入口与存储一并移除）

    #[error("Duplicate card id: {0}")]
    SanguoDuplicateCard(String),

    #[error("Cannot challenge yourself")]
    SanguoCannotSelfChallenge,

    #[error("PvP match not found: {0}")]
    SanguoMatchNotFound(String),

    #[error("Cannot accept your own match")]
    SanguoCannotSelfAccept,

    #[error("Caller is not the opponent of this match")]
    SanguoNotOpponent,

    #[error("Match timed out")]
    SanguoMatchTimeout,

    #[error("Match is not in waiting state")]
    SanguoMatchNotWaiting,

    #[error("No winner determined for match")]
    SanguoNoWinner,

    #[error("Match is not finished")]
    SanguoMatchNotFinished,

    #[error("Invalid size: {0}")]
    SanguoInvalidSize(u8),

    #[error("Royale match not found: {0}")]
    SanguoRoyaleNotFound(String),

    #[error("Royale is not in waiting state")]
    SanguoRoyaleNotWaiting,

    #[error("Already joined this royale")]
    SanguoAlreadyJoined,

    #[error("Royale is full")]
    SanguoRoyaleFull,

    #[error("Royale is not full yet")]
    SanguoRoyaleNotFull,

    #[error("Caller is not a participant of this royale")]
    SanguoNotParticipant,

    #[error("Royale is not finished")]
    SanguoRoyaleNotFinished,

    // 🟢 AI经济修复 ④：全局日赔付超过 cap（防多钱包绕限额抽干池子）
    #[error("Global daily payout exceeded: current={current}, cap={cap}")]
    SanguoGlobalDailyPayoutExceeded { current: u128, cap: u128 },

    // 🟢 审计 H5（2026-10-11）：遗留 TKCC 对局未结清，拒绝 migrate。
    //    碎片版领奖路径不再付 TKCC，强行迁移会使旧对局入场费/奖励永久锁死。
    #[error("Legacy battles unsettled: pvp={pvp}, royale={royale}, pending_rewards={pending}; settle them first or pass ignore_legacy_battles=true")]
    SanguoLegacyBattlesUnsettled { pvp: u64, royale: u64, pending: u64 },

    // ===== 🀄 三国卡牌：老合约资产迁移 =====
    #[error("Player {0} has already migrated assets from the old contract")]
    SanguoAlreadyMigrated(String),

    // 传入的 old_contract 不在 MIGRATION_WHITELIST 里。
    // 白名单是防止"自建恶意合约伪造卡牌无限刷"的唯一防线，宁可报错也别放行。
    #[error("Old contract is not whitelisted for migration")]
    SanguoMigrationNotAllowed,

    // 跨合约查询失败（地址不是 wasm / 接口名不匹配 / 反序列化失败）。
    // 常见原因是把新合约的 sanguo_ 前缀查询名误用到老合约上。
    #[error("Failed to query old contract: {0}")]
    SanguoOldContractQueryFailed(String),

    // ====================================================================
    // 🟢 合规模式（v0.1.1）
    // ====================================================================
    /// 通用博弈引擎 / 涉代币玩法在合规模式下被禁用
    #[error("Disabled in compliance mode (no token wagering or token payouts allowed)")]
    ComplianceDisabled,

    /// 免费抽卡达到当日限次
    #[error("Daily free draw limit reached: {limit}")]
    SanguoDrawLimitReached { limit: u64 },

    /// 每日碎片发放超过硬顶（所有渠道合计）
    #[error("Daily fragment cap exceeded: today granted {granted}, requested {requested}, cap {cap}")]
    SanguoFragmentCapReached { granted: u64, requested: u64, cap: u64 },

    /// 9v9 编队必须恰好 9 张卡
    #[error("Need exactly 9 cards to battle (3 groups of 3)")]
    SanguoNeed9Cards,

    /// 关卡不存在
    #[error("Stage not found: {0}")]
    SanguoStageNotFound(u64),

    /// 关卡敌人模板 id 不在卡池中
    #[error("Stage enemy template not found: {0}")]
    SanguoStageEnemyNotFound(String),

    /// 调用者不是运营白名单地址（GrantFragmentsBatch）
    #[error("Caller is not an authorized operator")]
    UnauthorizedOperator,

    /// 关卡 id 重复
    #[error("Duplicate stage id: {0}")]
    SanguoDuplicateStage(u64),

    /// batch_id 为空
    #[error("batch_id cannot be empty")]
    SanguoEmptyBatchId,

    /// 关卡未通关（复算判定玩家失败，整批拒绝）
    #[error("Stage battle recheck failed: player lost, batch rejected")]
    SanguoStageNotCleared,

    // ====================================================================
    // 🆕 付费系统：买卡 + 买碎片
    // ====================================================================
    #[error("Item not for sale: {0}")]
    SanguoNotForSale(String),

    #[error("Insufficient PAXI payment: expected {expected} upaxi, got {got} upaxi")]
    SanguoInsufficientPayment { expected: String, got: String },

    #[error("Project wallets not configured (admin must call SanguoSetProjectWallets)")]
    SanguoNoProjectWallet,

    #[error("Invalid PAXI amount: must be > 0")]
    SanguoInvalidPaxiAmount,

    #[error("Invalid pack count: must be 1..=100")]
    SanguoInvalidPackCount,

    // ====================================================================
    // 🆕 新玩法：天梯 / 皮肤 / 通行证 / 体力 / 每日任务
    // ====================================================================
    /// 体力不足
    #[error("Insufficient stamina: have {have}, need {need}")]
    SanguoInsufficientStamina { have: u64, need: u64 },

    /// 体力已满（二次审计：满体力购买多余部分会蒸发，直接拒绝）
    #[error("Stamina already full (current {current}/{max})")]
    SanguoStaminaFull { current: u64, max: u64 },

    /// funds 中混入非 upaxi 代币（二次审计：忽略会导致账实脱钩，直接拒绝）
    #[error("Unsupported denom in funds: {denom} (only upaxi accepted)")]
    SanguoUnsupportedDenom { denom: String },

    /// 玩家卡牌数达到上限（二次审计：防 PLAYER_CARDS 无界增长）
    #[error("Player card list full (max {0})")]
    SanguoTooManyCards(usize),

    /// 皮肤不存在 / 未配置
    #[error("Skin not found: {0}")]
    SanguoSkinNotFound(String),

    /// 已拥有该皮肤
    #[error("Skin already owned: {0}")]
    SanguoSkinAlreadyOwned(String),

    /// 赛季未激活（未配置或不在赛季窗口内）
    #[error("Season not active (no season configured or outside season window)")]
    SanguoSeasonNotActive,

    /// 未购买赛季通行证
    #[error("Season pass not purchased")]
    SanguoNoSeasonPass,

    /// 赛季通行证已购买
    #[error("Season pass already purchased")]
    SanguoSeasonPassOwned,

    /// 赛季层级已领取
    #[error("Season tier already claimed: {0}")]
    SanguoSeasonTierClaimed(u64),

    /// 赛季层级不存在
    #[error("Season tier not found: {0}")]
    SanguoSeasonTierNotFound(u64),

    /// 赛季层级未达要求（点数不足）
    #[error("Season tier not reached: have {have}, need {need}")]
    SanguoSeasonTierNotReached { have: u64, need: u64 },

    /// 天梯层级无效
    #[error("Invalid rank tier: {0}")]
    SanguoInvalidRankTier(u8),

    /// 每日任务不存在
    #[error("Daily task not found: {0}")]
    SanguoDailyTaskNotFound(u64),

    /// 每日任务未完成
    #[error("Daily task not completed yet (progress {progress}/{required})")]
    SanguoDailyTaskIncomplete { progress: u64, required: u64 },

    /// 每日任务今日已领取
    #[error("Daily task already claimed today")]
    SanguoDailyTaskClaimed,

    // ===== 🀄 三国卡牌：赠送系统 =====
    #[error("Cannot gift to yourself")]
    SanguoCannotGiftSelf,

    #[error("Gift banned for this account")]
    SanguoGiftBanned,

    #[error("Account too young: created {created}, need {need}s more")]
    SanguoGiftAccountTooYoung { created: u64, need: u64 },

    #[error("No established relation with recipient (need ≥1 shared battle)")]
    SanguoNoRelation,

    #[error("Relation too young: since {since}, need {need}s more")]
    SanguoRelationTooYoung { since: u64, need: u64 },

    #[error("Daily gift-sent limit reached: {used}/{limit}")]
    SanguoGiftSentLimitReached { used: u32, limit: u32 },

    #[error("Daily gift-received limit reached: {used}/{limit}")]
    SanguoGiftReceivedLimitReached { used: u32, limit: u32 },

    #[error("Holding period not met: last event at {acquired_at}, need {need}s more")]
    SanguoGiftHoldingPeriodNotMet { acquired_at: u64, need: u64 },

    #[error("Card gift meta missing for {card_id}")]
    SanguoGiftCardMetaMissing { card_id: String },

    #[error("Fragment amount exceeds cap: {amount} > {max}")]
    SanguoGiftFragmentCapExceeded { amount: u64, max: u64 },

    #[error("Rarity not giftable: {rarity}")]
    SanguoGiftRarityNotAllowed { rarity: String },

    // ====================================================================
    // 🆕 V2 新玩法错误（裁定 4.4 A/B/C 档 + 商店 + 战斗引擎 + 迁移方案 Z）
    // ====================================================================
    /// 商品不存在
    #[error("Shop item not found: {0}")]
    SanguoShopItemNotFound(String),

    /// 商品已下架
    #[error("Shop item disabled: {0}")]
    SanguoShopItemDisabled(String),

    /// 商品当日限购已达
    #[error("Shop item daily limit reached: {item} bought {bought}/{limit}")]
    SanguoShopDailyLimit { item: String, bought: u32, limit: u32 },

    /// 商品不接受该币种
    #[error("Shop item {item} does not accept {currency}")]
    SanguoShopCurrencyNotAccepted { item: String, currency: String },

    /// 化妆品装备位无效
    #[error("Invalid cosmetic slot: {0} (valid: skin/avatar/cardback/title)")]
    SanguoInvalidCosmeticSlot(String),

    /// 未拥有该化妆品（装备前置校验）
    #[error("Cosmetic not owned: slot {slot}, item {item}")]
    SanguoCosmeticNotOwned { slot: String, item: String },

    /// 卡已被锁定（分解/赠送/入预设前必查）
    #[error("Card is locked: {0}")]
    SanguoCardLocked(String),

    /// 预设槽位无效（0..MAX_TEAM_PRESETS-1）
    #[error("Invalid team preset slot: {0}")]
    SanguoTeamPresetSlotInvalid(u8),

    /// 编队 Σcost 超上限（T12）
    #[error("Team cost exceeded: total {total}, cap {cap}")]
    SanguoTeamCostExceeded { total: u32, cap: u32 },

    /// 混战结算静态预算超限（E6：防 OOG）
    #[error("Royale settle too expensive (participants produce too many engine battles)")]
    RoyaleSettleTooExpensive,

    /// 账号被全局封禁（T8：BANNED_PLAYERS，所有玩法入口前置检查）
    #[error("Account is globally banned")]
    SanguoBanned,

    /// 无待退提案押金
    #[error("No pending proposal deposit refund")]
    SanguoNoPendingRefund,

    /// V2 migrate 未执行（M5/M6 玩家数据分批迁移必须在 migrate 之后）
    #[error("V2 migration not done yet (run migrate first)")]
    SanguoMigrationNotDone,

    /// M6 游标非法（start_after 地址无待迁数据）
    #[error("Invalid migration cursor: {0}")]
    SanguoMigrationCursorInvalid(String),

    /// limit 超限（M7：分批 ≤50）
    #[error("Migration batch limit too large: {0} (max 50)")]
    SanguoMigrationLimitTooLarge(u32),

    /// 抽卡保底触发但卡池无传说模板
    #[error("Pity triggered but no legend templates available")]
    SanguoPityNoLegend,
}
