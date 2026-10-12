use std::str::FromStr;

use efr_protocol::{ConversationId, Presence};

use super::{AlwaysPresent, PresenceReader};

/// A reader that names only the presence, as every reader before phase C did.
#[derive(Debug)]
struct OnlyPresence;

impl PresenceReader for OnlyPresence {
    fn presence(&self, _conversation_id: ConversationId) -> Presence {
        Presence::Present
    }
}

#[test]
fn presence_followed_defaults_to_false() {
    let id = ConversationId::from_str("01928c4e-7a3b-7c1d-8e2f-000000000002").unwrap();
    assert!(!OnlyPresence.followed(id), "a reader that does not know counts nobody there");
    assert!(AlwaysPresent.followed(id), "always present keeps its meaning");
    assert_eq!(AlwaysPresent.presence(id), Presence::Present);
}
