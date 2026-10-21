// ============================================================================
// 三国卡牌 V2 查询层（裁定 4.6：删 3 投票查询、增 3 商店/化妆品/图鉴 = 33 持平）
// SanguoParamsResponse / SanguoCompliance / SanguoFragmentPrices 的语义变更
// 见 msg.rs 文件头说明；字段集合与 msg.rs 三处对齐（msg / query / 前端）。
// ============================================================================
use cosmwasm_std::{Deps, Env, Order, StdResult, Uint128};
use crate::sanguo::state::*;
use crate::state::PRC20_WHITELIST;
use crate::msg::*;

fn pvp_status_str(s: &PvpStatus) -> String {
    match s {
        PvpStatus::Waiting => "waiting".into(),
        PvpStatus::Pending => "pending".into(),
        PvpStatus::Finished => "finished".into(),
        PvpStatus::Cancelled => "cancelled".into(),
    }
}

fn royale_status_str(s: &RoyaleStatus) -> String {
    match s {
        RoyaleStatus::Waiting => "waiting".into(),
        RoyaleStatus::Full => "full".into(),
        RoyaleStatus::Finished => "finished".into(),
        RoyaleStatus::Cancelled => "cancelled".into(),
    }
}

pub fn query_config(deps: Deps) -> StdResult<SanguoConfigResponse> {
    let cfg = SANGUO_CONFIG.may_load(deps.storage)?;
    Ok(SanguoConfigResponse {
        tkcc_token: cfg.as_ref().map(|c| c.tkcc_token.to_string()),
        burn_address: cfg.as_ref().map(|c| c.burn_address.to_string()),
        tap_addresses: cfg
            .map(|c| c.tap_addresses.iter().map(|a| a.to_string()).collect())
            .unwrap_or_default(),
    })
}

