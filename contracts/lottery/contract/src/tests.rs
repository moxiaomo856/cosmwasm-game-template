//! paxi-lottery-contract-simple —— 单元测试（simple 版精简集）
//!
//! 覆盖：
//! * 三档规格数值与非法档位拒绝
//! * 未配置 TKCC 时创建返回 `TkccNotConfigured`
//! * 5 人档建池扣费与参数快照（费用/人数全部来自 tier_spec）
//! * 建池者不能参与自己的池
//! * 满员开奖 + 领取 + 分配比例 + 总额守恒
//! * 超时未满员退款（simple 版开奖只看满员）
//! * 模板池平台托管 secret：无承诺禁开奖、哈希链揭示、第二轮轮换
//!
//! 🔑 鉴权模型：行动玩家一律是**交易发送方**（`info.sender`），无会话密钥。

use cosmwasm_std::testing::{mock_env, MockApi, MockQuerier, MockStorage};
use cosmwasm_std::{Addr, Env, OwnedDeps, Uint128};
use std::marker::PhantomData;

use paxi_common::BalanceLedger;

use crate::contract::{execute, instantiate, query};
use crate::msg::{ExecuteMsg, InstantiateMsg, LotteryAdminAction, QueryMsg};
use crate::state::{
    tier_spec, LotteryStatus, LOTTERIES, TIER_LARGE, TIER_LARGE_PAXI, TIER_MEDIUM, TIER_SMALL,
    TIER_SMALL_HIGH,
};

type Deps = OwnedDeps<MockStorage, MockApi, MockQuerier>;

fn mock_deps() -> Deps {
    OwnedDeps {
        storage: MockStorage::default(),
        api: MockApi::default().with_prefix("paxi"),
        querier: MockQuerier::default(),
        custom_query_type: PhantomData,
    }
}

fn addr(deps: &Deps, name: &str) -> Addr {
    deps.api.addr_make(name)
}

fn init(deps: &mut Deps, admins: Vec<String>, treasury: String) {
    let deployer = deps.api.addr_make("deployer");
    let msg = InstantiateMsg {
        admins,
        treasury,
        tkcc_token: None,
        tkcc_burn_address: None,
        lottery_config: None,
        multisig_threshold: Some(1),
    };
    instantiate(deps.as_mut(), mock_env(), cosmwasm_std::MessageInfo { sender: deployer, funds: vec![] }, msg).unwrap();
}

fn set_tkcc(deps: &mut Deps, admin: &Addr, token: &Addr, burn: Option<&Addr>) {
    let _ = admin; // 保留参数签名，避免改所有调用点
    // 生产路径走 AdminAction::SetTkccToken（内含 assert_is_prc20 探测）；
    // MockQuerier 下 wasm 查询必然失败 → TkccProbeFailed。测试直接写 storage，
    // 绕过探测（decimals 固定 6，与被测 TKCC 一致）。
    paxi_common::set_tkcc_token(deps.as_mut().storage, token).unwrap();
    crate::state::TKCC_DECIMALS.save(deps.as_mut().storage, &6u8).unwrap();
    if let Some(b) = burn {
        paxi_common::set_tkcc_burn_address(deps.as_mut().storage, b).unwrap();
    }
}

fn fund(deps: &mut Deps, user: &Addr, token: &Addr, amount: Uint128) {
    BalanceLedger::credit(deps.as_mut().storage, user, token, amount).unwrap();
}

fn native() -> Addr {
    paxi_common::native_key()
}

fn fund_user(deps: &mut Deps, user: &Addr, tkcc: &Addr) {
    fund(deps, user, &native(), Uint128::new(1_000_000_000));
    fund(deps, user, tkcc, Uint128::new(1_000_000_000_000));
}

/// 建池（simple 版）：建池者 = 交易发送方。
/// 返回 lottery_id。
fn create_lottery(deps: &mut Deps, env: &Env, creator: &Addr, tier: u8) -> u64 {
    let info = cosmwasm_std::MessageInfo { sender: creator.clone(), funds: vec![] };
    let resp = execute(
        deps.as_mut(),
        env.clone(),
        info,
        ExecuteMsg::CreateLottery { tier, commit_hash: None },
    )
    .unwrap();
    resp.attributes
        .iter()
        .find(|a| a.key == "lottery_id")
        .unwrap()
        .value
        .parse()
        .unwrap()
}

/// 参与：参与者 = 交易发送方，金额一律从链上池子快照读取。
fn join_lottery(deps: &mut Deps, env: &Env, user: &Addr, id: u64) {
    let info = cosmwasm_std::MessageInfo { sender: user.clone(), funds: vec![] };
    execute(
        deps.as_mut(),
        env.clone(),
        info,
        ExecuteMsg::JoinLottery { id },
    )
    .unwrap();
}

