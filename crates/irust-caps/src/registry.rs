use irust_proto::RawMessage;

use crate::capability::{CapAction, Capability};

/// Drives IRCv3 `CAP` negotiation generically over whatever capabilities
/// were registered. Nothing in here matches on a specific cap name --
/// that knowledge lives entirely inside each `Capability` impl.
pub struct CapabilityRegistry {
    caps: Vec<Box<dyn Capability>>,
    /// Requested, waiting on ACK/NAK.
    pending: Vec<String>,
    /// ACKed and, if `blocks_cap_end()`, not yet `Done`.
    blocking: Vec<String>,
    /// ACKed caps (superset of `blocking`).
    active: Vec<String>,
    negotiating: bool,
    ls_buffer: String,
    ls_received: bool,
}

impl CapabilityRegistry {
    pub fn new(caps: Vec<Box<dyn Capability>>) -> Self {
        Self {
            caps,
            pending: Vec::new(),
            blocking: Vec::new(),
            active: Vec::new(),
            negotiating: false,
            ls_buffer: String::new(),
            ls_received: false,
        }
    }

    pub fn active_caps(&self) -> &[String] {
        &self.active
    }

    pub fn is_negotiating(&self) -> bool {
        self.negotiating
    }

    /// Lines to send to kick off negotiation. Call once, before
    /// registration (`NICK`/`USER`).
    pub fn start(&mut self) -> Vec<String> {
        self.negotiating = true;
        vec!["CAP LS 302".to_string()]
    }

    /// Feed every incoming message through here. Returns lines to send
    /// back (may be empty).
    pub fn handle_message(&mut self, msg: &RawMessage) -> Vec<String> {
        if msg.command == "CAP" && msg.params.len() >= 3 {
            return self.handle_cap_line(msg);
        }

        let mut out = Vec::new();
        for cap in self.caps.iter_mut() {
            match cap.on_message(msg) {
                CapAction::Continue => {}
                CapAction::Send(lines) => out.extend(lines),
                CapAction::Done => {
                    let name = cap.name().to_string();
                    self.blocking.retain(|n| n != &name);
                }
            }
        }
        if self.negotiating {
            self.maybe_end(&mut out);
        }
        out
    }

    fn handle_cap_line(&mut self, msg: &RawMessage) -> Vec<String> {
        let mut out = Vec::new();
        let subcommand = msg.params[1].to_uppercase();

        match subcommand.as_str() {
            "LS" => {
                // Multiline LS looks like: CAP <nick> LS * :chunk
                // The final chunk omits the '*' marker:  CAP <nick> LS :chunk
                let is_continuation = msg.params.len() >= 4 && msg.params[2] == "*";
                let chunk = msg.params.last().cloned().unwrap_or_default();
                if !self.ls_buffer.is_empty() && !chunk.is_empty() {
                    self.ls_buffer.push(' ');
                }
                self.ls_buffer.push_str(&chunk);

                if is_continuation {
                    return out;
                }

                let offered = std::mem::take(&mut self.ls_buffer);
                self.ls_received = true;
                let mut wanted = Vec::new();
                for token in offered.split(' ').filter(|t| !t.is_empty()) {
                    let (cap_name, value) = match token.split_once('=') {
                        Some((n, v)) => (n, Some(v)),
                        None => (token, None),
                    };
                    if let Some(cap) = self.caps.iter().find(|c| c.name() == cap_name) {
                        if cap.wants(value) {
                            wanted.push(cap_name.to_string());
                        }
                    }
                }

                if wanted.is_empty() {
                    self.negotiating = false;
                    out.push("CAP END".to_string());
                } else {
                    self.pending.extend(wanted.iter().cloned());
                    out.push(format!("CAP REQ :{}", wanted.join(" ")));
                }
            }
            "ACK" => {
                let names = msg.params.last().cloned().unwrap_or_default();
                for name in names.split(' ').filter(|t| !t.is_empty()) {
                    self.pending.retain(|p| p != name);
                    self.active.push(name.to_string());
                    if let Some(cap) = self.caps.iter_mut().find(|c| c.name() == name) {
                        out.extend(cap.on_ack());
                        if cap.blocks_cap_end() {
                            self.blocking.push(name.to_string());
                        }
                    }
                }
                self.maybe_end(&mut out);
            }
            "NEW" => {
                let announced = msg.params.last().cloned().unwrap_or_default();
                let mut wanted = Vec::new();
                for token in announced.split(' ').filter(|t| !t.is_empty()) {
                    let (cap_name, value) = match token.split_once('=') {
                        Some((n, v)) => (n, Some(v)),
                        None => (token, None),
                    };
                    let already_have =
                        self.active.iter().any(|n| n == cap_name) || self.pending.iter().any(|n| n == cap_name);
                    if already_have {
                        continue;
                    }
                    if let Some(cap) = self.caps.iter().find(|c| c.name() == cap_name) {
                        if cap.wants(value) {
                            wanted.push(cap_name.to_string());
                        }
                    }
                }
                if !wanted.is_empty() {
                    self.pending.extend(wanted.iter().cloned());
                    out.push(format!("CAP REQ :{}", wanted.join(" ")));
                }
            }
            "DEL" => {
                // The server/bouncer is taking a capability away (ZNC
                // does this around client detach/reattach for some of
                // its own virtual caps) -- drop it from every tracked
                // set so a stale entry doesn't linger in `active_caps()`.
                let removed = msg.params.last().cloned().unwrap_or_default();
                for name in removed.split(' ').filter(|t| !t.is_empty()) {
                    self.active.retain(|n| n != name);
                    self.blocking.retain(|n| n != name);
                    self.pending.retain(|n| n != name);
                }
            }
            "NAK" => {
                let names = msg.params.last().cloned().unwrap_or_default();
                for name in names.split(' ').filter(|t| !t.is_empty()) {
                    self.pending.retain(|p| p != name);
                    if let Some(cap) = self.caps.iter_mut().find(|c| c.name() == name) {
                        out.extend(cap.on_nak());
                    }
                }
                self.maybe_end(&mut out);
            }
            _ => {}
        }

        out
    }

