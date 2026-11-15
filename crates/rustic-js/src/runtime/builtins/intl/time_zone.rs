//! IANA time-zone identifiers and UTC offsets. The bundled data is
//! generated from the 64-bit TZif transitions and recurring footer rules.

use std::sync::OnceLock;

use super::super::date::{civil_from_days, days_from_civil, days_in_month};
use super::pack::{NONE, Pack, Table};

static PACK: Pack = Pack::new(include_bytes!("../../../../data/intl/time-zones.bin"));
const DAY_SECONDS: i64 = 86_400;

#[derive(Clone, Copy)]
pub(super) enum Zone {
    Named(u32),
    Fixed(i32),
}

pub(super) struct Offset {
    pub(super) seconds: i64,
    pub(super) abbreviation: &'static str,
}

struct Data {
    identifiers: Table,
    primary: Table,
    zones: Table,
    types: Table,
    transitions: Table,
    future: Table,
    rules: Table,
}

fn data() -> &'static Data {
    static DATA: OnceLock<Data> = OnceLock::new();
    DATA.get_or_init(|| Data {
        identifiers: PACK.table("ids"),
        primary: PACK.table("primary"),
        zones: PACK.table("zones"),
        types: PACK.table("types"),
        transitions: PACK.table("transitions"),
        future: PACK.table("future"),
        rules: PACK.table("rules"),
    })
}

pub(super) fn primary_identifiers() -> impl Iterator<Item = &'static str> {
    let table = &data().primary;
    (0..table.rows()).map(|row| table.text(row, 0))
}

impl Zone {
    pub(super) fn parse(name: &str) -> Option<Self> {
        if name.len() > 64 || !name.is_ascii() {
            return None;
        }
        if matches!(name.as_bytes().first(), Some(b'+' | b'-')) {
            return offset_minutes(name).map(Self::Fixed);
        }
        data()
            .identifiers
            .find_text(&name.to_ascii_lowercase())
            .map(Self::Named)
    }

    pub(super) fn name(self) -> String {
        match self {
            Self::Named(id) => data().identifiers.text(id, 1).to_owned(),
            Self::Fixed(minutes) => {
                let sign = if minutes < 0 { '-' } else { '+' };
                let minutes = i64::from(minutes).abs();
                format!("{sign}{:02}:{:02}", minutes / 60, minutes % 60)
            }
        }
    }

    pub(super) fn offset_at(self, epoch_seconds: i64) -> Offset {
        match self {
            Self::Named(id) => data().at(data().identifiers.get(id, 2), epoch_seconds),
            Self::Fixed(minutes) => Offset {
                seconds: i64::from(minutes) * 60,
                abbreviation: "",
            },
        }
    }
}

fn offset_minutes(text: &str) -> Option<i32> {
    fn decimal(first: u8, second: u8) -> Option<i32> {
        if !first.is_ascii_digit() || !second.is_ascii_digit() {
            return None;
        }
        Some(i32::from(first - b'0') * 10 + i32::from(second - b'0'))
    }

    let (&sign, digits) = text.as_bytes().split_first()?;
    let (hour, minute) = match *digits {
        [a, b] => (decimal(a, b)?, 0),
        [a, b, c, d] | [a, b, b':', c, d] => (decimal(a, b)?, decimal(c, d)?),
        _ => return None,
    };
    if hour > 23 || minute > 59 {
        return None;
    }
    let value = hour * 60 + minute;
    match sign {
        b'+' => Some(value),
        b'-' => Some(-value),
        _ => None,
    }
}

impl Data {
    fn transition_at(&self, row: u32) -> i64 {
        let lo = u64::from(self.transitions.get(row, 0));
        let hi = u64::from(self.transitions.get(row, 1));
        (lo | hi << 32) as i64
    }

    fn offset(&self, kind: u32) -> Offset {
        Offset {
            seconds: i64::from(self.types.get(kind, 0) as i32),
            abbreviation: self.types.text(kind, 1),
        }
    }

    fn at(&self, zone: u32, seconds: i64) -> Offset {
        let first = self.zones.get(zone, 1);
        let count = self.zones.get(zone, 2);
        let future = self.zones.get(zone, 3);
        let end = first + count;
        if future != NONE && (count == 0 || self.transition_at(end - 1) < seconds) {
            return self.future_offset(future, seconds);
        }
        let mut low = first;
        let mut high = end;
        while low < high {
            let middle = low + (high - low) / 2;
            if self.transition_at(middle) <= seconds {
                low = middle + 1;
            } else {
                high = middle;
            }
        }
        let kind = if low == first {
            self.zones.get(zone, 0)
        } else {
            self.transitions.get(low - 1, 2)
        };
        self.offset(kind)
    }

