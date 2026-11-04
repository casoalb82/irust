
pub fn expand(template: &str) -> String {
    let mut out = String::new();
    let mut rest = template;
    while let Some(i) = rest.find('$') {
        out.push_str(&rest[..i]);
        rest = &rest[i + 1..];
        if let Some(r) = rest.strip_prefix('$') {
            out.push('$');
            rest = r;
        } else if let Some(r) = rest.strip_prefix("sysname") {
            out.push_str(std::env::consts::OS);
            rest = r;
        } else if let Some(r) = rest.strip_prefix("sysarch") {
            out.push_str(std::env::consts::ARCH);
            rest = r;
        } else if let Some(r) = rest.strip_prefix('J') {
            out.push_str(env!("CARGO_PKG_VERSION"));
            rest = r;
        } else if let Some(r) = rest.strip_prefix('Y') {
            out.push_str(&irust_config::settings::realname());
            rest = r;
        } else {
            out.push('$');
        }
    }
    out.push_str(rest);
    out
}

/// `\x01<CMD> <expanded>\x01`, or `None` when the setting is empty.
pub fn reply_for(cmd: &str, setting: &str) -> Option<String> {
    if setting.is_empty() {
        return None;
    }
    Some(format!("\u{1}{cmd} {}\u{1}", expand(setting)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expands_the_supported_expandos() {
        assert_eq!(expand("irust $J on $sysname $sysarch"), format!("irust {} on {} {}", env!("CARGO_PKG_VERSION"), std::env::consts::OS, std::env::consts::ARCH));
        assert_eq!(expand("cost: $$5"), "cost: $5");
        assert_eq!(expand("unknown $Q stays"), "unknown $Q stays");
        assert_eq!(expand("trailing $"), "trailing $");
    }

    #[test]
    fn empty_setting_means_no_reply() {
        assert_eq!(reply_for("USERINFO", ""), None);
        assert_eq!(reply_for("VERSION", "hi").as_deref(), Some("\u{1}VERSION hi\u{1}"));
    }
}
