//! Credentials: API tokens (`h2s_<id>_<secret>`, ADR 0008) and browser session ids.
//!
//! A token is `h2s_` + an 8-character public id (`[a-z0-9]`) + `_` + a 256-bit random
//! secret in unpadded base64url (43 characters). The server stores only SHA-256 of the
//! secret: a random 256-bit secret needs no slow KDF, and the public id gives O(1) lookup
//! without comparing against every stored hash. Comparison is constant-time.
//!
//! Session ids are 256-bit random values (unpadded base64url) carried in the HttpOnly
//! `h2s_session` cookie; only their SHA-256 is stored.

use subtle::ConstantTimeEq;
use zeroize::Zeroizing;

use crate::util::{b64url, random_bytes, sha256};

/// Length of the public token id.
pub const TOKEN_ID_LEN: usize = 8;
/// Length of the encoded secret (32 bytes, unpadded base64url).
pub const SECRET_LEN: usize = 43;
/// Length of an encoded session id.
pub const SESSION_ID_LEN: usize = 43;

const ID_ALPHABET: &[u8; 36] = b"abcdefghijklmnopqrstuvwxyz0123456789";

/// A newly minted token. `token` is shown to the user exactly once.
pub struct MintedToken {
    pub token: Zeroizing<String>,
    pub id: String,
    pub secret_hash: [u8; 32],
}

impl std::fmt::Debug for MintedToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MintedToken")
            .field("id", &self.id)
            .finish_non_exhaustive()
    }
}

/// A random public token id.
///
/// # Errors
/// When the OS provides no randomness.
pub fn new_token_id() -> std::io::Result<String> {
    let mut out = String::with_capacity(TOKEN_ID_LEN);
    while out.len() < TOKEN_ID_LEN {
        for b in random_bytes::<16>()? {
            // Rejection sampling keeps the distribution uniform (252 = 7 * 36).
            if b < 252 && out.len() < TOKEN_ID_LEN {
                out.push(char::from(ID_ALPHABET[usize::from(b % 36)]));
            }
        }
    }
    Ok(out)
}

/// Mint a token with the given public id.
///
/// # Errors
/// When the OS provides no randomness.
pub fn mint(id: &str) -> std::io::Result<MintedToken> {
    let secret = Zeroizing::new(b64url(&random_bytes::<32>()?));
    let token = Zeroizing::new(format!("{}{id}_{}", h2s_api::TOKEN_PREFIX, secret.as_str()));
    Ok(MintedToken {
        secret_hash: hash_secret(&secret),
        token,
        id: id.to_owned(),
    })
}

/// The parts of a presented token.
#[derive(PartialEq, Eq)]
pub struct ParsedToken<'a> {
    pub id: &'a str,
    pub secret: &'a str,
}

impl std::fmt::Debug for ParsedToken<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ParsedToken")
            .field("id", &self.id)
            .finish_non_exhaustive()
    }
}

fn is_b64url(s: &str) -> bool {
    s.bytes()
        .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// Split `h2s_<id>_<secret>`; `None` unless every part has the exact shape.
///
/// Any non-ASCII input is refused before the string is split at byte offsets, so a
/// multi-byte character straddling a boundary cannot panic (`str::split_at`).
#[must_use]
pub fn parse(token: &str) -> Option<ParsedToken<'_>> {
    if !token.is_ascii() {
        return None;
    }
    let rest = token.strip_prefix(h2s_api::TOKEN_PREFIX)?;
    if rest.len() != TOKEN_ID_LEN + 1 + SECRET_LEN {
        return None;
    }
    let (id, tail) = rest.split_at(TOKEN_ID_LEN);
    let secret = tail.strip_prefix('_')?;
    let id_ok = id
        .bytes()
        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit());
    (id_ok && secret.len() == SECRET_LEN && is_b64url(secret)).then_some(ParsedToken { id, secret })
}

/// The public id of a token-shaped string, for audit records of failed attempts.
#[must_use]
pub fn token_id_hint(token: &str) -> Option<&str> {
    parse(token).map(|p| p.id)
}

/// SHA-256 of a token secret.
#[must_use]
pub fn hash_secret(secret: &str) -> [u8; 32] {
    sha256(secret.as_bytes())
}

/// Constant-time comparison of a presented secret against a stored hash.
#[must_use]
pub fn secret_matches(stored_hash: &[u8; 32], presented_secret: &str) -> bool {
    let h = hash_secret(presented_secret);
    bool::from(stored_hash.ct_eq(&h))
}

/// Constant-time equality of two byte strings (length is not secret).
#[must_use]
pub fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && bool::from(a.ct_eq(b))
}

