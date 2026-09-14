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
    source: github("whatnotbot/zalkanes", { branch: "main" }),
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

  // Zalkanes testnet indexer (separate from regtest). Indexes from our own
  // Zebra testnet validator; its RocksDB lives on its own volume.
  const zalkanesTestnetData = volume("zalkanes-testnet-volume", {
    region: "ams",
    sizeMB: 50_000,
  });

  const zalkanesTestnet = service("zalkanes-testnet", {
    start: "zalkanes node serve",
    source: github("whatnotbot/zalkanes", { branch: "main" }),
    build: {
      builder: "DOCKERFILE",
      dockerfilePath: "Dockerfile",
    },
    variables: {
      RUST_LOG: "zalkanes=info",
      ZALKANES_NETWORK: "testnet",
      ZALKANES_DATA_DIR: "/data/zalkanes",
      ZALKANES_RPC_URL: "http://zebra-testnet.railway.internal:18232",
    },
    volumeMounts: {
      "/data/zalkanes": zalkanesTestnetData,
    },
  });

  // RC2 public-testnet indexers (docs/release/testnet-rc2-deployment.md).
  //
  // Two INDEPENDENTLY initialised Zalkanes nodes on NEW, empty volumes, both
  // following the same validating `zebra-testnet`. They exist so that
  // agreement on the indexed block hash and state root is evidence, not an
  // artefact of one database. The pre-freeze `zalkanes-testnet` service above
  // holds a database built under the superseded activation; it is retired at
  // deployment (its volume is preserved as historical evidence, never wiped
  // and never reused).
  const rc2Indexer = (name: string, data: ReturnType<typeof volume>) =>
    service(name, {
      start: "zalkanes node serve",
      source: github("whatnotbot/zalkanes", { branch: "main" }),
      build: {
        builder: "DOCKERFILE",
        dockerfilePath: "Dockerfile",
      },
      variables: {
        RUST_LOG: "zalkanes=info",
        ZALKANES_NETWORK: "testnet",
        ZALKANES_DATA_DIR: "/data/zalkanes",
        ZALKANES_RPC_URL: "http://zebra-testnet.railway.internal:18232",
      },
      volumeMounts: {
        "/data/zalkanes": data,
      },
    });
  const zalkanesRc2AData = volume("zalkanes-testnet-rc2-a-volume", {
    region: "ams",
    sizeMB: 50_000,
  });
  const zalkanesRc2BData = volume("zalkanes-testnet-rc2-b-volume", {
    region: "ams",
    sizeMB: 50_000,
  });
  const zalkanesRc2A = rc2Indexer("zalkanes-testnet-rc2-a", zalkanesRc2AData);
  const zalkanesRc2B = rc2Indexer("zalkanes-testnet-rc2-b", zalkanesRc2BData);

  return project("zalkanes", {
    resources: [
      zebra,
      zalkanes,
      zalkanesData,
      zebraData,
      zebraTestnet,
      zebraTestnetData,
      zalkanesTestnet,
      zalkanesTestnetData,
      zalkanesRc2A,
      zalkanesRc2AData,
      zalkanesRc2B,
      zalkanesRc2BData,
    ],
  });
});
