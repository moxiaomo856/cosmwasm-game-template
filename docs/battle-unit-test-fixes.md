# 战斗引擎单测修复说明（2026-10-21）

> 场景：公开模板化过程中，首次在 CI 环境跑 `cargo test` 时发现三国游戏合约有 **2 个单测失败**：
> `sanguo::battle::compute::tests::overflow_guard` 与
> `sanguo::battle::compute::tests::strong_team_wins_early_stop`。
>
> 结论：**这不是本次改造引入的缺陷，而是上游生产合约长期存在的「坏测试」**。
> 引擎行为正确，错的是测试的断言前提与数据构造。

本文档记录完整的判定过程与修复方式，供审阅者复核。

---

## 一、如何确认是上游遗留而非本次引入

三重独立证据：

| # | 证据 | 结论 |
|---|---|---|
| 1 | `src/sanguo/battle/` 整个目录与上游生产合约的 `src/sanguo/battle/` **逐字一致**（`diff -rq` 无输出） | 被测代码与依赖它的 skills/bonds 表均未改动 |
| 2 | `src/sanguo/state.rs` 与上游的差异**仅存在于注释**（`diff` 过滤注释后无实质差异行） | 战斗参数来源未变 |
| 3 | **在上游生产合约目录直接跑 `cargo test`，得到完全相同的 2 个失败**（同一条 assert、同一个 panic 行号 975 / 961） | 决定性证据：上游同样失败 |

> 也就是说：线上 `code_id 36` 所依据的源码，从建立起就带着这两个红灯单测。
> 上游很可能因为只关心 `cargo build` 通过（发 wasm）而从未执行过 `cargo test`。

---

## 二、`overflow_guard`：断言前提在类型上不可达

### 原测试

```rust
/// E3 溢出防护：攻击力极端巨大 → Overflow
#[test]
fn overflow_guard() {
    let mut huge = guanyu();
    huge.attack = u32::MAX;
    huge.defense = u32::MAX;
    let a = team_with_slot0(huge);
    let b = team_with_slot0(filler("x"));
    let r = compute_battle(&ctx(), &a, &b);
    assert!(matches!(r, Err(ContractError::Overflow)));
}
```

### 为什么不成立

引擎的溢出防线是三层的，其中 `check_base_bound` 负责「base 不得越过 u64 容量」：

```rust
// compute.rs
fn check_base_bound(base: u128, ctx: &BattleContext) -> Result<(), ContractError> {
    let prod = fmax * tmax * bond_max * rmax;          // 11500×11000×11800×11500
    let bound = (u64::MAX as u128) * TEN_K4 / prod;       // TEN_K4 = 10^16
    if base > bound { return Err(ContractError::Overflow); }
    Ok(())
}
```

代入实际数值（`ctx()` 测试默认值）：

| 量 | 数值 |
|---|---|
| `base`（attack + defense = 2×u32::MAX） | **8.59 × 10⁹** |
| `prod` | 1.7166 × 10¹⁶ |
| `bound` = u64::MAX × 10¹⁶ ÷ prod | **1.0746 × 10¹⁹** |

`base` 比 `bound` **小 9 个数量级** → `if base > bound` 永远不成立 → 永远不会返回 `Overflow`。

更根本的是类型层面的限制：`BattleCard.attack` / `.defense` 是 **`u32`**，上限 4 294 967 295。
无论怎么填，base 最大也只有 ~8.59×10⁹，**在类型上就不可能达到 1.07×10¹⁹ 的溢出线**。

### 结论

引擎的溢出防护是**正确且有效**的（u128 内部运算 + u64 输出钳制 + `to_u64` 兜底），
**错的是测试选择了一个不可能触发该分支的输入**。

### 修复方式

把测试拆成两层，都验证**真实可达**的不变量：

```rust
#[test]
fn overflow_guard() {
    // ① u32 上限的攻防：u128 内部量足够消化，不应溢出
    let mut huge = guanyu();
    huge.attack = u32::MAX;
    huge.defense = u32::MAX;
    let a = team_with_slot0(huge);
    let b = team_with_slot0(filler("x"));
    let r = compute_battle(&ctx(), &a, &b);
    assert!(r.is_ok(), "u32::MAX 的攻防在 u128 域内可安全计算，不应触发 Overflow");

    // ② 真正可达的溢出防线：超过 u64::MAX 的战力必须被拒
    assert!(matches!(to_u64(u64::MAX as u128 + 1), Err(ContractError::Overflow)));
    assert!(to_u64(u64::MAX as u128).is_ok(), "u64::MAX 本身应通过");
}
```

---

## 三、`strong_team_wins_early_stop`：数据构造与断言互相矛盾

### 原测试的意图

