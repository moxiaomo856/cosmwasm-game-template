//! 🎁 赠送系统（Gift System v1）
//!
//! 支持资产：卡牌 / 碎片 / 皮肤
//! 门槛：账号 ≥30天 + 关系 ≥30天 + 资产冷静期 ≥30天（传说卡 7 天）
//! 关系过期：180 天无互动 → 懒删除
//!
//! ⚠️ 合规提示：碎片赠送意味着碎片在玩家之间「可转移」，与既有红线
//! 「只发不可转让碎片」存在张力。如需收紧，可禁用 SanguoGiftFragments 入口
//! （在 execute_sanguo 顶层拦截），不影响卡牌/皮肤赠送。

use cosmwasm_std::{Addr, Deps, DepsMut, Env, MessageInfo, Order, Response, StdResult};

use crate::admin::config::is_admin;
use crate::error::ContractError;
use crate::msg::{GiftCandidate, SanguoGiftCandidatesResponse, SanguoGiftStatusResponse};
use crate::sanguo::helpers::sanguo_auth;
use crate::sanguo::state::*;

// ============================================================================
// 工具
// ============================================================================

fn today(now: u64) -> u64 { now / SECS_PER_DAY }

fn holding_secs(params: &SanguoParams, rarity: &str) -> u64 {
    if rarity == "legend" { params.gift_legend_holding_secs() } else { params.gift_holding_secs() }
}

fn rarity_allowed(rarity: &str) -> bool {
    matches!(rarity, "common" | "rare" | "epic" | "legend")
}

// ============================================================================
// 挂钩函数（外部调用）
// ============================================================================

/// 新卡：从当前起算冷静期
pub fn on_card_created(
    storage: &mut dyn cosmwasm_std::Storage,
    card_id: &str,
    now: u64,
) -> StdResult<()> {
    CARD_LAST_GIFT.save(storage, card_id, &now)?;
    Ok(())
}

/// 迁移卡：`last_gift_at = 0` → 立即可送
pub fn on_card_migrated(
    storage: &mut dyn cosmwasm_std::Storage,
    card_id: &str,
) -> StdResult<()> {
    CARD_LAST_GIFT.save(storage, card_id, &0)?;
    Ok(())
}

/// 单稀有度碎片获得
pub fn on_fragments_gained(
    storage: &mut dyn cosmwasm_std::Storage,
    player: &Addr,
    rarity: &str,
    now: u64,
) -> StdResult<()> {
    FRAG_ACQUIRED.save(storage, (player, rarity), &now)?;
    Ok(())
}

/// 四档碎片批量获得（grant_fragments_checked 用）
pub fn on_fragments_gained_multi(
    storage: &mut dyn cosmwasm_std::Storage,
    player: &Addr,
    frags: (u64, u64, u64, u64),
    now: u64,
) -> StdResult<()> {
    let (c, r, e, l) = frags;
    if c > 0 { FRAG_ACQUIRED.save(storage, (player, "common"), &now)?; }
    if r > 0 { FRAG_ACQUIRED.save(storage, (player, "rare"), &now)?; }
    if e > 0 { FRAG_ACQUIRED.save(storage, (player, "epic"), &now)?; }
    if l > 0 { FRAG_ACQUIRED.save(storage, (player, "legend"), &now)?; }
    Ok(())
}

/// 四档碎片批量迁移（migrate_from_old 用）：`0 = 立即可送`
pub fn on_fragments_migrated_multi(
    storage: &mut dyn cosmwasm_std::Storage,
    player: &Addr,
    frags: (u64, u64, u64, u64),
) -> StdResult<()> {
    let (c, r, e, l) = frags;
    if c > 0 { FRAG_ACQUIRED.save(storage, (player, "common"), &0)?; }
    if r > 0 { FRAG_ACQUIRED.save(storage, (player, "rare"), &0)?; }
    if e > 0 { FRAG_ACQUIRED.save(storage, (player, "epic"), &0)?; }
    if l > 0 { FRAG_ACQUIRED.save(storage, (player, "legend"), &0)?; }
    Ok(())
}

