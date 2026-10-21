// ============================================================================
// 三国卡牌 V2 执行层（裁定书第 3/4/7/8/9 章，2026-10-13）
//
// 合规总闸硬化（T24 / C1~C6）：
//   C1 COMPLIANCE_MODE 开关已删——合规是唯一路径，本文件零 is_compliance 调用；
//   C2 抽卡/战斗全免费入场，奖励只发统一碎片 FRAGMENT_BALANCE；
//   C3 SanguoConfig 三字段废弃（存而不读），draw_pack 的 tap_index 忽略；
//   C4 SanguoParams deprecated 字段不读取（star_up 删除 TKCC 分支）；
//   C5 PROJECT_WALLETS 保留（PAXI 明码标价付费的收款通道）；
//   C6 set_payment_token 已放开（2026-10-14 用户拍板）：管理员可设任意
//      白名单内 PRC-20 为支付代币（TKCC 等），收款走 collect_payment 的
//      PRC-20 分支（内部余额扣款 + Wasm transfer）；空串 = 原生 PAXI。
//
// 碎片 9.1 P0 经济闭环：全部碎片产出/赠送/消耗统一走 FRAGMENT_BALANCE——
//   产出：grant_fragments_unified（运营批量/关卡/战斗/升星返还等全部入口）
//   消耗：spend_fragments（升星/升级/合成/商店碎片价）
//   转移：gift.rs gift_fragments（读 frag_balance / set_frag_balance）
//   旧四桶 FRAGMENTS 只作 migrate M6 的迁移源，V2 起零写入。
//
// 战斗：统一走 battle::compute_battle 引擎（E1~E3 取整/下限/溢出、
//   G2/G3 平局语义、G4 Royale 三级排序）；AI 侧 ai_gen（E4 day-bucket 种子、
//   E5 关卡 Fixed 敌人）。玩家卡星级/等级加成已烘入 card.attack/defense，
//   引擎不二次加成；技能 3★ 起生效（SKILL_UNLOCK_STAR）。
//
// 写码对账注记（留专家复核）：
//   ① M4 原文「migrate 清空 PLAYER_BATTLE_ORDER」与 M5「按 PLAYER_BATTLE_ORDER
//     镜像补偿」存在顺序冲突——实现改为：migrate 本体不清编队，M5 分批时
//     逐玩家补偿后清除（天然一次性语义）。
//   ② M5 补偿标记复用 PLAYER_BATTLE_ORDER 清除动作，未新增计数存储。
//   ③ COLLECTION 图鉴 key 用模板名（CardInfo 无模板 id 字段）。
//   ④ upgrade（普通升级）消耗沿用 V1 语义 = params.star_up_fragment_cost()。
// ============================================================================

use cosmwasm_std::{
    Addr, BankMsg, Coin, CosmosMsg, DepsMut, Env, MessageInfo, Order, Response, StdResult, Storage,
    Uint128, WasmMsg, to_json_binary,
};
use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet};

use crate::error::ContractError;
use crate::msg::SanguoMigrationWhitelistResponse;
use crate::msg::{AiMode, FragmentGrant, PvpAction, PvpMode};
use crate::admin::config::is_admin;
use crate::sanguo::state::*;
use crate::sanguo::helpers::*;
use crate::sanguo::battle::ai_gen::{
    generate_ai_cards, stage_enemy_cards, SKILL_UNLOCK_STAR,
};
use crate::sanguo::battle::{
    compute_battle, BattleCard, BattleContext, SkillTriggerLog, Winner,
};
use crate::state::{BALANCES, empty_addr, PAYMENT_TOKEN, PRC20_WHITELIST};

// ============================================================================
// 全局封禁 / 玩法暂停（T8 + C 档）
// ============================================================================

/// 全局封禁检查（所有玩法入口前置；赠送入口在 gift.rs 另有同源检查）
pub fn ensure_not_banned_global(storage: &dyn Storage, p: &Addr) -> Result<(), ContractError> {
    if BANNED_PLAYERS.may_load(storage, p)?.unwrap_or(false) {
        return Err(ContractError::SanguoBanned);
    }
    Ok(())
}

/// 单玩法紧急暂停检查（C 档 SanguoPauseGame）
pub fn ensure_game_active(storage: &dyn Storage, game_id: &str) -> Result<(), ContractError> {
    let paused = PAUSED_GAMES.may_load(storage)?.unwrap_or_default();
    if paused.iter().any(|g| g == game_id) {
        return Err(ContractError::GameDisabled(game_id.to_string()));
    }
    Ok(())
}

// ============================================================================
// 统一碎片币（T7 经济闭环核心）
// ============================================================================

/// 读统一碎片余额
pub fn frag_balance(storage: &dyn Storage, player: &Addr) -> u128 {
    FRAGMENT_BALANCE
        .may_load(storage, player)
        .ok()
        .flatten()
        .unwrap_or(0)
}

/// 写统一碎片余额（gift.rs 转移共用）
pub fn set_frag_balance(storage: &mut dyn Storage, player: &Addr, v: u128) -> StdResult<()> {
    FRAGMENT_BALANCE.save(storage, player, &v)
}

/// 消耗统一碎片（无日限——日限只约束产出渠道）
fn spend_fragments(
    storage: &mut dyn Storage,
    player: &Addr,
    amount: u128,
) -> Result<(), ContractError> {
    if amount == 0 {
        return Ok(());
    }
    let bal = frag_balance(storage, player);
    if bal < amount {
        return Err(ContractError::SanguoInsufficientFragments {
            needed: amount.min(u64::MAX as u128) as u64,
            have: bal.min(u64::MAX as u128) as u64,
        });
    }
    set_frag_balance(storage, player, bal - amount)?;
    Ok(())
}

/// 发放统一碎片（产出出口，强制 params.daily_fragment_cap 每日硬顶；
/// 冷静期统一记 FRAG_ACQUIRED[(player,"unified")]）
fn grant_fragments_unified(
    deps: &mut DepsMut,
    env: &Env,
    player: &Addr,
    amount: u128,
) -> Result<(), ContractError> {
    if amount == 0 {
        return Ok(());
    }
    let params = load_params(deps.storage)?;
    let cap = params.daily_fragment_cap();
    // 🟢 审计修复（P1）：接线全局日赔付上限 check_and_add_global_payout——
    //    此前 GLOBAL_DAILY_PAYOUT_CAP 存而不读，多钱包绕过玩家日限的
    //    "最后一道防线"从未生效。
    check_and_add_global_payout(
        deps.branch(),
        env.block.time.seconds(),
        Uint128::from(amount),
    )?;
    let today = env.block.time.seconds() / SECS_PER_DAY * SECS_PER_DAY;
    let last_day = SG_GRANT_DATE.may_load(deps.storage, player)?.unwrap_or(0);
    let granted_today = if last_day == today {
        SG_GRANT_AMOUNT.may_load(deps.storage, player)?.unwrap_or(0)
    } else {
        0
    };
    let requested = u64::try_from(amount).unwrap_or(u64::MAX);
    if granted_today.saturating_add(requested) > cap {
        return Err(ContractError::SanguoFragmentCapReached {
            granted: granted_today,
            requested,
            cap,
        });
    }
    let bal = frag_balance(deps.storage, player);
    set_frag_balance(deps.storage, player, bal.saturating_add(amount))?;
    FRAG_ACQUIRED.save(deps.storage, (player, "unified"), &env.block.time.seconds())?;
    SG_GRANT_DATE.save(deps.storage, player, &today)?;
    SG_GRANT_AMOUNT.save(deps.storage, player, &granted_today.saturating_add(requested))?;
    Ok(())
}

/// 四桶奖励表 → 统一碎片发放（to_unified 折算；全部旧调用点收敛至此）
fn grant_fragments_checked(
    deps: &mut DepsMut,
    env: &Env,
    player: &Addr,
    frags: &Fragments,
) -> Result<(), ContractError> {
    grant_fragments_unified(deps, env, player, frags.to_unified())
}

// ============================================================================
// 管理员：配置三国
// ============================================================================

pub fn set_config(
    deps: DepsMut,
    info: MessageInfo,
    tkcc_token: String,
    burn_address: String,
    tap_addresses: Vec<String>,
) -> Result<Response, ContractError> {
    if !is_admin(deps.as_ref(), &info.sender)? {
        return Err(ContractError::Unauthorized);
    }
    // C3 硬化（对齐 C6 set_payment_token）：三字段全废弃——传非空（非空串 / 非空数组）
    // 一律拒绝 ComplianceDisabled；空值放行并写空配置（migrate M2 直写存储，不走本入口）。
    if !tkcc_token.trim().is_empty()
        || !burn_address.trim().is_empty()
        || !tap_addresses.is_empty()
    {
        return Err(ContractError::ComplianceDisabled);
    }
    SANGUO_CONFIG.save(
        deps.storage,
        &SanguoConfig {
            tkcc_token: Addr::unchecked(""),
            burn_address: Addr::unchecked(""),
            tap_addresses: vec![],
        },
    )?;
    Ok(Response::new()
        .add_attribute("action", "sanguo_set_config")
        .add_attribute("deprecated", "true"))
}

pub fn set_params(
    deps: DepsMut,
    info: MessageInfo,
    mut params: SanguoParams,
) -> Result<Response, ContractError> {
    if !is_admin(deps.as_ref(), &info.sender)? {
        return Err(ContractError::Unauthorized);
    }
    // T4 数值加固：天梯 i64 字段禁止为负（负值会破坏积分累计 / 升档判定；
    //   0 仍表示「回退默认」，故只拦 < 0）。
    if params.rank_win_points < 0
        || params.rank_lose_points < 0
        || params.rank_tier_up_points < 0
    {
        return Err(ContractError::InvalidParams {
            reason: "rank points must be non-negative".to_string(),
        });
    }
    // 🟢 参数收敛护栏：技能/连携强度系数上限 3×（30000bp）——防止误填超大值
    //    把技能/连携倍率放大到突破 E3 溢出前置检查的假设范围
    if params.skill_strength_bp > 30_000 || params.bond_strength_bp > 30_000 {
        return Err(ContractError::InvalidParams {
            reason: "skill/bond strength bp must be <= 30000 (3x)".to_string(),
        });
    }
    // V2：deprecated 字段不再校验/不再读取（C4）；生效字段 0 = 回退默认，
    // 无需强校验。保存即生效。
    //
    // 🟢 外部审计 #2/#11 修复（2026-10-14）：deprecated 计费字段强制归零——
    //    set_params 不再信任入参，无论旧配置 JSON / 线上残留是否非零，
    //    落库前统一清零。杜绝「存了非零但 exec 不读」的审计噪音与误解。
    params.single_paxi_fee = 0;
    params.pack3_paxi_fee = 0;
    params.single_prc_total = 0;
    params.single_prc_eco = 0;
    params.single_prc_burn = 0;
    params.pack3_prc_total = 0;
    params.pack3_prc_eco = 0;
    params.pack3_prc_burn = 0;
    params.ai_fee = [0u128; 5];
    params.ai_reward = [0u128; 5];
    params.ai_burn_pct_bp = 0;
    params.upgrade_fees = [0u128; 4];
    params.pvp_fee = 0;
    params.royale_entry_fee = 0;
    params.royale_reward_pct_bp = 0;
    params.royale_burn_pct_bp = 0;
    params.proposal_deposit = 0;
    params.voting_period = 0;
    SANGUO_PARAMS.save(deps.storage, &params)?;
    Ok(Response::new()
        .add_attribute("action", "sanguo_set_params")
        .add_attribute("daily_ai_limit", params.daily_ai_limit.to_string()))
}

/// 管理员：设置全局日赔付上限（防多钱包绕过玩家日限额抽干池子）
pub fn set_global_daily_payout_cap(
    deps: DepsMut,
    info: MessageInfo,
    cap: Uint128,
) -> Result<Response, ContractError> {
    if !is_admin(deps.as_ref(), &info.sender)? {
        return Err(ContractError::Unauthorized);
    }
    if cap.is_zero() {
        return Err(ContractError::SanguoInvalidAmount);
    }
    GLOBAL_DAILY_PAYOUT_CAP.save(deps.storage, &cap)?;
    Ok(Response::new()
        .add_attribute("action", "sanguo_set_global_daily_payout_cap")
        .add_attribute("cap", cap.to_string()))
}

pub fn add_templates(
    deps: DepsMut,
    info: MessageInfo,
    templates: Vec<CardTemplate>,
) -> Result<Response, ContractError> {
    if !is_admin(deps.as_ref(), &info.sender)? {
        return Err(ContractError::Unauthorized);
    }
    let mut list = CARD_TEMPLATES.may_load(deps.storage)?.unwrap_or_default();
    let mut rarity_counts = collect_rarity_counts(deps.storage);
    let rarity_max = load_params(deps.storage)?.rarity_max();
    let mut total = list.len() as u32;
    let mut added = 0u32;
    let mut skipped = 0u32;
    for t in templates {
        if list.iter().any(|x| x.id == t.id) {
            skipped += 1;
            continue;
        }
        validate_card_template_internal(&rarity_counts, &t.rarity, t.weight, total, &rarity_max)
            .map_err(ContractError::SanguoTemplateError)?;
        *rarity_counts.entry(t.rarity.clone()).or_insert(0) += 1;
        total += 1;
        list.push(t);
        added += 1;
    }
    for (r, cnt) in &rarity_counts {
        RARITY_COUNT.save(deps.storage, r.as_str(), cnt)?;
    }
    CARD_TEMPLATE_COUNT.save(deps.storage, &(list.len() as u64))?;
    CARD_TEMPLATES.save(deps.storage, &list)?;
    Ok(Response::new()
        .add_attribute("action", "sanguo_add_templates")
        .add_attribute("added", added.to_string())
        .add_attribute("skipped", skipped.to_string())
        .add_attribute("total", list.len().to_string()))
}

/// C 档：60 张 × 11 字段整表覆盖写（裁定 8.1；卡片 id 在表内必须唯一，
/// 稀有度分布不得超 params.rarity_max）。旧数据整体替换，不做增量。
pub fn set_cards_batch(
    deps: DepsMut,
    info: MessageInfo,
    cards: Vec<CardTemplate>,
) -> Result<Response, ContractError> {
    if !is_admin(deps.as_ref(), &info.sender)? {
        return Err(ContractError::Unauthorized);
    }
    let mut counts: BTreeMap<String, u64> = BTreeMap::new();
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let rarity_max = load_params(deps.storage)?.rarity_max();
    for t in &cards {
        let idx = Fragments::rarity_index(&t.rarity)
            .ok_or_else(|| ContractError::SanguoInvalidRarity(t.rarity.clone()))?;
        if !seen.insert(t.id.clone()) {
            return Err(ContractError::InvalidParams {
                reason: format!("duplicate template id: {}", t.id),
            });
        }
        let (lo, hi) = RARITY_WEIGHT_RANGE[idx];
        if t.weight < lo || t.weight > hi {
            return Err(ContractError::InvalidParams {
                reason: format!("weight for {} must be in [{}, {}]", t.rarity, lo, hi),
            });
        }
        *counts.entry(t.rarity.clone()).or_insert(0) += 1;
    }
    for (r, c) in &counts {
        let idx = Fragments::rarity_index(r).unwrap_or(0);
        if *c > rarity_max[idx] {
            return Err(ContractError::InvalidParams {
                reason: format!("rarity {} count {} exceeds max {}", r, c, rarity_max[idx]),
            });
        }
    }
    for (r, c) in &counts {
        RARITY_COUNT.save(deps.storage, r.as_str(), c)?;
    }
    // 未覆盖到的稀有度计数清零
    for r in ["common", "rare", "epic", "legend"] {
        if !counts.contains_key(r) {
            RARITY_COUNT.save(deps.storage, r, &0)?;
        }
    }
    CARD_TEMPLATE_COUNT.save(deps.storage, &(cards.len() as u64))?;
    CARD_TEMPLATES.save(deps.storage, &cards)?;
    Ok(Response::new()
        .add_attribute("action", "sanguo_set_cards_batch")
        .add_attribute("total", cards.len().to_string()))
}

// ============================================================================
// 战斗辅助：编队校验（T12）+ BattleCard 装配
// ============================================================================

/// 玩家卡 → BattleCard：星级/等级加成已烘入 card.attack/defense，直接采用；
/// faction/troop/cost/skill_id 从同名模板补齐（缺失模板按空处理）；
/// 技能 3★ 起生效（SKILL_UNLOCK_STAR）。
fn player_battle_card(info: &CardInfo, tpl: Option<&CardTemplate>) -> BattleCard {
    let empty = String::new();
    let skill = if info.star >= SKILL_UNLOCK_STAR {
        tpl.map(|t| t.skill_id.clone()).unwrap_or_default()
    } else {
        empty
    };
    BattleCard {
        card_id: info.card_id.clone(),
        name: info.name.clone(),
        rarity: info.rarity.clone(),
        faction: tpl.map(|t| t.faction.clone()).unwrap_or_default(),
        troop: tpl.map(|t| t.troop.clone()).unwrap_or_default(),
        cost: tpl.map(|t| t.cost).unwrap_or(0),
        attack: info.attack,
        defense: info.defense,
        star: info.star,
        level: info.level,
        skill_id: skill,
    }
}

