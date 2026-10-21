// ============================================================================
// 战斗引擎核心：9v9 三组制（compute_battle）
//
// 权威依据：《三国卡牌V2修改裁定书》第 7 章（T9/E1~E3/G1~G4）
//           + 《三国卡牌V2设计终稿》文档 1（比赛模拟算法，终稿锁定版）。
//
// 确定性约定（链上与前端预演必须逐字节一致，112 组向量的意义）：
//   1. 阶段顺序：OnBattleStart → [每组] OnGroupStart → 预战力 → OnLow/HighPower
//      → 终战力 → 组内 best-of-3 → (下一组前) OnWin/LoseGroup
//   2. 每阶段内**固定 A 方先、B 方后**，卡位按 0..9 顺序（floor 顺序即此）
//   3. 所有乘除每步立即向下取整（floor），不缓存有理数（E1）
//   4. 四因子顺序固定 faction → troop → bond → rarity（E1）
//   5. OnLowPower/OnHighPower 按预战力**每卡对位**判定一次，应用后不重判（S8）；
//      判定口径以设计终稿文档 1 计算示例（关羽 239 vs 许褚 217，虎痴不触发）为准
//   6. 组内 1:1 / 0:0 / 1:0 → 该组双方均不胜（G2：无人拿到 2 胜）
//   7. 3 组打满仍无人先赢 2 组 → 整场平局（G3）
//   8. target=Opponent 的技能作用于对手**全队 9 张」（设计终稿文档 1 步骤 1 明示）
//   9. team_wide 仅对 target=Self 生效（自身全队）；Opponent 无视 team_wide
//  10. 平局组不触发 OnWinGroup / OnLoseGroup（非胜非负）
// ============================================================================

use cosmwasm_schema::cw_serde;
use sha2::{Digest, Sha256};

use crate::error::ContractError;
use crate::sanguo::state::SanguoParams;

use super::bonds::{bond_multiplier_bp, MAX_BOND_BP};
use super::skills::{find_skill, SkillEffect, SkillTarget, SkillTrigger};

/// 10000^4（四因子基点制的总分母）
const TEN_K4: u128 = 10_000_000_000_000_000;

// ---------------------------------------------------------------------------
// 参数默认值（SanguoParams 对应字段为 0 时回退；访问器在 state.rs 侧）
// ---------------------------------------------------------------------------
pub const DEFAULT_FACTION_COUNTER_BP: u64 = 11500;
pub const DEFAULT_FACTION_COUNTERED_BP: u64 = 9000;
pub const DEFAULT_FACTION_NEUTRAL_BP: u64 = 10000;
pub const DEFAULT_TROOP_COUNTER_BP: u64 = 11000;
pub const DEFAULT_TROOP_COUNTERED_BP: u64 = 10000; // S2：被克无惩罚
pub const DEFAULT_TROOP_NEUTRAL_BP: u64 = 10000;
pub const DEFAULT_RARITY_LEGEND_BP: u64 = 11500; // S5：仅传说享稀有度系数
pub const DEFAULT_RARITY_EPIC_BP: u64 = 10000;
pub const DEFAULT_RARITY_RARE_BP: u64 = 10000;
pub const DEFAULT_RARITY_COMMON_BP: u64 = 10000;
pub const DEFAULT_POWER_FLAT_CAP_BP: u64 = 3000; // S6+：战力 flat Σ≤3000 封顶
pub const DEFAULT_EFF_LOWER_BOUND_BP: u64 = 5000; // E2：攻防减益下限 50%
pub const DEFAULT_SKILL_STRENGTH_BP: u64 = 10000; // 技能强度全局系数（100%=原值）
pub const DEFAULT_BOND_STRENGTH_BP: u64 = 10000; // 连携强度全局系数（100%=原值）

/// 战斗系数快照（exec 层每场从 SanguoParams 解析一次，避免引擎反复读存储）
#[cw_serde]
pub struct BattleContext {
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
    /// 技能强度全局系数（bp）：所有技能 attack/defense/flat 效果乘以此值/10000
    pub skill_strength_bp: u64,
    /// 连携强度全局系数（bp）：连携加成部分（bond_mult−10000）乘以此值/10000
    pub bond_strength_bp: u64,
}

impl BattleContext {
    /// 从 SanguoParams 解析（0 = 回退默认常量；参数层可调、零 migrate）
    pub fn from_params(p: &SanguoParams) -> Self {
        let pick = |v: u64, d: u64| if v == 0 { d } else { v };
        BattleContext {
            faction_counter_bp: pick(p.faction_counter_bp, DEFAULT_FACTION_COUNTER_BP),
            faction_countered_bp: pick(p.faction_countered_bp, DEFAULT_FACTION_COUNTERED_BP),
            faction_neutral_bp: pick(p.faction_neutral_bp, DEFAULT_FACTION_NEUTRAL_BP),
            troop_counter_bp: pick(p.troop_counter_bp, DEFAULT_TROOP_COUNTER_BP),
            troop_countered_bp: pick(p.troop_countered_bp, DEFAULT_TROOP_COUNTERED_BP),
            troop_neutral_bp: pick(p.troop_neutral_bp, DEFAULT_TROOP_NEUTRAL_BP),
            rarity_legend_bp: pick(p.rarity_legend_bp, DEFAULT_RARITY_LEGEND_BP),
            rarity_epic_bp: pick(p.rarity_epic_bp, DEFAULT_RARITY_EPIC_BP),
            rarity_rare_bp: pick(p.rarity_rare_bp, DEFAULT_RARITY_RARE_BP),
            rarity_common_bp: pick(p.rarity_common_bp, DEFAULT_RARITY_COMMON_BP),
            power_flat_cap_bp: pick(p.power_flat_cap_bp, DEFAULT_POWER_FLAT_CAP_BP),
            eff_lower_bound_bp: pick(p.eff_lower_bound_bp, DEFAULT_EFF_LOWER_BOUND_BP),
            skill_strength_bp: pick(p.skill_strength_bp, DEFAULT_SKILL_STRENGTH_BP),
            bond_strength_bp: pick(p.bond_strength_bp, DEFAULT_BOND_STRENGTH_BP),
        }
    }
}

