
use irust_config::settings_net::{bool_of, int_of, str_of};

/// A snapshot of the proxy settings; the pure functions below take one so
/// they can be tested without touching the process-global `/set` values.
#[derive(Debug, Clone)]
pub struct ProxyCfg {
    pub enabled: bool,
    pub address: String,
    pub port: u16,
    pub password: String,
    pub string: String,
    pub string_after: String,
}

impl ProxyCfg {
    pub fn from_settings() -> Self {
        ProxyCfg {
            enabled: bool_of("use_proxy"),
            address: str_of("proxy_address"),
            port: int_of("proxy_port").clamp(1, 65535) as u16,
            password: str_of("proxy_password"),
            string: str_of("proxy_string"),
            string_after: str_of("proxy_string_after"),
        }
    }

    fn active(&self) -> bool {
        self.enabled && !self.address.is_empty()
    }

    /// `(address, port)` to open the TCP connection to instead of the real
    /// server, when the proxy is enabled and has an address.
    pub fn tcp_target(&self) -> Option<(String, u16)> {
        self.active().then(|| (self.address.clone(), self.port))
    }

    /// Lines to send before the usual `CAP`/`NICK`/`USER` registration.
    pub fn registration_prelude(&self, server: &str, port: u16) -> Vec<String> {
        if !self.active() {
            return Vec::new();
        }
        let mut lines = Vec::new();
        if !self.password.is_empty() {
            lines.push(format!("PASS {}", self.password));
        }
        if !self.string.is_empty() {
            lines.push(format_line(&self.string, server, port));
        }
        lines
    }

    /// Line to send right after `USER`, if any.
    pub fn after_user(&self, server: &str, port: u16) -> Option<String> {
        (self.active() && !self.string_after.is_empty()).then(|| format_line(&self.string_after, server, port))
    }
}

pub fn tcp_target() -> Option<(String, u16)> {
    ProxyCfg::from_settings().tcp_target()
}

pub fn registration_prelude(server: &str, port: u16) -> Vec<String> {
    ProxyCfg::from_settings().registration_prelude(server, port)
}

pub fn after_user(server: &str, port: u16) -> Option<String> {
    ProxyCfg::from_settings().after_user(server, port)
}

/// The `printf`-style substitution irssi does with `g_strdup_printf(fmt,
/// address, port)`: `%s` -> address, `%d` -> port, `%%` -> `%`.
pub fn format_line(fmt: &str, address: &str, port: u16) -> String {
    let mut out = String::new();
    let mut chars = fmt.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '%' {
            out.push(c);
            continue;
        }
        match chars.peek() {
            Some('s') => {
                chars.next();
                out.push_str(address);
            }
            Some('d') => {
                chars.next();
                out.push_str(&port.to_string());
            }
            Some('%') => {
                chars.next();
                out.push('%');
            }
            _ => out.push('%'),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_line_substitutes_like_printf() {
        assert_eq!(format_line("CONNECT %s %d", "irc.example.com", 6697), "CONNECT irc.example.com 6697");
        assert_eq!(format_line("100%% %x", "h", 1), "100% %x");
    }

    fn cfg() -> ProxyCfg {
        ProxyCfg {
            enabled: false,
            address: String::new(),
            port: 6667,
            password: String::new(),
            string: "CONNECT %s %d".to_string(),
            string_after: String::new(),
        }
    }

    #[test]
    fn proxy_is_inert_until_enabled_and_addressed() {
        let mut c = cfg();
        assert!(c.tcp_target().is_none());
        assert!(c.registration_prelude("h", 1).is_empty());
        c.address = "bnc.example.com".to_string();
        assert!(c.tcp_target().is_none(), "still off without use_proxy");
        c.enabled = true;
        c.port = 9000;
        c.password = "secret".to_string();
        c.string_after = "AFTER %s".to_string();
        assert_eq!(c.tcp_target(), Some(("bnc.example.com".to_string(), 9000)));
        assert_eq!(
            c.registration_prelude("irc.example.com", 6697),
            vec!["PASS secret".to_string(), "CONNECT irc.example.com 6697".to_string()]
        );
        assert_eq!(c.after_user("irc.example.com", 6697).as_deref(), Some("AFTER irc.example.com"));
    }

    #[test]
    fn defaults_from_settings_are_off() {
        assert!(!irust_config::settings_net::NetSettings::default().get("use_proxy").unwrap().eq("on"));
    }
}
