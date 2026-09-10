# Threat Model: Data Availability

## Attacker goal

Make WASM contract code permanently unavailable so nodes cannot reconstruct
state, or cause nodes to accept a deployment without being able to verify the
code.

## Attacks and mitigations

### WASM code not stored in carrier transaction

**Attack:** Deploy a contract with a code_hash in OP_RETURN but omit the
carrier scriptSig inputs (or include incomplete chunks).

**Mitigation:** The carrier decoder checks that ALL chunks 0..chunk_count-1
are present. Any missing chunk causes the deployment to fail with no state
mutation. The contract is never registered.

### WASM code mutated in carrier

**Attack:** Include carrier inputs with bytes that do not match the committed
code_hash.

**Mitigation:** After reconstruction, `SHA-256(concat_chunks) != code_hash` →
deployment rejected, no state mutation.

### Data availability on external system only

**Attack:** Store WASM code on IPFS, Arweave, or a project API and point to it
from OP_RETURN, expecting nodes to fetch it externally.

**Mitigation:** Zalkanes explicitly requires that WASM bytes be recoverable
entirely from Zcash blockchain data. No external fetch is ever performed.
A deployment without fully on-chain carrier data fails deterministically on
all honest nodes.

### Block data withheld by miner

**Attack:** A miner includes a deployment transaction in a block but withholds
the block from some nodes.

**Mitigation:** This is a standard Zcash data availability attack mitigated
by the Zcash P2P network. Zalkanes inherits Zcash's data availability
guarantees. Zalkanes does not add additional data availability guarantees
beyond what Zcash provides.

### Zebra RPC returns incomplete block data

**Attack:** A malicious or buggy Zebra RPC returns a block with missing or
truncated transaction data.

**Mitigation:** Zalkanes defensively parses all RPC data. A block that does
not parse completely (truncated transactions, missing fields) is treated as an
error. The indexer does not advance its tip. An alert is raised for manual
investigation.

## Conclusion

Data availability for deployed contract code is fully guaranteed by Zcash
on-chain data. No external dependencies are required or trusted.
