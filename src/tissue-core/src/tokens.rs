//! API token lifecycle (#159): issue / list / revoke, hash-only storage.
//!
//! Write auth on `tissue serve` resolves the presented bearer token against
//! stored entries. Managed tokens live in `.tissue/config.yml` under
//! `tokens:` as **SHA-256 hashes only** — the plaintext secret is printed
//! once at issue time and never written to the git-tracked config, so no
//! usable secret lands in history. The `TISSUE_API_TOKEN` env var remains
//! as a bootstrap/override (untracked), and a legacy plaintext `api_token:`
//! line still works but is reported by `token list` as a migration item.
//!
//! Comparison is constant-time (xor-fold over equal-length inputs; length
//! difference short-circuits — no early exit on content).

use std::path::{Path, PathBuf};

use crate::git::{Author, Repo};

/// One stored token entry: id, human label, SHA-256 hex of the secret, created ts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TokenEntry {
    pub id: String,
    pub label: String,
    pub sha256: String,
    pub created: String,
}

/// SHA-256 of the input, lowercase hex. Implemented directly (FIPS 180-4) so
/// the workspace gains no dependency for one hash (#159 scope note).
pub fn sha256_hex(data: &[u8]) -> String {
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
        0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
        0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
        0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
        0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
        0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
        0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
        0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
        0xc67178f2,
    ];
    let mut h: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
        0x5be0cd19,
    ];
    let bit_len = (data.len() as u64).wrapping_mul(8);
    let mut msg = data.to_vec();
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&bit_len.to_be_bytes());
    for block in msg.chunks_exact(64) {
        let mut w = [0u32; 64];
        for i in 0..16 {
            w[i] = u32::from_be_bytes([
                block[4 * i],
                block[4 * i + 1],
                block[4 * i + 2],
                block[4 * i + 3],
            ]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }
        let (mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut hh) =
            (h[0], h[1], h[2], h[3], h[4], h[5], h[6], h[7]);
        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ ((!e) & g);
            let t1 = hh
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(K[i])
                .wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let t2 = s0.wrapping_add(maj);
            hh = g;
            g = f;
            f = e;
            e = d.wrapping_add(t1);
            d = c;
            c = b;
            b = a;
            a = t1.wrapping_add(t2);
        }
        h[0] = h[0].wrapping_add(a);
        h[1] = h[1].wrapping_add(b);
        h[2] = h[2].wrapping_add(c);
        h[3] = h[3].wrapping_add(d);
        h[4] = h[4].wrapping_add(e);
        h[5] = h[5].wrapping_add(f);
        h[6] = h[6].wrapping_add(g);
        h[7] = h[7].wrapping_add(hh);
    }
    let mut out = String::with_capacity(64);
    for word in h {
        out.push_str(&format!("{word:08x}"));
    }
    out
}

/// Constant-time equality for two hex digests (or a digest vs a secret hash).
/// Equal-length check first, then xor-fold — no content-dependent early exit.
pub fn constant_time_eq(a: &str, b: &str) -> bool {
    let a = a.as_bytes();
    let b = b.as_bytes();
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// 32 random bytes, base64url-no-padding (43 chars). Uses OS randomness via
/// /dev/urandom; no new dependency.
pub fn generate_token() -> Result<String, String> {
    use std::io::Read;
    let mut buf = [0u8; 32];
    std::fs::File::open("/dev/urandom")
        .and_then(|mut f| f.read_exact(&mut buf))
        .map_err(|e| format!("reading /dev/urandom: {e}"))?;
    Ok(base64url_nopad(&buf))
}

fn base64url_nopad(data: &[u8]) -> String {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::with_capacity((data.len() * 4).div_ceil(3));
    for chunk in data.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = ((b[0] as u32) << 16) | ((b[1] as u32) << 8) | b[2] as u32;
        out.push(ALPHABET[(n >> 18) as usize & 63] as char);
        out.push(ALPHABET[(n >> 12) as usize & 63] as char);
        if chunk.len() > 1 {
            out.push(ALPHABET[(n >> 6) as usize & 63] as char);
        }
        if chunk.len() > 2 {
            out.push(ALPHABET[n as usize & 63] as char);
        }
    }
    out
}

