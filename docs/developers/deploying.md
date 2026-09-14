# Deploying contracts

## The simple view

```text
compiled WASM
     ↓
PREPARE transaction: creates N carrier outputs (one per 1400-byte chunk)
     ↓
DEPLOY transaction: spends the carriers, each input carrying one chunk,
                    plus one OP_RETURN with the DEPLOY message
     ↓
every Zalkanes node reconstructs the WASM from the DEPLOY inputs
     ↓
SHA-256(reconstructed) must equal the code hash in the message
     ↓
module validated; ContractId derived deterministically
     ↓
contract is executable from the block that contains DEPLOY
```

One command does all of it:

```bash
zalkanes contract deploy ./contracts/counter/target/wasm32-unknown-unknown/release/counter.wasm --funding transparent --yes --wait
```

| flag | meaning |
|---|---|
| `--funding transparent\|shielded\|auto` | which pool pays for PREPARE (see [wallet-funding.md](wallet-funding.md)); DEPLOY always spends the transparent carriers |
| `--yes` | skip the `Proceed? [y/N]` prompt (needed in scripts) |
| `--wait` | after DEPLOY is mined, poll the Zalkanes node until the contract is indexed and print its record |
| `--dry-run` | print the PREPARE plan and stop; nothing is signed or broadcast |
| `--confirm-mainnet` | required on mainnet in addition to `--yes`; mainnet execution is disabled in this release anyway |

The CLI validates the module first (same rules as every node), then prints
the size, chunk count, and code hash, then runs the two stages. On regtest
each stage is mined immediately; on testnet the CLI polls until each
transaction has one confirmation.

What it costs, on regtest, for the 2,513-byte counter (2 chunks):

| stage | ZIP-317 fee |
|---|---|
| PREPARE | 15,000 zat |
| DEPLOY | 95,000 zat |

For the 6,723-byte token (5 chunks) DEPLOY was 255,000 zat. Fees grow with
module size because each carrier input is about 1.5 KB of transaction data.

## Capturing the ContractId

The last lines of a successful deploy:

```text
DEPLOY mined at height 222: 99db762091a5ae5642fd5fb1ce00d45be1a44c64d7cf4b556a425445bfeee0ea
ContractId: 2d7e818e524195f15ee513dc5fa3c58999e99f4e2e35a765a244a6f4526e237f
indexed: {"code_hash":"fa8289fb…","code_size":2513,"contract_id":"2d7e818e…"}
```

The id is computed locally from the DEPLOY txid and the code hash
([contract-model.md](contract-model.md#contractid)); `--wait` confirms that
the node derived the same id. In a script:

```bash
CONTRACT_ID=$(zalkanes contract deploy "$WASM" --funding transparent --yes --wait | sed -n 's/^ContractId: //p')
```

You can also ask any node:

```bash
curl -s -X POST http://127.0.0.1:3030 -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","id":1,"method":"zalkanes_getContract","params":["<contract-id>"]}'
```

`zalkanes_getCode` returns the full WASM as hex.

## Three facts to design around

- **Code availability is on-chain.** The bytes live in the DEPLOY
  transaction's inputs. No IPFS, no URL, no registry. A node with only Zebra
  can reconstruct every contract ever deployed.
- **Deployed code is immutable.** There is no upgrade path. New code means a
  new deploy and a new id.
- **Deployment data is public.** Code, code hash, deployer's transparent key
  (in the carrier redeem script), and fees are all visible. Shielded funding
  hides which shielded notes paid for PREPARE; it does not hide the code or
  the carriers.

## Advanced: the P2SH carrier

Zcash limits an OP_RETURN to 80 bytes, far too small for a module, so the
bytes travel in transaction **inputs**, which have room for about 1,650
bytes of scriptSig each. The mechanism (ADR-0003, `docs/protocol-v0.md` §7):

1. **PREPARE** pays N outputs to a P2SH address whose redeem script is
   `<deployer_pubkey(33)> OP_CHECKSIG OP_NOP` (36 bytes). The trailing
   `OP_NOP` makes the script non-standard as a template, which is what lets
   Zebra's standardness rules accept extra data pushes in the spending
   scriptSig. The output still requires the deployer's signature to spend.
   Each carrier output carries just enough value to pay its share of the
   DEPLOY fee (`deploy fee: 95000 zat; carriers: 2 x 72500 zat`).
2. **DEPLOY** spends all N carriers. Input *i*'s scriptSig is
   `PUSH(i) PUSH(chunk_i…) PUSH(signature) PUSH(redeem_script)`, where the
   chunk is split into pushes of at most 520 bytes. The transaction also has
   an OP_RETURN with `ZALK || 0x00 || 0x01 || code_hash || code_length ||
   chunk_count || output_index` and a change output.
3. Every node collects the chunks by index, concatenates them, checks the
   length and the SHA-256 against the message, validates the module, and
   registers the contract. Any missing, duplicated, or out-of-range chunk,
   or a hash mismatch, rejects the deploy with no state change.

Because ZIP-244 txids exclude scriptSigs, the code hash in the OP_RETURN is
what binds the txid to the exact bytes: tampering with a chunk breaks the hash
check rather than changing the id.

The chunk size (1400), maximum chunk count, and maximum module size are in
[protocol-limits.md](protocol-limits.md). `crates/zalkanes-tx` builds these
transactions; `crates/zalkanes-carrier` decodes them.
