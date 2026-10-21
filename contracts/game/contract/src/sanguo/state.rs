// 本轮改动（C1 + C2 + F + G）：DIFF_BOOST_BP 与 ai_reward 下调、全局日赔付上限默认值下调
//   （F 修正 ai_reward 注释单位；G 将默认上限由 1e13 提到 1e14）。
use cosmwasm_schema::cw_serde;
// 🟢 AI经济修复 ④：全局日赔付上限函数需要这些类型
use cosmwasm_std::{Addr, Storage, Uint128, Deps, DepsMut, StdResult};
// V2：DAILY_PURCHASES 的值类型（每玩家每日一条，防存储膨胀，裁定 7.7）
use std::collections::BTreeMap;
use crate::error::ContractError;
use cw_storage_plus::{Item, Map};

// ============================================================================
// 三国卡牌子模块 —— 状态与存储
//
// 设计要点（复用新通用合约的基础设施，不另起炉灶）：
//   1. 玩家身份：取交易发送方 info.sender（本模板不含会话密钥 / 无感签名机制）。
//   2. 资产：付费入口（体力/皮肤/通行证/加购抽卡）走 PAXI 原生币或统一碎片
//      FRAGMENT_BALANCE；零 TKCC 红线已升级为代码层面（T24/C1~C6）。
//   3. 会计恒等式：付费从 info.funds 或碎片扣除 → 贷记 HOUSE_BANKROLL；
//      奖励只发不可转让碎片，绝不转出 PAXI/TKCC。
// ============================================================================

// ============================================================================
// V2 合规总闸硬化说明（2026-10-13，裁定书 T24 / C1~C6）：
//   1. COMPLIANCE_MODE 开关已删除（C1）——合规路径是唯一路径，恒真；
//      is_compliance() / compliance_draw_limit() / compliance_fragment_cap()
//      三函数随之删除，逻辑内联到参数访问器。
//   2. SanguoConfig 的 tkcc_token / burn_address / tap_addresses 三字段废弃
//      （C3）：struct 保留防旧 JSON 反序列化失败，migrate 清空值，不再读取。
//   3. SanguoParams 中全部 TKCC/PAXI 计费字段标 deprecated（C4）：字段保留
//      仅为旧配置 JSON 可反序列化，exec 不再读取；管理员 SetParams 填 0 即可。
//   4. 提案/投票三件套（PROPOSALS / PROPOSAL_COUNTER / VOTES）与
//      PROPOSAL_DEPOSITS 删除（投票功能全禁；押金收敛到 PENDING_REFUNDS，M3）。
//   5. DAILY_DRAW_LIMIT / DAILY_FRAGMENT_CAP 两个 Item 收敛进 SanguoParams
//      （migrate 负责把旧 Item 值搬进 params 后删键，见 8.2 M2）。
// ============================================================================

// ===== 三国专属配置（管理员设置，部署时未定，故单独存） =====

#[cw_serde]
pub struct SanguoConfig {
    /// @deprecated C3（T24）：零 TKCC 红线。字段保留仅为旧 JSON 兼容
    /// （deny_unknown_fields），migrate 写 Addr::unchecked("") 清值，不再读取。
    pub tkcc_token: Addr,
    /// @deprecated C3：同上（销毁地址废弃）
    pub burn_address: Addr,
    /// @deprecated C3：同上（生态抽水地址列表废弃，migrate 清空为 vec![]）
    pub tap_addresses: Vec<Addr>,
}

#[cw_serde]
#[derive(Default)]
// 🟢 全盘审计修复（升级安全）：加 serde(default)，旧版 JSON 缺字段时取默认值（0），
//    避免 may_load 报错导致 load_params 全链路失败。⚠️ 语义必需字段（如 *_prc_total）
//    若为 0 会导致资金分配异常，故 migrate 中仍会做一次完整性校验兜底。
#[serde(default)]
pub struct SanguoParams {
    // ==================== 生效参数（V2 合规模型：零 TKCC）====================
    // 裁定 6.2：全部经济/玩法数值收敛进本结构体，管理员 SanguoSetParams 可调，
    //   后期新增玩法变体只改参数、零 migrate。字段为 0（或全零数组）时，
    //   访问器回退对应 DEFAULT_* / 现有 const 值——旧配置 JSON 缺字段
    //   （serde default 填 0）时行为与 V1 一致。

    // ---- 抽卡（免费抽卡 + 碎片加购；PAXI/TKCC 抽卡费已废弃见下方 deprecated）----
    /// 每日免费抽卡张数上限（0 = 回退 DEFAULT_DAILY_DRAW_LIMIT=3；pack3 按张计数）
    pub daily_draw_limit: u64,
    /// 每日碎片加购抽卡张数上限（0 = 回退 DEFAULT_DAILY_DRAW_PURCHASE_CAP=5）
    pub daily_draw_purchase_cap: u32,
    /// 保底阈值（0 = 无保底；语义值，不回退）
    pub pity_threshold: u32,

    // ---- 碎片（统一币 FRAGMENT_BALANCE，T7）----
    /// 每玩家每日碎片获取硬顶（0 = 回退 DEFAULT_DAILY_FRAGMENT_CAP=200）
    pub daily_fragment_cap: u64,

    // ---- 体力 ----
    /// 体力上限（0 = 回退 STAMINA_MAX=120）
    pub stamina_max: u64,
    /// 每 N 秒恢复 1 点体力（0 = 回退 STAMINA_REGEN_SECS=300）
    pub stamina_regen_secs: u64,
    /// 每次购买体力包获得点数（0 = 回退 STAMINA_PACK_AMOUNT=30）
    pub stamina_pack_amount: u64,
    /// 体力包单价 upaxi（0 = 回退 STAMINA_PACK_PRICE=2 PAXI）
    pub stamina_pack_price: u128,

    // ---- 持有 ----
    /// 玩家持有卡牌上限（0 = 回退 MAX_PLAYER_CARDS=512）
    pub max_player_cards: u32,
    /// 卡池模板总数上限（现役字段，管理员直填）
    pub max_cards: u32,
    /// 卡池每稀有度数量上限 [common, rare, epic, legend]（全 0 = 回退 RARITY_MAX）
    pub rarity_max: [u64; 4],

    // ---- 合成 / 分解 / 升星（碎片计价）----
    /// 合成消耗碎片（0 = 回退 CRAFT_COST）
    pub craft_cost: [u64; 4],
    /// 分解同名卡返还碎片（0 = 回退 FRAGMENT_FROM_DUPLICATE）
    pub fragment_from_duplicate: [u64; 4],
    /// 升星消耗碎片（0 = 回退 STAR_UP_FRAGMENT_COST）
    pub star_up_fragment_cost: [u64; 4],
    /// 升星攻击加成（直写卡牌 attack；0 = 回退 DEFAULT_UPGRADE_ATK_BOOST）
    pub upgrade_atk_boost: [u32; 4],
    /// 升星防御加成（直写卡牌 defense；0 = 回退 DEFAULT_UPGRADE_DEF_BOOST）
    pub upgrade_def_boost: [u32; 4],
    /// 普通升级每级攻击增益（0 = 回退 UPGRADE_FRAG_ATK=3；6.2 末注推导项）
    pub upgrade_frag_atk: u32,
    /// 普通升级每级防御增益（0 = 回退 UPGRADE_FRAG_DEF=2；6.2 末注推导项）
    pub upgrade_frag_def: u32,
    /// 普通升级等级上限（0 = 回退 UPGRADE_FRAG_MAX_LEVEL=10；6.2 末注推导项）
    pub upgrade_frag_max_level: u8,

    // ---- AI 难度（ Casual / Ranked ）----
    /// AI 战力加成 bp（0 = 回退 DIFF_BOOST_BP）
    pub diff_boost_bp: [u64; 5],
    /// AI 战力地板 bp（0 = 回退 AI_FLOOR_BP）
    pub ai_floor_bp: [u64; 5],
    /// AI 对战每日限次（0 = 回退 DEFAULT_DAILY_AI_LIMIT=20；即 6.2 表 ai_daily_limit）
    pub daily_ai_limit: u64,

    // ---- 天梯排位 ----
    /// 各档 AI 战力加成 bp（0 = 回退 RANK_BOOST_BP）
    pub rank_boost_bp: [u64; 5],
    /// 天梯胜场加分（0 = 回退 RANK_WIN_POINTS=20）
    pub rank_win_points: i64,
    /// 天梯负场扣分（0 = 回退 RANK_LOSE_POINTS=10）
    pub rank_lose_points: i64,
    /// 升档所需分数（0 = 回退 RANK_TIER_UP_POINTS=100）
    pub rank_tier_up_points: i64,
    /// 天梯入场体力消耗（0 = 回退 RANK_STAMINA_COST=10）
    pub rank_stamina_cost: u64,
    /// 天梯胜场赛季分（0 = 回退 RANK_WIN_SEASON_POINTS=10）
    pub rank_win_season_points: u64,
    /// 各档胜利碎片奖励（0 = 回退 RANK_REWARD）
    pub rank_rewards: [Fragments; 5],