/// V2 参数响应：全部为访问器解析后的生效值（0/全零已回退默认）
pub fn query_params(deps: Deps) -> StdResult<SanguoParamsResponse> {
    let p = load_params_checked(deps.storage)?;
    let s = |x: u128| x.to_string();
    Ok(SanguoParamsResponse {
        // 抽卡
        daily_draw_limit: p.daily_draw_limit(),
        daily_draw_purchase_cap: p.daily_draw_purchase_cap(),
        pity_threshold: p.pity_threshold(),
        // 碎片
        daily_fragment_cap: p.daily_fragment_cap(),
        // 体力
        stamina_max: p.stamina_max(),
        stamina_regen_secs: p.stamina_regen_secs(),
        stamina_pack_amount: p.stamina_pack_amount(),
        stamina_pack_price: s(p.stamina_price().u128()),
        // 持有
        max_player_cards: p.max_player_cards(),
        max_cards: p.max_cards,
        rarity_max: p.rarity_max(),
        // 合成 / 分解 / 升星
        craft_cost: p.craft_cost(),
        fragment_from_duplicate: p.fragment_from_duplicate(),
        star_up_fragment_cost: p.star_up_fragment_cost(),
        upgrade_atk_boost: p.upgrade_atk_boost(),
        upgrade_def_boost: p.upgrade_def_boost(),
        upgrade_frag_atk: p.upgrade_frag_atk(),
        upgrade_frag_def: p.upgrade_frag_def(),
        upgrade_frag_max_level: p.upgrade_frag_max_level(),
        // AI 难度
        diff_boost_bp: p.diff_boost_bp(),
        ai_floor_bp: p.ai_floor_bp(),
        daily_ai_limit: p.ai_daily_limit(),
        // 天梯
        rank_boost_bp: p.rank_boost_bp(),
        rank_win_points: p.rank_win_points(),
        rank_lose_points: p.rank_lose_points(),
        rank_tier_up_points: p.rank_tier_up_points(),
        rank_stamina_cost: p.rank_stamina_cost(),
        rank_win_season_points: p.rank_win_season_points(),
        rank_rewards: p.rank_rewards(),
        // 每日任务
        daily_task_count: p.daily_task_count(),
        daily_task_required: p.daily_task_required(),
        daily_task_reward: p.daily_task_reward(),
        daily_task_season_points: p.daily_task_season_points(),
        // 赠送冷却
        gift_holding_secs: p.gift_holding_secs(),
        gift_legend_holding_secs: p.gift_legend_holding_secs(),
        gift_pair_age_secs: p.gift_pair_age_secs(),
        gift_acct_age_secs: p.gift_acct_age_secs(),
        gift_pair_idle_expire_secs: p.gift_pair_idle_expire_secs(),
        gift_pair_refresh_interval: p.gift_pair_refresh_interval(),
        gift_daily_sent: p.gift_daily_sent(),
        gift_daily_recv: p.gift_daily_recv(),
        gift_frag_cap: p.gift_frag_cap(),
        // 战斗奖励
        ai_frag_reward: p.ai_frag_reward,
        pvp_frag_reward: p.pvp_frag_reward,
        royale_frag_reward: p.royale_frag_reward,
        // 指定卡碎片价 / 编队
        card_frag_price: p.card_frag_price(),
        team_cost_cap: p.team_cost_cap(),
        // 战斗系数（14 项）
        faction_counter_bp: p.faction_counter_bp(),
        faction_countered_bp: p.faction_countered_bp(),
        faction_neutral_bp: p.faction_neutral_bp(),
        troop_counter_bp: p.troop_counter_bp(),
        troop_countered_bp: p.troop_countered_bp(),
        troop_neutral_bp: p.troop_neutral_bp(),
        rarity_legend_bp: p.rarity_legend_bp(),
        rarity_epic_bp: p.rarity_epic_bp(),
        rarity_rare_bp: p.rarity_rare_bp(),
        rarity_common_bp: p.rarity_common_bp(),
        power_flat_cap_bp: p.power_flat_cap_bp(),
        eff_lower_bound_bp: p.eff_lower_bound_bp(),
        skill_strength_bp: p.skill_strength_bp(),
        bond_strength_bp: p.bond_strength_bp(),
        // 支付代币（C6：恒空串 = 原生 PAXI）
        pay_token: crate::state::PAYMENT_TOKEN
            .may_load(deps.storage)?
            .filter(|a| a != &crate::state::empty_addr())
            .map(|a| a.to_string())
            .unwrap_or_default(),
    })
}

/// 读参数（带默认回落）；params 访问器在查询层复用
fn load_params_checked(
    storage: &dyn cosmwasm_std::Storage,
) -> StdResult<SanguoParams> {
    Ok(SANGUO_PARAMS
        .may_load(storage)?
        .unwrap_or_else(SanguoParams::default_sanguo))
}

pub fn query_player_cards(deps: Deps, address: String) -> StdResult<SanguoCardsResponse> {
    let addr = deps.api.addr_validate(&address)?;
    let ids = PLAYER_CARDS.may_load(deps.storage, &addr)?.unwrap_or_default();
    let mut cards = Vec::new();
    for id in ids {
        if let Some(c) = CARDS.may_load(deps.storage, &id)? {
            cards.push(c);
        }
    }
    Ok(SanguoCardsResponse { cards })
}

pub fn query_card(deps: Deps, card_id: String) -> StdResult<CardInfo> {
    CARDS.load(deps.storage, &card_id)
}