> 「强力卡放满一组应赢下该组并先赢 2 组结束」——即验证**先赢 2 组 → 提前结束**。

### 为什么不成立

编组规则是 **3 张一组 × 3 组**（slot 0-2 → 第 1 组，slot 3-5 → 第 2 组，slot 6-8 → 第 3 组），
组内是 best-of-3（**赢 2 小局才算赢下该组**）。

原数据构造：

```rust
let mut a = team_with_slot0(guanyu());   // slot 0 = 关羽
let mut g2 = guanyu(); g2.card_id = "gy2".to_string();
let mut g3 = guanyu(); g3.card_id = "gy3".to_string();
a[1] = g2; a[2] = g3;                    // slot 1、2 = 关羽
// → 3 张关羽全部落在第 1 组；slot 3-8 是 filler(攻10/防10)
```

实测结果（加诊断打印后）：

```
winner=Draw  groups_played=3  group_wins_a=1  group_wins_b=0
group_scores_a=[3, 0, 0]  group_scores_b=[0, 0, 0]
```

推导：

| 组 | A 侧 | B 侧 | 结果 |
|---|---|---|---|
| 第 1 组 | slot0-2 = 关羽×3 | filler×3 | A 三小局全胜 → sa=3 → **A 赢第 1 组** |
| 第 2 组 | slot3-5 = filler | filler×3 | 同值 → `fa>fb` 与 `fb>fa` 均不成立，base 也相等 → **真平局**（裁定书 G2：平局组双方均不计胜） |
| 第 3 组 | slot6-8 = filler | filler×3 | 同上 → 真平局 |

于是 A 只赢 1 组；3 组打满无人先赢 2 组 → 按裁定书 G3 判**整场平局**。

**引擎行为完全符合设计裁定书**，错的是测试：
① 3 张强卡全挤在第 1 组，只能赢 1 组，够不到「先赢 2 组」；
② 剩余 slot 是 filler，与 B 侧 filler 同值，必然平局。

### 修复方式

让 A 的**前两组**（slot 0-5）各放满 3 张强卡 —— 这样 A 连赢第 1、2 组，
在第 2 组后提前结束，恰好验证该测试真正想验证的语义：

```rust
#[test]
fn strong_team_wins_early_stop() {
    // A：slot 0-5 各放关羽（第 1、2 组满强卡）；B：全 filler
    let mut a = team_with_slot0(guanyu());
    for (k, idx) in [1usize, 2, 3, 4, 5].iter().enumerate() {
        let mut g = guanyu();
        g.card_id = format!("gy{}", k + 1);
        a[*idx] = g;
    }
    let b = team_with_slot0(filler("x"));
    let r = compute_battle(&ctx(), &a, &b).unwrap();
    assert!(matches!(r.winner, Winner::A));
    assert_eq!(r.groups_played, 2, "先赢 2 组应在第 2 组后结束");
    assert_eq!(r.group_wins_a, 2);
}
```

> 注意：**不能**只把强卡「分散到 3 个组、每组 1 张」——那样每组只赢 1 小局，
> 组内 best-of-3 达不到 2 胜，仍然赢不下任何一组（实测 `group_scores_a=[1,1,1]`）。
> 必须是**整组满强卡**。

---

## 四、附带修复：`find_skill_all_present` 的裸指针比较

原断言：

```rust
assert_eq!(*expect as *const SkillEffect, got as *const SkillEffect);
```

比较的是**静态 const 的裸指针地址**。这依赖编译器是否把 const 提升为同一份静态分配
（const promotion / codegen unit 划分），在不同工具链、优化等级、target 下会漂移 ——
实测在 Rust 1.74 + 依赖版本不同的组合下失败。

改为**值比较**（`SkillEffect` 由 `#[cw_serde]` 派生了 `PartialEq`），语义等价且跨环境稳定：

```rust
assert_eq!(**expect, *got, "技能 {id} 的 find_skill 命中了错误的定义");
```

（`expect` 是 `&&SkillEffect`，故为 `**expect`。）

---

## 五、验证结果

| crate | 测试数 | 结果 |
|---|---|---|
| `game-contract` | 18 | **18 passed / 0 failed** |
| `paxi-lottery-contract-simple` | 11 | **11 passed / 0 failed** |

---

## 六、对上游生产合约的处置建议

本模板的修复**只改测试、不改引擎**，因此：

- ✅ 引擎行为与线上 `code_id 36` **完全一致**（battle 目录逐字未动）→ 不影响任何线上对局结果；
- ✅ 修复后的测试**全部通过**，可作为公开仓 CI 的绿色基线；
- ⚠️ 但上游生产合约里的**同名坏测试仍在**。若上游后续要把该目录纳入 CI，
  需同步这两处修复（可直接取用本模板的 `compute.rs` / `skills.rs` 修正版）。
