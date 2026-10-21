// ============================================================================
// 连携表（23 个，编译期常量 &'static）
//
// 数据源：《三国卡牌V2设计终稿》文档 4（终稿，孤儿补 17 张）。
// 写法依据裁定书 E7：成员必须 `&'static [&'static str]`（Vec 不是 const fn）。
// 规则（S3）：连携不叠加，全队激活连携中仅取单个最高档加成。
// 成员引用卡牌 name 字段（严格一致，已逐条核对 60 张表内存在）。
// ============================================================================

/// 连携加成上限（bp）：最高完整档 1800 + 基数 10000（E3 溢出检查用）
pub const MAX_BOND_BP: u64 = 11800;

/// 连携定义
pub struct BondDef {
    pub id: &'static str,
    pub name: &'static str,
    pub faction: &'static str,
    /// 成员卡名（与 CardTemplate.name 严格一致）
    pub members: &'static [&'static str],
    /// 基础档所需张数
    pub base_count: usize,
    /// 基础档加成（bp）
    pub base_bp: u64,
    /// 完整档所需张数（None = 无完整档）
    pub full_count: Option<usize>,
    /// 完整档加成（bp）
    pub full_bp: Option<u64>,
}

// ---------------------------------------------------------------------------
// 23 个连携（按设计终稿文档 4 顺序）
// ---------------------------------------------------------------------------
pub const BOND_WUHU_SHANGJIANG: BondDef = BondDef {
    id: "wuhu_shangjiang", name: "五虎上将", faction: "shu",
    members: &["关羽", "张飞", "赵云", "马超", "黄忠"],
    base_count: 3, base_bp: 800, full_count: Some(5), full_bp: Some(1800),
};
pub const BOND_TAOYUAN_JIEYI: BondDef = BondDef {
    id: "taoyuan_jieyi", name: "桃园结义", faction: "shu",
    members: &["刘备", "关羽", "张飞"],
    base_count: 2, base_bp: 600, full_count: Some(3), full_bp: Some(1500),
};
pub const BOND_WOLONG_FENGCHU: BondDef = BondDef {
    id: "wolong_fengchu", name: "卧龙凤雏", faction: "shu",
    members: &["诸葛亮", "庞统"],
    base_count: 2, base_bp: 1000, full_count: None, full_bp: None,
};
pub const BOND_SHUHAN_SANJIE: BondDef = BondDef {
    id: "shuhan_sanjie", name: "蜀汉三杰", faction: "shu",
    members: &["诸葛亮", "关羽", "张飞"],
    base_count: 2, base_bp: 600, full_count: Some(3), full_bp: Some(1400),
};
pub const BOND_GUANYU_FUZI: BondDef = BondDef {
    id: "guanyu_fuzi", name: "关羽父子", faction: "shu",
    members: &["关羽", "关平", "关兴"],
    base_count: 2, base_bp: 600, full_count: Some(3), full_bp: Some(1200),
};
pub const BOND_ZHANGFEI_FUZI: BondDef = BondDef {
    id: "zhangfei_fuzi", name: "张飞父子", faction: "shu",
    members: &["张飞", "张苞"],
    base_count: 2, base_bp: 800, full_count: None, full_bp: None,
};
pub const BOND_SHUHAN_MINGJIANG: BondDef = BondDef {
    id: "shuhan_mingjiang", name: "蜀汉名将", faction: "shu",
    members: &["姜维", "魏延", "马岱", "廖化"],
    base_count: 2, base_bp: 600, full_count: Some(4), full_bp: Some(1200),
};
pub const BOND_WUZI_LIANGJIANG: BondDef = BondDef {
    id: "wuzi_liangjiang", name: "五子良将", faction: "wei",
    members: &["张辽", "张郃"],
    base_count: 2, base_bp: 800, full_count: None, full_bp: None,
};
pub const BOND_CAOWEI_SHUANGBI: BondDef = BondDef {
    id: "caowei_shuangbi", name: "曹魏双璧", faction: "wei",
    members: &["曹操", "司马懿"],
    base_count: 2, base_bp: 1200, full_count: None, full_bp: None,
};
pub const BOND_XUNYU_GUOJIA: BondDef = BondDef {
    id: "xunyu_guojia", name: "荀彧郭嘉", faction: "wei",
    members: &["荀彧", "郭嘉"],
    base_count: 2, base_bp: 1000, full_count: None, full_bp: None,
};
pub const BOND_XUCHU_HUWEI: BondDef = BondDef {
    id: "xuchu_huwei", name: "许褚虎卫", faction: "wei",
    members: &["许褚", "曹操"],
    base_count: 2, base_bp: 800, full_count: None, full_bp: None,
};
pub const BOND_WEIWU_SANJIANG: BondDef = BondDef {
    id: "weiwu_sanjiang", name: "魏武三将", faction: "wei",
    members: &["夏侯惇", "张辽", "许褚"],
    base_count: 2, base_bp: 600, full_count: Some(3), full_bp: Some(1400),
};
pub const BOND_DONGWU_DUDU: BondDef = BondDef {
    id: "dongwu_dudu", name: "东吴都督", faction: "wu",
    members: &["周瑜", "鲁肃", "吕蒙", "陆逊"],
    base_count: 2, base_bp: 600, full_count: Some(4), full_bp: Some(1600),
};
pub const BOND_JIANGDONG_SHUANGBI: BondDef = BondDef {
    id: "jiangdong_shuangbi", name: "江东双璧", faction: "wu",
    members: &["孙策", "周瑜"],
    base_count: 2, base_bp: 1200, full_count: None, full_bp: None,
};
pub const BOND_SUNSHI_SANXIONG: BondDef = BondDef {
    id: "sunshi_sanxiong", name: "孙氏三雄", faction: "wu",
    members: &["孙坚", "孙策", "孙权"],
    base_count: 2, base_bp: 800, full_count: Some(3), full_bp: Some(1600),
};
pub const BOND_ERQIAO: BondDef = BondDef {
    id: "erqiao", name: "二乔", faction: "wu",
    members: &["大乔", "小乔"],
    base_count: 2, base_bp: 1000, full_count: None, full_bp: None,
};
pub const BOND_SUNSHANGXIANG_GONGYAOJI: BondDef = BondDef {
    id: "sunshangxiang_gongyaoji", name: "弓腰姬", faction: "wu",
    members: &["孙尚香", "刘备"],
    base_count: 2, base_bp: 800, full_count: None, full_bp: None,
};
pub const BOND_GANNING_TAISHICI: BondDef = BondDef {
    id: "ganning_taishici", name: "甘宁太史慈", faction: "wu",
    members: &["甘宁", "太史慈"],
    base_count: 2, base_bp: 800, full_count: None, full_bp: None,
};
pub const BOND_HUANGGAI_KUROU: BondDef = BondDef {
    id: "huanggai_kurou", name: "黄盖苦肉", faction: "wu",
    members: &["黄盖", "周瑜"],
    base_count: 2, base_bp: 1000, full_count: None, full_bp: None,
};
pub const BOND_LVBU_DIAOCHAN: BondDef = BondDef {
    id: "lvbu_diaochan", name: "吕布貂蝉", faction: "qun",
    members: &["吕布", "貂蝉"],
    base_count: 2, base_bp: 1200, full_count: None, full_bp: None,
};
pub const BOND_DONGZHUO_LVBU: BondDef = BondDef {
    id: "dongzhuo_lvbu", name: "董卓吕布", faction: "qun",
    members: &["董卓", "吕布"],
    base_count: 2, base_bp: 1000, full_count: None, full_bp: None,
};
pub const BOND_QUNXIONG_ZHULU: BondDef = BondDef {
    id: "qunxiong_zhulu", name: "群雄逐鹿", faction: "qun",
    members: &["吕布", "董卓", "颜良", "华佗"],
    base_count: 2, base_bp: 600, full_count: Some(4), full_bp: Some(1400),
};
pub const BOND_HEBEI_TINGZHU: BondDef = BondDef {
    id: "hebei_tingzhu", name: "河北庭柱", faction: "qun",
    members: &["颜良", "审配", "田丰"],
    base_count: 2, base_bp: 600, full_count: Some(3), full_bp: Some(1200),
};

