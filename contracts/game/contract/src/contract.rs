use cosmwasm_std::{
    entry_point, to_json_binary, BankMsg, Binary, Coin, CosmosMsg, Deps, DepsMut, Env,
    MessageInfo, Response, StdResult, Uint128, Addr, WasmMsg,
};
use serde_json::json;

use crate::error::ContractError;
use crate::msg::{
    AdminsResponse, AllowanceResponse, BalanceResponse, BurnConfigResponse,
    ExecuteMsg, InstantiateMsg, MigrateMsg, Prc20ListResponse, QueryMsg, TreasuryConfigResponse,
};
use crate::admin::config::{
    add_admin, add_prc20, admin_sweep, burn_paxi, effective_burn_rate, effective_treasury_share,
    fund_bankroll, is_admin, list_admins, list_prc20, remove_admin, remove_prc20,
    set_burn_rate, set_game_config, set_paused, set_round_timeout, set_swap_module,
    set_treasury_list, set_treasury_share,
    set_game_limit as set_game_limit_fn,
    remove_game_limit as remove_game_limit_fn,
    update_game_params,
};
use crate::assets::allowance::query_allowance;
use crate::assets::balance::query_balance;
use crate::assets::deposit::deposit;
use crate::assets::swap::swap;
use crate::assets::withdraw::withdraw;
use crate::games::{
    play as game_play,
    set_game_engine_config as game_set_engine,
    remove_game_engine_config as game_remove_engine,
    add_game as game_add_game,
    force_expire_round as game_force_expire,
    query_game_engine_config as game_query_engine,
};
use crate::state::{
    ADMINS, BALANCES, BURN_RATE, CONTRACT_NAME, CONTRACT_VERSION, GAME_CONFIGS, GAME_LIMITS,
    GAME_PARAMS,
    GameParams, HOUSE_BANKROLL, PAUSED, PRC20_WHITELIST, SWAP_MODULE,
    ROUND_TIMEOUT, DEFAULT_ROUND_TIMEOUT_SECS,
    TREASURY_LIST, TREASURY_SHARE, TOTAL_BURNED, TOTAL_TREASURY, USER_TOTALS, empty_addr,
};

#[entry_point]
pub fn instantiate(
    deps: DepsMut,
    _env: Env,
    info: MessageInfo,
    msg: InstantiateMsg,
) -> Result<Response, ContractError> {
    cw2::set_contract_version(deps.storage, CONTRACT_NAME, CONTRACT_VERSION)?;

    // 🟢 审计修复：把部署者无条件加入管理员。
    // 原实现只信 msg.admins，一旦部署时地址填错/填了不存在的地址，
    // 合约就永久失去管理权（无法改配置、无法暂停、无法处理异常）——这是不可逆事故。
    //
    // 🟢 第二轮改进：正因为部署者已经自动入管理员，**不再要求 msg.admins 非空**。
    // 原来「admins 为空就拒绝 instantiate」的检查是多余且有害的：
    // 单人部署（admins = []）本来就该合法，部署者自己就是管理员。
    ADMINS.save(deps.storage, &info.sender, &true)?;

    for a in &msg.admins {
        let addr = deps.api.addr_validate(a)?;
        ADMINS.save(deps.storage, &addr, &true)?;
    }

    PAUSED.save(deps.storage, &false)?;
    BURN_RATE.save(deps.storage, &msg.initial_burn_rate.unwrap_or(0))?;

    GAME_PARAMS.save(deps.storage, &GameParams::default())?;

    // 🟢 第二轮新增：可配置项落初始值（后续由 SetRoundTimeout 调整）
    ROUND_TIMEOUT.save(deps.storage, &DEFAULT_ROUND_TIMEOUT_SECS)?;

    // 🔴 C1 硬化（V2 0.2.0）：合规总闸存储键已删除（M2），合规是唯一路径，
    //    instantiate 不再写 compliance_mode 键。

    let treasury_list = msg.treasury_list.clone().unwrap_or_default();
    for t in &treasury_list {
        if !t.is_empty() {
            deps.api.addr_validate(t)?;
        }
    }
    TREASURY_LIST.save(deps.storage, &treasury_list)?;
    let treasury_share = msg.treasury_share.unwrap_or(0);
    if treasury_share > 10000 {
        return Err(ContractError::InvalidAmount);
    }
    let burn_rate = msg.initial_burn_rate.unwrap_or(0);
    if burn_rate.checked_add(treasury_share).unwrap_or(10001) > 10000 {
        return Err(ContractError::SplitExceedsHundredPercent);
    }
    TREASURY_SHARE.save(deps.storage, &treasury_share)?;

    let swap_addr_str = msg
        .swap_module
        .clone()
        .unwrap_or_else(|| crate::state::DEFAULT_SWAP_MODULE.to_string());
    deps.api.addr_validate(&swap_addr_str)?;
    SWAP_MODULE.save(deps.storage, &swap_addr_str)?;

    for a in msg.prc20_whitelist {
        let addr = deps.api.addr_validate(&a)?;
        PRC20_WHITELIST.save(deps.storage, &addr, &true)?;
    }

    Ok(Response::new()
        .add_attribute("action", "instantiate")
        .add_attribute("admins_count", msg.admins.len().to_string())
        .add_attribute("contract_version", CONTRACT_VERSION))
}

#[entry_point]
pub fn execute(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
    msg: ExecuteMsg,
) -> Result<Response, ContractError> {
    let paused = PAUSED.load(deps.storage).unwrap_or(false);
    if paused && !is_admin(deps.as_ref(), &info.sender)? {
        // 🟢 审计修复（高危）：原实现在暂停时把 Withdraw 一起拦掉。
        // 只要管理员 SetPaused(true)（无论是应急还是误操作），
        // 所有用户的余额就被无限期锁死在合约里，无法取出。
        // 暂停的语义应当是「停止下注和充值」，而不是「冻结用户资产」。
        //
        // 🟢 第二轮新增：玩家操作同时豁免 Swap（用户可能想换资产离场）、
        // ForceExpireRound / FundBankroll（管理员应急操作）。
        let allowed_when_paused = matches!(
            &msg,
            ExecuteMsg::Withdraw { .. }
                | ExecuteMsg::Swap { .. }
                | ExecuteMsg::ForceExpireRound { .. }
                | ExecuteMsg::FundBankroll { .. }
                | ExecuteMsg::AdminSweep { .. }
        );
        if !allowed_when_paused {
            return Err(ContractError::Paused);
        }
    }

    match msg {
        // ========== 管理员 ==========
        ExecuteMsg::AddAdmin { admin } => add_admin(deps, info, admin),
        ExecuteMsg::RemoveAdmin { admin } => remove_admin(deps, info, admin),
        ExecuteMsg::AddPrc20 { contract } => add_prc20(deps, info, contract),
        ExecuteMsg::RemovePrc20 { contract } => remove_prc20(deps, info, contract),
        ExecuteMsg::SetPaused { paused } => set_paused(deps, info, paused),
        ExecuteMsg::UpdateGameParams { params } => update_game_params(deps, info, params),
        ExecuteMsg::SetBurnRate { rate } => set_burn_rate(deps, info, rate),
        ExecuteMsg::SetGameConfig { game_id, config } => set_game_config(deps, info, game_id, config),
        ExecuteMsg::BurnPaxi { amount } => burn_paxi(deps, info, amount),
        ExecuteMsg::SetSwapModule { contract } => set_swap_module(deps, info, contract),
        ExecuteMsg::SetTreasuryList { treasuries } => set_treasury_list(deps, info, treasuries),
        ExecuteMsg::SetTreasuryShare { share } => set_treasury_share(deps, info, share),

        // 🟢 新增：游戏限额配置（加游戏 = 调这个接口）
        ExecuteMsg::SetGameLimit { game_id, limit } => set_game_limit_fn(deps, info, game_id, limit),
        ExecuteMsg::RemoveGameLimit { game_id } => remove_game_limit_fn(deps, info, game_id),

        // 🟢 第二轮新增：Deduct 超时时长
        ExecuteMsg::SetRoundTimeout { secs } => set_round_timeout(deps, info, secs),

        // 🟢 新增：游戏引擎配置
        ExecuteMsg::SetGameEngineConfig { game_id, config } => game_set_engine(deps, info, game_id, config),
        ExecuteMsg::RemoveGameEngineConfig { game_id } => game_remove_engine(deps, info, game_id),

        // 🟢 审计新增：Deduct 超时强制结算 / 庄家准备金注资
        ExecuteMsg::ForceExpireRound { player, game_id, round_id } =>
            game_force_expire(deps, env, info, player, game_id, round_id),
        ExecuteMsg::FundBankroll { token, amount } => fund_bankroll(deps, env, info, token, amount),

        // 🟢 升级前清扫（2026-10-10）：仅管理员，把合约残留资金转到运营钱包。
        //    管理员操作不受暂停限制（与 FundBankroll / ForceExpireRound 同级豁免）。
        ExecuteMsg::AdminSweep { token, recipient, amount } =>
            admin_sweep(deps, env, info, token, recipient, amount),

        // 🟢 新增：一键加游戏（限额 + 引擎一起设）
        ExecuteMsg::AddGame { game_id, limit, engine_config } =>
            game_add_game(deps, info, game_id, limit, engine_config),

        // ========== 资产 ==========
        ExecuteMsg::Deposit { token, amount } => deposit(deps, env, info, token, amount),
        ExecuteMsg::Withdraw { token, amount } => withdraw(deps, env, info, token, amount),

        // ========== 🔴 安全修复（致命）：Deduct / Settle 两条外部路由已下线 ==========
        //
        // 【漏洞】games/mod.rs::settle 的 won / payout 完全信任调用方传入，只校验
        //   payout ≤ bet × max_payout_multiplier。会话私钥存在玩家浏览器里，
        //   玩家可以自己签一条 won=true / payout=上限标 的 Settle，把庄家准备金
        //   按「最大赔率」反复提走。流程上没有任何东西比玩家先知道真实胜负，
        //   所以这两段式接口在前端场景是一个必然被利用的后门。
        //
        // 【处置】从 ExecuteMsg 分发里摘除这两条路由（ExecuteMsg::Deduct /
        //   Settle 变体已删除，外部无法再构造）。所有游戏改走 Play：
        //   随机数与胜负判定都在合约内由 build_seed 生成，玩家无从预判或篡改。
        //
        // 【保留】games/mod.rs 里的 deduct / settle 函数体不删，标 #[allow(dead_code)]
        //   存档；将来若接第三方「服务端裁判」类玩法（可引入服务端地址白名单）再启用。
        // 注意：合约 EVENT：外部看到的行为变化是 {deduct:{}} / {settle:{}} 直接 unknown variant。
        // ==========
        //        ExecuteMsg::Deduct { .. } => unreachable!(),   // 已下线
        //        ExecuteMsg::Settle { .. } => unreachable!(),   // 已下线

        // 🟢 新增：引擎内 Play（一步式，合约生成随机数 + 判定 + 结算）
        //    🟢 玩家身份 = 交易发送方 info.sender（无会话机制）
        ExecuteMsg::Play {
            game_id, round_id, token, bet,
            guess_input, roulette_input, dice_input, card_input, score_input,
        } => {
            game_play(
                deps, env, info,
                game_id, round_id, token, bet,
                guess_input, roulette_input, dice_input, card_input, score_input,
            )
        }

        // ========== Swap ==========
        ExecuteMsg::Swap { prc20, offer_denom, offer_amount, min_receive } => {
            swap(deps, env, info, prc20, offer_denom, offer_amount, min_receive)
        }

        // ====================================================================
        // 🀄 三国卡牌：所有 Sanguo* 变体统一走子模块入口（玩家身份 = info.sender）
        // ====================================================================
        other => crate::sanguo::execute_sanguo(deps, env, info, other),
    }
}

