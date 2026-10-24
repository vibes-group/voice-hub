//! argon2id password hashing in the PHC-like layout the Go backend wrote:
//! `argon2id$t=...$m=...$p=...$<salt>$<key>` (raw std base64). Existing
//! APP_ADMIN_PASSWORD_HASH values and connection-password files stay valid.

use argon2::{Algorithm, Argon2, Params, Version};
use sha2::digest::Digest;

use crate::util::{b64_raw_std, b64_raw_std_decode, random_bytes};

// Tuned for an interactive login: ~50ms on a modern core, 64 MiB memory.
const ARGON_TIME: u32 = 2;
const ARGON_MEMORY: u32 = 64 * 1024; // KiB
const ARGON_THREADS: u32 = 1;
const ARGON_KEY_LEN: usize = 32;
const ARGON_SALT_LEN: usize = 16;

pub fn hash(plain: &str) -> String {
    let salt = random_bytes::<ARGON_SALT_LEN>();
    let key = derive(plain.as_bytes(), &salt, ARGON_TIME, ARGON_MEMORY, ARGON_THREADS, ARGON_KEY_LEN).expect("valid argon2 parameters");
    format!("argon2id$t={ARGON_TIME}$m={ARGON_MEMORY}$p={ARGON_THREADS}${}${}", b64_raw_std(&salt), b64_raw_std(&key))
}

/// Reports whether `plain` matches an encoded hash. A malformed hash never matches.
pub fn verify(encoded: &str, plain: &str) -> bool {
    let Some(h) = parse(encoded) else {
        return false;
    };
    let Some(got) = derive(plain.as_bytes(), &h.salt, h.t, h.m, h.p, h.key.len()) else {
        return false;
    };
    constant_time_eq(&got, &h.key)
}

/// Lets startup reject a mangled env value instead of locking the admin out.
pub fn valid_hash(encoded: &str) -> bool {
    parse(encoded).is_some()
}

pub fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// Constant-time comparison of two strings of possibly different length
/// (hashes both first, like hmac.Equal over fixed-size digests).
pub fn constant_time_str_eq(a: &str, b: &str) -> bool {
    constant_time_eq(&sha2::Sha256::digest(a.as_bytes()), &sha2::Sha256::digest(b.as_bytes())) && a.len() == b.len()
}

fn derive(plain: &[u8], salt: &[u8], t: u32, m: u32, p: u32, key_len: usize) -> Option<Vec<u8>> {
    let params = Params::new(m, t, p, Some(key_len)).ok()?;
    let mut out = vec![0u8; key_len];
    Argon2::new(Algorithm::Argon2id, Version::V0x13, params).hash_password_into(plain, salt, &mut out).ok()?;
    Some(out)
}

struct Parsed {
    t: u32,
    m: u32,
    p: u32,
    salt: Vec<u8>,
    key: Vec<u8>,
}

fn parse(encoded: &str) -> Option<Parsed> {
    let parts: Vec<&str> = encoded.split('$').collect();
    if parts.len() != 6 || parts[0] != "argon2id" {
        return None;
    }
    let num = |s: &str, prefix: &str| -> Option<u32> {
        let v: u32 = leading_digits(s.strip_prefix(prefix)?)?.parse().ok()?;
        (v != 0).then_some(v)
    };
    let t = num(parts[1], "t=")?;
    let m = num(parts[2], "m=")?;
    let p = num(parts[3], "p=")?;
    if p > u8::MAX as u32 {
        return None;
    }
    let salt = b64_raw_std_decode(parts[4]).filter(|s| !s.is_empty())?;
    let key = b64_raw_std_decode(parts[5]).filter(|k| !k.is_empty())?;
    Some(Parsed { t, m, p, salt, key })
}

/// fmt.Sscanf("%d") stops at the first non-digit; mirror that leniency.
fn leading_digits(s: &str) -> Option<&str> {
    let end = s.bytes().position(|b| !b.is_ascii_digit()).unwrap_or(s.len());
    (end > 0).then(|| &s[..end])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let h = hash("hunter2");
        assert!(!h.contains("hunter2"));
        assert!(valid_hash(&h));
        assert!(verify(&h, "hunter2"));
        assert!(!verify(&h, "hunter3"));
        assert_ne!(h, hash("hunter2"), "salt not random");
    }

    #[test]
    fn malformed_never_matches() {
        for h in [
            "",
            "hunter2",
            "argon2id$t=2$m=65536$p=1$c2FsdA",
            "argon2id$t=0$m=65536$p=1$c2FsdA$a2V5",
            "argon2id$t=2$m=65536$p=0$c2FsdA$a2V5",
            "$argon2id$v=19$m=65536,t=2,p=1$c2FsdA$a2V5",
        ] {
            assert!(!valid_hash(h), "{h}");
            assert!(!verify(h, "") && !verify(h, "hunter2"), "{h}");
        }
    }

    #[test]
    fn verifies_go_generated_hash() {
        // Produced by the Go backend's HashPassword("dev").
        let go = "argon2id$t=2$m=65536$p=1$+/1OcBL+kYPwO5orw0dikg$zrKVhZc6u3cNRw9YiVAT2Cl8WMfIPVs9Hp0I2zZg+uA";
        assert!(verify(go, "dev"));
        assert!(!verify(go, "dev2"));
    }
}
