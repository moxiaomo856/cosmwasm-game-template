# contracts/lottery — lottery contract (Apache-2.0)

A lottery / raffle contract: multi-tier player pools, platform template pools
with commit-reveal randomness, prize splitting, and refunds on timeout.

```
contracts/lottery/
├── LICENSE            # Apache-2.0
├── NOTICE             # upstream origin + modifications made for this template
├── README.md
└── contract/          # the Rust crate
    ├── Cargo.toml
    └── src/
```

**Derived from** the Paxi social & lottery workspace
(`paxi-lottery-contract-v2`), which is Apache-2.0. See
[`NOTICE`](./NOTICE) for the exact list of modifications — most importantly the
removal of the invisible-UX (session-key) authentication layer.

## Modes

| Mode | How it works |
|---|---|
| **Player pool** | A player creates a pool at a given tier, others join, the pool draws once it is full, winners claim their share. |
| **Platform template pool** | An admin pre-creates a template at a given tier; users activate it directly. No creation fee. Platform-run. |

## Tiers

Tiers are defined by `tier_spec()` in `src/state.rs` — participant count,
per-participant fee, and creation fee. **They are currently source constants
and are the first thing you should parameterise** (see
[`../../PARAMETERIZE.md`](../../PARAMETERIZE.md)).

## Randomness

Commit-reveal, with a block-hash fallback:

- `RandomSource::CommitReveal` — the creator commits to `hash(secret)`; the
  secret is revealed at draw time. Template pools additionally use a hash chain
  with round rotation.
- `RandomSource::BlockHashFallback` — `sha256(participants + lottery_id +
  block_hash + tx_index + nonce)`. **Small pools only**: the last-actor attack
  and the last validator's influence apply here.
- `RandomSource::Vrf` — reserved; not connected (needs chain-side VRF).

The seed construction is fully deterministic, so an off-chain indexer can
reproduce the draw and audit it.

Read [`../../docs/security-notes.md`](../../docs/security-notes.md) §2 before
running a pool with real value.

## The API is the schema

```bash
cd contract
cargo run --bin schema      # → ./schema/*.json
```

## Build

```bash
cargo test
cargo build --release --target wasm32-unknown-unknown
```

Requires **Rust 1.69.0** for the wasm artifact.

## License

Apache-2.0 — this directory contains code derived from an Apache-2.0 upstream.
Keep the `LICENSE` and `NOTICE` files in this directory when redistributing.