/// 皮肤获得
pub fn on_skin_acquired(
    storage: &mut dyn cosmwasm_std::Storage,
    player: &Addr,
    skin_id: &str,
    now: u64,
) -> StdResult<()> {
    SKIN_LAST_ACQUIRED.save(storage, (player, skin_id), &now)?;
    Ok(())
}

/// 记录一次同场：首次写入；后续若距上次 ≥1 天则刷新 last_at
pub fn record_pair(
    storage: &mut dyn cosmwasm_std::Storage,
    a: &Addr,
    b: &Addr,
    now: u64,
) -> StdResult<()> {
    if a == b || a.as_str().is_empty() || b.as_str().is_empty() {
        return Ok(());
    }
    upsert_pair_one(storage, a, b, now)?;
    upsert_pair_one(storage, b, a, now)?;
    Ok(())
}

fn upsert_pair_one(
    storage: &mut dyn cosmwasm_std::Storage,
    a: &Addr,
    b: &Addr,
    now: u64,
) -> StdResult<()> {
    let key = (a, b);
    match PAIR_INFO.may_load(storage, key)? {
        None => {
            PAIR_INFO.save(storage, key, &PairInfo { first_at: now, last_at: now })?;
        }
        Some(mut info) => {
            // 缺陷1 接线（T1）：改读 params 生效值（本函数仅有 storage，写全路径 + 错误转换）
            let refresh = crate::sanguo::helpers::load_params(storage)
                .map_err(|e| cosmwasm_std::StdError::generic_err(e.to_string()))?
                .gift_pair_refresh_interval();
            if now.saturating_sub(info.last_at) >= refresh {
                info.last_at = now;
                PAIR_INFO.save(storage, key, &info)?;
            }
        }
    }
    Ok(())
}

/// Royale 结算后批量记录
pub fn record_royale_pairs(
    storage: &mut dyn cosmwasm_std::Storage,
    players: &[Addr],
    now: u64,
) -> StdResult<()> {
    for i in 0..players.len() {
        for j in (i + 1)..players.len() {
            record_pair(storage, &players[i], &players[j], now)?;
        }
    }
    Ok(())
}

// ============================================================================
// 门槛检查
// ============================================================================

fn ensure_not_banned(deps: &DepsMut, p: &Addr) -> Result<(), ContractError> {
    // T8：BANNED_PLAYERS（原 GIFT_BANNED 泛化，key "sg_gban" 不变）；
    // 赠送入口与全局玩法入口共用此检查（玩法入口在 exec.rs ensure_not_banned_global）
    if BANNED_PLAYERS.may_load(deps.storage, p)?.unwrap_or(false) {
        return Err(ContractError::SanguoGiftBanned);
    }
    Ok(())
}

fn ensure_account_age(deps: &mut DepsMut, env: &Env, p: &Addr) -> Result<(), ContractError> {
    let now = env.block.time.seconds();
    // 缺陷1 接线（T1）：账号年龄门槛改读 params 生效值
    let acct_age = crate::sanguo::helpers::load_params(deps.storage)?.gift_acct_age_secs();
    match ACCT_FIRST_SEEN.may_load(deps.storage, p)? {
        None => {
            ACCT_FIRST_SEEN.save(deps.storage, p, &now)?;
            Err(ContractError::SanguoGiftAccountTooYoung {
                created: now, need: acct_age,
            })
        }
        Some(t) => {
            let age = now.saturating_sub(t);
            if age < acct_age {
                Err(ContractError::SanguoGiftAccountTooYoung {
                    created: t, need: acct_age - age,
                })
            } else {
                Ok(())
            }
        }
    }
}

/// 关系门槛 + 半年无互动懒删除
fn ensure_pair(deps: &mut DepsMut, env: &Env, a: &Addr, b: &Addr) -> Result<(), ContractError> {
    let now = env.block.time.seconds();
    // 缺陷1 接线（T1）：关系门槛 / 过期时长改读 params 生效值
    let params = crate::sanguo::helpers::load_params(deps.storage)?;
    let info = match PAIR_INFO.may_load(deps.storage, (a, b))? {
        Some(i) => i,
        None => return Err(ContractError::SanguoNoRelation),
    };

    // 半年无互动 → 懒删除双向记录
    if now.saturating_sub(info.last_at) >= params.gift_pair_idle_expire_secs() {
        PAIR_INFO.remove(deps.storage, (a, b));
        PAIR_INFO.remove(deps.storage, (b, a));
        return Err(ContractError::SanguoNoRelation);
    }

    // 关系成立 ≥30 天
    let age = now.saturating_sub(info.first_at);
    if age < params.gift_pair_age_secs() {
        return Err(ContractError::SanguoRelationTooYoung {
            since: info.first_at,
            need: params.gift_pair_age_secs() - age,
        });
    }
    Ok(())
}

