use cosmwasm_std::{
    Addr, Binary, Deps, DepsMut, Env, MessageInfo, Response, StdError, StdResult, Uint128,
};

use crate::error::ContractError;
use crate::msg::{
    DailyStatsResponse, GameDailyLimit, GameEngineConfigResponse, GameEntry, GameLimitResponse,
    ListGamesResponse, RoundStatusResponse,
};
use crate::state::{
    BALANCES, DAILY_TRACKING, GAME_LIMITS, GAME_ENGINE_CONFIGS, PRC20_WHITELIST,
    RoundStatus, USED_ROUNDS, ROUND_DEADLINE, round_timeout_secs,
    day_key, empty_addr, GameEngine, GameEngineConfig, GameEngineParams,
};
use crate::contract::update_user_totals;
use crate::admin::config::{effective_burn_rate, effective_treasury_share};

// 🟢 新模块：游戏引擎实现
pub mod engines;

/// 🟢 玩家认证方式：交易发送方 `info.sender`
///
/// 本模板不实现会话密钥 / 无感签名机制——所有玩家身份一律取自
/// `MessageInfo.sender`，即由链上交易签名本身背书，因此不存在
/// 会话公钥登记、签名原文拼接、nonce 自增与重放窗口等概念。
/// 防重放改由 `round_id` 唯一性检查（`USED_ROUNDS`）承担。

// ========================================================================
// 🟢 核心变化：Deduct —— 通用扣款（游戏开始时前端调用）
//
// 流程：
//   1. 取交易发送方 info.sender 作为玩家
//   2. 检查 game_id 的 GameLimit（是否启用、下注范围、限额）
//   3. 检查 round_id 是否已用（防重放）
//   4. 检查每日限额（DAILY_TRACKING）
//   5. 扣玩家 BALANCES[player][token] -= bet
//   6. 记录 USED_ROUNDS[key] = Deducted{ bet }
//   7. 更新 DAILY_TRACKING + 1 局
// ========================================================================

#[allow(dead_code)] // 🟢 全盘审计：路由已从 ExecuteMsg 摘除（Deduct/Settle 变体已删），仅存档备查
pub fn deduct(
    mut deps: DepsMut,
    env: Env,
    info: MessageInfo,
    game_id: String,
    round_id: String,
    token: Option<String>,
    amount: Uint128,
) -> Result<Response, ContractError> {
    // 0. 基本校验
    if amount.is_zero() {
        return Err(ContractError::InvalidAmount);
    }
    if round_id.is_empty() {
        return Err(ContractError::InvalidAmount);
    }

    // 1. 玩家身份 = 交易发送方
    let player = info.sender;

    // 2. 加载游戏限额
    let limit = GAME_LIMITS
        .may_load(deps.storage, &game_id)?
        .ok_or_else(|| ContractError::GameNotFound(game_id.clone()))?;

    if !limit.enabled {
        return Err(ContractError::GameDisabled(game_id.clone()));
    }

    // 3. 下注范围校验
    if amount < limit.min_bet {
        return Err(ContractError::BetTooLow {
            bet: amount.to_string(),
            min: limit.min_bet.to_string(),
        });
    }
    if amount > limit.max_bet {
        return Err(ContractError::BetTooHigh {
            bet: amount.to_string(),
            max: limit.max_bet.to_string(),
        });
    }

    // 4. round_id 去重（防重放）
    let round_key = make_round_key(&player, &game_id, &round_id);
    if USED_ROUNDS.has(deps.storage, &round_key) {
        let existing = USED_ROUNDS.load(deps.storage, &round_key)?;
        let status_str = match &existing {
            RoundStatus::Deducted { .. } => "deducted".to_string(),
            RoundStatus::Settled { .. } => "settled".to_string(),
        };
        return Err(ContractError::RoundAlreadyUsed {
            round_id: round_id.clone(),
            status: status_str,
        });
    }

    // 5. 构建 token addr
    let token_addr = if let Some(ref t) = token {
        if t == "upaxi" {
            empty_addr()
        } else {
            let addr = deps.api.addr_validate(t)?;
            // 🟢 修复：PRC-20 必须在白名单内
            if !PRC20_WHITELIST.has(deps.storage, &addr) {
                return Err(ContractError::ContractNotWhitelisted);
            }
            addr
        }
    } else {
        empty_addr()
    };

    // 6. 每日限额检查（DAILY_TRACKING）
    let day = day_key(env.block.time.seconds());
    let daily_key = format!("{}:{}", player, day);
    let mut daily = DAILY_TRACKING
        .may_load(deps.storage, &daily_key)?
        .unwrap_or_default();

    // 累计下注额 + amount 不能超过 max_daily_bet
    let new_total_bet = daily.total_bet.checked_add(amount).map_err(|_| ContractError::Overflow)?;
    if new_total_bet > limit.max_daily_bet {
        return Err(ContractError::GameDailyBetExceeded {
            used: new_total_bet.to_string(),
            limit: limit.max_daily_bet.to_string(),
        });
    }
    // 累计局数 + 1 不能超过 max_daily_rounds
    if daily.rounds_played + 1 > limit.max_daily_rounds {
        return Err(ContractError::GameDailyRoundsExceeded {
            played: daily.rounds_played + 1,
            limit: limit.max_daily_rounds,
        });
    }

    // 7. 扣余额
    let bal_key = (&player, &token_addr);
    let mut balance = BALANCES.may_load(deps.storage, bal_key)?.unwrap_or(Uint128::zero());
    if balance < amount {
        return Err(ContractError::InsufficientBalance);
    }
    balance = balance.checked_sub(amount).map_err(|_| ContractError::Overflow)?;
    BALANCES.save(deps.storage, bal_key, &balance)?;

    // USER_TOTALS 同步扣
    update_user_totals(deps.branch(), &token_addr, amount, false)?;

    // 8. 记录 round 状态（🟢 修复：存 token_addr，Settle 时沿用）
    USED_ROUNDS.save(deps.storage, &round_key, &RoundStatus::Deducted {
        bet: amount,
        token_addr: token_addr.to_string(),
    })?;

    // 9. 更新 DAILY_TRACKING
    // 🟢 修复：rounds_played 在 Deduct 时就 +1（防止攻击：连续 Deduct 不 Settle）
    daily.total_bet = new_total_bet;
    daily.rounds_played += 1;
    DAILY_TRACKING.save(deps.storage, &daily_key, &daily)?;

    // 10. 🟢 审计新增：记录本局的结算截止时间
    // Deduct 已经把钱从玩家余额里扣掉了，如果玩家一直不 Settle，
    // 这笔钱既不在任何人账上、也没进庄家准备金，会永久悬空在合约里。
    // 有了 deadline，管理员可以在超时后用 ForceExpireRound 强制入账。
    // 🟢 第二轮改进：超时时长改为可配置（SetRoundTimeout），不再写死 1 小时。
    let timeout = round_timeout_secs(deps.storage);
    let deadline = env
        .block
        .time
        .seconds()
        .saturating_add(timeout);
    ROUND_DEADLINE.save(deps.storage, &round_key, &deadline)?;

    Ok(Response::new()
        .add_attribute("action", "deduct")
        .add_attribute("game_id", game_id)
        .add_attribute("round_id", round_id)
        .add_attribute("player", player)
        .add_attribute("bet", amount)
        .add_attribute("token", if token_addr == empty_addr() { "upaxi".to_string() } else { token_addr.to_string() }))
}