    // ---- 每日任务 ----
    /// 每日任务数（0 = 回退 DAILY_TASK_COUNT=4）
    pub daily_task_count: u32,
    /// 各任务完成条件（0 = 回退 DAILY_TASK_REQUIRED）
    pub daily_task_required: [u64; 4],
    /// 各任务碎片奖励（0 = 回退 DAILY_TASK_REWARD）
    pub daily_task_reward: [Fragments; 4],
    /// 完成任务赛季分（0 = 回退 DAILY_TASK_SEASON_POINTS=10）
    pub daily_task_season_points: u64,

    // ---- 赠送冷却（9 项；gift.rs 全量改读访问器）----
    pub gift_holding_secs: u64,
    pub gift_legend_holding_secs: u64,
    pub gift_pair_age_secs: u64,
    pub gift_acct_age_secs: u64,
    pub gift_pair_idle_expire_secs: u64,
    pub gift_pair_refresh_interval: u64,
    pub gift_daily_sent: u32,
    pub gift_daily_recv: u32,
    pub gift_frag_cap: u64,

    // ---- 战斗奖励（合规：只发不可转让碎片；T7 四桶表 + to_unified 折算）----
    pub ai_frag_reward: [Fragments; 5],
    pub pvp_frag_reward: Fragments,
    pub royale_frag_reward: Fragments,

    // ---- 指定卡碎片价（T15：60 指定卡不逐条入商品表，按稀有度定价）----
    /// common/rare/epic/legend = 1500/600/250/80（0 = 回退 DEFAULT_CARD_FRAG_PRICE）
    pub card_frag_price: [u64; 4],

    // ---- 编队 ----
    /// 出战编队 Σcost 上限（0 = 回退 TEAM_COST_CAP=30；T12 validate_battle_team）
    pub team_cost_cap: u32,

    // ---- 战斗系数（12 项，裁定 6.2；0 = 回退 battle::compute DEFAULT_*）----
    /// 阵营克制加成（攻方克守方）
    pub faction_counter_bp: u64,
    /// 阵营被克减益（S2：默认 10000 无惩罚，运营可开）
    pub faction_countered_bp: u64,
    /// 阵营中立
    pub faction_neutral_bp: u64,
    /// 兵种克制加成
    pub troop_counter_bp: u64,
    /// 兵种被克减益
    pub troop_countered_bp: u64,
    /// 兵种中立
    pub troop_neutral_bp: u64,
    /// 传说稀有度系数（S5：仅传说享）
    pub rarity_legend_bp: u64,
    pub rarity_epic_bp: u64,
    pub rarity_rare_bp: u64,
    pub rarity_common_bp: u64,
    /// OnBattleStart 战力 flat Σ封顶（S6+）
    pub power_flat_cap_bp: u64,
    /// 攻防减益下限（E2：5000 = 不低于原值 50%）
    pub eff_lower_bound_bp: u64,
    /// 🟢 参数收敛（2026-10-14）：技能强度全局系数（bp，10000=100%）——
    ///    乘在全部 60 技能的 attack/defense/flat 效果上；0 = 回退默认。
    ///    各技能独立数值仍是编译期常量（battle::skills），管理员用此旋钮整体调强弱。
    #[serde(default)]
    pub skill_strength_bp: u64,
    /// 🟢 参数收敛：连携强度全局系数（bp，10000=100%）——乘在连携「加成部分」
    ///    （bond_mult−10000）上，只缩放加成不动中性基数；0 = 回退默认。
    #[serde(default)]
    pub bond_strength_bp: u64,

    // ==================== deprecated（C4：保留字段仅为旧配置 JSON 可反序列化）====================
    // ⚠️ 零 TKCC 红线（T24/C1~C6）：以下计费字段在合规模型下全部废弃——
    //    migrate 不清值、exec 不再读取；管理员 SanguoSetParams 时填 0 即可。
    //    保留原因：SanguoParams 是 JSON 反序列化目标，删字段会导致老配置解析失败。

    /// @deprecated C4：抽卡免费化，PAXI 抽卡费废弃
    pub single_paxi_fee: u128,
    /// @deprecated C4：同上（三连）
    pub pack3_paxi_fee: u128,
    /// @deprecated C4：TKCC 抽卡计费三件套
    pub single_prc_total: u128,
    pub single_prc_eco: u128,
    pub single_prc_burn: u128,
    pub pack3_prc_total: u128,
    pub pack3_prc_eco: u128,
    pub pack3_prc_burn: u128,
    /// @deprecated C4：TKCC AI 对战计价
    pub ai_fee: [u128; 5],
    pub ai_reward: [u128; 5],
    pub ai_burn_pct_bp: u128,
    /// @deprecated C4：TKCC 升星费（升星玩法保留，仅碎片路径）
    pub upgrade_fees: [u128; 4],
    /// @deprecated C4：TKCC PVP/混战计价
    pub pvp_fee: u128,
    pub royale_entry_fee: u128,
    pub royale_reward_pct_bp: u128,
    pub royale_burn_pct_bp: u128,
    /// @deprecated C4：投票功能全禁（押金退还走 PENDING_REFUNDS，M3）
    pub proposal_deposit: u128,
    pub voting_period: u64,
}

impl SanguoParams {
    /// 体力包单价（upaxi）：管理员未配置（0）时回退默认 2 PAXI。
    /// 2026-10-12：由硬编码常量改为 SanguoParams 可调字段（审计遗留 M 项）。
    pub fn stamina_price(&self) -> Uint128 {
        if self.stamina_pack_price == 0 {
            STAMINA_PACK_PRICE
        } else {
            Uint128::new(self.stamina_pack_price)
        }
    }

    // ==================== 参数访问器（0 / 全零数组 = 回退 const 默认值）====================
    // 统一模式（裁定 6.2）：管理员未配置（0）时回退下方 DEFAULT_* / 现有 const，
    // 旧配置 JSON 缺字段（serde default 填 0）时行为与 V1 一致。

    // ---- 抽卡 ----
    /// 每日免费抽卡张数上限
    pub fn daily_draw_limit(&self) -> u64 {
        nz_u64(self.daily_draw_limit, DEFAULT_DAILY_DRAW_LIMIT)
    }
    /// 每日碎片加购抽卡张数上限
    pub fn daily_draw_purchase_cap(&self) -> u32 {
        nz_u32(self.daily_draw_purchase_cap, DEFAULT_DAILY_DRAW_PURCHASE_CAP)
    }
    /// 保底阈值（0 = 无保底；语义值，不回退）
    pub fn pity_threshold(&self) -> u32 {
        self.pity_threshold
    }

    // ---- 碎片 ----
    /// 每玩家每日碎片获取硬顶
    pub fn daily_fragment_cap(&self) -> u64 {
        nz_u64(self.daily_fragment_cap, DEFAULT_DAILY_FRAGMENT_CAP)
    }

    // ---- 体力 ----
    pub fn stamina_max(&self) -> u64 {
        nz_u64(self.stamina_max, STAMINA_MAX)
    }
    pub fn stamina_regen_secs(&self) -> u64 {
        nz_u64(self.stamina_regen_secs, STAMINA_REGEN_SECS)
    }
    pub fn stamina_pack_amount(&self) -> u64 {
        nz_u64(self.stamina_pack_amount, STAMINA_PACK_AMOUNT)
    }

    // ---- 持有 ----
    pub fn max_player_cards(&self) -> u32 {
        nz_u32(self.max_player_cards, MAX_PLAYER_CARDS as u32)
    }
    /// 卡池每稀有度数量上限（全 0 = 回退 RARITY_MAX）
    pub fn rarity_max(&self) -> [u64; 4] {
        nz_arr(self.rarity_max, RARITY_MAX)
    }

    // ---- 合成 / 分解 / 升星 ----
    pub fn craft_cost(&self) -> [u64; 4] {
        nz_arr(self.craft_cost, CRAFT_COST)
    }
    pub fn fragment_from_duplicate(&self) -> [u64; 4] {
        nz_arr(self.fragment_from_duplicate, FRAGMENT_FROM_DUPLICATE)
    }
    pub fn star_up_fragment_cost(&self) -> [u64; 4] {
        nz_arr(self.star_up_fragment_cost, STAR_UP_FRAGMENT_COST)
    }
    pub fn upgrade_atk_boost(&self) -> [u32; 4] {
        nz_arr(self.upgrade_atk_boost, DEFAULT_UPGRADE_ATK_BOOST)
    }
    pub fn upgrade_def_boost(&self) -> [u32; 4] {
        nz_arr(self.upgrade_def_boost, DEFAULT_UPGRADE_DEF_BOOST)
    }
    pub fn upgrade_frag_atk(&self) -> u32 {
        nz_u32(self.upgrade_frag_atk, UPGRADE_FRAG_ATK)
    }
    pub fn upgrade_frag_def(&self) -> u32 {
        nz_u32(self.upgrade_frag_def, UPGRADE_FRAG_DEF)
    }
    pub fn upgrade_frag_max_level(&self) -> u8 {
        nz_u8(self.upgrade_frag_max_level, UPGRADE_FRAG_MAX_LEVEL)
    }

