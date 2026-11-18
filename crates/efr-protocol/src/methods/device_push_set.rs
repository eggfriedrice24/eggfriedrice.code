//! `device.push_set`: the push token of the phone app, or none.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::SecretText;

/// The params of `device.push_set`, a remote method with the scope `read` that runs only
/// after the proof. It acts on the device of the proof. efrd announces it with the
/// capability `push`.
///
/// efrd answers `invalid` for a token that is not an Expo push token
/// ([`is_expo_push_token`]). The Unix socket refuses the method.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DevicePushSet {
    /// The Expo push token of the app, `ExponentPushToken[...]`. Absent turns push off
    /// for the device. Expo knows the bundle id and the APNs environment of a token, so
    /// the app sends neither.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token: Option<SecretText>,
}

/// The result of `device.push_set`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DevicePushSetResult {}

/// The longest inside of an Expo push token, in characters.
pub const EXPO_TOKEN_MAX_INSIDE: usize = 128;

/// True for an Expo push token: `ExponentPushToken[`, then 1 to
/// [`EXPO_TOKEN_MAX_INSIDE`] printable ASCII characters with no blank, `[` or `]`, then
/// `]`.
///
/// ```
/// use efr_protocol::is_expo_push_token;
///
/// assert!(is_expo_push_token("ExponentPushToken[xxxxxxxxxxxxxxxxxxxxxx]"));
/// assert!(!is_expo_push_token("ExponentPushToken[]"));
/// ```
pub fn is_expo_push_token(token: &str) -> bool {
    let Some(inside) =
        token.strip_prefix("ExponentPushToken[").and_then(|rest| rest.strip_suffix(']'))
    else {
        return false;
    };
    (1..=EXPO_TOKEN_MAX_INSIDE).contains(&inside.len())
        && inside.bytes().all(|byte| byte.is_ascii_graphic() && byte != b'[' && byte != b']')
}
