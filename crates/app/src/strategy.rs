//! Deploy strategies, and a learner that picks the one that scores the highest
//! damage based on past attacks (epsilon-greedy over a local log).

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;

use anyhow::Result;
use serde::{Deserialize, Serialize};
use vision::deploy::{DeployPoint, DeployZone};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Strategy {
    /// Everything on the quarter of the border closest to the buildings.
    OneSide,
    /// Two opposite quarters.
    TwoSides,
    /// Four single points, a quarter turn apart.
    FourCorners,
    /// Spread evenly around the whole border.
    Surround,
}

impl Strategy {
    pub const ALL: [Strategy; 4] = [
        Strategy::OneSide,
        Strategy::TwoSides,
        Strategy::FourCorners,
        Strategy::Surround,
    ];

    /// Picks the deploy points to use, in the order troops should go down.
    pub fn points(self, zone: &DeployZone) -> Vec<(u32, u32)> {
        let pts = &zone.points;
        let n = pts.len();
        let quarter = (n / 4).max(1);
        let arc = |start: usize| (0..quarter).map(move |k| pts[(start + k) % n].pos);
        let best = best_arc_start(pts, quarter);
        match self {
            Strategy::OneSide => arc(best).collect(),
            Strategy::TwoSides => arc(best).chain(arc(best + n / 2)).collect(),
            Strategy::FourCorners => (0..4).map(|k| pts[(best + quarter / 2 + k * n / 4) % n].pos).collect(),
            Strategy::Surround => pts.iter().map(|p| p.pos).collect(),
        }
    }

    /// Where the reserve goes if the first push stalls: the quarter of the
    /// border next to the main attack, so it sweeps around the base.
    pub fn reserve_points(self, zone: &DeployZone) -> Vec<(u32, u32)> {
        let pts = &zone.points;
        let n = pts.len();
        let quarter = (n / 4).max(1);
        let start = best_arc_start(pts, quarter) + quarter;
        (0..quarter).map(|k| pts[(start + k) % n].pos).collect()
    }
}

/// Start of the run of `len` consecutive points where the border is closest
/// to the centre on average: buildings there are reached soonest.
fn best_arc_start(pts: &[DeployPoint], len: usize) -> usize {
    (0..pts.len())
        .min_by(|&a, &b| {
            let avg = |s: usize| (0..len).map(|k| pts[(s + k) % pts.len()].border_distance).sum::<f32>();
            avg(a).total_cmp(&avg(b))
        })
        .unwrap_or(0)
}

#[derive(Debug, Serialize, Deserialize)]
pub struct AttackRecord {
    pub unix_time: u64,
    /// Army composition name; records from before armies were tracked
    /// were all Barbarians.
    #[serde(default = "default_army")]
    pub army: String,
    pub strategy: Strategy,
    pub gold: u32,
    pub elixir: u32,
    pub damage: u32,
}

fn default_army() -> String {
    "Barbarians".into()
}

/// Remembers how each army and strategy performed and chooses the next ones.
pub struct Learner {
    log: PathBuf,
    records: Vec<AttackRecord>,
    explore: f64,
}

impl Learner {
    pub fn load(log: PathBuf, explore: f64) -> Result<Self> {
        let records = match fs::read_to_string(&log) {
            Ok(text) => text.lines().filter_map(|l| serde_json::from_str(l).ok()).collect(),
            Err(_) => Vec::new(),
        };
        Ok(Self { log, records, explore })
    }

    /// Mean damage and attack count for each option, grouping records by `key`.
    pub fn stats<T: Clone + PartialEq>(
        &self,
        options: &[T],
        key: impl Fn(&AttackRecord) -> T,
    ) -> Vec<(T, f64, usize)> {
        options
            .iter()
            .map(|o| {
                let damages: Vec<u32> =
                    self.records.iter().filter(|r| key(r) == *o).map(|r| r.damage).collect();
                let mean = if damages.is_empty() {
                    0.0
                } else {
                    damages.iter().sum::<u32>() as f64 / damages.len() as f64
                };
                (o.clone(), mean, damages.len())
            })
            .collect()
    }