/// 激活模板：参与者 = 交易发送方。
fn activate_template(deps: &mut Deps, env: &Env, user: &Addr, tid: u64) {
    let info = cosmwasm_std::MessageInfo { sender: user.clone(), funds: vec![] };
    execute(
        deps.as_mut(),
        env.clone(),
        info,
        ExecuteMsg::ActivateTemplate { template_id: tid },
    )
    .unwrap();
}

fn create_template(
    deps: &mut Deps,
    admin: &Addr,
    name: &str,
    tier: u8,
    commit_hash: Option<&str>,
) -> u64 {
    let info = cosmwasm_std::MessageInfo { sender: admin.clone(), funds: vec![] };
    let resp = execute(
        deps.as_mut(),
        mock_env(),
        info,
        ExecuteMsg::CreatePoolTemplate {
            name: name.to_string(),
            tier,
            commit_hash: commit_hash.map(|s| s.to_string()),
        },
    )
    .unwrap();
    resp.attributes
        .iter()
        .find(|a| a.key == "template_id")
        .unwrap()
        .value
        .parse()
        .unwrap()
}

fn reveal_template(deps: &mut Deps, admin: &Addr, tid: u64, secret: &str) -> Result<(), crate::error::ContractError> {
    let info = cosmwasm_std::MessageInfo { sender: admin.clone(), funds: vec![] };
    execute(
        deps.as_mut(),
        mock_env(),
        info,
        ExecuteMsg::AdminCustom(LotteryAdminAction::RevealTemplateSecret {
            template_id: tid,
            secret: secret.to_string(),
        }),
    )
    .map(|_| ())
}

fn draw(deps: &mut Deps, env: &Env, sender: &Addr, id: u64) -> Result<(), crate::error::ContractError> {
    let info = cosmwasm_std::MessageInfo { sender: sender.clone(), funds: vec![] };
    execute(deps.as_mut(), env.clone(), info, ExecuteMsg::DrawLottery { id }).map(|_| ())
}

fn sha256_hex(s: &str) -> String {
    use sha2::{Digest as _, Sha256};
    hex::encode(Sha256::digest(s.as_bytes()))
}

// ========================================================================
// 用例
// ========================================================================

#[test]
fn tier_specs_are_correct() {
    let small = tier_spec(TIER_SMALL).unwrap();
    assert_eq!(small.max_people, 5);
    assert_eq!(small.join_paxi, Uint128::new(1_000_000));
    assert_eq!(small.join_tkcc, Uint128::new(10_000));
    assert_eq!(small.create_fee_paxi, Uint128::new(1_000_000));
    assert_eq!(small.create_fee_tkcc, Uint128::new(20_000));

    let medium = tier_spec(TIER_MEDIUM).unwrap();
    assert_eq!(medium.max_people, 20);
    assert_eq!(medium.join_paxi, Uint128::new(2_000_000));
    assert_eq!(medium.join_tkcc, Uint128::new(20_000));
    assert_eq!(medium.create_fee_paxi, Uint128::new(15_000_000));
    assert_eq!(medium.create_fee_tkcc, Uint128::new(100_000));

    let large = tier_spec(TIER_LARGE).unwrap();
    assert_eq!(large.max_people, 50);
    assert_eq!(large.join_paxi, Uint128::new(10_000_000));
    assert_eq!(large.join_tkcc, Uint128::new(100_000));
    assert_eq!(large.create_fee_paxi, Uint128::new(60_000_000));
    assert_eq!(large.create_fee_tkcc, Uint128::new(600_000));

    // v2 新增档位
    let small_high = tier_spec(TIER_SMALL_HIGH).unwrap();
    assert_eq!(small_high.max_people, 5);
    assert_eq!(small_high.join_paxi, Uint128::new(10_000_000));
    assert_eq!(small_high.join_tkcc, Uint128::new(100_000));
    assert_eq!(small_high.create_fee_paxi, Uint128::new(15_000_000));
    assert_eq!(small_high.create_fee_tkcc, Uint128::new(120_000));

    let large_paxi = tier_spec(TIER_LARGE_PAXI).unwrap();
    assert_eq!(large_paxi.max_people, 50);
    assert_eq!(large_paxi.join_paxi, Uint128::new(10_000_000));
    assert!(large_paxi.join_tkcc.is_zero(), "50 人纯 PAXI 档参与费不应含 TKCC");
    assert_eq!(large_paxi.create_fee_paxi, Uint128::new(75_000_000));
    assert!(large_paxi.create_fee_tkcc.is_zero(), "50 人纯 PAXI 档建池费不应含 TKCC");

    // 原三档数值回归：老实例 migrate 后老池子按原值结算
    assert_eq!(tier_spec(TIER_SMALL).unwrap().join_paxi, Uint128::new(1_000_000));
    assert_eq!(tier_spec(TIER_MEDIUM).unwrap().create_fee_paxi, Uint128::new(15_000_000));
    assert_eq!(tier_spec(TIER_LARGE).unwrap().create_fee_tkcc, Uint128::new(600_000));

    assert!(tier_spec(5).is_none());
    assert!(tier_spec(255).is_none());
}

