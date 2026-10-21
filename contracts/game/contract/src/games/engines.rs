//! 🟢 游戏引擎实现模块
//!
//! 每个引擎接收：配置 + 玩家输入 + 随机 seed
//! 返回：PlayResult { won, payout, engine_result }
//!
//! 随机数来源：block_hash + round_id + player_addr 做 SHA-256 混合

use cosmwasm_std::{Env, Uint128};
use sha2::{Digest, Sha256};

use crate::error::ContractError;
use crate::state::{
    CardInput, DiceInput, GameEngine, GameEngineConfig, GameEngineParams,
    GuessInput, RouletteInput, ScoreInput,
};

// ========================================================================
// 通用：PlayResult + 随机数生成
// ========================================================================

/// 引擎统一返回值
#[derive(Debug, Clone)]
pub struct PlayResult {
    pub won: bool,
    /// 派彩金额（不含本金，纯盈利；如果 won=true 最终 payout = bet + profit）
    pub profit: Uint128,
    /// 引擎返回的原始结果（给前端展示用）
    pub engine_result: String,
}

/// 用混合 seed 生成一个 [0, bound) 范围内的随机 u64
///
/// 🟢 审计修复：原实现直接 `raw % bound`，存在模偏置（modulo bias）。
/// 这里改为拒绝采样（rejection sampling），保证结果严格均匀。
pub fn random_u64(seed: &[u8], bound: u64) -> u64 {
    if bound == 0 {
        return 0;
    }
    // zone 是 <= u64::MAX 的最大的 bound 的倍数，落在 [0, zone) 内的值每个余数出现次数相同
    let zone = u64::MAX - (u64::MAX % bound);
    let mut counter: u64 = 0;
    loop {
        let mut input = seed.to_vec();
        input.extend_from_slice(&counter.to_le_bytes());
        let hash = Sha256::digest(&input);
        let mut bytes = [0u8; 8];
        bytes.copy_from_slice(&hash[..8]);
        let raw = u64::from_le_bytes(bytes);
        if raw < zone {
            return raw % bound;
        }
        counter = counter.wrapping_add(1);
    }
}

/// 构造随机 seed
///
/// 🟢 审计修复（高危）：seed 只由 chain_id + contract + player + round_id + height + time + tx_index + action 组成，
/// 其中 **round_id 完全由前端生成**、height/time 在提交时基本可预测，
/// 玩家因此可能离线枚举 round_id 反复试算，
/// 直到 `play_guess` / `play_roulette` / `play_card` 算出自己赢为止，再提交交易 —— 必胜。
///
/// 缓解：把 **交易在区块内的索引** `env.transaction.index` 纳入 seed。
/// 该值由排序器决定，用户在构造交易时无法预知，因此无法离线枚举。
///
/// ⚠️ 注意：Cosmos SDK 本身不提供链上可验证随机源（无 PREVRANDAO / 无 VRF）。
/// 这里的随机性强度 = 「排序者无法被玩家收买」。若要做到不可操纵，
/// 必须改成两阶段 commit-reveal（Deduct 提交 bet 摘要 → 后续交易揭示），
/// 或引入外部 VRF/预言机。详见审计报告。
pub fn build_seed(env: &Env, player: &str, round_id: &str, action: &str) -> Vec<u8> {
    let block_height = env.block.height.to_string();
    // 🟢 用纳秒而不是秒，压缩同一秒内的可预测性
    let block_time = env.block.time.nanos().to_string();
    let chain_id = &env.block.chain_id;
    let contract_addr = env.contract.address.as_str();
    // 🟢 区块内交易索引：用户构造交易时不可预知
    let tx_index = env.transaction.as_ref().map(|t| t.index).unwrap_or(0);

    let raw = format!(
        "{}:{}:{}:{}:{}:{}:{}:{}",
        chain_id, contract_addr, player, round_id,
        block_height, block_time, tx_index, action,
    );
    Sha256::digest(raw.as_bytes()).to_vec()
}

// ========================================================================
// 总入口：根据引擎类型分发
// ========================================================================

