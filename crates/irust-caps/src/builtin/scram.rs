
use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};

type HmacSha256 = Hmac<Sha256>;

fn hmac_sha256(key: &[u8], data: &[u8]) -> [u8; 32] {
    let mut mac = HmacSha256::new_from_slice(key).expect("HMAC-SHA-256 accepts a key of any length");
    mac.update(data);
    mac.finalize().into_bytes().into()
}

fn sha256(data: &[u8]) -> [u8; 32] {
    Sha256::digest(data).into()
}

fn xor32(a: [u8; 32], b: [u8; 32]) -> [u8; 32] {
    let mut out = [0u8; 32];
    for i in 0..32 {
        out[i] = a[i] ^ b[i];
    }
    out
}

/// `PBKDF2(HMAC-SHA-256, password, salt, iterations, dkLen=32)` -- RFC
/// 5802's `SaltedPassword`.
pub fn salted_password(password: &[u8], salt: &[u8], iterations: u32) -> [u8; 32] {
    let mut out = [0u8; 32];
    pbkdf2::pbkdf2_hmac::<Sha256>(password, salt, iterations, &mut out);
    out
}

/// Escapes a SASL username for the `n=` attribute: RFC 5802 requires `=`
/// and `,` to become `=3D`/`=2C` (in that order -- escaping `,` first
/// would corrupt the `=2C` it just inserted on a second pass).
pub fn escape_username(name: &str) -> String {
    name.replace('=', "=3D").replace(',', "=2C")
}

pub fn client_nonce() -> Result<String, getrandom::Error> {
    let mut bytes = [0u8; 24];
    getrandom::fill(&mut bytes)?;
    Ok(B64.encode(bytes))
}

pub const GS2_HEADER_B64: &str = "biws";