    // ---- AI 难度 ----
    pub fn diff_boost_bp(&self) -> [u64; 5] {
        nz_arr(self.diff_boost_bp, DIFF_BOOST_BP)
    }
    pub fn ai_floor_bp(&self) -> [u64; 5] {
        nz_arr(self.ai_floor_bp, AI_FLOOR_BP)
    }
    /// AI 对战每日限次（6.2 表 ai_daily_limit）
    pub fn ai_daily_limit(&self) -> u64 {
        nz_u64(self.daily_ai_limit, DEFAULT_DAILY_AI_LIMIT)
    }

    // ---- 天梯排位 ----
    pub fn rank_boost_bp(&self) -> [u64; 5] {
        nz_arr(self.rank_boost_bp, RANK_BOOST_BP)
    }
    pub fn rank_win_points(&self) -> i64 {
        nz_i64(self.rank_win_points, RANK_WIN_POINTS)
    }
    pub fn rank_lose_points(&self) -> i64 {
        nz_i64(self.rank_lose_points, RANK_LOSE_POINTS)
    }
    pub fn rank_tier_up_points(&self) -> i64 {
        nz_i64(self.rank_tier_up_points, RANK_TIER_UP_POINTS)
    }
    pub fn rank_stamina_cost(&self) -> u64 {
        nz_u64(self.rank_stamina_cost, RANK_STAMINA_COST)
    }
    pub fn rank_win_season_points(&self) -> u64 {
        nz_u64(self.rank_win_season_points, RANK_WIN_SEASON_POINTS)
    }
    pub fn rank_rewards(&self) -> [Fragments; 5] {
        nz_arr(self.rank_rewards, RANK_REWARD)
    }

    // ---- 每日任务 ----
    pub fn daily_task_count(&self) -> u32 {
        nz_u32(self.daily_task_count, DAILY_TASK_COUNT as u32)
    }
    pub fn daily_task_required(&self) -> [u64; 4] {
        nz_arr(self.daily_task_required, DAILY_TASK_REQUIRED)
    }
    pub fn daily_task_reward(&self) -> [Fragments; 4] {
        nz_arr(self.daily_task_reward, DAILY_TASK_REWARD)
    }
    pub fn daily_task_season_points(&self) -> u64 {
        nz_u64(self.daily_task_season_points, DAILY_TASK_SEASON_POINTS)
    }

    // ---- 赠送冷却（gift.rs 全量改读）----
    pub fn gift_holding_secs(&self) -> u64 {
        nz_u64(self.gift_holding_secs, GIFT_HOLDING_SECS)
    }
    pub fn gift_legend_holding_secs(&self) -> u64 {
        nz_u64(self.gift_legend_holding_secs, GIFT_LEGEND_HOLDING_SECS)
    }
    pub fn gift_pair_age_secs(&self) -> u64 {
        nz_u64(self.gift_pair_age_secs, GIFT_PAIR_AGE_SECS)
    }
    pub fn gift_acct_age_secs(&self) -> u64 {
        nz_u64(self.gift_acct_age_secs, GIFT_ACCT_AGE_SECS)
    }
    pub fn gift_pair_idle_expire_secs(&self) -> u64 {
        nz_u64(self.gift_pair_idle_expire_secs, GIFT_PAIR_IDLE_EXPIRE_SECS)
    }
    pub fn gift_pair_refresh_interval(&self) -> u64 {
        nz_u64(self.gift_pair_refresh_interval, GIFT_PAIR_REFRESH_INTERVAL)
    }
    pub fn gift_daily_sent(&self) -> u32 {
        nz_u32(self.gift_daily_sent, GIFT_DAILY_SENT)
    }
    pub fn gift_daily_recv(&self) -> u32 {
        nz_u32(self.gift_daily_recv, GIFT_DAILY_RECV)
    }
    pub fn gift_frag_cap(&self) -> u64 {
        nz_u64(self.gift_frag_cap, GIFT_FRAG_CAP)
    }

    // ---- 指定卡碎片价 / 编队 ----
    pub fn card_frag_price(&self) -> [u64; 4] {
        nz_arr(self.card_frag_price, DEFAULT_CARD_FRAG_PRICE)
    }
    pub fn team_cost_cap(&self) -> u32 {
        nz_u32(self.team_cost_cap, TEAM_COST_CAP)
    }

    // ---- 战斗系数（0 = 回退 battle::compute DEFAULT_*）----
    pub fn faction_counter_bp(&self) -> u64 {
        nz_u64(self.faction_counter_bp, crate::sanguo::battle::compute::DEFAULT_FACTION_COUNTER_BP)
    }
    pub fn faction_countered_bp(&self) -> u64 {
        nz_u64(self.faction_countered_bp, crate::sanguo::battle::compute::DEFAULT_FACTION_COUNTERED_BP)
    }
    pub fn faction_neutral_bp(&self) -> u64 {
        nz_u64(self.faction_neutral_bp, crate::sanguo::battle::compute::DEFAULT_FACTION_NEUTRAL_BP)
    }
    pub fn troop_counter_bp(&self) -> u64 {
        nz_u64(self.troop_counter_bp, crate::sanguo::battle::compute::DEFAULT_TROOP_COUNTER_BP)
    }
    pub fn troop_countered_bp(&self) -> u64 {
        nz_u64(self.troop_countered_bp, crate::sanguo::battle::compute::DEFAULT_TROOP_COUNTERED_BP)
    }
    pub fn troop_neutral_bp(&self) -> u64 {
        nz_u64(self.troop_neutral_bp, crate::sanguo::battle::compute::DEFAULT_TROOP_NEUTRAL_BP)
    }
    pub fn rarity_legend_bp(&self) -> u64 {
        nz_u64(self.rarity_legend_bp, crate::sanguo::battle::compute::DEFAULT_RARITY_LEGEND_BP)
    }
    pub fn rarity_epic_bp(&self) -> u64 {
        nz_u64(self.rarity_epic_bp, crate::sanguo::battle::compute::DEFAULT_RARITY_EPIC_BP)
    }
    pub fn rarity_rare_bp(&self) -> u64 {
        nz_u64(self.rarity_rare_bp, crate::sanguo::battle::compute::DEFAULT_RARITY_RARE_BP)
    }
    pub fn rarity_common_bp(&self) -> u64 {
        nz_u64(self.rarity_common_bp, crate::sanguo::battle::compute::DEFAULT_RARITY_COMMON_BP)
    }
    pub fn power_flat_cap_bp(&self) -> u64 {
        nz_u64(self.power_flat_cap_bp, crate::sanguo::battle::compute::DEFAULT_POWER_FLAT_CAP_BP)
    }
    pub fn eff_lower_bound_bp(&self) -> u64 {
        nz_u64(self.eff_lower_bound_bp, crate::sanguo::battle::compute::DEFAULT_EFF_LOWER_BOUND_BP)
    }

    pub fn skill_strength_bp(&self) -> u64 {
        nz_u64(self.skill_strength_bp, crate::sanguo::battle::compute::DEFAULT_SKILL_STRENGTH_BP)
    }

    pub fn bond_strength_bp(&self) -> u64 {
        nz_u64(self.bond_strength_bp, crate::sanguo::battle::compute::DEFAULT_BOND_STRENGTH_BP)
    }