#[entry_point]
pub fn query(deps: Deps, env: Env, msg: QueryMsg) -> StdResult<Binary> {
    match msg {
        QueryMsg::Balance { address, token } => {
            to_json_binary(&query_balance(deps, address, token)?)
        }
        QueryMsg::BurnConfig {} => {
            let br = BURN_RATE.load(deps.storage).unwrap_or(0);
            let mut tb = Uint128::zero();
            for item in TOTAL_BURNED.range(deps.storage, None, None, cosmwasm_std::Order::Ascending) {
                let (_, v) = item?;
                tb = tb.checked_add(v)?;
            }
            to_json_binary(&BurnConfigResponse { burn_rate: br, total_burned: tb })
        }
        QueryMsg::TreasuryConfig {} => {
            let tl = TREASURY_LIST.load(deps.storage).unwrap_or_default();
            let ts = TREASURY_SHARE.load(deps.storage).unwrap_or(0);
            let mut tt = Uint128::zero();
            for item in TOTAL_TREASURY.range(deps.storage, None, None, cosmwasm_std::Order::Ascending) {
                let (_, v) = item?;
                tt = tt.checked_add(v)?;
            }
            to_json_binary(&TreasuryConfigResponse {
                treasury_list: tl,
                treasury_share: ts,
                total_treasury: tt,
            })
        }
        QueryMsg::GameConfig { game_id } => {
            let cfg = GAME_CONFIGS
                .may_load(deps.storage, &game_id)?
                .unwrap_or_default();
            to_json_binary(&cfg)
        }
        QueryMsg::ListAdmins {} => {
            let admins = list_admins(deps).map_err(|e| cosmwasm_std::StdError::generic_err(e.to_string()))?;
            to_json_binary(&AdminsResponse { admins })
        }
        QueryMsg::Allowance { owner, spender, token } => {
            to_json_binary(&query_allowance(deps, owner, spender, token)?)
        }
        QueryMsg::GameLimit { game_id } => {
            to_json_binary(&crate::games::query_game_limit(deps, game_id)?)
        }
        QueryMsg::ListGames {} => {
            to_json_binary(&crate::games::query_list_games(deps)?)
        }
        QueryMsg::RoundStatus { player, game_id, round_id } => {
            to_json_binary(&crate::games::query_round_status(deps, player, game_id, round_id)?)
        }
        QueryMsg::DailyStats { player, date, game_id } => {
            to_json_binary(&crate::games::query_daily_stats(deps, env.clone(), player, date, game_id)?)
        }
        QueryMsg::GameEngineConfigQuery { game_id } => {
            to_json_binary(&game_query_engine(deps, game_id)?)
        }
        QueryMsg::ListPrc20 {} => {
            let tokens = list_prc20(deps).map_err(|e| cosmwasm_std::StdError::generic_err(e.to_string()))?;
            to_json_binary(&Prc20ListResponse { tokens })
        }

        // ====================================================================
        // 🀄 三国卡牌：所有 Sanguo* 查询变体统一走子模块入口
        // ====================================================================
        other => crate::sanguo::query_sanguo(deps, env, other),
    }
}

// ========================================================================
// 🀄 V2 迁移（方案 Z，裁定 8.1）：卡牌模板 60 张 × 11 字段编译期内嵌表
//   数据源：卡牌设计/templates_v2_backfill_60.json（与 build_backfill_60.py /
//   设计终稿文档 3 三方一致；华佗等 7 张数值修正卡已含终稿值）
// ========================================================================

pub struct V2Tpl {
    pub id: &'static str,
    pub name: &'static str,
    pub rarity: &'static str,
    pub attack: u32,
    pub defense: u32,
    pub weight: u32,
    pub faction: &'static str,
    pub troop: &'static str,
    pub cost: u8,
    pub skill_id: &'static str,
    pub bonds: &'static [&'static str],
}

/// 7 张数值修正卡断言（设计终稿文档 3「7 张数值修正清单」：本文档值为准）
pub const V2_FIXED_CARDS: [(&str, u32, u32); 7] = [
    ("huatuo", 78, 96),         // 华佗 60/95 → 78/96
    ("sunshangxiang", 92, 78),  // 孙尚香 90/72 → 92/78
    ("madai", 90, 78),          // 马岱 84/76 → 90/78
    ("yanliang", 98, 78),       // 颜良 96/74 → 98/78
    ("daqiao", 58, 72),         // 大乔 55/70 → 58/72
    ("xiaoqiao", 56, 74),       // 小乔 54/68 → 56/74
    ("diaochan", 56, 74),       // 貂蝉 52/66 → 56/74
];

/// 稀有度分布断言：common 16 / rare 18 / epic 16 / legend 10（RARITY_COUNT 同步）
pub const V2_RARITY_COUNT: [u32; 4] = [16, 18, 16, 10];