fn bump_sent(deps: &mut DepsMut, env: &Env, p: &Addr) -> Result<(), ContractError> {
    let day = today(env.block.time.seconds());
    // 缺陷1 接线（T1）：日赠上限改读 params 生效值
    let limit = crate::sanguo::helpers::load_params(deps.storage)?.gift_daily_sent();
    let used = GIFT_SENT_TODAY.may_load(deps.storage, (p, day))?.unwrap_or(0);
    if used >= limit {
        return Err(ContractError::SanguoGiftSentLimitReached {
            used, limit,
        });
    }
    GIFT_SENT_TODAY.save(deps.storage, (p, day), &(used + 1))?;
    Ok(())
}

fn bump_recv(deps: &mut DepsMut, env: &Env, p: &Addr) -> Result<(), ContractError> {
    let day = today(env.block.time.seconds());
    // 缺陷1 接线（T1）：日收上限改读 params 生效值
    let limit = crate::sanguo::helpers::load_params(deps.storage)?.gift_daily_recv();
    let used = GIFT_RECV_TODAY.may_load(deps.storage, (p, day))?.unwrap_or(0);
    if used >= limit {
        return Err(ContractError::SanguoGiftReceivedLimitReached {
            used, limit,
        });
    }
    GIFT_RECV_TODAY.save(deps.storage, (p, day), &(used + 1))?;
    Ok(())
}

// ============================================================================
// 赠送：卡牌
// ============================================================================

pub fn gift_card(
    mut deps: DepsMut,
    env: &Env,
    info: MessageInfo,
    card_id: String,
    to: String,
    _round_id: String,
) -> Result<Response, ContractError> {
    let to_addr = deps.api.addr_validate(&to)?;

    let player = sanguo_auth(&mut deps, env, &info)?;

    if player == to_addr {
        return Err(ContractError::SanguoCannotGiftSelf);
    }

    ensure_not_banned(&deps, &player)?;
    ensure_account_age(&mut deps, env, &player)?;
    ensure_pair(&mut deps, env, &player, &to_addr)?;

    let mut card = CARDS
        .load(deps.storage, &card_id)
        .map_err(|_| ContractError::SanguoCardNotFound(card_id.clone()))?;
    if card.owner != player.to_string() {
        return Err(ContractError::Unauthorized);
    }
    if !rarity_allowed(&card.rarity) {
        return Err(ContractError::SanguoGiftRarityNotAllowed {
            rarity: card.rarity,
        });
    }

    // 冷静期
    let now = env.block.time.seconds();
    // 缺陷1 接线（T1）：本函数后续 max_player_cards 复用同一份 params
    let params = crate::sanguo::helpers::load_params(deps.storage)?;
    let last_gift = CARD_LAST_GIFT
        .may_load(deps.storage, &card_id)?
        .ok_or_else(|| ContractError::SanguoGiftCardMetaMissing {
            card_id: card_id.clone(),
        })?;
    if last_gift > 0 {
        let keep = holding_secs(&params, &card.rarity);
        let elapsed = now.saturating_sub(last_gift);
        if elapsed < keep {
            return Err(ContractError::SanguoGiftHoldingPeriodNotMet {
                acquired_at: last_gift,
                need: keep - elapsed,
            });
        }
    }

    // 🟢 审计修复（P2）：锁卡必查——lock_card 注释承诺"分解/赠送/预设写入前必查"，
    //    此前赠送路径漏检，锁卡可被绕过送出
    if LOCKED_CARDS.may_load(deps.storage, &card_id)?.unwrap_or(false) {
        return Err(ContractError::SanguoCardLocked(card_id.clone()));
    }

    bump_sent(&mut deps, env, &player)?;
    bump_recv(&mut deps, env, &to_addr)?;

    // ===== 转移 =====
    let rarity = card.rarity.clone();
    card.owner = to_addr.to_string();
    CARDS.save(deps.storage, &card_id, &card)?;

    let mut from_list = PLAYER_CARDS.may_load(deps.storage, &player)?.unwrap_or_default();
    from_list.retain(|id| id != &card_id);
    PLAYER_CARDS.save(deps.storage, &player, &from_list)?;

    // 出战顺序自动移除
    let mut order = PLAYER_BATTLE_ORDER.may_load(deps.storage, &player)?.unwrap_or_default();
    let before = order.len();
    order.retain(|id| id != &card_id);
    if order.len() != before {
        PLAYER_BATTLE_ORDER.save(deps.storage, &player, &order)?;
    }

    let mut to_list = PLAYER_CARDS.may_load(deps.storage, &to_addr)?.unwrap_or_default();
    // 🟢 审计修复（P2）：接收方卡数上限（此前赠送可绕过 MAX_PLAYER_CARDS）
    // 缺陷1 接线：复用函数顶部已读的 params（避免重复 load）
    let max_player_cards = params.max_player_cards() as usize;
    if to_list.len() >= max_player_cards {
        return Err(ContractError::SanguoTooManyCards(max_player_cards));
    }
    to_list.push(card_id.clone());
    PLAYER_CARDS.save(deps.storage, &to_addr, &to_list)?;

    CARD_LAST_GIFT.save(deps.storage, &card_id, &now)?;

    Ok(Response::new()
        .add_attribute("action", "sanguo_gift_card")
        .add_attribute("from", player)
        .add_attribute("to", to_addr)
        .add_attribute("card_id", card_id)
        .add_attribute("rarity", rarity))
}