/// 参战卡（exec loader 组装 CardInfo + CardTemplate；star/level 加成已烘进
/// attack/defense——养成侧 star_up/upgrade 直接改卡面数值，引擎不二次加成）
#[cw_serde]
pub struct BattleCard {
    pub card_id: String,
    pub name: String,
    pub rarity: String,
    pub faction: String,
    pub troop: String,
    pub cost: u8,
    pub attack: u32,
    pub defense: u32,
    pub star: u8,
    pub level: u8,
    /// 技能 id；空串 = 无技能（未到 3★ 或模板无技能）
    pub skill_id: String,
}

/// 单场胜负
#[cw_serde]
pub enum Winner {
    A,
    B,
    Draw,
}

/// 战斗结果（裁定 T9：补 groups_played / base_powers 字段）
#[cw_serde]
pub struct BattleResult {
    pub winner: Winner,
    /// 每组 A/B 方拿下的卡胜数（0..3）
    pub group_scores_a: [u8; 3],
    pub group_scores_b: [u8; 3],
    pub group_wins_a: u8,
    pub group_wins_b: u8,
    pub net_groups_a: i8,
    pub net_groups_b: i8,
    /// 实际打了几组（先赢 2 组即停 → 2 或 3；打满 3 组仍分不出 → 3）
    pub groups_played: u8,
    /// 每张卡的终战力（最后一次计算的 final_power，含技能/克制/连携/稀有度）
    pub a_final_powers: Vec<u64>,
    pub b_final_powers: Vec<u64>,
    /// 每张卡的 base_power（平局判定的 base 口径，末次计算值）
    pub a_base_powers: Vec<u64>,
    pub b_base_powers: Vec<u64>,
    /// G4 口径：Σ(OnBattleStart 生效后 base × rarity_mult) + bond 加成
    pub a_self_power: u64,
    pub b_self_power: u64,
    /// V2.1：技能触发日志（逐条记录本场实际触发的技能，供前端战斗回放展示）。
    /// 只观测、不改结算；side: 0=A 方，1=B 方。
    pub skill_logs: Vec<SkillTriggerLog>,
}

/// V2.1：单条技能触发记录（战斗回放用）
#[cw_serde]
pub struct SkillTriggerLog {
    /// 0 = A 方（我方/挑战者），1 = B 方（对手/应战者）
    pub side: u8,
    pub card_name: String,
    pub skill_id: String,
    pub trigger: SkillTrigger,
    pub target: SkillTarget,
    pub team_wide: bool,
    pub attack_bp: i32,
    pub defense_bp: i32,
    pub power_flat_bp: i32,
}

// ---------------------------------------------------------------------------
// 引擎内部状态
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq)]
enum Side {
    A,
    B,
}

impl Side {
    fn opponent(self) -> Side {
        match self {
            Side::A => Side::B,
            Side::B => Side::A,
        }
    }
    fn as_u8(self) -> u8 {
        match self {
            Side::A => 0,
            Side::B => 1,
        }
    }
}

/// 单卡运行时状态（技能加减益在此累积，每步 floor + 50% 下限）
struct CardState {
    attack_orig: u128,
    defense_orig: u128,
    /// 当前有效攻击（buff/debuff 累积值）
    attack: u128,
    defense: u128,
    /// 战力 flat 加成累积（bp，加法叠加；base 计算时封顶）
    power_flat_sum: i128,
    /// OnWinGroup/OnLoseGroup 已叠层数（受 max_stacks 限制）
    stacks: u8,
}

impl CardState {
    fn from_card(c: &BattleCard) -> Self {
        CardState {
            attack_orig: c.attack as u128,
            defense_orig: c.defense as u128,
            attack: c.attack as u128,
            defense: c.defense as u128,
            power_flat_sum: 0,
            stacks: 0,
        }
    }
}

struct BattleEngine<'a> {
    ctx: &'a BattleContext,
    a_cards: &'a [BattleCard; 9],
    b_cards: &'a [BattleCard; 9],
    a: [CardState; 9],
    b: [CardState; 9],
    /// V2.1：技能触发日志收集器
    skill_logs: Vec<SkillTriggerLog>,
}

// ---------------------------------------------------------------------------
// 基础算术（E1 每步 floor / E2 下限 / E3 u128 中间值）
// ---------------------------------------------------------------------------

/// x × mult_bp / 10000，向下取整（mult_bp > 0，由调用方保证）
fn mul_bp_floor(x: u128, mult_bp: u64) -> Result<u128, ContractError> {
    x.checked_mul(mult_bp as u128)
        .map(|v| v / 10000)
        .ok_or(ContractError::Overflow)
}

/// 50% 下限封底（E2）：不低于 orig × eff_lower_bound_bp / 10000
fn clamp_lower_bound(v: u128, orig: u128, lb_bp: u64) -> u128 {
    let floor_v = orig.saturating_mul(lb_bp as u128) / 10000;
    if v < floor_v {
        floor_v
    } else {
        v
    }
}

/// 技能强度缩放：把技能效果 bp 乘以全局系数（strength/10000，i64 中转防溢出、截断向零）
fn scale_effect_bp(v: i32, strength_bp: u64) -> i32 {
    ((v as i64) * (strength_bp as i64) / 10000) as i32
}