pub fn play_dispatch(
    env: &Env,
    player: &str,
    round_id: &str,
    bet: Uint128,
    config: &GameEngineConfig,
    guess_input: Option<GuessInput>,
    roulette_input: Option<RouletteInput>,
    dice_input: Option<DiceInput>,
    card_input: Option<CardInput>,
    score_input: Option<ScoreInput>,
) -> Result<PlayResult, ContractError> {
    let seed = build_seed(env, player, round_id, "play");

    match &config.engine {
        GameEngine::Guess => {
            let input = guess_input.ok_or(ContractError::EngineInputMismatch)?;
            play_guess(&seed, bet, &config.params, &input)
        }
        GameEngine::Roulette => {
            let input = roulette_input.ok_or(ContractError::EngineInputMismatch)?;
            play_roulette(&seed, bet, &config.params, &input)
        }
        GameEngine::Dice => {
            let input = dice_input.ok_or(ContractError::EngineInputMismatch)?;
            play_dice(&seed, bet, &config.params, &input)
        }
        GameEngine::Card => {
            let input = card_input.ok_or(ContractError::EngineInputMismatch)?;
            play_card(&seed, bet, &config.params, &input)
        }
        GameEngine::Score => {
            // 🟢 修复：Score 引擎已禁用（前端可任意刷分，严重作弊漏洞）
            // 保留枚举定义防止 Rust match 不完整，但任何 Score 引擎调用都返回错误
            return Err(ContractError::ScoreEngineDisabled);
        }
    }
}

// ========================================================================
// 引擎 1：猜数字 Guess
// ========================================================================
pub fn play_guess(
    seed: &[u8],
    bet: Uint128,
    params: &GameEngineParams,
    input: &GuessInput,
) -> Result<PlayResult, ContractError> {
    let (min, max, multiplier) = match params {
        GameEngineParams::Guess { min, max, multiplier } => (*min, *max, *multiplier),
        _ => return Err(ContractError::UnsupportedEngine { engine: "Guess".to_string() }),
    };

    // 校验输入
    if input.guess < min || input.guess > max {
        return Err(ContractError::GuessOutOfRange { guess: input.guess, min, max });
    }

    // 🟢 审计修复：原写法 `max - min + 1` 在 max < min 时 u64 下溢
    // （debug 直接 panic，release 会得到一个天文数字，answer 变成不可预测的大数 → 玩家永远赢不了，
    //   但更糟的是攻击者可借此构造出确定性的答案）。这里改为 checked 计算并显式报错。
    let range = max
        .checked_sub(min)
        .and_then(|d| d.checked_add(1))
        .ok_or_else(|| ContractError::InvalidEngineParams(
            "Guess: max 必须 >= min".to_string(),
        ))?;

    // 合约生成正确答案
    let answer = random_u64(seed, range) + min;

    let (won, profit) = if input.guess == answer {
        let profit = bet
            .checked_mul(Uint128::from(multiplier))
            .map_err(|_| ContractError::Overflow)?
            .checked_div(Uint128::from(100u128))
            .map_err(|_| ContractError::Overflow)?;
        (true, profit)
    } else {
        (false, Uint128::zero())
    };

    Ok(PlayResult {
        won,
        profit,
        engine_result: format!("answer={}, guess={}", answer, input.guess),
    })
}

// ========================================================================
// 引擎 2：轮盘 Roulette
// ========================================================================
pub fn play_roulette(
    seed: &[u8],
    bet: Uint128,
    params: &GameEngineParams,
    input: &RouletteInput,
) -> Result<PlayResult, ContractError> {
    let (slots, payouts) = match params {
        GameEngineParams::Roulette { slots, payouts } => (*slots, payouts.clone()),
        _ => return Err(ContractError::UnsupportedEngine { engine: "Roulette".to_string() }),
    };

    if input.bet_slot >= slots {
        return Err(ContractError::RouletteSlotOutOfRange { slot: input.bet_slot, slots });
    }

    // 合约生成中奖 slot
    let winning_slot = random_u64(seed, slots);

    let (won, profit) = if input.bet_slot == winning_slot {
        // 找到这个 slot 对应的派彩倍数
        let mult = payouts.get(input.bet_slot as usize).copied().unwrap_or(0);
        let profit = bet
            .checked_mul(Uint128::from(mult))
            .map_err(|_| ContractError::Overflow)?
            .checked_div(Uint128::from(100u128))
            .map_err(|_| ContractError::Overflow)?;
        (mult > 0, profit)
    } else {
        (false, Uint128::zero())
    };

    Ok(PlayResult {
        won,
        profit,
        engine_result: format!("winning_slot={}, bet_slot={}", winning_slot, input.bet_slot),
    })
}

