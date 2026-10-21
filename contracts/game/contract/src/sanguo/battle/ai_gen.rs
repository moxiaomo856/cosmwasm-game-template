// ============================================================================
// AI 敌生成（E4 day-bucket 种子）+ 关卡固定敌人（E5）
//
// E4：seed = SHA256(day_bucket : player : nonce : difficulty)，剔除 tx_idx 与
//     block.height —— 同块多笔/重放不翻盘，前端可复算（跨零点战斗按最新块时间复算）。
// E5：stage 敌人 Fixed（来自 STAGES 配置的模板 id + 星级），不随机 ——
//     关卡可复算、可预演、可攻略。
//
// 难度语义（与旧版 ai_battle 一致，移植到全量引擎）：
//   · diff_boost_bp：AI 卡攻/防各自 × bp/10000（floor），难度 1..5 递进
//   · ai_floor_bp：AI 该槽 base 战力 ≥ 玩家该槽 raw 战力 × bp/10000（防碾压农场）
//   · AI 卡 star=1（无技能，与旧版纯战力口径一致；难度完全由系数表达）
// ============================================================================

use cosmwasm_std::Uint128;

use crate::error::ContractError;
use crate::games::engines::random_u64;
use crate::sanguo::state::{CardTemplate, StageConfig, SECS_PER_DAY};

use super::compute::{battle_seed, BattleCard};

/// 技能解锁星级（设计终稿文档 2：每张卡 1 个技能，3★ 解锁）
pub const SKILL_UNLOCK_STAR: u8 = 3;

/// 加权模板选择（cum[k] = 前 k+1 个模板权重前缀和；rng ∈ [0, total)）
fn pick_weighted<'a>(
    rng: u64,
    templates: &'a [CardTemplate],
    cum: &[u128],
    total: u128,
) -> &'a CardTemplate {
    let target = rng as u128;
    for (i, c) in cum.iter().enumerate() {
        if target < *c {
            return &templates[i];
        }
    }
    &templates[templates.len() - 1]
}

/// 从模板构造参战卡（星级加成已烘入攻防；3★ 及以上带技能）。
/// AI 卡与关卡敌卡共用；card_id 由调用方覆盖。
pub fn battle_card_from_template(
    tpl: &CardTemplate,
    star: u8,
    atk_boosts: &[u32; 4],
    def_boosts: &[u32; 4],
) -> BattleCard {
    let s = star.clamp(1, 5);
    let mut attack = tpl.attack;
    let mut defense = tpl.defense;
    // star_up 语义：每升 1 星累加一档 boost（1★ 无加成）
    for k in 0..(s - 1) as usize {
        attack = attack.saturating_add(atk_boosts[k]);
        defense = defense.saturating_add(def_boosts[k]);
    }
    let skill_id = if s >= SKILL_UNLOCK_STAR {
        tpl.skill_id.clone()
    } else {
        String::new()
    };
    BattleCard {
        card_id: format!("tpl_{}", tpl.id),
        name: tpl.name.clone(),
        rarity: tpl.rarity.clone(),
        faction: tpl.faction.clone(),
        troop: tpl.troop.clone(),
        cost: tpl.cost,
        attack,
        defense,
        star: s,
        level: 0,
        skill_id,
    }
}