pub const V2_TEMPLATE_BACKFILL: [V2Tpl; 60] = [
    V2Tpl { id: "caocao", name: "曹操", rarity: "legend", attack: 105, defense: 95, weight: 3, faction: "wei", troop: "strategist", cost: 5, skill_id: "jian_xiong", bonds: &["caowei_shuangbi", "xuchu_huwei"] },
    V2Tpl { id: "zhugeliang", name: "诸葛亮", rarity: "legend", attack: 110, defense: 90, weight: 3, faction: "shu", troop: "strategist", cost: 5, skill_id: "kong_cheng", bonds: &["wolong_fengchu", "shuhan_sanjie"] },
    V2Tpl { id: "guanyu", name: "关羽", rarity: "legend", attack: 115, defense: 85, weight: 3, faction: "shu", troop: "cavalry", cost: 5, skill_id: "wu_sheng", bonds: &["wuhu_shangjiang", "taoyuan_jieyi", "shuhan_sanjie", "guanyu_fuzi"] },
    V2Tpl { id: "zhaoyun", name: "赵云", rarity: "legend", attack: 100, defense: 100, weight: 3, faction: "shu", troop: "cavalry", cost: 5, skill_id: "long_dan", bonds: &["wuhu_shangjiang"] },
    V2Tpl { id: "lvbu", name: "吕布", rarity: "legend", attack: 120, defense: 80, weight: 3, faction: "qun", troop: "cavalry", cost: 5, skill_id: "wu_shuang", bonds: &["lvbu_diaochan", "dongzhuo_lvbu", "qunxiong_zhulu"] },
    V2Tpl { id: "simayi", name: "司马懿", rarity: "legend", attack: 95, defense: 110, weight: 3, faction: "wei", troop: "strategist", cost: 5, skill_id: "zhong_hu", bonds: &["caowei_shuangbi"] },
    V2Tpl { id: "liubei", name: "刘备", rarity: "legend", attack: 100, defense: 105, weight: 4, faction: "shu", troop: "guard", cost: 4, skill_id: "ren_de", bonds: &["taoyuan_jieyi", "shuhan_sanjie", "sunshangxiang_gongyaoji"] },
    V2Tpl { id: "sunquan", name: "孙权", rarity: "legend", attack: 98, defense: 102, weight: 4, faction: "wu", troop: "guard", cost: 4, skill_id: "zhi_heng", bonds: &["sunshi_sanxiong"] },
    V2Tpl { id: "sunjian", name: "孙坚", rarity: "legend", attack: 108, defense: 92, weight: 4, faction: "wu", troop: "cavalry", cost: 5, skill_id: "jiang_dong_meng_hu", bonds: &["sunshi_sanxiong"] },
    V2Tpl { id: "dongzhuo", name: "董卓", rarity: "legend", attack: 112, defense: 88, weight: 4, faction: "qun", troop: "guard", cost: 5, skill_id: "bao_nue", bonds: &["dongzhuo_lvbu", "qunxiong_zhulu"] },
    V2Tpl { id: "zhangfei", name: "张飞", rarity: "epic", attack: 95, defense: 75, weight: 12, faction: "shu", troop: "cavalry", cost: 4, skill_id: "pao_xiao", bonds: &["wuhu_shangjiang", "taoyuan_jieyi", "zhangfei_fuzi", "shuhan_sanjie"] },
    V2Tpl { id: "machao", name: "马超", rarity: "epic", attack: 90, defense: 80, weight: 12, faction: "shu", troop: "cavalry", cost: 4, skill_id: "xi_liang_tie_qi", bonds: &["wuhu_shangjiang"] },
    V2Tpl { id: "huangzhong", name: "黄忠", rarity: "epic", attack: 85, defense: 85, weight: 12, faction: "shu", troop: "archer", cost: 4, skill_id: "bai_bu_chuan_yang", bonds: &["wuhu_shangjiang"] },
    V2Tpl { id: "jiangwei", name: "姜维", rarity: "epic", attack: 88, defense: 82, weight: 12, faction: "shu", troop: "cavalry", cost: 4, skill_id: "qi_lin", bonds: &["shuhan_mingjiang"] },
    V2Tpl { id: "zhouyu", name: "周瑜", rarity: "epic", attack: 80, defense: 90, weight: 12, faction: "wu", troop: "strategist", cost: 4, skill_id: "huo_gong", bonds: &["dongwu_dudu", "jiangdong_shuangbi", "huanggai_kurou"] },
    V2Tpl { id: "luxun", name: "陆逊", rarity: "epic", attack: 82, defense: 88, weight: 12, faction: "wu", troop: "strategist", cost: 4, skill_id: "lian_ying", bonds: &["dongwu_dudu"] },
    V2Tpl { id: "xiahoudun", name: "夏侯惇", rarity: "epic", attack: 92, defense: 78, weight: 12, faction: "wei", troop: "cavalry", cost: 4, skill_id: "ba_shi_dan_jing", bonds: &["weiwu_sanjiang"] },
    V2Tpl { id: "zhangliao", name: "张辽", rarity: "epic", attack: 88, defense: 82, weight: 12, faction: "wei", troop: "cavalry", cost: 4, skill_id: "wei_zhen_xiao_yao_jin", bonds: &["wuzi_liangjiang", "weiwu_sanjiang"] },
    V2Tpl { id: "guojia", name: "郭嘉", rarity: "epic", attack: 78, defense: 92, weight: 12, faction: "wei", troop: "strategist", cost: 3, skill_id: "gui_cai", bonds: &["xunyu_guojia"] },
    V2Tpl { id: "xuchu", name: "许褚", rarity: "epic", attack: 95, defense: 78, weight: 12, faction: "wei", troop: "guard", cost: 4, skill_id: "hu_chi", bonds: &["xuchu_huwei", "weiwu_sanjiang"] },
    V2Tpl { id: "zhanghe", name: "张郃", rarity: "epic", attack: 88, defense: 82, weight: 12, faction: "wei", troop: "cavalry", cost: 4, skill_id: "qiao_bian", bonds: &["wuzi_liangjiang"] },
    V2Tpl { id: "huanggai", name: "黄盖", rarity: "epic", attack: 86, defense: 80, weight: 12, faction: "wu", troop: "archer", cost: 3, skill_id: "ku_rou", bonds: &["huanggai_kurou"] },
    V2Tpl { id: "sunshangxiang", name: "孙尚香", rarity: "epic", attack: 92, defense: 78, weight: 12, faction: "wu", troop: "archer", cost: 3, skill_id: "gong_yao_ji", bonds: &["sunshangxiang_gongyaoji"] },
    V2Tpl { id: "madai", name: "马岱", rarity: "epic", attack: 90, defense: 78, weight: 12, faction: "shu", troop: "cavalry", cost: 3, skill_id: "lin_wei", bonds: &["shuhan_mingjiang"] },
    V2Tpl { id: "huatuo", name: "华佗", rarity: "epic", attack: 78, defense: 96, weight: 12, faction: "qun", troop: "strategist", cost: 3, skill_id: "shen_yi", bonds: &["qunxiong_zhulu"] },
    V2Tpl { id: "yanliang", name: "颜良", rarity: "epic", attack: 98, defense: 78, weight: 12, faction: "qun", troop: "cavalry", cost: 4, skill_id: "he_bei_ting_zhu", bonds: &["qunxiong_zhulu", "hebei_tingzhu"] },
    V2Tpl { id: "weiyan", name: "魏延", rarity: "rare", attack: 75, defense: 70, weight: 25, faction: "shu", troop: "cavalry", cost: 3, skill_id: "qi_mou", bonds: &["shuhan_mingjiang"] },
    V2Tpl { id: "pangtong", name: "庞统", rarity: "rare", attack: 70, defense: 75, weight: 25, faction: "shu", troop: "strategist", cost: 3, skill_id: "lian_huan", bonds: &["wolong_fengchu"] },
    V2Tpl { id: "fazheng", name: "法正", rarity: "rare", attack: 68, defense: 72, weight: 25, faction: "shu", troop: "strategist", cost: 3, skill_id: "qi_hua", bonds: &[] },
    V2Tpl { id: "ganning", name: "甘宁", rarity: "rare", attack: 80, defense: 60, weight: 25, faction: "wu", troop: "archer", cost: 3, skill_id: "jin_fan", bonds: &["ganning_taishici"] },
    V2Tpl { id: "taishici", name: "太史慈", rarity: "rare", attack: 72, defense: 68, weight: 25, faction: "wu", troop: "archer", cost: 3, skill_id: "shen_she", bonds: &["ganning_taishici"] },
    V2Tpl { id: "sunce", name: "孙策", rarity: "rare", attack: 78, defense: 62, weight: 25, faction: "wu", troop: "cavalry", cost: 4, skill_id: "xiao_ba_wang", bonds: &["jiangdong_shuangbi", "sunshi_sanxiong"] },
    V2Tpl { id: "lvmeng", name: "吕蒙", rarity: "rare", attack: 65, defense: 75, weight: 25, faction: "wu", troop: "strategist", cost: 3, skill_id: "bai_yi_du_jiang", bonds: &["dongwu_dudu"] },
    V2Tpl { id: "dengai", name: "邓艾", rarity: "rare", attack: 70, defense: 70, weight: 25, faction: "wei", troop: "strategist", cost: 3, skill_id: "tou_du_yin_ping", bonds: &[] },
    V2Tpl { id: "xunyu", name: "荀彧", rarity: "rare", attack: 62, defense: 78, weight: 25, faction: "wei", troop: "strategist", cost: 3, skill_id: "wang_zuo", bonds: &["xunyu_guojia"] },
    V2Tpl { id: "zhenji", name: "甄姬", rarity: "rare", attack: 58, defense: 72, weight: 25, faction: "wei", troop: "strategist", cost: 2, skill_id: "luo_shen", bonds: &[] },
    V2Tpl { id: "daqiao", name: "大乔", rarity: "rare", attack: 58, defense: 72, weight: 25, faction: "wu", troop: "strategist", cost: 2, skill_id: "guo_se", bonds: &["erqiao"] },
    V2Tpl { id: "xiaoqiao", name: "小乔", rarity: "rare", attack: 56, defense: 74, weight: 25, faction: "wu", troop: "strategist", cost: 2, skill_id: "tian_xiang", bonds: &["erqiao"] },
    V2Tpl { id: "zhoutai", name: "周泰", rarity: "rare", attack: 78, defense: 66, weight: 25, faction: "wu", troop: "guard", cost: 3, skill_id: "bu_qu", bonds: &[] },
    V2Tpl { id: "huangyueying", name: "黄月英", rarity: "rare", attack: 60, defense: 70, weight: 25, faction: "shu", troop: "strategist", cost: 3, skill_id: "ji_guan", bonds: &[] },
    V2Tpl { id: "guanxing", name: "关兴", rarity: "rare", attack: 76, defense: 64, weight: 25, faction: "shu", troop: "cavalry", cost: 3, skill_id: "an_guo", bonds: &["guanyu_fuzi"] },
    V2Tpl { id: "zhangbao", name: "张苞", rarity: "rare", attack: 78, defense: 62, weight: 25, faction: "shu", troop: "cavalry", cost: 3, skill_id: "hu_si", bonds: &["zhangfei_fuzi"] },
    V2Tpl { id: "chengong", name: "陈宫", rarity: "rare", attack: 60, defense: 72, weight: 25, faction: "qun", troop: "strategist", cost: 3, skill_id: "gong_tai", bonds: &[] },
    V2Tpl { id: "diaochan", name: "貂蝉", rarity: "rare", attack: 56, defense: 74, weight: 25, faction: "qun", troop: "strategist", cost: 2, skill_id: "bi_yue", bonds: &["lvbu_diaochan"] },
    V2Tpl { id: "liaohua", name: "廖化", rarity: "common", attack: 50, defense: 45, weight: 40, faction: "shu", troop: "cavalry", cost: 2, skill_id: "xian_feng", bonds: &["shuhan_mingjiang"] },
    V2Tpl { id: "jianyong", name: "简雍", rarity: "common", attack: 40, defense: 50, weight: 40, faction: "shu", troop: "archer", cost: 1, skill_id: "bian_shi", bonds: &[] },
    V2Tpl { id: "sunqian", name: "孙乾", rarity: "common", attack: 38, defense: 48, weight: 40, faction: "shu", troop: "archer", cost: 1, skill_id: "shi_ming", bonds: &[] },
    V2Tpl { id: "mizhu", name: "糜竺", rarity: "common", attack: 42, defense: 42, weight: 40, faction: "shu", troop: "guard", cost: 1, skill_id: "fu_shang", bonds: &[] },
    V2Tpl { id: "zhugejin", name: "诸葛瑾", rarity: "common", attack: 45, defense: 55, weight: 40, faction: "wu", troop: "guard", cost: 2, skill_id: "dun_hou", bonds: &[] },
    V2Tpl { id: "lusu", name: "鲁肃", rarity: "common", attack: 48, defense: 52, weight: 40, faction: "wu", troop: "guard", cost: 2, skill_id: "lian_meng", bonds: &["dongwu_dudu"] },
    V2Tpl { id: "chengyu", name: "程昱", rarity: "common", attack: 44, defense: 46, weight: 40, faction: "wei", troop: "guard", cost: 2, skill_id: "mou_zhu", bonds: &[] },
    V2Tpl { id: "jiaxu", name: "贾诩", rarity: "common", attack: 46, defense: 44, weight: 40, faction: "wei", troop: "strategist", cost: 2, skill_id: "du_shi", bonds: &[] },
    V2Tpl { id: "caohong", name: "曹洪", rarity: "common", attack: 52, defense: 48, weight: 40, faction: "wei", troop: "cavalry", cost: 2, skill_id: "piao_qi", bonds: &[] },
    V2Tpl { id: "xuyou", name: "许攸", rarity: "common", attack: 46, defense: 44, weight: 40, faction: "wei", troop: "strategist", cost: 1, skill_id: "mou_luan", bonds: &[] },
    V2Tpl { id: "zhangzhao", name: "张昭", rarity: "common", attack: 42, defense: 50, weight: 40, faction: "wu", troop: "strategist", cost: 1, skill_id: "wen_hou", bonds: &[] },
    V2Tpl { id: "zhuran", name: "朱然", rarity: "common", attack: 50, defense: 46, weight: 40, faction: "wu", troop: "archer", cost: 2, skill_id: "an_guo_2", bonds: &[] },
    V2Tpl { id: "guanping", name: "关平", rarity: "common", attack: 54, defense: 44, weight: 40, faction: "shu", troop: "cavalry", cost: 2, skill_id: "tan_zhi", bonds: &["guanyu_fuzi"] },
    V2Tpl { id: "zhoucang", name: "周仓", rarity: "common", attack: 56, defense: 42, weight: 40, faction: "shu", troop: "guard", cost: 1, skill_id: "zhong_yong", bonds: &[] },
    V2Tpl { id: "tianfeng", name: "田丰", rarity: "common", attack: 44, defense: 46, weight: 40, faction: "qun", troop: "strategist", cost: 2, skill_id: "gang_zhi", bonds: &["hebei_tingzhu"] },
    V2Tpl { id: "shenpei", name: "审配", rarity: "common", attack: 46, defense: 48, weight: 40, faction: "qun", troop: "strategist", cost: 1, skill_id: "zheng_nan", bonds: &["hebei_tingzhu"] },
];