fn template_map<'a>(
    templates: &'a [CardTemplate],
) -> BTreeMap<&'a str, &'a CardTemplate> {
    templates.iter().map(|t| (t.name.as_str(), t)).collect()
}

/// 校验并装配 9 张出战卡：==9 + 归属 + 去重 + Σcost ≤ params.team_cost_cap()
fn load_battle_team_checked(
    storage: &dyn Storage,
    player: &Addr,
    card_ids: &[String],
) -> Result<[BattleCard; 9], ContractError> {
    if card_ids.len() != 9 {
        return Err(ContractError::SanguoNeed9Cards);
    }
    let owned = PLAYER_CARDS.may_load(storage, player)?.unwrap_or_default();
    let mut set = BTreeSet::new();
    for id in card_ids {
        if !owned.contains(id) {
            return Err(ContractError::SanguoCardNotFound(id.clone()));
        }
        if !set.insert(id.clone()) {
            return Err(ContractError::SanguoDuplicateCard(id.clone()));
        }
    }
    let params = load_params(storage)?;
    let cap = params.team_cost_cap();
    let templates = CARD_TEMPLATES.may_load(storage)?.unwrap_or_default();
    let tmap = template_map(&templates);
    let mut total_cost: u32 = 0;
    let mut out: Vec<BattleCard> = Vec::with_capacity(9);
    for id in card_ids {
        let info = CARDS
            .load(storage, id)
            .map_err(|_| ContractError::SanguoCardNotFound(id.clone()))?;
        let tpl = tmap.get(info.name.as_str()).copied();
        total_cost = total_cost.saturating_add(tpl.map(|t| t.cost as u32).unwrap_or(0));
        out.push(player_battle_card(&info, tpl));
    }
    if total_cost > cap {
        return Err(ContractError::SanguoTeamCostExceeded { total: total_cost, cap });
    }
    out.try_into()
        .map_err(|_| ContractError::SanguoNeed9Cards)
}

/// V2.1：分模式战绩累计（首页「总对战/胜场」= 四模式合计）。
/// mode: 0=AI（休闲+天梯） 1=PVP 单挑 2=混战 3=关卡。
/// 注意：PVP/混战每位参战方各计 1 场（胜方 wins+1，平局双方仅 total）；
/// 关卡只有通关（tx 不回滚）才会走到这里，故 stage 恒 win=true 语义。
fn add_battle_stat(
    deps: &mut DepsMut,
    player: &Addr,
    mode: u8,
    win: bool,
) -> Result<(), ContractError> {
    let mut s = AI_BATTLE_STATS
        .may_load(deps.storage, player)?
        .unwrap_or_default();
    match mode {
        0 => {
            s.total = s.total.saturating_add(1);
            if win {
                s.wins = s.wins.saturating_add(1);
            }
        }
        1 => {
            s.pvp_total = s.pvp_total.saturating_add(1);
            if win {
                s.pvp_wins = s.pvp_wins.saturating_add(1);
            }
        }
        2 => {
            s.royale_total = s.royale_total.saturating_add(1);
            if win {
                s.royale_wins = s.royale_wins.saturating_add(1);
            }
        }
        _ => {
            s.stage_total = s.stage_total.saturating_add(1);
            if win {
                s.stage_wins = s.stage_wins.saturating_add(1);
            }
        }
    }
    AI_BATTLE_STATS.save(deps.storage, player, &s)?;
    Ok(())
}

/// V2.1：将战斗技能触发日志序列化为事件属性值（JSON 数组）。
/// 仅观测数据，序列化失败兜底空数组，绝不阻断主流程 / 不改动结算。
fn skill_logs_json(logs: &[SkillTriggerLog]) -> String {
    serde_json::to_string(logs).unwrap_or_else(|_| "[]".to_string())
}

/// 已校验场景（创建时校验过、结算时仅重读）的装配：不重复做归属/去重/cost 校验，
/// 但卡若已被分解/赠送会在这里报 NotFound（原子回滚）。
fn load_battle_team(
    storage: &dyn Storage,
    card_ids: &[String],
) -> Result<[BattleCard; 9], ContractError> {
    if card_ids.len() != 9 {
        return Err(ContractError::SanguoNeed9Cards);
    }
    let templates = CARD_TEMPLATES.may_load(storage)?.unwrap_or_default();
    let tmap = template_map(&templates);
    let mut out: Vec<BattleCard> = Vec::with_capacity(9);
    for id in card_ids {
        let info = CARDS
            .load(storage, id)
            .map_err(|_| ContractError::SanguoCardNotFound(id.clone()))?;
        out.push(player_battle_card(&info, tmap.get(info.name.as_str()).copied()));
    }
    out.try_into()
        .map_err(|_| ContractError::SanguoNeed9Cards)
}

/// 解析玩家出战卡：优先用预设顺序，不足 9 张按战力补（养成/迁移等非战斗场景用）
pub fn resolve_player_battle_cards(
    deps: &dyn Storage,
    player: &Addr,
) -> StdResult<Vec<CardInfo>> {
    let owned_ids = PLAYER_CARDS.may_load(deps, player)?.unwrap_or_default();
    let preset = PLAYER_BATTLE_ORDER
        .may_load(deps, player)?
        .unwrap_or_default();
    let mut result: Vec<CardInfo> = Vec::with_capacity(9);
    let mut seen = BTreeSet::new();
    for id in preset {
        if !owned_ids.contains(&id) || seen.contains(&id) {
            continue;
        }
        if let Some(c) = CARDS.may_load(deps, &id)? {
            seen.insert(id);
            result.push(c);
            if result.len() >= 9 {
                break;
            }
        }
    }
    if result.len() < 9 {
        let mut owned_cards: Vec<CardInfo> = Vec::with_capacity(owned_ids.len());
        for id in &owned_ids {
            if seen.contains(id) {
                continue;
            }
            if let Some(c) = CARDS.may_load(deps, id)? {
                owned_cards.push(c);
            }
        }
        owned_cards.sort_by(|a, b| {
            let pa = a.attack as u64 + a.defense as u64;
            let pb = b.attack as u64 + b.defense as u64;
            pb.cmp(&pa)
        });
        for c in owned_cards {
            if result.len() >= 9 {
                break;
            }
            result.push(c);
        }
    }
    Ok(result)
}

fn card_power(c: &CardInfo) -> u64 {
    c.attack as u64 + c.defense as u64
}

// ============================================================================
// 抽卡（免费 + 保底；碎片加购走商店 draw 臂）
// ============================================================================

/// 累计权重 + 二分（性能审计 TOP#1：避免每次抽卡分配 ~4500 元素 Vec 的等价展开）
fn build_cumulative(templates: &[CardTemplate]) -> (Vec<u128>, u128) {
    let mut cum = Vec::with_capacity(templates.len());
    let mut acc: u128 = 0;
    for t in templates {
        acc = acc.saturating_add(t.weight as u128);
        cum.push(acc);
    }
    (cum, acc)
}

fn pick_template<'a>(rng: u64, templates: &'a [CardTemplate], cum: &[u128], total: u128) -> &'a CardTemplate {
    if total == 0 {
        return &templates[0];
    }
    let target = (rng as u128) % total;
    let idx = cum.partition_point(|&c| c <= target);
    &templates[idx.min(templates.len() - 1)]
}

fn draw_cards(
    deps: &mut DepsMut,
    env: &Env,
    player: &Addr,
    num: u8,
    pity_threshold: u32,
) -> Result<(Vec<String>, Vec<String>), ContractError> {
    let templates = CARD_TEMPLATES
        .may_load(deps.storage)?
        .unwrap_or_default();
    if templates.is_empty() {
        return Err(ContractError::SanguoNoTemplates);
    }
    let (cum, total) = build_cumulative(&templates);
    let legends: Vec<&CardTemplate> =
        templates.iter().filter(|t| t.rarity == "legend").collect();

    let tx_idx = env.transaction.as_ref().map(|t| t.index as u64).unwrap_or(0);
    let player_salt = player
        .as_bytes()
        .iter()
        .fold(0u64, |acc, &b| acc.wrapping_add(b as u64))
        .wrapping_add(tx_idx);
    let mut rng_state: u64 = mix_seed(
        env.block.time.seconds(),
        env.block.height,
        player_salt,
    );
    let mut new_ids: Vec<String> = Vec::new();
    let mut new_names: Vec<String> = Vec::new();
    let mut owned = PLAYER_CARDS
        .may_load(deps.storage, player)?
        .unwrap_or_default();

    // 保底：PITY_COUNTER 累计抽数，达阈值必出传说并清零（threshold=0 无保底；
    // 保底语义留专家复核——设计终稿未给细则，此处取最简实现）
    let mut pity = PITY_COUNTER.may_load(deps.storage, player)?.unwrap_or(0);

    for i in 0..num as u64 {
        rng_state = mix_seed(rng_state, i, env.block.time.seconds() ^ env.block.height);
        pity = pity.saturating_add(1);
        let tpl = if pity_threshold > 0 && pity >= pity_threshold {
            if legends.is_empty() {
                return Err(ContractError::SanguoPityNoLegend);
            }
            pity = 0;
            let idx = (rng_state as usize) % legends.len();
            legends[idx]
        } else {
            pick_template(rng_state, &templates, &cum, total)
        };
        let card_id = generate_card_id(deps.storage, player, "draw")?;
        let card = CardInfo {
            card_id: card_id.clone(),
            owner: player.to_string(),
            name: tpl.name.clone(),
            rarity: tpl.rarity.clone(),
            attack: tpl.attack,
            defense: tpl.defense,
            star: 1,
            level: 0,
        };
        CARDS.save(deps.storage, &card_id, &card)?;
        crate::sanguo::gift::on_card_created(deps.storage, &card_id, env.block.time.seconds())?;
        mark_collection(deps.storage, player, &tpl.name)?;
        owned.push(card_id.clone());
        new_ids.push(card_id);
        new_names.push(format!("{}({})", tpl.name, tpl.rarity));
    }
    // 缺陷1 接线：改读 params 生效值（0 时访问器回退 MAX_PLAYER_CARDS）
    let max_player_cards = load_params(deps.storage)?.max_player_cards() as usize;
    if owned.len() > max_player_cards {
        return Err(ContractError::SanguoTooManyCards(max_player_cards));
    }
    PLAYER_CARDS.save(deps.storage, player, &owned)?;
    PITY_COUNTER.save(deps.storage, player, &pity)?;
    Ok((new_ids, new_names))
}

pub fn draw_pack(
    deps: &mut DepsMut,
    env: &Env,
    info: MessageInfo,
    tap_index: u8,
    pack3: bool,
    _round_id: String,
) -> Result<Response, ContractError> {
    // C2/C3：抽卡完全免费（spend=0，不碰 PAXI/TKCC/庄家账本），仅受每日限次
    // 约束；tap_index @deprecated 忽略。TKCC 付费分支已随 C1 删除。
    let _ = tap_index;
    let params = load_params(deps.storage)?;

    let player = sanguo_auth(deps, env, &info)?;
    ensure_not_banned_global(deps.storage, &player)?;
    ensure_game_active(deps.storage, "sanguo_draw")?;

    let num = if pack3 { 3u64 } else { 1u64 };
    let today = env.block.time.seconds() / SECS_PER_DAY * SECS_PER_DAY;
    let last_day = SG_DRAW_DATE.may_load(deps.storage, &player)?.unwrap_or(0);
    let used = if last_day == today {
        SG_DRAW_COUNT.may_load(deps.storage, &player)?.unwrap_or(0)
    } else {
        0
    };
    let limit = params.daily_draw_limit();
    // 🟢 审计修复（P1）：并入当日商店加购次数 SG_DRAW_PURCHASED——
    //    此前 buy_shop_item "draw" 臂付费写入后无消费方，加购纯扣款无效果。
    //    注意该键的天数索引 = seconds / SECS_PER_DAY（与 SG_DRAW_DATE 的
    //    "当日零点时间戳"口径不同）。
    let purchased_today = SG_DRAW_PURCHASED
        .may_load(deps.storage, (&player, env.block.time.seconds() / SECS_PER_DAY))?
        .unwrap_or(0) as u64;
    let limit = limit.saturating_add(purchased_today);
    if used + num > limit {
        return Err(ContractError::SanguoDrawLimitReached { limit });
    }

    let (_ids, names) = draw_cards(deps, env, &player, num as u8, params.pity_threshold())?;
    SG_DRAW_DATE.save(deps.storage, &player, &today)?;
    SG_DRAW_COUNT.save(deps.storage, &player, &(used + num))?;

    Ok(Response::new()
        .add_attribute("action", if pack3 { "draw_pack3_free" } else { "draw_free" })
        .add_attribute("player", player.to_string())
        .add_attribute("draws_used", (used + num).to_string())
        .add_attribute("draw_limit", limit.to_string())
        .add_attribute("cards", names.join(",")))
}

// ============================================================================
// 战斗入口一：SanguoBattlePvp（单挑 + 混战，裁定 4.3 合并）
// ============================================================================