// ============================================================================
// 赠送：碎片
// ============================================================================

pub fn gift_fragments(
    mut deps: DepsMut,
    env: &Env,
    info: MessageInfo,
    rarity: String,
    amount: u64,
    to: String,
    _round_id: String,
) -> Result<Response, ContractError> {
    // 🟢 V2（9.1 P0 经济闭环）：碎片统一为 FRAGMENT_BALANCE（u128 统一币）。
    //    签名保留 { rarity, amount } 以兼容前端：rarity 仅校验合法性，实际转移
    //    amount 个统一碎片（四桶口径随 migrate M6 折算进统一余额）。
    //    冷静期统一记 FRAG_ACQUIRED[(player, "unified")]（V1 按稀有度分桶冷却
    //    在统一币下无意义；放宽口径变更留专家审计确认）。
    // 🟢 碎片可转让（用户决策 2026-10-12）：不碰 TKCC，不违反零 TKCC 红线。
    // 缺陷1 接线（T1）：日赠上限 / 冷静期均改读 params 生效值（函数内复用）
    let params = crate::sanguo::helpers::load_params(deps.storage)?;
    let frag_cap = params.gift_frag_cap();
    if amount == 0 || amount as u128 > frag_cap as u128 {
        return Err(ContractError::SanguoGiftFragmentCapExceeded {
            amount, max: frag_cap,
        });
    }
    if Fragments::rarity_index(&rarity).is_none() {
        return Err(ContractError::SanguoInvalidRarity(rarity));
    }

    let to_addr = deps.api.addr_validate(&to)?;

    let player = sanguo_auth(&mut deps, env, &info)?;

    if player == to_addr {
        return Err(ContractError::SanguoCannotGiftSelf);
    }

    ensure_not_banned(&deps, &player)?;
    ensure_account_age(&mut deps, env, &player)?;
    ensure_pair(&mut deps, env, &player, &to_addr)?;

    let now = env.block.time.seconds();

    // 冷静期（统一碎片键）
    let holding = params.gift_holding_secs();
    if let Some(t) = FRAG_ACQUIRED.may_load(deps.storage, (&player, "unified"))? {
        if t > 0 {
            let elapsed = now.saturating_sub(t);
            if elapsed < holding {
                return Err(ContractError::SanguoGiftHoldingPeriodNotMet {
                    acquired_at: t,
                    need: holding - elapsed,
                });
            }
        }
    }

    bump_sent(&mut deps, env, &player)?;
    bump_recv(&mut deps, env, &to_addr)?;

    // ===== 转移（统一碎片币）=====
    let from_bal = crate::sanguo::exec::frag_balance(deps.storage, &player);
    if from_bal < amount as u128 {
        return Err(ContractError::SanguoInsufficientFragments {
            needed: amount,
            have: from_bal as u64,
        });
    }
    crate::sanguo::exec::set_frag_balance(deps.storage, &player, from_bal - amount as u128)?;
    let to_bal = crate::sanguo::exec::frag_balance(deps.storage, &to_addr);
    crate::sanguo::exec::set_frag_balance(deps.storage, &to_addr, to_bal.saturating_add(amount as u128))?;

    FRAG_ACQUIRED.save(deps.storage, (&player, "unified"), &now)?;
    FRAG_ACQUIRED.save(deps.storage, (&to_addr, "unified"), &now)?;

    Ok(Response::new()
        .add_attribute("action", "sanguo_gift_fragments")
        .add_attribute("from", player)
        .add_attribute("to", to_addr)
        .add_attribute("rarity", rarity)
        .add_attribute("amount", amount.to_string()))
}