/// 把单个技能效果应用到一张卡（攻防乘算每步 floor + 下限；flat 加法累积）
fn apply_to_card(
    st: &mut CardState,
    eff: &SkillEffect,
    ctx: &BattleContext,
) -> Result<(), ContractError> {
    // 🟢 参数收敛：技能强度全局系数（skill_strength_bp）——60 技能各自数值仍是
    //    编译期常量，管理员用这一个旋钮整体调强弱（0 参数=回退默认 100%）
    let atk_bp = scale_effect_bp(eff.attack_bp, ctx.skill_strength_bp);
    let def_bp = scale_effect_bp(eff.defense_bp, ctx.skill_strength_bp);
    let flat_bp = scale_effect_bp(eff.power_flat_bp, ctx.skill_strength_bp);
    if atk_bp != 0 {
        // mult = 10000 + bp（bp 最小 -1800 → mult ≥ 8200 > 0）
        let mult = (10000i64 + atk_bp as i64) as u64;
        let v = mul_bp_floor(st.attack, mult)?;
        st.attack = clamp_lower_bound(v, st.attack_orig, ctx.eff_lower_bound_bp);
    }
    if def_bp != 0 {
        let mult = (10000i64 + def_bp as i64) as u64;
        let v = mul_bp_floor(st.defense, mult)?;
        st.defense = clamp_lower_bound(v, st.defense_orig, ctx.eff_lower_bound_bp);
    }
    if flat_bp != 0 {
        st.power_flat_sum = st
            .power_flat_sum
            .saturating_add(flat_bp as i128);
    }
    Ok(())
}

/// base_power = (attack_eff + defense_eff) × (1 + min(Σpower_flat, cap)/10000)
fn base_power(st: &CardState, ctx: &BattleContext) -> Result<u128, ContractError> {
    let raw = st
        .attack
        .checked_add(st.defense)
        .ok_or(ContractError::Overflow)?;
    let sum: u128 = if st.power_flat_sum <= 0 {
        0
    } else {
        st.power_flat_sum as u128
    };
    let capped = sum.min(ctx.power_flat_cap_bp as u128);
    raw.checked_mul(10000 + capped)
        .map(|v| v / 10000)
        .ok_or(ContractError::Overflow)
}

/// E3 溢出前置检查：base × Π(最大系数) 不得越过 u64 容量
fn check_base_bound(base: u128, ctx: &BattleContext) -> Result<(), ContractError> {
    let fmax = ctx
        .faction_counter_bp
        .max(ctx.faction_countered_bp)
        .max(ctx.faction_neutral_bp) as u128;
    let tmax = ctx
        .troop_counter_bp
        .max(ctx.troop_countered_bp)
        .max(ctx.troop_neutral_bp) as u128;
    let rmax = ctx
        .rarity_legend_bp
        .max(ctx.rarity_epic_bp)
        .max(ctx.rarity_rare_bp)
        .max(ctx.rarity_common_bp) as u128;
    let bond_max = MAX_BOND_BP as u128;
    let prod = fmax
        .checked_mul(tmax)
        .and_then(|v| v.checked_mul(bond_max))
        .and_then(|v| v.checked_mul(rmax))
        .ok_or(ContractError::Overflow)?;
    let bound = (u64::MAX as u128)
        .checked_mul(TEN_K4)
        .and_then(|v| v.checked_div(prod))
        .ok_or(ContractError::Overflow)?;
    if base > bound {
        return Err(ContractError::Overflow);
    }
    Ok(())
}

/// final_power = base × faction × troop × bond × rarity ÷ 10000^4（每步 floor）
fn final_power(
    base: u128,
    faction_bp: u64,
    troop_bp: u64,
    bond_bp: u64,
    rarity_bp: u64,
) -> Result<u128, ContractError> {
    let mut v = base;
    for m in [faction_bp, troop_bp, bond_bp, rarity_bp] {
        v = mul_bp_floor(v, m)?;
    }
    Ok(v)
}

/// 阵营克制：wei→shu→wu→qun→wei
fn faction_bp(ctx: &BattleContext, atk: &str, def: &str) -> u64 {
    match (atk, def) {
        ("wei", "shu") | ("shu", "wu") | ("wu", "qun") | ("qun", "wei") => ctx.faction_counter_bp,
        ("shu", "wei") | ("wu", "shu") | ("qun", "wu") | ("wei", "qun") => ctx.faction_countered_bp,
        _ => ctx.faction_neutral_bp,
    }
}

/// 兵种克制：cavalry→archer→strategist→guard→cavalry（被克无惩罚 S2）
fn troop_bp(ctx: &BattleContext, atk: &str, def: &str) -> u64 {
    match (atk, def) {
        ("cavalry", "archer") | ("archer", "strategist") | ("strategist", "guard")
        | ("guard", "cavalry") => ctx.troop_counter_bp,
        ("archer", "cavalry") | ("strategist", "archer") | ("guard", "strategist")
        | ("cavalry", "guard") => ctx.troop_countered_bp,
        _ => ctx.troop_neutral_bp,
    }
}

/// 稀有度系数（S5：仅 legend 非 1.0）
fn rarity_bp(ctx: &BattleContext, rarity: &str) -> u64 {
    match rarity {
        "legend" => ctx.rarity_legend_bp,
        "epic" => ctx.rarity_epic_bp,
        "rare" => ctx.rarity_rare_bp,
        _ => ctx.rarity_common_bp,
    }
}

// ---------------------------------------------------------------------------
// 引擎主流程
// ---------------------------------------------------------------------------