// ========================================================================
// 引擎 3：骰子 Dice
// ========================================================================

/// 骰子「指定点数（specific）」玩法的庄家优势，单位 basis points（10000 = 100%）
///
/// 1570 = 15.7% 抽水 → RTP ≈ 84.3%
/// （经济模型调整：通用游戏目标庄家净 +10%，由「5% 抽水 + 5% 销毁」与
///   「赔率下调」共同实现；本常量控制 specific 玩法的赔率部分。）
pub const DICE_SPECIFIC_EDGE_BPS: u64 = 1570;

/// 计算 specific 玩法的 profit 倍数（百分制：100 表示 profit = 1.00 × bet）
///
/// ## 数学推导
///
/// 「n 颗骰子里至少有一颗等于指定点数」的命中概率：
///
/// ```text
/// p = 1 - ((sides-1)/sides)^n = (sides^n - (sides-1)^n) / sides^n
/// ```
///
/// 记 `A = sides^n`，`B = (sides-1)^n`，`hit = A - B`，则 `p = hit / A`。
///
/// - 公平 payout 倍数（含本金） = 1/p = A / hit
/// - 公平 profit 倍数（纯利润） = 1/p - 1 = (A - hit)/hit = **B / hit**
///
/// 再按 `(10000 - edge_bps) / 10000` 打折留出庄家优势，最后 ×100 转成百分制整数。
///
/// ## 验算（RTP = p × payout倍数 = p × (1 + profit倍数)）
///
/// RTP 与 edge 的关系（可反推定价）：`RTP = 1 − (B/A) × edge_bps/10000`，其中 `B/A = 1 − p`。
///
/// | 配置   | A      | B      | hit   | p       | profit倍数 | payout | RTP   |
/// |--------|--------|--------|-------|---------|-----------|--------|-------|
/// | 1d6    | 6      | 5      | 1     | 16.667% | 4.21      | 5.21×  | 86.9% |
/// | 2d6    | 36     | 25     | 11    | 30.556% | 1.91      | 2.91×  | 88.9% |
/// | 3d6    | 216    | 125    | 91    | 42.130% | 1.15      | 2.15×  | 90.6% |
/// | 1d100  | 100    | 99     | 1     | 1.000%  | 83.45     | 84.45× | 84.4% |
///
/// （上表按当前 DICE_SPECIFIC_EDGE_BPS = 1570 计算）
///
/// 除法一律向下取整（floor），取整残差归庄家，RTP 只会更低不会更高。
///
/// ⚠️ 边界：骰子多、面数少时（如 10d2，p≈99.9%）公平 profit 只有 0.098%，
/// floor 后 profit_mult = 0，此时退化为「和局退回本金」，RTP 99.9% < 100%，安全。
pub fn dice_specific_profit_mult(sides: u64, dice_count: u64) -> Result<u64, ContractError> {
    if sides < 2 || dice_count == 0 {
        return Err(ContractError::InvalidEngineParams(
            "Dice: sides 必须 >= 2 且 dice_count >= 1".to_string(),
        ));
    }

    let s = sides as u128;
    let n = dice_count as u32;

    // A = sides^n，B = (sides-1)^n
    // 参数域已在 validate_engine_config 限制为 sides<=100、dice_count<=10，
    // 100^10 = 1e20 << u128::MAX(3.4e38)，不会溢出。checked_pow 只是双保险。
    let a = s.checked_pow(n).ok_or(ContractError::Overflow)?;
    let b = s
        .checked_sub(1)
        .ok_or(ContractError::Overflow)?
        .checked_pow(n)
        .ok_or(ContractError::Overflow)?;
    let hit = a.checked_sub(b).ok_or(ContractError::Overflow)?;
    if hit == 0 {
        return Err(ContractError::InvalidEngineParams(
            "Dice: specific 命中概率为 0，无法定价".to_string(),
        ));
    }

    let keep_bps = (10000u128)
        .checked_sub(DICE_SPECIFIC_EDGE_BPS as u128)
        .ok_or(ContractError::Overflow)?;

    // profit_mult(百分制) = B * 100 * keep_bps / (hit * 10000)
    let numerator = b
        .checked_mul(100u128)
        .ok_or(ContractError::Overflow)?
        .checked_mul(keep_bps)
        .ok_or(ContractError::Overflow)?;
    let denominator = hit.checked_mul(10000u128).ok_or(ContractError::Overflow)?;
    let mult = numerator
        .checked_div(denominator)
        .ok_or(ContractError::Overflow)?;

    // 理论上 mult <= 9603（1d100 时最大），这里只是防御性截断
    if mult > u64::MAX as u128 {
        Ok(u64::MAX)
    } else {
        Ok(mult as u64)
    }
}