// ========================================================================
// 🟢 核心变化：Settle —— 通用结算（游戏结束时前端调用）
//
// 流程：
//   1. 取交易发送方 info.sender 作为玩家
//   2. 检查 round_id 存在且状态为 Deducted
//   3. 检查 payout 合法性（won=true 时 payout ≥ bet，且不超倍数上限）
//   4. 赢局：BALANCES += payout，更新 HOUSE_BANKROLL 和三分
//   5. 输局：bet 入 house_edge，做三分（burn + treasury + house_remain）
//   6. 更新 DAILY_TRACKING（rounds_played + 1）
//   7. 标记 round = Settled
// ========================================================================

#[allow(dead_code)] // 🟢 全盘审计：路由已从 ExecuteMsg 摘除（Deduct/Settle 变体已删），仅存档备查
pub fn settle(
    mut deps: DepsMut,
    env: Env,
    info: MessageInfo,
    game_id: String,
    round_id: String,
    won: bool,
    payout: Uint128,
) -> Result<Response, ContractError> {
    // 1. 玩家身份 = 交易发送方
    let player = info.sender;

    // 🟢 修复：变量提前声明，下面从 RoundStatus 中读取
    let mut token_addr_str = String::new();

    // 2. 加载游戏限额
    let limit = GAME_LIMITS
        .may_load(deps.storage, &game_id)?
        .ok_or_else(|| ContractError::GameNotFound(game_id.clone()))?;

    if !limit.enabled {
        return Err(ContractError::GameDisabled(game_id.clone()));
    }

    // 3. 检查 round 状态（🟢 修复：从状态中同时读 token_addr）
    let round_key = make_round_key(&player, &game_id, &round_id);
    let round_status = USED_ROUNDS.may_load(deps.storage, &round_key)?
        .ok_or_else(|| ContractError::RoundNotFoundOrNotDeducted { round_id: round_id.clone() })?;

    let bet = match &round_status {
        RoundStatus::Deducted { bet, token_addr } => {
            // 🟢 修复：从 Deduct 状态里读 token_addr
            token_addr_str = token_addr.clone();
            *bet
        }
        RoundStatus::Settled { bet, .. } => {
            return Err(ContractError::RoundAlreadyUsed {
                round_id: round_id.clone(),
                status: "settled".to_string(),
            });
        }
    };

    // 🟢 修复：解析 token_addr（从 Deduct 状态中读取）
    let token_addr = if token_addr_str.is_empty() {
        empty_addr()
    } else {
        deps.api.addr_validate(&token_addr_str)?
    };

    // 4. payout 合法性校验
    if won {
        if payout < bet {
            return Err(ContractError::PayoutLessThanBet);
        }
        let max_payout = bet
            .checked_mul(Uint128::from(limit.max_payout_multiplier))
            .map_err(|_| ContractError::Overflow)?
            .checked_div(Uint128::from(100u128))  // multiplier 如 300 表示 3.00x
            .unwrap_or(Uint128::MAX);
        if payout > max_payout {
            return Err(ContractError::PayoutTooHigh {
                payout: payout.to_string(),
                bet: bet.to_string(),
                max_mult: limit.max_payout_multiplier,
            });
        }
    }

    // 4. payout 合法性校验（已移到上面）

    // 5. 每日局数检查 —— Deduct 时已经 +1，这里只做结算，不再重复加
    // 但我们还是需要确保累计没超（双重检查）
    let day = day_key(env.block.time.seconds());
    let daily_key = format!("{}:{}", player, day);
    let mut daily = DAILY_TRACKING
        .may_load(deps.storage, &daily_key)?
        .unwrap_or_default();

    // 双重安全：如果 rounds_played 还没 Deduct 加过（异常情况），这里补一下
    // 但正常 Deduct 已经 +1 了，所以这里只检查不再超上限
    if daily.rounds_played > limit.max_daily_rounds {
        return Err(ContractError::GameDailyRoundsExceeded {
            played: daily.rounds_played,
            limit: limit.max_daily_rounds,
        });
    }

    // 7. 结算逻辑
    let mut resp = Response::new()
        .add_attribute("action", "settle")
        .add_attribute("game_id", game_id.clone())
        .add_attribute("round_id", round_id.clone())
        .add_attribute("player", player.clone())
        .add_attribute("bet", bet)
        .add_attribute("won", won.to_string())
        .add_attribute("payout", payout);

    if won {
        // 赢局：加钱到玩家余额
        let bal_key = (&player, &token_addr);
        let mut balance = BALANCES.may_load(deps.storage, bal_key)?.unwrap_or(Uint128::zero());
        balance = balance.checked_add(payout).map_err(|_| ContractError::Overflow)?;
        BALANCES.save(deps.storage, bal_key, &balance)?;

        update_user_totals(deps.branch(), &token_addr, payout, true)?;

        // house_edge = payout - bet（这部分从 HOUSE_BANKROLL 出）
        let profit = payout.checked_sub(bet).map_err(|_| ContractError::Overflow)?;
        let mut hb = crate::state::HOUSE_BANKROLL
            .may_load(deps.storage, &token_addr)?
            .unwrap_or(Uint128::zero());
        // 🟢 修复：庄家资金不足时报错
        if hb < profit {
            return Err(ContractError::InsufficientBankrollForProfit {
                have: hb.to_string(),
                need: profit.to_string(),
            });
        }
        hb = hb.checked_sub(profit).map_err(|_| ContractError::Overflow)?;
        crate::state::HOUSE_BANKROLL.save(deps.storage, &token_addr, &hb)?;

        // 🟢 第二轮新增：赢局放款后做偿付能力兜底检查
        // 原实现里 assert_solvency 定义了却从未被调用（死代码）。
        // 赢局是唯一「合约往外掏钱」的路径，一旦账目出现窟窿，
        // 先赢的人拿走了后面所有人的存款。这里在放款后立即验一遍恒等式，
        // 不通过就整笔交易回滚 —— 宁可这一局失败，也不能让窟窿继续扩大。
        crate::contract::assert_solvency(deps.as_ref(), &env.contract.address, &token_addr, true)?;

        resp = resp.add_attribute("result", "win");
        resp = resp.add_attribute("profit_paid", profit);
    } else {
        // 输局：bet 全部入 house_edge，做三分
        let split = crate::contract::calc_house_edge_split(
            &mut deps, &env, &game_id, &token_addr, bet,
        )?;

        resp = resp.add_attribute("result", "loss");
        if let Some(m) = split.burn_msg {
            resp = resp.add_message(m);
        }
        if let Some(m) = split.treasury_msg {
            resp = resp.add_message(m);
        }
        if !split.burn_amount.is_zero() {
            resp = resp.add_attribute("burn_amount", split.burn_amount);
        }
        if !split.treasury_amount.is_zero() {
            resp = resp.add_attribute("treasury_amount", split.treasury_amount);
        }
    }

    // 8. 标记 round = Settled
    USED_ROUNDS.save(
        deps.storage,
        &round_key,
        &RoundStatus::Settled {
            bet,
            payout: if won { payout } else { Uint128::zero() },
            token_addr: token_addr.to_string(),
        },
    )?;

    // 🟢 审计新增：已结算，清除超时记录
    ROUND_DEADLINE.remove(deps.storage, &round_key);

    Ok(resp)
}