impl<'a> BattleEngine<'a> {
    fn new(ctx: &'a BattleContext, a: &'a [BattleCard; 9], b: &'a [BattleCard; 9]) -> Self {
        BattleEngine {
            ctx,
            a_cards: a,
            b_cards: b,
            a: core::array::from_fn(|i| CardState::from_card(&a[i])),
            b: core::array::from_fn(|i| CardState::from_card(&b[i])),
            skill_logs: Vec::new(),
        }
    }

    fn cards(&self, side: Side) -> &'a [BattleCard; 9] {
        match side {
            Side::A => self.a_cards,
            Side::B => self.b_cards,
        }
    }

    fn states(&mut self, side: Side) -> &mut [CardState; 9] {
        match side {
            Side::A => &mut self.a,
            Side::B => &mut self.b,
        }
    }

    fn states_ref(&self, side: Side) -> &[CardState; 9] {
        match side {
            Side::A => &self.a,
            Side::B => &self.b,
        }
    }

    /// 收集并应用一个「简单阶段」（OnBattleStart / OnGroupStart）的技能。
    /// 顺序：A 方（卡 0..9）→ B 方（卡 0..9）。
    fn phase_simple(&mut self, trigger: SkillTrigger) -> Result<(), ContractError> {
        // V2.1：技能触发日志收集器（与结算解耦，避免借用冲突）
        let mut new_logs: Vec<SkillTriggerLog> = Vec::new();
        for side in [Side::A, Side::B] {
            // 先收集（eff 是 &'static，不借 self），再应用，避免借用冲突
            let mut ops: Vec<(Side, Option<usize>, &'static SkillEffect)> = Vec::new();
            for i in 0..9 {
                let card = &self.cards(side)[i];
                if let Some(eff) = find_skill(&card.skill_id) {
                    if eff.trigger == trigger {
                        new_logs.push(SkillTriggerLog {
                            side: side.as_u8(),
                            card_name: card.name.clone(),
                            skill_id: card.skill_id.clone(),
                            trigger: eff.trigger.clone(),
                            target: eff.target.clone(),
                            team_wide: eff.team_wide,
                            attack_bp: eff.attack_bp,
                            defense_bp: eff.defense_bp,
                            power_flat_bp: eff.power_flat_bp,
                        });
                        match eff.target {
                            SkillTarget::Self_ => {
                                if eff.team_wide {
                                    ops.push((side, None, eff));
                                } else {
                                    ops.push((side, Some(i), eff));
                                }
                            }
                            SkillTarget::Opponent => ops.push((side.opponent(), None, eff)),
                        }
                    }
                }
            }
            for (tgt, idx, eff) in ops {
                // 🟢 拆字段借用：states(可变) 与 ctx(不可变) 是不相交字段，可同时借
                let (states, ctx) = match tgt {
                    Side::A => (&mut self.a, self.ctx),
                    Side::B => (&mut self.b, self.ctx),
                };
                match idx {
                    Some(i) => apply_to_card(&mut states[i], eff, ctx)?,
                    None => {
                        for st in states.iter_mut() {
                            apply_to_card(st, eff, ctx)?;
                        }
                    }
                }
            }
        }
        self.skill_logs.append(&mut new_logs);
        Ok(())
    }

    /// OnLowPower / OnHighPower 阶段：按预战力每卡对位判定一次（S8）。
    /// 顺序：A 方（组内卡 0..3）→ B 方（组内卡 0..3）。
    fn phase_power(
        &mut self,
        g: usize,
        pre_a: &[u128; 9],
        pre_b: &[u128; 9],
    ) -> Result<(), ContractError> {
        // V2.1：技能触发日志收集器
        let mut new_logs: Vec<SkillTriggerLog> = Vec::new();
        for side in [Side::A, Side::B] {
            let mut ops: Vec<(Side, Option<usize>, &'static SkillEffect)> = Vec::new();
            for j in 0..3 {
                let i = g * 3 + j;
                let (mine, theirs) = match side {
                    Side::A => (pre_a[i], pre_b[i]),
                    Side::B => (pre_b[i], pre_a[i]),
                };
                let want = if mine < theirs {
                    SkillTrigger::OnLowPower
                } else if mine > theirs {
                    SkillTrigger::OnHighPower
                } else {
                    continue; // 预战力持平 → 双方都不触发
                };
                let card = &self.cards(side)[i];
                if let Some(eff) = find_skill(&card.skill_id) {
                    if eff.trigger == want {
                        new_logs.push(SkillTriggerLog {
                            side: side.as_u8(),
                            card_name: card.name.clone(),
                            skill_id: card.skill_id.clone(),
                            trigger: eff.trigger.clone(),
                            target: eff.target.clone(),
                            team_wide: eff.team_wide,
                            attack_bp: eff.attack_bp,
                            defense_bp: eff.defense_bp,
                            power_flat_bp: eff.power_flat_bp,
                        });
                        match eff.target {
                            SkillTarget::Self_ => {
                                if eff.team_wide {
                                    ops.push((side, None, eff));
                                } else {
                                    ops.push((side, Some(i), eff));
                                }
                            }
                            SkillTarget::Opponent => ops.push((side.opponent(), None, eff)),
                        }
                    }
                }
            }
            for (tgt, idx, eff) in ops {
                // 🟢 拆字段借用：states(可变) 与 ctx(不可变) 是不相交字段，可同时借
                let (states, ctx) = match tgt {
                    Side::A => (&mut self.a, self.ctx),
                    Side::B => (&mut self.b, self.ctx),
                };
                match idx {
                    Some(i) => apply_to_card(&mut states[i], eff, ctx)?,
                    None => {
                        for st in states.iter_mut() {
                            apply_to_card(st, eff, ctx)?;
                        }
                    }
                }
            }
        }
        self.skill_logs.append(&mut new_logs);
        Ok(())
    }

    /// OnWinGroup / OnLoseGroup 阶段：一组结束后对下一组挂效果。
    /// 平局组（双方均不胜）不触发任何技能。顺序：A 方 → B 方。
    fn phase_group_outcome(&mut self, a_won: bool, b_won: bool) -> Result<(), ContractError> {
        // V2.1：技能触发日志收集器
        let mut new_logs: Vec<SkillTriggerLog> = Vec::new();
        for side in [Side::A, Side::B] {
            let won = match side {
                Side::A => a_won,
                Side::B => b_won,
            };
            let lost = match side {
                Side::A => b_won,
                Side::B => a_won,
            };
            if !won && !lost {
                continue; // 平局组：非胜非负
            }
            let want = if won {
                SkillTrigger::OnWinGroup
            } else {
                SkillTrigger::OnLoseGroup
            };
            let mut ops: Vec<(Side, Option<usize>, &'static SkillEffect, usize)> = Vec::new();
            for i in 0..9 {
                let card = &self.cards(side)[i];
                if let Some(eff) = find_skill(&card.skill_id) {
                    if eff.trigger == want {
                        let stacks = self.states_ref(side)[i].stacks;
                        if stacks < eff.max_stacks {
                            new_logs.push(SkillTriggerLog {
                                side: side.as_u8(),
                                card_name: card.name.clone(),
                                skill_id: card.skill_id.clone(),
                                trigger: eff.trigger.clone(),
                                target: eff.target.clone(),
                                team_wide: eff.team_wide,
                                attack_bp: eff.attack_bp,
                                defense_bp: eff.defense_bp,
                                power_flat_bp: eff.power_flat_bp,
                            });
                            match eff.target {
                                SkillTarget::Self_ => {
                                    if eff.team_wide {
                                        ops.push((side, None, eff, i));
                                    } else {
                                        ops.push((side, Some(i), eff, i));
                                    }
                                }
                                SkillTarget::Opponent => {
                                    ops.push((side.opponent(), None, eff, i));
                                }
                            }
                        }
                    }
                }
            }
            for (tgt, idx, eff, src_i) in ops {
                // 叠层计数在**技能所属卡**上（team_wide 也记在源卡）
                self.states(side)[src_i].stacks += 1;
                // 🟢 拆字段借用：states(可变) 与 ctx(不可变) 是不相交字段，可同时借
                let (states, ctx) = match tgt {
                    Side::A => (&mut self.a, self.ctx),
                    Side::B => (&mut self.b, self.ctx),
                };
                match idx {
                    Some(i) => apply_to_card(&mut states[i], eff, ctx)?,
                    None => {
                        for st in states.iter_mut() {
                            apply_to_card(st, eff, ctx)?;
                        }
                    }
                }
            }
        }
        self.skill_logs.append(&mut new_logs);
        Ok(())
    }

    /// 全 9 张 base_power
    fn all_base(&self, side: Side) -> Result<[u128; 9], ContractError> {
        let mut out = [0u128; 9];
        for i in 0..9 {
            out[i] = base_power(&self.states_ref(side)[i], self.ctx)?;
        }
        Ok(out)
    }

    /// 全 9 张 final_power（faction/troop 按对位卡计算）
    fn all_final(
        &self,
        base: &[u128; 9],
        side: Side,
        bond_bp: u64,
    ) -> Result<[u128; 9], ContractError> {
        let cards = self.cards(side);
        let opp = self.cards(side.opponent());
        let mut out = [0u128; 9];
        for i in 0..9 {
            check_base_bound(base[i], self.ctx)?;
            let f = faction_bp(self.ctx, &cards[i].faction, &opp[i].faction);
            let t = troop_bp(self.ctx, &cards[i].troop, &opp[i].troop);
            let r = rarity_bp(self.ctx, &cards[i].rarity);
            let v = final_power(base[i], f, t, bond_bp, r)?;
            out[i] = v;
        }
        Ok(out)
    }

    /// G4 self_power：Σ(OnBattleStart 生效后 base × rarity_mult) + bond 加成
    fn self_power(&self, side: Side, bond_bp: u64) -> Result<u64, ContractError> {
        let cards = self.cards(side);
        let mut sum: u128 = 0;
        for i in 0..9 {
            let base = base_power(&self.states_ref(side)[i], self.ctx)?;
            let r = rarity_bp(self.ctx, &cards[i].rarity) as u128;
            let v = base.checked_mul(r).ok_or(ContractError::Overflow)? / 10000;
            sum = sum.checked_add(v).ok_or(ContractError::Overflow)?;
        }
        // bond 加成 = 激活连携 bp（bond_mult − 10000）
        sum = sum
            .checked_add(bond_bp.saturating_sub(10000) as u128)
            .ok_or(ContractError::Overflow)?;
        if sum > u64::MAX as u128 {
            return Err(ContractError::Overflow);
        }
        Ok(sum as u64)
    }

    fn run(&mut self) -> Result<BattleResult, ContractError> {
        // 连携每方每场只算一次（E6），3 组 × 3 张复用
        let names_a: Vec<&str> = self.a_cards.iter().map(|c| c.name.as_str()).collect();
        let names_b: Vec<&str> = self.b_cards.iter().map(|c| c.name.as_str()).collect();
        // 🟢 连携强度全局系数（bond_strength_bp）只缩放「加成部分」
        //    （bond_mult−10000），中性基数 10000 不动；缩放后 ≤ 原值，故
        //    check_base_bound 用 MAX_BOND_BP 做上界仍然成立。
        //    ⚠️ 系数先取局部值：闭包若捕获 &self.ctx 会与后续 &mut self
        //    的 phase 调用产生借用冲突（E0502）。
        let bond_strength = self.ctx.bond_strength_bp;
        let scale_bonus = |mult_bp: u64| -> u64 {
            let scaled = 10000u64 + (((mult_bp.saturating_sub(10000) as u128)
                * (bond_strength as u128)
                / 10000) as u64);
            // 上界钳制：缩放后不得越过 MAX_BOND_BP，保住 check_base_bound 的
            // 「bond ≤ MAX_BOND_BP」溢出上界不变量
            scaled.min(MAX_BOND_BP)
        };
        let bond_a = scale_bonus(bond_multiplier_bp(&names_a));
        let bond_b = scale_bonus(bond_multiplier_bp(&names_b));

        // 阶段 1：OnBattleStart（A 先 B 后）
        self.phase_simple(SkillTrigger::OnBattleStart)?;

        // G4 self_power 快照（OnBattleStart 生效后）
        let self_a = self.self_power(Side::A, bond_a)?;
        let self_b = self.self_power(Side::B, bond_b)?;

        let mut group_scores_a = [0u8; 3];
        let mut group_scores_b = [0u8; 3];
        let mut group_wins_a = 0u8;
        let mut group_wins_b = 0u8;
        let mut groups_played = 0u8;
        let mut last_base_a = [0u64; 9];
        let mut last_base_b = [0u64; 9];
        let mut last_fin_a = [0u64; 9];
        let mut last_fin_b = [0u64; 9];

        for g in 0..3usize {
            groups_played = (g + 1) as u8;

            // 阶段 2：OnGroupStart（A 先 B 后）
            self.phase_simple(SkillTrigger::OnGroupStart)?;

            // 阶段 3：base v1 + 预战力（供 OnLow/HighPower 判定）
            let base_a1 = self.all_base(Side::A)?;
            let base_b1 = self.all_base(Side::B)?;
            let pre_a = self.all_final(&base_a1, Side::A, bond_a)?;
            let pre_b = self.all_final(&base_b1, Side::B, bond_b)?;

            // 阶段 4：OnLowPower / OnHighPower（按预战力每卡对位判一次）
            self.phase_power(g, &pre_a, &pre_b)?;

            // 阶段 5：base v2 + 终战力（技能修改后重算）
            let base_a2 = self.all_base(Side::A)?;
            let base_b2 = self.all_base(Side::B)?;
            let fin_a = self.all_final(&base_a2, Side::A, bond_a)?;
            let fin_b = self.all_final(&base_b2, Side::B, bond_b)?;

            // 阶段 6：组内 best-of-3（final → base → 真平局）
            let mut sa = 0u8;
            let mut sb = 0u8;
            for j in 0..3 {
                let i = g * 3 + j;
                let (fa, fb) = (fin_a[i], fin_b[i]);
                let (ba, bb) = (base_a2[i], base_b2[i]);
                if fa > fb {
                    sa += 1;
                } else if fb > fa {
                    sb += 1;
                } else if ba > bb {
                    sa += 1;
                } else if bb > ba {
                    sb += 1;
                }
                // final 与 base 均相等 → 真平局，双方不记分
            }
            group_scores_a[g] = sa;
            group_scores_b[g] = sb;
            let a_won = sa >= 2;
            let b_won = sb >= 2;
            if a_won {
                group_wins_a += 1;
            }
            if b_won {
                group_wins_b += 1;
            }
            // 1:1 / 0:0 / 1:0 → 双方均不胜（G2：无人拿到 2 胜）

            for i in 0..9 {
                last_base_a[i] = to_u64(base_a2[i])?;
                last_base_b[i] = to_u64(base_b2[i])?;
                last_fin_a[i] = to_u64(fin_a[i])?;
                last_fin_b[i] = to_u64(fin_b[i])?;
            }

            if group_wins_a >= 2 || group_wins_b >= 2 {
                break; // 先赢 2 组即结束
            }

            // 阶段 7：OnWinGroup / OnLoseGroup 挂到下一组
            self.phase_group_outcome(a_won, b_won)?;
        }

        let winner = if group_wins_a >= 2 {
            Winner::A
        } else if group_wins_b >= 2 {
            Winner::B
        } else {
            Winner::Draw // G3：3 组打满 1:1 → 整场平局
        };

        Ok(BattleResult {
            winner,
            group_scores_a,
            group_scores_b,
            group_wins_a,
            group_wins_b,
            net_groups_a: group_wins_a as i8 - group_wins_b as i8,
            net_groups_b: group_wins_b as i8 - group_wins_a as i8,
            groups_played,
            a_final_powers: last_fin_a.to_vec(),
            b_final_powers: last_fin_b.to_vec(),
            a_base_powers: last_base_a.to_vec(),
            b_base_powers: last_base_b.to_vec(),
            a_self_power: self_a,
            b_self_power: self_b,
            skill_logs: self.skill_logs.clone(),
        })
    }
}

fn to_u64(v: u128) -> Result<u64, ContractError> {
    if v > u64::MAX as u128 {
        Err(ContractError::Overflow)
    } else {
        Ok(v as u64)
    }
}

/// 战斗引擎入口：A/B 各 9 张（编组顺序即 card_ids 顺序，3 张一组 × 3 组）
pub fn compute_battle(
    ctx: &BattleContext,
    a: &[BattleCard; 9],
    b: &[BattleCard; 9],
) -> Result<BattleResult, ContractError> {
    let mut eng = BattleEngine::new(ctx, a, b);
    eng.run()
}

/// AI/关卡种子派生（E4）：seed = SHA256(day_bucket : player : nonce : difficulty)
/// 剔除 tx_idx 与 block.height（同块多笔/重放不翻盘，前端可复算）。
pub fn battle_seed(block_time_secs: u64, player: &str, nonce: u64, difficulty: u64) -> Vec<u8> {
    let day_bucket = block_time_secs / crate::sanguo::state::SECS_PER_DAY;
    let raw = format!("{}:{}:{}:{}", day_bucket, player, nonce, difficulty);
    Sha256::digest(raw.as_bytes()).to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::bonds::BONDS;
    use super::super::skills::SKILLS;

    fn ctx() -> BattleContext {
        BattleContext {
            faction_counter_bp: 11500,
            faction_countered_bp: 9000,
            faction_neutral_bp: 10000,
            troop_counter_bp: 11000,
            troop_countered_bp: 10000,
            troop_neutral_bp: 10000,
            rarity_legend_bp: 11500,
            rarity_epic_bp: 10000,
            rarity_rare_bp: 10000,
            rarity_common_bp: 10000,
            power_flat_cap_bp: 3000,
            eff_lower_bound_bp: 5000,
            skill_strength_bp: 10000,
            bond_strength_bp: 10000,
        }
    }

    /// 无技能/无连携/无克制的填充卡（名字避开全部连携成员）
    fn filler(tag: &str) -> BattleCard {
        BattleCard {
            card_id: format!("f_{}", tag),
            name: format!("filler_{}", tag),
            rarity: "common".to_string(),
            faction: "none".to_string(),
            troop: "none".to_string(),
            cost: 1,
            attack: 10,
            defense: 10,
            star: 1,
            level: 0,
            skill_id: String::new(),
        }
    }

    fn team_with_slot0(mut c0: BattleCard) -> [BattleCard; 9] {
        c0.card_id = "c0".to_string();
        let mut t = [filler("0"), filler("1"), filler("2"), filler("3"), filler("4"),
                     filler("5"), filler("6"), filler("7"), filler("8")];
        t[0] = c0;
        t
    }

    fn guanyu() -> BattleCard {
        BattleCard {
            card_id: "gy".to_string(),
            name: "关羽".to_string(),
            rarity: "legend".to_string(),
            faction: "shu".to_string(),
            troop: "cavalry".to_string(),
            cost: 5,
            attack: 115,
            defense: 85,
            star: 3,
            level: 0,
            skill_id: "wu_sheng".to_string(),
        }
    }

    fn xuchu() -> BattleCard {
        BattleCard {
            card_id: "xc".to_string(),
            name: "许褚".to_string(),
            rarity: "epic".to_string(),
            faction: "wei".to_string(),
            troop: "guard".to_string(),
            cost: 4,
            attack: 95,
            defense: 78,
            star: 3,
            level: 0,
            skill_id: "hu_chi".to_string(),
        }
    }

    /// 设计终稿文档 1 计算示例（G1 取整后）：关羽 239 > 许褚 217
    #[test]
    fn example_guanyu_vs_xuchu() {
        let a = team_with_slot0(guanyu());
        let b = team_with_slot0(xuchu());
        let r = compute_battle(&ctx(), &a, &b).unwrap();
        assert_eq!(r.a_final_powers[0], 239, "关羽 final 应为 239（G1 取整）");
        assert_eq!(r.b_final_powers[0], 217, "许褚 final 应为 217（G1 取整）");
        assert_eq!(r.a_base_powers[0], 232, "关羽 base 应为 232（147+85）");
        assert_eq!(r.b_base_powers[0], 173, "许褚 base 应为 173");
        // slot0：关羽胜；其余 8 槽双方填充卡全平 → 每组 1:0 → 无人拿 2 胜 → 整场平局（G2/G3）
        assert_eq!(r.group_scores_a[0], 1);
        assert_eq!(r.group_scores_b[0], 0);
        assert!(matches!(r.winner, Winner::Draw));
        assert_eq!(r.groups_played, 3);
    }

    /// V2.1：技能触发日志——只观测、不改结算；并记录到实际触发的技能。
    /// 关羽 base(232) > 许褚 base(173)，故许褚 OnHighPower 不触发；仅关羽 OnBattleStart 触发。
    #[test]
    fn skill_log_records_triggers() {
        let a = team_with_slot0(guanyu()); // A 方关羽：wu_sheng(OnBattleStart, +2800 攻)
        let b = team_with_slot0(xuchu()); // B 方许褚：hu_chi(OnHighPower, +2200 防)
        let r = compute_battle(&ctx(), &a, &b).unwrap();
        assert!(!r.skill_logs.is_empty(), "应记录到至少 1 条技能触发");
        let entry = &r.skill_logs[0];
        assert_eq!(entry.side, 0, "关羽在 A 方");
        assert_eq!(entry.skill_id, "wu_sheng");
        assert_eq!(entry.trigger, SkillTrigger::OnBattleStart);
        assert_eq!(entry.attack_bp, 2800, "日志记录技能名义 bp（应用前采集）");
        // 日志为纯观测：结算数值不变，关羽终战力仍 239（G1）
        assert_eq!(r.a_final_powers[0], 239);
    }

    /// 同卡对同卡 → 全平局
    #[test]
    fn mirror_teams_draw() {
        let a = team_with_slot0(guanyu());
        let b = team_with_slot0(guanyu());
        let r = compute_battle(&ctx(), &a, &b).unwrap();
        assert!(matches!(r.winner, Winner::Draw));
        assert_eq!(r.a_self_power, r.b_self_power);
    }

    /// 强力卡放满一组应赢下该组并先赢 2 组结束
    ///
    /// ⚠️ 2026-10-21 修正（公开模板化）：本测试原断言 `winner == A`，
    ///    但**数据构造与断言互相矛盾**，从建立起就永远失败：
    ///    * 原构造只把 3 张关羽放在 slot 0/1/2 —— 全部落在**第 1 组**
    ///      （编组 = 3 张一组 × 3 组，组内 best-of-3 需赢 2 小局才算赢下该组）；
    ///    * A 侧其余 6 张是 filler(攻10/防10)，与 B 侧 filler **完全同值** →
    ///      组内 `fa > fb` 与 `fb > fa` 均不成立、base 也不等 → 真平局
    ///      （裁定书 G2：平局组双方均不计胜）；
    ///    * 于是 A 只赢第 1 组（group_wins_a = 1），第 2/3 组皆平，
    ///      3 组打满无人先赢 2 组 → 按裁定书 G3 判**整场平局**。
    ///    → 引擎行为本就正确，**错的是测试的期望值**。
    ///
    ///    修正方式：让 A 的**前两组**（slot 0-5）各放满 3 张强卡，
    ///    这样 A 连赢第 1、2 组 → 在第 2 组后提前结束，
    ///    恰好验证本测试真正想验证的语义（先赢 2 组 → 提前结束）。
    #[test]
    fn strong_team_wins_early_stop() {
        // A：slot 0-5 各放关羽（第 1、2 组满强卡）；B：全 filler
        let mut a = team_with_slot0(guanyu());
        for (k, idx) in [1usize, 2, 3, 4, 5].iter().enumerate() {
            let mut g = guanyu();
            g.card_id = format!("gy{}", k + 1);
            a[*idx] = g;
        }
        let b = team_with_slot0(filler("x"));
        let r = compute_battle(&ctx(), &a, &b).unwrap();
        assert!(matches!(r.winner, Winner::A));
        assert_eq!(r.groups_played, 2, "先赢 2 组应在第 2 组后结束");
        assert_eq!(r.group_wins_a, 2);
    }

    /// E3 溢出防护
    ///
    /// ⚠️ 2026-10-21 修正（公开模板化）：本测试原断言 `Err(Overflow)`，
    ///    但**该断言在类型上就不可达**，从建立起就永远失败：
    ///    * `BattleCard.attack` / `.defense` 是 `u32`，上限 4_294_967_295；
    ///    * `base_power` 用 u128 计算 → base ≤ 2×4.29e9 ≈ 8.59e9；
    ///    * `check_base_bound` 的上界 bound = u64::MAX×10⁴ ÷ (11500×11000×11800×11500)
    ///      ≈ 1.07e19 —— 比 base **大 9 个数量级**，故 base 永远 ≤ bound，
    ///      溢出分支 `if base > bound` **永不进入**。
    ///    → 引擎的溢出防护是正确的（u128 内部量 + u64 输出钳制 + to_u64 兜底），
    ///      **错的是测试选了不可能触发的输入**。
    ///
    ///    修正方式：把测试拆成两层，都验证**真实可达**的不变量：
    ///    ① `u32::MAX` 的攻防**不会**溢出（引擎在 u128 域内安全消化）；
    ///    ② `to_u64` 钳制：超过 u64::MAX 的值必须被拒（这是真正可达的溢出防线）。
    #[test]
    fn overflow_guard() {
        // ① u32 上限的攻防：u128 内部量足够消化，不应溢出
        let mut huge = guanyu();
        huge.attack = u32::MAX;
        huge.defense = u32::MAX;
        let a = team_with_slot0(huge);
        let b = team_with_slot0(filler("x"));
        let r = compute_battle(&ctx(), &a, &b);
        assert!(
            r.is_ok(),
            "u32::MAX 的攻防在 u128 域内可安全计算，不应触发 Overflow（实测 {:?}）",
            r.err()
        );

        // ② 真正可达的溢出防线：超过 u64::MAX 的战力必须被拒
        assert!(matches!(to_u64(u64::MAX as u128 + 1), Err(ContractError::Overflow)));
        assert!(to_u64(u64::MAX as u128).is_ok(), "u64::MAX 本身应通过");
    }

    /// E2 下限：连续减益不低于原值 50%
    #[test]
    fn eff_lower_bound() {
        // 贾诩 du_shi（OnWinGroup 对手防御-1000）不适用于此测；直接用 apply_to_card 语义验证
        let mut st = CardState::from_card(&xuchu()); // 防 78
        let eff = SkillEffect {
            trigger: SkillTrigger::OnBattleStart,
            attack_bp: 0,
            defense_bp: -9000, // -90%
            power_flat_bp: 0,
            team_wide: false,
            max_stacks: 1,
            target: SkillTarget::Self_,
        };
        apply_to_card(&mut st, &eff, &ctx()).unwrap();
        assert_eq!(st.defense, 39, "78 × 0.5 = 39（封底，不再更低）");
        apply_to_card(&mut st, &eff, &ctx()).unwrap();
        assert_eq!(st.defense, 39, "二次减益仍封底 39");
    }

    /// 常量规模自检（防手滑漏项）
    #[test]
    fn const_scales() {
        assert_eq!(SKILLS.len(), 60);
        assert_eq!(BONDS.len(), 23);
        assert_eq!(MAX_BOND_BP, 11800);
    }

    /// battle_seed 确定性（同输入同输出；day_bucket 变化则变）
    #[test]
    fn seed_deterministic() {
        let s1 = battle_seed(1_700_000_000, "paxi1abc", 7, 3);
        let s2 = battle_seed(1_700_000_000, "paxi1abc", 7, 3);
        assert_eq!(s1, s2);
        let s3 = battle_seed(1_700_000_000 + 86_400, "paxi1abc", 7, 3);
        assert_ne!(s1, s3, "跨天 day_bucket 变化");
        let s4 = battle_seed(1_700_000_000, "paxi1abc", 8, 3);
        assert_ne!(s1, s4, "nonce 变化");
    }
}