    pub fn default_sanguo() -> Self {
        // V2 默认值（裁定 6.2）：deprecated 字段恒 0（C4——不再读取）；
        // 生效字段全部显式初始化，作为「单一事实来源」供 migrate 合并与查询展示。
        SanguoParams {
            // 抽卡
            daily_draw_limit: DEFAULT_DAILY_DRAW_LIMIT,
            daily_draw_purchase_cap: DEFAULT_DAILY_DRAW_PURCHASE_CAP,
            pity_threshold: 0, // 无保底
            // 碎片
            daily_fragment_cap: DEFAULT_DAILY_FRAGMENT_CAP,
            // 体力
            stamina_max: STAMINA_MAX,
            stamina_regen_secs: STAMINA_REGEN_SECS,
            stamina_pack_amount: STAMINA_PACK_AMOUNT,
            stamina_pack_price: 2_000_000, // 2 PAXI/包
            // 持有
            max_player_cards: MAX_PLAYER_CARDS as u32,
            max_cards: 150, // 与 RARITY_MAX 总和一致
            rarity_max: RARITY_MAX,
            // 合成 / 分解 / 升星
            craft_cost: CRAFT_COST,
            fragment_from_duplicate: FRAGMENT_FROM_DUPLICATE,
            star_up_fragment_cost: STAR_UP_FRAGMENT_COST,
            upgrade_atk_boost: DEFAULT_UPGRADE_ATK_BOOST,
            upgrade_def_boost: DEFAULT_UPGRADE_DEF_BOOST,
            upgrade_frag_atk: UPGRADE_FRAG_ATK,
            upgrade_frag_def: UPGRADE_FRAG_DEF,
            upgrade_frag_max_level: UPGRADE_FRAG_MAX_LEVEL,
            // AI 难度
            diff_boost_bp: DIFF_BOOST_BP,
            ai_floor_bp: AI_FLOOR_BP,
            daily_ai_limit: DEFAULT_DAILY_AI_LIMIT,
            // 天梯排位
            rank_boost_bp: RANK_BOOST_BP,
            rank_win_points: RANK_WIN_POINTS,
            rank_lose_points: RANK_LOSE_POINTS,
            rank_tier_up_points: RANK_TIER_UP_POINTS,
            rank_stamina_cost: RANK_STAMINA_COST,
            rank_win_season_points: RANK_WIN_SEASON_POINTS,
            rank_rewards: RANK_REWARD,
            // 每日任务
            daily_task_count: DAILY_TASK_COUNT as u32,
            daily_task_required: DAILY_TASK_REQUIRED,
            daily_task_reward: DAILY_TASK_REWARD,
            daily_task_season_points: DAILY_TASK_SEASON_POINTS,
            // 赠送冷却
            gift_holding_secs: GIFT_HOLDING_SECS,
            gift_legend_holding_secs: GIFT_LEGEND_HOLDING_SECS,
            gift_pair_age_secs: GIFT_PAIR_AGE_SECS,
            gift_acct_age_secs: GIFT_ACCT_AGE_SECS,
            gift_pair_idle_expire_secs: GIFT_PAIR_IDLE_EXPIRE_SECS,
            gift_pair_refresh_interval: GIFT_PAIR_REFRESH_INTERVAL,
            gift_daily_sent: GIFT_DAILY_SENT,
            gift_daily_recv: GIFT_DAILY_RECV,
            gift_frag_cap: GIFT_FRAG_CAP,
            // 战斗奖励（合规：只发不可转让碎片）
            ai_frag_reward: [
                Fragments { common: 20, rare: 0, epic: 0, legend: 0 },
                Fragments { common: 30, rare: 0, epic: 0, legend: 0 },
                Fragments { common: 0, rare: 5, epic: 0, legend: 0 },
                Fragments { common: 0, rare: 10, epic: 0, legend: 0 },
                Fragments { common: 0, rare: 0, epic: 2, legend: 0 },
            ],
            pvp_frag_reward: Fragments { common: 10, rare: 5, epic: 0, legend: 0 },
            royale_frag_reward: Fragments { common: 20, rare: 10, epic: 1, legend: 0 },
            // 指定卡碎片价
            card_frag_price: DEFAULT_CARD_FRAG_PRICE,
            // 编队
            team_cost_cap: TEAM_COST_CAP,
            // 战斗系数（12 项）
            faction_counter_bp: crate::sanguo::battle::compute::DEFAULT_FACTION_COUNTER_BP,
            faction_countered_bp: crate::sanguo::battle::compute::DEFAULT_FACTION_COUNTERED_BP,
            faction_neutral_bp: crate::sanguo::battle::compute::DEFAULT_FACTION_NEUTRAL_BP,
            troop_counter_bp: crate::sanguo::battle::compute::DEFAULT_TROOP_COUNTER_BP,
            troop_countered_bp: crate::sanguo::battle::compute::DEFAULT_TROOP_COUNTERED_BP,
            troop_neutral_bp: crate::sanguo::battle::compute::DEFAULT_TROOP_NEUTRAL_BP,
            rarity_legend_bp: crate::sanguo::battle::compute::DEFAULT_RARITY_LEGEND_BP,
            rarity_epic_bp: crate::sanguo::battle::compute::DEFAULT_RARITY_EPIC_BP,
            rarity_rare_bp: crate::sanguo::battle::compute::DEFAULT_RARITY_RARE_BP,
            rarity_common_bp: crate::sanguo::battle::compute::DEFAULT_RARITY_COMMON_BP,
            power_flat_cap_bp: crate::sanguo::battle::compute::DEFAULT_POWER_FLAT_CAP_BP,
            eff_lower_bound_bp: crate::sanguo::battle::compute::DEFAULT_EFF_LOWER_BOUND_BP,
            skill_strength_bp: crate::sanguo::battle::compute::DEFAULT_SKILL_STRENGTH_BP,
            bond_strength_bp: crate::sanguo::battle::compute::DEFAULT_BOND_STRENGTH_BP,
            // deprecated（C4）：恒 0，exec 不再读取；保留字段仅为旧配置 JSON 可反序列化
            single_paxi_fee: 0,
            pack3_paxi_fee: 0,
            single_prc_total: 0,
            single_prc_eco: 0,
            single_prc_burn: 0,
            pack3_prc_total: 0,
            pack3_prc_eco: 0,
            pack3_prc_burn: 0,
            ai_fee: [0u128; 5],
            ai_reward: [0u128; 5],
            ai_burn_pct_bp: 0,
            upgrade_fees: [0u128; 4],
            pvp_fee: 0,
            royale_entry_fee: 0,
            royale_reward_pct_bp: 0,
            royale_burn_pct_bp: 0,
            proposal_deposit: 0,
            voting_period: 0,
        }
    }
}

// ==================== 参数访问器辅助（0 / 全零 = 未配置）====================

#[inline]
fn nz_u64(v: u64, d: u64) -> u64 {
    if v == 0 {
        d
    } else {
        v
    }
}

#[inline]
fn nz_u32(v: u32, d: u32) -> u32 {
    if v == 0 {
        d
    } else {
        v
    }
}

#[inline]
fn nz_u8(v: u8, d: u8) -> u8 {
    if v == 0 {
        d
    } else {
        v
    }
}

#[inline]
fn nz_i64(v: i64, d: i64) -> i64 {
    if v == 0 {
        d
    } else {
        v
    }
}

/// 全零数组 = 未配置 → 回退默认数组（u64/u32 数值数组与 Fragments 奖励表通用）
#[inline]
fn nz_arr<T, const N: usize>(v: [T; N], d: [T; N]) -> [T; N]
where
    T: PartialEq + Default + Copy,
{
    if v == [T::default(); N] {
        d
    } else {
        v
    }
}

// 🟢 AI经济修复 ④：全局日赔付上限（防止多钱包绕过玩家日限额抽干池子）
// 每天 0:00 (UTC+8) 重置；超过 cap 后所有 reward/claim 拒绝
pub const GLOBAL_DAILY_PAYOUT: Item<Uint128> = Item::new("sg_global_daily_payout");
pub const GLOBAL_DAILY_PAYOUT_DATE: Item<u64> = Item::new("sg_global_daily_payout_date");
pub const GLOBAL_DAILY_PAYOUT_CAP: Item<Uint128> = Item::new("sg_global_daily_payout_cap");
/// 默认 1e14 raw（约 1 亿 TKCC/天），管理员可通过 SanguoSetGlobalDailyPayoutCap 调整
pub const DEFAULT_GLOBAL_DAILY_PAYOUT_CAP: u128 = 100_000_000_000_000;

/// 纯查询：获取今日的全局日赔付（不会触发日期重置）
/// 用于 Query 接口（只读）
pub fn query_global_daily_payout(deps: Deps) -> StdResult<Uint128> {
    Ok(GLOBAL_DAILY_PAYOUT.may_load(deps.storage)?.unwrap_or(Uint128::zero()))
}

/// 🟢 AI经济修复 ④：累计全局日赔付 + 自动按日期重置 + 检查是否超过 cap
///
/// 防多钱包绕过玩家日限额抽干池子的最后一道防线。
/// 每天 0:00 (UTC) 重置 GLOBAL_DAILY_PAYOUT；超过 cap 后所有 reward/claim 拒绝。
pub fn check_and_add_global_payout(
    deps: DepsMut,
    now_seconds: u64,
    amount: Uint128,
) -> Result<(), ContractError> {
    let today = now_seconds / 86400;
    let stored_date = GLOBAL_DAILY_PAYOUT_DATE.may_load(deps.storage)?
        .unwrap_or(0);

    let current: Uint128 = if stored_date != today {
        // 新的一天 — 重置计数器
        GLOBAL_DAILY_PAYOUT_DATE.save(deps.storage, &today)?;
        GLOBAL_DAILY_PAYOUT.save(deps.storage, &Uint128::zero())?;
        Uint128::zero()
    } else {
        GLOBAL_DAILY_PAYOUT.may_load(deps.storage)?
            .unwrap_or(Uint128::zero())
    };

    let cap = GLOBAL_DAILY_PAYOUT_CAP.may_load(deps.storage)?
        .unwrap_or(Uint128::from(DEFAULT_GLOBAL_DAILY_PAYOUT_CAP));

    let after = current.checked_add(amount)
        .map_err(|_| ContractError::Overflow)?;
    if after > cap {
        return Err(ContractError::SanguoGlobalDailyPayoutExceeded {
            current: current.u128(),
            cap: cap.u128(),
        });
    }
    GLOBAL_DAILY_PAYOUT.save(deps.storage, &after)?;
    Ok(())
}

