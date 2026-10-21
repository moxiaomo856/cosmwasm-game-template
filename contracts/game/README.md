# contracts/game — card-game contract (MIT)

The card-game contract: cw721-style NFT cards, a deterministic on-chain battle
engine, stage progression, a fragment economy, and a PRC-20 asset layer.

```
contracts/game/
└── contract/          # the Rust crate
    ├── Cargo.toml
    ├── README.md      # ← capabilities, API, config, build
    └── src/
```

**Full documentation: [`contract/README.md`](./contract/README.md).**

## The API is the schema

```bash
cd contract
cargo run --bin schema      # → ./schema/*.json
```

The generated schema is the authoritative message reference — field names
exactly as they go on chain. Do not hand-maintain a message list.

## Things to do before you ship this

- [ ] Parameterise the remaining hardcoded game data — see
      [`../../PARAMETERIZE.md`](../../PARAMETERIZE.md)
      (`SG_SKILLS` 60 cards, `SG_BONDS` 23 bonds, `FRAG_UNIT`, daily limits).
- [ ] Decide on your authentication strategy — the template ships direct
      sender signing only. See
      [`../../docs/invisible-ux-boundaries.md`](../../docs/invisible-ux-boundaries.md).
- [ ] Replace the admin whitelist with a multisig, and add a timelock to
      treasury sweeps.
- [ ] Read [`../../docs/security-notes.md`](../../docs/security-notes.md) —
      this contract has **not** been professionally audited.

## License

MIT (see the repository-root `LICENSE`).
