// ============================================================================
// 技能表（60 个，编译期常量 &'static）
//
// 数据源：《三国卡牌V2设计终稿》文档 2（终稿，补 team_wide / max_stacks / target）。
// 写法依据裁定书 E7：必须 `&'static`（String/Vec 不是 const fn，原文档写法不可编译）。
// 查找依据裁定书 E6：skill_id 索引化（match 编译期决策树，禁运行时线性查表）。
//
// bp 单位：+1000 = +10%。target=Opponent 表示减益作用于对手攻防。
// 每张卡 1 个技能，3★ 解锁（解锁校验在 exec 层，引擎只读 skill_id）。
// ============================================================================

use cosmwasm_schema::cw_serde;

/// 技能触发时机（设计终稿文档 1 枚举，终稿锁定）
#[cw_serde]
pub enum SkillTrigger {
    /// 整场比赛开始时
    OnBattleStart,
    /// 每组对决开始
    OnGroupStart,
    /// 赢一组后，作用于下一组
    OnWinGroup,
    /// 输一组后，作用于下一组
    OnLoseGroup,
    /// 己方该卡预战力低于对位卡时（按步骤 3 预战力判定一次，S8）
    OnLowPower,
    /// 己方该卡预战力高于对位卡时（按步骤 3 预战力判定一次，S8）
    OnHighPower,
}

/// 技能作用对象
#[cw_serde]
pub enum SkillTarget {
    /// 作用于己方
    Self_,
    /// 减益作用于对手
    Opponent,
}

/// 技能效果结构（设计终稿文档 1，S1 终稿锁定）
#[cw_serde]
pub struct SkillEffect {
    pub trigger: SkillTrigger,
    /// 攻击加成基点（可负；对手减益时为负值）
    pub attack_bp: i32,
    /// 防御加成基点（可负；对手减益时为负值）
    pub defense_bp: i32,
    /// 战力(总 base)加成基点：base × (1 + min(Σ生效 power_flat_bp, cap)/10000)，
    /// 加法叠加且 Σ≤cap 封顶（S6+，cap 默认 3000）
    pub power_flat_bp: i32,
    /// 是否全队（team_wide 作用于全队，否则仅本卡）
    pub team_wide: bool,
    /// 最大叠层（OnWinGroup/OnLoseGroup 累计层数上限；1 = 不叠）
    pub max_stacks: u8,
    /// Self = 己方加成 / Opponent = 对手减益
    pub target: SkillTarget,
}

