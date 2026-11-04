use crate::capability::Capability;

/// Requests the `message-tags` cap so the server includes `@tags` on
/// messages. `irust-proto` already parses tags unconditionally, so this
/// cap has nothing to do beyond requesting it -- it exists as the
/// simplest possible example of the `Capability` trait.
#[derive(Default)]
pub struct MessageTags;

impl Capability for MessageTags {
    fn name(&self) -> &str {
        "message-tags"
    }
}