// ========================================================================
// 🀄 V2 迁移用旧存储键声明（新代码已删除这些常量，仅 migrate 读取旧数据）
// ========================================================================

/// 旧版提案押金（PAXI，upaxi 计量；锁定时已 add_house(empty)）→ M3 收敛到 PENDING_REFUNDS
const LEGACY_PROPOSAL_DEPOSITS: cw_storage_plus::Map<&Addr, u128> =
    cw_storage_plus::Map::new("sanguo_proposal_deposits");
/// 旧版合规日限 Item → M2 收敛进 SanguoParams 后删键
const LEGACY_DAILY_DRAW_LIMIT: cw_storage_plus::Item<u64> =
    cw_storage_plus::Item::new("sg_daily_draw_limit");
const LEGACY_DAILY_FRAGMENT_CAP: cw_storage_plus::Item<u64> =
    cw_storage_plus::Item::new("sg_daily_fragment_cap");

#[entry_point]
pub fn migrate(
    deps: DepsMut,
    _env: Env,
    msg: MigrateMsg,
) -> Result<Response, ContractError> {
    let stored = cw2::get_contract_version(deps.storage)?;
    // ==================================================================
    // M1（裁定 8.2）：版本校验放宽——主版本 0 内允许跨次版本（0.1.1 → 0.2.0）。
    //   仅当主版本不同（或旧版本号无法解析）时拒绝；空版本（从未写过 cw2）放行。
    // ==================================================================
    if !stored.version.is_empty() {
        let stored_parts: Vec<&str> = stored.version.split('.').collect();
        let curr_parts: Vec<&str> = CONTRACT_VERSION.split('.').collect();
        let comparable = stored_parts.len() >= 2 && curr_parts.len() >= 2;
        if !comparable || stored_parts[0] != curr_parts[0] {
            return Err(ContractError::VersionMismatch {
                expected: CONTRACT_VERSION.to_string(),
                got: stored.version,
            });
        }
    }

    // ==================================================================
    // 运营白名单：msg.operators 传了就覆盖（重复 migrate 亦可用本入口改名单）
    // ==================================================================
    if let Some(ops) = &msg.operators {
        for a in ops {
            deps.api.addr_validate(a)?;
        }
        crate::sanguo::state::OPERATOR_LIST.save(deps.storage, ops)?;
    }

    // ==================================================================
    // MIGRATION_V2_DONE 防重入（裁定 8.1）：V2 数据迁移只执行一次。
    //   二次 migrate 仅刷新版本号后直接返回（各迁移步骤本均幂等，跳过省 gas；
    //   exec 侧 SanguoMigrateFragments 依赖该标记放行）。
    // ==================================================================
    if crate::sanguo::state::MIGRATION_V2_DONE
        .may_load(deps.storage)?
        .unwrap_or(false)
    {
        cw2::set_contract_version(deps.storage, CONTRACT_NAME, CONTRACT_VERSION)?;
        return Ok(Response::new()
            .add_attribute("action", "migrate")
            .add_attribute("from_version", stored.version)
            .add_attribute("to_version", CONTRACT_VERSION)
            .add_attribute("already_migrated", "true"));
    }

    // ==================================================================
    // 🟢 审计 H5 修复（2026-10-11）：遗留 TKCC 对局清零守卫。
    //   碎片版合约的领奖/取消路径不再付 TKCC；若遗留对局未结清就 migrate，
    //   旧对局玩家的 TKCC 入场费/奖励将永久无法取回。默认拒绝迁移；
    //   管理员可传 ignore_legacy_battles = true 强制跳过（自担风险）。
    //   守卫在任何状态写入之前执行，失败即整体回滚、不落版本号。
    // ==================================================================
    if !msg.ignore_legacy_battles.unwrap_or(false) {
        use cosmwasm_std::Order;
        let mut pvp_unsettled: u64 = 0;
        for item in crate::sanguo::state::PVP_MATCHES.range(deps.storage, None, None, Order::Ascending) {
            let (_, m) = item?;
            let terminal = matches!(m.status, crate::sanguo::state::PvpStatus::Cancelled)
                || (matches!(m.status, crate::sanguo::state::PvpStatus::Finished) && m.reward_claimed);
            if !terminal {
                pvp_unsettled += 1;
            }
        }
        let mut royale_unsettled: u64 = 0;
        for item in crate::sanguo::state::ROYALE_MATCHES.range(deps.storage, None, None, Order::Ascending) {
            let (_, m) = item?;
            let terminal = matches!(m.status, crate::sanguo::state::RoyaleStatus::Cancelled)
                || (matches!(m.status, crate::sanguo::state::RoyaleStatus::Finished) && m.reward_claimed);
            if !terminal {
                royale_unsettled += 1;
            }
        }
        let mut pending_players: u64 = 0;
        for item in crate::sanguo::state::PENDING_REWARDS.range(deps.storage, None, None, Order::Ascending) {
            let (_, v) = item?;
            if !v.is_empty() {
                pending_players += 1;
            }
        }
        if pvp_unsettled > 0 || royale_unsettled > 0 || pending_players > 0 {
            return Err(ContractError::SanguoLegacyBattlesUnsettled {
                pvp: pvp_unsettled,
                royale: royale_unsettled,
                pending: pending_players,
            });
        }
    }

    cw2::set_contract_version(deps.storage, CONTRACT_NAME, CONTRACT_VERSION)?;

    // 🟢 7A：支付代币默认原生 PAXI（仅当未设置过才写，避免覆盖管理员已切的币种）
    if crate::state::PAYMENT_TOKEN.may_load(deps.storage)?.is_none() {
        crate::state::PAYMENT_TOKEN.save(deps.storage, &crate::state::empty_addr())?;
    }

    // ==================================================================
    // 🟢 参数合并（审计报告 #4 语义的 V2 版）：
    //    旧 SanguoParams JSON 可被新结构直读——全部旧字段名保留在 deprecated
    //    块（deny_unknown_fields 兼容），新字段 serde(default) 补 0 后由访问器
    //    回退默认值。此处仅对「管理员已调过的生效字段」做零值兜底；deprecated
    //    计费字段（C4：exec 不再读取）一律不管（裁定 M2：params deprecated 不动）。
    // ==================================================================
    let (draw_migrated, frag_migrated) = {
        let loaded = crate::sanguo::state::SANGUO_PARAMS.may_load(deps.storage)?;
        let d = crate::sanguo::state::SanguoParams::default_sanguo();
        let mut p = match loaded {
            Some(p) => p,
            None => d.clone(),
        };
        let z128 = |v: u128, dv: u128| if v == 0 { dv } else { v };
        let z32 = |v: u32, dv: u32| if v == 0 { dv } else { v };
        p.stamina_pack_price = z128(p.stamina_pack_price, d.stamina_pack_price);
        p.daily_ai_limit = if p.daily_ai_limit == 0 { d.daily_ai_limit } else { p.daily_ai_limit };
        p.max_cards = z32(p.max_cards, d.max_cards);
        if p.upgrade_atk_boost == [0u32; 4] { p.upgrade_atk_boost = d.upgrade_atk_boost; }
        if p.upgrade_def_boost == [0u32; 4] { p.upgrade_def_boost = d.upgrade_def_boost; }
        if p.ai_frag_reward == <[crate::sanguo::state::Fragments; 5]>::default() { p.ai_frag_reward = d.ai_frag_reward; }
        if p.pvp_frag_reward == crate::sanguo::state::Fragments::default() { p.pvp_frag_reward = d.pvp_frag_reward; }
        if p.royale_frag_reward == crate::sanguo::state::Fragments::default() { p.royale_frag_reward = d.royale_frag_reward; }

        // M2 收敛：旧合规日限 Item（sg_daily_*）→ params 对应字段，搬值后删键
        let mut draw_migrated = false;
        let mut frag_migrated = false;
        if let Some(v) = LEGACY_DAILY_DRAW_LIMIT.may_load(deps.storage)? {
            if v > 0 {
                p.daily_draw_limit = v;
                draw_migrated = true;
            }
            LEGACY_DAILY_DRAW_LIMIT.remove(deps.storage);
        }
        if let Some(v) = LEGACY_DAILY_FRAGMENT_CAP.may_load(deps.storage)? {
            if v > 0 {
                p.daily_fragment_cap = v;
                frag_migrated = true;
            }
            LEGACY_DAILY_FRAGMENT_CAP.remove(deps.storage);
        }
        crate::sanguo::state::SANGUO_PARAMS.save(deps.storage, &p)?;
        (draw_migrated, frag_migrated)
    };

    // ==================================================================
    // M2（裁定 8.2）：合规总闸硬化（C1/C3）。
    //   - 删 compliance_mode 存储键（通用博弈 Play 已在 games/mod.rs 代码级
    //     永久下线，不依赖该键，删键后不存在被误放开的可能）；
    //   - SanguoConfig 三字段清空：tkcc_token/burn_address = ""、tap = []。
    //     字段保留仅为旧 JSON deny_unknown_fields 兼容，exec 不再读取（C3）。
    // ==================================================================
    const LEGACY_COMPLIANCE_MODE: cw_storage_plus::Item<bool> =
        cw_storage_plus::Item::new("compliance_mode");
    LEGACY_COMPLIANCE_MODE.remove(deps.storage);

    crate::sanguo::state::SANGUO_CONFIG.save(
        deps.storage,
        &crate::sanguo::state::SanguoConfig {
            tkcc_token: Addr::unchecked(""),
            burn_address: Addr::unchecked(""),
            tap_addresses: vec![],
        },
    )?;

    // ==================================================================
    // M3（裁定 8.2）：提案押金收敛 PROPOSAL_DEPOSITS → PENDING_REFUNDS。
    //   旧押金在 propose 时已从玩家内部 PAXI 余额扣除并 add_house(empty_addr)，
    //   故此处仅做记账转移（不动 HOUSE_BANKROLL）；玩家随后调
    //   SanguoRefundProposalDeposit 领取（该入口 credit_balance + sub_house）。
    //   旧提案本体不动：0.1.1 migrate 已将未结算提案冻结（executed=true），
    //   V2 分发层已删除全部投票入口，旧提案永不可再被执行/取消。
    // ==================================================================
    let (refund_players, refund_total) = {
        use cosmwasm_std::Order;
        let mut players = 0u32;
        let mut total = Uint128::zero();
        // 🟢 审计修复（P0 借用冲突）：range() 迭代器持有 storage 的不可变借用，
        //    在迭代过程中直接 save/remove 会触发 E0502。先一次性收集（K::Output 为
        //    owned Addr，不借用 storage），再遍历写入。
        let deposits: Vec<(Addr, u128)> = LEGACY_PROPOSAL_DEPOSITS
            .range(deps.storage, None, None, Order::Ascending)
            .collect::<StdResult<Vec<_>>>()?;
        for (addr, amount) in deposits {
            let cur = crate::sanguo::state::PENDING_REFUNDS
                .may_load(deps.storage, &addr)?
                .unwrap_or(0);
            let merged = cur.checked_add(amount).ok_or(ContractError::Overflow)?;
            crate::sanguo::state::PENDING_REFUNDS.save(deps.storage, &addr, &merged)?;
            LEGACY_PROPOSAL_DEPOSITS.remove(deps.storage, &addr);
            players += 1;
            total = total.checked_add(Uint128::from(amount)).map_err(|_| ContractError::Overflow)?;
        }
        (players, total)
    };

    // ==================================================================
    // 付费系统初始化（只在对应项未配置时写入默认值，幂等）
    // ==================================================================
    use crate::state::{PROJECT_WALLETS, WALLET_INDEX, CARD_PRICES};
    if PROJECT_WALLETS.may_load(deps.storage)?.is_none() {
        // 🟢 2026-10-21 公开模板化：原实现在此处把**三个具体的项目方钱包地址**
        //    写死为默认值。对模板而言这是错的——部署者会把原作者的国库地址
        //    一起带进自己的合约里。现在改为**不设任何默认地址**：
        //    由管理员在部署后调用 `SanguoSetProjectWallets` 显式配置。
        //    未配置时 pick_project_wallet 返回 SanguoNoProjectWallet
        //    （错误文案已写明 "admin must call SanguoSetProjectWallets"），
        //    属于「显式失败」而非「静默把款打给陌生人」。
        PROJECT_WALLETS.save(deps.storage, &vec![])?;
        WALLET_INDEX.save(deps.storage, &0u64)?;
    }

    // ==================================================================
    // 方案 Z（裁定 8.1）：CardTemplate 60 张 × 11 字段覆盖写 + 断言。
    //   原子性：先对内嵌表做完整预校验（60 张 / id 唯一 / 稀有度分布
    //   16-18-16-10 / 7 张修正值），任一失败即整体报错回滚，不写半态。
    //   注记①（M4/M5 顺序冲突裁定）：PLAYER_BATTLE_ORDER 此处不清——
    //   由 SanguoMigrateFragments（M5）逐玩家读取补偿后清理。
    // ==================================================================
    let mut templates: Vec<crate::sanguo::state::CardTemplate> =
        Vec::with_capacity(V2_TEMPLATE_BACKFILL.len());
    let mut rarity_cnt = [0u32; 4];
    for (i, t) in V2_TEMPLATE_BACKFILL.iter().enumerate() {
        let idx = match t.rarity {
            "common" => 0usize,
            "rare" => 1,
            "epic" => 2,
            "legend" => 3,
            other => {
                return Err(ContractError::Std(cosmwasm_std::StdError::generic_err(
                    format!("backfill[{}] {} 非法稀有度 {}", i, t.id, other),
                )))
            }
        };
        rarity_cnt[idx] += 1;
        if templates.iter().any(|c| c.id == t.id) {
            return Err(ContractError::Std(cosmwasm_std::StdError::generic_err(
                format!("backfill[{}] {} id 重复", i, t.id),
            )));
        }
        templates.push(crate::sanguo::state::CardTemplate {
            id: t.id.to_string(),
            name: t.name.to_string(),
            rarity: t.rarity.to_string(),
            attack: t.attack,
            defense: t.defense,
            weight: t.weight,
            faction: t.faction.to_string(),
            troop: t.troop.to_string(),
            cost: t.cost,
            skill_id: t.skill_id.to_string(),
            bonds: t.bonds.iter().map(|b| b.to_string()).collect(),
        });
    }
    if rarity_cnt != V2_RARITY_COUNT {
        return Err(ContractError::Std(cosmwasm_std::StdError::generic_err(
            format!("稀有度分布 {:?} != 预期 {:?}", rarity_cnt, V2_RARITY_COUNT),
        )));
    }
    for (id, atk, def) in V2_FIXED_CARDS {
        let c = templates.iter().find(|c| c.id == id).ok_or_else(|| {
            ContractError::Std(cosmwasm_std::StdError::generic_err(format!("修正卡 {} 缺失", id)))
        })?;
        if c.attack != atk || c.defense != def {
            return Err(ContractError::Std(cosmwasm_std::StdError::generic_err(
                format!("修正卡 {} = {}/{} != 预期 {}/{}", id, c.attack, c.defense, atk, def),
            )));
        }
    }
    crate::sanguo::state::CARD_TEMPLATES.save(deps.storage, &templates)?;
    // 🟢 审计修复：覆盖写后同步卡池计数（裁定 8.1 步骤 3「RARITY_COUNT 同步
    //    16/18/16/10」）。不同步则 query_rarity_count / query_collection 的
    //    total_templates / add_templates 上限校验仍基于旧 6 字段时代的计数。
    crate::sanguo::state::CARD_TEMPLATE_COUNT
        .save(deps.storage, &(templates.len() as u64))?;
    for (r, cnt) in [
        ("common", rarity_cnt[0]),
        ("rare", rarity_cnt[1]),
        ("epic", rarity_cnt[2]),
        ("legend", rarity_cnt[3]),
    ] {
        crate::sanguo::state::RARITY_COUNT.save(deps.storage, r, &(cnt as u64))?;
    }

    // 默认卡牌价格（只对未设价的卡牌）：普通 5 / 稀有 15 / 史诗 40 / 传说 100 PAXI
    for tpl in &templates {
        if !CARD_PRICES.has(deps.storage, &tpl.name) {
            let price = match tpl.rarity.as_str() {
                "common" => Uint128::from(5_000_000u128),
                "rare"   => Uint128::from(15_000_000u128),
                "epic"   => Uint128::from(40_000_000u128),
                "legend" => Uint128::from(100_000_000u128),
                _ => continue,
            };
            CARD_PRICES.save(deps.storage, &tpl.name, &price)?;
        }
    }

    // 🔴 已删除 FRAGMENT_PRICES 初始化块（T7）：四桶碎片购买废弃，
    //    统一碎片按 params.card_frag_price[4] 计价，不再落任何键。

    // ==================================================================
    // 方案 Z 收尾（裁定 8.1 步骤 4）：写 MIGRATION_V2_DONE + 版本号。
    // ==================================================================
    crate::sanguo::state::MIGRATION_V2_DONE.save(deps.storage, &true)?;

    Ok(Response::new()
        .add_attribute("action", "migrate")
        .add_attribute("from_version", stored.version)
        .add_attribute("to_version", CONTRACT_VERSION)
        .add_attribute("legacy_refund_players", refund_players.to_string())
        .add_attribute("legacy_refund_total_upaxi", refund_total.to_string())
        .add_attribute("daily_draw_limit_migrated", draw_migrated.to_string())
        .add_attribute("daily_fragment_cap_migrated", frag_migrated.to_string())
        .add_attribute("templates_overwritten", templates.len().to_string()))
}

