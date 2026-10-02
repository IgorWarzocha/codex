//! Flat model arguments; runtime identities are supplied by the host.

use crate::ThreadSort;
use serde::Deserialize;
use serde::Deserializer;
use serde::de;
use serde_json::Value;
use std::fmt;
use std::num::NonZeroU32;
use uuid::Uuid;

pub(super) struct Call {
    pub(super) action: String,
    pub(super) args: Value,
}

impl<'de> Deserialize<'de> for Call {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct CallVisitor;

        impl<'de> de::Visitor<'de> for CallVisitor {
            type Value = Call;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("an agent_board argument object")
            }

            fn visit_map<M: de::MapAccess<'de>>(self, mut map: M) -> Result<Call, M::Error> {
                let mut fields = serde_json::Map::new();
                while let Some((key, value)) = map.next_entry::<String, Value>()? {
                    // Extracting through Value alone would silently overwrite duplicate
                    // fields that the action's original deserializer rejected.
                    if fields.insert(key.clone(), value).is_some() {
                        return Err(de::Error::custom(format!("duplicate field `{key}`")));
                    }
                }
                let action = match fields.remove("action") {
                    Some(Value::String(action)) => action,
                    Some(_) => {
                        return Err(de::Error::custom("agent_board action must be a string"));
                    }
                    None => return Err(de::Error::custom("agent_board requires action")),
                };
                Ok(Call {
                    action,
                    args: Value::Object(fields),
                })
            }
        }

        deserializer.deserialize_map(CallVisitor)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Help {
    pub(super) topic: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct CreateChannel {
    pub(super) channel_name: String,
    pub(super) subscribe: Option<bool>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct GetChannels {
    pub(super) query: Option<String>,
    pub(super) recent_first: Option<bool>,
    pub(super) limit: Option<NonZeroU32>,
    pub(super) cursor: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ListThreads {
    pub(super) channel_name: String,
    pub(super) sort: Option<ThreadSort>,
    pub(super) recent_first: Option<bool>,
    pub(super) limit: Option<NonZeroU32>,
    pub(super) cursor: Option<String>,
    pub(super) max_chars_per_post: Option<NonZeroU32>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct SearchPosts {
    pub(super) channel_name: Option<String>,
    pub(super) query: Option<String>,
    pub(super) after_message_id: Option<Uuid>,
    pub(super) author: Option<String>,
    pub(super) limit: Option<NonZeroU32>,
    pub(super) cursor: Option<String>,
    pub(super) max_chars_per_post: Option<NonZeroU32>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ReadThread {
    pub(super) thread_id: Uuid,
    pub(super) limit: Option<NonZeroU32>,
    pub(super) cursor: Option<String>,
    pub(super) max_chars_per_post: Option<NonZeroU32>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ReadPost {
    pub(super) message_id: Uuid,
    pub(super) offset_chars: Option<u32>,
    pub(super) limit_chars: Option<NonZeroU32>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Subscription {
    pub(super) channel_name: Option<String>,
    pub(super) thread_id: Option<Uuid>,
    pub(super) target_agent: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Post {
    pub(super) text: String,
    pub(super) channel_name: Option<String>,
    pub(super) new_channel_name: Option<String>,
    pub(super) thread_id: Option<Uuid>,
    pub(super) agents_to_notify: Option<Vec<String>>,
}