#[test]
fn create_fails_when_tkcc_unconfigured() {
    let mut deps = mock_deps();
    let admin = addr(&deps, "admin");
    let creator = addr(&deps, "creator");
    let tkcc = addr(&deps, "tkcc");
    init(&mut deps, vec![admin.to_string()], admin.to_string());

    // 给足余额：下面要用**同一条消息**重试一次成功建池，
    // 否则"重试失败"可能只是余额不足，掩盖断言。
    fund_user(&mut deps, &creator, &tkcc);
    let env = mock_env();
    let info = cosmwasm_std::MessageInfo { sender: creator.clone(), funds: vec![] };
    let err = execute(
        deps.as_mut(),
        env.clone(),
        info,
        ExecuteMsg::CreateLottery {
            tier: TIER_SMALL,
            commit_hash: None,
        },
    )
    .unwrap_err();
    assert!(matches!(
        err,
        crate::error::ContractError::Tkcc(paxi_common::TkccError::TkccNotConfigured)
    ));

    // 【失败不留状态】`require_tkcc` 是纯读校验，失败时不应消耗任何配额 /
    // 推进任何序列。配置好 TKCC 后用**完全相同的消息**重试：
    // 若失败路径写脏了状态（例如提前自增了 LOTTERY_SEQ），这里会拿到 id != 1。
    set_tkcc(&mut deps, &admin, &tkcc, None);
    let info2 = cosmwasm_std::MessageInfo { sender: creator.clone(), funds: vec![] };
    execute(
        deps.as_mut(),
        env.clone(),
        info2,
        ExecuteMsg::CreateLottery {
            tier: TIER_SMALL,
            commit_hash: None,
        },
    )
    .unwrap();
    let l = LOTTERIES.load(deps.as_ref().storage, 1).unwrap();
    assert_eq!(l.status, LotteryStatus::Open);
    // 🟢 建池者 = 发送方（链上原生鉴权，无会话密钥）
    assert_eq!(l.creator, creator);
}

#[test]
fn invalid_tier_rejected() {
    let mut deps = mock_deps();
    let admin = addr(&deps, "admin");
    let creator = addr(&deps, "creator");
    let tkcc = addr(&deps, "tkcc");
    init(&mut deps, vec![admin.to_string()], admin.to_string());
    set_tkcc(&mut deps, &admin, &tkcc, None);
    fund_user(&mut deps, &creator, &tkcc);

    let env = mock_env();
    let info = cosmwasm_std::MessageInfo { sender: creator.clone(), funds: vec![] };
    let err = execute(
        deps.as_mut(),
        env.clone(),
        info,
        ExecuteMsg::CreateLottery {
            tier: 5,
            commit_hash: None,
        },
    )
    .unwrap_err();
    match err {
        crate::error::ContractError::InvalidTier(t) => assert_eq!(t, 5),
        other => panic!("期望 InvalidTier，实得 {:?}", other),
    }
}

#[test]
fn small_tier_create_charges_and_snapshots() {
    let mut deps = mock_deps();
    let admin = addr(&deps, "admin");
    let creator = addr(&deps, "creator");
    let tkcc = addr(&deps, "tkcc");
    init(&mut deps, vec![admin.to_string()], admin.to_string());
    set_tkcc(&mut deps, &admin, &tkcc, None);
    fund_user(&mut deps, &creator, &tkcc);

    let env = mock_env();
    let id = create_lottery(&mut deps, &env, &creator, TIER_SMALL);

    let l = LOTTERIES.load(deps.as_ref().storage, id).unwrap();
    // 🟢 建池者 = 交易发送方
    assert_eq!(l.creator, creator);
    // 建池费 1 PAXI + 2 万 TKCC 进奖池
    assert_eq!(l.pool_paxi, Uint128::new(1_000_000));
    assert_eq!(l.pool_tkcc, Uint128::new(20_000_000_000));
    // 参与费快照来自 tier：1 PAXI + 1 万 TKCC
    assert_eq!(l.join_paxi, Uint128::new(1_000_000));
    assert_eq!(l.join_tkcc, Uint128::new(10_000_000_000));
    assert_eq!(l.max_people, 5);
    assert_eq!(l.tier, TIER_SMALL);
    assert!(!l.is_template_pool);
    // 建池者余额被扣：PAXI 剩 999_000_000，TKCC 剩 1e12 - 2e10
    assert_eq!(
        BalanceLedger::balance_of(deps.as_ref().storage, &creator, &native()),
        Uint128::new(999_000_000)
    );
    assert_eq!(
        BalanceLedger::balance_of(deps.as_ref().storage, &creator, &tkcc),
        Uint128::new(980_000_000_000)
    );
}