// ---------------------------------------------------------------------------
// 传奇卡技能（10 张）
// ---------------------------------------------------------------------------
pub const SKILL_JIAN_XIONG: SkillEffect = SkillEffect {
    trigger: SkillTrigger::OnWinGroup, attack_bp: 1800, defense_bp: 0, power_flat_bp: 0,
    team_wide: false, max_stacks: 1, target: SkillTarget::Self_,
};
pub const SKILL_KONG_CHENG: SkillEffect = SkillEffect {
    trigger: SkillTrigger::OnLowPower, attack_bp: 0, defense_bp: 3500, power_flat_bp: 0,
    team_wide: false, max_stacks: 1, target: SkillTarget::Self_,
};
pub const SKILL_WU_SHENG: SkillEffect = SkillEffect {
    trigger: SkillTrigger::OnBattleStart, attack_bp: 2800, defense_bp: 0, power_flat_bp: 0,
    team_wide: false, max_stacks: 1, target: SkillTarget::Self_,
};
pub const SKILL_LONG_DAN: SkillEffect = SkillEffect {
    trigger: SkillTrigger::OnLoseGroup, attack_bp: 1800, defense_bp: 0, power_flat_bp: 0,
    team_wide: false, max_stacks: 2, target: SkillTarget::Self_,
};
pub const SKILL_WU_SHUANG: SkillEffect = SkillEffect {
    trigger: SkillTrigger::OnHighPower, attack_bp: 2200, defense_bp: 0, power_flat_bp: 0,
    team_wide: false, max_stacks: 1, target: SkillTarget::Self_,
};
pub const SKILL_ZHONG_HU: SkillEffect = SkillEffect {
    trigger: SkillTrigger::OnLowPower, attack_bp: 0, defense_bp: 3000, power_flat_bp: 0,
    team_wide: false, max_stacks: 1, target: SkillTarget::Self_,
};
pub const SKILL_REN_DE: SkillEffect = SkillEffect {
    trigger: SkillTrigger::OnBattleStart, attack_bp: 0, defense_bp: 1800, power_flat_bp: 0,
    team_wide: true, max_stacks: 1, target: SkillTarget::Self_,
};
pub const SKILL_ZHI_HENG: SkillEffect = SkillEffect {
    trigger: SkillTrigger::OnWinGroup, attack_bp: 0, defense_bp: 0, power_flat_bp: 1000,
    team_wide: true, max_stacks: 2, target: SkillTarget::Self_,
};
pub const SKILL_JIANG_DONG_MENG_HU: SkillEffect = SkillEffect {
    trigger: SkillTrigger::OnBattleStart, attack_bp: 2200, defense_bp: 0, power_flat_bp: 0,
    team_wide: false, max_stacks: 1, target: SkillTarget::Self_,
};
pub const SKILL_BAO_NUE: SkillEffect = SkillEffect {
    trigger: SkillTrigger::OnWinGroup, attack_bp: 0, defense_bp: -1800, power_flat_bp: 0,
    team_wide: false, max_stacks: 1, target: SkillTarget::Opponent,
};

