use crate::capability::Capability;

#[derive(Default)]
pub struct Setname;

impl Capability for Setname {
    fn name(&self) -> &str {
        "setname"
    }
}
