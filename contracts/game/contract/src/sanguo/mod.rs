// ============================================================================
// 🀄 三国卡牌子模块：V2 分发层（裁定第 4 章入口清单，Execute 46 + Query 33）
//   - 🟢 玩家身份：取交易发送方 info.sender（无会话密钥 / 无感签名）
//   - 资产：付费走 PAXI/统一碎片；零 TKCC 红线已硬化为代码层面（C1~C6）
// ============================================================================
pub mod battle;
pub mod exec;
pub mod gift;
pub mod helpers;
pub mod query;
pub mod state;

use cosmwasm_std::{Binary, Deps, DepsMut, Env, MessageInfo, Response, StdResult, to_json_binary};
use crate::error::ContractError;
use crate::msg::{ExecuteMsg, QueryMsg};

/// 三国执行入口（由 contract.rs 的 execute 分发调用）
pub fn execute_sanguo(
    mut deps: DepsMut,
    env: Env,
    info: MessageInfo,
    msg: ExecuteMsg
) -> Result<Response, ContractError> {
    match msg {
        // ---- 管理员：配置 ----
        ExecuteMsg::SanguoSetConfig { tkcc_token, burn_address, tap_addresses } =>
            exec::set_config(deps, info, tkcc_token, burn_address, tap_addresses),
        ExecuteMsg::SanguoSetParams { params } =>
            exec::set_params(deps, info, params),
        ExecuteMsg::SanguoAddTemplates { templates } =>
            exec::add_templates(deps, info, templates),
        ExecuteMsg::SanguoSetCardsBatch { cards } =>
            exec::set_cards_batch(deps, info, cards),
        ExecuteMsg::SanguoSetGlobalDailyPayoutCap { cap } =>
            exec::set_global_daily_payout_cap(deps, info, cap),
        ExecuteMsg::SanguoSetPaymentToken { token } =>
            exec::set_payment_token(deps, info, token),
        ExecuteMsg::SanguoSetStages { stages } =>
            exec::set_stages(deps, info, stages),
        ExecuteMsg::SanguoSetComplianceConfig { operators, daily_draw_limit, daily_fragment_cap } =>
            exec::set_compliance_config(deps, info, operators, daily_draw_limit, daily_fragment_cap),
        ExecuteMsg::SanguoSetSkins { skins } =>
            exec::set_skins(deps, info, skins),
        ExecuteMsg::SanguoSetSeasonConfig { config } =>
            exec::set_season_config(deps, info, config),
        ExecuteMsg::SanguoSetShopItems { items } =>
            exec::set_shop_items(deps, info, items),
        ExecuteMsg::SanguoSetCardPrice { card_name, price } =>
            exec::set_card_price(deps, info, card_name, price),
        ExecuteMsg::SanguoSetProjectWallets { wallets } =>
            exec::set_project_wallets(deps, info, wallets),
        ExecuteMsg::SanguoPauseGame { game_id, paused } =>
            exec::pause_game(deps, info, game_id, paused),
        ExecuteMsg::SanguoBanPlayer { player, banned } =>
            gift::set_gift_ban(deps, info, player, banned),
        ExecuteMsg::SanguoGrantFragmentsBatch { grants } =>
            exec::grant_fragments_batch(deps, &env, info, grants),
        ExecuteMsg::SanguoAddMigrationTarget { old_contract } =>
            exec::add_migration_target(deps, info, old_contract),
        ExecuteMsg::SanguoMigrateFragments { start_after, limit } =>
            exec::migrate_fragments(&mut deps, info, start_after, limit),

        // ---- 玩家：抽卡 ----
        ExecuteMsg::SanguoDraw { tap_index, pack3, round_id } =>
            exec::draw_pack(&mut deps, &env, info, tap_index, pack3, round_id),

        // ---- 玩家：战斗（合并 11 → 2，裁定 4.3）----
        ExecuteMsg::SanguoBattlePvp {
            mode, action, match_id, opponent, public, royale_id, size,
            card_ids, round_id
        } => exec::battle_pvp(
            deps, &env, info, mode, action, match_id, opponent, public,
            royale_id, size, card_ids, round_id
        ),
        ExecuteMsg::SanguoBattleAi { mode, level, card_ids, round_id } =>
            exec::battle_ai(deps, &env, info, mode, level, card_ids, round_id),
        ExecuteMsg::SanguoClaimReward { battle_id, round_id } =>
            exec::claim_reward(&mut deps, &env, info, battle_id, round_id),

        // ---- 玩家：编队 / 养成 ----
        ExecuteMsg::SanguoSetBattleOrder { order, round_id } =>
            exec::set_battle_order(&mut deps, &env, info, order, round_id),
        ExecuteMsg::SanguoStarUp { card_id, use_fragments, round_id } =>
            exec::star_up(&mut deps, &env, info, card_id, use_fragments, round_id),
        ExecuteMsg::SanguoDecompose { card_id, round_id } =>
            exec::decompose_card(&mut deps, &env, info, card_id, round_id),
        ExecuteMsg::SanguoDecomposeBatch { card_ids, round_id } =>
            exec::decompose_batch(&mut deps, &env, info, card_ids, round_id),
        ExecuteMsg::SanguoUpgrade { card_id, round_id } =>
            exec::upgrade_card(&mut deps, &env, info, card_id, round_id),
        ExecuteMsg::SanguoCraft { rarity, round_id } =>
            exec::craft_card(&mut deps, &env, info, rarity, round_id),
        ExecuteMsg::SanguoLockCard { card_id, locked, round_id } =>
            exec::lock_card(&mut deps, &env, info, card_id, locked, round_id),
        ExecuteMsg::SanguoSetTeamPreset { preset_id, card_ids, name, round_id } =>
            exec::set_team_preset(&mut deps, &env, info, preset_id, card_ids, name, round_id),
        ExecuteMsg::SanguoUseTeamPreset { preset_id, round_id } =>
            exec::use_team_preset(&mut deps, &env, info, preset_id, round_id),

        // ---- 玩家：化妆品 ----
        ExecuteMsg::SanguoEquipCosmetic { slot, item_id, round_id } =>
            exec::equip_cosmetic(&mut deps, &env, info, slot, item_id, round_id),
        ExecuteMsg::SanguoUnequipCosmetic { slot, round_id } =>
            exec::unequip_cosmetic(&mut deps, &env, info, slot, round_id),

        // ---- 玩家：迁移 / 关卡 / 退款 ----
        ExecuteMsg::SanguoMigrateFromOld { old_contract, round_id } =>
            exec::migrate_from_old(&mut deps, &env, info, old_contract, round_id),
        ExecuteMsg::SanguoSubmitStageBatch { stage_id, batch_id, card_ids, round_id } =>
            exec::submit_stage_batch(&mut deps, &env, info, stage_id, batch_id, card_ids, round_id),
        ExecuteMsg::SanguoRefundProposalDeposit { round_id } =>
            exec::refund_proposal_deposit(&mut deps, &env, info, round_id),

        // ---- 玩家：商店（统一分发 + 委托改造 4）----
        ExecuteMsg::SanguoBuyShopItem { item_id, target, pay_with_fragment, round_id } =>
            exec::buy_shop_item(deps, &env, info, item_id, target, pay_with_fragment, round_id),
        ExecuteMsg::SanguoBuyCard { card_name, round_id } =>
            exec::buy_card(deps, &env, info, card_name, round_id),
        ExecuteMsg::SanguoBuySkin { skin_id, round_id } =>
            exec::buy_skin(deps, &env, info, skin_id, round_id),
        ExecuteMsg::SanguoBuyStamina { pack_count, round_id } =>
            exec::buy_stamina(deps, &env, info, pack_count, round_id),
        ExecuteMsg::SanguoBuySeasonPass { round_id } =>
            exec::buy_season_pass(deps, &env, info, round_id),

        // ---- 玩家：赛季 / 每日任务 ----
        ExecuteMsg::SanguoClaimSeasonTier { tier_index, round_id } =>
            exec::claim_season_tier(deps, &env, info, tier_index, round_id),
        ExecuteMsg::SanguoClaimDailyTask { task_id, round_id } =>
            exec::claim_daily_task(deps, &env, info, task_id, round_id),

        // ---- 玩家：赠送 ----
        ExecuteMsg::SanguoGiftCard { card_id, to, round_id } =>
            gift::gift_card(deps, &env, info, card_id, to, round_id),
        ExecuteMsg::SanguoGiftFragments { rarity, amount, to, round_id } =>
            gift::gift_fragments(deps, &env, info, rarity, amount, to, round_id),
        ExecuteMsg::SanguoGiftSkin { skin_id, to, round_id } =>
            gift::gift_skin(deps, &env, info, skin_id, to, round_id),

        // 其它消息不在此处理（通用域由 contract.rs 直接分发）
        _ => Err(ContractError::SanguoNotHandled)
    }
}