// ---------------------------------------------------------------------------
// 史诗卡技能（16 张）
// ---------------------------------------------------------------------------
pub const SKILL_PAO_XIAO: SkillEffect = SkillEffect {
    trigger: SkillTrigger::OnBattleStart, attack_bp: 2200, defense_bp: -800, power_flat_bp: 0,
    team_wide: false, max_stacks: 1, target: SkillTarget::Self_,
};
pub const SKILL_XI_LIANG_TIE_QI: SkillEffect = SkillEffect {
    trigger: SkillTrigger::OnBattleStart, attack_bp: 1600, defense_bp: 0, power_flat_bp: 0,
    team_wide: false, max_stacks: 1, target: SkillTarget::Self_,
};
pub const SKILL_BAI_BU_CHUAN_YANG: SkillEffect = SkillEffect {
    trigger: SkillTrigger::OnHighPower, attack_bp: 2000, defense_bp: 0, power_flat_bp: 0,
    team_wide: false, max_stacks: 1, target: SkillTarget::Self_,
};
pub const SKILL_QI_LIN: SkillEffect = SkillEffect {
    trigger: SkillTrigger::OnLoseGroup, attack_bp: 1600, defense_bp: 1600, power_flat_bp: 0,
    team_wide: false, max_stacks: 1, target: SkillTarget::Self_,
};
pub const SKILL_HUO_GONG: SkillEffect = SkillEffect {
    trigger: SkillTrigger::OnWinGroup, attack_bp: 0, defense_bp: -1400, power_flat_bp: 0,
    team_wide: false, max_stacks: 1, target: SkillTarget::Opponent,
};
pub const SKILL_LIAN_YING: SkillEffect = SkillEffect {
    trigger: SkillTrigger::OnWinGroup, attack_bp: 0, defense_bp: 0, power_flat_bp: 1100,
    team_wide: false, max_stacks: 1, target: SkillTarget::Self_,
};
pub const SKILL_BA_SHI_DAN_JING: SkillEffect = SkillEffect {
    trigger: SkillTrigger::OnLoseGroup, attack_bp: 2400, defense_bp: 0, power_flat_bp: 0,
    team_wide: false, max_stacks: 1, target: SkillTarget::Self_,
};
pub const SKILL_WEI_ZHEN_XIAO_YAO_JIN: SkillEffect = SkillEffect {
    trigger: SkillTrigger::OnLowPower, attack_bp: 2200, defense_bp: 0, power_flat_bp: 0,
    team_wide: false, max_stacks: 1, target: SkillTarget::Self_,
};
pub const SKILL_GUI_CAI: SkillEffect = SkillEffect {
    trigger: SkillTrigger::OnBattleStart, attack_bp: 0, defense_bp: 0, power_flat_bp: 1200,
    team_wide: true, max_stacks: 1, target: SkillTarget::Self_,
};
pub const SKILL_HU_CHI: SkillEffect = SkillEffect {
    trigger: SkillTrigger::OnHighPower, attack_bp: 0, defense_bp: 2200, power_flat_bp: 0,
    team_wide: false, max_stacks: 1, target: SkillTarget::Self_,
};
pub const SKILL_QIAO_BIAN: SkillEffect = SkillEffect {
    trigger: SkillTrigger::OnGroupStart, attack_bp: 0, defense_bp: 0, power_flat_bp: 900,
    team_wide: false, max_stacks: 1, target: SkillTarget::Self_,
};
pub const SKILL_KU_ROU: SkillEffect = SkillEffect {
    trigger: SkillTrigger::OnLoseGroup, attack_bp: 2800, defense_bp: -800, power_flat_bp: 0,
    team_wide: false, max_stacks: 1, target: SkillTarget::Self_,
};
pub const SKILL_GONG_YAO_JI: SkillEffect = SkillEffect {
    trigger: SkillTrigger::OnBattleStart, attack_bp: 1800, defense_bp: 0, power_flat_bp: 0,
    team_wide: false, max_stacks: 1, target: SkillTarget::Self_,
};
pub const SKILL_LIN_WEI: SkillEffect = SkillEffect {
    trigger: SkillTrigger::OnLowPower, attack_bp: 0, defense_bp: 0, power_flat_bp: 1200,
    team_wide: true, max_stacks: 1, target: SkillTarget::Self_,
};
pub const SKILL_SHEN_YI: SkillEffect = SkillEffect {
    trigger: SkillTrigger::OnWinGroup, attack_bp: 0, defense_bp: 0, power_flat_bp: 1400,
    team_wide: false, max_stacks: 1, target: SkillTarget::Self_,
};
pub const SKILL_HE_BEI_TING_ZHU: SkillEffect = SkillEffect {
    trigger: SkillTrigger::OnBattleStart, attack_bp: 2500, defense_bp: 0, power_flat_bp: 0,
    team_wide: false, max_stacks: 1, target: SkillTarget::Self_,
};

