use super::{ContentDomain, Reference};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UnitArchetype {
    pub id: String,
    pub name: String,
    pub faction: String,
    pub max_health: f32,
    pub move_speed: f32,
    /// Id of a a race.
    pub race: String,
    /// Id of a a behavior.
    pub behavior: String,
    /// Whether this unit archetype can be recruited into player control.
    #[serde(default)]
    pub recruitable: bool,
}

impl ContentDomain for UnitArchetype {
    const NAME: &'static str = "units";
    const KIND: &'static str = "unit";

    fn id(&self) -> &str {
        &self.id
    }

    fn validate(&self) -> Result<(), String> {
        if self.max_health <= 0.0 || self.move_speed < 0.0 {
            return Err(format!("unit '{}' has invalid stats", self.id));
        }
        Ok(())
    }

    fn references(&self) -> Vec<Reference<'_>> {
        vec![
            Reference::new("faction", &self.faction),
            Reference::new("race", &self.race),
            Reference::new("behavior", &self.behavior),
        ]
    }
}