pub fn query_ai_stats(deps: Deps, env: &Env, address: String) -> StdResult<SanguoAiStatsResponse> {
    let addr = deps.api.addr_validate(&address)?;
    let stats = AI_BATTLE_STATS
        .may_load(deps.storage, &addr)?
        .unwrap_or_default();
    let win_rate = if stats.total == 0 {
        "0".to_string()
    } else {
        format!("{:.2}", (stats.wins as f64 / stats.total as f64) * 100.0)
    };
    let today = env.block.time.seconds() / SECS_PER_DAY * SECS_PER_DAY;
    let last_day = AI_BATTLE_DATE.may_load(deps.storage, &addr)?.unwrap_or(0);
    let today_count = if last_day == today {
        AI_BATTLE_COUNT.may_load(deps.storage, &addr)?.unwrap_or(0)
    } else {
        0
    };
    let limit = load_params_checked(deps.storage)?.ai_daily_limit();
    Ok(SanguoAiStatsResponse {
        total: stats.total,
        wins: stats.wins,
        win_rate,
        today_count,
        daily_limit: limit,
        pvp_total: stats.pvp_total,
        pvp_wins: stats.pvp_wins,
        royale_total: stats.royale_total,
        royale_wins: stats.royale_wins,
        stage_total: stats.stage_total,
        stage_wins: stats.stage_wins,
        all_total: stats.all_total(),
        all_wins: stats.all_wins(),
    })
}

pub fn query_battle_order(deps: Deps, player: String) -> StdResult<SanguoBattleOrderResponse> {
    let addr = deps.api.addr_validate(&player)?;
    let order = PLAYER_BATTLE_ORDER
        .may_load(deps.storage, &addr)?
        .unwrap_or_default();
    let mut presets = Vec::new();
    for pid in 0..MAX_TEAM_PRESETS {
        if let Some(p) = TEAM_PRESETS.may_load(deps.storage, (&addr, pid))? {
            presets.push(SanguoTeamPresetResponse {
                preset_id: pid,
                name: p.name,
                card_ids: p.card_ids,
            });
        }
    }
    Ok(SanguoBattleOrderResponse { order, presets })
}

pub fn query_pending(deps: Deps, address: String) -> StdResult<SanguoPendingResponse> {
    let addr = deps.api.addr_validate(&address)?;
    // V2：奖励即时入账统一碎片，total_rewards = 当前统一碎片余额；
    // battle_ids 保留兼容（V2 恒空）
    let total = crate::sanguo::exec::frag_balance(deps.storage, &addr);
    Ok(SanguoPendingResponse {
        total_rewards: total.to_string(),
        battle_ids: vec![],
    })
}

/// 统一碎片余额查询（V2：返回折算展示值 + 原始统一余额由 total_rewards 承载）
pub fn query_fragments(deps: Deps, address: String) -> StdResult<SanguoFragmentsResponse> {
    let addr = deps.api.addr_validate(&address)?;
    // V2：统一碎片币（u128）；四桶展示字段保留兼容——统一余额全部记入
    // common 桶展示（前端应迁移使用 SanguoPending.total_rewards 原始值）
    let unified = crate::sanguo::exec::frag_balance(deps.storage, &addr);
    let legacy = FRAGMENTS.may_load(deps.storage, &addr)?.unwrap_or_default();
    Ok(SanguoFragmentsResponse {
        common: legacy.common.saturating_add(u64::try_from(unified).unwrap_or(u64::MAX)),
        rare: legacy.rare,
        epic: legacy.epic,
        legend: legacy.legend,
    })
}

pub fn query_templates(
    deps: Deps,
    start_after: Option<u32>,
    limit: Option<u32>,
) -> StdResult<Vec<CardTemplate>> {
    Ok(list_templates(deps.storage, start_after, limit))
}

pub fn query_rarity_count(deps: Deps) -> StdResult<SanguoRarityCountResponse> {
    let (c, r, e, l) = count_rarities(deps.storage);
    Ok(SanguoRarityCountResponse {
        common: c,
        rare: r,
        epic: e,
        legend: l,
        total: c + r + e + l,
    })
}

