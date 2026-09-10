import { defineRailway, project, service, volume, preserve } from "railway/iac";

// Zalkanes — Zcash-native WASM smart-contract metaprotocol.
//
// The `zalkanes` service is the indexer node: it connects to an upstream
// Zcash JSON-RPC endpoint (Zebra / zcashd / NOWNodes) and serves its own
// JSON-RPC API on $PORT.
//
// Secrets set on Railway (never committed, preserved by this config):
//   - ZALKANES_RPC_URL      upstream Zcash RPC
//   - ZALKANES_RPC_API_KEY  optional `api-key` header for hosted providers

export default defineRailway(() => {
  const data = volume("zalkanes-volume");

  const zalkanes = service("zalkanes", {
    start: "zalkanes node serve",
    build: {
      builder: "DOCKERFILE",
      dockerfilePath: "Dockerfile",
    },
    deploy: {
      healthcheckPath: "/health",
      restartPolicyType: "ON_FAILURE",
    },
    variables: {
      RUST_LOG: "zalkanes=info",
      ZALKANES_NETWORK: "regtest",
      // Persistent RocksDB lives on the attached volume.
      ZALKANES_DATA_DIR: "/data/zalkanes",
      // Preserve secrets managed outside this file.
      ZALKANES_RPC_URL: preserve(),
      ZALKANES_RPC_API_KEY: preserve(),
    },
    volumeMounts: {
      "/data/zalkanes": data,
    },
  });

  return project("zalkanes", {
    resources: [zalkanes, data],
  });
});