pub fn battle_pvp(
    mut deps: DepsMut,
    env: &Env,
    info: MessageInfo,
    mode: PvpMode,
    action: PvpAction,
    match_id: Option<String>,
    opponent: Option<String>,
    public: Option<bool>,
    royale_id: Option<String>,
    size: Option<u8>,
    card_ids: Vec<String>,
    _round_id: String,
) -> Result<Response, ContractError> {
    ensure_game_active(deps.storage, "sanguo_pvp")?;
    match (mode, action) {
        // ---- 单挑：创建 ----
        (PvpMode::Single, PvpAction::Create) => {
            let player = sanguo_auth(&mut deps, env, &info)?;
            ensure_not_banned_global(deps.storage, &player)?;
            let is_public = public.unwrap_or(false);
            let opponent_addr = if is_public {
                Addr::unchecked("")
            } else {
                let a = deps.api.addr_validate(&opponent.unwrap_or_default())?;
                if a == player {
                    return Err(ContractError::SanguoCannotSelfChallenge);
                }
                a
            };
            load_battle_team_checked(deps.storage, &player, &card_ids)?;
            // 自增序号进 id：防同秒同玩家重复创建互相覆盖（审计遗留项⑤）
            let seq = PVP_SEQ.may_load(deps.storage)?.unwrap_or(0) + 1;
            PVP_SEQ.save(deps.storage, &seq)?;
            let match_id_new = format!("pvp_{}_{}_{}", player, env.block.time.seconds(), seq);
            let m = PvpMatch {
                match_id: match_id_new.clone(),
                challenger: player.clone(),
                opponent: opponent_addr,
                is_public,
                challenger_order: card_ids,
                opponent_order: vec![],
                winner: None,
                status: PvpStatus::Waiting,
                created_at: env.block.time.seconds(),
                finished_at: None,
                reward_claimed: false,
                group_scores_a: [0; 3],
                group_scores_b: [0; 3],
            };
            PVP_MATCHES.save(deps.storage, &match_id_new, &m)?;
            Ok(Response::new()
                .add_attribute("action", "battle_pvp_create_single")
                .add_attribute("match_id", match_id_new)
                .add_attribute("challenger", player.to_string()))
        }
        // ---- 单挑：接受（接受即结算，9v9 引擎复算）----
        (PvpMode::Single, PvpAction::Accept) => {
            let match_id = match_id.ok_or_else(|| ContractError::InvalidParams {
                reason: "match_id required".to_string(),
            })?;
            let player = sanguo_auth(&mut deps, env, &info)?;
            ensure_not_banned_global(deps.storage, &player)?;
            let mut m = PVP_MATCHES
                .load(deps.storage, &match_id)
                .map_err(|_| ContractError::SanguoMatchNotFound(match_id.clone()))?;
            if m.is_public {
                if player == m.challenger {
                    return Err(ContractError::SanguoCannotSelfAccept);
                }
            } else if player != m.opponent {
                return Err(ContractError::SanguoNotOpponent);
            }
            if env.block.time.seconds().saturating_sub(m.created_at) > 7 * SECS_PER_DAY {
                m.status = PvpStatus::Cancelled;
                PVP_MATCHES.save(deps.storage, &match_id, &m)?;
                return Ok(Response::new()
                    .add_attribute("action", "battle_pvp_accept_timeout_cancelled")
                    .add_attribute("match_id", match_id));
            }
            if m.status != PvpStatus::Waiting {
                return Err(ContractError::SanguoMatchNotWaiting);
            }
            load_battle_team_checked(deps.storage, &player, &card_ids)?;

            if m.is_public {
                m.opponent = player.clone();
            }
            m.opponent_order = card_ids;
            m.status = PvpStatus::Pending;

            // 引擎复算：A=挑战者、B=应战者；Draw → 双方均不胜（G3）
            let params = load_params(deps.storage)?;
            let ctx = BattleContext::from_params(&params);
            // 🟢 审计修复（P2）：accept 时对挑战者编队做归属复核——等待期内
            //    卡可被赠送（CARDS.owner 变更但记录仍在），原 load_battle_team
            //    不查归属，挑战者可把卡送走后仍用其属性参战（卡池双份效用）。
            let a = load_battle_team_checked(deps.storage, &m.challenger, &m.challenger_order.clone())?;
            let b = load_battle_team_checked(deps.storage, &player, &m.opponent_order.clone())?;
            let result = compute_battle(&ctx, &a, &b)?;
            let winner = match result.winner {
                Winner::A => Some(m.challenger.clone()),
                Winner::B => Some(m.opponent.clone()),
                Winner::Draw => None,
            };
            m.winner = winner.clone();
            m.group_scores_a = result.group_scores_a;
            m.group_scores_b = result.group_scores_b;
            m.status = PvpStatus::Finished;
            m.finished_at = Some(env.block.time.seconds());
            if !m.opponent.as_str().is_empty() {
                crate::sanguo::gift::record_pair(
                    deps.storage, &m.challenger, &m.opponent, env.block.time.seconds(),
                )?;
            }
            PVP_MATCHES.save(deps.storage, &match_id, &m)?;

            // V2.1：战绩累计——双方各计 1 场，胜方 wins+1（平局仅 total）
            add_battle_stat(&mut deps, &m.challenger, 1, winner.as_ref() == Some(&m.challenger))?;
            if !m.opponent.as_str().is_empty() {
                add_battle_stat(&mut deps, &m.opponent, 1, winner.as_ref() == Some(&m.opponent))?;
            }

            Ok(Response::new()
                .add_attribute("action", "battle_pvp_accept_single")
                .add_attribute("match_id", match_id)
                .add_attribute(
                    "winner",
                    winner.as_ref().map(|w| w.to_string()).unwrap_or_else(|| "draw".to_string()),
                )
                .add_attribute("groups_played", result.groups_played.to_string())
                .add_attribute("group_scores_a", format!("{}-{}-{}", result.group_scores_a[0], result.group_scores_a[1], result.group_scores_a[2]))
                .add_attribute("group_scores_b", format!("{}-{}-{}", result.group_scores_b[0], result.group_scores_b[1], result.group_scores_b[2]))
                .add_attribute("skill_log", skill_logs_json(&result.skill_logs)))
        }
        // ---- 单挑：取消 ----
        (PvpMode::Single, PvpAction::Cancel) => {
            let match_id = match_id.ok_or_else(|| ContractError::InvalidParams {
                reason: "match_id required".to_string(),
            })?;
            let player = sanguo_auth(&mut deps, env, &info)?;
            let mut m = PVP_MATCHES
                .load(deps.storage, &match_id)
                .map_err(|_| ContractError::SanguoMatchNotFound(match_id.clone()))?;
            if player != m.challenger {
                return Err(ContractError::Unauthorized);
            }
            if m.status != PvpStatus::Waiting {
                return Err(ContractError::SanguoMatchNotWaiting);
            }
            m.status = PvpStatus::Cancelled;
            PVP_MATCHES.save(deps.storage, &match_id, &m)?;
            Ok(Response::new()
                .add_attribute("action", "battle_pvp_cancel_single")
                .add_attribute("match_id", match_id))
        }
        // ---- 单挑：领奖（赢家领碎片）----
        (PvpMode::Single, PvpAction::Claim) => {
            let match_id = match_id.ok_or_else(|| ContractError::InvalidParams {
                reason: "match_id required".to_string(),
            })?;
            let params = load_params(deps.storage)?;
            let player = sanguo_auth(&mut deps, env, &info)?;
            let mut m = PVP_MATCHES
                .load(deps.storage, &match_id)
                .map_err(|_| ContractError::SanguoMatchNotFound(match_id.clone()))?;
            if m.status != PvpStatus::Finished {
                return Err(ContractError::SanguoMatchNotFinished);
            }
            let winner = m
                .winner
                .clone()
                .ok_or(ContractError::SanguoNoWinner)?; // Draw → 无人可领
            if player != winner {
                return Err(ContractError::Unauthorized);
            }
            if m.reward_claimed {
                return Err(ContractError::SanguoAlreadyClaimed);
            }
            grant_fragments_checked(&mut deps, env, &winner, &params.pvp_frag_reward)?;
            m.reward_claimed = true;
            PVP_MATCHES.save(deps.storage, &match_id, &m)?;
            let f = &params.pvp_frag_reward;
            Ok(Response::new()
                .add_attribute("action", "battle_pvp_claim_single")
                .add_attribute("reward", format!("common:{},rare:{},epic:{},legend:{}", f.common, f.rare, f.epic, f.legend)))
        }

        // ---- 混战：创建（9v9 编队，4~6 人）----
        (PvpMode::Royale, PvpAction::Create) => {
            let player = sanguo_auth(&mut deps, env, &info)?;
            ensure_not_banned_global(deps.storage, &player)?;
            let sz = size.ok_or_else(|| ContractError::InvalidParams {
                reason: "size required for royale create".to_string(),
            })?;
            if !(4..=6).contains(&sz) {
                return Err(ContractError::SanguoInvalidSize(sz));
            }
            load_battle_team_checked(deps.storage, &player, &card_ids)?;
            let royale_seq = ROYALE_SEQ.may_load(deps.storage)?.unwrap_or(0) + 1;
            ROYALE_SEQ.save(deps.storage, &royale_seq)?;
            let royale_id_new = format!("royale_{}_{}_{}", player, env.block.time.seconds(), royale_seq);
            let r = RoyaleMatch {
                royale_id: royale_id_new.clone(),
                players: vec![player.clone()],
                player_orders: vec![card_ids],
                winner: None,
                status: RoyaleStatus::Waiting,
                created_at: env.block.time.seconds(),
                finished_at: None,
                size: sz,
                reward_claimed: false,
            };
            ROYALE_MATCHES.save(deps.storage, &royale_id_new, &r)?;
            Ok(Response::new()
                .add_attribute("action", "battle_pvp_create_royale")
                .add_attribute("royale_id", royale_id_new)
                .add_attribute("size", sz.to_string()))
        }
        // ---- 混战：入场 ----
        (PvpMode::Royale, PvpAction::Join) => {
            let royale_id = royale_id.ok_or_else(|| ContractError::InvalidParams {
                reason: "royale_id required".to_string(),
            })?;
            let player = sanguo_auth(&mut deps, env, &info)?;
            ensure_not_banned_global(deps.storage, &player)?;
            let mut r = ROYALE_MATCHES
                .load(deps.storage, &royale_id)
                .map_err(|_| ContractError::SanguoRoyaleNotFound(royale_id.clone()))?;
            if r.status != RoyaleStatus::Waiting {
                return Err(ContractError::SanguoRoyaleNotWaiting);
            }
            if env.block.time.seconds().saturating_sub(r.created_at) > 7 * SECS_PER_DAY {
                r.status = RoyaleStatus::Cancelled;
                ROYALE_MATCHES.save(deps.storage, &royale_id, &r)?;
                return Ok(Response::new()
                    .add_attribute("action", "battle_pvp_join_timeout_cancelled")
                    .add_attribute("royale_id", royale_id));
            }
            if r.players.contains(&player) {
                return Err(ContractError::SanguoAlreadyJoined);
            }
            if r.players.len() as u8 >= r.size {
                return Err(ContractError::SanguoRoyaleFull);
            }
            load_battle_team_checked(deps.storage, &player, &card_ids)?;
            r.players.push(player.clone());
            r.player_orders.push(card_ids);
            if r.players.len() as u8 == r.size {
                r.status = RoyaleStatus::Full;
            }
            ROYALE_MATCHES.save(deps.storage, &royale_id, &r)?;
            Ok(Response::new()
                .add_attribute("action", "battle_pvp_join_royale")
                .add_attribute("royale_id", royale_id)
                .add_attribute("current", r.players.len().to_string()))
        }
        // ---- 混战：结算（两两引擎对战 + G4 三级排序，E6 预算检查）----
        (PvpMode::Royale, PvpAction::Settle) => {
            let royale_id = royale_id.ok_or_else(|| ContractError::InvalidParams {
                reason: "royale_id required".to_string(),
            })?;
            let player = sanguo_auth(&mut deps, env, &info)?;
            let mut r = ROYALE_MATCHES
                .load(deps.storage, &royale_id)
                .map_err(|_| ContractError::SanguoRoyaleNotFound(royale_id.clone()))?;
            if r.status != RoyaleStatus::Full {
                return Err(ContractError::SanguoRoyaleNotFull);
            }
            if !r.players.contains(&player) {
                return Err(ContractError::SanguoNotParticipant);
            }
            let (winner, royale_logs) = settle_royale_engine(&mut deps, &mut r)?;
            r.winner = Some(winner.clone());
            r.status = RoyaleStatus::Finished;
            r.finished_at = Some(env.block.time.seconds());
            crate::sanguo::gift::record_royale_pairs(deps.storage, &r.players, env.block.time.seconds())?;
            ROYALE_MATCHES.save(deps.storage, &royale_id, &r)?;
            // V2.1：混战战绩——每位参战方各计 1 场，冠军 wins+1（混战无平局）
            for p in &r.players {
                add_battle_stat(&mut deps, p, 2, *p == winner)?;
            }
            Ok(Response::new()
                .add_attribute("action", "battle_pvp_settle_royale")
                .add_attribute("winner", winner.to_string())
                .add_attribute("skill_log", skill_logs_json(&royale_logs)))
        }
        // ---- 混战：领奖 ----
        (PvpMode::Royale, PvpAction::Claim) => {
            let royale_id = royale_id.ok_or_else(|| ContractError::InvalidParams {
                reason: "royale_id required".to_string(),
            })?;
            let params = load_params(deps.storage)?;
            let player = sanguo_auth(&mut deps, env, &info)?;
            let mut r = ROYALE_MATCHES
                .load(deps.storage, &royale_id)
                .map_err(|_| ContractError::SanguoRoyaleNotFound(royale_id.clone()))?;
            if r.status != RoyaleStatus::Finished {
                return Err(ContractError::SanguoRoyaleNotFinished);
            }
            let winner = r.winner.clone().ok_or(ContractError::SanguoNoWinner)?;
            if player != winner {
                return Err(ContractError::Unauthorized);
            }
            if r.reward_claimed {
                return Err(ContractError::SanguoAlreadyClaimed);
            }
            grant_fragments_checked(&mut deps, env, &winner, &params.royale_frag_reward)?;
            r.reward_claimed = true;
            ROYALE_MATCHES.save(deps.storage, &royale_id, &r)?;
            let f = &params.royale_frag_reward;
            Ok(Response::new()
                .add_attribute("action", "battle_pvp_claim_royale")
                .add_attribute("reward", format!("common:{},rare:{},epic:{},legend:{}", f.common, f.rare, f.epic, f.legend)))
        }

        // mode×action 非法组合（Single+Join / Single+Settle / Royale+Accept /
        // Royale+Cancel）在编译期由穷尽匹配兜底拒绝（裁定 4.3 分发约束）
        _ => Err(ContractError::InvalidParams {
            reason: "invalid pvp mode/action combination".to_string(),
        }),
    }
}

/// 混战结算（E6 + G4）：两两 compute_battle，积分 = 胜场数；
/// 三级排序：胜场 desc → self_power desc → 加入顺序 asc（join seq）。
/// 预算检查：n*(n-1)/2 场引擎对战，>15 场拒（4~6 人上限内实际 ≤15）。
fn settle_royale_engine(
    deps: &mut DepsMut,
    r: &mut RoyaleMatch,
) -> Result<(Addr, Vec<SkillTriggerLog>), ContractError> {
    let n = r.players.len();
    let budget = n * (n - 1) / 2;
    if budget > 15 {
        return Err(ContractError::RoyaleSettleTooExpensive);
    }
    let params = load_params(deps.storage)?;
    let ctx = BattleContext::from_params(&params);
    let mut teams: Vec<[BattleCard; 9]> = Vec::with_capacity(n);
    // 🟢 审计修复（P2）：settle 时逐玩家复核编队归属（与 PVP accept 同理，
    //    等待期内卡被赠送/分解即拒绝结算，防卡池双份效用）
    for (i, order) in r.player_orders.iter().enumerate() {
        teams.push(load_battle_team_checked(deps.storage, &r.players[i], order)?);
    }
    let mut match_wins = vec![0u32; n];
    let mut self_powers = vec![0u64; n];
    // V2.1：混战为多场 1v1 聚合，逐场收集技能触发日志（仅观测，供结算事件回放）
    let mut royale_logs: Vec<SkillTriggerLog> = Vec::new();
    for i in 0..n {
        for j in (i + 1)..n {
            let res = compute_battle(&ctx, &teams[i], &teams[j])?;
            // self_power 只依赖本队（OnBattleStart 后 base × rarity + bond），
            // 与对手无关，任一场的取值即为该玩家最终值（G4 口径）
            self_powers[i] = res.a_self_power;
            self_powers[j] = res.b_self_power;
            royale_logs.extend(res.skill_logs);
            match res.winner {
                Winner::A => match_wins[i] += 1,
                Winner::B => match_wins[j] += 1,
                Winner::Draw => {}
            }
        }
    }
    let mut idx: Vec<usize> = (0..n).collect();
    idx.sort_by(|&a, &b| {
        match_wins[b]
            .cmp(&match_wins[a])
            .then(self_powers[b].cmp(&self_powers[a]))
            .then(a.cmp(&b)) // 加入顺序 = players 下标（join seq）
    });
    // V2.1：安全上限——混战最多 15 场 1v1，极端情况下日志可能偏多；
    // 截断到 300 条：单场 ~54 条，300 已覆盖 ≤5 人混战的全部触发；
    // 更大混战截断为「尽力而为」时间线。收紧上限是为把单条事件属性
    // 压到 ~45KB 以内，避免 royale settle 这一关键路径因事件体胀失败。
    if royale_logs.len() > 300 {
        royale_logs.truncate(300);
    }
    Ok((r.players[idx[0]].clone(), royale_logs))
}

// ============================================================================
// 战斗入口二：SanguoBattleAi（休闲 + 天梯，裁定 4.3 合并）
// ============================================================================

pub fn battle_ai(
    mut deps: DepsMut,
    env: &Env,
    info: MessageInfo,
    mode: AiMode,
    level: u8,
    card_ids: Vec<String>,
    _round_id: String,
) -> Result<Response, ContractError> {
    ensure_game_active(deps.storage, "sanguo_ai")?;
    match mode {
        AiMode::Casual => battle_ai_casual(
            deps, env, info, level, card_ids,
        ),
        AiMode::Ranked => battle_ai_ranked(
            deps, env, info, level, card_ids,
        ),
    }
}