// ============================================================================
// 赠送：皮肤
// ============================================================================

pub fn gift_skin(
    mut deps: DepsMut,
    env: &Env,
    info: MessageInfo,
    skin_id: String,
    to: String,
    _round_id: String,
) -> Result<Response, ContractError> {
    let to_addr = deps.api.addr_validate(&to)?;

    let player = sanguo_auth(&mut deps, env, &info)?;

    if player == to_addr {
        return Err(ContractError::SanguoCannotGiftSelf);
    }

    ensure_not_banned(&deps, &player)?;
    ensure_account_age(&mut deps, env, &player)?;
    ensure_pair(&mut deps, env, &player, &to_addr)?;

    // 源拥有
    let mut from_skins = PLAYER_SKINS.may_load(deps.storage, &player)?.unwrap_or_default();
    if !from_skins.iter().any(|s| s == &skin_id) {
        return Err(ContractError::SanguoSkinNotFound(skin_id.clone()));
    }

    // 目标未拥有（防重复）
    let mut to_skins = PLAYER_SKINS.may_load(deps.storage, &to_addr)?.unwrap_or_default();
    if to_skins.iter().any(|s| s == &skin_id) {
        return Err(ContractError::SanguoSkinAlreadyOwned(skin_id.clone()));
    }

    // 冷静期
    let now = env.block.time.seconds();
    // 缺陷1 接线（T1）：皮肤冷静期改读 params 生效值
    let holding = crate::sanguo::helpers::load_params(deps.storage)?.gift_holding_secs();
    let last = SKIN_LAST_ACQUIRED
        .may_load(deps.storage, (&player, &skin_id))?
        .unwrap_or(0);
    if last > 0 {
        let elapsed = now.saturating_sub(last);
        if elapsed < holding {
            return Err(ContractError::SanguoGiftHoldingPeriodNotMet {
                acquired_at: last,
                need: holding - elapsed,
            });
        }
    }

    bump_sent(&mut deps, env, &player)?;
    bump_recv(&mut deps, env, &to_addr)?;

    // ===== 转移 =====
    from_skins.retain(|s| s != &skin_id);
    PLAYER_SKINS.save(deps.storage, &player, &from_skins)?;

    to_skins.push(skin_id.clone());
    PLAYER_SKINS.save(deps.storage, &to_addr, &to_skins)?;

    SKIN_LAST_ACQUIRED.save(deps.storage, (&to_addr, &skin_id), &now)?;

    Ok(Response::new()
        .add_attribute("action", "sanguo_gift_skin")
        .add_attribute("from", player)
        .add_attribute("to", to_addr)
        .add_attribute("skin_id", skin_id))
}

// ============================================================================
// 管理端
// ============================================================================

