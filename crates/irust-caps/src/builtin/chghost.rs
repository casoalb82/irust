use crate::capability::Capability;

#[derive(Default)]
pub struct ChgHost;

impl Capability for ChgHost {
    fn name(&self) -> &str {
        "chghost"
    }
}