// ---------------------------------------------------------------------------
// 稀有卡技能（18 张）
// ---------------------------------------------------------------------------
pub const SKILL_QI_MOU: SkillEffect = SkillEffect {
    trigger: SkillTrigger::OnLowPower, attack_bp: 1800, defense_bp: 0, power_flat_bp: 0,
    team_wide: false, max_stacks: 1, target: SkillTarget::Self_,
};
pub const SKILL_LIAN_HUAN: SkillEffect = SkillEffect {
    trigger: SkillTrigger::OnBattleStart, attack_bp: 0, defense_bp: 0, power_flat_bp: 900,
    team_wide: true, max_stacks: 1, target: SkillTarget::Self_,
};
pub const SKILL_QI_HUA: SkillEffect = SkillEffect {
    trigger: SkillTrigger::OnWinGroup, attack_bp: 1400, defense_bp: 0, power_flat_bp: 0,
    team_wide: false, max_stacks: 1, target: SkillTarget::Self_,
};
pub const SKILL_JIN_FAN: SkillEffect = SkillEffect {
    trigger: SkillTrigger::OnBattleStart, attack_bp: 1400, defense_bp: 0, power_flat_bp: 0,
    team_wide: false, max_stacks: 1, target: SkillTarget::Self_,
};
pub const SKILL_SHEN_SHE: SkillEffect = SkillEffect {
    trigger: SkillTrigger::OnHighPower, attack_bp: 1600, defense_bp: 0, power_flat_bp: 0,
    team_wide: false, max_stacks: 1, target: SkillTarget::Self_,
};
pub const SKILL_XIAO_BA_WANG: SkillEffect = SkillEffect {
    trigger: SkillTrigger::OnBattleStart, attack_bp: 1600, defense_bp: 0, power_flat_bp: 0,
    team_wide: false, max_stacks: 1, target: SkillTarget::Self_,
};
pub const SKILL_BAI_YI_DU_JIANG: SkillEffect = SkillEffect {
    trigger: SkillTrigger::OnLowPower, attack_bp: 0, defense_bp: 0, power_flat_bp: 1300,
    team_wide: false, max_stacks: 1, target: SkillTarget::Self_,
};
pub const SKILL_TOU_DU_YIN_PING: SkillEffect = SkillEffect {
    trigger: SkillTrigger::OnLowPower, attack_bp: 0, defense_bp: 0, power_flat_bp: 1100,
    team_wide: true, max_stacks: 1, target: SkillTarget::Self_,
};
pub const SKILL_WANG_ZUO: SkillEffect = SkillEffect {
    trigger: SkillTrigger::OnBattleStart, attack_bp: 0, defense_bp: 1400, power_flat_bp: 0,
    team_wide: true, max_stacks: 1, target: SkillTarget::Self_,
};
pub const SKILL_LUO_SHEN: SkillEffect = SkillEffect {
    trigger: SkillTrigger::OnBattleStart, attack_bp: 0, defense_bp: 1200, power_flat_bp: 0,
    team_wide: true, max_stacks: 1, target: SkillTarget::Self_,
};
pub const SKILL_GUO_SE: SkillEffect = SkillEffect {
    trigger: SkillTrigger::OnWinGroup, attack_bp: 0, defense_bp: 0, power_flat_bp: 900,
    team_wide: true, max_stacks: 1, target: SkillTarget::Self_,
};
pub const SKILL_TIAN_XIANG: SkillEffect = SkillEffect {
    trigger: SkillTrigger::OnWinGroup, attack_bp: 0, defense_bp: 1200, power_flat_bp: 0,
    team_wide: true, max_stacks: 1, target: SkillTarget::Self_,
};
pub const SKILL_BU_QU: SkillEffect = SkillEffect {
    trigger: SkillTrigger::OnLoseGroup, attack_bp: 0, defense_bp: 2400, power_flat_bp: 0,
    team_wide: false, max_stacks: 1, target: SkillTarget::Self_,
};
pub const SKILL_JI_GUAN: SkillEffect = SkillEffect {
    trigger: SkillTrigger::OnBattleStart, attack_bp: 0, defense_bp: 1200, power_flat_bp: 0,
    team_wide: true, max_stacks: 1, target: SkillTarget::Self_,
};
pub const SKILL_AN_GUO: SkillEffect = SkillEffect {
    trigger: SkillTrigger::OnWinGroup, attack_bp: 1500, defense_bp: 0, power_flat_bp: 0,
    team_wide: false, max_stacks: 1, target: SkillTarget::Self_,
};
pub const SKILL_HU_SI: SkillEffect = SkillEffect {
    trigger: SkillTrigger::OnBattleStart, attack_bp: 1400, defense_bp: 0, power_flat_bp: 0,
    team_wide: false, max_stacks: 1, target: SkillTarget::Self_,
};
pub const SKILL_GONG_TAI: SkillEffect = SkillEffect {
    trigger: SkillTrigger::OnLowPower, attack_bp: 1600, defense_bp: 0, power_flat_bp: 0,
    team_wide: false, max_stacks: 1, target: SkillTarget::Self_,
};
pub const SKILL_BI_YUE: SkillEffect = SkillEffect {
    trigger: SkillTrigger::OnWinGroup, attack_bp: -1200, defense_bp: 0, power_flat_bp: 0,
    team_wide: false, max_stacks: 1, target: SkillTarget::Opponent,
};