fn into_pvp_resp(m: PvpMatch) -> SanguoPvpResponse {
    // 9v9 防偷看：Waiting 期间对所有人屏蔽 challenger_order
    let (ch_order, op_order) = match m.status {
        PvpStatus::Waiting => (vec![], vec![]),
        PvpStatus::Pending => (m.challenger_order, vec![]),
        _ => (m.challenger_order, m.opponent_order),
    };
    SanguoPvpResponse {
        match_id: m.match_id,
        challenger: m.challenger.to_string(),
        opponent: m.opponent.to_string(),
        is_public: m.is_public,
        challenger_order: ch_order,
        opponent_order: op_order,
        winner: m.winner.map(|w| w.to_string()),
        status: pvp_status_str(&m.status),
        created_at: m.created_at,
        finished_at: m.finished_at,
        reward_claimed: m.reward_claimed,
        group_scores_a: m.group_scores_a,
        group_scores_b: m.group_scores_b,
    }
}

pub fn query_pvp(deps: Deps, match_id: String) -> StdResult<SanguoPvpResponse> {
    let m = PVP_MATCHES
        .load(deps.storage, &match_id)
        .map_err(|_| cosmwasm_std::StdError::not_found("sanguo_pvp"))?;
    Ok(into_pvp_resp(m))
}

pub fn query_pvp_list(
    deps: Deps,
    player: Option<String>,
    status: Option<String>,
    limit: Option<u32>,
) -> StdResult<SanguoPvpListResponse> {
    let limit = limit.unwrap_or(50) as usize;
    let player_addr = player.map(|p| deps.api.addr_validate(&p)).transpose()?;
    let matches: Vec<SanguoPvpResponse> = PVP_MATCHES
        .range(deps.storage, None, None, Order::Descending)
        .filter_map(|r| r.ok().map(|(_, m)| m))
        .filter(|m| {
            if let Some(p) = &player_addr {
                if &m.challenger != p && &m.opponent != p {
                    return false;
                }
            }
            if let Some(s) = &status {
                if &pvp_status_str(&m.status) != s {
                    return false;
                }
            }
            true
        })
        .take(limit)
        .map(into_pvp_resp)
        .collect();
    Ok(SanguoPvpListResponse { matches })
}

fn into_royale_resp(r: RoyaleMatch) -> SanguoRoyaleResponse {
    // 🟢 第二轮审计修复（P1，防偷看）：混战编队可见性与 PVP into_pvp_resp 对齐——
    //    Waiting/Full 期间屏蔽 player_orders，避免后入场玩家读到已入场玩家编队后
    //    针对性反制（胜者领碎片）。结算/取消后揭示（前端 showRoyaleBattle 仅在
    //    settle 之后调用，不受影响）。
    //    ⚠️ 若产品有意公开混战编队（"明牌混战"），请回退本 if。
    let reveal_orders = matches!(
        r.status,
        RoyaleStatus::Finished | RoyaleStatus::Cancelled
    );
    SanguoRoyaleResponse {
        royale_id: r.royale_id,
        players: r.players.iter().map(|a| a.to_string()).collect(),
        player_orders: if reveal_orders { r.player_orders } else { vec![] },
        winner: r.winner.map(|w| w.to_string()),
        status: royale_status_str(&r.status),
        created_at: r.created_at,
        finished_at: r.finished_at,
        size: r.size,
        reward_claimed: r.reward_claimed,
    }
}

pub fn query_royale(deps: Deps, royale_id: String) -> StdResult<SanguoRoyaleResponse> {
    let r = ROYALE_MATCHES
        .load(deps.storage, &royale_id)
        .map_err(|_| cosmwasm_std::StdError::not_found("sanguo_royale"))?;
    Ok(into_royale_resp(r))
}

pub fn query_royale_list(
    deps: Deps,
    status: Option<String>,
    limit: Option<u32>,
) -> StdResult<SanguoRoyaleListResponse> {
    let limit = limit.unwrap_or(50) as usize;
    let royales: Vec<SanguoRoyaleResponse> = ROYALE_MATCHES
        .range(deps.storage, None, None, Order::Descending)
        .filter_map(|r| r.ok().map(|(_, rm)| rm))
        .filter(|r| {
            if let Some(s) = &status {
                if &royale_status_str(&r.status) != s {
                    return false;
                }
            }
            true
        })
        .take(limit)
        .map(into_royale_resp)
        .collect();
    Ok(SanguoRoyaleListResponse { royales })
}

