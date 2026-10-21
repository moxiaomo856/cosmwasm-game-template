//! 合约内部余额账本。
//!
//! # 设计
//! ```text
//! BALANCES:      Map<(&Addr /*user*/, &Addr /*token*/), Uint128>
//! NATIVE_LEDGER: Item<Uint128>          // 合约实际持有的 upaxi 总量
//! TOKEN_LEDGER:  Map<&Addr, Uint128>    // 合约实际持有的每种 PRC-20 总量
//! USER_TOTALS:   Map<&Addr, Uint128>    // 每用户 PAXI 余额镜像，用于偿付能力 & 风控查询
//! ```
//! * `token == Addr::unchecked("")` 表示原生 PAXI（`native_key()`）。
//! * 任何 `credit` / `debit` 必须同步维护 ledger，
//!   保证 `Σ BALANCES[*, token] == LEDGER[token]` 恒成立（偿付能力恒等式）。
//! * 所有业务扣费都走内部余额，实现"一次充值、后续无感"。

use cosmwasm_std::{Addr, Order, StdResult, Storage, Uint128};
use cw_storage_plus::{Item, Map};

use crate::error::BalanceError;
use crate::tkcc::native_key;

/// 与 game-contract 同名的别名：空地址表示原生 PAXI。
pub fn empty_addr() -> Addr {
    native_key()
}

pub const BALANCES: Map<(&Addr, &Addr), Uint128> = Map::new("balances");
pub const NATIVE_LEDGER: Item<Uint128> = Item::new("native_ledger");
pub const TOKEN_LEDGER: Map<&Addr, Uint128> = Map::new("token_ledger");
pub const USER_TOTALS: Map<&Addr, Uint128> = Map::new("user_totals");

pub struct BalanceLedger;

impl BalanceLedger {
    /// 入账：用户充值、中奖、红包退款等。
    pub fn credit(
        store: &mut dyn Storage,
        user: &Addr,
        token: &Addr,
        amount: Uint128,
    ) -> Result<Uint128, BalanceError> {
        if amount.is_zero() {
            return Ok(Self::balance_of(store, user, token));
        }
        let new_balance = Self::balance_of(store, user, token) + amount;
        BALANCES.save(store, (user, token), &new_balance)?;
        Self::bump_ledger(store, token, amount, true)?;
        if *token == native_key() {
            let total = USER_TOTALS.may_load(store, user)?.unwrap_or_default() + amount;
            USER_TOTALS.save(store, user, &total)?;
        }
        Ok(new_balance)
    }

    /// 扣账：建群、发帖、打赏、参与抽奖等。余额不足直接报错。
    pub fn debit(
        store: &mut dyn Storage,
        user: &Addr,
        token: &Addr,
        amount: Uint128,
    ) -> Result<Uint128, BalanceError> {
        if amount.is_zero() {
            return Ok(Self::balance_of(store, user, token));
        }
        let have = Self::balance_of(store, user, token);
        if have < amount {
            return Err(BalanceError::Insufficient { need: amount, have });
        }
        let new_balance = have - amount;
        BALANCES.save(store, (user, token), &new_balance)?;
        Self::bump_ledger(store, token, amount, false)?;
        if *token == native_key() {
            let total = USER_TOTALS.may_load(store, user)?.unwrap_or_default();
            USER_TOTALS.save(store, user, &total.saturating_sub(amount))?;
        }
        Ok(new_balance)
    }

    /// 内部划转（用户 -> 用户），ledger 总量不变。
    pub fn transfer(
        store: &mut dyn Storage,
        from: &Addr,
        to: &Addr,
        token: &Addr,
        amount: Uint128,
    ) -> Result<(), BalanceError> {
        if from == to || amount.is_zero() {
            return Ok(());
        }
        Self::debit(store, from, token, amount)?;
        Self::credit(store, to, token, amount)?;
        Ok(())
    }

    pub fn balance_of(store: &dyn Storage, user: &Addr, token: &Addr) -> Uint128 {
        BALANCES.may_load(store, (user, token)).unwrap_or_default().unwrap_or_default()
    }

    pub fn native_balance(store: &dyn Storage, user: &Addr) -> Uint128 {
        Self::balance_of(store, user, &native_key())
    }

    pub fn user_total(store: &dyn Storage, user: &Addr) -> Uint128 {
        USER_TOTALS.may_load(store, user).unwrap_or_default().unwrap_or_default()
    }

