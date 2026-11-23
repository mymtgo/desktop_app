//! The rank band (Spark to Mythic) the user held when playing each match,
//! for the metagame page split by rank. Pure: a season is a calendar month,
//! and the caller works out each month in the user's time zone.

use crate::rank_line::{MatchPlace, position};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RankBand {
    /// Constructed only, below Bronze.
    Spark,
    Bronze,
    Silver,
    Gold,
    Platinum,
    Diamond,
    Mythic,
    Unranked,
}

impl RankBand {
    pub const ALL: [RankBand; 8] = [
        RankBand::Spark,
        RankBand::Bronze,
        RankBand::Silver,
        RankBand::Gold,
        RankBand::Platinum,
        RankBand::Diamond,
        RankBand::Mythic,
        RankBand::Unranked,
    ];

    pub fn key(self) -> &'static str {
        match self {
            RankBand::Spark => "spark",
            RankBand::Bronze => "bronze",
            RankBand::Silver => "silver",
            RankBand::Gold => "gold",
            RankBand::Platinum => "platinum",
            RankBand::Diamond => "diamond",
            RankBand::Mythic => "mythic",
            RankBand::Unranked => "unranked",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            RankBand::Spark => "Spark",
            RankBand::Bronze => "Bronze",
            RankBand::Silver => "Silver",
            RankBand::Gold => "Gold",
            RankBand::Platinum => "Platinum",
            RankBand::Diamond => "Diamond",
            RankBand::Mythic => "Mythic",
            RankBand::Unranked => "Unranked",
        }
    }

    pub fn from_key(key: &str) -> Option<RankBand> {
        RankBand::ALL.into_iter().find(|b| b.key() == key)
    }

    /// Arena class text; anything unknown is `Unranked`.
    pub fn from_class(class: &str) -> RankBand {
        match class {
            "Spark" => RankBand::Spark,
            "Bronze" => RankBand::Bronze,
            "Silver" => RankBand::Silver,
            "Gold" => RankBand::Gold,
            "Platinum" => RankBand::Platinum,
            "Diamond" => RankBand::Diamond,
            "Mythic" => RankBand::Mythic,
            _ => RankBand::Unranked,
        }
    }
}

/// A rank reading. `month` is `year * 100 + month` (202611).
#[derive(Debug, Clone, PartialEq)]
pub struct BandReading {
    pub file_id: i64,
    pub offset: u64,
    pub month: u32,
    pub class: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BandMatch {
    pub token: String,
    pub place: Option<MatchPlace>,
    pub month: u32,
}

/// The last reading before the log position `at`.
fn last_before<'a>(readings: &[&'a BandReading], at: (i64, u64)) -> Option<&'a BandReading> {
    readings
        .iter()
        .rev()
        .find(|r| position(r.file_id, r.offset) < at)
        .copied()
}