/// P1-2：建池者不能参与自己建的池。
#[test]
fn creator_cannot_join_own_lottery() {
    let mut deps = mock_deps();
    let admin = addr(&deps, "admin");
    let tkcc = addr(&deps, "tkcc");
    init(&mut deps, vec![admin.to_string()], admin.to_string());
    set_tkcc(&mut deps, &admin, &tkcc, None);

    let creator = addr(&deps, "creator");
    fund_user(&mut deps, &creator, &tkcc);

    let env = mock_env();
    let id = create_lottery(&mut deps, &env, &creator, TIER_SMALL);

    // 🟢 发送方即参与者：建池者用**自己的**地址调 JoinLottery
    let info = cosmwasm_std::MessageInfo { sender: creator.clone(), funds: vec![] };
    let err = execute(
        deps.as_mut(),
        env.clone(),
        info,
        ExecuteMsg::JoinLottery { id },
    )
    .unwrap_err();
    match err {
        crate::error::ContractError::CreatorCannotJoin(got) => assert_eq!(got, id),
        other => panic!("期望 CreatorCannotJoin，实得 {:?}", other),
    }

    let l = LOTTERIES.load(deps.as_ref().storage, id).unwrap();
    assert_eq!(l.participant_count, 0);
    assert!(matches!(l.status, LotteryStatus::Open));
}

#[test]
fn full_flow_small_tier_draw_claim() {
    let mut deps = mock_deps();
    let admin = addr(&deps, "admin");
    let tkcc = addr(&deps, "tkcc");
    let blackhole = addr(&deps, "blackhole");
    init(&mut deps, vec![admin.to_string()], admin.to_string());
    set_tkcc(&mut deps, &admin, &tkcc, Some(&blackhole));

    // 1 位建池者 + 5 位参与者（5 人档满员 = 5）
    let creator = addr(&deps, "creator");
    fund_user(&mut deps, &creator, &tkcc);

    let users: Vec<Addr> = (0..5).map(|i| addr(&deps, &format!("p{}", i))).collect();
    for u in users.iter() {
        fund_user(&mut deps, u, &tkcc);
    }

    let env = mock_env();
    let id = create_lottery(&mut deps, &env, &creator, TIER_SMALL);
    for i in 0..5 {
        join_lottery(&mut deps, &env, &users[i], id);
    }

    let l = LOTTERIES.load(deps.as_ref().storage, id).unwrap();
    assert!(matches!(l.status, LotteryStatus::Full));
    assert_eq!(l.participant_count, 5);
    // 奖池 = 建池费 + 5 × 参与费
    assert_eq!(l.pool_paxi, Uint128::new(6_000_000)); // 1 + 5×1 PAXI
    assert_eq!(l.pool_tkcc, Uint128::new(70_000_000_000)); // 2万 + 5×1万 TKCC
    // 🟢 扣款落在**发送方本人**头上：每人被扣 1 PAXI + 1 万 TKCC
    assert_eq!(
        BalanceLedger::balance_of(deps.as_ref().storage, &users[0], &native()),
        Uint128::new(1_000_000_000 - 1_000_000)
    );
    assert_eq!(
        BalanceLedger::balance_of(deps.as_ref().storage, &users[0], &tkcc),
        Uint128::new(1_000_000_000_000 - 10_000_000_000)
    );

    // 未满员前想提前开奖的池子不存在 —— 这里直接开奖
    draw(&mut deps, &env, &admin, id).unwrap();

    let l = LOTTERIES.load(deps.as_ref().storage, id).unwrap();
    assert!(matches!(l.status, LotteryStatus::Drawn));
    let winners = l.winners.clone().unwrap();
    let payout = l.payout.clone().unwrap();
    assert_eq!(winners.first.len(), 1);
    assert_eq!(winners.second.len(), 2);
    // 一等奖与二等奖不重复
    for w in &winners.second {
        assert_ne!(w, &winners.first[0]);
    }

    // 分配比例（TKCC 38/28/14/14/6，PAXI 40/30/15/15）+ 总额守恒
    let pool_tkcc = l.settled_pool_tkcc.u128();
    let pool_paxi = l.settled_pool_paxi.u128();
    let tkcc_sum = payout.first_tkcc.u128()
        + payout.second_each_tkcc.u128() * 2
        + payout.creator_tkcc.u128()
        + payout.ops_tkcc.u128()
        + payout.burn_tkcc.u128();
    assert_eq!(tkcc_sum, pool_tkcc, "TKCC 分配必须总额守恒");
    let paxi_sum = payout.first_paxi.u128()
        + payout.second_each_paxi.u128() * 2
        + payout.creator_paxi.u128()
        + payout.ops_paxi.u128();
    assert_eq!(paxi_sum, pool_paxi, "PAXI 分配必须总额守恒");

    // 领取：中奖者进内部余额
    let first = winners.first[0].clone();
    let bal_before = BalanceLedger::balance_of(deps.as_ref().storage, &first, &tkcc);
    let info = cosmwasm_std::MessageInfo { sender: first.clone(), funds: vec![] };
    execute(deps.as_mut(), mock_env(), info, ExecuteMsg::Claim { id }).unwrap();
    let bal_after = BalanceLedger::balance_of(deps.as_ref().storage, &first, &tkcc);
    assert!(bal_after > bal_before);
}