// ========================================================================
// 辅助
// ========================================================================

fn make_round_key(player: &Addr, game_id: &str, round_id: &str) -> String {
    format!("{}:{}:{}", player, game_id, round_id)
}

// ========================================================================
// 查询
// ========================================================================

pub fn query_game_limit(deps: Deps, game_id: String) -> StdResult<GameLimitResponse> {
    let limit = GAME_LIMITS.may_load(deps.storage, &game_id)?;
    Ok(GameLimitResponse { limit })
}

/// 🟢 第二轮改进：一次性返回引擎类型 + 每日限额
///
/// 原实现只给 5 个字段，前端拿到列表后：
///   1. 不知道这个游戏是猜数字还是骰子，渲染不出下注 UI → 得再调 N 次 GameEngineConfigQuery
///   2. 拿不到 max_daily_bet / max_daily_rounds → 做不了「今日剩余额度」提示
/// 这里全部补齐，前端一次查询就能渲染完整大厅。
pub fn query_list_games(deps: Deps) -> StdResult<ListGamesResponse> {
    let mut games: Vec<GameEntry> = Vec::new();
    for item in GAME_LIMITS
        .range(deps.storage, None, None, cosmwasm_std::Order::Ascending)
    {
        let (game_id, limit) = item?;
        // 没配引擎的游戏（纯 Deduct/Settle 两段式玩法）返回 None
        let engine = GAME_ENGINE_CONFIGS
            .may_load(deps.storage, &game_id)?
            .map(|cfg| cfg.engine);
        games.push(GameEntry {
            game_id: game_id.to_string(),
            enabled: limit.enabled,
            min_bet: limit.min_bet,
            max_bet: limit.max_bet,
            max_payout_multiplier: limit.max_payout_multiplier,
            engine,
            max_daily_bet: limit.max_daily_bet,
            max_daily_rounds: limit.max_daily_rounds,
        });
    }
    Ok(ListGamesResponse { games })
}