/// One result per input match, in input order.
pub fn match_bands(readings: &[BandReading], matches: &[BandMatch]) -> Vec<(String, RankBand)> {
    let mut sorted: Vec<&BandReading> = readings.iter().collect();
    sorted.sort_by_key(|r| position(r.file_id, r.offset));

    let mut out = Vec::new();
    for m in matches {
        let same_month: Vec<&BandReading> = sorted
            .iter()
            .copied()
            .filter(|r| r.month == m.month)
            .collect();
        let found = match &m.place {
            None => None,
            Some(MatchPlace::Log { file_id, offset }) => {
                let at = position(*file_id, *offset);
                last_before(&same_month, at).or_else(|| {
                    same_month
                        .iter()
                        .copied()
                        .find(|r| position(r.file_id, r.offset) > at)
                })
            }
            Some(MatchPlace::Claim { file_id, offset }) => {
                let at = position(*file_id, *offset);
                last_before(&same_month, at).or_else(|| {
                    same_month
                        .iter()
                        .copied()
                        .find(|r| position(r.file_id, r.offset) == at)
                })
            }
        };
        let band = found.map_or(RankBand::Unranked, |r| RankBand::from_class(&r.class));
        out.push((m.token.clone(), band));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reading(offset: u64, month: u32, class: &str) -> BandReading {
        BandReading {
            file_id: 1,
            offset,
            month,
            class: class.to_string(),
        }
    }

    fn at_log(token: &str, offset: u64, month: u32) -> BandMatch {
        BandMatch {
            token: token.to_string(),
            place: Some(MatchPlace::Log { file_id: 1, offset }),
            month,
        }
    }

    fn band_of(readings: &[BandReading], m: BandMatch) -> RankBand {
        match_bands(readings, &[m])[0].1
    }

    #[test]
    fn the_band_is_the_reading_before_the_match() {
        let readings = [reading(10, 202611, "Gold"), reading(50, 202611, "Platinum")];
        assert_eq!(band_of(&readings, at_log("a", 30, 202611)), RankBand::Gold);
        assert_eq!(
            band_of(&readings, at_log("a", 70, 202611)),
            RankBand::Platinum
        );
    }

    #[test]
    fn a_match_before_the_months_first_reading_takes_the_next_one() {
        let readings = [reading(10, 202610, "Gold"), reading(50, 202611, "Bronze")];
        assert_eq!(
            band_of(&readings, at_log("a", 30, 202611)),
            RankBand::Bronze
        );
    }

    #[test]
    fn a_match_with_no_reading_that_month_is_unranked() {
        let readings = [reading(10, 202610, "Gold")];
        assert_eq!(
            band_of(&readings, at_log("a", 30, 202611)),
            RankBand::Unranked
        );
    }

    #[test]
    fn a_claimed_manual_match_takes_the_reading_before_its_gap() {
        let readings = [reading(10, 202611, "Gold"), reading(30, 202611, "Platinum")];
        let claim = BandMatch {
            token: "m".to_string(),
            place: Some(MatchPlace::Claim {
                file_id: 1,
                offset: 30,
            }),
            month: 202611,
        };
        assert_eq!(band_of(&readings, claim.clone()), RankBand::Gold);
        let only = [reading(30, 202611, "Platinum")];
        assert_eq!(band_of(&only, claim), RankBand::Platinum);
    }

    #[test]
    fn an_unplaced_match_is_unranked() {
        let readings = [reading(10, 202611, "Gold")];
        let m = BandMatch {
            token: "a".to_string(),
            place: None,
            month: 202611,
        };
        assert_eq!(band_of(&readings, m), RankBand::Unranked);
    }

    #[test]
    fn spark_is_its_own_band_below_bronze() {
        assert_eq!(RankBand::from_class("Spark"), RankBand::Spark);
        assert_eq!(RankBand::ALL[0], RankBand::Spark);
        assert_eq!(RankBand::ALL[1], RankBand::Bronze);
        assert_eq!(RankBand::Spark.key(), "spark");
        assert_eq!(RankBand::Spark.label(), "Spark");
    }

    #[test]
    fn mythic_is_one_band() {
        assert_eq!(RankBand::from_class("Mythic"), RankBand::Mythic);
        assert_eq!(RankBand::from_class("Copper"), RankBand::Unranked);
    }

    #[test]
    fn keys_round_trip() {
        for b in RankBand::ALL {
            assert_eq!(RankBand::from_key(b.key()), Some(b));
        }
        assert_eq!(RankBand::from_key("nope"), None);
    }

    #[test]
    fn readings_in_any_order_give_the_same_band() {
        let readings = [reading(50, 202611, "Platinum"), reading(10, 202611, "Gold")];
        assert_eq!(band_of(&readings, at_log("a", 30, 202611)), RankBand::Gold);
        assert_eq!(
            band_of(&readings, at_log("a", 70, 202611)),
            RankBand::Platinum
        );
    }

    #[test]
    fn a_claim_ignores_last_months_reading() {
        let readings = [reading(10, 202610, "Gold"), reading(30, 202611, "Bronze")];
        let claim = BandMatch {
            token: "m".to_string(),
            place: Some(MatchPlace::Claim {
                file_id: 1,
                offset: 30,
            }),
            month: 202611,
        };
        assert_eq!(band_of(&readings, claim), RankBand::Bronze);
    }
}