/// The managed token store over `.tissue/config.yml`.
pub struct TokenStore {
    repo: Repo,
}

#[derive(Debug)]
pub enum TokenError {
    Io(String),
    DuplicateLabel(String),
    UnknownId(String),
}

impl std::fmt::Display for TokenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TokenError::Io(e) => write!(f, "token config error: {e}"),
            TokenError::DuplicateLabel(label) => {
                write!(
                    f,
                    "a token labeled `{label}` already exists; revoke it first"
                )
            }
            TokenError::UnknownId(id) => write!(f, "unknown token id `{id}`"),
        }
    }
}

impl TokenStore {
    pub fn new(repo: Repo) -> Self {
        TokenStore { repo }
    }

    fn config_path(&self) -> PathBuf {
        self.repo
            .resolve(&[".tissue", "config.yml"])
            .unwrap_or_else(|_| self.repo.root().join(".tissue").join("config.yml"))
    }

    /// All stored token entries (hashes only — the secret is never stored).
    pub fn list(&self) -> Result<Vec<TokenEntry>, String> {
        let text = std::fs::read_to_string(self.config_path())
            .map_err(|e| format!("reading config.yml: {e}"))?;
        let doc: serde_yaml::Value =
            serde_yaml::from_str(&text).map_err(|e| format!("parsing config.yml: {e}"))?;
        let mut out = Vec::new();
        if let Some(tokens) = doc.get("tokens").and_then(|t| t.as_sequence()) {
            for t in tokens {
                let get = |k: &str| t.get(k).and_then(|v| v.as_str()).unwrap_or("");
                out.push(TokenEntry {
                    id: get("id").to_string(),
                    label: get("label").to_string(),
                    sha256: get("sha256").to_string(),
                    created: get("created").to_string(),
                });
            }
        }
        Ok(out)
    }

    /// True when a legacy plaintext `api_token:` line exists (migration item).
    pub fn has_legacy_plaintext_token(&self) -> bool {
        std::fs::read_to_string(self.config_path())
            .map(|t| {
                t.lines().any(|l| {
                    l.trim_start().starts_with("api_token:") && {
                        let v = l
                            .trim_start()
                            .strip_prefix("api_token:")
                            .unwrap_or("")
                            .trim();
                        !v.is_empty() && !v.starts_with("sha256:")
                    }
                })
            })
            .unwrap_or(false)
    }

    /// Issue a new token: returns (plaintext secret — printed once, id).
    /// Stores only the SHA-256 hash, one commit.
    pub fn issue(
        &self,
        label: &str,
        actor: &Author,
        now: &str,
    ) -> Result<(String, String), TokenError> {
        if label.trim().is_empty() || label.contains('\n') {
            return Err(TokenError::Io(format!("invalid label `{label}`")));
        }
        let existing = self.list().map_err(TokenError::Io)?;
        if existing.iter().any(|t| t.label == label) {
            return Err(TokenError::DuplicateLabel(label.to_string()));
        }
        let secret = generate_token().map_err(TokenError::Io)?;
        let hash = sha256_hex(secret.as_bytes());
        let id = format!("tok-{:02}", existing.len() + 1);
        let entry = TokenEntry {
            id: id.clone(),
            label: label.to_string(),
            sha256: hash,
            created: now.to_string(),
        };
        self.append_entry(&entry).map_err(TokenError::Io)?;
        self.repo
            .commit_all(actor, &format!("token issue: {label} ({id})"))
            .map_err(|e| TokenError::Io(e.to_string()))?;
        Ok((secret, id))
    }

    /// Revoke a token by id; one commit. Unknown id is an error.
    pub fn revoke(&self, id: &str, actor: &Author) -> Result<TokenEntry, TokenError> {
        let mut entries = self.list().map_err(TokenError::Io)?;
        let Some(pos) = entries.iter().position(|t| t.id == id) else {
            return Err(TokenError::UnknownId(id.to_string()));
        };
        let removed = entries.remove(pos);
        self.rewrite_entries(&entries).map_err(TokenError::Io)?;
        self.repo
            .commit_all(actor, &format!("token revoke: {} ({})", removed.label, id))
            .map_err(|e| TokenError::Io(e.to_string()))?;
        Ok(removed)
    }