pub fn query_round_status(deps: Deps, player: String, game_id: String, round_id: String) -> StdResult<RoundStatusResponse> {
    let player_addr = deps.api.addr_validate(&player)?;
    let key = make_round_key(&player_addr, &game_id, &round_id);
    let status = USED_ROUNDS.may_load(deps.storage, &key)?;
    let resp = match status {
        Some(RoundStatus::Deducted { bet, .. }) => RoundStatusResponse {
            status: Some("deducted".to_string()),
            bet: Some(bet),
            payout: None,
        },
        Some(RoundStatus::Settled { bet, payout, .. }) => RoundStatusResponse {
            status: Some("settled".to_string()),
            bet: Some(bet),
            payout: Some(payout),
        },
        None => RoundStatusResponse {
            status: None,
            bet: None,
            payout: None,
        },
    };
    Ok(resp)
}

/// 🟢 第二轮改进：支持 game_id，并一次性返回所有游戏的限额
///
/// 原实现有两个问题：
///   1. 接口没有 game_id 参数，只能「取第一个启用游戏的限额」当参考值 ——
///      而这个"第一个"完全取决于存储迭代顺序（按 game_id 字典序），
///      管理员加一个新游戏就可能让所有玩家的额度提示突然变化，纯属玄学。
///   2. 只返回一个参考值，前端想显示「各游戏剩余额度」还得再调 N 次 GameLimit。
///
/// 现在：传 game_id 就返回该游戏的限额；不传则返回所有启用游戏里**最严格**的
/// 那组（取 min），保证拿到的数字在任何游戏下都不会超限；同时用 game_limits
/// 把全量限额一并返回。
pub fn query_daily_stats(
    deps: Deps,
    env: Env,
    player: String,
    date: Option<u64>,
    game_id: Option<String>,
) -> StdResult<DailyStatsResponse> {
    let player_addr = deps.api.addr_validate(&player)?;
    // 🟢 修复：用真实 env.block.time，不再用 Env::default()（那会返回 1970-01-01）
    let day = date.unwrap_or_else(|| day_key(env.block.time.seconds()));
    let daily_key = format!("{}:{}", player_addr, day);
    let daily = DAILY_TRACKING.may_load(deps.storage, &daily_key)?.unwrap_or_default();

    // 1. 收集所有启用游戏的限额
    let mut game_limits: Vec<GameDailyLimit> = Vec::new();
    for item in GAME_LIMITS
        .range(deps.storage, None, None, cosmwasm_std::Order::Ascending)
    {
        let (gid, limit) = item?;
        if !limit.enabled {
            continue;
        }
        game_limits.push(GameDailyLimit {
            game_id: gid,
            enabled: limit.enabled,
            max_daily_bet: limit.max_daily_bet,
            max_daily_rounds: limit.max_daily_rounds,
        });
    }

    // 2. 兜底值（一个启用游戏都没配时）
    let mut max_daily_bet = Uint128::from(crate::state::DEFAULT_MAX_DAILY_BET);
    let mut max_daily_rounds: u64 = crate::state::DEFAULT_MAX_DAILY_ROUNDS;
    for gl in &game_limits {
        max_daily_bet = max_daily_bet.min(gl.max_daily_bet);
        max_daily_rounds = max_daily_rounds.min(gl.max_daily_rounds);
    }

    // 3. 指定了 game_id 就覆盖成该游戏的限额（不要求 enabled，方便前端查已下线的游戏）
    if let Some(gid) = &game_id {
        if let Some(l) = GAME_LIMITS.may_load(deps.storage, gid)? {
            max_daily_bet = l.max_daily_bet;
            max_daily_rounds = l.max_daily_rounds;
        }
    }

    Ok(DailyStatsResponse {
        total_bet: daily.total_bet,
        rounds_played: daily.rounds_played,
        max_daily_bet,
        max_daily_rounds,
        game_id,
        game_limits,
    })
}

// ========================================================================
// 🟢 管理员：设置游戏引擎配置（add_game_id 绑定 engine + params）
// ========================================================================