// ===== 合规模式查询（C1：compliance_mode 已删） =====

pub fn query_compliance(deps: Deps) -> StdResult<SanguoComplianceResponse> {
    let p = load_params_checked(deps.storage)?;
    Ok(SanguoComplianceResponse {
        operators: OPERATOR_LIST.may_load(deps.storage)?.unwrap_or_default(),
        daily_draw_limit: p.daily_draw_limit(),
        daily_fragment_cap: p.daily_fragment_cap(),
    })
}

pub fn query_stages(deps: Deps) -> StdResult<SanguoStagesResponse> {
    Ok(SanguoStagesResponse {
        stages: STAGES.may_load(deps.storage)?.unwrap_or_default(),
    })
}

// ===== 商店 / 化妆品 / 图鉴（增 3） =====

pub fn query_shop_items(deps: Deps) -> StdResult<SanguoShopItemsResponse> {
    let items: Vec<ShopItem> = SHOP_ITEMS
        .range(deps.storage, None, None, Order::Ascending)
        .filter_map(|r| r.ok().map(|(_, it)| it))
        .collect();
    Ok(SanguoShopItemsResponse { items })
}

pub fn query_equipped_cosmetics(
    deps: Deps,
    address: String,
) -> StdResult<SanguoEquippedCosmeticsResponse> {
    let addr = deps.api.addr_validate(&address)?;
    let mut equipped = Vec::new();
    for slot in COSMETIC_SLOTS.iter() {
        if let Some(item_id) = EQUIPPED_COSMETICS.may_load(deps.storage, (&addr, slot))? {
            equipped.push((slot.to_string(), item_id));
        }
    }
    Ok(SanguoEquippedCosmeticsResponse { equipped })
}

pub fn query_collection(deps: Deps, address: String) -> StdResult<SanguoCollectionResponse> {
    let addr = deps.api.addr_validate(&address)?;
    let collected: Vec<String> = COLLECTION
        .prefix(&addr)
        .range(deps.storage, None, None, Order::Ascending)
        .filter_map(|r| r.ok().map(|(name, _)| name))
        .collect();
    let total_templates = CARD_TEMPLATE_COUNT
        .may_load(deps.storage)?
        .unwrap_or(0);
    Ok(SanguoCollectionResponse { collected, total_templates })
}

// ===== 付费系统查询 =====

pub fn query_card_prices(deps: Deps) -> StdResult<SanguoCardPricesResponse> {
    let prices: Vec<(String, String)> = crate::state::CARD_PRICES
        .range(deps.storage, None, None, Order::Ascending)
        .filter_map(|r| r.ok())
        .map(|(name, price)| (name.to_string(), price.to_string()))
        .collect();
    Ok(SanguoCardPricesResponse { prices })
}

/// 语义变更（T15）：返回指定卡碎片价 card_frag_price[4]
pub fn query_fragment_prices(deps: Deps) -> StdResult<SanguoFragmentPricesResponse> {
    let p = load_params_checked(deps.storage)?;
    Ok(SanguoFragmentPricesResponse {
        card_frag_price: p.card_frag_price(),
    })
}

pub fn query_project_wallets(deps: Deps) -> StdResult<SanguoProjectWalletsResponse> {
    let wallets = crate::state::PROJECT_WALLETS
        .may_load(deps.storage)?
        .unwrap_or_default()
        .iter()
        .map(|a| a.to_string())
        .collect();
    let next_index = crate::state::WALLET_INDEX.may_load(deps.storage)?.unwrap_or(0);
    Ok(SanguoProjectWalletsResponse { wallets, next_index })
}

// ===== 新玩法查询 =====

