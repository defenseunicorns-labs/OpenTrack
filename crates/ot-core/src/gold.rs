//! OTH-GOLD (OS-OTG Rev C) mandatory track fields.
//!
//! A GOLD contact report must carry, at minimum: the track number, the
//! class-name (`UNEQUATED-UNKNOWN` when neither is known), the force code and
//! the track type (CTC fields 1, 2, 11 and 13), and a position with its
//! date-time group (POS fields 1-4). OpenTrack's published message carries
//! exactly these, plus the admin-defined attributes.

use crate::schema::{Affiliation, Domain};

/// Class placeholder when the class is not known (CTC field 2).
pub const UNEQUATED: &str = "UNEQUATED";
/// Name placeholder when the name is not known (CTC field 2).
pub const UNKNOWN: &str = "UNKNOWN";

/// GOLD force code position: the first half of a force code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ForcePosition {
    Air,
    Sub,
    Surface,
    Land,
    Unknown,
}

/// GOLD threat identity: the second half of a force code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Threat {
    Friend,
    AssumedFriend,
    Neutral,
    Pending,
    Unknown,
    Suspect,
    Hostile,
}

fn position(domain: Option<Domain>) -> ForcePosition {
    match domain {
        Some(Domain::Air) | Some(Domain::Space) => ForcePosition::Air,
        Some(Domain::Surface) => ForcePosition::Surface,
        Some(Domain::Subsurface) => ForcePosition::Sub,
        Some(Domain::Ground) => ForcePosition::Land,
        None => ForcePosition::Unknown,
    }
}

fn threat(affiliation: Option<Affiliation>) -> Threat {
    match affiliation {
        Some(Affiliation::Friend) => Threat::Friend,
        Some(Affiliation::AssumedFriend) => Threat::AssumedFriend,
        Some(Affiliation::Neutral) => Threat::Neutral,
        Some(Affiliation::Pending) => Threat::Pending,
        // Joker and faker are exercise suspect and hostile (MIL-STD-2525).
        Some(Affiliation::Suspect) | Some(Affiliation::Joker) => Threat::Suspect,
        Some(Affiliation::Hostile) | Some(Affiliation::Faker) => Threat::Hostile,
        Some(Affiliation::Unknown) | Some(Affiliation::None) | None => Threat::Unknown,
    }
}

/// GOLD force code (Table 5-1) for a domain and affiliation, 0-39.
pub fn force_code(domain: Option<Domain>, affiliation: Option<Affiliation>) -> u8 {
    use ForcePosition as P;
    use Threat as T;
    match (position(domain), threat(affiliation)) {
        (P::Unknown, T::Pending) => 0,
        (P::Air, T::Hostile) => 1,
        (P::Air, T::Pending) => 2,
        (P::Air, T::Friend) => 3,
        (P::Sub, T::Hostile) => 4,
        (P::Sub, T::Pending) => 5,
        (P::Sub, T::Friend) => 6,
        (P::Surface, T::Hostile) => 7,
        (P::Surface, T::Pending) => 8,
        (P::Surface, T::Friend) => 9,
        (P::Air, T::AssumedFriend) => 10,
        (P::Air, T::Suspect) => 11,
        (P::Air, T::Neutral) => 12,
        (P::Sub, T::AssumedFriend) => 13,
        (P::Sub, T::Suspect) => 14,
        (P::Sub, T::Neutral) => 15,
        (P::Surface, T::AssumedFriend) => 16,
        (P::Surface, T::Suspect) => 17,
        (P::Surface, T::Neutral) => 18,
        (P::Unknown, T::Suspect) => 19,
        (P::Unknown, T::AssumedFriend) => 20,
        (P::Unknown, T::Neutral) => 21,
        (P::Land, T::Friend) => 22,
        (P::Land, T::AssumedFriend) => 23,
        (P::Land, T::Hostile) => 24,
        (P::Land, T::Suspect) => 25,
        (P::Land, T::Pending) => 26,
        (P::Land, T::Neutral) => 27,
        (P::Air, T::Unknown) => 28,
        (P::Sub, T::Unknown) => 29,
        (P::Surface, T::Unknown) => 30,
        (P::Land, T::Unknown) => 31,
        (P::Unknown, T::Unknown) => 32,
        (P::Unknown, T::Hostile) => 38,
        (P::Unknown, T::Friend) => 39,
    }
}

/// GOLD code of a track type (CTC field 13): `None` for tactical, which GOLD
/// sends as a null entry.
pub fn track_type_code(t: crate::schema::TrackType) -> Option<u8> {
    use crate::schema::TrackType;
    match t {
        TrackType::Tactical => None,
        TrackType::LiveTraining => Some(2),
        TrackType::SimulatedTraining => Some(3),
        TrackType::DemandEntry => Some(4),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn force_codes_follow_table_5_1() {
        assert_eq!(
            force_code(Some(Domain::Surface), Some(Affiliation::Friend)),
            9
        );
        assert_eq!(
            force_code(Some(Domain::Surface), Some(Affiliation::Pending)),
            8
        );
        assert_eq!(force_code(Some(Domain::Air), Some(Affiliation::Hostile)), 1);
        assert_eq!(
            force_code(Some(Domain::Subsurface), Some(Affiliation::Suspect)),
            14
        );
        assert_eq!(
            force_code(Some(Domain::Ground), Some(Affiliation::Neutral)),
            27
        );
        assert_eq!(force_code(None, None), 32);
        assert_eq!(force_code(None, Some(Affiliation::Friend)), 39);
        assert_eq!(
            force_code(Some(Domain::Space), Some(Affiliation::Joker)),
            11
        );
        // Every combination has a code, and codes 33-37 (unused) never appear.
        let domains = [
            None,
            Some(Domain::Air),
            Some(Domain::Surface),
            Some(Domain::Subsurface),
            Some(Domain::Ground),
            Some(Domain::Space),
        ];
        let affs = [
            None,
            Some(Affiliation::Pending),
            Some(Affiliation::Unknown),
            Some(Affiliation::AssumedFriend),
            Some(Affiliation::Friend),
            Some(Affiliation::Neutral),
            Some(Affiliation::Suspect),
            Some(Affiliation::Hostile),
            Some(Affiliation::Joker),
            Some(Affiliation::Faker),
            Some(Affiliation::None),
        ];
        for d in domains {
            for a in affs {
                let c = force_code(d, a);
                assert!(c <= 39 && !(33..=37).contains(&c), "{d:?} {a:?} -> {c}");
            }
        }
    }
}