/// 🟢 审计新增：引擎配置合法性校验
///
/// 原实现对引擎参数**零校验**，管理员可以直接写入致命配置：
///   - Guess 的 min > max  → `max - min + 1` 在 u64 上下溢（debug panic / release 得到天文数字）
///   - Roulette 的 slots = 0 → `random_u64(bound=0)` 恒返回 0，玩家押 0 号槽必胜
///   - Roulette 的 payouts 长度与 slots 不一致 → 派彩倍数静默取 0
///   - Dice 的 sides = 0 / dice_count = 0 → 除零、骰子点数恒为 1
///   - Card 的 deck_count = 0 → 牌堆为空，random_u64(bound=0) 恒发同一张牌
///   - engine 与 params 类型不匹配 → 运行时统一报 EngineInputMismatch，问题被推迟到玩家下注时才暴露
/// 这些都要在写入配置时就拦掉。
pub fn validate_engine_config(config: &GameEngineConfig) -> Result<(), ContractError> {
    // 1) engine 与 params 必须成对匹配
    let matched = matches!(
        (&config.engine, &config.params),
        (GameEngine::Guess, GameEngineParams::Guess { .. })
            | (GameEngine::Roulette, GameEngineParams::Roulette { .. })
            | (GameEngine::Dice, GameEngineParams::Dice { .. })
            | (GameEngine::Card, GameEngineParams::Card { .. })
            | (GameEngine::Score, GameEngineParams::Score { .. })
    );
    if !matched {
        return Err(ContractError::InvalidEngineParams(
            "engine 与 params 类型不匹配".to_string(),
        ));
    }

    // 2) Score 引擎因前端可任意刷分已被永久禁用，不允许再配置
    if matches!(config.engine, GameEngine::Score) {
        return Err(ContractError::ScoreEngineDisabled);
    }

    // 3) 各引擎的参数域校验
    match &config.params {
        GameEngineParams::Guess { min, max, multiplier } => {
            if *max <= *min {
                return Err(ContractError::InvalidEngineParams(
                    "Guess: max 必须大于 min".to_string(),
                ));
            }
            if *multiplier == 0 || *multiplier > 10000 {
                return Err(ContractError::InvalidEngineParams(
                    "Guess: multiplier 必须在 1..10000 之间".to_string(),
                ));
            }
        }
        GameEngineParams::Roulette { slots, payouts } => {
            if *slots < 2 {
                return Err(ContractError::InvalidEngineParams(
                    "Roulette: slots 必须 >= 2".to_string(),
                ));
            }
            if payouts.len() != *slots as usize {
                return Err(ContractError::InvalidEngineParams(
                    "Roulette: payouts 长度必须等于 slots".to_string(),
                ));
            }
            // 至少留一个空槽，否则玩家押中就必赢
            if !payouts.iter().any(|p| *p == 0) {
                return Err(ContractError::InvalidEngineParams(
                    "Roulette: payouts 至少要有一个 0（否则必赢）".to_string(),
                ));
            }
            if payouts.iter().any(|p| *p > 10000) {
                return Err(ContractError::InvalidEngineParams(
                    "Roulette: 单个槽位倍数不能超过 10000".to_string(),
                ));
            }
        }
        GameEngineParams::Dice {
            dice_count,
            sides,
            bet_types,
        } => {
            if *dice_count == 0 || *dice_count > 10 {
                return Err(ContractError::InvalidEngineParams(
                    "Dice: dice_count 必须在 1..10 之间".to_string(),
                ));
            }
            if *sides < 2 || *sides > 100 {
                return Err(ContractError::InvalidEngineParams(
                    "Dice: sides 必须在 2..100 之间".to_string(),
                ));
            }
            if bet_types.is_empty() {
                return Err(ContractError::InvalidEngineParams(
                    "Dice: bet_types 不能为空".to_string(),
                ));
            }
            for bt in bet_types {
                if !matches!(bt.as_str(), "big" | "small" | "odd" | "even" | "specific") {
                    return Err(ContractError::InvalidDiceBetType { bet_type: bt.clone() });
                }
            }
        }
        GameEngineParams::Card {
            deck_count,
            min_cards,
            max_cards,
            multiplier_by_hand,
        } => {
            if *deck_count == 0 || *deck_count > 8 {
                return Err(ContractError::InvalidEngineParams(
                    "Card: deck_count 必须在 1..8 之间".to_string(),
                ));
            }
            if *min_cards == 0 || *min_cards > *max_cards {
                return Err(ContractError::InvalidEngineParams(
                    "Card: min_cards/max_cards 非法".to_string(),
                ));
            }
            if *max_cards > 10 {
                return Err(ContractError::InvalidEngineParams(
                    "Card: max_cards 不能超过 10".to_string(),
                ));
            }
            // vs_dealer 模式下要发两倍牌，必须留够牌堆
            if (*max_cards as u128) * 2 > (*deck_count as u128) * 52 {
                return Err(ContractError::InvalidEngineParams(
                    "Card: max_cards 超过牌堆容量（vs_dealer 需发两倍牌）".to_string(),
                ));
            }
            if multiplier_by_hand.len() != 10 {
                return Err(ContractError::InvalidEngineParams(
                    "Card: multiplier_by_hand 必须恰好 10 项（对应 hand_rank 0..9）".to_string(),
                ));
            }
        }
        GameEngineParams::Score { .. } => {
            return Err(ContractError::ScoreEngineDisabled);
        }
    }

    Ok(())
}

pub fn set_game_engine_config(
    deps: DepsMut,
    info: MessageInfo,
    game_id: String,
    config: GameEngineConfig,
) -> Result<Response, ContractError> {
    // 🟢 管理员权限校验（用 is_admin 高效检查）
    // 🟢 修复：必须检查返回值 bool，不能只用 ?（Ok(false) 会被忽略）
    if !crate::admin::config::is_admin(deps.as_ref(), &info.sender)? {
        return Err(ContractError::Unauthorized);
    }

    // 🟢 审计新增：写入前校验参数
    validate_engine_config(&config)?;

    GAME_ENGINE_CONFIGS.save(deps.storage, &game_id, &config)?;

    Ok(Response::new()
        .add_attribute("action", "set_game_engine_config")
        .add_attribute("game_id", game_id)
        .add_attribute("engine", format!("{:?}", config.engine)))
}