/// 全量连携表（23 个，供测试/校验遍历用）
pub const BONDS: &[&BondDef] = &[
    &BOND_WUHU_SHANGJIANG,
    &BOND_TAOYUAN_JIEYI,
    &BOND_WOLONG_FENGCHU,
    &BOND_SHUHAN_SANJIE,
    &BOND_GUANYU_FUZI,
    &BOND_ZHANGFEI_FUZI,
    &BOND_SHUHAN_MINGJIANG,
    &BOND_WUZI_LIANGJIANG,
    &BOND_CAOWEI_SHUANGBI,
    &BOND_XUNYU_GUOJIA,
    &BOND_XUCHU_HUWEI,
    &BOND_WEIWU_SANJIANG,
    &BOND_DONGWU_DUDU,
    &BOND_JIANGDONG_SHUANGBI,
    &BOND_SUNSHI_SANXIONG,
    &BOND_ERQIAO,
    &BOND_SUNSHANGXIANG_GONGYAOJI,
    &BOND_GANNING_TAISHICI,
    &BOND_HUANGGAI_KUROU,
    &BOND_LVBU_DIAOCHAN,
    &BOND_DONGZHUO_LVBU,
    &BOND_QUNXIONG_ZHULU,
    &BOND_HEBEI_TINGZHU,
];