// ========================================================================
// v2 新增档位测试
// ========================================================================

/// 5 人高档（tier 3）：建池费 15 PAXI + 12 万 TKCC，参与费 10 PAXI + 10 万 TKCC。
#[test]
fn small_high_tier_create_charges_and_snapshots() {
    let mut deps = mock_deps();
    let admin = addr(&deps, "admin");
    let creator = addr(&deps, "creator");
    let tkcc = addr(&deps, "tkcc");
    init(&mut deps, vec![admin.to_string()], admin.to_string());
    set_tkcc(&mut deps, &admin, &tkcc, None);
    fund_user(&mut deps, &creator, &tkcc);

    let env = mock_env();
    let id = create_lottery(&mut deps, &env, &creator, TIER_SMALL_HIGH);

    let l = LOTTERIES.load(deps.as_ref().storage, id).unwrap();
    // 🟢 建池者 = 交易发送方
    assert_eq!(l.creator, creator);
    // 建池费 15 PAXI + 12 万 TKCC 进奖池
    assert_eq!(l.pool_paxi, Uint128::new(15_000_000));
    assert_eq!(l.pool_tkcc, Uint128::new(120_000_000_000));
    // 参与费快照：10 PAXI + 10 万 TKCC，满员 5 人
    assert_eq!(l.join_paxi, Uint128::new(10_000_000));
    assert_eq!(l.join_tkcc, Uint128::new(100_000_000_000));
    assert_eq!(l.max_people, 5);
    assert_eq!(l.tier, TIER_SMALL_HIGH);
    // 建池者余额被扣：PAXI 剩 1e9 - 15e6，TKCC 剩 1e12 - 1.2e11
    assert_eq!(
        BalanceLedger::balance_of(deps.as_ref().storage, &creator, &native()),
        Uint128::new(985_000_000)
    );
    assert_eq!(
        BalanceLedger::balance_of(deps.as_ref().storage, &creator, &tkcc),
        Uint128::new(880_000_000_000)
    );
}

