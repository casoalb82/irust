use crate::capability::Capability;

#[derive(Default)]
pub struct AccountNotify;

impl Capability for AccountNotify {
    fn name(&self) -> &str {
        "account-notify"
    }
}
