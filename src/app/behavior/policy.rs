use crate::types::{FilterLevel, Restriction};

pub const SEARCH_FLOOD: usize = 30;
pub const QUEUE_FLOOD: usize = 100;
pub const PRESET_STATS: &[(u32, u32)] = &[
    (1, 1),
    (1, 499),
    (500, 25),
    (1000, 50),
    (1500, 75),
    (2000, 100),
];
pub const CONTRADICTION_MIN_FILES: u32 = 50;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub enum Verdict {
    #[default]
    Clean,
    Verified,
    Suspect,
    Leech,
}

impl Verdict {
    pub fn as_str(self) -> &'static str {
        match self {
            Verdict::Clean => "clean",
            Verdict::Verified => "verified",
            Verdict::Suspect => "suspect",
            Verdict::Leech => "leech",
        }
    }

    pub fn from_str(value: &str) -> Self {
        match value {
            "clean" => Verdict::Clean,
            "verified" => Verdict::Verified,
            "suspect" => Verdict::Suspect,
            "leech" => Verdict::Leech,
            other => panic!("unknown verdict {other}"),
        }
    }
}

pub fn restriction_for(level: FilterLevel, verdict: Verdict, denied_message: &str) -> Restriction {
    match level {
        FilterLevel::Open => Restriction::None,
        FilterLevel::Guarded => match verdict {
            Verdict::Leech => Restriction::Denied {
                reason: denied_message.to_owned(),
            },
            Verdict::Suspect => Restriction::Deprioritized,
            Verdict::Clean | Verdict::Verified => Restriction::None,
        },
        FilterLevel::Strict => match verdict {
            Verdict::Leech | Verdict::Suspect => Restriction::Denied {
                reason: denied_message.to_owned(),
            },
            Verdict::Clean | Verdict::Verified => Restriction::None,
        },
    }
}

pub fn restriction_str(restriction: &Restriction) -> &'static str {
    match restriction {
        Restriction::None => "none",
        Restriction::Deprioritized => "deprioritized",
        Restriction::Hold => "hold",
        Restriction::Denied { .. } => "denied",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn levels_map_verdicts_to_restrictions() {
        assert!(Verdict::Clean < Verdict::Verified);
        assert!(Verdict::Verified < Verdict::Suspect);
        assert!(Verdict::Suspect < Verdict::Leech);
        assert_eq!(
            restriction_for(FilterLevel::Open, Verdict::Leech, "m"),
            Restriction::None
        );
        assert_eq!(
            restriction_for(FilterLevel::Guarded, Verdict::Suspect, "m"),
            Restriction::Deprioritized
        );
        assert_eq!(
            restriction_for(FilterLevel::Guarded, Verdict::Leech, "m"),
            Restriction::Denied { reason: "m".into() }
        );
        assert_eq!(
            restriction_for(FilterLevel::Strict, Verdict::Suspect, "m"),
            Restriction::Denied { reason: "m".into() }
        );
        assert_eq!(
            restriction_for(FilterLevel::Strict, Verdict::Verified, "m"),
            Restriction::None
        );
    }

    #[test]
    fn verdict_round_trips_through_storage() {
        for verdict in [
            Verdict::Clean,
            Verdict::Verified,
            Verdict::Suspect,
            Verdict::Leech,
        ] {
            assert_eq!(Verdict::from_str(verdict.as_str()), verdict);
        }
    }
}