// ========================================================================
// ⚠️ reply 入口 —— 保留 swap reply，去掉 plugin game reply
// ========================================================================

#[entry_point]
pub fn reply(mut deps: DepsMut, _env: Env, reply: cosmwasm_std::Reply) -> Result<Response, ContractError> {
    match reply.id {
        crate::assets::swap::SWAP_SUBMSG_ID => reply_swap(deps, reply),
        // 🟢 修复（低危 1）：未知 reply id 直接报错，而不是静默 Ok。
        //    当前只有 SWAP_SUBMSG_ID 一个 SubMsg，本分支不会命中；
        //    一旦将来新增 SubMsg 却漏写 handler，这里会立刻暴露而非静默吞掉。
        _ => Err(ContractError::UnhandledReplyId {
            id: reply.id,
            known: vec![crate::assets::swap::SWAP_SUBMSG_ID],
        }),
    }
}

fn reply_swap(mut deps: DepsMut, reply: cosmwasm_std::Reply) -> Result<Response, ContractError> {
    let payload_str = std::str::from_utf8(&reply.payload)
        .map_err(|_| ContractError::SwapPayloadMalformed)?;
    let payload: serde_json::Value = serde_json::from_str(payload_str)
        .map_err(|_| ContractError::SwapPayloadMalformed)?;

    let player_addr = deps.api.addr_validate(
        payload["player"].as_str().ok_or(ContractError::SwapPayloadMalformed)?
    )?;
    let offer_denom = payload["offer_denom"].as_str()
        .ok_or(ContractError::SwapPayloadMalformed)?.to_string();
    // 🟢 M2 审计修复（2026-10-10）：parse 失败显式报错回滚，不再 unwrap_or(0)。
    //    旧写法解析失败时 offer_amount=0（回滚分支按 0 记账 → 用户资产凭空消失）、
    //    min_receive=0（滑点保护完全失效）。
    let offer_amount = Uint128::from(
        payload["offer_amount"].as_str()
            .ok_or(ContractError::SwapPayloadMalformed)?
            .parse::<u128>()
            .map_err(|_| ContractError::SwapPayloadMalformed)?,
    );
    let prc20 = payload["prc20"].as_str()
        .ok_or(ContractError::SwapPayloadMalformed)?.to_string();
    let min_receive = Uint128::from(
        payload["min_receive"].as_str()
            .ok_or(ContractError::SwapPayloadMalformed)?
            .parse::<u128>()
            .map_err(|_| ContractError::SwapPayloadMalformed)?,
    );
    let offer_is_native = payload["offer_is_native"].as_bool().unwrap_or(true);

    let offer_token_addr = if offer_is_native {
        empty_addr()
    } else {
        deps.api.addr_validate(&offer_denom)?
    };
    let target_token_addr = deps.api.addr_validate(&prc20)?;

    match reply.result {
        cosmwasm_std::SubMsgResult::Ok(sub_resp) => {
            // 🟢 修复：必须解析到真实接收量，否则交易失败自动回滚
            // 之前用 unwrap_or(min_receive) 会导致实际多收到的代币留在合约无人认领
            let actual_receive = match extract_swap_receive(&sub_resp) {
                Some(amount) => amount,
                None => {
                    return Err(ContractError::SwapReceiveParseFailed);
                }
            };

            let mut resp = Response::new()
                .add_attribute("action", "swap_settle_ok")
                .add_attribute("player", player_addr.as_str())
                .add_attribute("offer_denom", offer_denom)
                .add_attribute("offer_amount", offer_amount.to_string())
                .add_attribute("prc20_received", actual_receive.to_string());

            // 🟢 审计修复（高危）：原实现在滑点超限时只加一个 `slippage_exceeded` attribute，
            // 然后照样把钱记给用户 —— min_receive 这个参数等于完全失效，
            // 用户可以先用大额单把池子砸出滑点再成交，或遭遇三明治攻击而无任何保护。
            // 这里改为直接返回 Err：整笔交易（含 MsgSwap 子消息）回滚，
            // 用户的 offer 侧余额也一并还原，等价于「没换过」。
            if actual_receive < min_receive {
                return Err(ContractError::SwapSlippageExceeded {
                    min_receive: min_receive.to_string(),
                    actual_receive: actual_receive.to_string(),
                });
            }

            let key = (&player_addr, &target_token_addr);
            let mut bal = BALANCES.may_load(deps.storage, key)?.unwrap_or(Uint128::zero());
            bal = bal.checked_add(actual_receive).map_err(|_| ContractError::Overflow)?;
            BALANCES.save(deps.storage, key, &bal)?;

            update_user_totals(deps.branch(), &target_token_addr, actual_receive, true)?;

            Ok(resp)
        }
        cosmwasm_std::SubMsgResult::Err(e) => {
            let mut resp = Response::new()
                .add_attribute("action", "swap_settle_rollback")
                .add_attribute("player", player_addr.as_str())
                .add_attribute("offer_denom", offer_denom)
                .add_attribute("rollback_amount", offer_amount.to_string())
                .add_attribute("error", e);

            let key = (&player_addr, &offer_token_addr);
            let mut bal = BALANCES.may_load(deps.storage, key)?.unwrap_or(Uint128::zero());
            bal = bal.checked_add(offer_amount).map_err(|_| ContractError::Overflow)?;
            BALANCES.save(deps.storage, key, &bal)?;

            update_user_totals(deps.branch(), &offer_token_addr, offer_amount, true)?;

            Ok(resp)
        }
    }
}

