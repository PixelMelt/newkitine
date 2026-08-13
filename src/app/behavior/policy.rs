use crate::types::{DenialMessages, FilterLevel, Restriction};

pub const SWEEP_SECS: u64 = 900;
pub const SEARCH_RATE_PER_DAY: u32 = 500;
pub const MIN_OBSERVATION_DAYS: i64 = 7;
pub const REPEAT_DOWNLOAD_LIMIT: u32 = 3;
pub const REPEAT_WINDOW_DAYS: i64 = 14;

pub const SECS_PER_DAY: i64 = 86_400;
pub const SEARCH_FLOOR: u32 = SEARCH_RATE_PER_DAY * MIN_OBSERVATION_DAYS as u32;

pub struct PeerCounters {
    pub searches: u32,
    pub queue_requests: u32,
    pub browses: u32,
    pub window_secs: i64,
}

pub fn is_search_scraper(counters: &PeerCounters) -> bool {
    if counters.queue_requests > 0 || counters.browses > 0 {
        return false;
    }
    if counters.window_secs < MIN_OBSERVATION_DAYS * SECS_PER_DAY {
        return false;
    }
    u64::from(counters.searches) * SECS_PER_DAY as u64
        >= u64::from(SEARCH_RATE_PER_DAY) * counters.window_secs as u64
}
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
    Leech,
    Abusive,
}

impl Verdict {
    pub fn as_str(self) -> &'static str {
        match self {
            Verdict::Clean => "clean",
            Verdict::Verified => "verified",
            Verdict::Leech => "leech",
            Verdict::Abusive => "abusive",
        }
    }

    pub fn from_str(value: &str) -> Self {
        match value {
            "clean" => Verdict::Clean,
            "verified" => Verdict::Verified,
            "leech" => Verdict::Leech,
            "abusive" => Verdict::Abusive,
            other => panic!("unknown verdict {other}"),
        }
    }
}

pub fn restriction_for(
    level: FilterLevel,
    verdict: Verdict,
    messages: &DenialMessages,
) -> Restriction {
    let denied = |reason: &str| Restriction::Denied {
        reason: reason.to_owned(),
    };
    match (level, verdict) {
        (FilterLevel::Open, _) => Restriction::None,
        (_, Verdict::Abusive) => denied(&messages.abusive),
        (FilterLevel::Strict, Verdict::Leech) => denied(&messages.leech),
        _ => Restriction::None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn messages() -> DenialMessages {
        DenialMessages {
            abusive: "stop flooding".into(),
            leech: "share something".into(),
        }
    }

    fn days(count: f64) -> i64 {
        (count * SECS_PER_DAY as f64) as i64
    }

    fn peer(searches: u32, queue_requests: u32, browses: u32, window_secs: i64) -> PeerCounters {
        PeerCounters {
            searches,
            queue_requests,
            browses,
            window_secs,
        }
    }

    fn scrapes(searches: u32, window_days: f64) -> bool {
        is_search_scraper(&peer(searches, 0, 0, days(window_days)))
    }

    #[test]
    fn sustained_searchers_with_no_transfer_intent_are_scrapers() {
        assert!(scrapes(33_152, 32.8));
        assert!(scrapes(38_655, 32.6));
        assert!(scrapes(23_322, 18.0));
        assert!(scrapes(132_023, 29.0));
        assert!(scrapes(127_763, 13.1));
    }

    #[test]
    fn any_transfer_intent_spares_a_peer_whatever_the_rate() {
        assert!(!is_search_scraper(&peer(3_632, 33, 0, days(32.8))));
        assert!(!is_search_scraper(&peer(1_306, 7, 1, days(32.8))));
        assert!(!is_search_scraper(&peer(34, 182, 3, days(31.7))));
        assert!(!is_search_scraper(&peer(202_330, 588, 0, days(32.2))));
        assert!(!is_search_scraper(&peer(72_702, 2_259, 0, days(32.8))));
    }

    #[test]
    fn short_windows_never_convict() {
        assert!(!scrapes(6_384, 2.9));
        let minimum = MIN_OBSERVATION_DAYS * SECS_PER_DAY;
        assert!(!is_search_scraper(&peer(1_000_000, 0, 0, minimum - 1)));
        assert!(is_search_scraper(&peer(SEARCH_FLOOR, 0, 0, minimum)));
    }

    #[test]
    fn ordinary_long_lived_users_stay_clean() {
        assert!(!scrapes(2_192, 32.6));
        assert!(!scrapes(977, 23.3));
        assert!(!scrapes(1_306, 32.8));
        assert!(!scrapes(2_447, 32.4));
    }

    #[test]
    fn the_sql_floor_never_hides_a_scraper() {
        let minimum = MIN_OBSERVATION_DAYS * SECS_PER_DAY;
        assert!(!is_search_scraper(&peer(SEARCH_FLOOR - 1, 0, 0, minimum)));
    }

    #[test]
    fn abuse_outranks_leeching() {
        assert!(Verdict::Clean < Verdict::Verified);
        assert!(Verdict::Verified < Verdict::Leech);
        assert!(Verdict::Leech < Verdict::Abusive);
    }

    #[test]
    fn levels_map_verdicts_to_restrictions() {
        let none = Restriction::None;
        let abusive = Restriction::Denied {
            reason: "stop flooding".into(),
        };
        let leech = Restriction::Denied {
            reason: "share something".into(),
        };
        let cases = [
            (FilterLevel::Open, Verdict::Clean, &none),
            (FilterLevel::Open, Verdict::Verified, &none),
            (FilterLevel::Open, Verdict::Leech, &none),
            (FilterLevel::Open, Verdict::Abusive, &none),
            (FilterLevel::Guarded, Verdict::Clean, &none),
            (FilterLevel::Guarded, Verdict::Verified, &none),
            (FilterLevel::Guarded, Verdict::Leech, &none),
            (FilterLevel::Guarded, Verdict::Abusive, &abusive),
            (FilterLevel::Strict, Verdict::Clean, &none),
            (FilterLevel::Strict, Verdict::Verified, &none),
            (FilterLevel::Strict, Verdict::Leech, &leech),
            (FilterLevel::Strict, Verdict::Abusive, &abusive),
        ];
        for (level, verdict, expected) in cases {
            assert_eq!(
                &restriction_for(level, verdict, &messages()),
                expected,
                "{level:?} {verdict:?}"
            );
        }
    }

    #[test]
    fn verdict_round_trips_through_storage() {
        for verdict in [
            Verdict::Clean,
            Verdict::Verified,
            Verdict::Leech,
            Verdict::Abusive,
        ] {
            assert_eq!(Verdict::from_str(verdict.as_str()), verdict);
        }
    }
}
