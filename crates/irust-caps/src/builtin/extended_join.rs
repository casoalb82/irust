use crate::capability::Capability;

#[derive(Default)]
pub struct ExtendedJoin;

impl Capability for ExtendedJoin {
    fn name(&self) -> &str {
        "extended-join"
    }
}