pub fn remove_game_engine_config(
    deps: DepsMut,
    info: MessageInfo,
    game_id: String,
) -> Result<Response, ContractError> {
    // 🟢 修复：必须检查返回值 bool，不能只用 ?（Ok(false) 会被忽略）
    if !crate::admin::config::is_admin(deps.as_ref(), &info.sender)? {
        return Err(ContractError::Unauthorized);
    }

    GAME_ENGINE_CONFIGS.remove(deps.storage, &game_id);

    Ok(Response::new()
        .add_attribute("action", "remove_game_engine_config")
        .add_attribute("game_id", game_id))
}

/// 🟢 一键加游戏（同时设限额 + 引擎配置）
pub fn add_game(
    deps: DepsMut,
    info: MessageInfo,
    game_id: String,
    limit: crate::state::GameLimit,
    engine_config: GameEngineConfig,
) -> Result<Response, ContractError> {
    // 🟢 管理员权限校验（用 is_admin 高效检查）
    // 🟢 修复：必须检查返回值 bool，不能只用 ?（Ok(false) 会被忽略）
    if !crate::admin::config::is_admin(deps.as_ref(), &info.sender)? {
        return Err(ContractError::Unauthorized);
    }

    // 🟢 审计新增：AddGame 原先完全不校验，可写入非法限额/非法引擎参数（绕过 SetGameLimit 的校验）
    crate::admin::config::validate_game_limit(&limit)?;
    validate_engine_config(&engine_config)?;

    // 两个都存
    GAME_LIMITS.save(deps.storage, &game_id, &limit)?;
    GAME_ENGINE_CONFIGS.save(deps.storage, &game_id, &engine_config)?;

    Ok(Response::new()
        .add_attribute("action", "add_game")
        .add_attribute("game_id", game_id)
        .add_attribute("engine", format!("{:?}", engine_config.engine))
        .add_attribute("min_bet", limit.min_bet.to_string())
        .add_attribute("max_bet", limit.max_bet.to_string())
        .add_attribute("max_payout_multiplier", limit.max_payout_multiplier.to_string()))
}

// ========================================================================
// 🟢 引擎内 Play —— 一步式：合约生成随机数 + 判定 + 结算
//
// 流程：
//   1. 取交易发送方 info.sender 作为玩家
//   2. 查 GAME_ENGINE_CONFIGS 确认引擎已绑定
//   3. 查 GAME_LIMITS 确认限额 + 下注范围
//   4. round_id 去重（防重放）
//   5. 每日限额检查
//   6. 调引擎 play_dispatch 生成结果
//   7. 扣款 + 结算（win: bet + profit → BALANCE / loss: bet → house_edge）
//   8. 记录 USED_ROUNDS + DAILY_TRACKING
// ========================================================================

