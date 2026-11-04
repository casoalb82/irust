use crate::capability::Capability;

#[derive(Default)]
pub struct UserhostInNames;

impl Capability for UserhostInNames {
    fn name(&self) -> &str {
        "userhost-in-names"
    }
}
