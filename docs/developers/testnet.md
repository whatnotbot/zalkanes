# Public testnet

Status is read from the repository's canonical release data
(`protocol/v0.toml`, `audit/gate-attestations.json`,
`audit/RC2-DECISION-RECORD.md`) and checked by `scripts/check-docs.sh`.

```text
Network status:    NOT YET ACTIVE
protocol:          v0
candidate:         audit-candidate-v0-rc2
activation height: 4,346,500
manifest hash:     06e3df62e5e98a3c276b583e038cbea8d05934e9d2a8e7c00299fec3140bf4bb
mainnet:           disabled
```

## What "not yet active" means

Protocol v0 is frozen in the signed release candidate
`audit-candidate-v0-rc2`, whose manifest sets the Zcash testnet activation
height to **4,346,500**. Below that height a Zalkanes node treats testnet
blocks as empty and its state root is the empty root. The fresh activation of
this candidate on the public testnet has been planned
(`audit/TESTNET-ACTIVATION-RUNBOOK.md`) but **not executed**: no RC2 node has
been deployed for it, no test ZEC has been spent on it, and no evidence has
been recorded (`audit/gate-attestations.json`,
`fresh_frozen_rc_testnet_activation: false`). Until that happens, use
regtest ([local-regtest.md](local-regtest.md)).

An earlier, pre-freeze deployment ran on testnet from height 4,338,100 with
the superseded RC1 manifest; its records (`docs/release/testnet-activation.md`,
`audit/TESTNET-EVIDENCE.md`) are historical evidence for that activation and
are not RC2. The hosted endpoint from that period is not part of the RC2
activation and should not be read as its state.

Mainnet is disabled by construction: `mainnet_activation_height = "None"`
in the manifest and `MAINNET_ACTIVATION_HEIGHT = None` in the code, and
every money-moving mainnet command also requires `--confirm-mainnet`.

## When the testnet is active

The procedure below is what the CLI supports today, so it will apply
unchanged once activation is executed and this page's status line flips to
`ACTIVE`. Items marked *to be published* are filled in by the activation
runbook when it is run.

**Zebra.** Run your own `zebrad` v6.3.0 on the Zcash public testnet, fully
synced. Zalkanes does not trust third-party RPC providers or lightwalletd.

**Zalkanes node.**

```bash
ZALKANES_NETWORK=testnet ZALKANES_RPC_URL=http://127.0.0.1:18232 \
ZALKANES_DATA_DIR=./zalkanes-data zalkanes node serve --port 3030
```

The node fast-forwards to the activation height and indexes from there.
Confirm `protocol_manifest_hash` in `zalkanes_getInfo` equals the value
above; a different hash means a different protocol.

**Public RPC endpoint.** *To be published* with the activation evidence. Any
node you run yourself is equivalent; at equal heights all nodes report the
same state root.

**Wallet and test ZEC.** Set `ZALKANES_NETWORK=testnet` and
`ZALKANES_ZCASH_RPC_URL` to your Zebra. For transparent funding, set
`ZALKANES_SIGNING_KEY` to a fresh 32-byte hex key, obtain test ZEC (TAZ) from
a testnet faucet to that key's transparent address (the CLI prints the address
in its error if `ZALKANES_FUNDING_TXID` is unset), then set
`ZALKANES_FUNDING_TXID`/`ZALKANES_FUNDING_VOUT`. For shielded funding, create
a wallet, fund its unified address from a faucet, `wallet scan`, then
`wallet unlock`. See [wallet-funding.md](wallet-funding.md). Never put a
signing key, seed, or passphrase in a file you commit.

**Build, deploy, call, view.** Identical to regtest except that nothing is
mined for you: `deploy` and `call` poll Zebra until the transaction has one
confirmation (testnet blocks are about 75 seconds apart), and `--wait` then
polls the node.

```bash
zalkanes contract build --manifest-path ./contracts/counter
zalkanes contract deploy ./contracts/counter/target/wasm32-unknown-unknown/release/counter.wasm --funding transparent --yes --wait
zalkanes contract call <contract-id> 1 "" --funding transparent --yes --wait
zalkanes contract view <contract-id> 2 "" --rpc-url http://127.0.0.1:3030
```

**Debugging.** `zalkanes_getExecution` with a txid (display order) shows
whether a call succeeded, its fuel, output, and error; `zalkanes_getBlockExecutions`
lists every execution in a block; `zalkanes_getInfo` shows whether the node
is still syncing. Compare state roots between two nodes at the same height
with `scripts/compare-state-roots.sh`. A Zcash testnet block explorer shows
the raw transaction, its OP_RETURN, and (for DEPLOY) the carrier inputs.

## Deployment runbook

The operator procedure for bringing up two clean RC2 nodes, proving they
agree, and running the one-time platform acceptance is
`docs/release/testnet-rc2-deployment.md`. It is prepared and not executed.

## Historical evidence

- `docs/release/testnet-activation.md`: the pre-freeze testnet deployment
  (activation 4,338,100, RC1 manifest), with txids and roots.
- `audit/LIVE-ACCEPTANCE-EVIDENCE.md`: shielded CALLs and a shielded-funded
  deploy on that testnet, without secrets.
- `audit/RC2-DECISION-RECORD.md`: how 4,346,500 was chosen.