pub fn play(
    mut deps: DepsMut,
    env: Env,
    info: MessageInfo,
    game_id: String,
    round_id: String,
    token: Option<String>,
    bet: Uint128,
    guess_input: Option<crate::state::GuessInput>,
    roulette_input: Option<crate::state::RouletteInput>,
    dice_input: Option<crate::state::DiceInput>,
    card_input: Option<crate::state::CardInput>,
    score_input: Option<crate::state::ScoreInput>,
) -> Result<Response, ContractError> {
    // 🔴 C1 硬化（V2 0.2.0，裁定 M2）：合规总闸存储键 COMPLIANCE_MODE 已删除。
    //    通用博弈引擎（Play）自 V0.1.1 起即被总闸禁用；V2 合规是唯一路径，
    //    该入口改为代码级永久下线——不依赖任何存储开关（删键后不存在被误放开的可能）。
    //    如需恢复博弈玩法，必须发布新合约版本并走完整合规评审。
    let compliance_mode: Option<bool> = None; // M2 删键占位：unwrap_or(true) 恒拒
    if compliance_mode.unwrap_or(true) {
        return Err(ContractError::ComplianceDisabled);
    }
    if bet.is_zero() {
        return Err(ContractError::InvalidAmount);
    }
    if round_id.is_empty() {
        return Err(ContractError::InvalidAmount);
    }

    // 1. 玩家身份 = 交易发送方
    let player = info.sender;

    // 2. 加载引擎配置
    let engine_config = GAME_ENGINE_CONFIGS
        .may_load(deps.storage, &game_id)?
        .ok_or_else(|| ContractError::GameEngineNotConfigured(game_id.clone()))?;

    // 3. 加载游戏限额
    let limit = GAME_LIMITS
        .may_load(deps.storage, &game_id)?
        .ok_or_else(|| ContractError::GameNotFound(game_id.clone()))?;

    if !limit.enabled {
        return Err(ContractError::GameDisabled(game_id.clone()));
    }

    // 4. 下注范围校验
    if bet < limit.min_bet {
        return Err(ContractError::BetTooLow {
            bet: bet.to_string(),
            min: limit.min_bet.to_string(),
        });
    }
    if bet > limit.max_bet {
        return Err(ContractError::BetTooHigh {
            bet: bet.to_string(),
            max: limit.max_bet.to_string(),
        });
    }

    // 5. round_id 去重（防重放）
    let round_key = make_round_key(&player, &game_id, &round_id);
    if USED_ROUNDS.has(deps.storage, &round_key) {
        let existing = USED_ROUNDS.load(deps.storage, &round_key)?;
        let status_str = match &existing {
            RoundStatus::Deducted { .. } => "deducted".to_string(),
            RoundStatus::Settled { .. } => "settled".to_string(),
        };
        return Err(ContractError::RoundAlreadyUsed {
            round_id: round_id.clone(),
            status: status_str,
        });
    }

    // 6. 构建 token addr
    let token_addr = if let Some(ref t) = token {
        if t == "upaxi" {
            empty_addr()
        } else {
            let addr = deps.api.addr_validate(t)?;
            // 🟢 修复：PRC-20 必须在白名单内
            if !PRC20_WHITELIST.has(deps.storage, &addr) {
                return Err(ContractError::ContractNotWhitelisted);
            }
            addr
        }
    } else {
        empty_addr()
    };

    // 7. 每日限额检查
    let day = day_key(env.block.time.seconds());
    let daily_key = format!("{}:{}", player, day);
    let mut daily = DAILY_TRACKING
        .may_load(deps.storage, &daily_key)?
        .unwrap_or_default();

    let new_total_bet = daily.total_bet.checked_add(bet).map_err(|_| ContractError::Overflow)?;
    if new_total_bet > limit.max_daily_bet {
        return Err(ContractError::GameDailyBetExceeded {
            used: new_total_bet.to_string(),
            limit: limit.max_daily_bet.to_string(),
        });
    }
    if daily.rounds_played + 1 > limit.max_daily_rounds {
        return Err(ContractError::GameDailyRoundsExceeded {
            played: daily.rounds_played + 1,
            limit: limit.max_daily_rounds,
        });
    }

    // 8. 扣余额
    let bal_key = (&player, &token_addr);
    let mut balance = BALANCES.may_load(deps.storage, bal_key)?.unwrap_or(Uint128::zero());
    if balance < bet {
        return Err(ContractError::InsufficientBalance);
    }
    balance = balance.checked_sub(bet).map_err(|_| ContractError::Overflow)?;
    BALANCES.save(deps.storage, bal_key, &balance)?;

    update_user_totals(deps.branch(), &token_addr, bet, false)?;

    // 9. 🟢 调引擎 play_dispatch —— 合约生成随机数 + 判定结果
    //    seed 混合了区块内交易索引等链上数据（详见 engines::build_seed 注释）
    let result = engines::play_dispatch(
        &env,
        player.as_str(),
        &round_id,
        bet,
        &engine_config,
        guess_input,
        roulette_input,
        dice_input,
        card_input,
        score_input,
    )?;

    // 10. 🟢 修复：计算最终 payout
    // 只要 result.won=true 就走赢局逻辑（包括平局 profit=0 的情况，退回本金）
    let (won, payout, payout_capped) = if result.won {
        // profit 是纯盈利；profit 为 0 时 payout = bet（平局退回本金）
        let raw_payout = bet.checked_add(result.profit).map_err(|_| ContractError::Overflow)?;

        // 再检查 payout 不超过限额倍数（双重安全）
        // 🟢 M1 审计修复（2026-10-10）：截断不再静默 —— payout_capped=true 会写进
        //    交易 attribute，链下可监控「引擎 max_mult 配置 > GameLimit 倍数」的错配。
        let max_payout = bet
            .checked_mul(Uint128::from(limit.max_payout_multiplier))
            .map_err(|_| ContractError::Overflow)?
            .checked_div(Uint128::from(100u128))
            .unwrap_or(Uint128::MAX);
        let capped = raw_payout > max_payout;
        let final_payout = if capped { max_payout } else { raw_payout };
        (true, final_payout, capped)
    } else {
        (false, Uint128::zero(), false)
    };

    // 11. 结算
    let mut resp = Response::new()
        .add_attribute("action", "play")
        .add_attribute("game_id", game_id.clone())
        .add_attribute("round_id", round_id.clone())
        .add_attribute("player", player.clone())
        .add_attribute("bet", bet)
        .add_attribute("won", won.to_string())
        .add_attribute("payout", payout)
        .add_attribute("engine_result", &result.engine_result);
    if payout_capped {
        resp = resp.add_attribute("payout_capped", "true");
    }

    if won {
        // 赢局：加钱到玩家余额
        let bal_key2 = (&player, &token_addr);
        let mut bal = BALANCES.may_load(deps.storage, bal_key2)?.unwrap_or(Uint128::zero());
        bal = bal.checked_add(payout).map_err(|_| ContractError::Overflow)?;
        BALANCES.save(deps.storage, bal_key2, &bal)?;

        update_user_totals(deps.branch(), &token_addr, payout, true)?;

        // 🟢 修复：庄家资金不足时报错，不再静默减到 0 掩盖问题
        let profit_actual = payout.checked_sub(bet).map_err(|_| ContractError::Overflow)?;
        let mut hb = crate::state::HOUSE_BANKROLL
            .may_load(deps.storage, &token_addr)?
            .unwrap_or(Uint128::zero());
        if hb < profit_actual {
            return Err(ContractError::InsufficientBankrollForProfit {
                have: hb.to_string(),
                need: profit_actual.to_string(),
            });
        }
        hb = hb.checked_sub(profit_actual).map_err(|_| ContractError::Overflow)?;
        crate::state::HOUSE_BANKROLL.save(deps.storage, &token_addr, &hb)?;

        // 🟢 第二轮新增：同上，Play 赢局放款后做偿付能力兜底检查
        crate::contract::assert_solvency(deps.as_ref(), &env.contract.address, &token_addr, true)?;

        resp = resp.add_attribute("result", "win");
        resp = resp.add_attribute("profit_paid", profit_actual);
    } else {
        // 输局：bet 入 house_edge，做三分
        let split = crate::contract::calc_house_edge_split(
            &mut deps, &env, &game_id, &token_addr, bet,
        )?;

        resp = resp.add_attribute("result", "loss");
        if let Some(m) = split.burn_msg {
            resp = resp.add_message(m);
        }
        if let Some(m) = split.treasury_msg {
            resp = resp.add_message(m);
        }
        if !split.burn_amount.is_zero() {
            resp = resp.add_attribute("burn_amount", split.burn_amount);
        }
        if !split.treasury_amount.is_zero() {
            resp = resp.add_attribute("treasury_amount", split.treasury_amount);
        }
    }

    // 12. 记录 round + 更新 DAILY_TRACKING
    USED_ROUNDS.save(
        deps.storage,
        &round_key,
        &RoundStatus::Settled {
            bet,
            payout,
            token_addr: token_addr.to_string(),
        },
    )?;

    daily.total_bet = new_total_bet;
    daily.rounds_played += 1;
    DAILY_TRACKING.save(deps.storage, &daily_key, &daily)?;

    Ok(resp)
}