/// AI 休闲（level = 难度 1..5；E4 day-bucket 种子；胜发碎片，Draw 不发）
fn battle_ai_casual(
    mut deps: DepsMut,
    env: &Env,
    info: MessageInfo,
    difficulty: u8,
    card_ids: Vec<String>,
) -> Result<Response, ContractError> {
    let params = load_params(deps.storage)?;
    if difficulty < 1 || difficulty > 5 {
        return Err(ContractError::SanguoInvalidDifficulty(difficulty));
    }
    let diff_idx = (difficulty - 1) as usize;
    let player = sanguo_auth(&mut deps, env, &info)?;
    ensure_not_banned_global(deps.storage, &player)?;

    // 每日限次
    let today = env.block.time.seconds() / SECS_PER_DAY * SECS_PER_DAY;
    let last_day = AI_BATTLE_DATE.may_load(deps.storage, &player)?.unwrap_or(0);
    let today_count = if last_day == today {
        AI_BATTLE_COUNT.may_load(deps.storage, &player)?.unwrap_or(0)
    } else {
        0
    };
    let ai_limit = params.ai_daily_limit();
    if today_count >= ai_limit {
        return Err(ContractError::SanguoDailyAiLimit(ai_limit));
    }

    // 玩家编队（归属 + 去重 + Σcost 校验）
    let player_cards = load_battle_team_checked(deps.storage, &player, &card_ids)?;

    // E4：seed = SHA256(day_bucket:player:nonce:difficulty)；nonce = 当日第几场
    //     （玩家域内单调确定，剔除 tx_idx/height，前端可复算）
    let seed = crate::sanguo::battle::ai_gen::ai_seed(
        env.block.time.seconds(),
        player.as_str(),
        today_count,
        difficulty as u64,
    );
    let templates = CARD_TEMPLATES
        .may_load(deps.storage)?
        .unwrap_or_default();
    let ai_cards = generate_ai_cards(
        &templates,
        &seed,
        params.diff_boost_bp()[diff_idx],
        params.ai_floor_bp()[diff_idx],
        &player_cards,
    )?;

    let ctx = BattleContext::from_params(&params);
    let result = compute_battle(&ctx, &player_cards, &ai_cards)?;
    let win = result.winner == Winner::A; // Draw → 无奖励（不扣不赔）

    let battle_id = format!(
        "battle_{}_{}_{}_{}",
        player, env.block.height, env.block.time.seconds(), today_count
    );
    let reward_frags = if win {
        // 💡 ai_frag_reward 是裸字段数组（SanguoParams 无同名访问器）：
        //    零值回填已在 migrate 期完成，故此处直接按难度下标取数组元素。
        params.ai_frag_reward[diff_idx]
    } else {
        Fragments::default()
    };
    let reward_str = format!(
        "common:{},rare:{},epic:{},legend:{}",
        reward_frags.common, reward_frags.rare, reward_frags.epic, reward_frags.legend
    );
    if win {
        grant_fragments_checked(&mut deps, env, &player, &reward_frags)?;
    }
    let record = BattleRecord {
        battle_id: battle_id.clone(),
        player: player.clone(),
        difficulty,
        result: if win { "win".to_string() } else { "lose".to_string() },
        reward: reward_str.clone(),
        claimed: false,
        timestamp: env.block.time.seconds(),
    };
    BATTLES.save(deps.storage, &battle_id, &record)?;

    AI_BATTLE_DATE.save(deps.storage, &player, &today)?;
    AI_BATTLE_COUNT.save(deps.storage, &player, &(today_count + 1))?;
    add_battle_stat(&mut deps, &player, 0, win)?;

    let a_powers: Vec<String> = result.a_final_powers.iter().map(|x| x.to_string()).collect();
    let b_powers: Vec<String> = result.b_final_powers.iter().map(|x| x.to_string()).collect();
    Ok(Response::new()
        .add_attribute("action", "battle_ai_casual")
        .add_attribute("player", player.to_string())
        .add_attribute("difficulty", difficulty.to_string())
        .add_attribute("result", if win { "win" } else { "lose" })
        .add_attribute("group_scores_a", format!("{}-{}-{}", result.group_scores_a[0], result.group_scores_a[1], result.group_scores_a[2]))
        .add_attribute("group_scores_b", format!("{}-{}-{}", result.group_scores_b[0], result.group_scores_b[1], result.group_scores_b[2]))
        .add_attribute("reward", reward_str)
        .add_attribute("battle_id", battle_id)
        .add_attribute("player_powers", a_powers.join(","))
        .add_attribute("ai_powers", b_powers.join(","))
        .add_attribute("skill_log", skill_logs_json(&result.skill_logs)))
}

/// AI 天梯（level = 段位 0..4；消耗体力；胜加分 + 赛季点 + 碎片）
fn battle_ai_ranked(
    mut deps: DepsMut,
    env: &Env,
    info: MessageInfo,
    tier: u8,
    card_ids: Vec<String>,
) -> Result<Response, ContractError> {
    let params = load_params(deps.storage)?;
    if tier as usize >= params.rank_boost_bp().len() {
        return Err(ContractError::SanguoInvalidRankTier(tier));
    }
    let player = sanguo_auth(&mut deps, env, &info)?;
    ensure_not_banned_global(deps.storage, &player)?;

    // M9：先验卡再扣体力
    let player_cards = load_battle_team_checked(deps.storage, &player, &card_ids)?;
    let stamina_cost = params.rank_stamina_cost();
    consume_stamina(&mut deps, &player, stamina_cost, env.block.time.seconds(),
        params.stamina_max(), params.stamina_regen_secs())?;

    // E4 种子：nonce = 0（同日同段位 AI 阵容确定，可预演；跨日变化）
    let seed = crate::sanguo::battle::ai_gen::ai_seed(
        env.block.time.seconds(),
        player.as_str(),
        0,
        tier as u64,
    );
    let templates = CARD_TEMPLATES
        .may_load(deps.storage)?
        .unwrap_or_default();
    let ai_cards = generate_ai_cards(
        &templates,
        &seed,
        params.rank_boost_bp()[tier as usize],
        0, // 天梯无地板（boost 即门槛；如需地板由运营调 ai_floor_bp 并改此处）
        &player_cards,
    )?;

    let ctx = BattleContext::from_params(&params);
    let result = compute_battle(&ctx, &player_cards, &ai_cards)?;
    let won = result.winner == Winner::A;

    let mut rank = RANK_INFO.may_load(deps.storage, &player)?.unwrap_or_default();
    let mut season_pts: u64 = 0;
    let reward = if won {
        rank.points = rank.points.saturating_add(params.rank_win_points());
        season_pts = params.rank_win_season_points();
        let tier_up = params.rank_tier_up_points();
        while (rank.tier as usize) < params.rank_boost_bp().len() - 1 && rank.points >= tier_up {
            rank.points -= tier_up;
            rank.tier += 1;
        }
        let r = params.rank_rewards()[rank.tier as usize];
        grant_fragments_checked(&mut deps, env, &player, &r)?;
        r
    } else {
        rank.points = rank.points.saturating_sub(params.rank_lose_points());
        if rank.points < 0 {
            rank.points = 0;
        }
        let tier_up = params.rank_tier_up_points();
        if rank.points == 0 && rank.tier > 0 {
            rank.tier -= 1;
            // 🟢 审计修复（P2）：saturating 防参数误配（tier_up < lose）时 i64 下溢
            rank.points = tier_up.saturating_sub(params.rank_lose_points());
        }
        Fragments::default()
    };
    RANK_INFO.save(deps.storage, &player, &rank)?;
    add_battle_stat(&mut deps, &player, 0, won)?;
    if season_pts > 0 {
        add_season_points(&mut deps, &player, season_pts)?;
        inc_daily_task(&mut deps, &player, 1, env.block.time.seconds())?;
    }
    let total = reward.common + reward.rare + reward.epic + reward.legend;
    // 🟢 天梯：补齐双方分组亮牌所需事件（与休闲 AI 一致，纯 append，不改状态）
    let a_powers: Vec<String> = result.a_final_powers.iter().map(|x| x.to_string()).collect();
    let b_powers: Vec<String> = result.b_final_powers.iter().map(|x| x.to_string()).collect();
    Ok(Response::new()
        .add_attribute("action", "battle_ai_ranked")
        .add_attribute("player", player.to_string())
        .add_attribute("tier", tier.to_string())
        .add_attribute("won", won.to_string())
        .add_attribute("result", if won { "win" } else { "lose" })
        .add_attribute("group_scores_a", format!("{}-{}-{}", result.group_scores_a[0], result.group_scores_a[1], result.group_scores_a[2]))
        .add_attribute("group_scores_b", format!("{}-{}-{}", result.group_scores_b[0], result.group_scores_b[1], result.group_scores_b[2]))
        .add_attribute("rank_tier", rank.tier.to_string())
        .add_attribute("rank_points", rank.points.to_string())
        .add_attribute("reward_total", total.to_string())
        .add_attribute("player_powers", a_powers.join(","))
        .add_attribute("ai_powers", b_powers.join(","))
        .add_attribute("skill_log", skill_logs_json(&result.skill_logs)))
}

/// 🔴 C 硬化：合规模型下战斗奖励已在战斗时即时发放（统一碎片），
/// 本入口仅 legacy TKCC 对局需要——恒拒，封死孤儿 TKCC 出金路径（P0）。
pub fn claim_reward(
    _deps: &mut DepsMut,
    _env: &Env,
    _info: MessageInfo,
    _battle_id: String,
    _round_id: String,
) -> Result<Response, ContractError> {
    Err(ContractError::ComplianceDisabled)
}

// ============================================================================
// 出战顺序 / 升星 / 分解 / 升级 / 合成
// ============================================================================

pub fn set_battle_order(
    deps: &mut DepsMut,
    env: &Env,
    info: MessageInfo,
    order: Vec<String>,
    _round_id: String,
) -> Result<Response, ContractError> {
    let player = sanguo_auth(deps, env, &info)?;
    ensure_not_banned_global(deps.storage, &player)?;
    let owned = PLAYER_CARDS
        .may_load(deps.storage, &player)?
        .unwrap_or_default();
    let filtered: Vec<String> = order
        .into_iter()
        .filter(|id| owned.contains(id))
        .take(9)
        .collect();
    PLAYER_BATTLE_ORDER.save(deps.storage, &player, &filtered)?;
    Ok(Response::new()
        .add_attribute("action", "set_battle_order")
        .add_attribute("player", player.to_string())
        .add_attribute("order_len", filtered.len().to_string()))
}

pub fn star_up(
    deps: &mut DepsMut,
    env: &Env,
    info: MessageInfo,
    card_id: String,
    use_fragments: Option<bool>,
    _round_id: String,
) -> Result<Response, ContractError> {
    // C4：TKCC 升星分支删除，唯一路径 = 统一碎片；use_fragments 参数
    // 保留仅为签名兼容，恒按碎片处理。
    let _ = use_fragments;
    let params = load_params(deps.storage)?;
    let mut card = CARDS
        .load(deps.storage, &card_id)
        .map_err(|_| ContractError::SanguoCardNotFound(card_id.clone()))?;
    if card.star >= 5 {
        return Err(ContractError::SanguoMaxStar);
    }
    // 🟢 第二轮审计修复（P2）：saturating_sub 防历史/迁移数据 star=0 时
    //    `card.star - 1` 下溢（release 无 overflow-checks 会 wrap 成 255 →
    //    star_up_fragment_cost()[255] 数组越界 abort）。
    let star_idx = card.star.saturating_sub(1) as usize;
    let cost = params.star_up_fragment_cost()[star_idx];

    let player = sanguo_auth(deps, env, &info)?;
    ensure_not_banned_global(deps.storage, &player)?;
    if card.owner != player.to_string() {
        return Err(ContractError::Unauthorized);
    }

    spend_fragments(deps.storage, &player, cost as u128)?;
    card.star += 1;
    card.attack = card.attack.saturating_add(params.upgrade_atk_boost()[star_idx]);
    card.defense = card.defense.saturating_add(params.upgrade_def_boost()[star_idx]);
    CARDS.save(deps.storage, &card_id, &card)?;
    Ok(Response::new()
        .add_attribute("action", "star_up")
        .add_attribute("card_id", card_id)
        .add_attribute("new_star", card.star.to_string())
        .add_attribute("mode", "fragments")
        .add_attribute("fragments_cost", cost.to_string()))
}

/// 分解单卡内部实现（单卡入口与批量入口共用）：锁卡必查；返还统一碎片
fn decompose_one(
    deps: &mut DepsMut,
    env: &Env,
    player: &Addr,
    card_id: &str,
    frag_back: &[u64; 4],
) -> Result<(String, u64), ContractError> {
    if LOCKED_CARDS
        .may_load(deps.storage, card_id)?
        .unwrap_or(false)
    {
        return Err(ContractError::SanguoCardLocked(card_id.to_string()));
    }
    let card = CARDS
        .load(deps.storage, card_id)
        .map_err(|_| ContractError::SanguoCardNotFound(card_id.to_string()))?;
    if card.owner != player.to_string() {
        return Err(ContractError::Unauthorized);
    }
    let ri = Fragments::rarity_index(&card.rarity).unwrap_or(0);
    let frags = frag_back[ri];

    CARDS.remove(deps.storage, card_id);
    let mut owned = PLAYER_CARDS
        .may_load(deps.storage, player)?
        .unwrap_or_default();
    owned.retain(|x| x != card_id);
    PLAYER_CARDS.save(deps.storage, player, &owned)?;

    // 出战顺序 / 预设自动移除
    let mut order = PLAYER_BATTLE_ORDER
        .may_load(deps.storage, player)?
        .unwrap_or_default();
    let before = order.len();
    order.retain(|x| x != card_id);
    if order.len() != before {
        PLAYER_BATTLE_ORDER.save(deps.storage, player, &order)?;
    }
    for pid in 0..MAX_TEAM_PRESETS {
        if let Some(mut preset) = TEAM_PRESETS.may_load(deps.storage, (player, pid))? {
            let before_p = preset.card_ids.len();
            preset.card_ids.retain(|x| x != card_id);
            if preset.card_ids.len() != before_p {
                TEAM_PRESETS.save(deps.storage, (player, pid), &preset)?;
            }
        }
    }

    grant_fragments_unified(deps, env, player, frags as u128)?;
    Ok((card.rarity, frags))
}

pub fn decompose_card(
    deps: &mut DepsMut,
    env: &Env,
    info: MessageInfo,
    card_id: String,
    _round_id: String,
) -> Result<Response, ContractError> {
    let params = load_params(deps.storage)?;
    let player = sanguo_auth(deps, env, &info)?;
    ensure_not_banned_global(deps.storage, &player)?;
    let (rarity, frags) = decompose_one(deps, env, &player, &card_id, &params.fragment_from_duplicate())?;
    Ok(Response::new()
        .add_attribute("action", "decompose")
        .add_attribute("card_id", card_id)
        .add_attribute("rarity", rarity)
        .add_attribute("fragments", frags.to_string()))
}

pub fn decompose_batch(
    deps: &mut DepsMut,
    env: &Env,
    info: MessageInfo,
    card_ids: Vec<String>,
    _round_id: String,
) -> Result<Response, ContractError> {
    if card_ids.is_empty() || card_ids.len() > 100 {
        return Err(ContractError::InvalidParams {
            reason: "card_ids count must be 1..=100".to_string(),
        });
    }
    let params = load_params(deps.storage)?;
    let player = sanguo_auth(deps, env, &info)?;
    ensure_not_banned_global(deps.storage, &player)?;
    let mut total = 0u64;
    let mut decomposed = 0u32;
    for card_id in &card_ids {
        let (_rarity, frags) =
            decompose_one(deps, env, &player, card_id, &params.fragment_from_duplicate())?;
        total = total.saturating_add(frags);
        decomposed += 1;
    }
    Ok(Response::new()
        .add_attribute("action", "decompose_batch")
        .add_attribute("player", player.to_string())
        .add_attribute("decomposed", decomposed.to_string())
        .add_attribute("fragments", total.to_string()))
}

pub fn upgrade_card(
    deps: &mut DepsMut,
    env: &Env,
    info: MessageInfo,
    card_id: String,
    _round_id: String,
) -> Result<Response, ContractError> {
    let params = load_params(deps.storage)?;
    let player = sanguo_auth(deps, env, &info)?;
    ensure_not_banned_global(deps.storage, &player)?;
    let mut card = CARDS
        .load(deps.storage, &card_id)
        .map_err(|_| ContractError::SanguoCardNotFound(card_id.clone()))?;
    if card.owner != player.to_string() {
        return Err(ContractError::Unauthorized);
    }
    let max_level = params.upgrade_frag_max_level();
    if card.level >= max_level {
        return Err(ContractError::SanguoMaxLevel);
    }
    // 沿用 V1 语义：升级消耗 = star_up_fragment_cost[ri]（对账注记 ④）
    let cost = params.star_up_fragment_cost()[Fragments::rarity_index(&card.rarity).unwrap_or(0)];
    spend_fragments(deps.storage, &player, cost as u128)?;

    card.attack = card.attack.saturating_add(params.upgrade_frag_atk());
    card.defense = card.defense.saturating_add(params.upgrade_frag_def());
    card.level = card.level.saturating_add(1);
    CARDS.save(deps.storage, &card_id, &card)?;

    Ok(Response::new()
        .add_attribute("action", "upgrade")
        .add_attribute("card_id", card_id)
        .add_attribute("new_level", card.level.to_string())
        .add_attribute("new_attack", card.attack.to_string())
        .add_attribute("new_defense", card.defense.to_string())
        .add_attribute("fragments_spent", cost.to_string()))
}

