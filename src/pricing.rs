//! Pricing tables: embedded defaults, user overrides, and model lookup.
//!
//! Rates are USD per million tokens, held as [`Decimal`] — floats are for
//! graphs, not ledgers. The default table ships inside the binary via
//! `include_str!`; a user table merges over it per model.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use rust_decimal::Decimal;
use serde::Deserialize;

/// The embedded default pricing table (see `pricing/default.toml` for
/// sources and verification date).
const DEFAULT_TOML: &str = include_str!("../pricing/default.toml");

/// USD per million tokens for one model.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub struct ModelPricing {
    /// Uncached input tokens.
    pub input: Decimal,
    /// Output tokens.
    pub output: Decimal,
    /// Cache writes with a 5-minute TTL (1.25x input).
    pub cache_write_5m: Decimal,
    /// Cache writes with a 1-hour TTL (2x input).
    pub cache_write_1h: Decimal,
    /// Cache reads (0.1x input).
    pub cache_read: Decimal,
}

/// Errors from loading a pricing table.
#[derive(Debug, thiserror::Error)]
pub enum PricingError {
    /// The TOML failed to parse or had the wrong shape.
    #[error("invalid pricing table: {0}")]
    Parse(#[from] toml::de::Error),
    /// The file could not be read.
    #[error("cannot read pricing file: {0}")]
    Io(#[from] std::io::Error),
    /// A `[shadow]` entry names a model with no pricing entry.
    #[error("shadow mapping for {model:?} names {reference:?}, which has no pricing entry")]
    UnknownShadowReference {
        /// The zero-rated or unpriced model the mapping is for.
        model: String,
        /// The reference id that could not be priced.
        reference: String,
    },
}

/// A set of per-model rates, keyed by model-id prefix.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct PricingTable {
    #[serde(default)]
    models: BTreeMap<String, ModelPricing>,
    /// Stand-in rates for models that are deliberately zero-rated or absent:
    /// observed model id -> the id whose rates represent it. Feeds `doctor`'s
    /// shadow estimate only; the cost engine never consults it.
    #[serde(default)]
    shadow: BTreeMap<String, String>,
}

/// Longest-prefix match with a `-` boundary, shared by the rate and shadow
/// maps so the two can never disagree about what an id resolves to.
fn longest_prefix_match<'a, V>(map: &'a BTreeMap<String, V>, model: &str) -> Option<&'a V> {
    map.iter()
        .filter(|(key, _)| {
            model == key.as_str()
                || (model.starts_with(key.as_str())
                    && model.as_bytes().get(key.len()) == Some(&b'-'))
        })
        .max_by_key(|(key, _)| key.len())
        .map(|(_, value)| value)
}

impl PricingTable {
    /// The pricing table compiled into the binary.
    pub fn embedded() -> Self {
        // The embedded TOML is validated by a unit test; if it were ever
        // corrupted, CI fails rather than this panicking at runtime.
        Self::parse(DEFAULT_TOML).unwrap_or_default()
    }

    /// Parse a pricing table from TOML text.
    pub fn parse(text: &str) -> Result<Self, PricingError> {
        Ok(toml::from_str(text)?)
    }

    /// Load a pricing table from a file.
    pub fn load(path: &Path) -> Result<Self, PricingError> {
        Self::parse(&std::fs::read_to_string(path)?)
    }

    /// Find rates for a model id by longest-prefix match with a `-`
    /// boundary, so `claude-opus-4-8-20270101` matches a `claude-opus-4-8`
    /// entry (and a more specific entry always beats a shorter one), while
    /// `gpt-5.41` does *not* spuriously match a `gpt-5.4` entry.
    pub fn lookup(&self, model: &str) -> Option<&ModelPricing> {
        longest_prefix_match(&self.models, model)
    }

    /// The reference model whose rates stand in for `model` in `doctor`'s
    /// shadow estimate, if the table defines one. Resolved by the same
    /// prefix rule as [`Self::lookup`].
    pub fn shadow_reference(&self, model: &str) -> Option<&str> {
        longest_prefix_match(&self.shadow, model).map(String::as_str)
    }

    /// Check that every `[shadow]` value names a model this table can price.
    ///
    /// Run *after* merging user overrides: a user table may legitimately
    /// reference a model defined only in the embedded defaults, so validating
    /// each layer in isolation would reject valid configurations.
    pub fn validate(&self) -> Result<(), PricingError> {
        for (model, reference) in &self.shadow {
            if self.lookup(reference).is_none() {
                return Err(PricingError::UnknownShadowReference {
                    model: model.clone(),
                    reference: reference.clone(),
                });
            }
        }
        Ok(())
    }

