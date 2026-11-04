use irust_proto::RawMessage;

/// What a capability wants to do in response to a message it just saw.
pub enum CapAction {
    /// Nothing to do.
    Continue,
    /// Send these raw lines back to the server (e.g. SASL's next
    /// AUTHENTICATE step).
    Done,
    /// This cap's post-negotiation business (e.g. SASL's AUTHENTICATE
    /// exchange) is finished -- the registry can stop blocking CAP END
    /// on it.
    Send(Vec<String>),
}

pub trait Capability: Send {
    /// The exact IRCv3 cap token, e.g. "sasl", "message-tags".
    fn name(&self) -> &str;

    /// Called once per cap name the server advertised in `CAP LS`, with
    /// the `cap=value` payload if the server sent one (e.g. SASL's
    /// mechanism list). Return true to `CAP REQ` it.
    fn wants(&self, offered_value: Option<&str>) -> bool {
        let _ = offered_value;
        true
    }

    /// The server ACKed this cap -- now active. May return lines to send
    /// immediately (e.g. SASL kicking off `AUTHENTICATE PLAIN`).
    fn on_ack(&mut self) -> Vec<String> {
        Vec::new()
    }

    fn on_nak(&mut self) -> Vec<String> {
        Vec::new()
    }

    /// True if, once ACKed, this cap needs further async work before
    /// `CAP END` should be sent (SASL: yes, until 903/904; most caps: no).
    fn blocks_cap_end(&self) -> bool {
        false
    }

    /// Called for every non-CAP message once this cap is active, so it
    /// can react to protocol events relevant to it.
    fn on_message(&mut self, _msg: &RawMessage) -> CapAction {
        CapAction::Continue
    }
}
