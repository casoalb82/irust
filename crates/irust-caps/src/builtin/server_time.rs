use crate::capability::Capability;

#[derive(Default)]
pub struct ServerTime;

impl Capability for ServerTime {
    fn name(&self) -> &str {
        "server-time"
    }
}
