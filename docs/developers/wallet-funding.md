# Wallet and funding

## What is and is not private

Read this first, because it is the most common misunderstanding.

**Shielded funding does not make smart-contract execution private.** A
contract's code, every CALL's calldata (contract id, opcode, input), every
execution record, and all contract state are public: cleartext in Zcash
transactions and in every Zalkanes node's database. They are equally public
whether the transaction that carried them was funded from transparent coins
or from shielded notes.

What shielded funding hides is the **source of funds and the change**: which
notes paid the fee, and where the change went. That is useful (it does not
link your contract activity to a transparent address) but it is all it does.
The CLI prints this boundary before every shielded spend:

```text
Privacy boundary: the ZALK message (contract id, opcode, calldata,
and deployed code) is ALWAYS public on chain. Shielded funding hides
only the SOURCE OF FUNDS, never the contract interaction.
```

## Which stage can use which pool

| stage | transparent | shielded |
|---|---|---|
| CALL | yes | yes |
| PREPARE (creates the carrier outputs) | yes | yes |
| DEPLOY (spends the carriers) | **always transparent** | no |

DEPLOY spends P2SH carrier outputs, which are transparent by construction, so
a "shielded deploy" is really:

```text
shielded PREPARE  (notes pay; creates public P2SH carrier UTXOs)
        ↓
transparent DEPLOY  (spends those carriers; publishes the code)
```

`zalkanes contract deploy --funding shielded` does exactly this and says so
in its output (`── DEPLOY (transparent carrier spends) ──`).

## Funding modes

`--funding` on `deploy` and `call`:

| mode | behaviour |
|---|---|
| `transparent` (default) | spends transparent UTXOs controlled by the signing key; never touches shielded notes |
| `shielded` | spends shielded notes from the wallet; never silently falls back to transparent |
| `auto` | prefers shielded when a wallet exists, is unlocked, and can cover the amount alone; otherwise transparent. Never mixes pools in one transaction |

## Transparent funding

Transparent operations are signed with a secp256k1 key from
`ZALKANES_SIGNING_KEY` (32-byte hex). If it is unset the CLI uses a fixed
development key and warns:

```text
WARNING: using deterministic dev signing key (regtest only)
```

Never use the dev key outside regtest; it is public.

- **Regtest**: each transparent deploy or call mines 110 blocks to the key's
  P2PKH address and spends the matured coinbase. `zalkanes contract fund`
  does the mining on its own and prints the address.
- **Testnet**: the CLI does not mine. Send test ZEC to the key's transparent
  address, then set `ZALKANES_FUNDING_TXID` (and `ZALKANES_FUNDING_VOUT`,
  default `0`) to that payment; the CLI verifies the output pays the address
  and spends it. Without them, deploy/call fail with a message that includes
  the address to fund.

## Shielded funding: the wallet

The wallet is a separate store from the node's consensus state. It is
`no_std`-free tooling, not part of consensus, and is only needed for
`--funding shielded|auto`.

| command | does |
|---|---|
| `zalkanes wallet create` | generates a seed, encrypts it into `keystore.age`, prints the unified address |
| `zalkanes wallet restore --birthday <height>` | rebuilds the wallet database from an existing `keystore.age` in the wallet directory |
| `zalkanes wallet address` | prints the unified address to fund |
| `zalkanes wallet scan` | scans to the chain tip; works while locked |
| `zalkanes wallet balance` | spendable balance in zatoshi |
| `zalkanes wallet status` | lock state, birthday, balance, sync position |
| `zalkanes wallet unlock --ttl 900` | arms spending for the given seconds (default 900) |
| `zalkanes wallet lock` | disarms immediately |

Files live in `ZALKANES_WALLET_DIR` (default `~/.zalkanes/wallet/<network>`):
`keystore.age` (the encrypted seed; back it up), `wallet.sqlite`,
`journal.sqlite`, and a `session` marker. The passphrase is prompted for, or
read from `ZALKANES_PASSPHRASE` for scripts.

Custody model, in brief: the seed exists only inside `keystore.age`.
`unlock` verifies the passphrase and writes an expiring session marker that
contains no key material; each shielded spend still requires the passphrase,
decrypts the seed for that one command, and drops it. A locked wallet can
scan, show its address, and report its balance, but refuses to spend:

```text
Error: wallet is LOCKED: run `zalkanes wallet unlock` before a shielded spend
```

`--funding shielded` without any wallet fails rather than falling back:

```text
Error: --funding shielded requires a wallet; run `zalkanes wallet create`
```

A shielded CALL, once the wallet has a balance and is unlocked:

```bash
zalkanes contract call <contract-id> 1 "" --funding shielded --yes --wait
```

The ZALK payload it produces is byte-identical to the transparent one.

### Where shielded funding has been exercised

The CLI cannot create shielded notes on regtest (coinbase pays a transparent
address and there is no shield command), so shielded funding is exercised on
the public testnet with faucet funds: shielded CALLs, payload equality with
transparent CALLs, and a shielded PREPARE followed by a transparent DEPLOY
are recorded, with txids and no secrets, in `audit/LIVE-ACCEPTANCE-EVIDENCE.md`.
The regtest developer flow in these docs uses transparent funding.

## What you do not need to know

The shielded path builds a PCZT, proves, signs, verifies the extracted
transaction against the plan, journals every stage, and broadcasts through
Zebra. None of that changes what the contract sees. ADR-0007 and
`audit/WALLET-PRIVACY.md` have the details if you want them.

## Mainnet

Mainnet contract execution is disabled (`MAINNET_ACTIVATION_HEIGHT = None`).
Money-moving commands on mainnet additionally refuse to run without
`--confirm-mainnet`, and `--yes` does not imply it. Do not send mainnet ZEC
to anything in this repository.