pub fn craft_card(
    deps: &mut DepsMut,
    env: &Env,
    info: MessageInfo,
    rarity: String,
    _round_id: String,
) -> Result<Response, ContractError> {
    let params = load_params(deps.storage)?;
    let player = sanguo_auth(deps, env, &info)?;
    ensure_not_banned_global(deps.storage, &player)?;
    let ri = Fragments::rarity_index(&rarity)
        .ok_or_else(|| ContractError::SanguoInvalidRarity(rarity.clone()))?;
    let templates = CARD_TEMPLATES
        .may_load(deps.storage)?
        .unwrap_or_default();
    let candidates: Vec<&CardTemplate> =
        templates.iter().filter(|t| t.rarity == rarity).collect();
    if candidates.is_empty() {
        return Err(ContractError::SanguoNoTemplates);
    }
    let cost = params.craft_cost()[ri];
    spend_fragments(deps.storage, &player, cost as u128)?;

    let tx_idx = env.transaction.as_ref().map(|t| t.index as u64).unwrap_or(0);
    let rng = mix_seed(
        env.block.time.seconds(),
        env.block.height,
        player
            .as_bytes()
            .iter()
            .fold(0u64, |acc, &b| acc.wrapping_add(b as u64))
            .wrapping_add(tx_idx),
    );
    let idx = (rng % candidates.len() as u64) as usize;
    let tpl = candidates[idx];

    let card_id = generate_card_id(deps.storage, &player, "craft")?;
    let card = CardInfo {
        card_id: card_id.clone(),
        owner: player.to_string(),
        name: tpl.name.clone(),
        rarity: tpl.rarity.clone(),
        attack: tpl.attack,
        defense: tpl.defense,
        star: 1,
        level: 0,
    };
    CARDS.save(deps.storage, &card_id, &card)?;
    crate::sanguo::gift::on_card_created(deps.storage, &card_id, env.block.time.seconds())?;
    mark_collection(deps.storage, &player, &tpl.name)?;
    let mut owned = PLAYER_CARDS
        .may_load(deps.storage, &player)?
        .unwrap_or_default();
    owned.push(card_id.clone());
    // 缺陷1 接线：复用本函数已 load 的 params（改读生效值）
    let max_player_cards = params.max_player_cards() as usize;
    if owned.len() > max_player_cards {
        return Err(ContractError::SanguoTooManyCards(max_player_cards));
    }
    PLAYER_CARDS.save(deps.storage, &player, &owned)?;

    Ok(Response::new()
        .add_attribute("action", "craft")
        .add_attribute("player", player.to_string())
        .add_attribute("rarity", rarity)
        .add_attribute("fragments_cost", cost.to_string())
        .add_attribute("new_card", tpl.name.clone()))
}

/// 图鉴标记（COLLECTION key = 模板名；对账注记 ③）
fn mark_collection(storage: &mut dyn Storage, player: &Addr, name: &str) -> StdResult<()> {
    if !COLLECTION.has(storage, (player, name)) {
        COLLECTION.save(storage, (player, name), &true)?;
    }
    Ok(())
}

/// 全局卡牌 ID 生成器（防同块同玩家重复）
fn generate_card_id(
    storage: &mut dyn Storage,
    sender: &Addr,
    prefix: &str,
) -> StdResult<String> {
    let next = CARD_ID_COUNTER.may_load(storage)?.unwrap_or(0) + 1;
    CARD_ID_COUNTER.save(storage, &next)?;
    Ok(format!("card_{}_{}_{}", prefix, sender, next))
}

// ============================================================================
// 🀄 老三国合约资产迁移（卡牌 + 四桶碎片 → 统一碎片）
// ============================================================================

/// 老合约 `player_cards` 返回体
#[derive(Deserialize)]
struct OldCardsResponse {
    cards: Vec<OldCardInfo>,
}

#[derive(Deserialize)]
struct OldCardInfo {
    card_id: String,
    owner: String,
    name: String,
    rarity: String,
    attack: u32,
    defense: u32,
    star: u8,
    level: u8,
}

/// 老合约 `get_fragments` 返回体
#[derive(Deserialize)]
struct OldFragmentsResponse {
    common: u64,
    rare: u64,
    epic: u64,
    legend: u64,
}

/// 管理员：放行一个老合约地址
pub fn add_migration_target(
    deps: DepsMut,
    info: MessageInfo,
    old_contract: String,
) -> Result<Response, ContractError> {
    if !is_admin(deps.as_ref(), &info.sender)? {
        return Err(ContractError::Unauthorized);
    }
    let old_addr = deps.api.addr_validate(&old_contract)?;
    MIGRATION_WHITELIST.save(deps.storage, &old_addr, &true)?;
    Ok(Response::new()
        .add_attribute("action", "sanguo_add_migration_target")
        .add_attribute("old_contract", old_contract))
}

/// 玩家自助迁移：卡保留原属性；四桶碎片折算为统一碎片入 FRAGMENT_BALANCE
pub fn migrate_from_old(
    deps: &mut DepsMut,
    env: &Env,
    info: MessageInfo,
    old_contract: String,
    _round_id: String,
) -> Result<Response, ContractError> {
    let old_addr = deps.api.addr_validate(&old_contract)?;
    let allowed = MIGRATION_WHITELIST
        .may_load(deps.storage, &old_addr)?
        .unwrap_or(false);
    if !allowed {
        return Err(ContractError::SanguoMigrationNotAllowed);
    }
    let player = sanguo_auth(deps, env, &info)?;
    ensure_not_banned_global(deps.storage, &player)?;
    if MIGRATED_PLAYERS
        .may_load(deps.storage, &player)?
        .unwrap_or(false)
    {
        return Err(ContractError::SanguoAlreadyMigrated(player.to_string()));
    }

    let cards_query = serde_json::json!({
        "player_cards": { "address": player.to_string() }
    });
    let cards_resp: OldCardsResponse = deps
        .querier
        .query_wasm_smart(old_addr.to_string(), &cards_query)
        .map_err(|e| ContractError::SanguoOldContractQueryFailed(e.to_string()))?;

    let frags_query = serde_json::json!({
        "get_fragments": { "address": player.to_string() }
    });
    let frags_resp: OldFragmentsResponse = deps
        .querier
        .query_wasm_smart(old_addr.to_string(), &frags_query)
        .map_err(|e| ContractError::SanguoOldContractQueryFailed(e.to_string()))?;

    let player_str = player.to_string();
    let mut owned = PLAYER_CARDS
        .may_load(deps.storage, &player)?
        .unwrap_or_default();
    let mut minted = 0u32;
    let mut skipped = 0u32;
    for oc in cards_resp.cards {
        if MIGRATED_CARDS
            .may_load(deps.storage, &oc.card_id)?
            .unwrap_or(false)
        {
            skipped += 1;
            continue;
        }
        if oc.owner != player_str {
            skipped += 1;
            continue;
        }
        let new_card_id = generate_card_id(deps.storage, &player, "migrate")?;
        let card = CardInfo {
            card_id: new_card_id.clone(),
            owner: player_str.clone(),
            name: oc.name.clone(),
            rarity: oc.rarity,
            attack: oc.attack,
            defense: oc.defense,
            star: oc.star,
            level: oc.level,
        };
        CARDS.save(deps.storage, &new_card_id, &card)?;
        crate::sanguo::gift::on_card_migrated(deps.storage, &new_card_id)?;
        mark_collection(deps.storage, &player, &oc.name)?;
        owned.push(new_card_id);
        MIGRATED_CARDS.save(deps.storage, &oc.card_id, &true)?;
        minted += 1;
    }
    // 缺陷1 接线：改读 params 生效值
    let max_player_cards = load_params(deps.storage)?.max_player_cards() as usize;
    if owned.len() > max_player_cards {
        return Err(ContractError::SanguoTooManyCards(max_player_cards));
    }
    PLAYER_CARDS.save(deps.storage, &player, &owned)?;

    // 碎片：四桶 → 统一（M6 折算口径；老合约碎片直接入统一余额，
    // 不受 2 倍/50000 上限约束——来源为链上可查的真实资产）
    let old_frag = Fragments {
        common: frags_resp.common,
        rare: frags_resp.rare,
        epic: frags_resp.epic,
        legend: frags_resp.legend,
    };
    let unified = old_frag.to_unified();
    let bal = frag_balance(deps.storage, &player);
    set_frag_balance(deps.storage, &player, bal.saturating_add(unified))?;
    FRAG_ACQUIRED.save(deps.storage, (&player, "unified"), &0u64)?;

    MIGRATED_PLAYERS.save(deps.storage, &player, &true)?;

    Ok(Response::new()
        .add_attribute("action", "sanguo_migrate_from_old")
        .add_attribute("old_contract", old_contract)
        .add_attribute("player", player_str)
        .add_attribute("cards_minted", minted.to_string())
        .add_attribute("cards_skipped", skipped.to_string())
        .add_attribute("fragments_unified", unified.to_string()))
}

/// 查询已被放行的老合约地址列表
pub fn query_migration_whitelist(
    deps: cosmwasm_std::Deps,
) -> StdResult<SanguoMigrationWhitelistResponse> {
    let contracts: Vec<String> = MIGRATION_WHITELIST
        .range(deps.storage, None, None, Order::Ascending)
        .filter_map(|r| r.ok())
        .map(|(addr, _)| addr.to_string())
        .collect();
    Ok(SanguoMigrationWhitelistResponse { contracts })
}

// ============================================================================
// 合规配置 / 关卡 / 运营发碎片
// ============================================================================

/// 管理员：设置合规配置（C1：compliance_mode 已删——合规是唯一路径；
/// daily limits 直接写入 params）
pub fn set_compliance_config(
    deps: DepsMut,
    info: MessageInfo,
    operators: Option<Vec<String>>,
    daily_draw_limit: Option<u64>,
    daily_fragment_cap: Option<u64>,
) -> Result<Response, ContractError> {
    if !is_admin(deps.as_ref(), &info.sender)? {
        return Err(ContractError::Unauthorized);
    }
    let mut attrs = vec![];
    if let Some(ops) = operators {
        for a in &ops {
            deps.api.addr_validate(a)?;
        }
        OPERATOR_LIST.save(deps.storage, &ops)?;
        attrs.push(cosmwasm_std::Attribute::new("operators", ops.join(",")));
    }
    if daily_draw_limit.is_some() || daily_fragment_cap.is_some() {
        let mut params = load_params(deps.storage)?;
        if let Some(l) = daily_draw_limit {
            params.daily_draw_limit = l;
            attrs.push(cosmwasm_std::Attribute::new("daily_draw_limit", l.to_string()));
        }
        if let Some(c) = daily_fragment_cap {
            params.daily_fragment_cap = c;
            attrs.push(cosmwasm_std::Attribute::new("daily_fragment_cap", c.to_string()));
        }
        SANGUO_PARAMS.save(deps.storage, &params)?;
    }
    Ok(Response::new()
        .add_attribute("action", "sanguo_set_compliance_config")
        .add_attributes(attrs))
}

/// 判定调用者是否为运营白名单地址
fn is_operator(deps: &DepsMut, sender: &Addr) -> bool {
    OPERATOR_LIST
        .may_load(deps.storage)
        .ok()
        .flatten()
        .unwrap_or_default()
        .iter()
        .filter_map(|s| deps.api.addr_validate(s).ok())
        .any(|a| &a == sender)
}

/// 管理员/运营：整表写入关卡配置（覆盖式）
pub fn set_stages(
    deps: DepsMut,
    info: MessageInfo,
    stages: Vec<StageConfig>,
) -> Result<Response, ContractError> {
    if !is_admin(deps.as_ref(), &info.sender)? && !is_operator(&deps, &info.sender) {
        return Err(ContractError::Unauthorized);
    }
    let templates = CARD_TEMPLATES.may_load(deps.storage)?.unwrap_or_default();
    let mut seen_ids = std::collections::HashSet::new();
    for s in &stages {
        if s.id == 0 {
            return Err(ContractError::InvalidParams {
                reason: "stage id must be >= 1".to_string(),
            });
        }
        if !seen_ids.insert(s.id) {
            return Err(ContractError::SanguoDuplicateStage(s.id));
        }
        if s.name.trim().is_empty() {
            return Err(ContractError::InvalidParams {
                reason: format!("stage {} name empty", s.id),
            });
        }
        if s.enemy_tpl_ids.len() != 9 {
            return Err(ContractError::InvalidParams {
                reason: format!("stage {} enemy_tpl_ids must be 9", s.id),
            });
        }
        // E5：enemy_stars 可空（全 1★）或与 enemy_tpl_ids 等长
        if !s.enemy_stars.is_empty() && s.enemy_stars.len() != 9 {
            return Err(ContractError::InvalidParams {
                reason: format!("stage {} enemy_stars must be empty or 9", s.id),
            });
        }
        for tid in &s.enemy_tpl_ids {
            if !templates.iter().any(|t| &t.id == tid) {
                return Err(ContractError::SanguoStageEnemyNotFound(tid.clone()));
            }
        }
    }
    let count = stages.len() as u64;
    STAGES.save(deps.storage, &stages)?;
    Ok(Response::new()
        .add_attribute("action", "sanguo_set_stages")
        .add_attribute("stage_count", count.to_string()))
}

/// 运营/管理员：批量发统一碎片（每人每日硬顶内）
pub fn grant_fragments_batch(
    mut deps: DepsMut,
    env: &Env,
    info: MessageInfo,
    grants: Vec<FragmentGrant>,
) -> Result<Response, ContractError> {
    if !is_admin(deps.as_ref(), &info.sender)? && !is_operator(&deps, &info.sender) {
        return Err(ContractError::UnauthorizedOperator);
    }
    let mut granted_players = 0u32;
    let mut total_frags: u128 = 0;
    for g in &grants {
        let addr = deps.api.addr_validate(&g.player)?;
        grant_fragments_unified(&mut deps, env, &addr, g.fragments.to_unified())?;
        granted_players += 1;
        total_frags = total_frags.saturating_add(g.fragments.to_unified());
    }
    Ok(Response::new()
        .add_attribute("action", "sanguo_grant_fragments_batch")
        .add_attribute("players", granted_players.to_string())
        .add_attribute("total_fragments", total_frags.to_string()))
}

/// 玩家：批量提交关卡进度（链下打、链上复算；E5 Fixed 敌人 + 引擎复算）
pub fn submit_stage_batch(
    deps: &mut DepsMut,
    env: &Env,
    info: MessageInfo,
    stage_id: u64,
    batch_id: String,
    card_ids: Vec<String>,
    _round_id: String,
) -> Result<Response, ContractError> {
    if batch_id.trim().is_empty() {
        return Err(ContractError::SanguoEmptyBatchId);
    }
    let params = load_params(deps.storage)?;
    let player = sanguo_auth(deps, env, &info)?;
    ensure_not_banned_global(deps.storage, &player)?;
    ensure_game_active(deps.storage, "sanguo_stage")?;

    // 防重放：每关卡每玩家仅一次
    if STAGE_CLEARED
        .may_load(deps.storage, (&player, stage_id))?
        .is_some()
    {
        return Err(ContractError::SanguoAlreadyClaimed);
    }

    let stages = STAGES.may_load(deps.storage)?.unwrap_or_default();
    let stage = stages
        .iter()
        .find(|s| s.id == stage_id)
        .ok_or(ContractError::SanguoStageNotFound(stage_id))?
        .clone();

    // 引擎复算：玩家编队（校验过）vs 关卡 Fixed 敌人
    let player_team = load_battle_team_checked(deps.storage, &player, &card_ids)?;
    let templates = CARD_TEMPLATES.may_load(deps.storage)?.unwrap_or_default();
    let enemy_team = stage_enemy_cards(
        &templates,
        &stage,
        &params.upgrade_atk_boost(),
        &params.upgrade_def_boost(),
    )?;
    let ctx = BattleContext::from_params(&params);
    let result = compute_battle(&ctx, &player_team, &enemy_team)?;
    if result.winner != Winner::A {
        // G3：整场平局亦判失败；不发奖励、不写防重放标记
        return Err(ContractError::SanguoStageNotCleared);
    }

    grant_fragments_checked(deps, env, &player, &stage.reward)?;
    STAGE_CLEARED.save(deps.storage, (&player, stage_id), &batch_id)?;
    add_battle_stat(deps, &player, 3, true)?; // V2.1：关卡通关才计（失败 tx 回滚不计数）
    inc_daily_task(deps, &player, 0, env.block.time.seconds())?;

    let reward_total = stage.reward.to_unified();
    Ok(Response::new()
        .add_attribute("action", "sanguo_submit_stage_batch")
        .add_attribute("player", player.to_string())
        .add_attribute("stage_id", stage_id.to_string())
        .add_attribute("batch_id", batch_id)
        .add_attribute("reward_total", reward_total.to_string())
        .add_attribute("skill_log", skill_logs_json(&result.skill_logs)))
}

// ============================================================================
// 🆕 商店（T3/T14/T15 + 2.1 统一购买分发 + 9.1 P0 碎片闭环）
// ============================================================================

