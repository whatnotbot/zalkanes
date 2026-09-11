# Zalkanes Funding Compatibility Rule (non-negotiable)

## One contract system, two funding modes

Every normal user-facing, state-changing Zalkanes operation MUST work with **both**
a transparent-funded and a shielded-funded Zcash transaction:

| operation | transparent wallet | shielded wallet |
|-----------|--------------------|-----------------|
| DEPLOY    | works              | works           |
| CALL      | works              | works           |
| send value (where supported) | works | works   |

```text
transparent wallet ──┐
                      ├─▶ Zcash transaction ─▶ Contract A ─▶ Contract B ─▶ Contract C
shielded wallet ──────┘
```

Contract-to-contract execution MUST behave identically regardless of how the
*outer* Zcash transaction was funded. Contracts do not originate Zcash
transactions; a wallet does, and that transaction triggers Zalkanes execution.

## Funding mode is wallet-level only

Funding mode MUST NOT alter any consensus behavior:

- protocol decoding
- opcode
- calldata
- WASM execution
- fuel semantics
- storage semantics
- contract-to-contract calls
- state root

For the same logical call:

```text
ZALK payload (transparent funding)  ==  ZALK payload (shielded funding)
```

byte-for-byte. This is enforced structurally: the ZALK payload is the
funding-pool-agnostic `TxRequest`'s OP_RETURN bytes, and **both** funding
sources serialize it with the shared `zalkanes_tx::op_return_script` helper.
`FundingPlan::intent_hash` commits those exact bytes regardless of pool.

## Shielded-funding privacy boundary

For shielded funding:

```text
funding note/address ──▶ shielded
change                 ──▶ shielded
ZALK contract call     ──▶ public
contract execution     ──▶ public
```

Deployment is correctly described as "shielded-funded" even though the WASM
carrier stage is public:

```text
shielded wallet
    ─▶ shielded PREPARE
    ─▶ public carrier UTXOs
    ─▶ public DEPLOY
```

The contract code and carrier are always public; only the funding/change side
gains shielded-pool privacy. Zalkanes does NOT claim private contract
execution.

## CLI

```text
zalkanes contract deploy app.wasm --funding shielded     # == semantics
zalkanes contract deploy app.wasm --funding transparent

zalkanes contract call <id> swap ... --funding shielded  # == semantics
zalkanes contract call <id> swap ... --funding transparent
```

`--funding transparent`, `--funding shielded`, and `--funding auto` are all
first-class. `auto` prefers shielded and falls back to transparent only for the
stages capable of shielded funding (PREPARE and CALL); DEPLOY is always a
transparent carrier-input transaction.

## Acceptance

Mainnet readiness is NOT declared until real testnet transactions prove both
transparent-funded and shielded-funded end-to-end (DEPLOY + CALL + state
transition), with the byte-equality test passing.