/// 50 人纯 PAXI 档（tier 4）：全链路 0 TKCC —— 建池 / 参与 / 开奖 / 领奖。
/// 重点验证 is_zero() 守卫路径：不发生任何 TKCC 划转，销毁消息不产生。
#[test]
fn full_flow_large_paxi_tier_zero_tkcc() {
    let mut deps = mock_deps();
    let admin = addr(&deps, "admin");
    let tkcc = addr(&deps, "tkcc");
    let blackhole = addr(&deps, "blackhole");
    init(&mut deps, vec![admin.to_string()], admin.to_string());
    set_tkcc(&mut deps, &admin, &tkcc, Some(&blackhole));

    let creator = addr(&deps, "creator");
    fund_user(&mut deps, &creator, &tkcc);

    // 50 位参与者（50 人档满员 = 50）
    let users: Vec<Addr> = (0..50).map(|i| addr(&deps, &format!("px{}", i))).collect();
    for u in users.iter() {
        fund_user(&mut deps, u, &tkcc);
    }
    let creator_tkcc_before = BalanceLedger::balance_of(deps.as_ref().storage, &creator, &tkcc);

    let env = mock_env();
    let id = create_lottery(&mut deps, &env, &creator, TIER_LARGE_PAXI);

    let l = LOTTERIES.load(deps.as_ref().storage, id).unwrap();
    // 建池费只有 PAXI：75 PAXI，TKCC 一分不动
    assert_eq!(l.pool_paxi, Uint128::new(75_000_000));
    assert!(l.pool_tkcc.is_zero());
    assert_eq!(l.join_paxi, Uint128::new(10_000_000));
    assert!(l.join_tkcc.is_zero());
    assert_eq!(
        BalanceLedger::balance_of(deps.as_ref().storage, &creator, &tkcc),
        creator_tkcc_before,
        "建池不应划转任何 TKCC"
    );

    for i in 0..50 {
        join_lottery(&mut deps, &env, &users[i], id);
    }

    let l = LOTTERIES.load(deps.as_ref().storage, id).unwrap();
    assert!(matches!(l.status, LotteryStatus::Full));
    assert_eq!(l.participant_count, 50);
    // 奖池 = 75 + 50×10 PAXI，TKCC 恒为 0
    assert_eq!(l.pool_paxi, Uint128::new(575_000_000));
    assert!(l.pool_tkcc.is_zero());
    // 参与者的 TKCC 一分未动
    assert_eq!(
        BalanceLedger::balance_of(deps.as_ref().storage, &users[0], &tkcc),
        creator_tkcc_before,
        "参与不应划转任何 TKCC"
    );

    draw(&mut deps, &env, &admin, id).unwrap();

    let l = LOTTERIES.load(deps.as_ref().storage, id).unwrap();
    assert!(matches!(l.status, LotteryStatus::Drawn));
    let winners = l.winners.clone().unwrap();
    let payout = l.payout.clone().unwrap();
    assert_eq!(winners.first.len(), 1);
    assert_eq!(winners.second.len(), 2);

    // TKCC 侧全为 0（守卫路径：无销毁消息、无 TKCC 转账）
    assert!(payout.first_tkcc.is_zero());
    assert!(payout.second_each_tkcc.is_zero());
    assert!(payout.creator_tkcc.is_zero());
    assert!(payout.ops_tkcc.is_zero());
    assert!(payout.burn_tkcc.is_zero());

    // PAXI 分配总额守恒（40/30/15/15，此处恰好整除无余数）
    let pool_paxi = l.settled_pool_paxi.u128();
    let paxi_sum = payout.first_paxi.u128()
        + payout.second_each_paxi.u128() * 2
        + payout.creator_paxi.u128()
        + payout.ops_paxi.u128();
    assert_eq!(paxi_sum, pool_paxi, "PAXI 分配必须总额守恒");
    assert_eq!(payout.first_paxi, Uint128::new(230_000_000)); // 40% of 575

    // 领奖：一等奖得主 PAXI 进内部余额，TKCC 余额不变
    let first = winners.first[0].clone();
    let paxi_before = BalanceLedger::balance_of(deps.as_ref().storage, &first, &native());
    let tkcc_before = BalanceLedger::balance_of(deps.as_ref().storage, &first, &tkcc);
    let info = cosmwasm_std::MessageInfo { sender: first.clone(), funds: vec![] };
    execute(deps.as_mut(), mock_env(), info, ExecuteMsg::Claim { id }).unwrap();
    assert_eq!(
        BalanceLedger::balance_of(deps.as_ref().storage, &first, &native()),
        paxi_before.checked_add(payout.first_paxi).unwrap()
    );
    assert_eq!(
        BalanceLedger::balance_of(deps.as_ref().storage, &first, &tkcc),
        tkcc_before,
        "领奖不应产生 TKCC 变动"
    );
}

#[test]
fn refund_when_timeout_not_full() {
    let mut deps = mock_deps();
    let admin = addr(&deps, "admin");
    let tkcc = addr(&deps, "tkcc");
    init(&mut deps, vec![admin.to_string()], admin.to_string());
    set_tkcc(&mut deps, &admin, &tkcc, None);

    let creator = addr(&deps, "creator");
    fund_user(&mut deps, &creator, &tkcc);

    let user = addr(&deps, "u0");
    fund_user(&mut deps, &user, &tkcc);

    let env = mock_env();
    let id = create_lottery(&mut deps, &env, &creator, TIER_SMALL);
    join_lottery(&mut deps, &env, &user, id);

    // 未满员（1/5）未到期：不能开奖
    assert!(draw(&mut deps, &env, &admin, id).is_err());

    // 快进到过期：仍未满员 → 参与者退款（拿回参与费）
    let mut env2 = mock_env();
    env2.block.time = env.block.time.plus_seconds(25 * 3600);
    let info = cosmwasm_std::MessageInfo { sender: user.clone(), funds: vec![] };
    execute(deps.as_mut(), env2.clone(), info, ExecuteMsg::Refund { id }).unwrap();

    // user 拿回参与费：PAXI 全额 1e9
    assert_eq!(
        BalanceLedger::balance_of(deps.as_ref().storage, &user, &native()),
        Uint128::new(1_000_000_000)
    );
    // 建池者需**自己**调一次 Refund 才能拿回建池费
    let info = cosmwasm_std::MessageInfo { sender: creator.clone(), funds: vec![] };
    execute(deps.as_mut(), env2.clone(), info, ExecuteMsg::Refund { id }).unwrap();
    assert_eq!(
        BalanceLedger::balance_of(deps.as_ref().storage, &creator, &tkcc),
        Uint128::new(1_000_000_000_000)
    );
    // 过期未满员也不能开奖（simple 版没有 min_people 分支）
    assert!(draw(&mut deps, &env2, &admin, id).is_err());
}