/// 内部：轮询选下一个项目方钱包
fn pick_project_wallet(deps: &mut DepsMut) -> Result<Addr, ContractError> {
    let wallets = crate::state::PROJECT_WALLETS
        .may_load(deps.storage)?
        .ok_or(ContractError::SanguoNoProjectWallet)?;
    if wallets.is_empty() {
        return Err(ContractError::SanguoNoProjectWallet);
    }
    let idx = crate::state::WALLET_INDEX.may_load(deps.storage)?.unwrap_or(0);
    let target = wallets[(idx as usize) % wallets.len()].clone();
    crate::state::WALLET_INDEX.save(deps.storage, &idx.wrapping_add(1))?;
    Ok(target)
}

/// C6：统一收款（原生 PAXI 或 PRC-20 内部余额扣款）——PRC-20 路径因
/// set_payment_token 恒拒非空实际不可达，保留为未来政策放开的零改动能力。
fn collect_payment(
    deps: &mut DepsMut,
    player: &Addr,
    price: Uint128,
    info: &MessageInfo,
    target: &Addr,
) -> Result<CosmosMsg, ContractError> {
    let pay_token = PAYMENT_TOKEN.may_load(deps.storage)?.unwrap_or_else(empty_addr);
    let empty = empty_addr();

    if pay_token == empty {
        for c in &info.funds {
            if c.denom != "upaxi" {
                return Err(ContractError::SanguoUnsupportedDenom {
                    denom: c.denom.clone(),
                });
            }
        }
        let funds_paid = info.funds.iter()
            .find(|c| c.denom == "upaxi")
            .map(|c| c.amount)
            .unwrap_or(Uint128::zero());
        let bal_key = (&player.clone(), &empty);

        if funds_paid >= price {
            let excess = funds_paid.checked_sub(price).map_err(|_| ContractError::Overflow)?;
            if !excess.is_zero() {
                let mut bal = BALANCES.may_load(deps.storage, bal_key)?.unwrap_or(Uint128::zero());
                bal = bal.checked_add(excess).map_err(|_| ContractError::Overflow)?;
                BALANCES.save(deps.storage, bal_key, &bal)?;
                crate::contract::update_user_totals(deps.branch(), &empty, excess, true)?;
            }
        } else {
            let shortfall = price.checked_sub(funds_paid).map_err(|_| ContractError::Overflow)?;
            let mut bal = BALANCES.may_load(deps.storage, bal_key)?.unwrap_or(Uint128::zero());
            if bal < shortfall {
                return Err(ContractError::SanguoInsufficientPayment {
                    expected: price.to_string(),
                    got: funds_paid.u128().saturating_add(bal.u128()).to_string(),
                });
            }
            bal = bal.checked_sub(shortfall).map_err(|_| ContractError::Overflow)?;
            BALANCES.save(deps.storage, bal_key, &bal)?;
            crate::contract::update_user_totals(deps.branch(), &empty, shortfall, false)?;
        }
        Ok(CosmosMsg::Bank(BankMsg::Send {
            to_address: target.to_string(),
            amount: vec![Coin { denom: "upaxi".to_string(), amount: price }],
        }))
    } else {
        if !info.funds.is_empty() {
            return Err(ContractError::SanguoUnsupportedDenom {
                denom: info.funds[0].denom.clone(),
            });
        }
        let bal_key = (&player.clone(), &pay_token);
        let mut bal = BALANCES.may_load(deps.storage, bal_key)?.unwrap_or(Uint128::zero());
        if bal < price {
            return Err(ContractError::SanguoInsufficientPayment {
                expected: price.to_string(),
                got: bal.u128().to_string(),
            });
        }
        bal = bal.checked_sub(price).map_err(|_| ContractError::Overflow)?;
        BALANCES.save(deps.storage, bal_key, &bal)?;
        crate::contract::update_user_totals(deps.branch(), &pay_token, price, false)?;
        Ok(CosmosMsg::Wasm(WasmMsg::Execute {
            contract_addr: pay_token.to_string(),
            msg: to_json_binary(&serde_json::json!({
                "transfer": {
                    "recipient": target.to_string(),
                    "amount": price.to_string(),
                }
            }))?,
            funds: vec![],
        }))
    }
}

/// admin 批量 upsert 商品表（未来加商品只改这张表，零 migrate）
pub fn set_shop_items(
    deps: DepsMut,
    info: MessageInfo,
    items: Vec<ShopItem>,
) -> Result<Response, ContractError> {
    if !is_admin(deps.as_ref(), &info.sender)? {
        return Err(ContractError::Unauthorized);
    }
    if items.len() > 200 {
        return Err(ContractError::InvalidParams {
            reason: "too many items (max 200 per call)".to_string(),
        });
    }
    let valid_types = [
        "skin", "avatar", "cardback", "title", "card", "stamina", "draw", "reset",
    ];
    let mut ids: BTreeSet<String> = BTreeSet::new();
    for it in &items {
        if it.id.trim().is_empty() {
            return Err(ContractError::InvalidParams {
                reason: "shop item id empty".to_string(),
            });
        }
        if !ids.insert(it.id.clone()) {
            return Err(ContractError::InvalidParams {
                reason: format!("duplicate shop item id: {}", it.id),
            });
        }
        if !valid_types.contains(&it.item_type.as_str()) {
            return Err(ContractError::InvalidParams {
                reason: format!("invalid item_type: {}", it.item_type),
            });
        }
        // 缺陷2：card 商品允许两价全空——成交价按模板稀有度回退
        //   params.card_frag_price（8.3/861：指定卡碎片价不逐条入表）。
        //   其余类型仍要求至少一个价。
        if it.item_type != "card" && it.price_paxi.is_none() && it.price_fragment.is_none() {
            return Err(ContractError::InvalidParams {
                reason: format!("item {} must have at least one price", it.id),
            });
        }
        SHOP_ITEMS.save(deps.storage, it.id.as_str(), it)?;
    }
    Ok(Response::new()
        .add_attribute("action", "sanguo_set_shop_items")
        .add_attribute("count", items.len().to_string()))
}

/// 每日购买计数读写（7.7：每玩家每日一条 BTreeMap，防存储膨胀）
fn daily_purchases_load(
    storage: &dyn Storage,
    player: &Addr,
    day: u64,
) -> StdResult<BTreeMap<String, u32>> {
    Ok(DAILY_PURCHASES
        .may_load(storage, (player, day))?
        .unwrap_or_default())
}

/// 统一购买分发（2.1：原 SanguoResetStage / SanguoBuyDrawCount 收编为分发臂）
pub fn buy_shop_item(
    mut deps: DepsMut,
    env: &Env,
    info: MessageInfo,
    item_id: String,
    target: Option<String>,
    pay_with_fragment: bool,
    _round_id: String,
) -> Result<Response, ContractError> {
    let item = SHOP_ITEMS
        .load(deps.storage, item_id.as_str())
        .map_err(|_| ContractError::SanguoShopItemNotFound(item_id.clone()))?;
    if !item.enabled {
        return Err(ContractError::SanguoShopItemDisabled(item_id.clone()));
    }
    // 价格（T14 双币种 Option）
    // 缺陷2 接线：card 商品两价字段全空时，按模板稀有度回退 params.card_frag_price[ri]
    //   （设计终稿 8.3/861：指定卡碎片价不逐条入表）。显式 price_fragment/price_paxi
    //   优先级不变，回退价仅用于碎片支付分支。
    let frag_fallback: Option<Uint128> =
        if item.price_paxi.is_none() && item.price_fragment.is_none() && item.item_type == "card" {
            let templates = CARD_TEMPLATES.may_load(deps.storage)?.unwrap_or_default();
            match templates.iter().find(|t| t.id == item.item_id || t.name == item.item_id) {
                Some(t) => {
                    let ri = Fragments::rarity_index(&t.rarity).unwrap_or(0);
                    Some(Uint128::from(load_params(deps.storage)?.card_frag_price()[ri]))
                }
                None => None,
            }
        } else {
            None
        };
    let price: Uint128 = if pay_with_fragment {
        item.price_fragment
            .or(frag_fallback)
            .ok_or(ContractError::SanguoShopCurrencyNotAccepted {
                item: item_id.clone(),
                currency: "fragment".to_string(),
            })?
    } else {
        item.price_paxi
            .ok_or(ContractError::SanguoShopCurrencyNotAccepted {
                item: item_id.clone(),
                currency: "paxi".to_string(),
            })?
    };

    let player = sanguo_auth(&mut deps, env, &info)?;
    ensure_not_banned_global(deps.storage, &player)?;

    let today = env.block.time.seconds() / SECS_PER_DAY;
    let mut bought_map = daily_purchases_load(deps.storage, &player, today)?;
    if item.daily_limit > 0 {
        let bought = bought_map.get(&item.id).copied().unwrap_or(0);
        if bought >= item.daily_limit {
            return Err(ContractError::SanguoShopDailyLimit {
                item: item.id.clone(),
                bought,
                limit: item.daily_limit,
            });
        }
    }

    let mut resp = Response::new();
    if pay_with_fragment {
        spend_fragments(deps.storage, &player, price.u128())?;
    } else {
        let target_addr = pick_project_wallet(&mut deps)?;
        let send_msg = collect_payment(&mut deps, &player, price, &info, &target_addr)?;
        resp = resp.add_message(send_msg)
            .add_attribute("received_by", target_addr.to_string());
    }

    // ===== 效果分发臂 =====
    match item.item_type.as_str() {
        "skin" | "avatar" | "cardback" | "title" => {
            // 拥有列表统一记 PLAYER_SKINS（化妆品位共用）
            let mut owned = PLAYER_SKINS.may_load(deps.storage, &player)?.unwrap_or_default();
            if owned.iter().any(|s| s == &item.item_id) {
                return Err(ContractError::SanguoSkinAlreadyOwned(item.item_id.clone()));
            }
            owned.push(item.item_id.clone());
            PLAYER_SKINS.save(deps.storage, &player, &owned)?;
            crate::sanguo::gift::on_skin_acquired(
                deps.storage, &player, &item.item_id, env.block.time.seconds(),
            )?;
            resp = resp.add_attribute("granted", item.item_id.clone());
        }
        "card" => {
            // 零价守卫（T3）：card_frag_price[ri] 可能为 0（管理员只改了数组部分元素），
            // 导致 0 碎片白拿卡；最终价为 0 一律拒绝（此处 price 已覆盖 PAXI/碎片两路径）。
            if price.is_zero() {
                return Err(ContractError::InvalidParams {
                    reason: "zero price not allowed".to_string(),
                });
            }
            // 指定卡购买：item_id = 模板 id（回退按名匹配）
            let templates = CARD_TEMPLATES.may_load(deps.storage)?.unwrap_or_default();
            let tpl = templates
                .iter()
                .find(|t| t.id == item.item_id || t.name == item.item_id)
                .ok_or(ContractError::SanguoNotForSale(item.item_id.clone()))?
                .clone();
            let card_id = generate_card_id(deps.storage, &player, "buy")?;
            let card = CardInfo {
                card_id: card_id.clone(),
                owner: player.to_string(),
                name: tpl.name.clone(),
                rarity: tpl.rarity.clone(),
                attack: tpl.attack,
                defense: tpl.defense,
                star: 1,
                level: 0,
            };
            CARDS.save(deps.storage, &card_id, &card)?;
            crate::sanguo::gift::on_card_created(
                deps.storage, &card_id, env.block.time.seconds(),
            )?;
            mark_collection(deps.storage, &player, &tpl.name)?;
            let mut owned = PLAYER_CARDS.may_load(deps.storage, &player)?.unwrap_or_default();
            owned.push(card_id.clone());
            // 缺陷1 接线：改读 params 生效值
            let max_player_cards = load_params(deps.storage)?.max_player_cards() as usize;
            if owned.len() > max_player_cards {
                return Err(ContractError::SanguoTooManyCards(max_player_cards));
            }
            PLAYER_CARDS.save(deps.storage, &player, &owned)?;
            resp = resp.add_attribute("card_id", card_id);
        }
        "stamina" => {
            let params = load_params(deps.storage)?;
            let st = load_stamina(&deps, &player, env.block.time.seconds(),
                params.stamina_max(), params.stamina_regen_secs());
            if st.current >= params.stamina_max() {
                return Err(ContractError::SanguoStaminaFull {
                    current: st.current,
                    max: params.stamina_max(),
                });
            }
            let mut st = st;
            st.current = (st.current + params.stamina_pack_amount()).min(params.stamina_max());
            st.last_update = env.block.time.seconds();
            save_stamina(&mut deps, &player, &st)?;
            resp = resp.add_attribute("stamina_gained", params.stamina_pack_amount().to_string());
        }
        "draw" => {
            // 加购抽卡：+1 次（受 params.daily_draw_purchase_cap 日限）
            let params = load_params(deps.storage)?;
            let cap = params.daily_draw_purchase_cap();
            let mut purchased = SG_DRAW_PURCHASED
                .may_load(deps.storage, (&player, today))?
                .unwrap_or(0);
            if purchased >= cap {
                return Err(ContractError::SanguoDrawLimitReached { limit: cap as u64 });
            }
            purchased += 1;
            SG_DRAW_PURCHASED.save(deps.storage, (&player, today), &purchased)?;
            resp = resp.add_attribute("draws_purchased", purchased.to_string());
        }
        "reset" => {
            // 关卡重置：target = stage_id（字符串数字）
            let stage_id: u64 = target
                .as_ref()
                .ok_or_else(|| ContractError::InvalidParams {
                    reason: "reset requires target = stage_id".to_string(),
                })?
                .parse()
                .map_err(|_| ContractError::InvalidParams {
                    reason: "target must be numeric stage_id".to_string(),
                })?;
            if STAGE_CLEARED
                .may_load(deps.storage, (&player, stage_id))?
                .is_none()
            {
                return Err(ContractError::InvalidParams {
                    reason: format!("stage {} not cleared yet", stage_id),
                });
            }
            STAGE_CLEARED.remove(deps.storage, (&player, stage_id));
            resp = resp.add_attribute("reset_stage", stage_id.to_string());
        }
        _ => {
            return Err(ContractError::InvalidParams {
                reason: format!("unsupported item_type: {}", item.item_type),
            });
        }
    }

    // 记录每日购买计数（限购 >0 时）
    if item.daily_limit > 0 {
        *bought_map.entry(item.id.clone()).or_insert(0) += 1;
        DAILY_PURCHASES.save(deps.storage, (&player, today), &bought_map)?;
    }

    Ok(resp
        .add_attribute("action", "sanguo_buy_shop_item")
        .add_attribute("player", player.to_string())
        .add_attribute("item_id", item.id)
        .add_attribute(
            "paid",
            if pay_with_fragment {
                format!("fragment:{}", price)
            } else {
                format!("upaxi:{}", price)
            },
        ))
}

// ============================================================================
// 🟢 7A/C6：支付代币（硬编码锁原生 PAXI）
// ============================================================================

pub fn set_payment_token(
    deps: DepsMut,
    info: MessageInfo,
    token: String,
) -> Result<Response, ContractError> {
    if !is_admin(deps.as_ref(), &info.sender)? {
        return Err(ContractError::Unauthorized);
    }
    let t = token.trim().to_string();
    if t.is_empty() {
        // 空串 = 原生 PAXI（默认行为不变）
        PAYMENT_TOKEN.save(deps.storage, &empty_addr())?;
        return Ok(Response::new()
            .add_attribute("action", "sanguo_set_payment_token")
            .add_attribute("token", "upaxi"));
    }
    // 🟢 政策放开（2026-10-14 用户拍板）：允许管理员把支付代币切到任意
    //    PRC-20（TKCC 等）。必须先 AddPrc20 进白名单——保证 Deposit 充值
    //    通道可用（collect_payment 的 PRC-20 分支从合约内部余额扣款），
    //    防止误填打错币导致玩家资金无法进出。
    let addr = deps.api.addr_validate(&t)?;
    if !PRC20_WHITELIST.has(deps.storage, &addr) {
        return Err(ContractError::InvalidParams {
            reason: "pay token not in PRC20 whitelist; admin must AddPrc20 first".to_string(),
        });
    }
    PAYMENT_TOKEN.save(deps.storage, &addr)?;
    Ok(Response::new()
        .add_attribute("action", "sanguo_set_payment_token")
        .add_attribute("token", addr.to_string()))
}

// ============================================================================
// 付费系统：指定卡 PAXI 价 + 委托改造 4 入口
// ============================================================================

