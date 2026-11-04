use crate::capability::Capability;

#[derive(Default)]
pub struct Batch;

impl Capability for Batch {
    fn name(&self) -> &str {
        "batch"
    }
}
