pub mod codec;
pub mod connection;
pub mod proxy;
pub mod recode;
pub mod tls;

pub use connection::{ConnectOptions, Connection};

/// The `/set` values are process-global, so every test in this crate that
/// sets one (recode, hostname, proxy...) serializes on this.
#[cfg(test)]
pub(crate) static NET_SETTINGS_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