// ========================================================================
// 🟢 审计新增：ForceExpireRound —— Deduct 超时后管理员强制按「输局」结算
//
// 背景：Deduct 已经把玩家余额扣掉，但没有任何机制强制玩家在合理时间内 Settle。
// 玩家可以 Deduct 之后直接消失（或者前端崩了 / 故意不结算），
// 这笔 bet 就永久卡在「已扣款未结算」状态：
//   - 用户余额里没有
//   - 庄家准备金里没有
//   - 实际代币还躺在合约银行账户里，但账上无人认领
//
// 有了这个接口，超时后管理员可以把该局按输局处理，让资金正常进入三分（burn/treasury/house）。
// 同时也让 USED_ROUNDS 从 Deducted 推进到 Settled，避免堆积脏状态。
// ========================================================================

pub fn force_expire_round(
    mut deps: DepsMut,
    env: Env,
    info: MessageInfo,
    player: String,
    game_id: String,
    round_id: String,
) -> Result<Response, ContractError> {
    if !crate::admin::config::is_admin(deps.as_ref(), &info.sender)? {
        return Err(ContractError::Unauthorized);
    }

    let player_addr = deps.api.addr_validate(&player)?;
    let round_key = make_round_key(&player_addr, &game_id, &round_id);

    // 1. 必须已经过了 deadline
    let deadline = ROUND_DEADLINE
        .may_load(deps.storage, &round_key)?
        .ok_or_else(|| ContractError::RoundNotFoundOrNotDeducted { round_id: round_id.clone() })?;

    let now = env.block.time.seconds();
    if now < deadline {
        return Err(ContractError::RoundNotExpired {
            round_id: round_id.clone(),
            deadline,
        });
    }

    // 2. 必须仍处于 Deducted 状态
    let status = USED_ROUNDS
        .may_load(deps.storage, &round_key)?
        .ok_or_else(|| ContractError::RoundNotFoundOrNotDeducted { round_id: round_id.clone() })?;

    let (bet, token_addr_str) = match &status {
        RoundStatus::Deducted { bet, token_addr } => (*bet, token_addr.clone()),
        RoundStatus::Settled { .. } => {
            return Err(ContractError::RoundAlreadyUsed {
                round_id: round_id.clone(),
                status: "settled".to_string(),
            });
        }
    };

    let token_addr = if token_addr_str.is_empty() {
        empty_addr()
    } else {
        deps.api.addr_validate(&token_addr_str)?
    };

    // 3. 按输局处理：bet 全部计入 house_edge 并做三分
    let split = crate::contract::calc_house_edge_split(
        &mut deps, &env, &game_id, &token_addr, bet,
    )?;

    let mut resp = Response::new()
        .add_attribute("action", "force_expire_round")
        .add_attribute("game_id", game_id)
        .add_attribute("round_id", round_id.clone())
        .add_attribute("player", player_addr.as_str())
        .add_attribute("bet", bet)
        .add_attribute("result", "loss_by_timeout");

    if let Some(m) = split.burn_msg {
        resp = resp.add_message(m);
    }
    if let Some(m) = split.treasury_msg {
        resp = resp.add_message(m);
    }
    if !split.burn_amount.is_zero() {
        resp = resp.add_attribute("burn_amount", split.burn_amount);
    }
    if !split.treasury_amount.is_zero() {
        resp = resp.add_attribute("treasury_amount", split.treasury_amount);
    }

    // 4. 推进状态 + 清除 deadline
    USED_ROUNDS.save(
        deps.storage,
        &round_key,
        &RoundStatus::Settled {
            bet,
            payout: Uint128::zero(),
            token_addr: token_addr.to_string(),
        },
    )?;
    ROUND_DEADLINE.remove(deps.storage, &round_key);

    Ok(resp)
}

// ========================================================================
// 查询：引擎配置
// ========================================================================

pub fn query_game_engine_config(deps: Deps, game_id: String) -> StdResult<GameEngineConfigResponse> {
    let config = GAME_ENGINE_CONFIGS.may_load(deps.storage, &game_id)?;
    Ok(GameEngineConfigResponse { config })
}