/// 从 Swap 模块返回的事件里解析「实际到账数量」
///
/// 🟢 审计修复：原实现的模糊匹配会把 `min_receive` / `expected_receive` 之类的
/// **输入参数事件**当成实际到账金额（"receive" 是 "min_receive" 的子串），
/// 导致用户被按 min_receive 记账 —— 实际多收到的代币留在合约无人认领，
/// 实际少收到时则让用户凭空多拿钱。
///
/// 改进点：
///   1. 显式排除 min / expected / max / offer / fee 等非结果字段
///   2. 优先在 event type 含 "swap" 的事件里找（Swap 模块自己的事件）
///   3. 先做精确 key 匹配，再做模糊匹配
fn extract_swap_receive(sub_resp: &cosmwasm_std::SubMsgResponse) -> Option<Uint128> {
    const EXACT_KEYS: [&str; 8] = [
        "receive_amount", "received_amount", "out_amount", "output_amount",
        "prc20_amount", "target_amount", "swapped_amount", "to_amount",
    ];
    const FUZZY_KEYS: [&str; 3] = ["receive_amount", "receive", "received"];
    // 这些是入参/无关字段，绝不能被当成结果
    const EXCLUDE: [&str; 6] = ["min", "max", "expected", "offer", "fee", "creator"];

    let try_attr = |key: &str, value: &str| -> Option<Uint128> {
        let key_lower = key.to_lowercase();
        if EXCLUDE.iter().any(|x| key_lower.contains(x)) {
            return None;
        }
        value.parse::<u128>().ok().map(Uint128::from)
    };

    // 第一轮：只在 swap 事件里做精确匹配
    for event in sub_resp.events.iter().filter(|e| e.ty.to_lowercase().contains("swap")) {
        for attr in &event.attributes {
            if EXACT_KEYS.contains(&attr.key.to_lowercase().as_str()) {
                if let Some(v) = try_attr(&attr.key, &attr.value) {
                    return Some(v);
                }
            }
        }
    }

    // 第二轮：swap 事件里的模糊匹配
    for event in sub_resp.events.iter().filter(|e| e.ty.to_lowercase().contains("swap")) {
        for attr in &event.attributes {
            let key_lower = attr.key.to_lowercase();
            if FUZZY_KEYS.iter().any(|f| key_lower.contains(f)) {
                if let Some(v) = try_attr(&attr.key, &attr.value) {
                    return Some(v);
                }
            }
        }
    }

    // 第三轮：所有事件里做精确匹配（兜底，链上事件类型命名可能不同）
    for event in &sub_resp.events {
        for attr in &event.attributes {
            if EXACT_KEYS.contains(&attr.key.to_lowercase().as_str()) {
                if let Some(v) = try_attr(&attr.key, &attr.value) {
                    return Some(v);
                }
            }
        }
    }

    None
}