// ---------------------------------------------------------------------------
// 普通卡技能（16 张）
// ---------------------------------------------------------------------------
pub const SKILL_XIAN_FENG: SkillEffect = SkillEffect {
    trigger: SkillTrigger::OnBattleStart, attack_bp: 1000, defense_bp: 0, power_flat_bp: 0,
    team_wide: false, max_stacks: 1, target: SkillTarget::Self_,
};
pub const SKILL_BIAN_SHI: SkillEffect = SkillEffect {
    trigger: SkillTrigger::OnBattleStart, attack_bp: 0, defense_bp: 0, power_flat_bp: 600,
    team_wide: true, max_stacks: 1, target: SkillTarget::Self_,
};
pub const SKILL_SHI_MING: SkillEffect = SkillEffect {
    trigger: SkillTrigger::OnWinGroup, attack_bp: 0, defense_bp: 0, power_flat_bp: 700,
    team_wide: true, max_stacks: 1, target: SkillTarget::Self_,
};
pub const SKILL_FU_SHANG: SkillEffect = SkillEffect {
    trigger: SkillTrigger::OnBattleStart, attack_bp: 0, defense_bp: 900, power_flat_bp: 0,
    team_wide: true, max_stacks: 1, target: SkillTarget::Self_,
};
pub const SKILL_DUN_HOU: SkillEffect = SkillEffect {
    trigger: SkillTrigger::OnBattleStart, attack_bp: 0, defense_bp: 800, power_flat_bp: 0,
    team_wide: true, max_stacks: 1, target: SkillTarget::Self_,
};
pub const SKILL_LIAN_MENG: SkillEffect = SkillEffect {
    trigger: SkillTrigger::OnWinGroup, attack_bp: 0, defense_bp: 900, power_flat_bp: 0,
    team_wide: true, max_stacks: 1, target: SkillTarget::Self_,
};
pub const SKILL_MOU_ZHU: SkillEffect = SkillEffect {
    trigger: SkillTrigger::OnLowPower, attack_bp: 0, defense_bp: 1200, power_flat_bp: 0,
    team_wide: false, max_stacks: 1, target: SkillTarget::Self_,
};
pub const SKILL_DU_SHI: SkillEffect = SkillEffect {
    trigger: SkillTrigger::OnWinGroup, attack_bp: 0, defense_bp: -1000, power_flat_bp: 0,
    team_wide: false, max_stacks: 1, target: SkillTarget::Opponent,
};
pub const SKILL_PIAO_QI: SkillEffect = SkillEffect {
    trigger: SkillTrigger::OnBattleStart, attack_bp: 1200, defense_bp: 0, power_flat_bp: 0,
    team_wide: false, max_stacks: 1, target: SkillTarget::Self_,
};
pub const SKILL_MOU_LUAN: SkillEffect = SkillEffect {
    trigger: SkillTrigger::OnLowPower, attack_bp: 1200, defense_bp: 0, power_flat_bp: 0,
    team_wide: false, max_stacks: 1, target: SkillTarget::Self_,
};
pub const SKILL_WEN_HOU: SkillEffect = SkillEffect {
    trigger: SkillTrigger::OnBattleStart, attack_bp: 0, defense_bp: 900, power_flat_bp: 0,
    team_wide: true, max_stacks: 1, target: SkillTarget::Self_,
};
pub const SKILL_AN_GUO_2: SkillEffect = SkillEffect {
    trigger: SkillTrigger::OnBattleStart, attack_bp: 1000, defense_bp: 0, power_flat_bp: 0,
    team_wide: false, max_stacks: 1, target: SkillTarget::Self_,
};
pub const SKILL_TAN_ZHI: SkillEffect = SkillEffect {
    trigger: SkillTrigger::OnBattleStart, attack_bp: 1100, defense_bp: 0, power_flat_bp: 0,
    team_wide: false, max_stacks: 1, target: SkillTarget::Self_,
};
pub const SKILL_ZHONG_YONG: SkillEffect = SkillEffect {
    trigger: SkillTrigger::OnBattleStart, attack_bp: 0, defense_bp: 1400, power_flat_bp: 0,
    team_wide: false, max_stacks: 1, target: SkillTarget::Self_,
};
pub const SKILL_GANG_ZHI: SkillEffect = SkillEffect {
    trigger: SkillTrigger::OnLowPower, attack_bp: 0, defense_bp: 0, power_flat_bp: 900,
    team_wide: true, max_stacks: 1, target: SkillTarget::Self_,
};
pub const SKILL_ZHENG_NAN: SkillEffect = SkillEffect {
    trigger: SkillTrigger::OnBattleStart, attack_bp: 0, defense_bp: 800, power_flat_bp: 0,
    team_wide: true, max_stacks: 1, target: SkillTarget::Self_,
};