/// 生成 9 张 AI 卡（加权抽模板 + 难度缩放 + 战力地板）。
///
/// `player_cards`：玩家方 9 张（用于按槽位计算 ai_floor_bp 地板）。
/// 返回的卡序即 AI 方编组（3 张一组 × 3 组）。
pub fn generate_ai_cards(
    templates: &[CardTemplate],
    seed: &[u8],
    diff_boost_bp: u64,
    ai_floor_bp: u64,
    player_cards: &[BattleCard; 9],
) -> Result<[BattleCard; 9], ContractError> {
    if templates.is_empty() {
        return Err(ContractError::SanguoNoTemplates);
    }
    // 权重前缀和（weight 已由 AddTemplates 校验在稀有度区间内）
    let mut cum: Vec<u128> = Vec::with_capacity(templates.len());
    let mut total: u128 = 0;
    for t in templates.iter() {
        total = total
            .checked_add(t.weight as u128)
            .ok_or(ContractError::Overflow)?;
        cum.push(total);
    }
    if total == 0 {
        return Err(ContractError::SanguoNoTemplates);
    }

    let mut out: Vec<BattleCard> = Vec::with_capacity(9);
    for slot in 0..9usize {
        // 每槽独立子种子（seed + 槽位），拒绝采样保证均匀
        let mut sub_seed = seed.to_vec();
        sub_seed.extend_from_slice(&(slot as u64).to_le_bytes());
        let rng = random_u64(&sub_seed, total as u64);
        let tpl = pick_weighted(rng, templates, &cum, total);

        // 难度缩放：攻/防各自 × bp/10000（floor）
        let mut attack = (tpl.attack as u64 * diff_boost_bp / 10000) as u32;
        let mut defense = (tpl.defense as u64 * diff_boost_bp / 10000) as u32;

        // 战力地板：AI 该槽 base ≥ 玩家该槽 raw × ai_floor_bp/10000
        let pp = player_cards[slot].attack as u64 + player_cards[slot].defense as u64;
        let floor_target = pp * ai_floor_bp / 10000;
        let ai_base = attack as u64 + defense as u64;
        if ai_base < floor_target {
            // 差额均摊到攻防（确定性；先攻后半防，余数给防）
            let deficit = floor_target - ai_base;
            attack = attack.saturating_add((deficit / 2) as u32);
            defense = defense.saturating_add((deficit - deficit / 2) as u32);
        }

        out.push(BattleCard {
            card_id: format!("ai_s{}", slot),
            name: tpl.name.clone(),
            rarity: tpl.rarity.clone(),
            faction: tpl.faction.clone(),
            troop: tpl.troop.clone(),
            cost: tpl.cost,
            attack,
            defense,
            star: 1, // AI 无技能（难度完全由系数表达）
            level: 0,
            skill_id: String::new(),
        });
    }
    let arr: [BattleCard; 9] = out
        .try_into()
        .map_err(|_| ContractError::SanguoNoTemplates)?;
    Ok(arr)
}

/// 关卡敌人（E5：Fixed）—— 来自 STAGES 配置的 9 个模板 id + 星级。
/// 敌人带技能当且仅当星级 ≥ 3（与玩家同规则）。
pub fn stage_enemy_cards(
    templates: &[CardTemplate],
    stage: &StageConfig,
    atk_boosts: &[u32; 4],
    def_boosts: &[u32; 4],
) -> Result<[BattleCard; 9], ContractError> {
    if stage.enemy_tpl_ids.len() != 9 {
        return Err(ContractError::SanguoStageEnemyNotFound(format!(
            "stage {} enemy_tpl_ids must be 9, got {}",
            stage.id,
            stage.enemy_tpl_ids.len()
        )));
    }
    // enemy_stars 缺省（旧数据 serde default）按 1★ 处理
    let stars: Vec<u8> = if stage.enemy_stars.len() == 9 {
        stage.enemy_stars.clone()
    } else {
        vec![1u8; 9]
    };
    let mut out: Vec<BattleCard> = Vec::with_capacity(9);
    for (i, tid) in stage.enemy_tpl_ids.iter().enumerate() {
        let tpl = templates
            .iter()
            .find(|t| t.id == *tid)
            .ok_or_else(|| ContractError::SanguoStageEnemyNotFound(tid.clone()))?;
        let mut c = battle_card_from_template(tpl, stars[i], atk_boosts, def_boosts);
        c.card_id = format!("stage{}_{}", stage.id, tid);
        out.push(c);
    }
    let arr: [BattleCard; 9] = out
        .try_into()
        .map_err(|_| ContractError::SanguoStageEnemyNotFound(stage.id.to_string()))?;
    Ok(arr)
}

