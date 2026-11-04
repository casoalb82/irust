/// One server address within a network preset. `port` is optional --
/// `None` falls back to the preset's `default_port`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerAddr {
    pub host: String,
    pub port: Option<u16>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NetworkPreset {
    pub id: String,
    pub display_name: String,
    pub servers: Vec<ServerAddr>,
    pub default_port: u16,
    pub tls: bool,
    pub autojoin: Vec<String>,
}