/// 全量技能表（60 个，按稀有度分组排列，供测试/校验遍历用）。
/// 运行时查找一律走 `find_skill`（match 编译期索引）。
pub const SKILLS: &[(&str, &SkillEffect)] = &[
    // 传奇
    ("jian_xiong", &SKILL_JIAN_XIONG),
    ("kong_cheng", &SKILL_KONG_CHENG),
    ("wu_sheng", &SKILL_WU_SHENG),
    ("long_dan", &SKILL_LONG_DAN),
    ("wu_shuang", &SKILL_WU_SHUANG),
    ("zhong_hu", &SKILL_ZHONG_HU),
    ("ren_de", &SKILL_REN_DE),
    ("zhi_heng", &SKILL_ZHI_HENG),
    ("jiang_dong_meng_hu", &SKILL_JIANG_DONG_MENG_HU),
    ("bao_nue", &SKILL_BAO_NUE),
    // 史诗
    ("pao_xiao", &SKILL_PAO_XIAO),
    ("xi_liang_tie_qi", &SKILL_XI_LIANG_TIE_QI),
    ("bai_bu_chuan_yang", &SKILL_BAI_BU_CHUAN_YANG),
    ("qi_lin", &SKILL_QI_LIN),
    ("huo_gong", &SKILL_HUO_GONG),
    ("lian_ying", &SKILL_LIAN_YING),
    ("ba_shi_dan_jing", &SKILL_BA_SHI_DAN_JING),
    ("wei_zhen_xiao_yao_jin", &SKILL_WEI_ZHEN_XIAO_YAO_JIN),
    ("gui_cai", &SKILL_GUI_CAI),
    ("hu_chi", &SKILL_HU_CHI),
    ("qiao_bian", &SKILL_QIAO_BIAN),
    ("ku_rou", &SKILL_KU_ROU),
    ("gong_yao_ji", &SKILL_GONG_YAO_JI),
    ("lin_wei", &SKILL_LIN_WEI),
    ("shen_yi", &SKILL_SHEN_YI),
    ("he_bei_ting_zhu", &SKILL_HE_BEI_TING_ZHU),
    // 稀有
    ("qi_mou", &SKILL_QI_MOU),
    ("lian_huan", &SKILL_LIAN_HUAN),
    ("qi_hua", &SKILL_QI_HUA),
    ("jin_fan", &SKILL_JIN_FAN),
    ("shen_she", &SKILL_SHEN_SHE),
    ("xiao_ba_wang", &SKILL_XIAO_BA_WANG),
    ("bai_yi_du_jiang", &SKILL_BAI_YI_DU_JIANG),
    ("tou_du_yin_ping", &SKILL_TOU_DU_YIN_PING),
    ("wang_zuo", &SKILL_WANG_ZUO),
    ("luo_shen", &SKILL_LUO_SHEN),
    ("guo_se", &SKILL_GUO_SE),
    ("tian_xiang", &SKILL_TIAN_XIANG),
    ("bu_qu", &SKILL_BU_QU),
    ("ji_guan", &SKILL_JI_GUAN),
    ("an_guo", &SKILL_AN_GUO),
    ("hu_si", &SKILL_HU_SI),
    ("gong_tai", &SKILL_GONG_TAI),
    ("bi_yue", &SKILL_BI_YUE),
    // 普通
    ("xian_feng", &SKILL_XIAN_FENG),
    ("bian_shi", &SKILL_BIAN_SHI),
    ("shi_ming", &SKILL_SHI_MING),
    ("fu_shang", &SKILL_FU_SHANG),
    ("dun_hou", &SKILL_DUN_HOU),
    ("lian_meng", &SKILL_LIAN_MENG),
    ("mou_zhu", &SKILL_MOU_ZHU),
    ("du_shi", &SKILL_DU_SHI),
    ("piao_qi", &SKILL_PIAO_QI),
    ("mou_luan", &SKILL_MOU_LUAN),
    ("wen_hou", &SKILL_WEN_HOU),
    ("an_guo_2", &SKILL_AN_GUO_2),
    ("tan_zhi", &SKILL_TAN_ZHI),
    ("zhong_yong", &SKILL_ZHONG_YONG),
    ("gang_zhi", &SKILL_GANG_ZHI),
    ("zheng_nan", &SKILL_ZHENG_NAN),
];

