# Zalkanes operator guide

Production operations for a Zalkanes indexer node and wallet. This release
is **pre-mainnet**: `MAINNET_ACTIVATION_HEIGHT = None`, so contract execution
is disabled on mainnet by construction.

---

## 1. Security and privacy model — read this first

Zalkanes is a **metaprotocol indexer**, not a chain and not a private
computation system.

- **Everything about a contract interaction is PUBLIC.** The ZALK message
  (contract id, opcode, calldata), the deployed WASM, and all contract state
  are in cleartext on the Zcash chain and in every indexer's database.
- **Shielded wallet funding does NOT make contracts private.** It hides only
  the *source of funds* (which notes paid the fee). The contract call itself
  is equally public whether funded transparently or from shielded notes.
- `DEPLOY` is **always** a transparent carrier-spending transaction. With
  `--funding shielded`, only the PREPARE stage is shielded; the carrier
  outputs it creates are public by design.
- Zalkanes does **not** re-verify Zcash consensus. It trusts the Zebra node
  you point it at. A compromised or forked Zebra yields a divergent index —
  which is why you must run your own.

---

## 2. Supported base node

| item | value |
|---|---|
| Node | Zebra (`zebrad`) |
| Pinned version | **6.3.0** |
| Transport | JSON-RPC over loopback / private network only |
| Auth | cookie auth disabled only when the port is not publicly reachable |

Zalkanes validates the node's network and version at startup and refuses to
index a mismatched or unsynced node. Never point it at a public RPC
provider, lightwalletd, or a block explorer.

---

## 3. Hardware and storage

| resource | guidance |
|---|---|
| CPU | 2+ cores. Contract execution is single-threaded and fuel-metered. |
| RAM | 4 GB minimum for the indexer; Zebra wants considerably more. |
| Disk (Zebra) | Size for the target network's full chain; testnet ~50 GB today. |
| Disk (Zalkanes) | Grows with deployed code + contract storage only. |
| Filesystem | Must not be capacity-starved — see §11. |

---

## 4. Configuration

All configuration is environment-driven.

| variable | meaning |
|---|---|
| `ZALKANES_NETWORK` | `mainnet` \| `testnet` \| `regtest` |
| `ZALKANES_ZCASH_RPC_URL` | Your Zebra JSON-RPC endpoint |
| `ZALKANES_DATA_DIR` | RocksDB consensus state directory |
| `ZALKANES_WALLET_DIR` | Wallet directory (default `~/.zalkanes/wallet/<network>`) |
| `ZALKANES_URL` | Zalkanes JSON-RPC endpoint, for client commands |
| `PORT` | JSON-RPC bind port (health server binds `PORT+1`) |
| `ZALKANES_PASSPHRASE` | Non-interactive keystore passphrase (automation only) |

### Ports

| port | service |
|---|---|
| `PORT` (default 3030) | Zalkanes JSON-RPC |
| `PORT + 1` | health/readiness (`/health`, `/ready`) |
| 8232 / 18232 | Zebra RPC (mainnet / test-regtest), private only |

> The Zalkanes JSON-RPC has **no authentication and no rate limiting**. Do not
> expose it directly to the internet; put it behind a reverse proxy or keep it
> on a private network.

### Directories

| path | contents | back up? |
|---|---|---|
| `$ZALKANES_DATA_DIR` | RocksDB consensus state | No — rebuildable by reindex |
| `$ZALKANES_WALLET_DIR/keystore.age` | **Encrypted seed** | **YES — irreplaceable** |
| `$ZALKANES_WALLET_DIR/wallet.sqlite` | Notes, trees, locks | Yes (rebuildable by restore, slowly) |
| `$ZALKANES_WALLET_DIR/journal.sqlite` | Operation lifecycle journal | Yes |
| `$ZALKANES_WALLET_DIR/session` | Armed-session marker (no secrets) | No |

**Wallet separation:** consensus state (RocksDB) and wallet state (SQLite)
are separate stores. No seed or spending key is ever written to RocksDB, the
journal, logs, or evidence files.

---

## 5. Key custody

The seed exists **only** inside `keystore.age` — an `age` passphrase-encrypted
file (scrypt + ChaCha20-Poly1305). Losing the keystore *or* the passphrase
loses the funds.

```
zalkanes wallet create             # generates a seed, encrypts it, prints the UA
zalkanes wallet restore --birthday <height>   # from an existing keystore
zalkanes wallet address | balance | status
zalkanes wallet scan               # works while LOCKED
zalkanes wallet unlock [--ttl N]   # arms spending (verifies the passphrase)
zalkanes wallet lock               # disarms immediately
```

