import { defineRailway, project, service, volume, preserve, ref, github } from "railway/iac";

// Zalkanes — Zcash-native WASM smart-contract metaprotocol.
//
// Architecture (security model: our own full Zebra validator is the trust anchor):
//
//   Zebra (regtest, private)  ── JSON-RPC ──► Zalkanes indexer ──► public JSON-RPC
//   Zebra-testnet (private)   ── JSON-RPC ──► Zalkanes-testnet    ──► public JSON-RPC
//
// Each network has its own Zebra full node (consensus-validating) and its own
// Zalkanes indexer, each with a persistent chain-state volume. Regtest and
// testnet are fully isolated: they never share a volume or trust anchor.

export default defineRailway(() => {
  // Regtest (private) — the accepted Milestone-1 environment.
  const zalkanesData = volume("zalkanes-volume", {
    region: "ams",
    sizeMB: 50_000,
  });
  const zebraData = volume("zebra-volume", {
    region: "ams",
    sizeMB: 50_000,
  });

  const zebra = service("zebra", {
    build: {
      builder: "DOCKERFILE",
      dockerfilePath: "deploy/zebra/Dockerfile",
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

  // Public Zcash testnet — Milestone 2.
  // Separate full Zebra validator (no miner; real testnet PoW), separate volume.
  const zebraTestnetData = volume("zebra-testnet-volume", {
    region: "ams",
    sizeMB: 50_000,
  });

  const zebraTestnet = service("zebra-testnet", {
    source: github("whatnotbot/zalkanes", { branch: "main" }),
    build: {
      builder: "DOCKERFILE",
      dockerfilePath: "deploy/zebra/Dockerfile.testnet",
    },
    variables: {
      RUST_LOG: "info",
    },
    volumeMounts: {
      "/data/zebra": zebraTestnetData,
    },
  });

  return project("zalkanes", {
    resources: [
      zebra,
      zalkanes,
      zalkanesData,
      zebraData,
      zebraTestnet,
      zebraTestnetData,
    ],
  });
});
