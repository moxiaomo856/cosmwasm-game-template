//! 管理员白名单 + 多签提案。
//!
//! # 策略
//! * `threshold == 1`（默认）：任何单个管理员可直接执行 `AdminAction`。
//! * `threshold >= 2`：管理员先 `ProposeAdminAction`，其他管理员 `ConfirmAdminAction`，
//!   确认数达到阈值后该动作被**原子执行**（与提案同一次交易内完成）。
//!
//! 这样"多签"不需要额外的链下协调服务，也不需要在合约里保存可执行 payload 队列。

use cosmwasm_std::{Addr, Binary, StdResult, Storage, Timestamp};
use cosmwasm_schema::cw_serde;
use cw_storage_plus::{Item, Map};

use crate::error::AdminError;
use crate::tkcc::BurnMode;

/// 管理员白名单（与 game-contract 的 `ADMINS: Map<&Addr, bool>` 保持一致）
pub const ADMINS: Map<&Addr, bool> = Map::new("admins");
/// 多签阈值
pub const ADMIN_THRESHOLD: Item<u32> = Item::new("admin_threshold");
/// 提案存储
pub const PROPOSALS: Map<u64, AdminProposal> = Map::new("admin_proposals");
pub const PROPOSAL_SEQ: Item<u64> = Item::new("admin_proposal_seq");
/// 提案有效期
pub const PROPOSAL_TTL_SECONDS: u64 = 7 * 24 * 3600;

/// 两个合约共用的管理员动作。
///
/// 合约特有动作使用 `Custom`（合约按 `label` 自行反序列化 `payload`）。
#[cw_serde]
pub enum AdminAction {
    /// 设置 TKCC（外部 PRC-20）地址 —— 发币后启用所有 TKCC 功能的开关
    SetTkccToken { token: String },
    /// 设置 TKCC 销毁黑洞地址
    SetTkccBurnAddress { address: String },
    /// 设置销毁方式：Burn / BlackHole / Skip
    SetTkccBurnMode { mode: BurnMode },
    /// 设置运营金库
    SetTreasury { treasury: String },
    /// 暂停 / 恢复
    SetPaused { paused: bool },
    /// 增删管理员 + 调整多签阈值
    UpdateAdmins {
        add: Vec<String>,
        remove: Vec<String>,
        threshold: Option<u32>,
    },
    /// 合约特有动作
    Custom { label: String, payload: Binary },
}

#[cw_serde]
pub struct AdminProposal {
    pub id: u64,
    pub action: AdminAction,
    pub proposer: Addr,
    pub confirmations: Vec<Addr>,
    pub created_at: u64,
    pub expires_at: u64,
    pub executed: bool,
}

pub struct AdminSet;

impl AdminSet {
    pub fn init(storage: &mut dyn Storage, admins: &[Addr], threshold: Option<u32>) -> Result<(), AdminError> {
        if admins.is_empty() {
            return Err(AdminError::InvalidThreshold);
        }
        for a in admins {
            ADMINS.save(storage, a, &true)?;
        }
        let t = threshold.unwrap_or(1).max(1);
        if t as usize > admins.len() {
            return Err(AdminError::InvalidThreshold);
        }
        ADMIN_THRESHOLD.save(storage, &t)?;
        PROPOSAL_SEQ.save(storage, &0u64)?;
        Ok(())
    }

    pub fn is_admin(storage: &dyn Storage, addr: &Addr) -> bool {
        ADMINS.may_load(storage, addr).unwrap_or_default().unwrap_or(false)
    }

    pub fn assert_admin(storage: &dyn Storage, addr: &Addr) -> Result<(), AdminError> {
        if Self::is_admin(storage, addr) {
            Ok(())
        } else {
            Err(AdminError::Unauthorized(addr.clone()))
        }
    }

    pub fn threshold(storage: &dyn Storage) -> u32 {
        ADMIN_THRESHOLD.may_load(storage).unwrap_or_default().unwrap_or(1)
    }

    pub fn count(storage: &dyn Storage) -> usize {
        ADMINS
            .range(storage, None, None, cosmwasm_std::Order::Ascending)
            .filter(|r| r.as_ref().map(|(_, v)| *v).unwrap_or(false))
            .count()
    }

    pub fn list(storage: &dyn Storage) -> StdResult<Vec<Addr>> {
        ADMINS
            .range(storage, None, None, cosmwasm_std::Order::Ascending)
            .filter_map(|r| match r {
                Ok((a, true)) => Some(Ok(a)),
                Ok(_) => None,
                Err(e) => Some(Err(e)),
            })
            .collect()
    }

    pub fn add(storage: &mut dyn Storage, admin: &Addr) -> StdResult<()> {
        ADMINS.save(storage, admin, &true)
    }

