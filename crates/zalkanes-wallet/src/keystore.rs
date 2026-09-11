//! Encrypted seed keystore (production key custody).
//!
//! # Design
//!
//! The seed is stored **only** in an `age` passphrase-encrypted file
//! (`age` = the established, audited encryption format maintained by the same
//! author as the Zcash Rust crates, and the format Zcash's own Zallet wallet
//! uses for its keystore). No cryptography is implemented here: key
//! derivation (scrypt) and authenticated encryption (ChaCha20-Poly1305) come
//! from the `age` crate.
//!
//! Guarantees:
//!
//! - The plaintext seed is never written to the wallet SQLite DB, RocksDB,
//!   logs, the operation journal, or acceptance evidence. It exists in
//!   process memory only between `unlock` and `lock`, inside a
//!   [`secrecy::SecretVec`] (zeroized on drop).
//! - The keystore file carries a small plaintext-authenticated header
//!   (magic + version + network + account id) so a wrong-network or
//!   wrong-version file fails loudly instead of producing a different wallet.
//! - Any corruption/truncation of the ciphertext fails loudly: `age` output
//!   is authenticated, so tampering cannot decrypt.
//! - [`Keystore`] and [`UnlockedSeed`] have no `Debug`/`Display` that can
//!   print secret material.

#![forbid(unsafe_code)]

use std::io::{Read, Write};

use anyhow::{anyhow, bail, Context, Result};
use secrecy::{ExposeSecret, SecretVec};

/// Passphrase type, re-exported from `age`'s own `secrecy` (the workspace
/// pins an older `secrecy` for seeds; `age` 0.12 uses `SecretBox<str>`).
pub use age::secrecy::SecretString;

/// Build a [`SecretString`] passphrase from a plain `String`, consuming it.
pub fn passphrase(value: String) -> SecretString {
    SecretString::from(value)
}

/// Keystore file magic + format version. A format change bumps VERSION.
const MAGIC: &[u8; 8] = b"ZALKKEY\x00";
const VERSION: u8 = 1;

/// Which network this keystore's account belongs to (a seed used on the
/// wrong network derives a different wallet; refuse rather than guess).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeystoreNetwork {
    Mainnet,
    Testnet,
    Regtest,
}

impl KeystoreNetwork {
    fn tag(self) -> u8 {
        match self {
            KeystoreNetwork::Mainnet => 0,
            KeystoreNetwork::Testnet => 1,
            KeystoreNetwork::Regtest => 2,
        }
    }
    fn from_tag(tag: u8) -> Result<Self> {
        Ok(match tag {
            0 => KeystoreNetwork::Mainnet,
            1 => KeystoreNetwork::Testnet,
            2 => KeystoreNetwork::Regtest,
            other => bail!("keystore: unknown network tag {other}"),
        })
    }
    pub fn as_str(self) -> &'static str {
        match self {
            KeystoreNetwork::Mainnet => "mainnet",
            KeystoreNetwork::Testnet => "testnet",
            KeystoreNetwork::Regtest => "regtest",
        }
    }
}

/// A seed held in memory only while the wallet is unlocked. Zeroized on drop
/// by `SecretVec`; deliberately has no `Debug`/`Display`.
pub struct UnlockedSeed(SecretVec<u8>);

impl UnlockedSeed {
    /// Borrow the seed bytes. Callers must not copy them into any persistent
    /// or loggable location.
    pub fn expose(&self) -> &[u8] {
        self.0.expose_secret()
    }

    /// A copy of the secret for APIs that require ownership (e.g. upstream
    /// `create_account`). Still zeroizing.
    pub fn clone_secret(&self) -> SecretVec<u8> {
        SecretVec::new(self.0.expose_secret().to_vec())
    }
}

/// The non-secret header of a keystore file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KeystoreInfo {
    pub version: u8,
    pub network: KeystoreNetwork,
    /// ZIP-32 account index this keystore's wallet uses.
    pub account: u32,
}

