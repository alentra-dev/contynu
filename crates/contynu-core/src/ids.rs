use std::str::FromStr;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::{ContynuError, Result};

macro_rules! typed_id {
    ($name:ident, $prefix:literal) => {
        #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(String);

        impl $name {
            pub const PREFIX: &'static str = $prefix;

            pub fn new() -> Self {
                Self(format!("{}_{}", Self::PREFIX, Uuid::now_v7().simple()))
            }

            pub fn parse(value: impl Into<String>) -> Result<Self> {
                let value = value.into();
                if Self::is_valid(&value) {
                    Ok(Self(value))
                } else {
                    Err(ContynuError::InvalidId {
                        prefix: Self::PREFIX,
                        value,
                    })
                }
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }

            fn is_valid(value: &str) -> bool {
                let Some(rest) = value.strip_prefix(concat!($prefix, "_")) else {
                    return false;
                };
                rest.len() == 32 && rest.chars().all(|ch| ch.is_ascii_hexdigit())
            }
        }

        impl FromStr for $name {
            type Err = ContynuError;

            fn from_str(s: &str) -> Result<Self> {
                Self::parse(s)
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(&self.0)
            }
        }

        impl TryFrom<String> for $name {
            type Error = ContynuError;

            fn try_from(value: String) -> Result<Self> {
                Self::parse(value)
            }
        }

        impl From<$name> for String {
            fn from(value: $name) -> Self {
                value.0
            }
        }
    };
}

typed_id!(SessionId, "ses");
typed_id!(TurnId, "trn");
typed_id!(EventId, "evt");
typed_id!(ArtifactId, "art");
typed_id!(CheckpointId, "chk");
typed_id!(FileId, "fil");
typed_id!(MemoryId, "mem");

#[cfg(test)]
mod tests {
    use super::{EventId, SessionId};

    #[test]
    fn ids_are_prefixed_and_parseable() {
        let session = SessionId::new();
        let event = EventId::new();

        assert!(session.as_str().starts_with("ses_"));
        assert!(event.as_str().starts_with("evt_"));
        assert_eq!(
            SessionId::parse(session.as_str()).unwrap().as_str(),
            session.as_str()
        );
    }
}
