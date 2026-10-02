use schemars::JsonSchema;
use schemars::schema::InstanceType;
use schemars::schema::Schema;
use schemars::schema::SchemaObject;
use serde::Deserialize;
use serde::Serialize;

/// Continuity policy for context windows. Notes requires usable remote storage.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ContextStrategy {
    #[default]
    Notes,
    Compaction,
}

/// User-message retention budget for normal compaction, in tokens.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "i64", into = "i64")]
pub enum CompactionRetentionTokens {
    Tokens16000,
    Tokens32000,
    #[default]
    Tokens64000,
}

impl CompactionRetentionTokens {
    pub const fn tokens(self) -> i64 {
        match self {
            Self::Tokens16000 => 16_000,
            Self::Tokens32000 => 32_000,
            Self::Tokens64000 => 64_000,
        }
    }
}

impl TryFrom<i64> for CompactionRetentionTokens {
    type Error = &'static str;

    fn try_from(tokens: i64) -> Result<Self, Self::Error> {
        match tokens {
            16_000 => Ok(Self::Tokens16000),
            32_000 => Ok(Self::Tokens32000),
            64_000 => Ok(Self::Tokens64000),
            _ => Err("compaction_retention_tokens must be 16000, 32000, or 64000"),
        }
    }
}

impl From<CompactionRetentionTokens> for i64 {
    fn from(value: CompactionRetentionTokens) -> Self {
        value.tokens()
    }
}

impl JsonSchema for CompactionRetentionTokens {
    fn schema_name() -> String {
        "CompactionRetentionTokens".to_string()
    }

    fn json_schema(_: &mut schemars::r#gen::SchemaGenerator) -> Schema {
        SchemaObject {
            instance_type: Some(InstanceType::Integer.into()),
            enum_values: Some(vec![16_000.into(), 32_000.into(), 64_000.into()]),
            ..Default::default()
        }
        .into()
    }
}

#[cfg(test)]
mod tests {
    use crate::config_toml::ConfigToml;
    use crate::profile_toml::ConfigProfile;

    use super::*;

    #[test]
    fn context_controls_validate_toml_values() {
        for (tokens, expected) in [
            (16_000, CompactionRetentionTokens::Tokens16000),
            (32_000, CompactionRetentionTokens::Tokens32000),
            (64_000, CompactionRetentionTokens::Tokens64000),
        ] {
            let input = format!(
                "context_strategy = 'compaction'\ncompaction_retention_tokens = {tokens}\ncontext_idle_rollover_minutes = 25"
            );
            let config: ConfigToml = toml::from_str(&input).unwrap();
            assert_eq!(config.context_strategy, Some(ContextStrategy::Compaction));
            assert_eq!(config.compaction_retention_tokens, Some(expected));
            assert_eq!(config.context_idle_rollover_minutes.unwrap().get(), 25);
            let profile: ConfigProfile = toml::from_str(&input).unwrap();
            assert_eq!(profile.context_strategy, config.context_strategy);
            assert_eq!(profile.compaction_retention_tokens, Some(expected));
            assert_eq!(
                profile.context_idle_rollover_minutes,
                config.context_idle_rollover_minutes
            );
        }
        for input in [
            "context_strategy = 'local'",
            "context_strategy = 'tree'",
            "context_strategy = 'invalid'",
            "compaction_retention_tokens = 0",
            "compaction_retention_tokens = 16001",
            "compaction_retention_tokens = -1",
            "compaction_retention_tokens = '64000'",
            "context_idle_rollover_minutes = 0",
            "context_idle_rollover_minutes = -1",
            "context_idle_rollover_minutes = 1.5",
        ] {
            assert!(toml::from_str::<ConfigToml>(input).is_err(), "{input}");
            assert!(toml::from_str::<ConfigProfile>(input).is_err(), "{input}");
        }
    }

    #[test]
    fn context_controls_serialize_as_public_toml_values() {
        let config: ConfigToml =
            toml::from_str("context_strategy = 'notes'\ncompaction_retention_tokens = 32000")
                .unwrap();
        let value = toml::Value::try_from(&config).unwrap();
        assert_eq!(value["context_strategy"].as_str(), Some("notes"));
        assert_eq!(
            value["compaction_retention_tokens"].as_integer(),
            Some(32_000)
        );
    }
}