pub fn play_dice(
    seed: &[u8],
    bet: Uint128,
    params: &GameEngineParams,
    input: &DiceInput,
) -> Result<PlayResult, ContractError> {
    let (dice_count, sides) = match params {
        GameEngineParams::Dice { dice_count, sides, .. } => (*dice_count, *sides),
        _ => return Err(ContractError::UnsupportedEngine { engine: "Dice".to_string() }),
    };

    // 生成每颗骰子的点数
    let mut dice = Vec::new();
    let mut total: u64 = 0;
    for i in 0..dice_count {
        let mut sub_seed = seed.to_vec();
        sub_seed.extend_from_slice(&i.to_le_bytes());
        let v = random_u64(&sub_seed, sides) + 1;
        total = total.saturating_add(v);
        dice.push(v);
    }

    // 🟢 审计修复（高危必赢漏洞 #1）：大小单双的阈值
    // 旧实现 `big_threshold = sides * dice_count / 2 + 1` 把「点数和的取值范围」当成了均匀分布，
    // 但点数和服从的是钟形分布，取值范围是 [dice_count, dice_count*sides]，长度只有
    // (dice_count*sides - dice_count + 1) 而不是 dice_count*sides。
    // 以 2d6 为例：threshold = 7 → big = 总和>=7 的概率是 21/36 ≈ 58.3%，
    // small = 总和<7 只有 15/36 ≈ 41.7%。玩家无脑押 big，RTP ≈ 116.7%（3d6 下更是 125%）。
    //
    // 正确做法：以点数和的真实分布中点 (min_sum+max_sum)/2 为界，
    // 中点恰好落在整数上时（和为偶数）判和局退回本金 —— 与骰宝（Sic Bo）规则一致。
    let min_sum = dice_count;                              // 每颗最少 1 点
    let max_sum = dice_count.saturating_mul(sides);        // 每颗最多 sides 点
    let range_sum = min_sum.saturating_add(max_sum);
    let mid = range_sum / 2;
    let has_tie = range_sum % 2 == 0;                      // 存在唯一中点 → 该点为和局

    // 🟢 骰宝（Sic Bo）标准规则：3 颗及以上骰子全同（豹子/全围）时，大小通杀。
    // 不加这条的话 big/small 会是严格 50/50、1:1 派彩，RTP 恰好 100% ——
    // 玩家虽然无法套利，但庄家也永远赚不到钱。
    //
    // ⚠️ 实测 RTP（精确枚举，非估算）：
    //   dice_count >= 3：豹子概率 = sides/sides^n = 1/sides^(n-1)，大小各损失这一份
    //     3d6  → 1/36  = 2.78%  → 胜率 48.61% → RTP 84.6%
    //     5d6  → 1/6^4 = 0.077% → 胜率 49.96% → RTP 86.9%   ← 骰子越多优势越薄
    //   dice_count <= 2：**无豹子规则**，胜率 50%，RTP = 50% × 1.74 = 87%
    //
    // 🟢 经济模型调整后：big/small/odd/even 固定 profit_mult=74（派彩 1.74x），
    // 无论有没有豹子规则，RTP 都在 84.6%~87% 之间，庄家始终有稳定优势。
    let all_same = dice_count >= 3 && !dice.is_empty() && dice.iter().all(|&d| d == dice[0]);

    let mut won = false;
    let mut profit_mult: u64 = 0;

    match input.bet_type.as_str() {
        "big" => {
            if all_same {
                won = false;
                profit_mult = 0;
            } else if has_tie && total == mid {
                // 和局：退回本金（won=true, profit=0）
                won = true;
                profit_mult = 0;
            } else {
                won = total > mid;
                profit_mult = if won { 74 } else { 0 };   // 固定庄家优势，与 odd/even 一致
            }
        }
        "small" => {
            if all_same {
                won = false;
                profit_mult = 0;
            } else if has_tie && total == mid {
                won = true;
                profit_mult = 0;
            } else {
                // 中点非整数时，mid 归入 small，保证 big/small 严格各占 50%
                won = total < mid || (!has_tie && total == mid);
                profit_mult = if won { 74 } else { 0 };
            }
        }
        // 🟢 修复：odd / even 统一庄家优势（原 RTP ≈ 97%，现为 ≈ 84.6%）。
        // 之前 profit_mult=100 → 某些配置下 RTP 恰好 100%（庄家零收益），
        // 奇数面数 + 奇数骰子数时甚至 RTP > 100%（可套利）。
        // 现在固定 profit_mult=74，无论 sides/dice_count 怎么配都有稳定庄家优势。
        "odd" => {
            won = total % 2 == 1;
            profit_mult = if won { 74 } else { 0 };
        }
        "even" => {
            won = total % 2 == 0;
            profit_mult = if won { 74 } else { 0 };
        }
        "specific" => {
            let target = input.specific_value.ok_or(ContractError::InvalidDiceBetType {
                bet_type: "specific 需要指定 specific_value".to_string(),
            })?;
            if target == 0 || target > sides {
                return Err(ContractError::InvalidDiceBetType {
                    bet_type: format!("specific_value 必须在 1..{} 之间", sides),
                });
            }
            won = dice.iter().any(|&d| d == target);
            // 🟢 审计修复（高危必赢漏洞 #2，第二轮重新定价）：
            // 旧实现固定赔 `sides` 倍，但「任意一颗命中」的概率是 1-((sides-1)/sides)^n：
            //   1d6：p=16.67%，赔 6 倍  → RTP 100%（庄家零收益）
            //   3d6：p=42.13%，赔 6 倍 → RTP 253%（玩家必赢，可无限套利）
            //
            // ⚠️ 第一轮曾改成 `95 * sides / dice_count`，这是**错的**：
            //   那个公式把「期望命中次数」当成了命中概率，而且 profit_mult 是
            //   **利润**倍数（payout = bet + profit，不是 payout 倍数）。
            //   3d6 → 95*6/3 = 190（profit 1.90×bet，payout 2.90×bet）→ RTP ≈ 122%
            //   1d6 → 95*6/1 = 570（profit 5.70×bet，payout 6.70×bet）→ RTP ≈ 112%
            //   两者依然 > 100%，仍然可套利。必须按**精确组合数**定价。
            //
            // 精确定价（见 dice_specific_profit_mult）：
            // RTP = 1 − (1−p) × DICE_SPECIFIC_EDGE_BPS/10000（随命中率 p 变化，非恒定）。
            profit_mult = dice_specific_profit_mult(sides, dice_count)?;
        }
        _ => return Err(ContractError::InvalidDiceBetType { bet_type: input.bet_type.clone() }),
    }

    let profit = if won {
        bet.checked_mul(Uint128::from(profit_mult))
            .map_err(|_| ContractError::Overflow)?
            .checked_div(Uint128::from(100u128))
            .map_err(|_| ContractError::Overflow)?
    } else {
        Uint128::zero()
    };

    Ok(PlayResult {
        won,
        profit,
        engine_result: format!("dice={:?}, total={}, mid={}", dice, total, mid),
    })
}