// ===== 卡牌与卡池 =====

// V2（裁定 8.1 / T4）：后 5 字段全部 #[serde(default)]——cw-storage-plus 2.0.0
// 走 JSON 序列化（to_json_vec），旧 6 字段配置/链上数据可被 11 字段结构直读，
// 无需显式 CardTemplateV1 迁移结构。
#[cw_serde]
pub struct CardTemplate {
    pub id: String,
    pub name: String,
    pub rarity: String, // common / rare / epic / legend
    pub attack: u32,
    pub defense: u32,
    pub weight: u32,
    /// 阵营：wei / shu / wu / qun（克制链 wei→shu→wu→qun→wei）
    #[serde(default)]
    pub faction: String,
    /// 兵种：cavalry / archer / strategist / guard（克制链 cav→archer→strat→guard→cav）
    #[serde(default)]
    pub troop: String,
    /// 编队 cost（Σcost ≤ params.team_cost_cap()）
    #[serde(default)]
    pub cost: u8,
    /// 技能 id（空串 = 无技能；3★ 起生效，见 battle::skills）
    #[serde(default)]
    pub skill_id: String,
    /// 所属连携 id 列表（见 battle::bonds）
    #[serde(default)]
    pub bonds: Vec<String>,
}

#[cw_serde]
pub struct CardInfo {
    pub card_id: String,
    pub owner: String,
    pub name: String,
    pub rarity: String,
    pub attack: u32,
    pub defense: u32,
    pub star: u8,
    pub level: u8,
}

#[cw_serde]
pub struct BattleRecord {
    pub battle_id: String,
    pub player: Addr,
    pub difficulty: u8,
    pub result: String, // "win" / "lose"
    pub reward: String, // u128 字符串
    pub claimed: bool,
    pub timestamp: u64,
}

#[cw_serde]
#[derive(Default)]
pub struct AiStats {
    pub total: u64,
    pub wins: u64,
    /// V2.1：分模式计数（首页「总对战」= 四模式合计）。
    /// #[serde(default)] 关键：线上旧数据形如 {"total":x,"wins":y}，
    /// 缺字段时必须能反序列化，否则 migrate 后首次 may_load 直接炸。
    #[serde(default)]
    pub pvp_total: u64,
    #[serde(default)]
    pub pvp_wins: u64,
    #[serde(default)]
    pub royale_total: u64,
    #[serde(default)]
    pub royale_wins: u64,
    #[serde(default)]
    pub stage_total: u64,
    #[serde(default)]
    pub stage_wins: u64,
}

impl AiStats {
    /// 全模式总对战（AI + PVP + 混战 + 关卡）
    pub fn all_total(&self) -> u64 {
        self.total
            .saturating_add(self.pvp_total)
            .saturating_add(self.royale_total)
            .saturating_add(self.stage_total)
    }
    /// 全模式总胜场
    pub fn all_wins(&self) -> u64 {
        self.wins
            .saturating_add(self.pvp_wins)
            .saturating_add(self.royale_wins)
            .saturating_add(self.stage_wins)
    }
}

// Fragments 结构体本身仍被使用（Fragments::rarity_index 由
// validate_card_template_internal 调用，FRAGMENTS 仍为 Map 值类型），
// 此处不加 allow；仅 get / set 方法加（见下方 impl）。
#[cw_serde]
#[derive(Default, Copy)]
pub struct Fragments {
    pub common: u64,
    pub rare: u64,
    pub epic: u64,
    pub legend: u64,
}

/// 四桶 → 统一碎片折算单位（T7/M6：common×1 / rare×5 / epic×20 / legend×80）
pub const FRAG_UNIT: [u64; 4] = [1, 5, 20, 80];

impl Fragments {
    pub fn get(&self, rarity: &str) -> u64 {
        match rarity {
            "common" => self.common,
            "rare" => self.rare,
            "epic" => self.epic,
            "legend" => self.legend,
            _ => 0,
        }
    }
    pub fn set(&mut self, rarity: &str, v: u64) {
        match rarity {
            "common" => self.common = v,
            "rare" => self.rare = v,
            "epic" => self.epic = v,
            "legend" => self.legend = v,
            _ => {}
        }
    }
    /// T7：折算为统一碎片总量（FRAGMENT_BALANCE 入账/产出统一口径；
    /// 折算率同 migrate M6：common×1 / rare×5 / epic×20 / legend×80）
    pub fn to_unified(&self) -> u128 {
        self.common as u128 * FRAG_UNIT[0] as u128
            + self.rare as u128 * FRAG_UNIT[1] as u128
            + self.epic as u128 * FRAG_UNIT[2] as u128
            + self.legend as u128 * FRAG_UNIT[3] as u128
    }
    // rarity_index 被 validate_card_template_internal 使用，不加 allow
    pub fn rarity_index(r: &str) -> Option<usize> {
        match r {
            "common" => Some(0),
            "rare" => Some(1),
            "epic" => Some(2),
            "legend" => Some(3),
            _ => None,
        }
    }
}

// ===== 提案（V2 全禁，T24/4.2：SanguoProposeCard/VoteCard/ExecuteProposal/
//          CancelProposal 四入口删除；CardProposal 结构体与 PROPOSALS/
//          PROPOSAL_COUNTER / VOTES / PROPOSAL_DEPOSITS 四件套随之删除。
//          存量押金由 migrate M3 收敛到 PENDING_REFUNDS（PAXI 键）退还）=====

// ===== PVP / 混战 =====

#[cw_serde]
pub enum PvpStatus {
    Waiting,
    Pending,
    Finished,
    Cancelled,
}

impl Default for PvpStatus {
    fn default() -> Self {
        PvpStatus::Waiting
    }
}

#[cw_serde]
pub struct PvpMatch {
    pub match_id: String,
    pub challenger: Addr,
    pub opponent: Addr,
    pub is_public: bool,
    pub challenger_order: Vec<String>,
    pub opponent_order: Vec<String>,
    pub winner: Option<Addr>,
    pub status: PvpStatus,
    pub created_at: u64,
    pub finished_at: Option<u64>,
    pub reward_claimed: bool,
    /// 🟢 Q3 亮牌：逐组胜负（每组 3 张 vs 3 张，三局两胜）。
    /// A=挑战者、B=应战者；每个元素是该方在该组赢下的局数（0~3）。
    /// 用 serde(default) 兼容升级前已存储的对局（回退 [0,0,0]）。
    #[serde(default)]
    pub group_scores_a: [u8; 3],
    #[serde(default)]
    pub group_scores_b: [u8; 3],
}

#[cw_serde]
pub enum RoyaleStatus {
    Waiting,
    Full,
    Finished,
    Cancelled,
}

impl Default for RoyaleStatus {
    fn default() -> Self {
        RoyaleStatus::Waiting
    }
}

#[cw_serde]
pub struct RoyaleMatch {
    pub royale_id: String,
    pub players: Vec<Addr>,
    pub player_orders: Vec<Vec<String>>,
    pub winner: Option<Addr>,
    pub status: RoyaleStatus,
    pub created_at: u64,
    pub finished_at: Option<u64>,
    pub size: u8,
    pub reward_claimed: bool,
}

// ============================================================================
// 🟢 合规模式（V2 起为唯一路径，C1 恒真）—— 三国侧状态项
//
// 红线：绝不奖励/收取 PAXI / TKCC。奖励只允许不可转让的碎片（Fragments）。
//   - 抽卡：免费，每日限次（params.daily_draw_limit，默认 3 次，pack3 按张计数）
//   - 运营发碎片：仅 OPERATOR_LIST / 管理员可调，受 params.daily_fragment_cap 硬顶
//   - 关卡（链下打、链上复算）：玩家提交 9 张卡，合约用关卡敌人配置复算 9v9
//   - DAILY_DRAW_LIMIT / DAILY_FRAGMENT_CAP 两个 Item 已收敛进 SanguoParams
//     （5.3 收敛项；migrate 负责把旧值搬进 params 后删键，见 8.2 M2）
// ============================================================================
pub const OPERATOR_LIST: Item<Vec<String>> = Item::new("sg_operator_list");
/// 每日免费抽卡默认上限（params.daily_draw_limit = 0 时回退）
pub const DEFAULT_DAILY_DRAW_LIMIT: u64 = 3;
/// 每日碎片获取默认硬顶（params.daily_fragment_cap = 0 时回退）
pub const DEFAULT_DAILY_FRAGMENT_CAP: u64 = 200;
/// 每日碎片加购抽卡默认上限（params.daily_draw_purchase_cap = 0 时回退）
pub const DEFAULT_DAILY_DRAW_PURCHASE_CAP: u32 = 5;
/// 保底默认阈值（0 = 无保底；params.pity_threshold 为语义值直读，不回退）
pub const DEFAULT_PITY_THRESHOLD: u32 = 0;