/// Builds `client-first-message-bare` (`n=<user>,r=<nonce>`) -- returned
/// separately from the full wire message (`GS2_HEADER` + this) because
/// the *bare* form, not the gs2-header-prefixed one, is what feeds into
/// `AuthMessage` later.
pub fn client_first_message_bare(username: &str, nonce: &str) -> String {
    format!("n={},r={nonce}", escape_username(username))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerFirst {
    pub nonce: String,
    pub salt: Vec<u8>,
    pub iterations: u32,
}

pub fn parse_server_first(msg: &str) -> Option<ServerFirst> {
    let mut nonce = None;
    let mut salt = None;
    let mut iterations = None;
    for field in msg.split(',') {
        let (key, value) = field.split_once('=')?;
        match key {
            "r" => nonce = Some(value.to_string()),
            "s" => salt = Some(B64.decode(value).ok()?),
            "i" => iterations = value.parse::<u32>().ok(),
            _ => {}
        }
    }
    Some(ServerFirst {
        nonce: nonce?,
        salt: salt?,
        iterations: iterations?,
    })
}

/// The server's final reply: `v=<signature>` on success, `e=<text>` on
/// failure (the latter arrives alongside a 904, but is parsed here too
/// so a caller can log/show the actual reason rather than just "failed").
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServerFinal {
    Verified(Vec<u8>),
    Error(String),
}

pub fn parse_server_final(msg: &str) -> Option<ServerFinal> {
    let (key, value) = msg.split_once('=')?;
    match key {
        "v" => Some(ServerFinal::Verified(B64.decode(value).ok()?)),
        "e" => Some(ServerFinal::Error(value.to_string())),
        _ => None,
    }
}

pub struct ClientFinal {
    pub message: String,
    pub server_signature: [u8; 32],
}

pub fn client_final(
    password: &[u8],
    client_first_bare: &str,
    server_first: &ServerFirst,
    combined_nonce: &str,
) -> ClientFinal {
    let salted = salted_password(password, &server_first.salt, server_first.iterations);
    let client_final_without_proof = format!("c={GS2_HEADER_B64},r={combined_nonce}");
    let auth_message = format!(
        "{client_first_bare},{},{client_final_without_proof}",
        server_first_wire(server_first)
    );

    let client_key = hmac_sha256(&salted, b"Client Key");
    let stored_key = sha256(&client_key);
    let client_signature = hmac_sha256(&stored_key, auth_message.as_bytes());
    let client_proof = xor32(client_key, client_signature);

    let server_key = hmac_sha256(&salted, b"Server Key");
    let server_signature = hmac_sha256(&server_key, auth_message.as_bytes());

    let message = format!("{client_final_without_proof},p={}", B64.encode(client_proof));
    ClientFinal {
        message,
        server_signature,
    }
}

fn server_first_wire(sf: &ServerFirst) -> String {
    format!("r={},s={},i={}", sf.nonce, B64.encode(&sf.salt), sf.iterations)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escape_username_handles_equals_and_comma_in_the_right_order() {
        assert_eq!(escape_username("plain"), "plain");
        assert_eq!(escape_username("a,b"), "a=2Cb");
        assert_eq!(escape_username("a=b"), "a=3Db");
        // If '=' were escaped AFTER ',', this would come back wrong
        // ("a=3D2Cb" instead of "a=2C=3Db" is the wrong-order failure
        // mode this test specifically guards against).
        assert_eq!(escape_username("a,=b"), "a=2C=3Db");
    }

    #[test]
    fn client_nonce_is_url_safe_length_and_has_no_comma() {
        let nonce = client_nonce().unwrap();
        assert_eq!(nonce.len(), 32); // 24 bytes -> 32 base64 chars, no padding needed (24 % 3 == 0)
        assert!(!nonce.contains(','));
        let nonce2 = client_nonce().unwrap();
        assert_ne!(nonce, nonce2, "two nonces must not collide in practice");
    }

    #[test]
    fn parse_server_first_extracts_nonce_salt_and_iterations() {
        let sf = parse_server_first("r=fyko+d2lbbFgONRv9qkxdawL3rfcNHYJY1ZVvWVs7j,s=QSXCR+Q6sek8bf92,i=4096").unwrap();
        assert_eq!(sf.nonce, "fyko+d2lbbFgONRv9qkxdawL3rfcNHYJY1ZVvWVs7j");
        assert_eq!(sf.iterations, 4096);
        assert_eq!(B64.encode(&sf.salt), "QSXCR+Q6sek8bf92");
    }

    #[test]
    fn parse_server_final_distinguishes_success_and_error() {
        assert!(matches!(
            parse_server_final("v=cmVhbGh5").unwrap(),
            ServerFinal::Verified(_)
        ));
        assert!(matches!(parse_server_final("e=other-error").unwrap(), ServerFinal::Error(s) if s == "other-error"));
        assert!(parse_server_final("garbage").is_none());
    }

    #[test]
    fn matches_the_rfc_7677_worked_example() {
        let username = "user";
        let password = b"pencil";
        let client_nonce = "rOprNGfwEbeRWgbNEkqO";
        let server_first_wire_str =
            "r=rOprNGfwEbeRWgbNEkqO%hvYDpWUa2RaTCAfuxFIlj)hNlF$k0,s=W22ZaJ0SNY7soEsUEjb6gQ==,i=4096";

        let client_first_bare = client_first_message_bare(username, client_nonce);
        assert_eq!(client_first_bare, "n=user,r=rOprNGfwEbeRWgbNEkqO");

        let server_first = parse_server_first(server_first_wire_str).unwrap();
        assert_eq!(server_first.iterations, 4096);
        assert!(
            server_first.nonce.starts_with(client_nonce),
            "server must extend the client's own nonce"
        );

        let result = client_final(password, &client_first_bare, &server_first, &server_first.nonce);
        assert_eq!(
            result.message,
            "c=biws,r=rOprNGfwEbeRWgbNEkqO%hvYDpWUa2RaTCAfuxFIlj)hNlF$k0,p=dHzbZapWIk4jUhN+Ute9ytag9zjfMHgsqmmiz7AndVQ="
        );

        let server_final = parse_server_final("v=6rriTRBi23WpRR/wtup+mMhUZUn/dB5nLTJRsjl95G4=").unwrap();
        let ServerFinal::Verified(expected_signature) = server_final else {
            panic!("expected a verified server-final message");
        };
        assert_eq!(result.server_signature.to_vec(), expected_signature);
    }
}