    fn maybe_end(&mut self, out: &mut Vec<String>) {
        if self.negotiating && self.ls_received && self.pending.is_empty() && self.blocking.is_empty() {
            self.negotiating = false;
            out.push("CAP END".to_string());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::builtin::{AwayNotify, MessageTags, Sasl, SaslOutcome};
    use irust_proto::parse;

    fn full_registry() -> CapabilityRegistry {
        CapabilityRegistry::new(vec![Box::new(Sasl::new("abcX", "hunter2")), Box::new(MessageTags)])
    }

    #[test]
    fn regression_unrelated_message_before_ls_must_not_end_negotiation() {
        let mut reg = full_registry();
        reg.start();

        let notice = parse(":gallium.libera.chat NOTICE * :*** Checking Ident").unwrap();
        let out = reg.handle_message(&notice);
        assert!(out.is_empty(), "must not react before CAP LS has arrived");
        assert!(reg.is_negotiating());
    }

    #[test]
    fn single_line_ls_requests_only_known_wanted_caps() {
        let mut reg = full_registry();
        reg.start();

        let ls = parse("CAP * LS :sasl=PLAIN,EXTERNAL message-tags server-time").unwrap();
        let out = reg.handle_message(&ls);

        assert_eq!(out.len(), 1);
        assert!(out[0].starts_with("CAP REQ :"));
        // server-time isn't a registered capability, so it must never be requested
        assert!(!out[0].contains("server-time"));
        assert!(out[0].contains("sasl"));
        assert!(out[0].contains("message-tags"));
    }

    #[test]
    fn multiline_ls_is_accumulated_before_requesting() {
        let mut reg = full_registry();
        reg.start();

        let chunk1 = parse("CAP * LS * :sasl=PLAIN").unwrap();
        let out1 = reg.handle_message(&chunk1);
        assert!(out1.is_empty(), "must wait for the final LS chunk");

        let chunk2 = parse("CAP * LS :message-tags").unwrap();
        let out2 = reg.handle_message(&chunk2);
        assert_eq!(out2.len(), 1);
        assert!(out2[0].contains("sasl"));
        assert!(out2[0].contains("message-tags"));
    }

    #[test]
    fn cap_end_waits_for_sasl_to_finish_even_after_ack() {
        let mut reg = full_registry();
        reg.start();
        reg.handle_message(&parse("CAP * LS :sasl message-tags").unwrap());

        // ACK both -- message-tags has nothing left to do, but sasl blocks
        // CAP END until the AUTHENTICATE exchange finishes.
        let ack = parse("CAP * ACK :sasl message-tags").unwrap();
        let out = reg.handle_message(&ack);
        assert!(
            out.iter().any(|l| l == "AUTHENTICATE PLAIN"),
            "sasl's on_ack should kick off AUTHENTICATE"
        );
        assert!(
            !out.iter().any(|l| l == "CAP END"),
            "must not end negotiation while sasl is still authenticating"
        );
        assert!(reg.is_negotiating());

        let challenge = parse("AUTHENTICATE +").unwrap();
        let out2 = reg.handle_message(&challenge);
        assert!(out2.iter().any(|l| l.starts_with("AUTHENTICATE ")));
        assert!(reg.is_negotiating(), "still waiting on 903/904");

        let success = parse(":irc.example.com 903 abcX :SASL authentication successful").unwrap();
        let out3 = reg.handle_message(&success);
        assert!(out3.contains(&"CAP END".to_string()));
        assert!(!reg.is_negotiating());
        assert_eq!(reg.active_caps(), &["sasl".to_string(), "message-tags".to_string()]);
    }

    #[test]
    fn nak_of_the_only_requested_cap_ends_negotiation_immediately() {
        let mut reg = CapabilityRegistry::new(vec![Box::new(MessageTags)]);
        reg.start();
        reg.handle_message(&parse("CAP * LS :message-tags").unwrap());

        let nak = parse("CAP * NAK :message-tags").unwrap();
        let out = reg.handle_message(&nak);
        assert_eq!(out, vec!["CAP END".to_string()]);
        assert!(!reg.is_negotiating());
    }

    #[test]
    fn no_wanted_caps_ends_negotiation_right_after_ls() {
        let mut reg = CapabilityRegistry::new(vec![Box::new(MessageTags)]);
        reg.start();

        let ls = parse("CAP * LS :server-time chathistory").unwrap();
        let out = reg.handle_message(&ls);
        assert_eq!(out, vec!["CAP END".to_string()]);
        assert!(!reg.is_negotiating());
    }

    #[test]
    fn sasl_failure_still_unblocks_cap_end() {
        let mut reg = CapabilityRegistry::new(vec![Box::new(Sasl::new("abcX", "wrong"))]);
        reg.start();
        reg.handle_message(&parse("CAP * LS :sasl").unwrap());
        reg.handle_message(&parse("CAP * ACK :sasl").unwrap());
        reg.handle_message(&parse("AUTHENTICATE +").unwrap());

        let fail = parse(":irc.example.com 904 abcX :SASL authentication failed").unwrap();
        let out = reg.handle_message(&fail);
        assert!(out.contains(&"CAP END".to_string()));
        assert!(!reg.is_negotiating());
        let _ = SaslOutcome::Failed; // outcome itself is covered in sasl.rs's own tests
    }

    #[test]
    fn cap_new_after_negotiation_requests_a_wanted_capability() {
        let mut reg = CapabilityRegistry::new(vec![Box::new(AwayNotify)]);
        reg.start();
        let ls = parse("CAP * LS :server-time").unwrap();
        let out = reg.handle_message(&ls);
        assert_eq!(out, vec!["CAP END".to_string()]);
        assert!(!reg.is_negotiating());

        let cap_new = parse(":irc.znc.in CAP sh NEW :away-notify").unwrap();
        let out = reg.handle_message(&cap_new);
        assert_eq!(out, vec!["CAP REQ :away-notify".to_string()]);
        // Still not "negotiating" in the CAP-LS sense -- this must not
        // resend CAP END or otherwise reopen initial negotiation.
        assert!(!reg.is_negotiating());

        let ack = parse("CAP sh ACK :away-notify").unwrap();
        let out = reg.handle_message(&ack);
        assert!(out.is_empty(), "no CAP END should be resent this late: {out:?}");
        assert!(reg.active_caps().contains(&"away-notify".to_string()));
    }

    /// A `CAP NEW` for something already active (or already requested)
    /// must not re-request it.
    #[test]
    fn cap_new_for_an_already_active_capability_is_a_no_op() {
        let mut reg = CapabilityRegistry::new(vec![Box::new(AwayNotify)]);
        reg.start();
        reg.handle_message(&parse("CAP * LS :away-notify").unwrap());
        reg.handle_message(&parse("CAP * ACK :away-notify").unwrap());
        assert!(reg.active_caps().contains(&"away-notify".to_string()));

        let cap_new = parse(":irc.znc.in CAP sh NEW :away-notify").unwrap();
        let out = reg.handle_message(&cap_new);
        assert!(out.is_empty(), "already-active cap must not be re-requested: {out:?}");
    }

    /// `CAP DEL` must drop the capability from `active_caps()`.
    #[test]
    fn cap_del_removes_the_capability_from_active() {
        let mut reg = CapabilityRegistry::new(vec![Box::new(AwayNotify)]);
        reg.start();
        reg.handle_message(&parse("CAP * LS :away-notify").unwrap());
        reg.handle_message(&parse("CAP * ACK :away-notify").unwrap());
        assert!(reg.active_caps().contains(&"away-notify".to_string()));

        let cap_del = parse(":irc.znc.in CAP sh DEL :away-notify").unwrap();
        reg.handle_message(&cap_del);
        assert!(!reg.active_caps().contains(&"away-notify".to_string()));
    }
}
