# Deployment

How to build, store, instantiate and migrate the contracts in this template.

> This is a **generic** guide. It contains no production addresses and no
> deployment-specific settings — fill those in yourself.

---

## 1. Prerequisites

| Requirement | Why |
|---|---|
| Rust **1.69.0** + `wasm32-unknown-unknown` | the on-chain VM rejects `sign-ext` instructions; Rust 1.70+ prebuilt wasm std includes them. The repo-root `rust-toolchain.toml` handles this automatically. |
| A funded key on the target chain | pays gas for store / instantiate / migrate |
| A chain RPC + LCD endpoint | for queries |
| (optional) `wasm-opt` | shrinks the artifact; see §2 |

```bash
rustup toolchain install 1.69.0 --target wasm32-unknown-unknown --profile minimal
```

---

## 2. Build

```bash
cd contracts/game/contract        # or contracts/lottery/contract
cargo build --release --target wasm32-unknown-unknown
```

The artifact lands in `target/wasm32-unknown-unknown/release/*.wasm`.

**Size.** Most wasmd deployments cap the stored code (commonly 800 KiB).
If `store` reports `wasm code too large`, compress it:

```bash
docker run --rm -v "$PWD":/code \
  --mount type=volume,source=registry_cache,target=/usr/local/cargo/registry \
  cosmwasm/rust-optimizer:0.14.0
```

**Verify before uploading.** Never upload a file you have not hashed:

```bash
sha256sum target/wasm32-unknown-unknown/release/*.wasm
```

---

## 3. Store the code

```bash
paxid tx wasm store <contract>.wasm \
  --from <your-key> \
  --node <rpc-endpoint> \
  --chain-id <chain-id> \
  --gas auto --gas-adjustment 1.3 \
  --fees <fee> \
  -y
```

Get the new `code_id`:

```bash
paxid q wasm list-code --node <lcd-endpoint> -o json | jq -r '.code_infos[-1].code_id'
```

Confirm the chain stored the bytes you built:

```bash
paxid q wasm code <new-code-id> --node <lcd-endpoint> -o json | jq -r '.data_hash'
```

---

## 4. Instantiate

```bash
paxid tx wasm instantiate <code-id> '<instantiate-json>' \
  --from <admin-key> \
  --node <rpc-endpoint> --chain-id <chain-id> \
  --gas auto --gas-adjustment 1.3 --fees <fee> \
  --label "<label>" -y
```

**Set the admin to a key you control** — the admin can change parameters and
sweep balances. Prefer a multisig address; see
[`security-notes.md`](./security-notes.md) §3.3.

The generated schema tells you the exact `InstantiateMsg` shape:

```bash
cargo run --bin schema && cat schema/*.instantiate.json
```

---

## 5. Migrate an existing contract

Use `MsgMigrateContract` to point an existing contract at a new `code_id`,
in place. **State is preserved.**

```bash
paxid tx wasm migrate <contract-address> <new-code-id> '<migrate-json>' \
  --from <admin-key> \
  --node <rpc-endpoint> --chain-id <chain-id> \
  --gas auto --gas-adjustment 1.3 --fees <fee> -y
```

Notes:

- The `MigrateMsg` payload is usually `{}` — check the contract's `MigrateMsg`
  definition. Some fields are deliberately *not* read on migrate (for example
  a compliance flag), because Schema compatibility matters more than
  re-applying a flag that would change live behaviour.
- The migrate message can only be sent by the contract's **admin** — not by an
  operator or by a treasury multisig that only holds funds.

### Rollback

Keep the old `code_id`. If the new code misbehaves, migrate back to it:

```bash
paxid tx wasm migrate <contract-address> <old-code-id> '{}' --from <admin-key> ...
```

State is not lost in either direction.

---

## 6. Verify after deploying

1. **Code identity** — the contract reports the expected `code_id`:
   ```bash
   paxid q wasm contract <contract-address> --node <lcd-endpoint>
   ```
2. **Admin is set** — check the `admin` field is non-empty and that you control it.
3. **Config landed** — query the contract's config/params endpoint and compare
   against what you intended to set.
4. **A write path works** — exercise one cheap write (e.g. a small draw) and
   confirm the state change and the response attributes.

---

## 7. Clean up before migrating (important)

If you are migrating a contract that has **already been in production**, do
not skip this. A new code version may drop the exit paths for funds that are
still stuck in the old version.

The card-game contract in this template ships a dedicated pair of tools for
exactly this:

| Interface | Type | Purpose |
|---|---|---|
| `SanguoLegacyStatus {}` | Query | progress of the old version: unfinished battles, unclaimed rewards, unexecuted proposals, and the contract's actual balances |
| `AdminSweep { token, recipient, amount }` | Exec | admin-only sweep of funds out of the contract |

### Recommended order

1. **Open a withdrawal window** (2 days in the reference deployment) and
   announce it. Players withdraw via `Withdraw`; unfinished battles are
   cancelled/claimed/settled through the old channels. Player withdrawal must
   not be blocked by a token whitelist.
2. **Check the window closed.** Query `SanguoLegacyStatus`:
   - pending / finished-but-unclaimed counters should be zero (or match your
     off-chain records);
   - note the reported contract balances.
3. **Sweep.** `AdminSweep` the amount that is *not* owed to players
   (`contract_balance − player_owed`) to your operational wallet. Pass the
   recipient explicitly on every call — do not hardcode it in the contract.
4. **Migrate.**
5. **Optional tail:** once the old channels are drained, close them off
   (the reference version does not close them automatically).

> **Do not migrate first and sweep later.** The funds stay in the contract and
> are not lost — but retrieving them afterwards means shipping sweep logic
> again. Sweep first.

### AdminSweep safety rails (as implemented)

The contract side enforces:

- admin-only;
- rejects `recipient == the contract itself`;
- rejects zero amounts;
- **hard cap: `amount ≤ contract_balance − player_owed[token]`** — an attempt to
  sweep funds owed to players is rejected on-chain;
- decrements the house bankroll with a saturating subtraction and re-runs the
  solvency check afterwards;
- a failing PRC-20 balance query aborts the sweep (it does not let it through);
- works while the contract is paused.

Players' assets cannot be swept. Only genuinely unaccounted funds (e.g. tokens
sent to the contract by mistake) can be taken out.

---

## 8. Pre-flight checklist

- [ ] Built with Rust **1.69.0**; `sha256sum` recorded
- [ ] Artifact size under the chain's code limit
- [ ] Schema regenerated and committed
- [ ] Admin set to a multisig you control
- [ ] Contract instantiated with explicit, reviewed parameters
- [ ] For a production migration: legacy cleanup completed (§7) **before** migrate
- [ ] Old `code_id` recorded for rollback
- [ ] Post-deploy verification done (§6)
- [ ] Monitoring in place for the solvency counters and the sweep counter