/// 按 bond id 查连携定义（None = 无此连携）。
/// match 编译期索引（裁定 E6 同款要求）。
pub fn find_bond(bond_id: &str) -> Option<&'static BondDef> {
    let b: &BondDef = match bond_id {
        "wuhu_shangjiang" => &BOND_WUHU_SHANGJIANG,
        "taoyuan_jieyi" => &BOND_TAOYUAN_JIEYI,
        "wolong_fengchu" => &BOND_WOLONG_FENGCHU,
        "shuhan_sanjie" => &BOND_SHUHAN_SANJIE,
        "guanyu_fuzi" => &BOND_GUANYU_FUZI,
        "zhangfei_fuzi" => &BOND_ZHANGFEI_FUZI,
        "shuhan_mingjiang" => &BOND_SHUHAN_MINGJIANG,
        "wuzi_liangjiang" => &BOND_WUZI_LIANGJIANG,
        "caowei_shuangbi" => &BOND_CAOWEI_SHUANGBI,
        "xunyu_guojia" => &BOND_XUNYU_GUOJIA,
        "xuchu_huwei" => &BOND_XUCHU_HUWEI,
        "weiwu_sanjiang" => &BOND_WEIWU_SANJIANG,
        "dongwu_dudu" => &BOND_DONGWU_DUDU,
        "jiangdong_shuangbi" => &BOND_JIANGDONG_SHUANGBI,
        "sunshi_sanxiong" => &BOND_SUNSHI_SANXIONG,
        "erqiao" => &BOND_ERQIAO,
        "sunshangxiang_gongyaoji" => &BOND_SUNSHANGXIANG_GONGYAOJI,
        "ganning_taishici" => &BOND_GANNING_TAISHICI,
        "huanggai_kurou" => &BOND_HUANGGAI_KUROU,
        "lvbu_diaochan" => &BOND_LVBU_DIAOCHAN,
        "dongzhuo_lvbu" => &BOND_DONGZHUO_LVBU,
        "qunxiong_zhulu" => &BOND_QUNXIONG_ZHULU,
        "hebei_tingzhu" => &BOND_HEBEI_TINGZHU,
        _ => return None,
    };
    Some(b)
}

/// 计算某方 9 张卡的连携系数（bp，10000 = 1.0）。
///
/// 规则（S3 单档封顶）：遍历 23 个连携，统计成员在场张数，
/// 达完整档取完整 bp，否则达基础档取基础 bp；全队只取**单个最高** bp。
/// bond_mult = 10000 + max_bp（整场生效，见设计终稿文档 1 步骤 3 注）。
///
/// gas 注意（E6）：每玩家每次战斗只算一次，结果复用 3 组 × 3 张。
pub fn bond_multiplier_bp(team_names: &[&str]) -> u64 {
    let mut best: u64 = 0;
    for bond in BONDS.iter() {
        let mut count = 0usize;
        for m in bond.members.iter() {
            if team_names.iter().any(|n| n == m) {
                count += 1;
            }
        }
        let bp = if let (Some(fc), Some(fb)) = (bond.full_count, bond.full_bp) {
            if count >= fc {
                fb
            } else if count >= bond.base_count {
                bond.base_bp
            } else {
                0
            }
        } else if count >= bond.base_count {
            bond.base_bp
        } else {
            0
        };
        if bp > best {
            best = bp;
        }
    }
    10000 + best
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bonds_count_23() {
        assert_eq!(BONDS.len(), 23);
    }

    #[test]
    fn find_bond_all_present() {
        for b in BONDS.iter() {
            assert!(find_bond(b.id).is_some());
        }
        assert!(find_bond("nope").is_none());
    }

    #[test]
    fn bond_single_tier_cap() {
        // 五虎上将 5 张全 → 完整档 1800
        let team = ["关羽", "张飞", "赵云", "马超", "黄忠", "x", "y", "z", "w"];
        assert_eq!(bond_multiplier_bp(&team), 11800);
        // 五虎 3 张 → 基础档 800（同时激活关羽父子 2 张 600、桃园 2 张 600，取最高 800）
        let team = ["关羽", "张飞", "赵云", "x", "y", "z", "w", "v", "u"];
        assert_eq!(bond_multiplier_bp(&team), 10800);
        // 无连携 → 10000
        let team = ["a", "b", "c", "d", "e", "f", "g", "h", "i"];
        assert_eq!(bond_multiplier_bp(&team), 10000);
    }

    #[test]
    fn bond_members_exist_in_60_table() {
        // 23 连携全部成员必须落在 60 张卡名集合内（文档 4 声明已核对，这里防手滑）
        let all_names = [
            "曹操", "诸葛亮", "关羽", "赵云", "吕布", "司马懿", "刘备", "孙权", "孙坚", "董卓",
            "张飞", "马超", "黄忠", "姜维", "周瑜", "陆逊", "夏侯惇", "张辽", "郭嘉", "许褚",
            "张郃", "黄盖", "孙尚香", "马岱", "华佗", "颜良", "魏延", "庞统", "法正", "甘宁",
            "太史慈", "孙策", "吕蒙", "邓艾", "荀彧", "甄姬", "大乔", "小乔", "周泰", "黄月英",
            "关兴", "张苞", "陈宫", "貂蝉", "廖化", "简雍", "孙乾", "糜竺", "诸葛瑾", "鲁肃",
            "程昱", "贾诩", "曹洪", "许攸", "张昭", "朱然", "关平", "周仓", "田丰", "审配",
        ];
        for b in BONDS.iter() {
            for m in b.members.iter() {
                assert!(all_names.contains(m), "bond {} member {} not in 60-card table", b.id, m);
            }
        }
    }
}
