# Wallet security and privacy boundary

## What Zalkanes does NOT hide

Contract interaction is **public by construction**. The ZALK OP_RETURN
message (contract id, opcode, calldata), the deployed WASM carried in P2SH
scriptSigs, every execution record, and all contract state are in cleartext
on the Zcash chain and in every indexer's database.

**Shielded funding hides only the source of funds** — which notes paid the
fee. It does not make the contract call private. Any claim to the contrary
would be false.

`DEPLOY` is **always** a transparent carrier-spending transaction. With
`--funding shielded`, only the PREPARE stage is shielded; the carrier
outputs it creates are public by design (ADR-0007). The CLI prints this
boundary before signing.

## Privacy policy vocabulary

Each plan reports the minimum Zallet-style policy it requires, derived from
what it actually reveals:

| operation | policy | reveals |
|---|---|---|
| shielded CALL | `FullPrivacy` | nothing beyond the (public) ZALK message |
| shielded PREPARE | `AllowRevealedAmounts` | carrier output amounts |
| transparent / DEPLOY | `AllowFullyTransparent` | addresses and amounts |

This is deliberately separate from the always-public contract disclosure.

## Key custody

The seed exists **only** inside `keystore.age`, an `age` passphrase-encrypted
file (scrypt KDF + ChaCha20-Poly1305). No cryptography is implemented in this
project; `age` is the established, audited format maintained by the same
author as the Zcash Rust crates.

- No plaintext seed or spending key is written to RocksDB, the wallet SQLite
  database, the operation journal, logs, or evidence files. Tests assert the
  raw seed bytes appear in neither the keystore file nor the wallet DB nor
  any printable wallet output.
- The keystore carries an authenticated header (magic, version, network,
  account). Wrong network, wrong version, bad magic, truncation, and any
  ciphertext corruption all fail loudly.
- Creation is atomic (temp + fsync + rename, mode 600) and refuses to
  overwrite an existing seed.
- `UnlockedSeed` is a zeroizing `SecretVec` with no `Debug`/`Display`.

## Locked vs unlocked

A wallet reopens **LOCKED** every time; unlock never persists across restart.

| capability | LOCKED | UNLOCKED |
|---|---|---|
| open wallet DB | yes | yes |
| scan / sync | yes | yes |
| address, balance, status | yes | yes |
| shielded spend authorization | **refused** | permitted |

Locked is watch-capable because the viewing key already lives in the wallet
database; spend authorization requires the seed. `unlock_with_seed` verifies
the supplied seed actually derives **this** account — a valid but unrelated
seed is rejected rather than silently used.

## Ephemeral-CLI session model

A one-shot CLI cannot hold authorization in memory between invocations, and
persisting a decrypted seed would defeat the keystore. So `wallet unlock`
writes an **armed-session marker that contains no key material** (owner-only,
with an expiry, default 900 s). Spending commands require BOTH an armed
session AND the passphrase; the seed is decrypted for that single command and
dropped at exit. A stolen session file therefore grants nothing. `wallet lock`
removes the marker and spending is refused immediately.

This is stricter than a seed-caching session; the trade-off is a passphrase
prompt per spending command (`$ZALKANES_PASSPHRASE` for automation).

## Transaction authorization boundary

Shielded: `FundingPlan` → `prove` → `sign` → **`VerifiedPczt`** → canonical
`TransactionExtractor` → **`VerifiedTransaction`** → journal-integrated
broadcaster.

Transparent: `FundingPlan` → `sign` → **`VerifiedTransaction`** → broadcaster.

Both `VerifiedPczt` and `VerifiedTransaction` have private fields and no
public constructor, so only the verification paths can produce them, and the
production extractor/broadcaster accept nothing else. There is no raw-PCZT or
raw-bytes production path anywhere in the workspace.

## Wallet store separation

Consensus state (RocksDB) and wallet state (SQLite: notes, trees, locks) are
separate databases with **no cross-store atomicity** — recovery never assumes
they moved together (see `audit/TESTING.md` and
`crates/zalkanes-wallet/src/recovery.rs`).