**Locked vs unlocked.** A wallet reopens LOCKED every time. Locked is
watch-capable: scanning, address, balance, and status all work from the
viewing key in the wallet database, while any shielded spend is refused.
`unlock` writes an expiring, owner-only session marker that **contains no key
material**; each spending command still requires the passphrase, and the seed
is decrypted for that one command and dropped at exit. A stolen session file
therefore grants nothing.

---

## 6. Startup order

1. Start Zebra; wait until it reports fully synced.
2. Start the Zalkanes indexer (`zalkanes node serve`). It validates the
   node's network/version and the protocol manifest hash, then indexes from
   the activation height.
3. Poll `/ready` on `PORT+1` until it returns 200.

---

## 7. Initial sync

The indexer fast-forwards to the activation height, then processes one block
at a time. Progress is logged per block with the resulting state root. On a
fresh database, expect the initial catch-up to be RPC-bound.

---

## 8. Health, readiness, monitoring

| endpoint | meaning |
|---|---|
| `GET /health` (`PORT+1`) | process is alive |
| `GET /ready` | 200 synced, 202 syncing, 503 unhealthy |
| `zalkanes_getInfo` | height, block hash, **state root**, manifest hash, syncing |

**Monitor the state root.** It is the single number that proves your node
agrees with the network.

### Comparing roots between nodes

```
for url in http://node-a:3030 http://node-b:3030; do
  curl -s -X POST "$url" -H 'Content-Type: application/json' \
    -d '{"jsonrpc":"2.0","id":1,"method":"zalkanes_getInfo","params":[]}' \
  | python3 -c 'import json,sys; r=json.load(sys.stdin)["result"]; print(r["indexed_height"], r["state_root"])'
done
```

At equal heights the roots **must** be byte-identical. A mismatch means one
node is running different code, a different protocol manifest, or a divergent
Zebra — stop and investigate before trusting either. `scripts/compare-state-roots.sh`
automates this across N nodes.

Also alert on: `protocol_manifest_hash` changing unexpectedly, `indexed_height`
falling behind Zebra's tip, and readiness flapping.

---

## 9. Restart, reindex, reorg

**Restart.** Stop and start; the indexer resumes from its persisted height.
Commits are atomic (data, undo journal, metadata, height record and root all
land in one write batch), so a crash leaves the last COMPLETE block.

**Clean reindex.** Stop the node, delete `$ZALKANES_DATA_DIR`, restart. The
resulting root at a given height must equal the root the old database had at
that height.

**Reorg.** The indexer compares the stored hash at its tip against Zebra and
rolls back via the undo journal to the common ancestor, then re-indexes. The
wallet performs the equivalent rewind/rescan against its note commitment
trees, refusing to rewind past its birthday (that requires a restore at an
earlier birthday).

**In-flight transactions during a reorg.** The journal keeps note
reservations held for anything that might still be in flight and never
releases them on an ambiguous outcome. After a reorg, run
`zalkanes wallet status` to let reconciliation resolve broadcast-phase rows
against your Zebra by txid.

---

## 10. Upgrades and rollback

1. Read the release notes for protocol-manifest changes. **If the manifest
   hash changes, it is a protocol change** — coordinate, don't roll it out
   silently.
2. Upgrade one node first and compare its root against an un-upgraded node at
   the same height. They must match unless the release explicitly states
   otherwise.
3. Rollback: reinstall the previous binary. If the new version wrote state a
   downgrade cannot read, delete the data directory and reindex.

Verify a release artifact before installing:

```
sha256sum -c SHA256SUMS
cat MANIFEST.txt      # source commit, toolchain, protocol manifest hash
```

---

## 11. Database corruption and disk exhaustion

The store is built to be loud, never ambiguous:

- **Disk full** — the failing commit errors; the persisted height stays at the
  last complete block. Free space and restart.
- **Read-only files / permission loss** — writes refuse loudly.
- **Truncated `CURRENT` or corrupted `MANIFEST`** — the store refuses to open.

Recovery for any of these: restore free space/permissions and restart, or
delete `$ZALKANES_DATA_DIR` and reindex. The wallet database is *not*
rebuildable from the chain alone — restore it from backup or from the
keystore with `wallet restore --birthday`.

---

## 12. Logs

`RUST_LOG` / `ZALKANES_LOG` control verbosity (`zalkanes=info` default). Logs
record heights, roots, execution outcomes and fuel. **No secret material is
ever logged**: seeds, passphrases, and spending keys never enter log output.

---

## 13. Mainnet status

```
MAINNET_ACTIVATION_HEIGHT = None
```

Contract execution on mainnet is disabled in this release. Money-moving
mainnet CLI commands additionally require `--confirm-mainnet` (and `--yes`
does not imply it). Activation is gated on external audit — see
`audit/KNOWN-LIMITATIONS.md`.