/// 管理员：设置指定卡 PAXI 价（碎片价走 params.card_frag_price，T15）
pub fn set_card_price(
    deps: DepsMut,
    info: MessageInfo,
    card_name: String,
    price: Uint128,
) -> Result<Response, ContractError> {
    if !is_admin(deps.as_ref(), &info.sender)? {
        return Err(ContractError::Unauthorized);
    }
    if price.is_zero() {
        return Err(ContractError::SanguoInvalidPaxiAmount);
    }
    let templates = CARD_TEMPLATES.may_load(deps.storage)?.unwrap_or_default();
    if !templates.iter().any(|t| t.name == card_name) {
        return Err(ContractError::SanguoNotForSale(card_name.clone()));
    }
    crate::state::CARD_PRICES.save(deps.storage, &card_name, &price)?;
    Ok(Response::new()
        .add_attribute("action", "sanguo_set_card_price")
        .add_attribute("card_name", card_name)
        .add_attribute("price", price.to_string()))
}

/// C5：废弃清值但入口保留（写码对账注记见文件头；传空列表即清值）
pub fn set_project_wallets(
    deps: DepsMut,
    info: MessageInfo,
    wallets: Vec<String>,
) -> Result<Response, ContractError> {
    if !is_admin(deps.as_ref(), &info.sender)? {
        return Err(ContractError::Unauthorized);
    }
    if wallets.len() > 10 {
        return Err(ContractError::InvalidParams {
            reason: "wallets count must be 0..=10 (empty clears)".to_string(),
        });
    }
    let mut addrs = Vec::with_capacity(wallets.len());
    for w in &wallets {
        addrs.push(deps.api.addr_validate(w)?);
    }
    crate::state::PROJECT_WALLETS.save(deps.storage, &addrs)?;
    Ok(Response::new()
        .add_attribute("action", "sanguo_set_project_wallets")
        .add_attribute("count", wallets.len().to_string()))
}

/// 玩家：买指定卡。委托改造：复用商店 card 臂定价逻辑（2.1）。
/// 定价源 = CARD_PRICES（PAXI 价沿用 SanguoSetCardPrice，T15）；
/// 缺陷2 接线：CARD_PRICES 未配置（无价）时，按模板稀有度回退
///   params.card_frag_price[ri] 作为**碎片价**并改走碎片扣款（设计终稿 8.3/861）。
pub fn buy_card(
    mut deps: DepsMut,
    env: &Env,
    info: MessageInfo,
    card_name: String,
    _round_id: String,
) -> Result<Response, ContractError> {
    // 模板先解析（回退定价需稀有度）
    let templates = CARD_TEMPLATES.may_load(deps.storage)?.unwrap_or_default();
    let tpl = templates.iter()
        .find(|t| t.name == card_name)
        .ok_or(ContractError::SanguoNoTemplates)?
        .clone();
    // 显式 PAXI 价优先；缺失时按稀有度回退碎片价
    let paxi_price = crate::state::CARD_PRICES.may_load(deps.storage, &card_name)?;
    let pay_with_fragment = paxi_price.is_none();
    let price: Uint128 = match paxi_price {
        Some(p) => p,
        None => {
            let ri = Fragments::rarity_index(&tpl.rarity).unwrap_or(0);
            Uint128::from(load_params(deps.storage)?.card_frag_price()[ri])
        }
    };
    // 零价守卫（T3）：回退价 card_frag_price[ri] 可能为 0（管理员只改了数组部分元素），
    // 导致 0 碎片白拿卡；最终价为 0 一律拒绝（覆盖 PAXI/碎片两路径）。
    if price.is_zero() {
        return Err(ContractError::InvalidParams {
            reason: "zero price not allowed".to_string(),
        });
    }
    let player = sanguo_auth(&mut deps, env, &info)?;
    ensure_not_banned_global(deps.storage, &player)?;

    let mut resp = Response::new();
    if pay_with_fragment {
        spend_fragments(deps.storage, &player, price.u128())?;
    } else {
        let target = pick_project_wallet(&mut deps)?;
        let send_msg = collect_payment(&mut deps, &player, price, &info, &target)?;
        resp = resp
            .add_message(send_msg)
            .add_attribute("received_by", target.to_string());
    }

    let card_id = generate_card_id(deps.storage, &player, "buy")?;
    let card = CardInfo {
        card_id: card_id.clone(),
        owner: player.to_string(),
        name: tpl.name.clone(),
        rarity: tpl.rarity.clone(),
        attack: tpl.attack,
        defense: tpl.defense,
        star: 1,
        level: 0,
    };
    CARDS.save(deps.storage, &card_id, &card)?;
    crate::sanguo::gift::on_card_created(deps.storage, &card_id, env.block.time.seconds())?;
    mark_collection(deps.storage, &player, &tpl.name)?;
    let mut owned = PLAYER_CARDS.may_load(deps.storage, &player)?.unwrap_or_default();
    owned.push(card_id.clone());
    // 缺陷1 接线：改读 params 生效值
    let max_player_cards = load_params(deps.storage)?.max_player_cards() as usize;
    if owned.len() > max_player_cards {
        return Err(ContractError::SanguoTooManyCards(max_player_cards));
    }
    PLAYER_CARDS.save(deps.storage, &player, &owned)?;

    inc_daily_task(&mut deps, &player, 2, env.block.time.seconds())?;

    Ok(resp
        .add_attribute("action", "sanguo_buy_card")
        .add_attribute("player", player.to_string())
        .add_attribute("card_name", card_name)
        .add_attribute("card_id", card_id)
        .add_attribute("rarity", tpl.rarity)
        .add_attribute(
            "paid",
            if pay_with_fragment {
                format!("fragment:{}", price)
            } else {
                format!("upaxi:{}", price)
            },
        ))
}

/// 玩家：买皮肤（PAXI 明码标价，纯外观不可转让）
pub fn buy_skin(
    mut deps: DepsMut,
    env: &Env,
    info: MessageInfo,
    skin_id: String,
    _round_id: String,
) -> Result<Response, ContractError> {
    let skin = crate::sanguo::state::SKINS
        .may_load(deps.storage, &skin_id)?
        .ok_or_else(|| ContractError::SanguoSkinNotFound(skin_id.clone()))?;
    let player = sanguo_auth(&mut deps, env, &info)?;
    ensure_not_banned_global(deps.storage, &player)?;
    let mut owned = PLAYER_SKINS.may_load(deps.storage, &player)?.unwrap_or_default();
    if owned.iter().any(|s| s == &skin_id) {
        return Err(ContractError::SanguoSkinAlreadyOwned(skin_id));
    }
    let target = pick_project_wallet(&mut deps)?;
    let send_msg = collect_payment(&mut deps, &player, skin.price_paxi, &info, &target)?;
    owned.push(skin_id.clone());
    PLAYER_SKINS.save(deps.storage, &player, &owned)?;
    crate::sanguo::gift::on_skin_acquired(deps.storage, &player, &skin_id, env.block.time.seconds())?;
    Ok(Response::new()
        .add_message(send_msg)
        .add_attribute("action", "sanguo_buy_skin")
        .add_attribute("player", player.to_string())
        .add_attribute("skin_id", skin_id)
        .add_attribute("paid_upaxi", skin.price_paxi.to_string())
        .add_attribute("received_by", target.to_string()))
}

/// 玩家：购买体力（PAXI 明码标价，确定获得）
pub fn buy_stamina(
    mut deps: DepsMut,
    env: &Env,
    info: MessageInfo,
    pack_count: u32,
    _round_id: String,
) -> Result<Response, ContractError> {
    if pack_count == 0 || pack_count > 100 {
        return Err(ContractError::SanguoInvalidPackCount);
    }
    let params = load_params(deps.storage)?;
    let unit = params.stamina_price();
    let price = unit
        .checked_mul(Uint128::from(pack_count as u128))
        .map_err(|_| ContractError::Overflow)?;
    let s_max = params.stamina_max();
    let s_regen = params.stamina_regen_secs();
    let s_pack = params.stamina_pack_amount();
    let player = sanguo_auth(&mut deps, env, &info)?;
    ensure_not_banned_global(deps.storage, &player)?;
    let target = pick_project_wallet(&mut deps)?;

    // M-3：满体力购买会蒸发，直接拒绝
    let st_check = load_stamina(&deps, &player, env.block.time.seconds(), s_max, s_regen);
    if st_check.current >= s_max {
        return Err(ContractError::SanguoStaminaFull {
            current: st_check.current,
            max: s_max,
        });
    }

    let send_msg = collect_payment(&mut deps, &player, price, &info, &target)?;
    let mut st = load_stamina(&deps, &player, env.block.time.seconds(), s_max, s_regen);
    // T4 数值加固：pack 数 * 单包点数用饱和乘，防溢出
    let gained = s_pack.saturating_mul(pack_count as u64);
    st.current = (st.current + gained).min(s_max);
    st.last_update = env.block.time.seconds();
    save_stamina(&mut deps, &player, &st)?;

    Ok(Response::new()
        .add_message(send_msg)
        .add_attribute("action", "sanguo_buy_stamina")
        .add_attribute("player", player.to_string())
        .add_attribute("packs", pack_count.to_string())
        .add_attribute("stamina_gained", gained.to_string())
        .add_attribute("paid_upaxi", price.to_string())
        .add_attribute("received_by", target.to_string()))
}

/// 玩家：购买赛季通行证（PAXI 明码标价）
pub fn buy_season_pass(
    mut deps: DepsMut,
    env: &Env,
    info: MessageInfo,
    _round_id: String,
) -> Result<Response, ContractError> {
    let cfg = SEASON_CONFIG
        .may_load(deps.storage)?
        .ok_or(ContractError::SanguoSeasonNotActive)?;
    if !season_active(&cfg, env.block.time.seconds()) {
        return Err(ContractError::SanguoSeasonNotActive);
    }
    let player = sanguo_auth(&mut deps, env, &info)?;
    ensure_not_banned_global(deps.storage, &player)?;
    let mut ps = PLAYER_SEASON.may_load(deps.storage, &player)?.unwrap_or_default();
    if ps.pass_owned {
        return Err(ContractError::SanguoSeasonPassOwned);
    }
    let target = pick_project_wallet(&mut deps)?;
    let send_msg = collect_payment(&mut deps, &player, cfg.pass_price_paxi, &info, &target)?;
    ps.pass_owned = true;
    PLAYER_SEASON.save(deps.storage, &player, &ps)?;
    Ok(Response::new()
        .add_message(send_msg)
        .add_attribute("action", "sanguo_buy_season_pass")
        .add_attribute("player", player.to_string())
        .add_attribute("paid_upaxi", cfg.pass_price_paxi.to_string())
        .add_attribute("received_by", target.to_string()))
}

/// 玩家：领取赛季层级奖励（统一碎片）
pub fn claim_season_tier(
    mut deps: DepsMut,
    env: &Env,
    info: MessageInfo,
    tier_index: u64,
    _round_id: String,
) -> Result<Response, ContractError> {
    let cfg = SEASON_CONFIG
        .may_load(deps.storage)?
        .ok_or(ContractError::SanguoSeasonNotActive)?;
    if !season_active(&cfg, env.block.time.seconds()) {
        return Err(ContractError::SanguoSeasonNotActive);
    }
    let player = sanguo_auth(&mut deps, env, &info)?;
    ensure_not_banned_global(deps.storage, &player)?;
    let mut ps = PLAYER_SEASON.may_load(deps.storage, &player)?.unwrap_or_default();
    if !ps.pass_owned {
        return Err(ContractError::SanguoNoSeasonPass);
    }
    let tier = cfg
        .tiers
        .get(tier_index as usize)
        .ok_or(ContractError::SanguoSeasonTierNotFound(tier_index))?;
    if ps.points < tier.points_req {
        return Err(ContractError::SanguoSeasonTierNotReached {
            have: ps.points,
            need: tier.points_req,
        });
    }
    if ps.claimed.iter().any(|t| *t == tier_index) {
        return Err(ContractError::SanguoSeasonTierClaimed(tier_index));
    }
    grant_fragments_checked(&mut deps, env, &player, &tier.reward)?;
    ps.claimed.push(tier_index);
    PLAYER_SEASON.save(deps.storage, &player, &ps)?;
    Ok(Response::new()
        .add_attribute("action", "sanguo_claim_season_tier")
        .add_attribute("player", player.to_string())
        .add_attribute("tier_index", tier_index.to_string()))
}

/// 玩家：领取每日任务奖励（统一碎片 + 赛季点）
pub fn claim_daily_task(
    mut deps: DepsMut,
    env: &Env,
    info: MessageInfo,
    task_id: u64,
    _round_id: String,
) -> Result<Response, ContractError> {
    let params = load_params(deps.storage)?;
    // 🟢 审计修复（P2）：task_count 与固定 4 槽 required/reward 数组取 min，
    //    防参数误配（count > 4）时 required[task_id] 越界 panic
    let task_count = (params.daily_task_count() as usize)
        .min(params.daily_task_required().len())
        .min(params.daily_task_reward().len());
    if (task_id as usize) >= task_count {
        return Err(ContractError::SanguoDailyTaskNotFound(task_id));
    }
    let player = sanguo_auth(&mut deps, env, &info)?;
    ensure_not_banned_global(deps.storage, &player)?;
    let day = daily_task_day(env.block.time.seconds());
    let mut st = DAILY_TASK
        .may_load(deps.storage, (&player, day))?
        .unwrap_or_else(|| DailyTaskState {
            progress: vec![0u64; task_count],
            claimed: vec![false; task_count],
        });
    // 🟢 防历史记录长度与新 task_count 不符导致越界：口径变化即按当日新表重建
    if st.progress.len() != task_count || st.claimed.len() != task_count {
        st = DailyTaskState {
            progress: vec![0u64; task_count],
            claimed: vec![false; task_count],
        };
    }
    if st.claimed[task_id as usize] {
        return Err(ContractError::SanguoDailyTaskClaimed);
    }
    let required = params.daily_task_required()[task_id as usize];
    let progress = st.progress[task_id as usize];
    if progress < required {
        return Err(ContractError::SanguoDailyTaskIncomplete { progress, required });
    }
    let reward = params.daily_task_reward()[task_id as usize];
    grant_fragments_checked(&mut deps, env, &player, &reward)?;
    st.claimed[task_id as usize] = true;
    DAILY_TASK.save(deps.storage, (&player, day), &st)?;
    add_season_points(&mut deps, &player, params.daily_task_season_points())?;
    let total = reward.to_unified();
    Ok(Response::new()
        .add_attribute("action", "sanguo_claim_daily_task")
        .add_attribute("player", player.to_string())
        .add_attribute("task_id", task_id.to_string())
        .add_attribute("reward_total", total.to_string()))
}

/// 管理员：设置皮肤目录（覆盖式）
pub fn set_skins(
    deps: DepsMut,
    info: MessageInfo,
    skins: Vec<SkinInfo>,
) -> Result<Response, ContractError> {
    if !is_admin(deps.as_ref(), &info.sender)? {
        return Err(ContractError::Unauthorized);
    }
    for s in &skins {
        if s.id.trim().is_empty() || s.price_paxi.is_zero() {
            return Err(ContractError::InvalidParams {
                reason: "skin id empty or price zero".to_string(),
            });
        }
    }
    for s in &skins {
        SKINS.save(deps.storage, &s.id, s)?;
    }
    Ok(Response::new()
        .add_attribute("action", "sanguo_set_skins")
        .add_attribute("count", skins.len().to_string()))
}

/// 管理员：设置赛季配置（覆盖式）
pub fn set_season_config(
    deps: DepsMut,
    info: MessageInfo,
    config: SeasonConfig,
) -> Result<Response, ContractError> {
    if !is_admin(deps.as_ref(), &info.sender)? {
        return Err(ContractError::Unauthorized);
    }
    if config.pass_price_paxi.is_zero() {
        return Err(ContractError::SanguoInvalidPaxiAmount);
    }
    if config.end_time <= config.start_time {
        return Err(ContractError::InvalidParams {
            reason: "season end must be after start".to_string(),
        });
    }
    SEASON_CONFIG.save(deps.storage, &config)?;
    Ok(Response::new()
        .add_attribute("action", "sanguo_set_season_config")
        .add_attribute("season_id", config.season_id.to_string()))
}

// ============================================================================
// B/C 档新入口：化妆品 / 卡锁 / 预设 / 暂停 / 押金退款
// ============================================================================

/// 装备化妆品（slot ∈ skin/avatar/cardback/title，校验已拥有）
pub fn equip_cosmetic(
    deps: &mut DepsMut,
    env: &Env,
    info: MessageInfo,
    slot: String,
    item_id: String,
    _round_id: String,
) -> Result<Response, ContractError> {
    if !COSMETIC_SLOTS.contains(&slot.as_str()) {
        return Err(ContractError::SanguoInvalidCosmeticSlot(slot));
    }
    let player = sanguo_auth(deps, env, &info)?;
    ensure_not_banned_global(deps.storage, &player)?;
    let owned = PLAYER_SKINS.may_load(deps.storage, &player)?.unwrap_or_default();
    if !owned.iter().any(|s| s == &item_id) {
        return Err(ContractError::SanguoCosmeticNotOwned {
            slot: slot.clone(),
            item: item_id.clone(),
        });
    }
    EQUIPPED_COSMETICS.save(deps.storage, (&player, slot.as_str()), &item_id)?;
    Ok(Response::new()
        .add_attribute("action", "sanguo_equip_cosmetic")
        .add_attribute("player", player.to_string())
        .add_attribute("slot", slot)
        .add_attribute("item_id", item_id))
}