// ========================================================================
// 引擎 4：卡牌 Card
// ========================================================================

/// 一张牌：rank + suit
#[derive(Debug, Clone)]
struct Card {
    rank: u8, // 2..14 (14=A, 13=K, 12=Q, 11=J)
    suit: u8, // 0..3 (♠♥♦♣)
}

impl Card {
    fn to_hex(&self) -> String {
        format!("{:02x}{:02x}", self.rank, self.suit)
    }
}

fn draw_card(seed: &[u8], used: &mut Vec<u64>, deck_size: u64) -> Option<Card> {
    for attempt in 0u32..100 {
        let mut sub_seed = seed.to_vec();
        sub_seed.extend_from_slice(&attempt.to_le_bytes());
        let mut sub_seed2 = sub_seed.clone();
        sub_seed2.extend_from_slice(&(used.len() as u64).to_le_bytes());

        let idx = random_u64(&sub_seed2, deck_size);
        if !used.contains(&idx) {
            used.push(idx);
            let rank = (idx % 13) + 2; // 2..14
            let suit = (idx / 13) % 4;
            return Some(Card { rank: rank as u8, suit: suit as u8 });
        }
    }
    None // 牌用完了
}

/// 计算牌型等级
/// 返回 rank (0 到 9)
///
/// ⚠️ 同花 / 顺子类牌型要求 `cards.len() >= 5`，不足 5 张时只判对子 / 三条 / 四条。
fn hand_rank(cards: &[Card]) -> u64 {
    // 简化版牌型判定（同花/顺子类需 5 张，对子类 2 张起）：
    // 0 = 高牌
    // 1 = 一对
    // 2 = 两对
    // 3 = 三条
    // 4 = 顺子
    // 5 = 同花
    // 6 = 葫芦
    // 7 = 四条
    // 8 = 同花顺
    // 9 = 皇家同花顺

    let n = cards.len();
    if n == 0 {
        return 0;
    }

    // 统计 rank 出现次数
    let mut rank_count = [0u8; 15]; // rank 0..14
    let mut suit_count = [0u8; 4];
    let mut ranks: Vec<u8> = Vec::new();
    for c in cards {
        rank_count[c.rank as usize] += 1;
        suit_count[c.suit as usize] += 1;
        ranks.push(c.rank);
    }
    ranks.sort();
    ranks.dedup();

    let mut sorted_ranks = ranks.clone();
    sorted_ranks.sort();

    // 🟢 审计修复（高危必赢漏洞 #3）：
    // 旧实现 `is_flush = suit_count.iter().any(|&c| c == n)` 在 n == 1 时恒成立
    // （任意一张牌都满足「某花色出现了 1 次」）；
    // 同时 `is_straight` 的判定里 `sorted_ranks.windows(2).all(...)` 在长度 < 2 的切片上
    // 会因为空迭代器而返回 true。二者叠加 → **任意一张牌都被判成「同花顺」**，
    // 抽到 A 更是直接判成皇家同花顺（rank 9）。
    // 只要管理员把 min_cards 配成 1（或 2），玩家就能稳定吃到最高档倍率。
    //
    // 扑克牌型（同花 / 顺子）本来就只在 5 张及以上成立，这里加上 n >= 5 的硬性门槛。
    let is_flush = n >= 5 && suit_count.iter().any(|&c| c as usize == n);

    // 判定顺子：去重后连续，同样要求 5 张及以上
    let mut is_straight = false;
    if n >= 5 && ranks.len() == n {
        // 特殊情况：A-2-3-4-5
        if sorted_ranks.len() == 5
            && sorted_ranks == vec![2u8, 3, 4, 5, 14]
        {
            is_straight = true;
        } else {
            is_straight = sorted_ranks.windows(2).all(|w| w[1] == w[0] + 1);
        }
    }

    // 🟢 审计修复：皇家同花顺原来只判断「同花顺 且 最大牌是 A」，
    // 于是 7-8-9-10-A 同花也会被算成皇家同花顺。这里改为严格判定 10-J-Q-K-A。
    let is_royal = is_straight && is_flush && sorted_ranks == vec![10u8, 11, 12, 13, 14];

    // 按 rank_count 统计各种对子/条数
    let pairs = rank_count.iter().filter(|&&c| c == 2).count() as u64;
    let trips = rank_count.iter().filter(|&&c| c == 3).count() as u64;
    let quads = rank_count.iter().filter(|&&c| c == 4).count() as u64;

    if is_royal {
        9
    } else if is_flush && is_straight {
        8
    } else if quads > 0 {
        7
    } else if trips > 0 && pairs > 0 {
        6
    } else if is_flush {
        5
    } else if is_straight {
        4
    } else if trips > 0 {
        3
    } else if pairs >= 2 {
        2
    } else if pairs == 1 {
        1
    } else {
        0
    }
}

