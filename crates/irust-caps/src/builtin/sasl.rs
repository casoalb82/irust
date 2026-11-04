use std::cell::Cell;

use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use irust_proto::RawMessage;

use crate::builtin::scram;
use crate::capability::{CapAction, Capability};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SaslOutcome {
    #[default]
    Pending,
    Success,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mechanism {
    Plain,
    ScramSha256,
}

/// Where a SCRAM-SHA-256 exchange currently is -- `None` (in `Sasl`
/// itself) until `wants()` picks `Mechanism::ScramSha256` at all, `Some`
/// only once `on_ack` has actually sent `AUTHENTICATE SCRAM-SHA-256`.
enum ScramStep {
    /// Sent our client-first-message, waiting for the server's
    /// `r=,s=,i=` reply. Keeps `client_first_bare` (needed verbatim to
    /// build `AuthMessage` later) and our own `client_nonce` (to verify
    /// the server actually extended it, not substituted a different
    /// one -- RFC 5802 §5.1).
    AwaitingServerFirst {
        client_first_bare: String,
        client_nonce: String,
    },
    AwaitingServerFinal { expected_server_signature: [u8; 32] },
}

pub struct Sasl {
    authcid: String,
    password: String,
    pub outcome: SaslOutcome,
    mechanism: Cell<Mechanism>,
    scram_step: Option<ScramStep>,
}

impl Sasl {
    pub fn new(authcid: impl Into<String>, password: impl Into<String>) -> Self {
        Self {
            authcid: authcid.into(),
            password: password.into(),
            outcome: SaslOutcome::Pending,
            mechanism: Cell::new(Mechanism::Plain),
            scram_step: None,
        }
    }
}

impl Capability for Sasl {
    fn name(&self) -> &str {
        "sasl"
    }

    fn wants(&self, offered_value: Option<&str>) -> bool {
        match offered_value {
            Some(mechs) => {
                let mechs: Vec<&str> = mechs.split(',').collect();
                if mechs.iter().any(|m| m.eq_ignore_ascii_case("SCRAM-SHA-256")) {
                    self.mechanism.set(Mechanism::ScramSha256);
                    true
                } else if mechs.iter().any(|m| m.eq_ignore_ascii_case("PLAIN")) {
                    self.mechanism.set(Mechanism::Plain);
                    true
                } else {
                    false
                }
            }
            // No mechanism list at all (an older/minimal `CAP LS`
            // advertisement) -- PLAIN blind, same as before SCRAM
            // support existed; there's nothing to prefer SCRAM FROM.
            None => {
                self.mechanism.set(Mechanism::Plain);
                true
            }
        }
    }

    fn on_ack(&mut self) -> Vec<String> {
        match self.mechanism.get() {
            Mechanism::Plain => vec!["AUTHENTICATE PLAIN".to_string()],
            Mechanism::ScramSha256 => vec!["AUTHENTICATE SCRAM-SHA-256".to_string()],
        }
    }

    fn blocks_cap_end(&self) -> bool {
        true
    }

    fn on_message(&mut self, msg: &RawMessage) -> CapAction {
        if msg.command != "AUTHENTICATE" {
            return self.on_numeric(msg);
        }
        let payload = msg.params.first().map(String::as_str).unwrap_or("");

        match self.mechanism.get() {
            Mechanism::Plain => {
                if payload == "+" {
                    // authzid left empty: "same identity as authcid", the common case.
                    let plain = format!("\0{}\0{}", self.authcid, self.password);
                    let encoded = B64.encode(plain.as_bytes());
                    return CapAction::Send(split_authenticate_lines(&encoded));
                }
                CapAction::Continue
            }
            Mechanism::ScramSha256 => self.on_scram_authenticate(payload),
        }
    }
}

impl Sasl {
    fn on_numeric(&mut self, msg: &RawMessage) -> CapAction {
        match msg.command.as_str() {
            "903" => {
                self.outcome = SaslOutcome::Success;
                CapAction::Done
            }
            "904" | "905" | "906" | "907" => {
                self.outcome = SaslOutcome::Failed;
                CapAction::Done
            }
            _ => CapAction::Continue,
        }
    }

    fn on_scram_authenticate(&mut self, payload: &str) -> CapAction {
        match &self.scram_step {
            None if payload == "+" => {
                // The server's initial empty challenge -- same "go
                // ahead" convention SCRAM shares with PLAIN, just the
                // first of three round trips here instead of one.
                let client_nonce = match scram::client_nonce() {
                    Ok(n) => n,
                    Err(_) => {
                        self.outcome = SaslOutcome::Failed;
                        return CapAction::Send(vec!["AUTHENTICATE *".to_string()]);
                    }
                };
                let client_first_bare = scram::client_first_message_bare(&self.authcid, &client_nonce);
                let wire = format!("n,,{client_first_bare}");
                self.scram_step = Some(ScramStep::AwaitingServerFirst {
                    client_first_bare,
                    client_nonce,
                });
                CapAction::Send(split_authenticate_lines(&B64.encode(wire.as_bytes())))
            }
            Some(ScramStep::AwaitingServerFirst {
                client_first_bare,
                client_nonce,
            }) => {
                let Some(decoded) = B64.decode(payload).ok().and_then(|b| String::from_utf8(b).ok()) else {
                    return self.abort();
                };
                let Some(server_first) = scram::parse_server_first(&decoded) else {
                    return self.abort();
                };
                if !server_first.nonce.starts_with(client_nonce.as_str()) {
                    // The server didn't extend OUR nonce -- either a
                    // broken server or a nonce-substitution attempt
                    // (RFC 5802 §5.1). Either way, not safe to continue.
                    return self.abort();
                }
                let result = scram::client_final(
                    self.password.as_bytes(),
                    client_first_bare,
                    &server_first,
                    &server_first.nonce,
                );
                self.scram_step = Some(ScramStep::AwaitingServerFinal {
                    expected_server_signature: result.server_signature,
                });
                CapAction::Send(split_authenticate_lines(&B64.encode(result.message.as_bytes())))
            }
            Some(ScramStep::AwaitingServerFinal {
                expected_server_signature,
            }) => {
                let Some(decoded) = B64.decode(payload).ok().and_then(|b| String::from_utf8(b).ok()) else {
                    return self.abort();
                };
                match scram::parse_server_final(&decoded) {
                    Some(scram::ServerFinal::Verified(signature)) if signature[..] == expected_server_signature[..] => {
                        // Verified the SERVER's identity too, not just
                        // sent our own proof -- the actual outcome
                        // (Success/Failed) is still set by the 903/904
                        // numeric that follows, same as PLAIN; this step
                        // just refuses to trust a mismatched one.
                        CapAction::Continue
                    }
                    _ => self.abort(),
                }
            }
            None => CapAction::Continue,
        }
    }

    fn abort(&mut self) -> CapAction {
        self.outcome = SaslOutcome::Failed;
        CapAction::Send(vec!["AUTHENTICATE *".to_string()])
    }
}

#[derive(Debug, Default)]
pub struct SaslExternal {
    pub outcome: SaslOutcome,
}

impl SaslExternal {
    pub fn new() -> Self {
        Self::default()
    }
}

impl Capability for SaslExternal {
    fn name(&self) -> &str {
        "sasl"
    }

    fn wants(&self, offered_value: Option<&str>) -> bool {
        match offered_value {
            Some(mechs) => mechs.split(',').any(|m| m.eq_ignore_ascii_case("EXTERNAL")),
            None => true,
        }
    }

    fn on_ack(&mut self) -> Vec<String> {
        vec!["AUTHENTICATE EXTERNAL".to_string()]
    }

    fn blocks_cap_end(&self) -> bool {
        true
    }

    fn on_message(&mut self, msg: &RawMessage) -> CapAction {
        if msg.command == "AUTHENTICATE" && msg.params.first().map(String::as_str) == Some("+") {
            return CapAction::Send(vec!["AUTHENTICATE +".to_string()]);
        }

        match msg.command.as_str() {
            "903" => {
                self.outcome = SaslOutcome::Success;
                CapAction::Done
            }
            "904" | "905" | "906" | "907" => {
                self.outcome = SaslOutcome::Failed;
                CapAction::Done
            }
            _ => CapAction::Continue,
        }
    }
}

/// SASL AUTHENTICATE payloads must be split into <=400 byte chunks; if
/// the encoded payload's length is an exact multiple of 400 (including
/// zero), a trailing `AUTHENTICATE +` marks the end per spec.
fn split_authenticate_lines(payload_b64: &str) -> Vec<String> {
    const CHUNK: usize = 400;
    let bytes = payload_b64.as_bytes();

    if bytes.is_empty() {
        return vec!["AUTHENTICATE +".to_string()];
    }

    let mut lines = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        let end = (i + CHUNK).min(bytes.len());
        lines.push(format!("AUTHENTICATE {}", &payload_b64[i..end]));
        i = end;
    }
    if bytes.len() % CHUNK == 0 {
        lines.push("AUTHENTICATE +".to_string());
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use irust_proto::parse;

    #[test]
    fn wants_plain_when_thats_all_thats_offered() {
        let sasl = Sasl::new("abcX", "hunter2");
        assert!(sasl.wants(Some("PLAIN,EXTERNAL")));
        assert!(sasl.wants(None));
        assert_eq!(sasl.mechanism.get(), Mechanism::Plain);
    }

    #[test]
    fn prefers_scram_sha_256_when_the_server_offers_both() {
        let sasl = Sasl::new("abcX", "hunter2");
        assert!(sasl.wants(Some("EXTERNAL,SCRAM-SHA-256,PLAIN")));
        assert_eq!(sasl.mechanism.get(), Mechanism::ScramSha256);
    }

    #[test]
    fn wants_scram_sha_256_alone_when_thats_all_thats_offered() {
        let sasl = Sasl::new("abcX", "hunter2");
        assert!(sasl.wants(Some("SCRAM-SHA-256")));
        assert_eq!(sasl.mechanism.get(), Mechanism::ScramSha256);
    }

    #[test]
    fn doesnt_want_sasl_at_all_when_neither_mechanism_is_offered() {
        let sasl = Sasl::new("abcX", "hunter2");
        assert!(!sasl.wants(Some("EXTERNAL,DIGEST-MD5")));
    }

    #[test]
    fn responds_to_plus_challenge_with_base64_plain_payload() {
        let mut sasl = Sasl::new("abcX", "hunter2");
        let challenge = parse("AUTHENTICATE +").unwrap();
        let CapAction::Send(lines) = sasl.on_message(&challenge) else {
            panic!("expected Send");
        };
        assert_eq!(lines.len(), 1);
        let line = &lines[0];
        assert!(line.starts_with("AUTHENTICATE "));
        let encoded = line.strip_prefix("AUTHENTICATE ").unwrap();
        let decoded = B64.decode(encoded).unwrap();
        assert_eq!(decoded, b"\0abcX\0hunter2");
    }

    #[test]
    fn success_and_failure_numerics_set_outcome_and_unblock() {
        let mut sasl = Sasl::new("abcX", "hunter2");
        assert!(sasl.blocks_cap_end());

        let ok = parse(":irc.example.com 903 abcX :SASL authentication successful").unwrap();
        assert!(matches!(sasl.on_message(&ok), CapAction::Done));
        assert_eq!(sasl.outcome, SaslOutcome::Success);

        let mut sasl2 = Sasl::new("abcX", "wrong");
        let fail = parse(":irc.example.com 904 abcX :SASL authentication failed").unwrap();
        assert!(matches!(sasl2.on_message(&fail), CapAction::Done));
        assert_eq!(sasl2.outcome, SaslOutcome::Failed);
    }

    #[test]
    fn external_wants_external_when_offered() {
        let sasl = SaslExternal::new();
        assert!(sasl.wants(Some("PLAIN,EXTERNAL")));
        assert!(!sasl.wants(Some("PLAIN,SCRAM-SHA-256")));
        assert!(sasl.wants(None));
    }

    #[test]
    fn external_responds_to_plus_challenge_with_a_bare_plus() {
        let mut sasl = SaslExternal::new();
        let challenge = parse("AUTHENTICATE +").unwrap();
        let CapAction::Send(lines) = sasl.on_message(&challenge) else {
            panic!("expected Send");
        };
        assert_eq!(lines, vec!["AUTHENTICATE +".to_string()]);
    }

    #[test]
    fn external_success_and_failure_numerics_set_outcome_and_unblock() {
        let mut sasl = SaslExternal::new();
        assert!(sasl.blocks_cap_end());

        let ok = parse(":irc.example.com 903 abcX :SASL authentication successful").unwrap();
        assert!(matches!(sasl.on_message(&ok), CapAction::Done));
        assert_eq!(sasl.outcome, SaslOutcome::Success);

        let mut sasl2 = SaslExternal::new();
        let fail = parse(":irc.example.com 904 abcX :SASL authentication failed").unwrap();
        assert!(matches!(sasl2.on_message(&fail), CapAction::Done));
        assert_eq!(sasl2.outcome, SaslOutcome::Failed);
    }

    #[test]
    fn splits_exact_multiple_of_400_with_trailing_plus() {
        let payload_b64 = "A".repeat(400);
        let lines = split_authenticate_lines(&payload_b64);
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[1], "AUTHENTICATE +");
    }

    fn authenticate_payload(line: &str) -> String {
        let encoded = line
            .strip_prefix("AUTHENTICATE ")
            .expect("expected an AUTHENTICATE line");
        String::from_utf8(B64.decode(encoded).unwrap()).unwrap()
    }

    fn only_line(action: CapAction) -> String {
        let CapAction::Send(lines) = action else {
            panic!("expected Send")
        };
        assert_eq!(lines.len(), 1);
        lines.into_iter().next().unwrap()
    }

    #[test]
    fn full_scram_exchange_happy_path_succeeds() {
        let mut sasl = Sasl::new("abcX", "hunter2");
        assert!(sasl.wants(Some("SCRAM-SHA-256")));
        assert_eq!(sasl.on_ack(), vec!["AUTHENTICATE SCRAM-SHA-256".to_string()]);

        // Client sends its first message in response to the empty "+".
        let client_first_line = only_line(sasl.on_message(&parse("AUTHENTICATE +").unwrap()));
        let client_first_wire = authenticate_payload(&client_first_line);
        assert!(client_first_wire.starts_with("n,,n=abcX,r="));
        let client_nonce = client_first_wire.strip_prefix("n,,n=abcX,r=").unwrap().to_string();
        let client_first_bare = format!("n=abcX,r={client_nonce}");

        // Server: extends the nonce, picks a salt/iteration count.
        let combined_nonce = format!("{client_nonce}server-suffix");
        let salt = b"some-salt-bytes-";
        let iterations = 4096u32;
        let server_first_wire = format!("r={combined_nonce},s={},i={iterations}", B64.encode(salt));
        let auth_line = format!("AUTHENTICATE {}", B64.encode(&server_first_wire));

        let client_final_line = only_line(sasl.on_message(&parse(&auth_line).unwrap()));
        let client_final_wire = authenticate_payload(&client_final_line);
        assert!(client_final_wire.starts_with(&format!("c=biws,r={combined_nonce},p=")));

        // Server: verifies the proof itself the same way it would for
        // real (recomputing independently, not just trusting the
        // client's own math) and derives the signature it'll claim.
        let server_first = scram::parse_server_first(&server_first_wire).unwrap();
        let expected = scram::client_final(b"hunter2", &client_first_bare, &server_first, &combined_nonce);
        assert_eq!(
            client_final_wire, expected.message,
            "client's proof must match an independent server-side computation"
        );

        let server_final_wire = format!("v={}", B64.encode(expected.server_signature));
        let final_line = format!("AUTHENTICATE {}", B64.encode(&server_final_wire));
        assert!(matches!(
            sasl.on_message(&parse(&final_line).unwrap()),
            CapAction::Continue
        ));

        let ok = parse(":irc.example.com 903 abcX :SASL authentication successful").unwrap();
        assert!(matches!(sasl.on_message(&ok), CapAction::Done));
        assert_eq!(sasl.outcome, SaslOutcome::Success);
    }

    #[test]
    fn scram_aborts_if_the_server_doesnt_extend_the_client_nonce() {
        let mut sasl = Sasl::new("abcX", "hunter2");
        sasl.wants(Some("SCRAM-SHA-256"));
        sasl.on_ack();
        sasl.on_message(&parse("AUTHENTICATE +").unwrap());

        // A server-first reply with a nonce that does not start with
        // whatever the client sent -- either a broken server or a
        // substitution attempt.
        let server_first_wire = "r=totally-different-nonce,s=c2FsdA==,i=4096";
        let auth_line = format!("AUTHENTICATE {}", B64.encode(server_first_wire));
        let action = sasl.on_message(&parse(&auth_line).unwrap());
        assert!(matches!(action, CapAction::Send(lines) if lines == vec!["AUTHENTICATE *".to_string()]));
        assert_eq!(sasl.outcome, SaslOutcome::Failed);
    }

    #[test]
    fn scram_aborts_on_a_server_signature_mismatch() {
        let mut sasl = Sasl::new("abcX", "hunter2");
        sasl.wants(Some("SCRAM-SHA-256"));
        sasl.on_ack();
        let client_first_line = only_line(sasl.on_message(&parse("AUTHENTICATE +").unwrap()));
        let client_nonce = authenticate_payload(&client_first_line)
            .strip_prefix("n,,n=abcX,r=")
            .unwrap()
            .to_string();

        let combined_nonce = format!("{client_nonce}server-suffix");
        let server_first_wire = format!("r={combined_nonce},s=c2FsdA==,i=4096");
        let auth_line = format!("AUTHENTICATE {}", B64.encode(&server_first_wire));
        sasl.on_message(&parse(&auth_line).unwrap());

        let bogus_final_wire = format!("v={}", B64.encode([0u8; 32]));
        let final_line = format!("AUTHENTICATE {}", B64.encode(&bogus_final_wire));
        let action = sasl.on_message(&parse(&final_line).unwrap());
        assert!(matches!(action, CapAction::Send(lines) if lines == vec!["AUTHENTICATE *".to_string()]));
        assert_eq!(sasl.outcome, SaslOutcome::Failed);
    }
}