pub fn set_gift_ban(
    deps: DepsMut,
    info: MessageInfo,
    player: String,
    banned: bool,
) -> Result<Response, ContractError> {
    if !is_admin(deps.as_ref(), &info.sender)? {
        return Err(ContractError::Unauthorized);
    }
    let addr = deps.api.addr_validate(&player)?;
    // T8：泛化为全局封禁（BANNED_PLAYERS，key 不变）；入口名由 msg 层
    // SanguoBanPlayer 分发至此，语义 = 全局禁止所有玩法
    BANNED_PLAYERS.save(deps.storage, &addr, &banned)?;
    Ok(Response::new()
        .add_attribute("action", "sanguo_ban_player")
        .add_attribute("player", player)
        .add_attribute("banned", banned.to_string()))
}

// ============================================================================
// 查询
// ============================================================================

pub fn query_gift_status(
    deps: Deps,
    env: &Env,
    player: String,
    with: Option<String>,
) -> StdResult<SanguoGiftStatusResponse> {
    let p = deps.api.addr_validate(&player)?;
    let now = env.block.time.seconds();
    let day = today(now);
    // 缺陷1 接线（T1）：查询回显同样取 params 生效值（与执行路径一致）
    let params = crate::sanguo::helpers::load_params(deps.storage)
        .map_err(|e| cosmwasm_std::StdError::generic_err(e.to_string()))?;

    let sent_today = GIFT_SENT_TODAY.may_load(deps.storage, (&p, day))?.unwrap_or(0);
    let recv_today = GIFT_RECV_TODAY.may_load(deps.storage, (&p, day))?.unwrap_or(0);
    let banned = BANNED_PLAYERS.may_load(deps.storage, &p)?.unwrap_or(false);
    let first_seen = ACCT_FIRST_SEEN.may_load(deps.storage, &p)?.unwrap_or(0);

    let (pair_first, pair_last, pair_ok) = match with {
        None => (0u64, 0u64, false),
        Some(w) => {
            let wa = deps.api.addr_validate(&w)?;
            match PAIR_INFO.may_load(deps.storage, (&p, &wa))? {
                Some(i) => {
                    let age = now.saturating_sub(i.first_at);
                    let idle = now.saturating_sub(i.last_at);
                    let ok = age >= params.gift_pair_age_secs()
                        && idle < params.gift_pair_idle_expire_secs();
                    (i.first_at, i.last_at, ok)
                }
                None => (0, 0, false),
            }
        }
    };

    Ok(SanguoGiftStatusResponse {
        banned,
        account_first_seen: first_seen,
        account_age_secs: now.saturating_sub(first_seen),
        sent_today,
        recv_today,
        daily_sent_limit: params.gift_daily_sent(),
        daily_recv_limit: params.gift_daily_recv(),
        pair_first_at: pair_first,
        pair_last_at: pair_last,
        pair_ok,
        pair_age_needed_secs: params.gift_pair_age_secs(),
        pair_idle_expire_secs: params.gift_pair_idle_expire_secs(),
        holding_secs: params.gift_holding_secs(),
        legend_holding_secs: params.gift_legend_holding_secs(),
        fragment_cap: params.gift_frag_cap(),
    })
}

pub fn query_gift_candidates(
    deps: Deps,
    env: &Env,
    player: String,
    limit: Option<u32>,
) -> StdResult<SanguoGiftCandidatesResponse> {
    let p = deps.api.addr_validate(&player)?;
    let now = env.block.time.seconds();
    let limit = limit.unwrap_or(50) as usize;
    // 缺陷1 接线（T1）：候选资格同样取 params 生效值
    let params = crate::sanguo::helpers::load_params(deps.storage)
        .map_err(|e| cosmwasm_std::StdError::generic_err(e.to_string()))?;

    let mut out = Vec::new();
    for item in PAIR_INFO
        .range(deps.storage, None, None, Order::Ascending)
        .filter_map(|r| r.ok())
        .filter(|((a, _), _)| a == &p)
        .take(limit)
    {
        let ((_, other), info) = item;
        let age = now.saturating_sub(info.first_at);
        let idle = now.saturating_sub(info.last_at);
        let eligible = age >= params.gift_pair_age_secs() && idle < params.gift_pair_idle_expire_secs();
        out.push(GiftCandidate {
            player: other.to_string(),
            first_at: info.first_at,
            last_at: info.last_at,
            age_secs: age,
            idle_secs: idle,
            eligible,
        });
    }
    Ok(SanguoGiftCandidatesResponse { candidates: out })
}