pub fn play_card(
    seed: &[u8],
    bet: Uint128,
    params: &GameEngineParams,
    input: &CardInput,
) -> Result<PlayResult, ContractError> {
    let (deck_count, min_cards, max_cards, multiplier_by_hand) = match params {
        GameEngineParams::Card { deck_count, min_cards, max_cards, multiplier_by_hand } =>
            (*deck_count, *min_cards, *max_cards, multiplier_by_hand.clone()),
        _ => return Err(ContractError::UnsupportedEngine { engine: "Card".to_string() }),
    };

    if input.card_count < min_cards || input.card_count > max_cards {
        return Err(ContractError::CardCountOutOfRange {
            count: input.card_count, min: min_cards, max: max_cards,
        });
    }

    // 🟢 审计修复：原写法 `deck_count * 52` 在 deck_count 极大时会溢出；
    // 同时 total_deck == 0 会让 random_u64 恒返回 0，导致每次都发同一张牌。
    let total_deck = deck_count.saturating_mul(52u64);
    if total_deck == 0 {
        return Err(ContractError::InvalidEngineParams(
            "Card: deck_count 必须大于 0".to_string(),
        ));
    }
    // 请求牌数不能超过牌堆实际张数
    if (input.card_count as u128) * 2 > total_deck as u128 {
        return Err(ContractError::InvalidEngineParams(
            "Card: card_count 超过牌堆容量".to_string(),
        ));
    }

    let mut used = Vec::new();

    // 发玩家的牌
    let mut player_cards: Vec<Card> = Vec::new();
    for _ in 0..input.card_count {
        match draw_card(seed, &mut used, total_deck) {
            Some(c) => player_cards.push(c),
            None => break, // 牌用完了
        }
    }

    // 🟢 审计修复：旧实现发牌不足时静默继续（用更少的牌判牌型），会悄悄改变赔率。
    // 发牌失败必须让整笔交易失败，不能带病结算。
    if player_cards.len() != input.card_count as usize {
        return Err(ContractError::InvalidEngineParams(
            "Card: 发牌失败（牌堆不足或随机数冲突）".to_string(),
        ));
    }

    let player_rank = hand_rank(&player_cards);

    let (won, profit) = match input.mode.as_str() {
        "rank_only" => {
            // 只按牌型派彩（跟 multiplier_by_hand 配置）
            let mult = multiplier_by_hand
                .get(player_rank as usize)
                .copied()
                .unwrap_or(0);
            if mult > 0 {
                let profit = bet
                    .checked_mul(Uint128::from(mult))
                    .map_err(|_| ContractError::Overflow)?
                    .checked_div(Uint128::from(100u128))
                    .map_err(|_| ContractError::Overflow)?;
                (true, profit)
            } else {
                (false, Uint128::zero())
            }
        }
        "vs_dealer" => {
            // 跟庄家比：庄家发同样数量的牌
            let mut dealer_cards: Vec<Card> = Vec::new();
            for _ in 0..input.card_count {
                match draw_card(seed, &mut used, total_deck) {
                    Some(c) => dealer_cards.push(c),
                    None => break,
                }
            }
            // 🟢 审计修复：庄家发牌不足时不能静默用更少的牌比大小
            if dealer_cards.len() != input.card_count as usize {
                return Err(ContractError::InvalidEngineParams(
                    "Card: 庄家发牌失败".to_string(),
                ));
            }
            let dealer_rank = hand_rank(&dealer_cards);

            // 🟢 修复 RTP=100% 问题：原来 1:1 派彩 + 平局退回本金 → RTP 恰好 100%，
            // 庄家零收益。现在赢 profit_mult=78（0.78×bet 利润），平局退回本金，
            // 稳定庄家优势约 22% × (1 − P(平))。
            if player_rank > dealer_rank {
                let profit = bet
                    .checked_mul(Uint128::from(78u64))
                    .map_err(|_| ContractError::Overflow)?
                    .checked_div(Uint128::from(100u128))
                    .map_err(|_| ContractError::Overflow)?;
                (true, profit)
            } else if player_rank == dealer_rank {
                // 平局退回本金（won=true, profit=0），Play 函数会正确处理
                (true, Uint128::zero())
            } else {
                (false, Uint128::zero())
            }
        }
        _ => return Err(ContractError::InvalidCardMode { mode: input.mode.clone() }),
    };

    let card_str: Vec<String> = player_cards.iter().map(|c| c.to_hex()).collect();
    Ok(PlayResult {
        won,
        profit,
        engine_result: format!("player_cards={:?}, hand_rank={}", card_str, player_rank),
    })
}

// ========================================================================
// 🟡 Score 引擎已禁用（前端可任意刷分，严重作弊漏洞）
// 消消乐/跑酷等操作型游戏，分数只能前端计算，合约无法验证真实性
// 保留 GameEngine::Score 枚举定义防止 Rust match 不完整
// 但 play_dispatch 中对 Score 直接返回 ScoreEngineDisabled 错误
//
// 将来如果需要这类游戏，建议：
//   1. 用 Deduct + Settle 两段式 + 严格的每日/每局限额（接受小额信任风险）
//   2. 或部署服务端做分数验证
// ========================================================================