    /// 合约托管的某币种总量（账本口径）。
    pub fn ledger_of(store: &dyn Storage, token: &Addr) -> Uint128 {
        if *token == native_key() {
            NATIVE_LEDGER.may_load(store).unwrap_or_default().unwrap_or_default()
        } else {
            TOKEN_LEDGER.may_load(store, token).unwrap_or_default().unwrap_or_default()
        }
    }

    fn bump_ledger(
        store: &mut dyn Storage,
        token: &Addr,
        amount: Uint128,
        up: bool,
    ) -> Result<(), BalanceError> {
        if *token == native_key() {
            let cur = NATIVE_LEDGER.may_load(store)?.unwrap_or_default();
            let next = if up {
                cur + amount
            } else {
                cur.checked_sub(amount)
                    .map_err(|_| BalanceError::Insolvency {
                        token: "upaxi".to_string(),
                        liabilities: amount,
                        assets: cur,
                    })?
            };
            NATIVE_LEDGER.save(store, &next)?;
        } else {
            let cur = TOKEN_LEDGER.may_load(store, token)?.unwrap_or_default();
            let next = if up {
                cur + amount
            } else {
                cur.checked_sub(amount)
                    .map_err(|_| BalanceError::Insolvency {
                        token: token.to_string(),
                        liabilities: amount,
                        assets: cur,
                    })?
            };
            TOKEN_LEDGER.save(store, token, &next)?;
        }
        Ok(())
    }

    /// 偿付能力校验：账本负债总和必须等于合约实际持有量。
    ///
    /// 用于单元测试与管理员审计查询（主网可在关键操作后调用）。
    pub fn solvency_check(store: &dyn Storage) -> Result<(), BalanceError> {
        let mut sum = Uint128::zero();
        for item in BALANCES.range(store, None, None, Order::Ascending) {
            let ((_, token), amount) = item.map_err(BalanceError::Std)?;
            if token == native_key() {
                sum += amount;
            }
        }
        let assets = NATIVE_LEDGER.may_load(store).map_err(BalanceError::Std)?.unwrap_or_default();
        if sum != assets {
            return Err(BalanceError::Insolvency {
                token: "upaxi".to_string(),
                liabilities: sum,
                assets,
            });
        }
        Ok(())
    }

    /// 列出某用户在所有币种上的余额（供查询接口使用）。
    pub fn all_balances(store: &dyn Storage, user: &Addr) -> StdResult<Vec<(Addr, Uint128)>> {
        let mut out = Vec::new();
        for item in BALANCES.prefix(user).range(store, None, None, Order::Ascending) {
            let (token, amount) = item?;
            out.push((token, amount));
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cosmwasm_std::testing::MockStorage;

    #[test]
    fn credit_debit_and_solvency() {
        let mut store = MockStorage::new();
        let alice = Addr::unchecked("alice");
        let bob = Addr::unchecked("bob");
        let paxi = native_key();

        BalanceLedger::credit(&mut store, &alice, &paxi, Uint128::new(100)).unwrap();
        assert_eq!(BalanceLedger::native_balance(&store, &alice), Uint128::new(100));
        assert_eq!(BalanceLedger::ledger_of(&store, &paxi), Uint128::new(100));

        BalanceLedger::transfer(&mut store, &alice, &bob, &paxi, Uint128::new(30)).unwrap();
        assert_eq!(BalanceLedger::native_balance(&store, &alice), Uint128::new(70));
        assert_eq!(BalanceLedger::native_balance(&store, &bob), Uint128::new(30));
        assert_eq!(BalanceLedger::ledger_of(&store, &paxi), Uint128::new(100));
        BalanceLedger::solvency_check(&store).unwrap();
    }

    #[test]
    fn debit_insufficient() {
        let mut store = MockStorage::new();
        let alice = Addr::unchecked("alice");
        let paxi = native_key();
        let err = BalanceLedger::debit(&mut store, &alice, &paxi, Uint128::new(1)).unwrap_err();
        assert!(matches!(
            err,
            BalanceError::Insufficient { need: _, have: _ }
        ));
    }

    #[test]
    fn user_totals_track_native() {
        let mut store = MockStorage::new();
        let alice = Addr::unchecked("alice");
        let paxi = native_key();
        BalanceLedger::credit(&mut store, &alice, &paxi, Uint128::new(500)).unwrap();
        assert_eq!(BalanceLedger::user_total(&store, &alice), Uint128::new(500));
        BalanceLedger::debit(&mut store, &alice, &paxi, Uint128::new(200)).unwrap();
        assert_eq!(BalanceLedger::user_total(&store, &alice), Uint128::new(300));
    }
}