/// 每玩家每日抽卡计数（按 UTC 日重置，与 SECS_PER_DAY 对齐）
pub const SG_DRAW_DATE: Map<&Addr, u64> = Map::new("sg_draw_date");
pub const SG_DRAW_COUNT: Map<&Addr, u64> = Map::new("sg_draw_count");

/// 每玩家每日已发放碎片总量（所有渠道合计：运营批量 + 关卡奖励）
pub const SG_GRANT_DATE: Map<&Addr, u64> = Map::new("sg_grant_date");
pub const SG_GRANT_AMOUNT: Map<&Addr, u64> = Map::new("sg_grant_amount");

/// 关卡配置（敌人用卡池模板 id + 星级表示；E5：Fixed 不可随机，关卡可复算可预演）
#[cw_serde]
pub struct StageConfig {
    pub id: u64,
    pub name: String,
    /// 9 个卡池模板 id（按 3 张一组 × 3 组）
    pub enemy_tpl_ids: Vec<String>,
    /// 敌卡星级（与 enemy_tpl_ids 等长；缺省/空 = 全部 1★，旧数据兼容；
    /// 3★ 起带技能，见 battle::ai_gen::SKILL_UNLOCK_STAR）
    #[serde(default)]
    pub enemy_stars: Vec<u8>,
    /// 通关奖励（碎片）
    pub reward: Fragments,
}
pub const STAGES: Item<Vec<StageConfig>> = Item::new("sg_stages");

/// 防重放：key = (player, stage_id) → 已通关的 batch_id
pub const STAGE_CLEARED: Map<(&Addr, u64), String> = Map::new("sg_stage_cleared");

// ============================================================================
// 🆕 新玩法：体力 / 皮肤 / 赛季通行证 / 天梯排位 / 每日任务
//
// 合规约束：所有奖励只发不可转让碎片（Fragments）；所有付费入口（体力/皮肤/
// 通行证）明码标价、确定性获得、PAXI 经通用收费通道转入项目方钱包，绝不发
// PAXI/TKCC 赔付。玩家操作全部复用 sanguo_auth 无感验签。
// ============================================================================

// ===== 体力 =====
#[cw_serde]
#[derive(Default)]
pub struct StaminaInfo {
    pub current: u64,
    pub last_update: u64,
}
pub const STAMINA: Map<&Addr, StaminaInfo> = Map::new("sg_stamina");
pub const STAMINA_MAX: u64 = 120;
pub const STAMINA_REGEN_SECS: u64 = 300; // 每 5 分钟恢复 1 点
pub const STAMINA_PACK_AMOUNT: u64 = 30; // 每次购买 +30
pub const STAMINA_PACK_PRICE: Uint128 = Uint128::new(2_000_000); // 默认 2 PAXI/包（params.stamina_pack_price 为 0 时的兜底）

/// 二次审计 M-6：玩家持有卡牌上限（防 PLAYER_CARDS 无界增长撑爆 storage）
pub const MAX_PLAYER_CARDS: usize = 512;

/// 按自然时间再生体力（不写盘，调用方自行 save）
pub fn regen_stamina_info(info: &mut StaminaInfo, now: u64) {
    if info.current >= STAMINA_MAX {
        info.last_update = now;
        return;
    }
    let elapsed = now.saturating_sub(info.last_update);
    let gained = (elapsed / STAMINA_REGEN_SECS) as u64;
    if gained > 0 {
        info.current = (info.current + gained).min(STAMINA_MAX);
        info.last_update = info.last_update + gained * STAMINA_REGEN_SECS;
    }
}

// ===== 皮肤（纯外观，PAXI 购买，不可转让）=====
#[cw_serde]
#[derive(Default)]
pub struct SkinInfo {
    pub id: String,
    pub name: String,
    pub rarity: String,
    pub price_paxi: Uint128,
    pub desc: String,
}
pub const SKINS: Map<&str, SkinInfo> = Map::new("sg_skins");
pub const PLAYER_SKINS: Map<&Addr, Vec<String>> = Map::new("sg_player_skins");

// ===== 赛季通行证 =====
#[cw_serde]
#[derive(Default)]
pub struct SeasonTier {
    pub points_req: u64,
    pub reward: Fragments,
}
#[cw_serde]
#[derive(Default)]
pub struct SeasonConfig {
    pub season_id: u64,
    pub start_time: u64,
    pub end_time: u64,
    pub pass_price_paxi: Uint128,
    pub tiers: Vec<SeasonTier>,
}
#[cw_serde]
#[derive(Default)]
pub struct PlayerSeason {
    pub pass_owned: bool,
    pub points: u64,
    pub claimed: Vec<u64>,
}
pub const SEASON_CONFIG: Item<SeasonConfig> = Item::new("sg_season_cfg");
pub const PLAYER_SEASON: Map<&Addr, PlayerSeason> = Map::new("sg_player_season");

/// 赛季是否处于激活窗口（管理员已配置且时间在 start..end 内）
pub fn season_active(cfg: &SeasonConfig, now: u64) -> bool {
    !cfg.tiers.is_empty() && cfg.start_time <= now && now < cfg.end_time
}

// ===== 天梯排位 =====
#[cw_serde]
#[derive(Default)]
pub struct RankInfo {
    pub tier: u8,
    pub points: i64,
}
pub const RANK_INFO: Map<&Addr, RankInfo> = Map::new("sg_rank");
/// 各档 AI 战力加成（基点），tier 0..4
pub const RANK_BOOST_BP: [u64; 5] = [11000, 12500, 14000, 16000, 18000];
pub const RANK_WIN_POINTS: i64 = 20;
pub const RANK_LOSE_POINTS: i64 = 10;
pub const RANK_TIER_UP_POINTS: i64 = 100; // 满 100 升一档
pub const RANK_STAMINA_COST: u64 = 10;
pub const RANK_WIN_SEASON_POINTS: u64 = 10;
/// 各档胜利碎片奖励（受每日硬顶）
pub const RANK_REWARD: [Fragments; 5] = [
    Fragments { common: 5, rare: 0, epic: 0, legend: 0 },
    Fragments { common: 0, rare: 2, epic: 0, legend: 0 },
    Fragments { common: 0, rare: 0, epic: 1, legend: 0 },
    Fragments { common: 0, rare: 0, epic: 3, legend: 0 },
    Fragments { common: 0, rare: 0, epic: 0, legend: 1 },
];

// ===== 每日任务 =====
#[cw_serde]
#[derive(Default)]
pub struct DailyTaskState {
    pub progress: Vec<u64>,
    pub claimed: Vec<bool>,
}
pub const DAILY_TASK: Map<(&Addr, u64), DailyTaskState> = Map::new("sg_daily_task");
pub const DAILY_TASK_COUNT: usize = 4;
pub const DAILY_TASK_REQUIRED: [u64; 4] = [1, 1, 1, 1];
pub const DAILY_TASK_REWARD: [Fragments; 4] = [
    Fragments { common: 5, rare: 0, epic: 0, legend: 0 }, // 任务 0：通关 1 关
    Fragments { common: 0, rare: 2, epic: 0, legend: 0 }, // 任务 1：天梯胜 1 场
    Fragments { common: 10, rare: 0, epic: 0, legend: 0 }, // 任务 2：买 1 张卡
    Fragments { common: 5, rare: 0, epic: 0, legend: 0 }, // 任务 3：买 1 包碎片
];
pub const DAILY_TASK_SEASON_POINTS: u64 = 10;

// ============================================================================
// 🆕 V2 新玩法存储（裁定 5.1 新增 11 项 + 5.2 泛化 2 项 + T3 SHOP_ITEMS 独立 Map）
// ============================================================================

// ---- 统一碎片币（T7 经济闭环核心）----
/// 统一不可转让碎片余额（纯游戏内计数）。
/// 9.1 P0：所有碎片产出/入账/消耗全量改指本表；旧四桶 FRAGMENTS 仅作奖励表
/// 数据结构 + migrate 迁移源，V2 起停止写入。
pub const FRAGMENT_BALANCE: Map<&Addr, u128> = Map::new("sg_frag_balance");