// ========================================================================
// 以下是从旧 contract.rs 保留的公共函数（update_user_totals / 
// calc_house_edge_split / assert_solvency）
// games/mod.rs 和 swap 模块引用这些函数
// ========================================================================

pub fn update_user_totals(
    deps: DepsMut,
    token: &Addr,
    delta: Uint128,
    is_deposit: bool,
) -> Result<(), ContractError> {
    let mut total = USER_TOTALS.may_load(deps.storage, token)?.unwrap_or(Uint128::zero());
    if is_deposit {
        total = total.checked_add(delta).map_err(|_| ContractError::Overflow)?;
    } else {
        if total < delta {
            return Err(ContractError::Insolvent);
        }
        total = total.checked_sub(delta).map_err(|_| ContractError::Overflow)?;
    }
    USER_TOTALS.save(deps.storage, token, &total)?;
    Ok(())
}

/// PRC-20 balance 查询（cw20 兼容）
#[derive(serde::Serialize)]
#[serde(rename_all = "snake_case")]
enum Prc20QueryMsg {
    Balance { address: String },
}

#[derive(serde::Deserialize)]
struct Prc20BalanceResponse {
    balance: Uint128,
}

/// 偿付能力兜底检查：合约实际持有的代币，必须 >= 用户权益 + 庄家准备金
///
/// 会计恒等式（每一条资金路径都成立）：
/// ```text
/// 合约实际余额 == USER_TOTALS[token] + HOUSE_BANKROLL[token]
/// ```
///   - 充值：余额 +Δ，USER_TOTALS +Δ
///   - Deduct：USER_TOTALS -bet（余额不变，钱还在合约里）
///   - 输局：USER_TOTALS 不动，HOUSE_BANKROLL += house_remain，同时 burn/treasury 的钱流出合约
///   - 赢局：USER_TOTALS += payout，HOUSE_BANKROLL -= profit，净变化 = +bet（正好补回 Deduct）
///
/// 一旦这个等式被打破，说明出现了账外流失（被偷、被误转、会计 bug），
/// 继续放款只会让后面的用户承担前面的窟窿，必须立刻停下。
///
/// ⚠️ PRC-20 分支查不到余额时**放行而不是报错**：偿付能力检查是兜底，
/// 绝不能因为一个查询失败就把用户的提现永久卡死（那比资不抵债还糟）。
/// 原生 upaxi 分支是硬检查，查不到就是真的出事了。
pub fn assert_solvency(
    deps: cosmwasm_std::Deps,
    contract_addr: &Addr,
    token: &Addr,
    outbound: bool,
) -> Result<(), ContractError> {
    let user_total = USER_TOTALS.may_load(deps.storage, token)?.unwrap_or(Uint128::zero());
    let hb = HOUSE_BANKROLL.may_load(deps.storage, token)?.unwrap_or(Uint128::zero());
    let required = user_total.checked_add(hb).map_err(|_| ContractError::Overflow)?;

    let native = empty_addr();
    let balance = if token == &native {
        deps.querier
            .query_balance(contract_addr.as_str(), "upaxi")?
            .amount
    } else {
        let query: cosmwasm_std::QueryRequest<cosmwasm_std::Empty> =
            cosmwasm_std::QueryRequest::Wasm(cosmwasm_std::WasmQuery::Smart {
                contract_addr: token.to_string(),
                msg: to_json_binary(&Prc20QueryMsg::Balance {
                    address: contract_addr.to_string(),
                })?,
            });
        match deps.querier.query::<Prc20BalanceResponse>(&query) {
            Ok(resp) => resp.balance,
            Err(_) => {
                // 出金路径：白名单代币余额查询失败属异常，必须报错回滚，
                // 否则账实缺口会被静默放过（防御纵深，H-1 安全审计修复）。
                // 非出金（如充值对账）路径保留静默放行，可用性优先（见上方注释）。
                if outbound {
                    return Err(ContractError::SolvencyQueryFailed {
                        token: token.to_string(),
                    });
                }
                return Ok(());
            }
        }
    };

    if balance < required {
        return Err(ContractError::Insolvent);
    }
    Ok(())
}

