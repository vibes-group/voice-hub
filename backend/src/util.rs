//! Small helpers shared across modules: logging, randomness, encodings, time.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use base64ct::Encoding;
use time::OffsetDateTime;

/// `log!` prints one line to stderr in Go's default log layout
/// (`2006/01/02 15:04:05 message`), so logs read the same as before the port.
#[macro_export]
macro_rules! log {
    ($($arg:tt)*) => {
        $crate::util::log_line(format_args!($($arg)*))
    };
}

pub fn log_line(args: std::fmt::Arguments<'_>) {
    let now = OffsetDateTime::now_utc();
    eprintln!(
        "{:04}/{:02}/{:02} {:02}:{:02}:{:02} {}",
        now.year(),
        u8::from(now.month()),
        now.day(),
        now.hour(),
        now.minute(),
        now.second(),
        args
    );
}

pub fn random_bytes<const N: usize>() -> [u8; N] {
    let mut buf = [0u8; N];
    getrandom::fill(&mut buf).expect("OS random source");
    buf
}

pub fn random_u32() -> u32 {
    u32::from_le_bytes(random_bytes::<4>())
}

pub fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push(DIGITS[(b >> 4) as usize] as char);
        out.push(DIGITS[(b & 0xf) as usize] as char);
    }
    out
}

pub fn b64_std(bytes: &[u8]) -> String {
    base64ct::Base64::encode_string(bytes)
}

pub fn b64_raw_std(bytes: &[u8]) -> String {
    base64ct::Base64Unpadded::encode_string(bytes)
}

pub fn b64_raw_std_decode(s: &str) -> Option<Vec<u8>> {
    base64ct::Base64Unpadded::decode_vec(s).ok()
}

pub fn b64_raw_url(bytes: &[u8]) -> String {
    base64ct::Base64UrlUnpadded::encode_string(bytes)
}

/// ULID (https://github.com/ulid/spec): 48-bit millisecond timestamp plus 80
/// random bits, Crockford base32. Lexicographically sortable by creation time.
pub fn ulid(at: SystemTime) -> String {
    const ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
    let ms = at.duration_since(UNIX_EPOCH).unwrap_or_default().as_millis();
    let rand = random_bytes::<10>();
    let mut v: u128 = ms & ((1 << 48) - 1);
    for b in rand {
        v = (v << 8) | b as u128;
    }
    let mut out = [0u8; 26];
    for i in (0..26).rev() {
        out[i] = ALPHABET[(v & 0x1f) as usize];
        v >>= 5;
    }
    String::from_utf8(out.to_vec()).expect("ascii")
}

pub fn unix_millis(t: SystemTime) -> i64 {
    t.duration_since(UNIX_EPOCH).unwrap_or_default().as_millis() as i64
}

pub fn unix_secs(t: SystemTime) -> i64 {
    t.duration_since(UNIX_EPOCH).unwrap_or_default().as_secs() as i64
}

/// Formats like Go's `time.Time` JSON encoding for UTC values (RFC 3339 with
/// nanoseconds, trailing zeros trimmed), so persisted files and API responses
/// stay byte-compatible.
pub fn rfc3339_nano(t: SystemTime) -> String {
    let dt = OffsetDateTime::from(t);
    let mut out =
        format!("{:04}-{:02}-{:02}T{:02}:{:02}:{:02}", dt.year(), u8::from(dt.month()), dt.day(), dt.hour(), dt.minute(), dt.second());
    let nanos = dt.nanosecond();
    if nanos != 0 {
        let frac = format!("{nanos:09}");
        out.push('.');
        out.push_str(frac.trim_end_matches('0'));
    }
    out.push('Z');
    out
}

pub fn parse_rfc3339(s: &str) -> Option<SystemTime> {
    let dt = OffsetDateTime::parse(s, &time::format_description::well_known::Rfc3339).ok()?;
    Some(SystemTime::from(dt))
}

/// Serde adapter for `SystemTime` fields in Go's JSON time layout.
pub mod go_time {
    use std::time::SystemTime;

    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(t: &SystemTime, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&super::rfc3339_nano(*t))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<SystemTime, D::Error> {
        let s = String::deserialize(d)?;
        super::parse_rfc3339(&s).ok_or_else(|| serde::de::Error::custom("bad RFC 3339 time"))
    }

    /// `omitzero` counterpart: `None` stands for Go's zero time.
    pub mod opt {
        use std::time::SystemTime;

        use serde::{Deserialize, Deserializer, Serializer};

        pub fn serialize<S: Serializer>(t: &Option<SystemTime>, s: S) -> Result<S::Ok, S::Error> {
            match t {
                Some(t) => super::serialize(t, s),
                None => s.serialize_none(),
            }
        }

        pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Option<SystemTime>, D::Error> {
            let s = Option::<String>::deserialize(d)?;
            match s {
                None => Ok(None),
                // Go's zero time, as written by an omitempty-less encoder.
                Some(s) if s.starts_with("0001-01-01T00:00:00") => Ok(None),
                Some(s) => super::super::parse_rfc3339(&s).map(Some).ok_or_else(|| serde::de::Error::custom("bad RFC 3339 time")),
            }
        }
    }
}

pub fn ceil_secs(d: Duration) -> u64 {
    d.as_secs() + u64::from(d.subsec_nanos() > 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rfc3339_matches_go_layout() {
        let t = UNIX_EPOCH + Duration::new(1_700_000_000, 120_000_000);
        assert_eq!(rfc3339_nano(t), "2023-11-14T22:13:20.12Z");
        let t = UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        assert_eq!(rfc3339_nano(t), "2023-11-14T22:13:20Z");
        assert_eq!(parse_rfc3339("2023-11-14T22:13:20.12Z"), Some(UNIX_EPOCH + Duration::new(1_700_000_000, 120_000_000)));
    }

    #[test]
    fn ulid_shape_and_order() {
        let a = ulid(UNIX_EPOCH + Duration::from_millis(1));
        let b = ulid(UNIX_EPOCH + Duration::from_millis(2));
        assert_eq!(a.len(), 26);
        assert!(a < b);
    }
}