    fn future_offset(&self, row: u32, seconds: i64) -> Offset {
        let standard = self.offset(self.future.get(row, 0));
        let daylight = self.future.get(row, 1);
        if daylight == NONE {
            return standard;
        }
        let daylight = self.offset(daylight);
        let start = self.future.get(row, 2);
        let end = self.future.get(row, 3);
        let (year, ..) = civil_from_days(seconds.div_euclid(DAY_SECONDS));
        // POSIX transition times are local wall times before the change and
        // may extend into neighbouring days, even across a year boundary.
        let mut latest = (i64::MIN, false);
        for year in year - 1..=year + 1 {
            for (rule, prior, is_dst) in [
                (start, standard.seconds, true),
                (end, daylight.seconds, false),
            ] {
                let time = self.rule_in_year(rule, year) - prior;
                if time <= seconds && time > latest.0 {
                    latest = (time, is_dst);
                }
            }
        }
        if latest.1 { daylight } else { standard }
    }

    fn rule_in_year(&self, rule: u32, year: i64) -> i64 {
        let value = i64::from(self.rules.get(rule, 1));
        let days = match self.rules.get(rule, 0) {
            0 => {
                let first = days_from_civil(year, value, 1);
                let weekday = i64::from(self.rules.get(rule, 3));
                let week = i64::from(self.rules.get(rule, 2));
                let first_weekday = (first + 4).rem_euclid(7);
                let mut day = 1 + (weekday - first_weekday).rem_euclid(7) + (week - 1) * 7;
                if day > days_in_month(year, value) {
                    day -= 7;
                }
                first + day - 1
            }
            1 => {
                // Julian dates start at one and omit February 29.
                let leap = value >= 60 && days_in_month(year, 2) == 29;
                days_from_civil(year, 1, 1) + value - 1 + i64::from(leap)
            }
            _ => days_from_civil(year, 1, 1) + value,
        };
        days * DAY_SECONDS + i64::from(self.rules.get(rule, 4) as i32)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn epoch(year: i64, month: i64, day: i64, hour: i64) -> i64 {
        days_from_civil(year, month, day) * DAY_SECONDS + hour * 3600
    }

    #[test]
    fn named_zones_use_transitions_and_recur_after_the_last_one() {
        let ny = Zone::parse("america/new_york").unwrap();
        assert_eq!(ny.name(), "America/New_York");
        let transition = epoch(2025, 3, 9, 7);
        assert_eq!(ny.offset_at(transition - 1).seconds, -18_000);
        assert_eq!(ny.offset_at(transition).abbreviation, "EDT");
        assert_eq!(ny.offset_at(epoch(2405, 1, 1, 0)).seconds, -18_000);
        assert_eq!(ny.offset_at(epoch(2405, 7, 1, 0)).seconds, -14_400);

        let sydney = Zone::parse("Australia/Sydney").unwrap();
        assert_eq!(sydney.offset_at(epoch(2405, 1, 1, 0)).seconds, 39_600);
        assert_eq!(sydney.offset_at(epoch(2405, 7, 1, 0)).seconds, 36_000);
        let kathmandu = Zone::parse("Asia/Kathmandu").unwrap();
        assert_eq!(kathmandu.offset_at(epoch(2025, 1, 1, 0)).seconds, 20_700);
        assert_eq!(Zone::parse("UTC").unwrap().offset_at(-1).seconds, 0);
    }

    #[test]
    fn fixed_offsets_and_aliases_keep_the_requested_identifier() {
        for (requested, name, offset) in [
            ("-03", "-03:00", -10_800),
            ("+0545", "+05:45", 20_700),
            ("-00:00", "+00:00", 0),
            ("uS/eAsTeRn", "US/Eastern", -18_000),
        ] {
            let zone = Zone::parse(requested).unwrap();
            assert_eq!(zone.name(), name);
            assert_eq!(zone.offset_at(0).seconds, offset);
        }
        for invalid in [
            "−05",
            "-5",
            "+24:00",
            "+005",
            "+01:60",
            "GMT+01:00",
            "+01:00:00",
        ] {
            assert!(Zone::parse(invalid).is_none(), "{invalid}");
        }
        let primary = primary_identifiers().collect::<Vec<_>>();
        assert!(primary.windows(2).all(|pair| pair[0] < pair[1]));
        assert!(primary.contains(&"Europe/Bratislava"));
        assert!(!primary.contains(&"US/Eastern"));
    }
}
