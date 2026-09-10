import { defineRailway, project, service } from "railway/iac";

// Zalkanes — Zcash-native WASM smart-contract metaprotocol.
//
// The `zalkanes` service is the indexer node: it connects to an upstream
// Zcash JSON-RPC endpoint (Zebra / zcashd / NOWNodes) and serves its own
// JSON-RPC API on $PORT.
//
// Required variables (set as Railway secrets, never committed):
//   - ZALKANES_RPC_URL      upstream Zcash RPC, e.g. https://zec.nownodes.io/<key>
//   - ZALKANES_RPC_API_KEY  optional `api-key` header for hosted providers
//   - ZALKANES_NETWORK      mainnet | testnet | regtest (default: regtest)

export default defineRailway(() => {
  const zalkanes = service("zalkanes", {
    start: "zalkanes node serve",
    builder: "dockerfile",
    dockerfilePath: "Dockerfile",
    restart: "on_failure",
    variables: {
      RUST_LOG: "zalkanes=info",
      ZALKANES_NETWORK: "regtest",
    },
  });

  return project("courageous-surprise", {
    resources: [zalkanes],
  });
});
