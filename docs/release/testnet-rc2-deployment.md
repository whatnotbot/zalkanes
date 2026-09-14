# Public testnet deployment runbook: RC2 (`audit-candidate-v0-rc2`)

**Status: PREPARED, NOT EXECUTED.** Nothing in this document has been run
against the public testnet. Executing it broadcasts real testnet transactions
and changes the hosted infrastructure, and requires explicit owner
authorisation. Mainnet is not authorised and stays disabled.

This is the operational companion to `audit/TESTNET-ACTIVATION-RUNBOOK.md`
(which explains *why* the fresh activation is a release event). It lists the
exact commands. Nothing here changes protocol v0.

## 0. Fixed parameters

| item | value |
|---|---|
| protocol manifest (`protocol/v0.toml`) | SHA-256 `06e3df62e5e98a3c276b583e038cbea8d05934e9d2a8e7c00299fec3140bf4bb` |
| testnet activation height | `4,346,500` |
| mainnet activation | `None` (unchanged) |
| Zebra | own full validator, v6.3.0 (`zebra-testnet` on Railway, already synced; **not wiped, not reconfigured**) |
| node binary | built from `main` at the merged release SHA, `cargo build --release --locked` (Dockerfile) |

Verify before anything else, on the checkout being deployed:

```bash
git rev-parse HEAD
shasum -a 256 protocol/v0.toml        # must print 06e3df62…
scripts/check-docs.sh                 # metadata consistency
scripts/mainnet-gate.sh --no-network  # must still report MAINNET RELEASE BLOCKED
```

## 1. Why the existing Railway indexers are retired, not reused

`zalkanes-testnet` serves a state root (`28256d5f…`) produced under the
superseded activation (4,338,100) from a database that was not cleaned; under
the RC2 manifest a node below 4,346,500 must have the empty root
(`b120099c…`). `zalkanes-testnet-b` and `zalkanes-testnet-a2` were started
clean but were created by hand, not from source control, and all three stopped
indexing on the transient-RPC bug fixed in PR #8. None of them is acceptance
evidence. Their volumes are preserved as historical evidence and are never
wiped or reused.

## 2. Provision two clean indexers (owner action)

`.railway/railway.ts` now defines `zalkanes-testnet-rc2-a` and
`zalkanes-testnet-rc2-b`, each on a **new, empty** volume, both reading
`http://zebra-testnet.railway.internal:18232`. Apply the IaC (or create the
two services in the dashboard with exactly those variables and new volumes),
deploying from `main` at the release SHA. Expose a public domain on each.

Then, for each service:

```bash
railway service zalkanes-testnet-rc2-a && railway logs -n 20
```

Expect `chain source connection verified`, then a run of
`fast-forwarded below activation height` lines, then `caught up` polling. The
indexer never exits on `Provided index is greater than the current tip`
anymore; it logs `transient upstream failure; retrying on the next tick`.

## 3. Prove the two nodes are clean, current, and in agreement

```bash
scripts/testnet-two-node-check.sh https://<rc2-a-domain> https://<rc2-b-domain>
```

Passes only when both nodes advertise the RC2 manifest, report
`indexer.state` of `healthy` or `syncing`, and agree on indexed height,
indexed block hash, and state root at that height. Below activation the root
must be `b120099c167da673588b15aa827c3bbd9339a9a2d934b0d20c99cebe69f8ffe6`
on both. Run it on a schedule until activation and for 24 hours after.

Also check readiness on the health port from inside the project (it is
`PORT+1`, not exposed publicly): `GET /ready` must be 200 or 202, never 503.

## 4. Wait for the activation height

Do not broadcast anything before both nodes report
`indexed_height >= 4,346,500` and still agree. The lead was chosen so no
pre-existing testnet history is reinterpreted.

## 5. Platform acceptance (exactly once; owner-authorised)

On an operator machine with the release build, your own Zebra testnet RPC
(`ZALKANES_ZCASH_RPC_URL`), and test ZEC:

```bash
export ZALKANES_NETWORK=testnet
export ZALKANES_ZCASH_RPC_URL=http://<your-zebra>:18232
export ZALKANES_URL=https://<rc2-a-domain>
export ZALKANES_SIGNING_KEY=<fresh 32-byte hex, never committed>
# fund that key's transparent address from a faucet, then:
export ZALKANES_FUNDING_TXID=<faucet txid> ZALKANES_FUNDING_VOUT=0
ZALKANES_TESTNET_ACCEPTANCE=yes scripts/testnet-acceptance.sh
```

The script builds the counter, deploys it (PREPARE + DEPLOY), verifies the
indexed code hash equals the built WASM and prints the `ContractId`, then
checks `get == 0`, increments, `get == 1`, increments, `get == 2`, saving an
evidence log with every txid, height, fee, root, and fuel value (no secrets).

Shielded-funded flow (supported developer interface, ADR-0007): repeat the
deploy with `FUNDING=shielded` after `zalkanes wallet create`, funding the
unified address from a faucet, `wallet scan`, and `wallet unlock`. The
resulting ZALK payloads must be byte-identical to the transparent ones.

## 6. Restart, clean reindex, agreement (steps 9 to 12)

1. Restart node A (`railway service zalkanes-testnet-rc2-a && railway redeploy`
   or a restart from the dashboard). After it reports caught up, `VIEW get`
   must still be 2 and the state root unchanged.
2. Clean-reindex node B: attach a **new** empty volume (or delete the contents
   of its `/data/zalkanes` while stopped) and start it. It fast-forwards to
   activation and replays every block from Zebra alone.
3. `scripts/testnet-two-node-check.sh <A> <B>` must report AGREEMENT: same
   height, same indexed block hash, same root. `VIEW get` on B must be 2 and
   `zalkanes_getContract` on B must return the same `ContractId` and code hash.
4. `zalkanes_getExecution` for both CALL txids must match on A and B
   (`success`, `fuel_used`, roots before and after).

## 7. Evidence to record

Into `audit/TESTNET-ACTIVATION-RC2-EVIDENCE.md`, with no seeds, keys, or
passphrases: the release SHA and CI run IDs; both nodes' `zalkanes_getInfo`
at the same height before activation (empty root) and after acceptance; the
acceptance evidence log; restart and reindex roots; the two-node check output.
`docs/developers/testnet.md`'s status line flips to `ACTIVE` only after
`audit/gate-attestations.json` records the evidence (the docs check enforces
this).

## 8. Retire the superseded indexers

After §6 is green: stop `zalkanes-testnet`, `zalkanes-testnet-b`, and
`zalkanes-testnet-a2`. Keep their volumes. Remove the public domain from
`zalkanes-testnet` so nobody reads superseded state as current. Delete the
unused domains accidentally created on `zebra-testnet`, `zebra`, and
`zalkanes-rc2` (they serve nothing, but should not exist).

## 9. Abort conditions

Stop and do not "pick a winner" if: the two nodes disagree at equal height;
either node reports `indexer.state` `stalled` or `dead`; any node advertises a
manifest hash other than `06e3df62…`; a planned fee exceeds 100,000 zat; or
`mainnet_activation_height` is anything but `"None"`. Rollback semantics are in
`audit/TESTNET-ACTIVATION-RUNBOOK.md` §6.