/// 三国查询入口（由 contract.rs 的 query 分发调用）
pub fn query_sanguo(deps: Deps, env: Env, msg: QueryMsg) -> StdResult<Binary> {
    match msg {
        QueryMsg::SanguoConfig {} => to_json_binary(&query::query_config(deps)?),
        QueryMsg::SanguoParams {} => to_json_binary(&query::query_params(deps)?),
        QueryMsg::SanguoPlayerCards { address } =>
            to_json_binary(&query::query_player_cards(deps, address)?),
        QueryMsg::SanguoCard { card_id } => to_json_binary(&query::query_card(deps, card_id)?),
        QueryMsg::SanguoAiStats { address } =>
            to_json_binary(&query::query_ai_stats(deps, &env, address)?),
        QueryMsg::SanguoBattleOrder { player } =>
            to_json_binary(&query::query_battle_order(deps, player)?),
        QueryMsg::SanguoPending { address } =>
            to_json_binary(&query::query_pending(deps, address)?),
        QueryMsg::SanguoFragments { address } =>
            to_json_binary(&query::query_fragments(deps, address)?),
        QueryMsg::SanguoTemplates { start_after, limit } =>
            to_json_binary(&query::query_templates(deps, start_after, limit)?),
        QueryMsg::SanguoRarityCount {} => to_json_binary(&query::query_rarity_count(deps)?),
        QueryMsg::SanguoPvp { match_id } =>
            to_json_binary(&query::query_pvp(deps, match_id)?),
        QueryMsg::SanguoPvpList { player, status, limit } =>
            to_json_binary(&query::query_pvp_list(deps, player, status, limit)?),
        QueryMsg::SanguoRoyale { royale_id } =>
            to_json_binary(&query::query_royale(deps, royale_id)?),
        QueryMsg::SanguoRoyaleList { status, limit } =>
            to_json_binary(&query::query_royale_list(deps, status, limit)?),
        QueryMsg::SanguoMigrationWhitelist {} =>
            to_json_binary(&exec::query_migration_whitelist(deps)?),
        QueryMsg::SanguoCompliance {} =>
            to_json_binary(&query::query_compliance(deps)?),
        QueryMsg::SanguoStages {} =>
            to_json_binary(&query::query_stages(deps)?),
        QueryMsg::SanguoShopItems {} =>
            to_json_binary(&query::query_shop_items(deps)?),
        QueryMsg::SanguoEquippedCosmetics { address } =>
            to_json_binary(&query::query_equipped_cosmetics(deps, address)?),
        QueryMsg::SanguoCollection { address } =>
            to_json_binary(&query::query_collection(deps, address)?),
        QueryMsg::SanguoCardPrices {} =>
            to_json_binary(&query::query_card_prices(deps)?),
        QueryMsg::SanguoFragmentPrices {} =>
            to_json_binary(&query::query_fragment_prices(deps)?),
        QueryMsg::SanguoProjectWallets {} =>
            to_json_binary(&query::query_project_wallets(deps)?),
        QueryMsg::SanguoStamina { address } =>
            to_json_binary(&query::query_stamina(deps, address)?),
        QueryMsg::SanguoSkins {} =>
            to_json_binary(&query::query_skins(deps)?),
        QueryMsg::SanguoPlayerSkins { address } =>
            to_json_binary(&query::query_player_skins(deps, address)?),
        QueryMsg::SanguoSeason {} =>
            to_json_binary(&query::query_season(deps, &env)?),
        QueryMsg::SanguoPlayerSeason { address } =>
            to_json_binary(&query::query_player_season(deps, address)?),
        QueryMsg::SanguoRank { address } =>
            to_json_binary(&query::query_rank(deps, address)?),
        QueryMsg::SanguoDailyTask { address } =>
            to_json_binary(&query::query_daily_task(deps, &env, address)?),
        QueryMsg::SanguoLegacyStatus {} =>
            to_json_binary(&query::query_legacy_status(deps, &env)?),
        QueryMsg::SanguoGiftStatus { player, with } =>
            to_json_binary(&gift::query_gift_status(deps, &env, player, with)?),
        QueryMsg::SanguoGiftCandidates { player, limit } =>
            to_json_binary(&gift::query_gift_candidates(deps, &env, player, limit)?),
        _ => Err(cosmwasm_std::StdError::generic_err("not a sanguo query"))
    }
}