    /// Resolve a presented secret to its entry (constant-time hash compare).
    /// `env_token`, when set, is a bootstrap/override and matches directly.
    pub fn resolve(&self, presented: &str, env_token: Option<&str>) -> Option<TokenEntry> {
        if let Some(env) = env_token {
            if !env.is_empty() && constant_time_eq(env, presented) {
                return Some(TokenEntry {
                    id: "env".to_string(),
                    label: "TISSUE_API_TOKEN (env override)".to_string(),
                    sha256: sha256_hex(env.as_bytes()),
                    created: String::new(),
                });
            }
        }
        let hash = sha256_hex(presented.as_bytes());
        self.list()
            .ok()?
            .into_iter()
            .find(|t| constant_time_eq(&t.sha256, &hash))
    }

    fn append_entry(&self, entry: &TokenEntry) -> Result<(), String> {
        let mut entries = self.list()?;
        entries.push(entry.clone());
        self.rewrite_entries(&entries)
    }

    /// Rewrite the `tokens:` block (or append it), preserving the rest of
    /// the file. Hash-only: only id/label/sha256/created are written.
    fn rewrite_entries(&self, entries: &[TokenEntry]) -> Result<(), String> {
        let path = self.config_path();
        let text = std::fs::read_to_string(&path)
            .map_err(|e| format!("reading {}: {e}", path.display()))?;
        let mut out = String::new();
        let mut in_tokens = false;
        let mut wrote = false;
        for line in text.lines() {
            if line.starts_with("tokens:") {
                out.push_str("tokens:\n");
                for t in entries {
                    out.push_str(&format!(
                        "  - id: {}\n    label: {}\n    sha256: {}\n    created: {}\n",
                        yaml_scalar(&t.id),
                        yaml_scalar(&t.label),
                        yaml_scalar(&t.sha256),
                        yaml_scalar(&t.created),
                    ));
                }
                in_tokens = true;
                wrote = true;
                continue;
            }
            if in_tokens {
                let trimmed = line.trim_start();
                if trimmed.starts_with('#') || trimmed.is_empty() {
                    continue;
                }
                if line.starts_with(' ') {
                    continue; // old block entries
                }
                in_tokens = false;
            }
            out.push_str(line);
            out.push('\n');
        }
        if !wrote {
            if !out.ends_with('\n') {
                out.push('\n');
            }
            out.push_str("tokens:\n");
            for t in entries {
                out.push_str(&format!(
                    "  - id: {}\n    label: {}\n    sha256: {}\n    created: {}\n",
                    yaml_scalar(&t.id),
                    yaml_scalar(&t.label),
                    yaml_scalar(&t.sha256),
                    yaml_scalar(&t.created),
                ));
            }
        }
        std::fs::write(&path, out).map_err(|e| format!("writing {}: {e}", path.display()))
    }
}

fn yaml_scalar(value: &str) -> String {
    if value.is_empty()
        || value.contains(": ")
        || value.contains('\n')
        || value.starts_with(['-', '[', '{'])
    {
        format!("{value:?}")
    } else {
        value.to_string()
    }
}

