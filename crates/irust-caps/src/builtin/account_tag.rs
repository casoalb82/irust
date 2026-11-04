use crate::capability::Capability;

#[derive(Default)]
pub struct AccountTag;

impl Capability for AccountTag {
    fn name(&self) -> &str {
        "account-tag"
    }
}