/// AI 种子便捷构造（E4）：day_bucket = block_time / SECS_PER_DAY
pub fn ai_seed(block_time_secs: u64, player: &str, nonce: u64, difficulty: u64) -> Vec<u8> {
    battle_seed(block_time_secs, player, nonce, difficulty)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tpl(id: &str, name: &str, rarity: &str, atk: u32, def: u32, weight: u32) -> CardTemplate {
        CardTemplate {
            id: id.to_string(),
            name: name.to_string(),
            rarity: rarity.to_string(),
            attack: atk,
            defense: def,
            weight,
            faction: "wei".to_string(),
            troop: "cavalry".to_string(),
            cost: 3,
            skill_id: "jian_xiong".to_string(),
            bonds: vec![],
        }
    }

    fn boosts() -> [u32; 4] {
        [5, 12, 25, 50]
    }
    fn def_boosts() -> [u32; 4] {
        [3, 8, 18, 35]
    }

    #[test]
    fn star_boosts_cumulative() {
        let t = tpl("t1", "测试", "epic", 100, 100, 12);
        // 1★ 无加成
        let c1 = battle_card_from_template(&t, 1, &boosts(), &def_boosts());
        assert_eq!((c1.attack, c1.defense), (100, 100));
        assert_eq!(c1.skill_id, "", "1★ 无技能");
        // 3★ = 1★ + boost[0] + boost[1] = 100+5+12 / 100+3+8
        let c3 = battle_card_from_template(&t, 3, &boosts(), &def_boosts());
        assert_eq!((c3.attack, c3.defense), (117, 111));
        assert_eq!(c3.skill_id, "jian_xiong", "3★ 带技能");
        // 5★ = 加满四档
        let c5 = battle_card_from_template(&t, 5, &boosts(), &def_boosts());
        assert_eq!((c5.attack, c5.defense), (192, 164));
    }

    #[test]
    fn ai_generation_deterministic_and_floored() {
        let templates = vec![
            tpl("a", "甲", "common", 40, 40, 40),
            tpl("b", "乙", "epic", 90, 80, 12),
        ];
        let player: Vec<BattleCard> = (0..9)
            .map(|i| BattleCard {
                card_id: format!("p{}", i),
                name: format!("p{}", i),
                rarity: "common".to_string(),
                faction: "wei".to_string(),
                troop: "cavalry".to_string(),
                cost: 3,
                attack: 100,
                defense: 100,
                star: 3,
                level: 0,
                skill_id: String::new(),
            })
            .collect();
        let player: [BattleCard; 9] = player.try_into().unwrap();

        let seed = ai_seed(1_700_000_000, "paxi1x", 1, 3);
        let cards = generate_ai_cards(&templates, &seed, 14000, 9300, &player).unwrap();
        assert_eq!(cards.len(), 9);
        // 地板：玩家每槽 raw=200，floor 93% → AI base ≥ 186
        for c in cards.iter() {
            assert!(c.attack as u64 + c.defense as u64 >= 186, "floor not applied");
            assert_eq!(c.star, 1);
            assert_eq!(c.skill_id, "");
        }
        // 确定性：同种子同结果
        let cards2 = generate_ai_cards(&templates, &seed, 14000, 9300, &player).unwrap();
        assert_eq!(
            cards.iter().map(|c| c.name.clone()).collect::<Vec<_>>(),
            cards2.iter().map(|c| c.name.clone()).collect::<Vec<_>>()
        );
    }

    #[test]
    fn stage_enemy_fixed() {
        let templates = vec![tpl("e1", "敌一", "epic", 90, 80, 12)];
        let stage = StageConfig {
            id: 1,
            name: "第一关".to_string(),
            enemy_tpl_ids: vec!["e1".to_string(); 9],
            enemy_stars: vec![3u8; 9],
            reward: Default::default(),
        };
        let cards = stage_enemy_cards(&templates, &stage, &boosts(), &def_boosts()).unwrap();
        for c in cards.iter() {
            assert_eq!(c.name, "敌一");
            assert_eq!(c.star, 3);
            assert_eq!(c.skill_id, "jian_xiong");
        }
        // 缺 enemy_stars（旧数据）→ 全 1★
        let stage_old = StageConfig {
            id: 1,
            name: "旧关".to_string(),
            enemy_tpl_ids: vec!["e1".to_string(); 9],
            enemy_stars: vec![],
            reward: Default::default(),
        };
        let cards = stage_enemy_cards(&templates, &stage_old, &boosts(), &def_boosts()).unwrap();
        assert_eq!(cards[0].star, 1);
        assert_eq!(cards[0].skill_id, "");
    }
}
