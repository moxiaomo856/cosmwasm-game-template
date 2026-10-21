# Card Game Contract

A cw721-style card-game contract for CosmWasm 2.0: NFT cards, a deterministic
battle engine, stage progression, a fragment economy, and an asset layer.

> This is the **template** variant of the contract. It is derived from a
> production deployment on Paxi, reworked into a generic, reusable starting
> point: game-specific numbers live in config, and the invisible-UX
> (session-key) authentication layer has been removed in favour of direct
> transaction-sender signing.
>
> See [`../../../docs/invisible-ux-boundaries.md`](../../../docs/invisible-ux-boundaries.md)
> for what that means and how to add your own auth layer.

---

## Capabilities

| Area | What it does |
|---|---|
| **Cards** | NFT cards with rarity, faction, and title. Minted in batches; ownership on-chain. |
| **Draw** | Card drawing (free, or fragment-paid), with an on-chain daily cap. |
| **Battle** | Single PvP + free-for-all + AI battles, all **recomputed on-chain** from card stats — the client never decides the outcome. |
| **Progression** | Stage clearing, star-up, upgrade, crafting, decomposition, and batch decomposition. |
| **Fragments** | A unified non-transferable currency. All rewards are paid in fragments, never in a freely-transferable token. |
| **Gifting** | Transfer cards or fragments to another address. |
| **Shop & extras** | Shop purchases, stamina, cosmetics, season pass, ranked play, daily tasks. |
| **Assets** | Deposit / withdraw / swap of PRC-20 (CW20-compatible) tokens, plus an internal balance ledger with solvency checks. |
| **Admin** | Parameter config, card template tables, stage tables, migration and cleanup tooling. |

---

## Authentication

Player actions are signed by the **transaction sender** (`info.sender`). There
are no `session_addr` / `nonce` / `signature` fields in any message, and no
session-related execute or query messages.

Player identity is resolved in exactly one place:

```rust
// src/sanguo/helpers.rs
pub fn sanguo_auth(deps: Deps, env: Env, info: &MessageInfo) -> Result<Addr, ContractError>
```

Swap that body for your own scheme (session table, permit, off-chain signer)
and everything else keeps working. **Note**: it also records the account's first
on-chain activity (`ACCT_FIRST_SEEN`), which the gifting system uses for its
"account older than N days" rule — preserve that side effect if you replace it.

---

## The API is the schema

Do not hand-maintain a message list. Generate it:

```bash
cargo run --bin schema      # → ./schema/game_contract.json (+ per-message files)
```

`schema/` is the authoritative API reference: every execute and query variant,
with field names **exactly as they go on chain**.

> ⚠️ **Snake case on chain.** These messages use `#[cw_serde]`, which serialises
> enum variants in `snake_case`. `SanguoBattlePvp` goes on chain as
> `sanguo_battle_pvp`, `SanguoBattleAi` as `sanguo_battle_ai`, and so on.
> PascalCase in a transaction will fail with `unknown variant`.

---

## Configuration

Game-specific numbers are collected in `SanguoParams` (see `src/sanguo/state.rs`)
and can be set by an admin via `SanguoSetParams`. The container is
`#[serde(default)]`, so **old JSON payloads still load** after an upgrade — this
matters when you migrate an existing deployment.

Values that are *still* hardcoded in source and are candidates for
parameterisation are tracked in [`../../../PARAMETERIZE.md`](../../../PARAMETERIZE.md).
The main ones:

- `SG_SKILLS` — 60 card skill definitions
- `SG_BONDS` — 23 bond (combination) definitions
- `FRAG_UNIT = [1, 5, 20, 80]` — fragment exchange rates
- battle difficulty boost (`DIFF_BOOST_BP`) and ranked-tier boost

Move these into `InstantiateMsg` or an example config before you ship a
production deployment of this template.

---

## Deterministic battle

The outcome of every battle is **recomputed on-chain** from card stats. The
client submits its lineup; it does not submit a result.

- Team validation (`validate_battle_team`) runs at creation/join time.
- Power comparison and resolution happen in `src/sanguo/battle/compute.rs`.
- Ranked play is fully deterministic: the AI baseline is the player's own
  nine-card average power multiplied by a per-tier boost, so the front-end can
  preview the result exactly. Higher-than-average cards beat the AI,
  lower ones lose.
- Ties are broken by within-group total power, then by group wins.

Because the engine is deterministic, a front-end can offer a battle
simulation. That is a property of the design, not a security assumption —
the chain is still the arbiter.

---

## Reward and cap invariants

- All gameplay rewards are paid in **fragments** (non-transferable), via
  `grant_fragments_checked`, which enforces a hard daily cap.
- Paid actions (draw packs, stamina, shop) are **fixed-price** and the payment
  is forwarded directly to the configured receiver — funds do not sit in the
  contract and there is no variable payout.
- The contract maintains an internal ledger and runs solvency checks; skipped
  checks are counted so they can be monitored off-chain.

---

## Build

```bash
cargo test
cargo build --release --target wasm32-unknown-unknown
cargo run --bin schema
```

Requires **Rust 1.69.0** for the wasm artifact (see the repo-root
`rust-toolchain.toml` — the on-chain VM rejects `sign-ext` instructions, and
Rust 1.70+ prebuilt wasm std includes them).

Deployment and migration: see [`../../../docs/deployment.md`](../../../docs/deployment.md).

---

## Source layout

```
src/
├── contract.rs        # instantiate / execute / query / migrate
├── msg.rs             # InstantiateMsg / ExecuteMsg / QueryMsg
├── state.rs           # shared state
├── error.rs
├── helpers.rs
├── admin/             # admin config + migration
├── assets/            # PRC-20 deposit / withdraw / swap
├── games/             # generic engine + limits (also used by the hub games)
├── sanguo/            # the card-game domain
│   ├── battle/        # compute / skills / bonds / ai generation
│   ├── exec.rs        # player-facing entry points
│   ├── gift.rs
│   ├── query.rs
│   └── state.rs
└── bin/schema.rs      # JSON schema generator
```