    /// Tries every option once, then mostly uses the best one while
    /// occasionally exploring the others.
    fn pick<T: Clone + PartialEq>(&self, options: &[T], key: impl Fn(&AttackRecord) -> T) -> T {
        let stats = self.stats(options, key);
        if let Some((o, _, _)) = stats.iter().find(|(_, _, n)| *n == 0) {
            return o.clone();
        }
        if fastrand::f64() < self.explore {
            return options[fastrand::usize(..options.len())].clone();
        }
        stats.into_iter().max_by(|a, b| a.1.total_cmp(&b.1)).map(|s| s.0).unwrap()
    }

    pub fn choose_strategy(&self) -> Strategy {
        self.pick(&Strategy::ALL, |r| r.strategy)
    }

    /// Chooses among army names (the strongest few the account can build).
    pub fn choose_army(&self, names: &[String]) -> String {
        self.pick(names, |r| r.army.clone())
    }

    pub fn strategy_stats(&self) -> Vec<(Strategy, f64, usize)> {
        self.stats(&Strategy::ALL, |r| r.strategy)
    }

    /// Stats for every army that has been used at least once.
    pub fn army_stats(&self) -> Vec<(String, f64, usize)> {
        let mut names: Vec<String> = self.records.iter().map(|r| r.army.clone()).collect();
        names.sort();
        names.dedup();
        self.stats(&names, |r| r.army.clone())
    }

    pub fn record(&mut self, record: AttackRecord) -> Result<()> {
        if let Some(dir) = self.log.parent() {
            fs::create_dir_all(dir)?;
        }
        let mut file = OpenOptions::new().create(true).append(true).open(&self.log)?;
        writeln!(file, "{}", serde_json::to_string(&record)?)?;
        self.records.push(record);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn zone(distances: &[f32]) -> DeployZone {
        let n = distances.len();
        DeployZone {
            center: (0.0, 0.0),
            points: distances
                .iter()
                .enumerate()
                .map(|(i, &d)| DeployPoint {
                    pos: (i as u32, 0),
                    angle: i as f32 / n as f32 * std::f32::consts::TAU,
                    border_distance: d,
                })
                .collect(),
        }
    }

    #[test]
    fn one_side_picks_the_closest_arc() {
        let z = zone(&[9.0, 9.0, 9.0, 9.0, 1.0, 1.0, 9.0, 9.0]);
        assert_eq!(Strategy::OneSide.points(&z), vec![(4, 0), (5, 0)]);
        assert_eq!(Strategy::TwoSides.points(&z), vec![(4, 0), (5, 0), (0, 0), (1, 0)]);
        assert_eq!(Strategy::Surround.points(&z).len(), 8);
        assert_eq!(Strategy::FourCorners.points(&z).len(), 4);
    }

    fn rec(army: &str, strategy: Strategy, damage: u32) -> AttackRecord {
        AttackRecord { unix_time: 0, army: army.into(), strategy, gold: 0, elixir: 0, damage }
    }

    #[test]
    fn untried_options_come_first_then_the_best() {
        let mut l = Learner { log: PathBuf::new(), records: Vec::new(), explore: 0.0 };
        for s in [Strategy::OneSide, Strategy::TwoSides, Strategy::FourCorners] {
            l.records.push(rec("Barch", s, 50));
        }
        assert_eq!(l.choose_strategy(), Strategy::Surround);
        l.records.push(rec("Barch", Strategy::Surround, 90));
        assert_eq!(l.choose_strategy(), Strategy::Surround);

        let armies = ["Barch".to_string(), "GiBarch".to_string()];
        assert_eq!(l.choose_army(&armies), "GiBarch");
        l.records.push(rec("GiBarch", Strategy::OneSide, 95));
        assert_eq!(l.choose_army(&armies), "GiBarch");
    }

    #[test]
    fn old_records_without_army_count_as_barbarians() {
        let r: AttackRecord = serde_json::from_str(
            r#"{"unix_time":1,"strategy":"OneSide","gold":0,"elixir":0,"damage":100}"#,
        )
        .unwrap();
        assert_eq!(r.army, "Barbarians");
    }
}