/// A new session id and the hash to store.
///
/// # Errors
/// When the OS provides no randomness.
pub fn new_session() -> std::io::Result<(Zeroizing<String>, [u8; 32])> {
    let id = Zeroizing::new(b64url(&random_bytes::<32>()?));
    let hash = sha256(id.as_bytes());
    Ok((id, hash))
}

/// Hash of a presented session cookie value, when it has the right shape.
#[must_use]
pub fn session_hash(value: &str) -> Option<[u8; 32]> {
    (value.len() == SESSION_ID_LEN && is_b64url(value)).then(|| sha256(value.as_bytes()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn minted_tokens_parse_and_verify() {
        let id = new_token_id().unwrap();
        assert_eq!(id.len(), TOKEN_ID_LEN);
        let m = mint(&id).unwrap();
        let p = parse(&m.token).unwrap();
        assert_eq!(p.id, id);
        assert!(secret_matches(&m.secret_hash, p.secret));
        let other = mint(&id).unwrap();
        assert!(!secret_matches(
            &m.secret_hash,
            parse(&other.token).unwrap().secret
        ));
        assert!(!format!("{m:?}").contains(p.secret));
        assert!(!format!("{p:?}").contains(p.secret));
    }

    #[test]
    fn malformed_tokens_are_refused() {
        let good = mint("abcd1234").unwrap();
        let t: &str = &good.token;
        assert!(parse(t).is_some());
        let cases = [
            t.replacen("h2s_", "h3s_", 1),
            t.to_uppercase(),
            format!("{t}x"),
            t[..t.len() - 1].to_owned(),
            t.replacen("abcd1234", "ABCD1234", 1),
            t.replacen("abcd1234_", "abcd1234-", 1),
            format!("{}!", &t[..t.len() - 1]),
            String::new(),
            "h2s_".into(),
        ];
        for c in cases {
            assert!(parse(&c).is_none(), "{c}");
        }
    }

    #[test]
    fn a_multibyte_character_at_the_id_boundary_is_refused() {
        // "é" (2 bytes) starts at byte 7 of the 52-byte rest, so byte 8 is inside it.
        let t = format!("h2s_abcdefgé{}", "A".repeat(SECRET_LEN));
        assert_eq!(t.len(), 4 + TOKEN_ID_LEN + 1 + SECRET_LEN);
        assert!(!t.is_char_boundary(4 + TOKEN_ID_LEN));
        assert!(parse(&t).is_none());
        assert!(token_id_hint(&t).is_none());
    }

    #[test]
    fn session_ids() {
        let (id, hash) = new_session().unwrap();
        assert_eq!(session_hash(&id), Some(hash));
        assert!(session_hash("short").is_none());
        assert!(session_hash(&format!("{}=", &id[..42])).is_none());
    }

    #[test]
    fn ids_use_the_alphabet() {
        for _ in 0..50 {
            let id = new_token_id().unwrap();
            assert!(id.bytes().all(|b| ID_ALPHABET.contains(&b)), "{id}");
        }
    }

    proptest! {
        #[test]
        fn parse_never_panics(s in ".{0,80}") {
            let _ = parse(&s);
            let _ = session_hash(&s);
        }

        /// Token-shaped strings (prefix + exactly the right byte length) with arbitrary
        /// Unicode, including multi-byte characters across the id/secret boundary.
        #[test]
        fn parse_never_panics_on_token_sized_unicode(
            head in "[a-z0-9]{0,8}",
            middle in proptest::char::any(),
            tail in "\\PC{0,52}",
        ) {
            let mut rest: String = format!("{head}{middle}{tail}");
            while rest.len() > TOKEN_ID_LEN + 1 + SECRET_LEN {
                rest.pop();
            }
            while rest.len() < TOKEN_ID_LEN + 1 + SECRET_LEN {
                rest.push('a');
            }
            let t = format!("h2s_{rest}");
            prop_assert_eq!(t.len(), 4 + TOKEN_ID_LEN + 1 + SECRET_LEN);
            let p = parse(&t);
            if !t.is_ascii() {
                prop_assert!(p.is_none());
            }
            let _ = token_id_hint(&t);
        }

        #[test]
        fn parse_accepts_exactly_the_format(id in "[a-z0-9]{8}", secret in "[A-Za-z0-9_-]{43}") {
            let t = format!("h2s_{id}_{secret}");
            let p = parse(&t).unwrap();
            prop_assert_eq!(p.id, id.as_str());
            prop_assert_eq!(p.secret, secret.as_str());
        }

        #[test]
        fn ct_eq_matches_eq(a in proptest::collection::vec(any::<u8>(), 0..40), b in proptest::collection::vec(any::<u8>(), 0..40)) {
            prop_assert_eq!(ct_eq(&a, &b), a == b);
        }
    }
}