// ---- 商店商品表（T3：独立 Map 逐商品 upsert，改一个商品不用重写全表）----
#[cw_serde]
pub struct ShopItem {
    /// 商品 id（如 "skin_gold" / "item_draw_1" / "item_reset_stage"）
    pub id: String,
    /// 类型：skin/avatar/cardback/title/card/stamina/draw/reset
    pub item_type: String,
    /// 类型内目标 id（皮肤 id / 模板 id / 关卡 id 等；draw/stamina 可空串）
    pub item_id: String,
    /// PAXI 价（None = 本商品不接受 PAXI）
    pub price_paxi: Option<Uint128>,
    /// 碎片价（None = 本商品不接受碎片）
    pub price_fragment: Option<Uint128>,
    /// 每日限购（0 = 买断制；T14 删 one_time，纯日限语义）
    pub daily_limit: u32,
    /// 是否上架
    pub enabled: bool,
}
pub const SHOP_ITEMS: Map<&str, ShopItem> = Map::new("sg_shop_items");

// ---- 卡锁定（分解/赠送/入预设前必查）----
pub const LOCKED_CARDS: Map<&str, bool> = Map::new("sg_locked_cards");

// ---- 队伍预设 ----
#[cw_serde]
#[derive(Default)]
pub struct TeamPreset {
    pub name: String,
    /// 9 张卡 id（写入时校验归属 + 去重 + Σcost ≤ params.team_cost_cap()）
    pub card_ids: Vec<String>,
}
pub const TEAM_PRESETS: Map<(&Addr, u8), TeamPreset> = Map::new("sg_team_presets");
/// 预设槽位数（preset_id ∈ 0..MAX_TEAM_PRESETS-1）
pub const MAX_TEAM_PRESETS: u8 = 3;
/// 化妆品装备位（EQUIPPED_COSMETICS 的 slot 维度）
pub const COSMETIC_SLOTS: [&str; 4] = ["skin", "avatar", "cardback", "title"];

// ---- 化妆品装备（4 位；拥有列表仍看 PLAYER_SKINS）----
/// (player, slot) → 已装备 item_id（slot ∈ COSMETIC_SLOTS）
pub const EQUIPPED_COSMETICS: Map<(&Addr, &str), String> = Map::new("sg_equipped_cos");

// ---- 每日购买计数（每玩家每日一条 BTreeMap，防存储膨胀，7.7）----
pub const DAILY_PURCHASES: Map<(&Addr, u64), BTreeMap<String, u32>> = Map::new("sg_daily_purchases");

// ---- 抽卡保底计数 ----
pub const PITY_COUNTER: Map<&Addr, u32> = Map::new("sg_pity");

// ---- 加购抽卡计数（日；受 params.daily_draw_purchase_cap() 约束）----
pub const SG_DRAW_PURCHASED: Map<(&Addr, u64), u32> = Map::new("sg_draw_purchased");

// ---- 提案押金待退（PAXI 键；M3：PROPOSAL_DEPOSITS 收敛至此）----
pub const PENDING_REFUNDS: Map<&Addr, u128> = Map::new("sg_pending_refunds");

// ---- 图鉴（曾拥有；收集进度前端展示）----
pub const COLLECTION: Map<(&Addr, &str), bool> = Map::new("sg_collection");

// ---- 碎片迁移超额待审队列（M6：折算值 + 游标）----
pub const PENDING_FRAGMENT_REVIEW: Map<&Addr, (u128, u64)> = Map::new("sg_frag_review");

// ---- V2 迁移防重入标记（裁定 8.1 步骤 4；写码对账：超出 5.1 新增 11 清单的
//      4 个必要存储项——MIGRATION_V2_DONE / PAUSED_GAMES / PVP_SEQ / ROYALE_SEQ，
//      供专家审计确认。其中 PVP_SEQ / ROYALE_SEQ 为对局 ID 自增序号，避免
//      同一秒内同一玩家重复发起时 match_id / royale_id 相互覆盖）----
pub const MIGRATION_V2_DONE: Item<bool> = Item::new("sg_migration_v2_done");

// ---- 单玩法紧急暂停表（C 档 SanguoPauseGame；存 game_id 列表，
//      有序小表避免逐玩法 Map 遍历）----
pub const PAUSED_GAMES: Item<Vec<String>> = Item::new("sg_paused_games");

// ---- 对局 ID 自增序号（审计遗留项⑤修复：同秒同玩家重复创建会互相覆盖）----
/// PVP_MATCHES 的 match_id 后缀自增序号（保证同秒同玩家不重复）
pub const PVP_SEQ: Item<u64> = Item::new("sg_pvp_seq");
/// ROYALE_MATCHES 的 royale_id 后缀自增序号（保证同秒同玩家不重复）
pub const ROYALE_SEQ: Item<u64> = Item::new("sg_royale_seq");

// ---- 指定卡碎片价 / 编队上限 默认值（params 对应字段 0 时回退）----
/// common/rare/epic/legend = 1500/600/250/80（T15）
pub const DEFAULT_CARD_FRAG_PRICE: [u64; 4] = [1500, 600, 250, 80];
/// 出战编队 Σcost 上限（T12）
pub const TEAM_COST_CAP: u32 = 30;

// ===== 存储 =====

pub const SANGUO_CONFIG: Item<SanguoConfig> = Item::new("sanguo_config");pub const SANGUO_PARAMS: Item<SanguoParams> = Item::new("sanguo_params");

pub const CARD_TEMPLATES: Item<Vec<CardTemplate>> = Item::new("sanguo_templates");
pub const CARD_TEMPLATE_COUNT: Item<u64> = Item::new("sanguo_tpl_count");
pub const RARITY_COUNT: Map<&str, u64> = Map::new("sanguo_rarity_count");
pub const CARDS: Map<&str, CardInfo> = Map::new("sanguo_cards"); // key = card_id
pub const PLAYER_CARDS: Map<&Addr, Vec<String>> = Map::new("sanguo_player_cards");
pub const CARD_ID_COUNTER: Item<u64> = Item::new("sanguo_card_counter");

pub const BATTLES: Map<&str, BattleRecord> = Map::new("sanguo_battles");
pub const PENDING_REWARDS: Map<&Addr, Vec<String>> = Map::new("sanguo_pending");
pub const AI_BATTLE_COUNT: Map<&Addr, u64> = Map::new("sanguo_ai_count");
pub const AI_BATTLE_DATE: Map<&Addr, u64> = Map::new("sanguo_ai_date");
pub const AI_BATTLE_STATS: Map<&Addr, AiStats> = Map::new("sanguo_ai_stats");

pub const PLAYER_BATTLE_ORDER: Map<&Addr, Vec<String>> = Map::new("sanguo_battle_order");

pub const FRAGMENTS: Map<&Addr, Fragments> = Map::new("sanguo_fragments");

// （提案/投票三件套已删除，见上方「提案」区注记；
//  PROPOSAL_DEPOSITS 收敛为 PENDING_REFUNDS，migrate M3 仅记账转帐）

pub const PVP_MATCHES: Map<&str, PvpMatch> = Map::new("sanguo_pvp");
pub const ROYALE_MATCHES: Map<&str, RoyaleMatch> = Map::new("sanguo_royale");

// ============================================================================
// 🀄 老三国合约资产迁移
//
// 三张表各司其职，缺一不可：
//   1. MIGRATION_WHITELIST：管理员放行的**老合约**地址。
//      没有它，任何人都能部署一个恶意合约伪造卡牌然后调 SanguoMigrateFromOld
//      无限刷卡（ newcomers 凭空造出 ★5 传说卡）。
//   2. MIGRATED_PLAYERS：**钱包维度**的一次性标记。
//      老合约的碎片没有链上"已领"标记，重复调用会反复累加，必须本合约侧去重。
//   3. MIGRATED_CARDS：**卡 ID 维度**的一次性标记。
//      防止同一个钱包通过重放/多次调用重复 mint 同一张老卡
//      （配合"老合约已经不再发新卡"的前提，双保险）。
// ============================================================================
pub const MIGRATION_WHITELIST: Map<&Addr, bool> = Map::new("sanguo_mig_wl");
pub const MIGRATED_PLAYERS: Map<&Addr, bool> = Map::new("sanguo_mig_players");
pub const MIGRATED_CARDS: Map<&str, bool> = Map::new("sanguo_mig_cards");

// ===== 常量 =====

pub const SECS_PER_DAY: u64 = 86_400;

/// 卡池各稀有度数量上限默认值（仅作 params.rarity_max 全零时的回退兜底；
/// 运营调整请改 params（SanguoSetParams.rarity_max），无需重编译）
pub const RARITY_MAX: [u64; 4] = [90, 30, 20, 10]; // common / rare / epic / legend（总和 150，2026-10-11 由 90 上调至 150）
/// 各稀有度权重范围 [lo, hi]
pub const RARITY_WEIGHT_RANGE: [(u32, u32); 4] = [
    (35, 45), // common
    (20, 30), // rare
    (10, 15), // epic
    (2, 5),   // legend
];