pub fn query_stamina(deps: Deps, address: String) -> StdResult<SanguoStaminaResponse> {
    let addr = deps.api.addr_validate(&address)?;
    let p = load_params_checked(deps.storage)?;
    let info = STAMINA
        .may_load(deps.storage, &addr)?
        .unwrap_or_default();
    // 查询为静态展示（查询无区块时间可注入）：regen 由前端按
    // (now - last_update) / regen_per_secs 自行折算
    Ok(SanguoStaminaResponse {
        current: info.current,
        max: p.stamina_max(),
        last_update: info.last_update,
        regen_per_secs: p.stamina_regen_secs(),
    })
}

pub fn query_skins(deps: Deps) -> StdResult<SanguoSkinsResponse> {
    let skins: Vec<SkinInfo> = SKINS
        .range(deps.storage, None, None, Order::Ascending)
        .filter_map(|r| r.ok().map(|(_, s)| s))
        .take(200)
        .collect();
    Ok(SanguoSkinsResponse { skins })
}

pub fn query_player_skins(deps: Deps, address: String) -> StdResult<SanguoPlayerSkinsResponse> {
    let addr = deps.api.addr_validate(&address)?;
    Ok(SanguoPlayerSkinsResponse {
        skins: PLAYER_SKINS.may_load(deps.storage, &addr)?.unwrap_or_default(),
    })
}

pub fn query_season(deps: Deps, env: &Env) -> StdResult<SanguoSeasonResponse> {
    let cfg = SEASON_CONFIG.may_load(deps.storage)?.unwrap_or_default();
    let active = season_active(&cfg, env.block.time.seconds());
    Ok(SanguoSeasonResponse {
        season_id: cfg.season_id,
        active,
        start_time: cfg.start_time,
        end_time: cfg.end_time,
        pass_price_paxi: cfg.pass_price_paxi.to_string(),
        tiers: cfg
            .tiers
            .iter()
            .map(|t| SanguoSeasonTierResponse {
                points_req: t.points_req,
                reward: SanguoFragmentsResponse {
                    common: t.reward.common,
                    rare: t.reward.rare,
                    epic: t.reward.epic,
                    legend: t.reward.legend,
                },
            })
            .collect(),
    })
}

pub fn query_player_season(deps: Deps, address: String) -> StdResult<SanguoPlayerSeasonResponse> {
    let addr = deps.api.addr_validate(&address)?;
    let ps = PLAYER_SEASON.may_load(deps.storage, &addr)?.unwrap_or_default();
    Ok(SanguoPlayerSeasonResponse {
        pass_owned: ps.pass_owned,
        points: ps.points,
        claimed: ps.claimed,
    })
}

pub fn query_rank(deps: Deps, address: String) -> StdResult<SanguoRankResponse> {
    let addr = deps.api.addr_validate(&address)?;
    let r = RANK_INFO.may_load(deps.storage, &addr)?.unwrap_or_default();
    Ok(SanguoRankResponse {
        tier: r.tier,
        points: r.points,
    })
}

pub fn query_daily_task(deps: Deps, env: &Env, address: String) -> StdResult<SanguoDailyTaskResponse> {
    let addr = deps.api.addr_validate(&address)?;
    let p = load_params_checked(deps.storage)?;
    let count = p.daily_task_count() as usize;
    let day = env.block.time.seconds() / SECS_PER_DAY;
    let st = DAILY_TASK
        .may_load(deps.storage, (&addr, day))?
        .unwrap_or_else(|| DailyTaskState {
            progress: vec![0u64; count],
            claimed: vec![false; count],
        });
    let required = p.daily_task_required();
    let rewards = p.daily_task_reward();
    let mut tasks = Vec::new();
    for i in 0..count {
        let r = rewards.get(i).copied().unwrap_or_default();
        tasks.push(SanguoDailyTaskItem {
            task_id: i as u64,
            progress: *st.progress.get(i).unwrap_or(&0),
            required: *required.get(i).unwrap_or(&1),
            claimed: *st.claimed.get(i).unwrap_or(&false),
            reward: SanguoFragmentsResponse {
                common: r.common,
                rare: r.rare,
                epic: r.epic,
                legend: r.legend,
            },
        });
    }
    Ok(SanguoDailyTaskResponse { day, tasks })
}