    /// Merge `overrides` over this table: per-model replacement, new
    /// models added, and the same for shadow mappings.
    pub fn merge(&mut self, overrides: PricingTable) {
        self.models.extend(overrides.models);
        self.shadow.extend(overrides.shadow);
    }
}

/// Where the user's override table lives: `$XDG_CONFIG_HOME/tycho/pricing.toml`
/// when the variable is set, else `~/.config/tycho/pricing.toml` (accepted on
/// macOS too, per spec). Pure so it is testable without env mutation.
pub fn user_override_path(xdg_config_home: Option<&str>, home: &Path) -> PathBuf {
    match xdg_config_home {
        Some(xdg) => Path::new(xdg).join("tycho/pricing.toml"),
        None => home.join(".config/tycho/pricing.toml"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dec(s: &str) -> Decimal {
        s.parse().unwrap()
    }

    #[test]
    fn embedded_table_parses_and_covers_observed_models() {
        let table = PricingTable::embedded();
        for model in [
            "claude-fable-5",
            "claude-mythos-5",
            "claude-opus-5",
            "claude-opus-4-8",
            "claude-opus-4-7",
            "claude-sonnet-5",
            "claude-sonnet-4-6",
            "claude-haiku-4-5-20251001", // dated id resolves via prefix
            "gpt-5.6-sol",
            "gpt-5.6-terra",
            "gpt-5.6-luna",
            "gpt-5.5",
            "gpt-5.5-pro",
            "gpt-5.4-mini",
            "gpt-5.3-codex",
            "gpt-5.2-codex",
            "gpt-5.1-codex",
            "gpt-5-codex",
            "chat-latest",
            "codex-auto-review",
            "codex-unknown",
            "gpt-5-chat-latest",
        ] {
            assert!(table.lookup(model).is_some(), "missing rates for {model}");
        }
        let opus = table.lookup("claude-opus-4-8").unwrap();
        assert_eq!(opus.input, dec("5"));
        assert_eq!(opus.output, dec("25"));
        assert_eq!(opus.cache_write_5m, dec("6.25"));
        assert_eq!(opus.cache_write_1h, dec("10"));
        assert_eq!(opus.cache_read, dec("0.5"));
        // A new major version is never a prefix of the prior one, so
        // claude-opus-5 needs its own stanza rather than inheriting 4.8's.
        let opus5 = table.lookup("claude-opus-5").unwrap();
        assert_eq!(opus5.input, dec("5"));
        assert_eq!(opus5.output, dec("25"));
        assert_eq!(opus5.cache_write_5m, dec("6.25"));
        assert_eq!(opus5.cache_write_1h, dec("10"));
        assert_eq!(opus5.cache_read, dec("0.5"));
        let mythos = table.lookup("claude-mythos-5").unwrap();
        assert_eq!(mythos.input, dec("10"));
        assert_eq!(mythos.output, dec("50"));
        assert_eq!(mythos.cache_read, dec("1"));
        let gpt = table.lookup("gpt-5.5").unwrap();
        assert_eq!(gpt.input, dec("5"));
        assert_eq!(gpt.output, dec("30"));
        assert_eq!(gpt.cache_read, dec("0.5"));
        let pro = table.lookup("gpt-5.5-pro").unwrap();
        assert_eq!(pro.input, dec("30"));
        let luna = table.lookup("gpt-5.6-luna").unwrap();
        assert_eq!(luna.input, dec("1"));
        assert_eq!(luna.output, dec("6"));
        assert_eq!(luna.cache_read, dec("0.1"));
        // gpt-5.1-codex-max has no stanza of its own; it resolves via the
        // '-' boundary onto gpt-5.1-codex, which is zero-rated.
        let legacy = table.lookup("gpt-5.1-codex-max").unwrap();
        assert_eq!(legacy.input, dec("0"));
        assert_eq!(legacy.output, dec("0"));
        // Ollama-style name:tag ids have no entry at all; doctor lists
        // these as "local" rather than "unpriced".
        assert!(table.lookup("qwen3.6:27b").is_none());
    }

    #[test]
    fn lookup_requires_a_dash_boundary() {
        let table = PricingTable::embedded();
        assert!(table.lookup("claude-opus-4-8-20270101").is_some()); // dated id, '-' boundary
        assert!(table.lookup("claude-opus-5-20270101").is_some()); // dated id, '-' boundary
        assert!(table.lookup("claude-opus-50").is_none()); // no boundary: must not match claude-opus-5
        assert!(table.lookup("gpt-5.41").is_none()); // no boundary: must not match gpt-5.4
        assert!(table.lookup("gpt-5.1-codex-max").is_some()); // '-' boundary onto gpt-5.1-codex
        assert!(table.lookup("codex-unknown").is_some());
        assert!(table.lookup("gpt-5-chat-latest").is_some());
        assert!(table.lookup("gemma4:12b").is_none()); // stanza deleted; handled as local
    }

    #[test]
    fn shadow_reference_resolves_by_the_same_prefix_rule_as_lookup() {
        let table = PricingTable::embedded();
        assert_eq!(table.shadow_reference("gpt-5-codex"), Some("gpt-5.4"));
        // The '-' boundary applies here too: the -max variant inherits it.
        assert_eq!(table.shadow_reference("gpt-5.1-codex-max"), Some("gpt-5.4"));
        // Priced models need no stand-in.
        assert_eq!(table.shadow_reference("claude-opus-5"), None);
        // codex-unknown is deliberately unmapped: its defining property is
        // that the model is unknown, so no reference rate is justifiable.
        assert_eq!(table.shadow_reference("codex-unknown"), None);
    }

    #[test]
    fn shadow_value_naming_an_unpriced_model_fails_validation() {
        let table = PricingTable::parse(
            r#"
            [models."real-model"]
            input = 1.0
            output = 1.0
            cache_write_5m = 1.0
            cache_write_1h = 1.0
            cache_read = 1.0

            [shadow]
            "legacy" = "does-not-exist"
            "#,
        )
        .unwrap();
        assert!(matches!(
            table.validate(),
            Err(PricingError::UnknownShadowReference { .. })
        ));
    }

    #[test]
    fn embedded_table_validates() {
        PricingTable::embedded().validate().unwrap();
    }

    #[test]
    fn merge_carries_shadow_mappings() {
        let mut base = PricingTable::embedded();
        base.merge(
            PricingTable::parse(
                r#"
                [shadow]
                "gpt-5-codex" = "gpt-5.5"
                "#,
            )
            .unwrap(),
        );
        assert_eq!(base.shadow_reference("gpt-5-codex"), Some("gpt-5.5"));
    }

    #[test]
    fn lookup_prefers_the_longest_prefix() {
        let table = PricingTable::parse(
            r#"
            [models."claude-opus-4"]
            input = 1.0
            output = 1.0
            cache_write_5m = 1.0
            cache_write_1h = 1.0
            cache_read = 1.0

            [models."claude-opus-4-8"]
            input = 5.0
            output = 25.0
            cache_write_5m = 6.25
            cache_write_1h = 10.0
            cache_read = 0.5
            "#,
        )
        .unwrap();
        assert_eq!(
            table.lookup("claude-opus-4-8-20270101").unwrap().input,
            dec("5")
        );
        assert_eq!(table.lookup("claude-opus-4-1").unwrap().input, dec("1"));
    }

    #[test]
    fn lookup_unknown_model_is_none() {
        let table = PricingTable::embedded();
        assert!(table.lookup("gpt-9-mega").is_none());
        assert!(table.lookup("<synthetic>").is_none());
    }

    #[test]
    fn merge_replaces_per_model_and_adds_new() {
        let mut table = PricingTable::embedded();
        let overrides = PricingTable::parse(
            r#"
            [models."claude-opus-4-8"]
            input = 4.0
            output = 20.0
            cache_write_5m = 5.0
            cache_write_1h = 8.0
            cache_read = 0.4

            [models."my-fine-tune"]
            input = 99.0
            output = 99.0
            cache_write_5m = 99.0
            cache_write_1h = 99.0
            cache_read = 99.0
            "#,
        )
        .unwrap();
        table.merge(overrides);
        assert_eq!(table.lookup("claude-opus-4-8").unwrap().input, dec("4"));
        assert_eq!(table.lookup("my-fine-tune").unwrap().input, dec("99"));
        // untouched models keep their defaults
        assert_eq!(table.lookup("claude-haiku-4-5").unwrap().input, dec("1"));
    }

    #[test]
    fn parse_rejects_invalid_toml() {
        assert!(PricingTable::parse("not toml [[[").is_err());
        assert!(PricingTable::parse(r#"[models."x"]"#).is_err()); // missing fields
    }

    #[test]
    fn user_override_path_prefers_xdg_config_home() {
        assert_eq!(
            user_override_path(Some("/xdg"), Path::new("/home/v")),
            PathBuf::from("/xdg/tycho/pricing.toml")
        );
        assert_eq!(
            user_override_path(None, Path::new("/home/v")),
            PathBuf::from("/home/v/.config/tycho/pricing.toml")
        );
    }
}
