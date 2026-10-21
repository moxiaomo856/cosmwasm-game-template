# frontend-demo — reference front-end

> **Status: placeholder.** The demo is not written yet; it is tracked as an  
> open item in [`../PARAMETERIZE.md`](../PARAMETERIZE.md). This file describes  
> what it will cover so the scope is fixed before anyone starts.

A small React app that exercises **both** contracts end to end, so a new  
developer can see a working integration in one place.

---

## What it will cover

### Card game

- connect wallet, read the card list / balance / config from the contract
- draw a card, upgrade a card, decompose a card
- create a battle, join it, settle it, claim the reward
- read the generated JSON schema instead of hand-written message builders

### Lottery

- read the available tiers
- create a pool, join a pool, activate a platform template pool
- draw and claim, and see the fee split

---

## Design constraints

These are fixed, not open questions:

**1. Direct signing only.** The template ships no session-key mechanism, so the  
demo signs each transaction with the connected wallet. There is no  
"register session" flow and no session-key storage in the demo.

**2. Optional feegrant relay, clearly labelled.** If the demo includes a  
feegrant-based relayer, it must be visibly separated from the default path and  
documented in the trust-boundary panel. The relay is **not** part of the  
open-source template — see  
[`../docs/invisible-ux-boundaries.md`](../docs/invisible-ux-boundaries.md).

**3. The schema is the source of truth.** Message builders should be generated  
from `schema/*.json`, not hand-written, so the demo cannot drift from the  
contract.

**4. Chain configuration is externalised.** RPC endpoint, chain id, contract  
addresses and the admin/treasury addresses belong in a single config file  
that is **not** committed with production values.

---

## A note on the source material

The demo will be adapted from the card-game front-end and the lottery  
front-end that run against the production contracts. Those applications are  
**not** part of this template — they are much larger, they carry  
deployment-specific configuration, and they depend on the invisible-UX layer.  
What goes into the demo is a *small, generic* subset: enough to demonstrate  
the contracts, not to replace either product.

Anything carried over must be scrubbed of: production contract addresses,  
internal config, analytics/identifiers, and any code that depends on the  
session mechanism.

---

## Status checklist

- [ ] Scaffold (Vite + TypeScript) and config externalisation
- [ ] Schema-driven message builders for both contracts
- [ ] Card-game flows
- [ ] Lottery flows
- [ ] Trust-boundary panel (who signs, who pays gas, what is authorised)
- [ ] README with a runnable walkthrough