// ========================================================================
// 升级前存量核对（方案 Z 前置）：旧对局计数 + 合约余额 + 待退押金 + 待审碎片
// ========================================================================
pub fn query_legacy_status(deps: Deps, env: &Env) -> StdResult<SanguoLegacyStatusResponse> {
    let mut pending_pvp = 0u64;
    let mut finished_unclaimed_pvp = 0u64;
    for item in PVP_MATCHES.range(deps.storage, None, None, Order::Ascending) {
        let (_, m) = item?;
        match m.status {
            PvpStatus::Waiting | PvpStatus::Pending => pending_pvp += 1,
            PvpStatus::Finished => {
                if !m.reward_claimed {
                    finished_unclaimed_pvp += 1;
                }
            }
            PvpStatus::Cancelled => {}
        }
    }

    let mut pending_royale = 0u64;
    let mut finished_unclaimed_royale = 0u64;
    for item in ROYALE_MATCHES.range(deps.storage, None, None, Order::Ascending) {
        let (_, m) = item?;
        match m.status {
            RoyaleStatus::Waiting | RoyaleStatus::Full => pending_royale += 1,
            RoyaleStatus::Finished => {
                if !m.reward_claimed {
                    finished_unclaimed_royale += 1;
                }
            }
            RoyaleStatus::Cancelled => {}
        }
    }

    // 投票已全禁（T24）：恒 0，字段保留兼容
    let unexecuted_proposals = 0u64;

    let contract_paxi = deps
        .querier
        .query_balance(env.contract.address.as_str(), "upaxi")?
        .amount;

    let mut prc20_balances = Vec::new();
    for key in PRC20_WHITELIST.keys(deps.storage, None, None, Order::Ascending) {
        let token = key?;
        let query: cosmwasm_std::QueryRequest<cosmwasm_std::Empty> =
            cosmwasm_std::QueryRequest::Wasm(cosmwasm_std::WasmQuery::Smart {
                contract_addr: token.to_string(),
                msg: cosmwasm_std::to_json_binary(&serde_json::json!({
                    "balance": { "address": env.contract.address.to_string() }
                }))?,
            });
        let bal = deps
            .querier
            .query::<LegacyPrc20Balance>(&query)
            .map(|r| r.balance)
            .unwrap_or_default();
        prc20_balances.push((token.to_string(), bal.to_string()));
    }

    // M3：待退押金
    let pending_refunds: Vec<(String, String)> = PENDING_REFUNDS
        .range(deps.storage, None, None, Order::Ascending)
        .filter_map(|r| r.ok())
        .map(|(addr, amt)| (addr.to_string(), amt.to_string()))
        .collect();

    // M6：待审超额碎片（player, 折算值, 游标）
    let pending_fragment_review: Vec<(String, String, u64)> = PENDING_FRAGMENT_REVIEW
        .range(deps.storage, None, None, Order::Ascending)
        .filter_map(|r| r.ok())
        .map(|(addr, (val, cur))| (addr.to_string(), val.to_string(), cur))
        .collect();

    Ok(SanguoLegacyStatusResponse {
        pending_pvp,
        pending_royale,
        finished_unclaimed_pvp,
        finished_unclaimed_royale,
        unexecuted_proposals,
        contract_paxi,
        prc20_balances,
        pending_refunds,
        pending_fragment_review,
    })
}

/// PRC-20 balance 查询返回结构（仅 query_legacy_status 本地反序列化用）
#[derive(serde::Deserialize)]
struct LegacyPrc20Balance {
    balance: Uint128,
}