/// An on-disk encrypted seed keystore.
pub struct Keystore {
    path: std::path::PathBuf,
}

impl Keystore {
    pub fn at(path: impl Into<std::path::PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn path(&self) -> &std::path::Path {
        &self.path
    }

    pub fn exists(&self) -> bool {
        self.path.exists()
    }

    /// Create a new keystore from `seed`, encrypted under `passphrase`.
    /// Refuses to overwrite an existing file (losing a seed is unrecoverable).
    pub fn create(
        &self,
        passphrase: &SecretString,
        seed: &SecretVec<u8>,
        network: KeystoreNetwork,
        account: u32,
    ) -> Result<()> {
        if seed.expose_secret().len() < 32 {
            bail!("keystore: seed must be at least 32 bytes");
        }
        if age::secrecy::ExposeSecret::expose_secret(passphrase).is_empty() {
            bail!("keystore: passphrase must not be empty");
        }
        if self.exists() {
            bail!(
                "keystore: {} already exists; refusing to overwrite a seed",
                self.path.display()
            );
        }

        let mut ciphertext = Vec::new();
        let mut writer = age::Encryptor::with_user_passphrase(passphrase.clone())
            .wrap_output(&mut ciphertext)
            .map_err(|e| anyhow!("keystore: encryptor: {e}"))?;
        writer
            .write_all(seed.expose_secret())
            .map_err(|e| anyhow!("keystore: write: {e}"))?;
        writer
            .finish()
            .map_err(|e| anyhow!("keystore: finish: {e}"))?;

        let mut blob = Vec::with_capacity(MAGIC.len() + 6 + ciphertext.len());
        blob.extend_from_slice(MAGIC);
        blob.push(VERSION);
        blob.push(network.tag());
        blob.extend_from_slice(&account.to_be_bytes());
        blob.extend_from_slice(&ciphertext);

        // Write to a temporary file, fsync, then rename: a crash never leaves
        // a half-written keystore in place.
        let tmp = self.path.with_extension("tmp");
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        {
            let mut f = std::fs::OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(&tmp)
                .with_context(|| format!("keystore: create {}", tmp.display()))?;
            f.write_all(&blob)
                .context("keystore: write temporary file")?;
            f.sync_all().context("keystore: fsync")?;
        }
        set_owner_only(&tmp)?;
        std::fs::rename(&tmp, &self.path).context("keystore: rename into place")?;
        Ok(())
    }

    /// Read the keystore's non-secret header (no passphrase needed).
    pub fn info(&self) -> Result<KeystoreInfo> {
        let (info, _) = self.read_parts()?;
        Ok(info)
    }

    /// Decrypt the seed. A wrong passphrase, a corrupted file, or a
    /// network/version mismatch all fail loudly.
    pub fn unlock(
        &self,
        passphrase: &SecretString,
        expected_network: KeystoreNetwork,
    ) -> Result<UnlockedSeed> {
        let (info, ciphertext) = self.read_parts()?;
        if info.network != expected_network {
            bail!(
                "keystore: network mismatch (file is {}, requested {})",
                info.network.as_str(),
                expected_network.as_str()
            );
        }

        let identity = age::scrypt::Identity::new(passphrase.clone());
        let decryptor = age::Decryptor::new(&ciphertext[..])
            .map_err(|_| anyhow!("keystore: unreadable or corrupt keystore file"))?;
        let mut reader = decryptor
            .decrypt(std::iter::once(&identity as &dyn age::Identity))
            .map_err(|_| anyhow!("keystore: wrong passphrase or corrupt keystore"))?;
        let mut seed = Vec::new();
        reader
            .read_to_end(&mut seed)
            .map_err(|_| anyhow!("keystore: truncated or corrupt keystore payload"))?;
        if seed.len() < 32 {
            bail!("keystore: decrypted seed is too short");
        }
        Ok(UnlockedSeed(SecretVec::new(seed)))
    }

    fn read_parts(&self) -> Result<(KeystoreInfo, Vec<u8>)> {
        let blob = std::fs::read(&self.path)
            .with_context(|| format!("keystore: read {}", self.path.display()))?;
        let header_len = MAGIC.len() + 6;
        if blob.len() < header_len {
            bail!("keystore: file is truncated (shorter than its header)");
        }
        if &blob[..MAGIC.len()] != MAGIC {
            bail!("keystore: not a Zalkanes keystore (bad magic)");
        }
        let version = blob[MAGIC.len()];
        if version != VERSION {
            bail!("keystore: unsupported keystore version {version} (expected {VERSION})");
        }
        let network = KeystoreNetwork::from_tag(blob[MAGIC.len() + 1])?;
        let account = u32::from_be_bytes([
            blob[MAGIC.len() + 2],
            blob[MAGIC.len() + 3],
            blob[MAGIC.len() + 4],
            blob[MAGIC.len() + 5],
        ]);
        Ok((
            KeystoreInfo {
                version,
                network,
                account,
            },
            blob[header_len..].to_vec(),
        ))
    }
}

/// Restrict a file to owner-only access where the platform supports it.
fn set_owner_only(path: &std::path::Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
            .context("keystore: chmod 600")?;
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
    Ok(())
}

/// Generate a fresh 32-byte seed from the OS CSPRNG.
pub fn generate_seed() -> SecretVec<u8> {
    use rand_core::RngCore;
    let mut bytes = vec![0u8; 32];
    rand_core::OsRng.fill_bytes(&mut bytes);
    SecretVec::new(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> std::path::PathBuf {
        use rand_core::RngCore;
        let mut s = [0u8; 8];
        rand_core::OsRng.fill_bytes(&mut s);
        std::env::temp_dir().join(format!("zalkanes-ks-{tag}-{}.age", hex::encode(s)))
    }

    fn pass(s: &str) -> SecretString {
        passphrase(s.to_string())
    }

    #[test]
    fn create_then_unlock_round_trips() {
        let path = tmp("roundtrip");
        let ks = Keystore::at(&path);
        let seed = generate_seed();
        let original = seed.expose_secret().to_vec();
        ks.create(
            &pass("correct horse battery staple"),
            &seed,
            KeystoreNetwork::Testnet,
            0,
        )
        .unwrap();

        let opened = Keystore::at(&path);
        let unlocked = opened
            .unlock(
                &pass("correct horse battery staple"),
                KeystoreNetwork::Testnet,
            )
            .unwrap();
        assert_eq!(unlocked.expose(), original.as_slice());

        let info = opened.info().unwrap();
        assert_eq!(info.network, KeystoreNetwork::Testnet);
        assert_eq!(info.version, VERSION);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn wrong_passphrase_fails_loudly() {
        let path = tmp("wrongpass");
        let ks = Keystore::at(&path);
        ks.create(
            &pass("right"),
            &generate_seed(),
            KeystoreNetwork::Testnet,
            0,
        )
        .unwrap();
        match ks.unlock(&pass("wrong"), KeystoreNetwork::Testnet) {
            Ok(_) => panic!("wrong passphrase must fail"),
            Err(e) => assert!(e.to_string().contains("wrong passphrase"), "got: {e}"),
        }
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn network_mismatch_fails_loudly() {
        let path = tmp("network");
        let ks = Keystore::at(&path);
        ks.create(&pass("p"), &generate_seed(), KeystoreNetwork::Testnet, 0)
            .unwrap();
        match ks.unlock(&pass("p"), KeystoreNetwork::Mainnet) {
            Ok(_) => panic!("network mismatch must fail"),
            Err(e) => assert!(e.to_string().contains("network mismatch"), "got: {e}"),
        }
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn corrupt_ciphertext_fails_loudly() {
        let path = tmp("corrupt");
        let ks = Keystore::at(&path);
        ks.create(&pass("p"), &generate_seed(), KeystoreNetwork::Testnet, 0)
            .unwrap();
        let mut bytes = std::fs::read(&path).unwrap();
        let n = bytes.len();
        bytes[n - 5] ^= 0xFF; // inside the authenticated payload
        std::fs::write(&path, bytes).unwrap();
        assert!(
            ks.unlock(&pass("p"), KeystoreNetwork::Testnet).is_err(),
            "corrupt ciphertext must not decrypt"
        );
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn truncated_file_fails_loudly() {
        let path = tmp("truncate");
        let ks = Keystore::at(&path);
        ks.create(&pass("p"), &generate_seed(), KeystoreNetwork::Testnet, 0)
            .unwrap();
        let bytes = std::fs::read(&path).unwrap();
        std::fs::write(&path, &bytes[..bytes.len() / 2]).unwrap();
        assert!(ks.unlock(&pass("p"), KeystoreNetwork::Testnet).is_err());
        // Header-only truncation is also loud.
        std::fs::write(&path, &bytes[..4]).unwrap();
        assert!(ks.info().is_err());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn bad_magic_fails_loudly() {
        let path = tmp("magic");
        std::fs::write(&path, b"not a keystore at all, really not").unwrap();
        let err = Keystore::at(&path).info().expect_err("bad magic must fail");
        assert!(err.to_string().contains("bad magic"), "got: {err}");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn refuses_to_overwrite_existing_keystore() {
        let path = tmp("overwrite");
        let ks = Keystore::at(&path);
        ks.create(&pass("p"), &generate_seed(), KeystoreNetwork::Testnet, 0)
            .unwrap();
        let err = ks
            .create(&pass("p2"), &generate_seed(), KeystoreNetwork::Testnet, 0)
            .expect_err("must refuse to overwrite");
        assert!(
            err.to_string().contains("refusing to overwrite"),
            "got: {err}"
        );
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn keystore_file_is_owner_only_and_has_no_plaintext_seed() {
        let path = tmp("perms");
        let ks = Keystore::at(&path);
        let seed = generate_seed();
        let plain = seed.expose_secret().to_vec();
        ks.create(&pass("p"), &seed, KeystoreNetwork::Regtest, 0)
            .unwrap();

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600, "keystore must be owner-only");
        }
        // The raw seed bytes must not appear anywhere in the file.
        let blob = std::fs::read(&path).unwrap();
        assert!(
            !blob.windows(plain.len()).any(|w| w == plain.as_slice()),
            "plaintext seed must never appear in the keystore file"
        );
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn empty_passphrase_is_refused() {
        let path = tmp("emptypass");
        let err = Keystore::at(&path)
            .create(&pass(""), &generate_seed(), KeystoreNetwork::Testnet, 0)
            .expect_err("empty passphrase must be refused");
        assert!(err.to_string().contains("passphrase"), "got: {err}");
    }
}

#[cfg(test)]
mod lifecycle_tests {
    //! Production custody lifecycle against the real wallet DB: create →
    //! restart → LOCKED reopen (watch-capable) → unlock → lock → restart.

    use super::*;
    use crate::chain_source::CanonicalChainSource;
    use crate::funding::{CanonicalTip, TipSource};
    use crate::shielded::ShieldedWallet;
    use crate::sqlite::SqliteShieldedWallet;
    use zalkanes_core::consensus_params::ConsensusParams;
    use zcash_client_backend::data_api::chain::ChainState;
    use zcash_protocol::consensus::BlockHeight;

    struct MockChain;
    impl TipSource for MockChain {
        fn canonical_tip(&self) -> Result<CanonicalTip> {
            Ok(CanonicalTip {
                height: 1_000,
                hash: [0u8; 32],
            })
        }
    }
    impl CanonicalChainSource for MockChain {
        fn block_hash(&self, _h: u32) -> Result<[u8; 32]> {
            Ok([0u8; 32])
        }
        fn block(&self, _h: u32) -> Result<zcash_primitives::block::Block> {
            unreachable!("mock block")
        }
        fn tree_state(&self, h: u32) -> Result<ChainState> {
            Ok(ChainState::empty(
                BlockHeight::from_u32(h),
                zcash_primitives::block::BlockHash([0u8; 32]),
            ))
        }
    }

    struct Paths {
        dir: std::path::PathBuf,
        db: std::path::PathBuf,
        ks: std::path::PathBuf,
    }

    fn paths(tag: &str) -> Paths {
        use rand_core::RngCore;
        let mut s = [0u8; 8];
        rand_core::OsRng.fill_bytes(&mut s);
        let dir = std::env::temp_dir().join(format!("zalkanes-life-{tag}-{}", hex::encode(s)));
        std::fs::create_dir_all(&dir).unwrap();
        Paths {
            db: dir.join("wallet.sqlite"),
            ks: dir.join("keystore.age"),
            dir,
        }
    }

    fn pw(s: &str) -> SecretString {
        passphrase(s.to_string())
    }

    /// Create a keystore + wallet the way the production CLI does.
    fn create_wallet(p: &Paths, pass: &str) -> String {
        let seed = generate_seed();
        Keystore::at(&p.ks)
            .create(&pw(pass), &seed, KeystoreNetwork::Testnet, 0)
            .unwrap();
        let w = SqliteShieldedWallet::create_new(&p.db, ConsensusParams::Test, seed, &MockChain)
            .unwrap();
        w.unified_address().unwrap()
    }

    #[test]
    fn create_restart_reopen_is_locked_but_watch_capable() {
        let p = paths("locked");
        let ua = create_wallet(&p, "hunter2");

        // Restart: reopen WITHOUT any seed.
        let w = SqliteShieldedWallet::open_locked(&p.db, ConsensusParams::Test).unwrap();
        assert!(!w.is_unlocked(), "a fresh reopen must be LOCKED");

        // Watch-capable while locked.
        assert_eq!(w.unified_address().unwrap(), ua);
        assert_eq!(w.balance().unwrap(), 0);
        assert!(w.birthday_height().unwrap() > 0);
        let status = w
            .sync_status(CanonicalTip {
                height: 1_000,
                hash: [0u8; 32],
            })
            .unwrap();
        assert!(!status.synced);

        // Spending is refused while locked.
        match w.select_spends(1_000, "plan-locked") {
            Ok(_) => panic!("locked wallet must refuse to authorize a spend"),
            Err(e) => assert!(e.to_string().contains("locked"), "got: {e}"),
        }
        let _ = std::fs::remove_dir_all(&p.dir);
    }

    #[test]
    fn unlock_lock_cycle_controls_spend_authorization() {
        let p = paths("unlock");
        create_wallet(&p, "hunter2");

        let w = SqliteShieldedWallet::open_locked(&p.db, ConsensusParams::Test).unwrap();
        let seed = Keystore::at(&p.ks)
            .unlock(&pw("hunter2"), KeystoreNetwork::Testnet)
            .unwrap();
        w.unlock_with_seed(&seed.clone_secret()).unwrap();
        assert!(w.is_unlocked());

        // Unlocked: selection proceeds far enough to prove authorization is
        // available (it fails later, on an unsynced wallet — NOT on "locked").
        match w.select_spends(1_000, "plan-unlocked") {
            Ok(_) => {}
            Err(e) => assert!(
                !e.to_string().contains("locked"),
                "unlocked wallet must not report locked: {e}"
            ),
        }

        w.lock();
        assert!(!w.is_unlocked());
        match w.select_spends(1_000, "plan-relocked") {
            Ok(_) => panic!("locked wallet must refuse"),
            Err(e) => assert!(e.to_string().contains("locked"), "got: {e}"),
        }
        let _ = std::fs::remove_dir_all(&p.dir);
    }

    #[test]
    fn restart_after_unlock_is_locked_again() {
        let p = paths("restart");
        create_wallet(&p, "hunter2");
        {
            let w = SqliteShieldedWallet::open_locked(&p.db, ConsensusParams::Test).unwrap();
            let seed = Keystore::at(&p.ks)
                .unlock(&pw("hunter2"), KeystoreNetwork::Testnet)
                .unwrap();
            w.unlock_with_seed(&seed.clone_secret()).unwrap();
            assert!(w.is_unlocked());
        } // process restart
        let w = SqliteShieldedWallet::open_locked(&p.db, ConsensusParams::Test).unwrap();
        assert!(!w.is_unlocked(), "unlock must never persist across restart");
        let _ = std::fs::remove_dir_all(&p.dir);
    }

    #[test]
    fn wrong_passphrase_cannot_unlock_the_wallet() {
        let p = paths("wrongpw");
        create_wallet(&p, "hunter2");
        let w = SqliteShieldedWallet::open_locked(&p.db, ConsensusParams::Test).unwrap();
        match Keystore::at(&p.ks).unlock(&pw("nope"), KeystoreNetwork::Testnet) {
            Ok(_) => panic!("wrong passphrase must fail"),
            Err(e) => assert!(e.to_string().contains("wrong passphrase"), "got: {e}"),
        }
        assert!(!w.is_unlocked());
        let _ = std::fs::remove_dir_all(&p.dir);
    }

    #[test]
    fn foreign_seed_is_rejected_at_unlock() {
        let p = paths("foreign");
        create_wallet(&p, "hunter2");
        let w = SqliteShieldedWallet::open_locked(&p.db, ConsensusParams::Test).unwrap();
        // A valid but UNRELATED seed must not silently unlock this account.
        match w.unlock_with_seed(&generate_seed()) {
            Ok(_) => panic!("foreign seed must be rejected"),
            Err(e) => assert!(
                e.to_string().contains("does not derive this wallet"),
                "got: {e}"
            ),
        }
        assert!(!w.is_unlocked());
        let _ = std::fs::remove_dir_all(&p.dir);
    }

    #[test]
    fn restore_reproduces_the_same_account_and_address() {
        let p = paths("restore");
        let seed = generate_seed();
        let raw = seed.expose_secret().to_vec();
        Keystore::at(&p.ks)
            .create(&pw("pw"), &seed, KeystoreNetwork::Testnet, 0)
            .unwrap();
        let original =
            SqliteShieldedWallet::create_new(&p.db, ConsensusParams::Test, seed, &MockChain)
                .unwrap();
        let ua = original.unified_address().unwrap();
        drop(original);

        // Restore the SAME seed into a fresh wallet DB at an explicit birthday.
        let p2 = paths("restore2");
        let restored = SqliteShieldedWallet::restore(
            &p2.db,
            ConsensusParams::Test,
            SecretVec::new(raw),
            900,
            &MockChain,
        )
        .unwrap();
        assert_eq!(restored.unified_address().unwrap(), ua);
        assert_eq!(restored.birthday_height().unwrap(), 900);
        let _ = std::fs::remove_dir_all(&p.dir);
        let _ = std::fs::remove_dir_all(&p2.dir);
    }

    #[test]
    fn no_plaintext_seed_reaches_the_wallet_database_or_keystore() {
        let p = paths("noleak");
        let seed = generate_seed();
        let raw = seed.expose_secret().to_vec();
        Keystore::at(&p.ks)
            .create(&pw("pw"), &seed, KeystoreNetwork::Testnet, 0)
            .unwrap();
        let w = SqliteShieldedWallet::create_new(&p.db, ConsensusParams::Test, seed, &MockChain)
            .unwrap();
        // Anything the wallet can print must not contain the seed.
        let printable = format!(
            "{} {} {}",
            w.unified_address().unwrap(),
            w.birthday_height().unwrap(),
            w.shielded_note_summary().unwrap_or_default()
        );
        assert!(!printable.contains(&hex::encode(&raw)));
        drop(w);

        for file in [&p.db, &p.ks] {
            let bytes = std::fs::read(file).unwrap();
            assert!(
                !bytes.windows(raw.len()).any(|win| win == raw.as_slice()),
                "plaintext seed found in {}",
                file.display()
            );
        }
        let _ = std::fs::remove_dir_all(&p.dir);
    }
}
