//! Troop catalog and army selection: which compositions the account can
//! build, and how many of each troop fit in the camps.

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{Context, Result, bail};
use serde::Deserialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    Tank,
    WallBreaker,
    Air,
    Damage,
    Raider,
    Support,
}

impl Role {
    /// Deploy order: lower goes first.
    pub fn wave(self) -> u8 {
        match self {
            Role::Tank => 0,
            Role::WallBreaker => 1,
            Role::Air => 2,
            Role::Damage => 3,
            Role::Raider => 4,
            Role::Support => 5,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct Troop {
    pub name: String,
    pub housing: u32,
    pub role: Role,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Composition {
    pub name: String,
    /// Troop name -> share of camp space.
    pub troops: BTreeMap<String, f32>,
}

#[derive(Deserialize)]
struct TroopFile {
    troop: Vec<Troop>,
}

#[derive(Deserialize)]
struct ArmyFile {
    army: Vec<Composition>,
}

/// One troop type in a planned army, in editor order.
#[derive(Debug, Clone, PartialEq)]
pub struct ArmyEntry {
    /// Index of the troop's card in the army editor.
    pub card: usize,
    pub name: String,
    pub role: Role,
    pub count: u32,
}

pub struct Catalog {
    /// In army editor order.
    pub troops: Vec<Troop>,
    /// Strongest first.
    pub armies: Vec<Composition>,
}

impl Catalog {
    pub fn load(dir: &Path) -> Result<Self> {
        let read = |name: &str| {
            let path = dir.join(name);
            std::fs::read_to_string(&path).with_context(|| format!("failed to read {}", path.display()))
        };
        let troops: TroopFile = toml::from_str(&read("troops.toml")?).context("invalid troops.toml")?;
        let armies: ArmyFile = toml::from_str(&read("armies.toml")?).context("invalid armies.toml")?;
        let catalog = Self { troops: troops.troop, armies: armies.army };
        for army in &catalog.armies {
            for name in army.troops.keys() {
                if catalog.card(name).is_none() {
                    bail!("army '{}' uses unknown troop '{name}'", army.name);
                }
            }
        }
        Ok(catalog)
    }

    fn card(&self, name: &str) -> Option<usize> {
        self.troops.iter().position(|t| t.name == name)
    }

    pub fn troop_at(&self, card: usize) -> Option<&Troop> {
        self.troops.get(card)
    }

    /// Armies whose troops are all unlocked, strongest first.
    pub fn available(&self, unlocked: &[usize]) -> Vec<&Composition> {
        self.armies
            .iter()
            .filter(|a| a.troops.keys().all(|n| self.card(n).is_some_and(|c| unlocked.contains(&c))))
            .collect()
    }

    /// Turns a composition into troop counts that fit `capacity` camp space.
    pub fn plan(&self, army: &Composition, capacity: u32) -> Vec<ArmyEntry> {
        let mut entries: Vec<ArmyEntry> = army
            .troops
            .iter()
            .map(|(name, share)| {
                let card = self.card(name).expect("validated on load");
                let troop = &self.troops[card];
                let count = (capacity as f32 * share / troop.housing as f32).floor() as u32;
                ArmyEntry { card, name: name.clone(), role: troop.role, count }
            })
            .collect();
        entries.sort_by_key(|e| e.card);

        // Rounding down leaves space; top up with the cheapest troop that fits.
        let used: u32 = entries.iter().map(|e| e.count * self.troops[e.card].housing).sum();
        let mut free = capacity.saturating_sub(used);
        while let Some(e) = entries
            .iter_mut()
            .filter(|e| self.troops[e.card].housing <= free)
            .min_by_key(|e| self.troops[e.card].housing)
        {
            e.count += 1;
            free -= self.troops[e.card].housing;
        }
        entries.retain(|e| e.count > 0);
        entries
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn catalog() -> Catalog {
        Catalog::load(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets")).unwrap()
    }

    #[test]
    fn only_barbarians_when_only_barbarians_unlocked() {
        let c = catalog();
        let names: Vec<_> = c.available(&[0]).iter().map(|a| a.name.as_str()).collect();
        assert_eq!(names, ["Barbarians"]);
    }

    #[test]
    fn plans_fill_the_camps_exactly() {
        let c = catalog();
        let gibarch = c.armies.iter().find(|a| a.name == "GiBarch").unwrap();
        let plan = c.plan(gibarch, 70);
        let used: u32 = plan.iter().map(|e| e.count * c.troops[e.card].housing).sum();
        assert_eq!(used, 70);
        // Editor order: Barbarian, Archer, Giant, Wall Breaker.
        let order: Vec<_> = plan.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(order, ["Barbarian", "Archer", "Giant", "Wall Breaker"]);
        let giants = plan.iter().find(|e| e.name == "Giant").unwrap().count;
        assert_eq!(giants, 4);
    }

    #[test]
    fn more_unlocks_offer_stronger_armies_first() {
        let c = catalog();
        // Barbarian, Archer, Giant, Wall Breaker.
        let names: Vec<_> = c.available(&[0, 1, 2, 4]).iter().map(|a| a.name.as_str()).collect();
        assert_eq!(names, ["GiBarch", "Barch", "Barbarians"]);
    }
}