pub fn unequip_cosmetic(
    deps: &mut DepsMut,
    env: &Env,
    info: MessageInfo,
    slot: String,
    _round_id: String,
) -> Result<Response, ContractError> {
    if !COSMETIC_SLOTS.contains(&slot.as_str()) {
        return Err(ContractError::SanguoInvalidCosmeticSlot(slot));
    }
    let player = sanguo_auth(deps, env, &info)?;
    EQUIPPED_COSMETICS.remove(deps.storage, (&player, slot.as_str()));
    Ok(Response::new()
        .add_attribute("action", "sanguo_unequip_cosmetic")
        .add_attribute("player", player.to_string())
        .add_attribute("slot", slot))
}

/// 锁卡（分解/赠送/预设写入前必查）
pub fn lock_card(
    deps: &mut DepsMut,
    env: &Env,
    info: MessageInfo,
    card_id: String,
    locked: bool,
    _round_id: String,
) -> Result<Response, ContractError> {
    let player = sanguo_auth(deps, env, &info)?;
    let card = CARDS
        .load(deps.storage, &card_id)
        .map_err(|_| ContractError::SanguoCardNotFound(card_id.clone()))?;
    if card.owner != player.to_string() {
        return Err(ContractError::Unauthorized);
    }
    if locked {
        LOCKED_CARDS.save(deps.storage, &card_id, &true)?;
    } else {
        LOCKED_CARDS.remove(deps.storage, &card_id);
    }
    Ok(Response::new()
        .add_attribute("action", "sanguo_lock_card")
        .add_attribute("card_id", card_id)
        .add_attribute("locked", locked.to_string()))
}

/// 写入队伍预设（preset_id ∈ 0..2；校验归属 + 去重 + Σcost + 锁卡）
pub fn set_team_preset(
    deps: &mut DepsMut,
    env: &Env,
    info: MessageInfo,
    preset_id: u8,
    card_ids: Vec<String>,
    name: String,
    _round_id: String,
) -> Result<Response, ContractError> {
    if preset_id >= MAX_TEAM_PRESETS {
        return Err(ContractError::SanguoTeamPresetSlotInvalid(preset_id));
    }
    let player = sanguo_auth(deps, env, &info)?;
    ensure_not_banned_global(deps.storage, &player)?;
    // 完整编队校验（==9 + 归属 + 去重 + Σcost）
    load_battle_team_checked(deps.storage, &player, &card_ids)?;
    // 锁卡必查（裁定 B 档：预设先查锁）
    for id in &card_ids {
        if LOCKED_CARDS.may_load(deps.storage, id)?.unwrap_or(false) {
            return Err(ContractError::SanguoCardLocked(id.clone()));
        }
    }
    TEAM_PRESETS.save(
        deps.storage,
        (&player, preset_id),
        &TeamPreset { name: name.clone(), card_ids },
    )?;
    Ok(Response::new()
        .add_attribute("action", "sanguo_set_team_preset")
        .add_attribute("player", player.to_string())
        .add_attribute("preset_id", preset_id.to_string())
        .add_attribute("name", name))
}

/// 一键切 PLAYER_BATTLE_ORDER
pub fn use_team_preset(
    deps: &mut DepsMut,
    env: &Env,
    info: MessageInfo,
    preset_id: u8,
    _round_id: String,
) -> Result<Response, ContractError> {
    if preset_id >= MAX_TEAM_PRESETS {
        return Err(ContractError::SanguoTeamPresetSlotInvalid(preset_id));
    }
    let player = sanguo_auth(deps, env, &info)?;
    ensure_not_banned_global(deps.storage, &player)?;
    let preset = TEAM_PRESETS
        .may_load(deps.storage, (&player, preset_id))?
        .ok_or(ContractError::SanguoTeamPresetSlotInvalid(preset_id))?;
    // 卡可能已被分解/赠送：逐张复核归属
    let owned = PLAYER_CARDS.may_load(deps.storage, &player)?.unwrap_or_default();
    for id in &preset.card_ids {
        if !owned.contains(id) {
            return Err(ContractError::SanguoCardNotFound(id.clone()));
        }
    }
    PLAYER_BATTLE_ORDER.save(deps.storage, &player, &preset.card_ids)?;
    Ok(Response::new()
        .add_attribute("action", "sanguo_use_team_preset")
        .add_attribute("player", player.to_string())
        .add_attribute("preset_id", preset_id.to_string()))
}

/// C 档：紧急暂停/恢复单玩法（game_id 如 sanguo_draw / sanguo_pvp / sanguo_ai / sanguo_stage）
pub fn pause_game(
    deps: DepsMut,
    info: MessageInfo,
    game_id: String,
    paused: bool,
) -> Result<Response, ContractError> {
    if !is_admin(deps.as_ref(), &info.sender)? {
        return Err(ContractError::Unauthorized);
    }
    let mut list = PAUSED_GAMES.may_load(deps.storage)?.unwrap_or_default();
    if paused {
        if !list.iter().any(|g| g == &game_id) {
            list.push(game_id.clone());
        }
    } else {
        list.retain(|g| g != &game_id);
    }
    PAUSED_GAMES.save(deps.storage, &list)?;
    Ok(Response::new()
        .add_attribute("action", "sanguo_pause_game")
        .add_attribute("game_id", game_id)
        .add_attribute("paused", paused.to_string()))
}

/// C 档：玩家领取提案押金退款（PAXI 键；M3 记入 PENDING_REFUNDS）
pub fn refund_proposal_deposit(
    deps: &mut DepsMut,
    env: &Env,
    info: MessageInfo,
    _round_id: String,
) -> Result<Response, ContractError> {
    let player = sanguo_auth(deps, env, &info)?;
    let amount = PENDING_REFUNDS
        .may_load(deps.storage, &player)?
        .unwrap_or(0);
    if amount == 0 {
        return Err(ContractError::SanguoNoPendingRefund);
    }
    PENDING_REFUNDS.remove(deps.storage, &player);
    // 押金锁定时已 add_house(empty)；退款 = 玩家内部余额加回 + 庄家减记
    let empty = empty_addr();
    credit_balance(deps, &player, &empty, Uint128::from(amount))?;
    sub_house(deps, &empty, Uint128::from(amount))?;
    Ok(Response::new()
        .add_attribute("action", "sanguo_refund_proposal_deposit")
        .add_attribute("player", player.to_string())
        .add_attribute("amount", amount.to_string()))
}

// ============================================================================
// M5/M6：V2 玩家数据分批迁移（admin 反复调用，limit ≤ 50，M7 防 OOG）
// ============================================================================

pub fn migrate_fragments(
    deps: &mut DepsMut,
    info: MessageInfo,
    start_after: Option<String>,
    limit: Option<u32>,
) -> Result<Response, ContractError> {
    if !is_admin(deps.as_ref(), &info.sender)? {
        return Err(ContractError::Unauthorized);
    }
    // 必须先跑 migrate 本体（MIGRATION_V2_DONE 防重入标记）
    if !MIGRATION_V2_DONE.may_load(deps.storage)?.unwrap_or(false) {
        return Err(ContractError::SanguoMigrationNotDone);
    }
    let limit = limit.unwrap_or(50);
    if limit == 0 || limit > 50 {
        return Err(ContractError::SanguoMigrationLimitTooLarge(limit));
    }

    // M6 折算上限：单账户折算 ≤2 倍原始碎片总数 且 ≤50000；
    // 超额不发放，入 PENDING_FRAGMENT_REVIEW（值 + 游标 0）待管理员审核
    // 🟢 审计修复（P1）：删除游标存在性校验。原校验要求 start_after 玩家必须仍有
    //   FRAGMENTS/PLAYER_BATTLE_ORDER 记录，但上一批正常迁移的 last_player
    //   恰好两个键都已清除（M6 remove 旧桶 + M5 remove 编队）——续跑必然
    //   SanguoMigrationCursorInvalid，分批迁移第二批起无法推进。
    //   Bound::exclusive 本身保证不重读：游标必须取上批 Response 的 last_player；
    //   传入其他地址只会从该位置起漏扫，属操作失误而非合约可防御项。
    // 🟢 审计修复（P0 类型不匹配）：`Map<&Addr, _>` 的 K = `&'static Addr`，
    //   而 `Bound::exclusive<T: Into<K>>` 要求「owned Addr → &'static Addr」的转换，
    //   cosmwasm-std/cw-storage-plus 均未提供该 impl（E0277）。运行期 Addr 也拿不到
    //   'static 引用，故改为「全量升序 + 显式跳过 ≤ 游标」的等价游标语义：
    //   地址存储键为原始 bech32 字节，且长度一致，字符串序 == 存储序。
    let cursor: Option<Addr> = match &start_after {
        Some(s) => Some(deps.api.addr_validate(s)?),
        None => None,
    };

    let params = load_params(deps.storage)?;
    let team_cap = params.team_cost_cap();
    let mut processed = 0u32;
    let mut migrated_total: u128 = 0;
    let mut pending_total = 0u32;
    let mut last_player = String::new();

    // 🟢 审计修复（P0 借用冲突）：range() 迭代器持有 storage 的不可变借用，
    //    循环体内 save/remove 同一 storage 会触发 E0502。先收集本批（K::Output
    //    为 owned Addr，不借用 storage），再遍历写入。
    let batch: Vec<(Addr, Fragments)> = FRAGMENTS
        .range(deps.storage, None, None, Order::Ascending)
        .filter(|r| match r {
            Ok((addr, _)) => match &cursor {
                Some(c) => addr.as_str() > c.as_str(),
                None => true,
            },
            Err(_) => true,
        })
        .take(limit as usize)
        .collect::<Result<Vec<_>, _>>()?;

    for (player, old) in batch {
        last_player = player.to_string();
        processed += 1;

        // ---- M6：四桶 → 统一 ----
        let raw_sum = old
            .common
            .saturating_add(old.rare)
            .saturating_add(old.epic)
            .saturating_add(old.legend);
        let converted = old.to_unified();
        // 🟢 口径修正（二审发现）：折算率 common×1 / rare×5 / epic×20 / legend×80 下，
        //   原判据「折算 ≤ 2×原始枚数」对任何持有 rare 及以上的玩家恒不成立
        //   （单枚比值即 ≥5），会把绝大多数老玩家碎片挂进人工审核——而
        //   PENDING_FRAGMENT_REVIEW 目前无自动发放通道，等同资产卡死。
        //   故改为「按最高折算率 80× 的 2 倍」做数据异常兜底（正常数据永不触发），
        //   真正生效的护栏是下一行的 5 万绝对上限。
        let over_ratio = converted > (raw_sum as u128).saturating_mul(80).saturating_mul(2);
        let over_abs = converted > 50_000u128;
        if over_ratio || over_abs {
            PENDING_FRAGMENT_REVIEW.save(deps.storage, &player, &(converted, 0u64))?;
            // 超额同样移除旧桶（裁定 M6「转换后旧桶 remove」覆盖两种路径）：
            // 防下一批重跑重复枚举、防审核发放后旧桶残留导致二次入账。
            FRAGMENTS.remove(deps.storage, &player);
            pending_total += 1;
        } else {
            let bal = frag_balance(deps.storage, &player);
            set_frag_balance(deps.storage, &player, bal.saturating_add(converted))?;
            FRAGMENTS.remove(deps.storage, &player);
            migrated_total = migrated_total.saturating_add(converted);
        }

        // ---- M5：超限编队补偿（裁定 T17：按 PLAYER_BATTLE_ORDER 镜像
        //      resolve_player_battle_cards = 实际参战 9 张：过滤非拥有卡 +
        //      去重 + 截 9；严禁按 PLAYER_CARDS 聚合。补偿后清编队 =
        //      天然一次性语义，对账注记 ①）----
        let order = PLAYER_BATTLE_ORDER.may_load(deps.storage, &player)?.unwrap_or_default();
        if !order.is_empty() {
            let squad = resolve_player_battle_cards(deps.storage, &player)?;
            let templates = CARD_TEMPLATES.may_load(deps.storage)?.unwrap_or_default();
            let tmap = template_map(&templates);
            let mut total_cost: u32 = 0;
            for c in &squad {
                total_cost = total_cost
                    .saturating_add(tmap.get(c.name.as_str()).map(|t| t.cost as u32).unwrap_or(0));
            }
            if total_cost > team_cap {
                let bal = frag_balance(deps.storage, &player);
                set_frag_balance(deps.storage, &player, bal.saturating_add(500))?;
                migrated_total = migrated_total.saturating_add(500);
            }
            PLAYER_BATTLE_ORDER.remove(deps.storage, &player);
        }
    }

    Ok(Response::new()
        .add_attribute("action", "sanguo_migrate_fragments")
        .add_attribute("processed", processed.to_string())
        .add_attribute("migrated_total", migrated_total.to_string())
        .add_attribute("pending_review", pending_total.to_string())
        .add_attribute("last_player", last_player))
}

// ============================================================================
// 体力 / 每日任务 / 赛季（参数化版本）
// ============================================================================

fn load_stamina(
    deps: &DepsMut,
    player: &Addr,
    now: u64,
    s_max: u64,
    s_regen: u64,
) -> StaminaInfo {
    let mut info = STAMINA
        .may_load(deps.storage, player)
        .unwrap_or_default()
        .unwrap_or_default();
    regen_stamina_with(&mut info, now, s_max, s_regen);
    info
}

fn save_stamina(deps: &mut DepsMut, player: &Addr, info: &StaminaInfo) -> Result<(), ContractError> {
    STAMINA.save(deps.storage, player, info)?;
    Ok(())
}

/// 按自然时间再生体力（参数化：max/regen 来自 params）
fn regen_stamina_with(info: &mut StaminaInfo, now: u64, s_max: u64, s_regen: u64) {
    if info.current >= s_max {
        info.last_update = now;
        return;
    }
    let elapsed = now.saturating_sub(info.last_update);
    let gained = elapsed / s_regen;
    if gained > 0 {
        info.current = (info.current + gained).min(s_max);
        info.last_update = info.last_update + gained * s_regen;
    }
}

/// 消耗体力（先再生再扣），不足报错
fn consume_stamina(
    deps: &mut DepsMut,
    player: &Addr,
    cost: u64,
    now: u64,
    s_max: u64,
    s_regen: u64,
) -> Result<(), ContractError> {
    let mut info = load_stamina(deps, player, now, s_max, s_regen);
    if info.current < cost {
        return Err(ContractError::SanguoInsufficientStamina {
            have: info.current,
            need: cost,
        });
    }
    info.current -= cost;
    if info.current >= s_max {
        info.last_update = now;
    }
    save_stamina(deps, player, &info)?;
    Ok(())
}

/// 给玩家加赛季点数（购买通行证不重置，赛季内累加）
fn add_season_points(
    deps: &mut DepsMut,
    player: &Addr,
    pts: u64,
) -> Result<(), ContractError> {
    let mut ps = PLAYER_SEASON.may_load(deps.storage, player)?.unwrap_or_default();
    ps.points = ps.points.saturating_add(pts);
    PLAYER_SEASON.save(deps.storage, player, &ps)?;
    Ok(())
}

/// 每日任务按 UTC 日对齐的索引
fn daily_task_day(now: u64) -> u64 {
    now / SECS_PER_DAY
}

/// 给玩家某每日任务进度 +1（按 UTC 日去重；进度向量长度 = params.daily_task_count）
fn inc_daily_task(
    deps: &mut DepsMut,
    player: &Addr,
    task_id: u64,
    now: u64,
) -> Result<(), ContractError> {
    let params = load_params(deps.storage)?;
    // 🟢 审计修复（P2）：与 claim_daily_task 同口径 min 防越界
    let count = (params.daily_task_count() as usize)
        .min(params.daily_task_required().len())
        .min(params.daily_task_reward().len());
    let day = daily_task_day(now);
    let mut st = DAILY_TASK
        .may_load(deps.storage, (player, day))?
        .unwrap_or_else(|| DailyTaskState {
            progress: vec![0u64; count],
            claimed: vec![false; count],
        });
    if (task_id as usize) < st.progress.len() {
        st.progress[task_id as usize] = st.progress[task_id as usize].saturating_add(1);
    }
    DAILY_TASK.save(deps.storage, (player, day), &st)?;
    Ok(())
}
