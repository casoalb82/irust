use crate::capability::Capability;

#[derive(Default)]
pub struct EchoMessage;

impl Capability for EchoMessage {
    fn name(&self) -> &str {
        "echo-message"
    }
}