/// P2-1：模板池平台托管 secret 全流程。
/// * 无承诺模板 → 池子满员也无法开奖（TemplateCommitRequired）
/// * 有承诺模板 → reveal 错值被拒；reveal 后开奖成功（CommitReveal）
/// * 哈希链第二轮：揭示 s2（s1 = sha256(s2)）仍能通过
#[test]
fn template_pool_commit_reveal_flow() {
    // ⚠️ 本测试依赖 mock_deps() 的**全新 storage**：LOTTERY_SEQ 未初始化
    // （next_id 的 may_load().unwrap_or_default() = 0，首个 id = 0 + 1 = **1**），
    // 池 id 严格按激活顺序自增 → 下方 pid_plain=1 / pid=2 / pid3=3 是确定的。
    // 若未来在 arrange 阶段插入任何会创建池子的操作，这些硬编码必须同步更新
    // （更稳的做法是从 activate_template 响应 attributes 里取 lottery_id）。
    let mut deps = mock_deps();
    let admin = addr(&deps, "admin");
    let tkcc = addr(&deps, "tkcc");
    let blackhole = addr(&deps, "blackhole");
    init(&mut deps, vec![admin.to_string()], admin.to_string());
    set_tkcc(&mut deps, &admin, &tkcc, Some(&blackhole));

    let users: Vec<Addr> = (0..5).map(|i| addr(&deps, &format!("tu{}", i))).collect();
    for u in users.iter() {
        fund_user(&mut deps, u, &tkcc);
    }
    let env = mock_env();

    // ---- 1) 无承诺模板：满员也禁开奖 ----
    let tid_plain = create_template(&mut deps, &admin, "无承诺", TIER_SMALL, None);
    for i in 0..5 {
        activate_template(&mut deps, &env, &users[i], tid_plain);
    }
    let pid_plain = 1u64;
    let l = LOTTERIES.load(deps.as_ref().storage, pid_plain).unwrap();
    assert!(l.is_template_pool);
    assert_eq!(l.commit_hash, None);
    let err = draw(&mut deps, &env, &admin, pid_plain).unwrap_err();
    assert!(matches!(err, crate::error::ContractError::TemplateCommitRequired(1)));

    // ---- 2) 有承诺模板：错值拒、对值过、开奖带 CommitReveal ----
    // 链：s1 = sha256(s2)；创建时提交 c0 = sha256(s1)
    let s2 = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"; // 64 hex
    let s1 = sha256_hex(s2); // = hex(sha256(s2_bytes))
    let c0 = sha256_hex(&s1); // hex(sha256(s1_bytes))

    let tid = create_template(&mut deps, &admin, "官方池", TIER_SMALL, Some(&c0));
    // 同一批用户第二次激活：此时 P1 已满，会另开新池（而非复用 P1）
    for i in 0..5 {
        activate_template(&mut deps, &env, &users[i], tid);
    }
    let pid = 2u64;
    let l = LOTTERIES.load(deps.as_ref().storage, pid).unwrap();
    // 池子快照的是"创建那一刻"模板的承诺 —— 它是开奖时查 TEMPLATE_SECRETS 的 key
    assert_eq!(l.commit_hash.as_deref(), Some(c0.as_str()));
    assert_eq!(l.revealed, None);
    assert_eq!(l.max_people, 5);

    // 未揭示不能开奖（模板池查 TEMPLATE_SECRETS 落空 → CommitNotRevealed）
    let err = draw(&mut deps, &env, &admin, pid).unwrap_err();
    assert!(matches!(err, crate::error::ContractError::CommitNotRevealed(2)));

    // 错值揭示被拒
    assert!(reveal_template(&mut deps, &admin, tid, &"b".repeat(64)).is_err());

    // 正确揭示 s1（验证 sha256(s1) == c0）
    reveal_template(&mut deps, &admin, tid, &s1).unwrap();
    draw(&mut deps, &env, &admin, pid).unwrap();

    let l = LOTTERIES.load(deps.as_ref().storage, pid).unwrap();
    assert!(matches!(l.status, LotteryStatus::Drawn));
    assert_eq!(
        l.random_source.as_ref().map(|r| format!("{:?}", r)).as_deref(),
        Some("CommitReveal")
    );
    // 模板池：建池者分成归运营
    let pt = l.payout.clone().unwrap();
    assert!(pt.creator_tkcc.is_zero() && pt.creator_paxi.is_zero());
    assert!(!pt.ops_tkcc.is_zero() && !pt.ops_paxi.is_zero());

    // ---- 3) 多池并行：新池用新承诺，必须等下一轮揭示 ----
    // 揭示 s1 后模板承诺推进为 s1；用户再激活 → 开新池 P3，快照 commit = s1。
    // P2 已开奖、指针在 draw 时清理，不影响 P3。
    // 注意必须**先填满 P3**再尝试 draw：开奖的满员检查在随机源分支之前，
    // 不满员的池会先报 LotteryNotFull，根本走不到查表那一步。
    activate_template(&mut deps, &env, &users[0], tid);
    for i in 1..5 {
        activate_template(&mut deps, &env, &users[i], tid);
    }
    let pid3 = 3u64;
    let l3 = LOTTERIES.load(deps.as_ref().storage, pid3).unwrap();
    assert_eq!(l3.commit_hash.as_deref(), Some(s1.as_str()));
    assert_eq!(l3.participant_count, 5);
    // P3 满员，但其承诺（s1）还没被揭示 → 不能开奖
    let err = draw(&mut deps, &env, &admin, pid3).unwrap_err();
    assert!(matches!(err, crate::error::ContractError::CommitNotRevealed(3)));
    // 揭示 s2（sha256(s2) == s1 == P3 的承诺快照）→ P3 可开奖
    reveal_template(&mut deps, &admin, tid, s2).unwrap();
    draw(&mut deps, &env, &admin, pid3).unwrap();
    let l3 = LOTTERIES.load(deps.as_ref().storage, pid3).unwrap();
    assert!(matches!(l3.status, LotteryStatus::Drawn));

    // 哈希链第二轮轮换由 template_pool_hash_chain_rotation 独立覆盖
}
#[test]
fn template_pool_hash_chain_rotation() {
    let mut deps = mock_deps();
    let admin = addr(&deps, "admin");
    let tkcc = addr(&deps, "tkcc");
    init(&mut deps, vec![admin.to_string()], admin.to_string());
    set_tkcc(&mut deps, &admin, &tkcc, None);

    // 链：s1 = sha256(s2)，s2 = sha256(s3)；c0 = sha256(s1)
    let s3 = "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";
    let s2 = sha256_hex(s3);
    let s1 = sha256_hex(&s2);
    let c0 = sha256_hex(&s1);

    let tid = create_template(&mut deps, &admin, "链轮换", TIER_SMALL, Some(&c0));

    // 第一轮：揭示 s1 → 通过，新承诺 = s1
    reveal_template(&mut deps, &admin, tid, &s1).unwrap();
    // 复用同一值 → CommitMismatch（承诺已推进）
    assert!(reveal_template(&mut deps, &admin, tid, &s1).is_err());
    // 第二轮：揭示 s2（sha256(s2) == s1 == 当前承诺）→ 通过，新承诺 = s2
    reveal_template(&mut deps, &admin, tid, &s2).unwrap();
    // 第三轮：揭示 s3 → 通过
    reveal_template(&mut deps, &admin, tid, s3).unwrap();
    // 链走完后再揭示任意值 → 失败
    assert!(reveal_template(&mut deps, &admin, tid, &"d".repeat(64)).is_err());

    // 非 64-hex 的 secret 被拒（哈希链要求）
    let tid2 = create_template(&mut deps, &admin, "非hex", TIER_SMALL, Some(&sha256_hex("plain-secret-1")));
    assert!(reveal_template(&mut deps, &admin, tid2, "plain-secret-1").is_err());

    // 非管理员不能揭示
    let user = addr(&deps, "noadmin");
    assert!(reveal_template(&mut deps, &user, tid, &s3).is_err());

    // 查询入口可用
    let _raw = query(
        deps.as_ref(),
        mock_env(),
        QueryMsg::LotteryConfig {},
    )
    .unwrap();
}
