use crate::capability::Capability;

pub struct Starttls {
    already_secure: bool,
}

impl Starttls {
    pub fn new(already_secure: bool) -> Self {
        Self { already_secure }
    }
}

impl Capability for Starttls {
    fn name(&self) -> &str {
        "starttls"
    }

    fn wants(&self, _offered_value: Option<&str>) -> bool {
        !self.already_secure
    }

    fn on_ack(&mut self) -> Vec<String> {
        vec!["STARTTLS".to_string()]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn never_wants_it_once_already_secure() {
        assert!(!Starttls::new(true).wants(None));
        assert!(Starttls::new(false).wants(None));
    }

    #[test]
    fn ack_sends_the_bare_command() {
        assert_eq!(Starttls::new(false).on_ack(), vec!["STARTTLS".to_string()]);
    }
}
