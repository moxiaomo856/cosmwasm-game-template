// ============================================================================
// 🀄 三国卡牌 V2 战斗引擎（battle 模块）
//
// 权威依据：《三国卡牌V2修改裁定书》第 7 章 + 《三国卡牌V2设计终稿》文档 1。
// 裁定时序（E1~E7）与设计终稿「比赛模拟算法」逐字节一致：
//   E1 四因子依次乘除、每步 floor，顺序 faction → troop → bond → rarity
//   E2 attack_eff/defense_eff ≥ 原值 50%（eff_lower_bound_bp，默认 5000）
//   E3 全程 u128 中间值，末尾下转 u64，超限报 Overflow
//   E7 SKILLS/BONDS 编译期常量（&'static），find_skill/find_bond 编译期索引
//
// 模块划分：
//   skills.rs —— 60 技能编译期常量 + find_skill（match 编译期索引，禁线性查表）
//   bonds.rs  —— 23 连携编译期常量 + 激活计算（单档封顶 S3）
//   compute.rs—— BattleCard/BattleResult/compute_battle（9v9 三组制主引擎）
//   ai_gen.rs —— AI 敌生成（day-bucket 种子 E4）+ stage Fixed 敌人（E5）
// ============================================================================

pub mod ai_gen;
pub mod bonds;
pub mod compute;
pub mod skills;

pub use bonds::{find_bond, BondDef, BONDS};
pub use compute::{compute_battle, BattleCard, BattleContext, BattleResult, SkillTriggerLog, Winner};
pub use skills::{find_skill, SkillEffect, SkillTarget, SkillTrigger, SKILLS};