    pub fn remove(storage: &mut dyn Storage, admin: &Addr) -> Result<(), AdminError> {
        if Self::count(storage) <= 1 && Self::is_admin(storage, admin) {
            return Err(AdminError::LastAdmin);
        }
        ADMINS.remove(storage, admin);
        let remaining = Self::count(storage);
        // 阈值不得超过管理员数
        let t = Self::threshold(storage);
        if t as usize > remaining {
            ADMIN_THRESHOLD.save(storage, &(remaining as u32))?;
        }
        Ok(())
    }

    pub fn set_threshold(storage: &mut dyn Storage, threshold: u32) -> Result<(), AdminError> {
        let n = Self::count(storage);
        if threshold < 1 || threshold as usize > n {
            return Err(AdminError::InvalidThreshold);
        }
        ADMIN_THRESHOLD.save(storage, &threshold)?;
        Ok(())
    }

    /// 创建提案，并计入提议者本人作为第一个确认者。
    pub fn propose(
        storage: &mut dyn Storage,
        now: Timestamp,
        proposer: &Addr,
        action: AdminAction,
    ) -> Result<u64, AdminError> {
        Self::assert_admin(storage, proposer)?;
        let id = PROPOSAL_SEQ.may_load(storage)?.unwrap_or_default() + 1;
        PROPOSAL_SEQ.save(storage, &id)?;
        let created_at = now.seconds();
        PROPOSALS.save(
            storage,
            id,
            &AdminProposal {
                id,
                action,
                proposer: proposer.clone(),
                confirmations: vec![proposer.clone()],
                created_at,
                expires_at: created_at + PROPOSAL_TTL_SECONDS,
                executed: false,
            },
        )?;
        Ok(id)
    }

    /// 确认提案。达到阈值时返回 `(true, action)`，由调用方立即执行。
    pub fn confirm(
        storage: &mut dyn Storage,
        now: Timestamp,
        confirmer: &Addr,
        id: u64,
    ) -> Result<(bool, AdminAction), AdminError> {
        Self::assert_admin(storage, confirmer)?;
        let mut p = PROPOSALS
            .may_load(storage, id)?
            .ok_or(AdminError::ProposalNotFound(id))?;
        if p.executed {
            return Err(AdminError::AlreadyExecuted(id));
        }
        if p.expires_at < now.seconds() {
            return Err(AdminError::ProposalExpired(id));
        }
        if p.confirmations.contains(confirmer) {
            return Err(AdminError::AlreadyConfirmed(id, confirmer.clone()));
        }
        p.confirmations.push(confirmer.clone());
        let reached = p.confirmations.len() as u32 >= Self::threshold(storage);
        if reached {
            p.executed = true;
        }
        PROPOSALS.save(storage, id, &p)?;
        Ok((reached, p.action))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cosmwasm_std::testing::MockStorage;

    #[test]
    fn admin_whitelist() {
        let mut s = MockStorage::new();
        let a = Addr::unchecked("a");
        let b = Addr::unchecked("b");
        AdminSet::init(&mut s, &[a.clone(), b.clone()], None).unwrap();
        assert!(AdminSet::is_admin(&s, &a));
        assert!(!AdminSet::is_admin(&s, &Addr::unchecked("c")));
        assert_eq!(AdminSet::threshold(&s), 1);
        assert!(AdminSet::assert_admin(&s, &b).is_ok());
    }

    #[test]
    fn multisig_reaches_threshold() {
        let mut s = MockStorage::new();
        let a = Addr::unchecked("a");
        let b = Addr::unchecked("b");
        let c = Addr::unchecked("c");
        AdminSet::init(&mut s, &[a.clone(), b.clone(), c.clone()], Some(2)).unwrap();

        let id = AdminSet::propose(
            &mut s,
            Timestamp::from_seconds(1000),
            &a,
            AdminAction::SetTkccToken { token: "paxi1tkcc".to_string() },
        )
        .unwrap();

        let (reached, action) =
            AdminSet::confirm(&mut s, Timestamp::from_seconds(1001), &b, id).unwrap();
        assert!(reached);
        match action {
            AdminAction::SetTkccToken { token } => assert_eq!(token, "paxi1tkcc"),
            _ => panic!("wrong action"),
        }

        // 已执行的提案不能再次确认
        assert!(matches!(
            AdminSet::confirm(&mut s, Timestamp::from_seconds(1002), &c, id),
            Err(AdminError::AlreadyExecuted(_))
        ));
    }

    #[test]
    fn cannot_remove_last_admin() {
        let mut s = MockStorage::new();
        let a = Addr::unchecked("a");
        AdminSet::init(&mut s, &[a.clone()], None).unwrap();
        assert!(matches!(AdminSet::remove(&mut s, &a), Err(AdminError::LastAdmin)));
    }
}