// ========================================================================
// house_edge 三分结算（多币种，输局时调用）
// ========================================================================

pub fn update_house_bankroll(
    deps: DepsMut,
    token: &Addr,
    house_edge_amount: Uint128,
) -> Result<(), ContractError> {
    if house_edge_amount.is_zero() {
        return Ok(());
    }
    let mut hb = HOUSE_BANKROLL.may_load(deps.storage, token)?.unwrap_or(Uint128::zero());
    hb = hb.checked_add(house_edge_amount).map_err(|_| ContractError::Overflow)?;
    HOUSE_BANKROLL.save(deps.storage, token, &hb)?;
    Ok(())
}

pub fn calc_house_edge_split(
    deps: &mut DepsMut,
    env: &Env,
    game_id: &str,
    token: &Addr,
    house_edge_amount: Uint128,
) -> Result<SplitResult, ContractError> {
    if house_edge_amount.is_zero() {
        return Ok(SplitResult::empty());
    }

    let is_native = token == &empty_addr();

    // 只销毁 PRC-20，原生 PAXI 不销毁
    let burn_amount = if is_native {
        Uint128::zero()
    } else if let Some(rate) = effective_burn_rate(&deps.as_ref(), game_id)? {
        house_edge_amount
            .checked_mul(Uint128::from(rate)).map_err(|_| ContractError::Overflow)?
            .checked_div(Uint128::from(10000u128)).map_err(|_| ContractError::Overflow)?
    } else {
        Uint128::zero()
    };

    // 🟢 复审 #1 修正：从多个抽水地址里按 height+nanos 确定性轮转选一个（见下，非随机）
    let treasury_list = TREASURY_LIST.load(deps.storage).unwrap_or_default();
    let has_treasury = !treasury_list.is_empty();
    // 🟢 M4 审计修正（2026-10-10）：这是 height+nanos 的**确定性函数**，不是随机数，
    //    验证者可预先推知下一笔交易走哪个地址（均为项目方地址，危害有限）。
    //    统一表述为「确定性轮转」，不声称任何随机性保证。
    let treasury_addr = if has_treasury {
        let h = env.block.height as usize;
        let t = env.block.time.nanos() as usize;
        let idx = (h.wrapping_add(t)) % treasury_list.len();
        treasury_list[idx].clone()
    } else {
        String::new()
    };

    let mut treasury_amount = if treasury_addr.is_empty() {
        Uint128::zero()
    } else {
        let treasury_share = effective_treasury_share(&deps.as_ref(), game_id)?;
        if treasury_share > 0 {
            house_edge_amount
                .checked_mul(Uint128::from(treasury_share)).map_err(|_| ContractError::Overflow)?
                .checked_div(Uint128::from(10000u128)).map_err(|_| ContractError::Overflow)?
        } else {
            Uint128::zero()
        }
    };

    let burn_rate_bps = effective_burn_rate(&deps.as_ref(), game_id)?.unwrap_or(0);
    let total_bps = burn_rate_bps.checked_add(if treasury_addr.is_empty() { 0 } else { effective_treasury_share(&deps.as_ref(), game_id)? }).unwrap_or(10001);
    if total_bps > 10000 && !treasury_addr.is_empty() {
        let capped_ts = 10000u64.checked_sub(burn_rate_bps).unwrap_or(0);
        treasury_amount = if capped_ts > 0 {
            house_edge_amount
                .checked_mul(Uint128::from(capped_ts)).map_err(|_| ContractError::Overflow)?
                .checked_div(Uint128::from(10000u128)).map_err(|_| ContractError::Overflow)?
        } else {
            Uint128::zero()
        };
    }

    let house_remain = house_edge_amount
        .checked_sub(burn_amount).map_err(|_| ContractError::Overflow)?
        .checked_sub(treasury_amount).map_err(|_| ContractError::Overflow)?;

    let mut hb = HOUSE_BANKROLL.may_load(deps.storage, token)?.unwrap_or(Uint128::zero());
    hb = hb.checked_add(house_remain).map_err(|_| ContractError::Overflow)?;
    HOUSE_BANKROLL.save(deps.storage, token, &hb)?;

    let burn_msg = if !burn_amount.is_zero() {
        let mut total = TOTAL_BURNED.may_load(deps.storage, token)?.unwrap_or(Uint128::zero());
        total = total.checked_add(burn_amount).map_err(|_| ContractError::Overflow)?;
        TOTAL_BURNED.save(deps.storage, token, &total)?;

        if is_native {
            Some(BankMsg::Burn {
                amount: vec![Coin { denom: "upaxi".to_string(), amount: burn_amount }],
            }.into())
        } else {
            let wasm = WasmMsg::Execute {
                contract_addr: token.to_string(),
                msg: to_json_binary(&json!({
                    "burn": { "amount": burn_amount.to_string() }
                }))?,
                funds: vec![],
            };
            Some(wasm.into())
        }
    } else {
        None
    };

    let treasury_msg = if !treasury_amount.is_zero() {
        if treasury_addr.is_empty() {
            return Ok(SplitResult { burn_msg, treasury_msg: None, burn_amount, treasury_amount: Uint128::zero() });
        }
        let mut total = TOTAL_TREASURY.may_load(deps.storage, token)?.unwrap_or(Uint128::zero());
        total = total.checked_add(treasury_amount).map_err(|_| ContractError::Overflow)?;
        TOTAL_TREASURY.save(deps.storage, token, &total)?;

        if is_native {
            Some(BankMsg::Send {
                to_address: treasury_addr,
                amount: vec![Coin { denom: "upaxi".to_string(), amount: treasury_amount }],
            }.into())
        } else {
            let wasm = WasmMsg::Execute {
                contract_addr: token.to_string(),
                msg: to_json_binary(&json!({
                    "transfer": {
                        "recipient": treasury_addr,
                        "amount": treasury_amount.to_string()
                    }
                }))?,
                funds: vec![],
            };
            Some(wasm.into())
        }
    } else {
        None
    };

    Ok(SplitResult { burn_msg, treasury_msg, burn_amount, treasury_amount })
}

#[derive(Debug)]
pub struct SplitResult {
    pub burn_msg: Option<CosmosMsg>,
    pub treasury_msg: Option<CosmosMsg>,
    pub burn_amount: Uint128,
    pub treasury_amount: Uint128,
}

impl SplitResult {
    pub fn empty() -> Self {
        Self {
            burn_msg: None, treasury_msg: None,
            burn_amount: Uint128::zero(), treasury_amount: Uint128::zero(),
        }
    }
}
