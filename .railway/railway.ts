import { defineRailway, project, service, volume, preserve, ref } from "railway/iac";

// Zalkanes — Zcash-native WASM smart-contract metaprotocol.
//
// Architecture (security model: our own full Zebra validator is the trust anchor):
//
//   Zebra (regtest, private) ── JSON-RPC ──► Zalkanes indexer ──► public JSON-RPC
//
// `zebra` runs our own Zcash full node (consensus-validating) with a persistent
// chain-state volume. `zalkanes` indexes from it and serves the public RPC.

export default defineRailway(() => {
  const zalkanesData = volume("zalkanes-volume");
  const zebraData = volume("zebra-volume");

  const zebra = service("zebra", {
    build: {
      builder: "DOCKERFILE",
      dockerfilePath: "deploy/zebra/Dockerfile",
    },
    deploy: {
      restartPolicyType: "ON_FAILURE",
    },
    variables: {
      RUST_LOG: "info",
    },
    volumeMounts: {
      "/data/zebra": zebraData,
    },
  });

  const zalkanes = service("zalkanes", {
    start: "zalkanes node serve",
    build: {
      builder: "DOCKERFILE",
      dockerfilePath: "Dockerfile",
    },
    deploy: {
      restartPolicyType: "ON_FAILURE",
    },
    variables: {
      RUST_LOG: "zalkanes=info",
      ZALKANES_NETWORK: "regtest",
      // Persistent RocksDB lives on the attached volume.
      ZALKANES_DATA_DIR: "/data/zalkanes",
      // Our own Zebra node over the private network (trust anchor).
      ZALKANES_RPC_URL: "http://zebra.railway.internal:18232",
      ZALKANES_RPC_API_KEY: preserve(),
    },
    volumeMounts: {
      "/data/zalkanes": zalkanesData,
    },
  });

  return project("zalkanes", {
    resources: [zebra, zalkanes, zalkanesData, zebraData],
  });
});
