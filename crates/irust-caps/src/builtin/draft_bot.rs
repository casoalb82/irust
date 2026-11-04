use crate::capability::Capability;

#[derive(Default)]
pub struct DraftBot;

impl Capability for DraftBot {
    fn name(&self) -> &str {
        "draft/bot"
    }
}
