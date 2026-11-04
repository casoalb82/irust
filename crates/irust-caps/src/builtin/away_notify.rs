use crate::capability::Capability;

#[derive(Default)]
pub struct AwayNotify;

impl Capability for AwayNotify {
    fn name(&self) -> &str {
        "away-notify"
    }
}
