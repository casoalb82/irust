use crate::capability::Capability;

#[derive(Default)]
pub struct InviteNotify;

impl Capability for InviteNotify {
    fn name(&self) -> &str {
        "invite-notify"
    }
}
