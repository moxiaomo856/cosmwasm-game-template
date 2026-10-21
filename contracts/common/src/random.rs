//! 随机源与不重复抽样。
//!
//! # 优先级
//! 1. **VRF** —— 链上可用时使用（Paxi 链若提供 VRF 预编译/enclave，优先接入）。
//! 2. **commit-reveal** —— 建池者提交 `hash(secret)`，开奖阶段揭示 secret，
//!    与区块数据混合。可抵御"验证者/矿工操纵单条交易"的攻击。
//! 3. **兜底 MVP** —— `sha256(参与者列表 + lottery_id + block_hash + tx_index + nonce)`。
//!
//! # 兜底方案的风险（必须写在文档与前端提示里）
//! * `block_hash` 由验证者在出块后确定，理论上最后出块的验证者有一定影响力；
//! * 若参与者列表在开奖前可被操纵（例如最后一个参与者是攻击者），
//!   攻击者可通过"加入/不加入"改变结果（**last-actor 攻击**）；
//! * 因此兜底方案**仅适用于奖池较小**的场景，主网大额奖池必须启用 VRF 或 commit-reveal。

use cosmwasm_schema::cw_serde;
use sha2::{Digest, Sha256};

#[cw_serde]
pub enum RandomSource {
    /// 链上 VRF（需 Paxi 链支持）
    Vrf,
    /// 建池者提交承诺 + 开奖时揭示
    CommitReveal,
    /// 兜底：sha256(参与者 + lottery_id + block_hash + tx_index + nonce)
    BlockHashFallback,
}

/// 把任意数量的熵源混合成 32 字节种子。
pub fn mix_seed(parts: &[&[u8]]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    for p in parts {
        // 先写长度，避免拼接歧义（例如 "ab"+"c" 与 "a"+"bc"）
        hasher.update((p.len() as u64).to_be_bytes());
        hasher.update(p);
    }
    let out = hasher.finalize();
    let mut res = [0u8; 32];
    res.copy_from_slice(&out);
    res
}

/// 用 SHA256(seed || counter) 构造确定性 PRNG，抽取一个 `[0, upper)` 的随机数。
fn next_u64(seed: &[u8; 32], counter: u64, upper: u64) -> u64 {
    let mut i = counter;
    loop {
        let mut hasher = Sha256::new();
        hasher.update(seed);
        hasher.update(i.to_be_bytes());
        let out = hasher.finalize();
        let mut bytes = [0u8; 8];
        bytes.copy_from_slice(&out[0..8]);
        let v = u64::from_be_bytes(bytes);
        // 取模会产生极小的偏置，对于奖池规模可以忽略；
        // 为严谨起见做 rejection sampling。
        let limit = u64::MAX - (u64::MAX % upper);
        if v < limit {
            return v % upper;
        }
        i += 1;
    }
}

/// 从 `[0, total)` 中不放回地抽取 `count` 个下标（部分 Fisher-Yates）。
///
/// 保证：不重复；`count <= total`；同一 seed 结果确定可复现（便于链下审计）。
pub fn draw_distinct_indices(seed: [u8; 32], total: u64, count: u64) -> Vec<u64> {
    if count == 0 || total == 0 {
        return Vec::new();
    }
    let count = count.min(total);
    let mut table: Vec<u64> = (0..total).collect();
    let mut picked: Vec<u64> = Vec::with_capacity(count as usize);
    let mut counter = 0u64;
    for i in 0..count {
        let j = i + next_u64(&seed, counter, total - i);
        counter += 1;
        table.swap(i as usize, j as usize);
        picked.push(table[i as usize]);
    }
    picked
}

/// 构造兜底随机种子。
///
/// 入参顺序固定，链下 Indexer 必须能一字不差复现。
pub fn fallback_seed(
    participants: &[String],
    lottery_id: u64,
    block_hash: &str,
    tx_index: u64,
    nonce: u64,
    reveal: Option<&[u8]>,
) -> [u8; 32] {
    let joined = participants.join(",");
    let mut parts: Vec<Vec<u8>> = vec![
        joined.as_bytes().to_vec(),
        lottery_id.to_be_bytes().to_vec(),
        block_hash.as_bytes().to_vec(),
        tx_index.to_be_bytes().to_vec(),
        nonce.to_be_bytes().to_vec(),
    ];
    if let Some(r) = reveal {
        parts.push(r.to_vec());
    }
    let refs: Vec<&[u8]> = parts.iter().map(|v| v.as_slice()).collect();
    mix_seed(&refs)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn draw_is_deterministic_and_distinct() {
        let seed = mix_seed(&[b"abc", b"123"]);
        let a = draw_distinct_indices(seed, 60, 3);
        let b = draw_distinct_indices(seed, 60, 3);
        assert_eq!(a, b);
        assert_eq!(a.len(), 3);
        let mut sorted = a.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), 3, "中奖者不能重复: {:?}", a);
    }

    #[test]
    fn draw_clamps_count() {
        let seed = mix_seed(&[b"x"]);
        assert_eq!(draw_distinct_indices(seed, 2, 10).len(), 2);
        assert_eq!(draw_distinct_indices(seed, 0, 3).len(), 0);
    }

    #[test]
    fn different_seed_different_result() {
        let s1 = mix_seed(&[b"a"]);
        let s2 = mix_seed(&[b"b"]);
        let r1 = draw_distinct_indices(s1, 60, 3);
        let r2 = draw_distinct_indices(s2, 60, 3);
        assert_ne!(r1, r2);
    }

    #[test]
    fn fallback_seed_is_stable() {
        let p = vec!["a".to_string(), "b".to_string()];
        let s1 = fallback_seed(&p, 7, "BLOCKHASH", 3, 1, None);
        let s2 = fallback_seed(&p, 7, "BLOCKHASH", 3, 1, None);
        assert_eq!(s1, s2);
        let s3 = fallback_seed(&p, 8, "BLOCKHASH", 3, 1, None);
        assert_ne!(s1, s3);
    }
}