/// 合成消耗的碎片数量（按稀有度）
pub const CRAFT_COST: [u64; 4] = [30, 60, 150, 400];

/// 分解同名卡返还的碎片数量（按稀有度）
pub const FRAGMENT_FROM_DUPLICATE: [u64; 4] = [5, 10, 20, 50];

/// 升星消耗的碎片数量（按稀有度，替代 TKCC 模式）
pub const STAR_UP_FRAGMENT_COST: [u64; 4] = [50, 100, 200, 500];

/// 普通升级（消耗碎片）每级属性增益（params.upgrade_frag_atk/def/max_level = 0 时回退）
pub const UPGRADE_FRAG_ATK: u32 = 3;
pub const UPGRADE_FRAG_DEF: u32 = 2;
pub const UPGRADE_FRAG_MAX_LEVEL: u8 = 10;

/// 升星属性加成默认值（params.upgrade_atk_boost / upgrade_def_boost 全零时回退）
pub const DEFAULT_UPGRADE_ATK_BOOST: [u32; 4] = [5, 12, 25, 50];
pub const DEFAULT_UPGRADE_DEF_BOOST: [u32; 4] = [3, 8, 18, 35];

/// AI 难度系数（基点），单调递进 `[1.15, 1.25, 1.40, 1.55, 1.70]`
///   index = difficulty - 1，difficulty ∈ 1..5
/// 意义：AI 单卡战力 = round((ai_card.attack + ai_card.defense) * DIFF_BOOST_BP / 10000)
///   —— AI 用卡池真实卡牌，难度只做缩放，因此自然与玩家出战卡战力匹配。
///   难度 1 也加 ×1.15 加成，消除"低难零加成套利"。
/// ⚠️ 本 const 仅作 params.diff_boost_bp 全零时的回退默认值；运营调整请改
///    params（SanguoSetParams），无需重编译（V2 参数化，裁定 6.2）。
pub const DIFF_BOOST_BP: [u64; 5] = [11500, 12500, 14000, 15500, 17000];

/// AI 战力地板（基点），随难度递增 `[85%, 90%, 93%, 96%, 99%]`。
///   index = difficulty - 1。ai_power = max(base_ai_power, 玩家单卡战力 * floor / 10000)。
/// 🟢 审计 H2 修复（2026-10-11）：旧版恒定 90% 地板导致「低难度与高难度同地板」
///   的梯度倒置，强卡玩家在高难度也能套利。地板随难度抬升后，低难度套利空间
///   收窄；残余 farm 行为由每日碎片硬顶（params.daily_fragment_cap()）兜底。
/// ⚠️ 本 const 仅作 params.ai_floor_bp 全零时的回退默认值。
pub const AI_FLOOR_BP: [u64; 5] = [8500, 9000, 9300, 9600, 9900];

/// AI 对战每日限次默认值（params.daily_ai_limit = 0 时回退）
pub const DEFAULT_DAILY_AI_LIMIT: u64 = 20;

// ===== 卡池校验辅助 =====

pub fn validate_card_template_internal(
    rarity_count: &std::collections::BTreeMap<String, u64>,
    rarity: &str,
    weight: u32,
    total_templates: u32,
    rarity_max: &[u64; 4],
) -> Result<(), String> {
    let idx = match Fragments::rarity_index(rarity) {
        Some(i) => i,
        None => return Err(format!("invalid rarity: {}", rarity)),
    };
    if total_templates as u64 >= rarity_max.iter().sum::<u64>() {
        return Err("max cards reached".to_string());
    }
    let current = rarity_count.get(rarity).copied().unwrap_or(0);
    if current >= rarity_max[idx] {
        return Err(format!(
            "rarity {} reached max {}",
            rarity, rarity_max[idx]
        ));
    }
    let (lo, hi) = RARITY_WEIGHT_RANGE[idx];
    if weight < lo || weight > hi {
        return Err(format!("weight for {} must be in [{}, {}]", rarity, lo, hi));
    }
    Ok(())
}

pub fn validate_card_template_uniqueness(
    storage: &dyn Storage,
    id: &str,
) -> Result<(), String> {
    let existing = CARD_TEMPLATES.may_load(storage).map_err(|e| e.to_string())?.unwrap_or_default();
    if existing.iter().any(|t| t.id == id) {
        return Err(format!("template id '{}' already exists", id));
    }
    Ok(())
}

/// 按稀有度把现有模板数汇总成 BTreeMap（用于校验）
pub fn collect_rarity_counts(storage: &dyn Storage) -> std::collections::BTreeMap<String, u64> {
    let mut m = std::collections::BTreeMap::new();
    for r in ["common", "rare", "epic", "legend"].iter() {
        if let Some(c) = RARITY_COUNT.may_load(storage, r).unwrap_or(None) {
            m.insert(r.to_string(), c);
        }
    }
    m
}

/// 分页遍历全部模板
pub fn list_templates(storage: &dyn Storage, start_after: Option<u32>, limit: Option<u32>) -> Vec<CardTemplate> {
    let all = CARD_TEMPLATES.may_load(storage).unwrap_or(None).unwrap_or_default();
    let start = start_after.unwrap_or(0) as usize;
    let lim = limit.unwrap_or(30) as usize;
    all.into_iter().skip(start).take(lim).collect()
}

/// 统计当前各稀有度数量
pub fn count_rarities(storage: &dyn Storage) -> (u64, u64, u64, u64) {
    let c = RARITY_COUNT.may_load(storage, "common").ok().flatten().unwrap_or(0);
    let r = RARITY_COUNT.may_load(storage, "rare").ok().flatten().unwrap_or(0);
    let e = RARITY_COUNT.may_load(storage, "epic").ok().flatten().unwrap_or(0);
    let l = RARITY_COUNT.may_load(storage, "legend").ok().flatten().unwrap_or(0);
    (c, r, e, l)
}

// ============================================================================
// 🎁 赠送系统（Gift System v1）
// ============================================================================

// ===== 卡牌 / 碎片 / 皮肤 冷静期 =====

/// card_id → 最近一次赠送时间。
/// - 缺失：拒绝（暴露 init 漏调）
/// - 0    ：迁移卡 / 早期卡，立即可送
/// - >0   ：从该时刻起算冷静期
pub const CARD_LAST_GIFT: Map<&str, u64> = Map::new("sg_clg");

/// (player, rarity) → 最近一次获得该稀有度碎片的时间
pub const FRAG_ACQUIRED: Map<(&Addr, &str), u64> = Map::new("sg_fa");

/// (player, skin_id) → 最近一次获得该皮肤的时间
pub const SKIN_LAST_ACQUIRED: Map<(&Addr, &str), u64> = Map::new("sg_sla");

// ===== 日限额（三类资产共享） =====

pub const GIFT_SENT_TODAY: Map<(&Addr, u64), u32> = Map::new("sg_gst");
pub const GIFT_RECV_TODAY: Map<(&Addr, u64), u32> = Map::new("sg_grt");

// ===== 关系 =====

/// 首次同场 / 最近互动。
/// `last_at` 每天最多刷新一次，其他情况零写入。
#[cw_serde]
pub struct PairInfo {
    pub first_at: u64,
    pub last_at: u64,
}

pub const PAIR_INFO: Map<(&Addr, &Addr), PairInfo> = Map::new("sg_pair");

// ===== 账号 / 管理 =====

pub const ACCT_FIRST_SEEN: Map<&Addr, u64> = Map::new("sg_afs");
/// 全局封禁（T8：由 GIFT_BANNED 泛化而来，**存储 key 不变** "sg_gban"，
/// 零迁移；语义从「禁赠送」扩为「全局封禁」，所有玩法入口前置检查）
pub const BANNED_PLAYERS: Map<&Addr, bool> = Map::new("sg_gban");

// ===== 常量 =====

pub const GIFT_HOLDING_SECS: u64 = 30 * SECS_PER_DAY;
pub const GIFT_LEGEND_HOLDING_SECS: u64 = 7 * SECS_PER_DAY;
pub const GIFT_PAIR_AGE_SECS: u64 = 30 * SECS_PER_DAY;
pub const GIFT_ACCT_AGE_SECS: u64 = 30 * SECS_PER_DAY;
pub const GIFT_PAIR_IDLE_EXPIRE_SECS: u64 = 180 * SECS_PER_DAY;
pub const GIFT_PAIR_REFRESH_INTERVAL: u64 = SECS_PER_DAY;

pub const GIFT_DAILY_SENT: u32 = 3;
pub const GIFT_DAILY_RECV: u32 = 5;
pub const GIFT_FRAG_CAP: u64 = 30;
