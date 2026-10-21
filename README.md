# cosmwasm-game-template

**Paxi's first open-source CosmWasm game + lottery template.**
Two validated contract families (card game & lottery), extracted into reusable,
parameterizable MIT-licensed templates with full documentation, a reference
front-end and an honest security write-up.

> **Published at** **`github.com/moxiaomo856/cosmwasm-game-template`**.
> This repository is the funded open-source template; see
> [`docs/scope-clarification.md`](docs/scope-clarification.md) for the disclosure
> of the narrowed invisible-UX scope versus the funding proposal.
>
> *Why a personal account rather than an org?* The funding proposal named
> "the Paxi Ecosystem org". The proposer does not have repository-creation
> rights in that org, so the template is published under
> `moxiaomo856/` instead, with this deviation recorded in
> [`docs/scope-clarification.md`](docs/scope-clarification.md). The project
> is not affiliated with, endorsed by, or operated on behalf of any official
> Paxi organisation.

---

## What's in the box

```
cosmwasm-game-template/
├── contracts/
│   ├── game/       # card-game contract (NFT cards, battle, settlement, gifting)
│   ├── lottery/    # lottery contract (tiers, commit-reveal draw, prize split)
│   └── common/     # shared crate: balance ledger, PRC-20 helpers, commit-reveal RNG
├── frontend-demo/  # reference front-end (game + lottery flows)
├── docs/           # architecture, contracts, integration, deployment, FAQ, security
└── scripts/        # deploy / store-code helpers
```

### The two contracts

| | `contracts/game` | `contracts/lottery` |
|---|---|---|
| Language | Rust, CosmWasm 2.0.4 | Rust, CosmWasm 2.0.4 |
| Toolchain | Rust **1.69.0** for wasm (see below) | Rust **1.69.0** for wasm |
| Auth | transaction sender signs directly | transaction sender signs directly |
| Highlights | cw721-style NFT cards, PvP + AI battle with on-chain recomputation, fragment economy, gifting, stage progression, shop | multi-tier pools (5/20/50 participants), platform template pools with commit-reveal, prize split ratios, refunds on timeout, optional external PRC-20 token with burn modes |

### The shared crate

`contracts/common` is a vendored copy of `paxi-common` (Apache-2.0) providing:

- `balance` — internal balance ledger (deposit / withdraw / internal transfer / solvency checks)
- `prc20` — PRC-20 (CW20-compatible) message helpers
- `tkcc` — external PRC-20 token wiring and burn modes
- `random` — **commit-reveal randomness** with a deterministic, chain-reproducible seed
- `admin` — admin whitelist + multi-signature proposal data structures

---

## Quick start

```bash
# 0. Toolchain is pinned by the repo-root rust-toolchain.toml (Rust 1.69.0).
#    rustup reads it automatically; no manual switch needed.

# 1. Build & test (each crate is independent — there is no root workspace)
cd contracts/game
cargo test
cargo build --release --target wasm32-unknown-unknown

# 2. Generate JSON schema (for your front-end / indexer)
cargo run --bin schema        # → ./schema/*.json

# 3. Same for the lottery
cd ../lottery
cargo test
cargo run --bin schema
```

### Cargo.lock — commit it

This repository **commits `Cargo.lock`** on purpose. Nearly every dependency is
pinned to an exact version (`cosmwasm-std =2.0.4`, `cw-storage-plus =2.0.0`, …)
so that a `cargo update` cannot silently pull in edition-2024 indirect
dependencies that Rust 1.69 cannot compile. Those pins only buy you
reproducibility if the lock file travels with them.

If you change dependencies, commit the regenerated lock file in the same commit.

### ⚠️ Toolchain: 1.69.0 for wasm, not 1.74

The on-chain CosmWasm VM (0.55) does not accept `sign-ext` instructions.
Pre-compiled `wasm32-unknown-unknown` std from Rust **1.70+** enables `sign-ext`
by default, and `RUSTFLAGS` cannot turn it off (the instructions live inside
`std`). The `1.69.0` std does not have it, so the wasm artifact is pure
MVP(+mutable-globals).

- **wasm build** → use Rust **1.69.0**
- **host-side `cargo test` / `cargo check`** → 1.74.1 is fine and faster

`Cargo.toml` pins `rust-version = "1.69"` so newer Cargo resolvers avoid
incompatible dependency versions automatically.

---

## Authentication model

Player actions are signed by the **transaction sender** (`info.sender`).
There are no `session_addr` / `nonce` / `signature` fields in any message.

This template deliberately does **not** ship a session-key invisible-UX
implementation. What that means, why, and how to plug in your own
authentication layer is documented in
[`docs/invisible-ux-boundaries.md`](./docs/invisible-ux-boundaries.md).

---

## Documentation

| Document | What it covers |
|---|---|
| [`docs/invisible-ux-boundaries.md`](docs/invisible-ux-boundaries.md) | what the invisible-UX boundary is, trust model, how to add your own auth |
| [`docs/security-notes.md`](docs/security-notes.md) | randomness, residual risks, admin trust assumptions, **audit status** |
| [`docs/scope-clarification.md`](docs/scope-clarification.md) | disclosure of the narrowed invisible-UX scope vs. the funding proposal |
| [`docs/battle-unit-test-fixes.md`](docs/battle-unit-test-fixes.md) | two upstream battle-engine unit tests that could never pass, how they were diagnosed, and how they were fixed (tests only — engine behaviour unchanged) |
| [`docs/deployment.md`](docs/deployment.md) | deploy / upgrade runbook for both contracts |
| [`PARAMETERIZE.md`](PARAMETERIZE.md) | which values are game-specific and still need to become parameters |

More documents (architecture, contracts, integration guide, deployment, FAQ,
maintenance) are on the publication roadmap — see the build log.

---

## Honesty notes

This template is deliberately blunt about its limits:

- **It has not been professionally audited.** Auditing was an explicit
  exclusion in the funded maintenance scope. Read
  [`docs/security-notes.md`](docs/security-notes.md) before putting real value
  behind it.
- **The commit-reveal RNG has a weaker block-hash fallback.** Use the fallback
  only for small pools. The exact conditions are in the security notes.
- **Admin powers are single-key.** There is no multisig or timelock wired up.
- **Some game-specific constants are still hardcoded** (`SG_SKILLS`, `SG_BONDS`,
  `FRAG_UNIT`, daily limits). See [`PARAMETERIZE.md`](PARAMETERIZE.md).

---

## License

This repository is **dual-licensed**. Read [`NOTICE`](./NOTICE) first.

| Path | License |
|---|---|
| `contracts/game/`, `docs/`, `frontend-demo/`, `scripts/` | **MIT** (root [`LICENSE`](./LICENSE)) |
| `contracts/common/`, `contracts/lottery/` | **Apache-2.0** — derived from the upstream Paxi contracts workspace (github.com/paxi-web3/paxi); each directory carries its own `LICENSE` + `NOTICE` |

Apache-2.0 code cannot be relicensed as MIT. If you redistribute this repo,
keep those two directories' `LICENSE`/`NOTICE` files intact.

---

## Funding

This template was funded by a Paxi DAO proposal (188,000 PAXI). The proposal
text, the scope clarification and the build log are public — the build log is
updated weekly with progress, testnet addresses and screenshots.

---

## License headers

Contributions are welcome. Please keep the existing comment style (the
severity-marker convention) and run `cargo fmt` + `cargo clippy` before
opening a pull request.
