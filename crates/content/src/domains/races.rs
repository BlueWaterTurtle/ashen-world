use super::ContentDomain;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// A modification to a named stat: `(base + flat) * (1 + percent)`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StatModifier {
    pub stat: String,
    #[serde(default)]
    pub flat: f32,
    #[serde(default)]
    pub percent: f32,
}

/// Apply modifiers for `stat` to `base`. Flat bonuses sum first, then percentages sum.
pub fn effective_stat(stat: &str, base: f32, modifiers: &[StatModifier]) -> f32 {
    let (flat, pct) = modifiers
        .iter()
        .filter(|m| m.stat == stat)
        .fold((0.0, 0.0), |(f, p), m| (f + m.flat, p + m.percent));
    (base + flat) * (1.0 + pct)
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Race {
    pub id: String,
    pub name: String,
    /// Base stat values keyed by stat name (e.g. "strength").
    pub base_stats: BTreeMap<String, f32>,
    /// Racial buffs and debuffs.
    #[serde(default)]
    pub modifiers: Vec<StatModifier>,
}

impl Race {
    /// Effective value of `stat` (base plus racial modifiers); 0 base if undefined.
    pub fn stat(&self, stat: &str) -> f32 {
        let base = self.base_stats.get(stat).copied().unwrap_or(0.0);
        effective_stat(stat, base, &self.modifiers)
    }
}

impl ContentDomain for Race {
    const NAME: &'static str = "races";
    const KIND: &'static str = "race";

    fn id(&self) -> &str {
        &self.id
    }

    fn validate(&self) -> Result<(), String> {
        for m in &self.modifiers {
            if !self.base_stats.contains_key(&m.stat) {
                return Err(format!(
                    "race '{}' modifies undefined stat '{}'",
                    self.id, m.stat
                ));
            }
        }
        Ok(())
    }
}