/// Read the legacy/env token resolution inputs for serve: env override plus
/// the plaintext legacy `api_token:` line (kept working, reported as a
/// migration item by `token list`).
pub fn legacy_plaintext_token(repo_root: &Path) -> Option<String> {
    let config = std::fs::read_to_string(repo_root.join(".tissue").join("config.yml")).ok()?;
    let line = config
        .lines()
        .find(|l| l.trim_start().starts_with("api_token:"))?;
    let token = line
        .trim_start()
        .strip_prefix("api_token:")?
        .trim()
        .trim_matches('"')
        .trim();
    if token.is_empty() || token.starts_with("sha256:") {
        None
    } else {
        Some(token.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::Repo;

    #[test]
    fn sha256_known_vectors() {
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(
            sha256_hex(b"The quick brown fox jumps over the lazy dog"),
            "d7a8fbb307d7809469ca9abcb0082e4f8d5651e46d3cdb762d02d0bf37c9e592"
        );
        // >64-byte input exercises multi-block padding.
        let long = vec![b'a'; 200];
        assert_eq!(sha256_hex(&long).len(), 64);
    }

    #[test]
    fn constant_time_eq_basics() {
        assert!(constant_time_eq("abc", "abc"));
        assert!(!constant_time_eq("abc", "abd"));
        assert!(!constant_time_eq("abc", "abcd"));
        assert!(!constant_time_eq("", "a"));
    }

    #[test]
    fn base64url_roundtrip_shape() {
        let t = generate_token().unwrap();
        assert_eq!(t.len(), 43, "32 bytes -> 43 base64url chars");
        assert!(t
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'));
        let t2 = generate_token().unwrap();
        assert_ne!(t, t2, "tokens are random");
    }

    fn temp_store() -> (PathBuf, TokenStore) {
        let dir = std::env::temp_dir().join(format!(
            "tissue-tokens-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(dir.join(".tissue")).unwrap();
        std::fs::write(
            dir.join(".tissue").join("config.yml"),
            "project: demo\nprojects:\n  demo: IAM\n",
        )
        .unwrap();
        let repo = Repo::init(&dir).unwrap();
        repo.commit_all(
            &Author {
                name: "t".into(),
                email: "t@t".into(),
            },
            "init",
        )
        .unwrap();
        (dir, TokenStore::new(repo))
    }

    fn author() -> Author {
        Author {
            name: "human:gui".into(),
            email: "human-gui@tissue.local".into(),
        }
    }

    #[test]
    fn issue_stores_hash_only_and_resolves() {
        let (dir, store) = temp_store();
        let (secret, id) = store
            .issue("human", &author(), "2026-11-01T19:00:00Z")
            .unwrap();
        assert_eq!(id, "tok-01");
        // config.yml contains the hash, never the secret.
        let config = std::fs::read_to_string(dir.join(".tissue").join("config.yml")).unwrap();
        assert!(
            !config.contains(&secret),
            "plaintext secret must not be stored: {config}"
        );
        assert!(config.contains(&sha256_hex(secret.as_bytes())));
        // Resolve works with the secret, fails with anything else.
        let resolved = store.resolve(&secret, None).unwrap();
        assert_eq!(resolved.label, "human");
        assert!(store.resolve("wrong", None).is_none());
        // Env override still wins when set.
        let env_resolved = store.resolve("env-secret", Some("env-secret")).unwrap();
        assert_eq!(env_resolved.id, "env");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn revoke_removes_entry_and_fails_closed() {
        let (dir, store) = temp_store();
        let (secret, id) = store
            .issue("agent:dev-1", &author(), "2026-11-01T19:00:00Z")
            .unwrap();
        assert!(store.resolve(&secret, None).is_some());
        let removed = store.revoke(&id, &author()).unwrap();
        assert_eq!(removed.label, "agent:dev-1");
        assert!(
            store.resolve(&secret, None).is_none(),
            "revoked token fails closed"
        );
        assert!(store.revoke("tok-99", &author()).is_err());
        let config = std::fs::read_to_string(dir.join(".tissue").join("config.yml")).unwrap();
        assert!(!config.contains("tokens:") || !config.contains(&sha256_hex(secret.as_bytes())));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn duplicate_label_rejected() {
        let (dir, store) = temp_store();
        store
            .issue("human", &author(), "2026-11-01T19:00:00Z")
            .unwrap();
        assert!(store
            .issue("human", &author(), "2026-11-01T19:01:00Z")
            .is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn legacy_plaintext_detection() {
        let (dir, _store) = temp_store();
        assert!(!TokenStore::new(Repo::open(&dir).unwrap()).has_legacy_plaintext_token());
        std::fs::write(
            dir.join(".tissue").join("config.yml"),
            "project: demo\napi_token: \"plain-secret\"\n",
        )
        .unwrap();
        assert!(TokenStore::new(Repo::open(&dir).unwrap()).has_legacy_plaintext_token());
        assert_eq!(
            legacy_plaintext_token(&dir).as_deref(),
            Some("plain-secret")
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