/// 按 skill_id 查技能（None = 无此技能，引擎按无技能处理）。
/// match 由编译器编为决策树，等效编译期索引（裁定 E6：禁线性查表）。
pub fn find_skill(skill_id: &str) -> Option<&'static SkillEffect> {
    let s: &SkillEffect = match skill_id {
        "jian_xiong" => &SKILL_JIAN_XIONG,
        "kong_cheng" => &SKILL_KONG_CHENG,
        "wu_sheng" => &SKILL_WU_SHENG,
        "long_dan" => &SKILL_LONG_DAN,
        "wu_shuang" => &SKILL_WU_SHUANG,
        "zhong_hu" => &SKILL_ZHONG_HU,
        "ren_de" => &SKILL_REN_DE,
        "zhi_heng" => &SKILL_ZHI_HENG,
        "jiang_dong_meng_hu" => &SKILL_JIANG_DONG_MENG_HU,
        "bao_nue" => &SKILL_BAO_NUE,
        "pao_xiao" => &SKILL_PAO_XIAO,
        "xi_liang_tie_qi" => &SKILL_XI_LIANG_TIE_QI,
        "bai_bu_chuan_yang" => &SKILL_BAI_BU_CHUAN_YANG,
        "qi_lin" => &SKILL_QI_LIN,
        "huo_gong" => &SKILL_HUO_GONG,
        "lian_ying" => &SKILL_LIAN_YING,
        "ba_shi_dan_jing" => &SKILL_BA_SHI_DAN_JING,
        "wei_zhen_xiao_yao_jin" => &SKILL_WEI_ZHEN_XIAO_YAO_JIN,
        "gui_cai" => &SKILL_GUI_CAI,
        "hu_chi" => &SKILL_HU_CHI,
        "qiao_bian" => &SKILL_QIAO_BIAN,
        "ku_rou" => &SKILL_KU_ROU,
        "gong_yao_ji" => &SKILL_GONG_YAO_JI,
        "lin_wei" => &SKILL_LIN_WEI,
        "shen_yi" => &SKILL_SHEN_YI,
        "he_bei_ting_zhu" => &SKILL_HE_BEI_TING_ZHU,
        "qi_mou" => &SKILL_QI_MOU,
        "lian_huan" => &SKILL_LIAN_HUAN,
        "qi_hua" => &SKILL_QI_HUA,
        "jin_fan" => &SKILL_JIN_FAN,
        "shen_she" => &SKILL_SHEN_SHE,
        "xiao_ba_wang" => &SKILL_XIAO_BA_WANG,
        "bai_yi_du_jiang" => &SKILL_BAI_YI_DU_JIANG,
        "tou_du_yin_ping" => &SKILL_TOU_DU_YIN_PING,
        "wang_zuo" => &SKILL_WANG_ZUO,
        "luo_shen" => &SKILL_LUO_SHEN,
        "guo_se" => &SKILL_GUO_SE,
        "tian_xiang" => &SKILL_TIAN_XIANG,
        "bu_qu" => &SKILL_BU_QU,
        "ji_guan" => &SKILL_JI_GUAN,
        "an_guo" => &SKILL_AN_GUO,
        "hu_si" => &SKILL_HU_SI,
        "gong_tai" => &SKILL_GONG_TAI,
        "bi_yue" => &SKILL_BI_YUE,
        "xian_feng" => &SKILL_XIAN_FENG,
        "bian_shi" => &SKILL_BIAN_SHI,
        "shi_ming" => &SKILL_SHI_MING,
        "fu_shang" => &SKILL_FU_SHANG,
        "dun_hou" => &SKILL_DUN_HOU,
        "lian_meng" => &SKILL_LIAN_MENG,
        "mou_zhu" => &SKILL_MOU_ZHU,
        "du_shi" => &SKILL_DU_SHI,
        "piao_qi" => &SKILL_PIAO_QI,
        "mou_luan" => &SKILL_MOU_LUAN,
        "wen_hou" => &SKILL_WEN_HOU,
        "an_guo_2" => &SKILL_AN_GUO_2,
        "tan_zhi" => &SKILL_TAN_ZHI,
        "zhong_yong" => &SKILL_ZHONG_YONG,
        "gang_zhi" => &SKILL_GANG_ZHI,
        "zheng_nan" => &SKILL_ZHENG_NAN,
        _ => return None,
    };
    Some(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skills_count_60() {
        assert_eq!(SKILLS.len(), 60);
    }

    #[test]
    fn find_skill_all_present() {
        // ⚠️ 2026-10-21 修正（公开模板化）：原断言比较的是
        //    `*expect as *const SkillEffect == got as *const SkillEffect`，
        //    即**静态 const 的裸指针地址**。这依赖编译器是否把 const
        //    提升为同一份静态分配（const promotion / codegen unit 划分），
        //    在不同工具链、优化等级与 target 下结果会漂移 —— 实测在
        //    Rust 1.74 + 依赖版本不同的组合下会失败。
        //    → 改为**值比较**：`SkillEffect` 派生了 `PartialEq`（`#[cw_serde]`），
        //      语义等价（验证 find_skill 命中了正确的技能定义）且跨环境稳定。
        for (id, expect) in SKILLS.iter() {
            let got = find_skill(id).expect(id);
            assert_eq!(**expect, *got, "技能 {id} 的 find_skill 命中了错误的定义");
        }
        assert!(find_skill("not_exist").is_none());
    }

    #[test]
    fn dual_effect_skills() {
        // 张飞：攻击+2200/防御-800
        assert_eq!(SKILL_PAO_XIAO.attack_bp, 2200);
        assert_eq!(SKILL_PAO_XIAO.defense_bp, -800);
        // 姜维：攻防各+1600
        assert_eq!(SKILL_QI_LIN.attack_bp, 1600);
        assert_eq!(SKILL_QI_LIN.defense_bp, 1600);
        // 董卓：对手防御-1800
        assert!(matches!(SKILL_BAO_NUE.target, SkillTarget::Opponent));
        assert_eq!(SKILL_BAO_NUE.defense_bp, -1800);
        // 孙权：战力+1000 team_wide 2 层
        assert_eq!(SKILL_ZHI_HENG.power_flat_bp, 1000);
        assert!(SKILL_ZHI_HENG.team_wide);
        assert_eq!(SKILL_ZHI_HENG.max_stacks, 2);
    }
}
