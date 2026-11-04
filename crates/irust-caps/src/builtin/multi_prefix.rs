use crate::capability::Capability;

#[derive(Default)]
pub struct MultiPrefix;

impl Capability for MultiPrefix {
    fn name(&self) -> &str {
        "multi-prefix"
    }
}
