//! Decimal digit options and rounding for ECMA-402 plural operands.

use super::super::matching;
use crate::runtime::{Agent, ErrorKind, JsResult, ObjectId, Value};
use std::cmp::Ordering;

pub(super) const MODES: &[&str] = &[
    "halfExpand",
    "ceil",
    "floor",
    "expand",
    "trunc",
    "halfCeil",
    "halfFloor",
    "halfTrunc",
    "halfEven",
];
pub(super) const PRIORITIES: &[&str] = &["auto", "morePrecision", "lessPrecision"];
pub(super) const TRAILING: &[&str] = &["auto", "stripIfInteger"];
const INCREMENTS: &[i32] = &[
    1, 2, 5, 10, 20, 25, 50, 100, 200, 250, 500, 1000, 2000, 2500, 5000,
];

#[derive(Clone, Copy, Debug)]
pub(super) struct Options {
    pub(super) integer: i32,
    pub(super) fraction: Option<(i32, i32)>,
    pub(super) significant: Option<(i32, i32)>,
    pub(super) increment: i32,
    pub(super) mode: usize,
    pub(super) priority: usize,
    pub(super) trailing: usize,
}

fn number_option(
    agent: &mut Agent,
    value: Value,
    name: &str,
    min: i32,
    max: i32,
    default: i32,
) -> JsResult<i32> {
    if value == Value::UNDEFINED {
        return Ok(default);
    }
    let number = agent.to_number(value)?;
    if (f64::from(min)..=f64::from(max)).contains(&number) {
        Ok(number.floor() as i32)
    } else {
        Err(agent.throw(
            ErrorKind::RangeError,
            &format!("invalid Intl {name} option"),
        ))
    }
}

pub(super) fn options(
    agent: &mut Agent,
    options: Option<ObjectId>,
    compact: bool,
) -> JsResult<Options> {
    let integer = matching::get_option(agent, options, "minimumIntegerDigits")?;
    let integer = number_option(agent, integer, "minimumIntegerDigits", 1, 21, 1)?;
    let min_fd = matching::get_option(agent, options, "minimumFractionDigits")?;
    let max_fd = matching::get_option(agent, options, "maximumFractionDigits")?;
    let min_sd = matching::get_option(agent, options, "minimumSignificantDigits")?;
    let max_sd = matching::get_option(agent, options, "maximumSignificantDigits")?;
    let increment = matching::get_option(agent, options, "roundingIncrement")?;
    let increment = number_option(agent, increment, "roundingIncrement", 1, 5000, 1)?;
    if !INCREMENTS.contains(&increment) {
        return Err(agent.throw(ErrorKind::RangeError, "invalid Intl roundingIncrement"));
    }
    let mode = matching::option_index(agent, options, "roundingMode", MODES)?;
    let mut priority = matching::option_index(agent, options, "roundingPriority", PRIORITIES)?;
    let trailing = matching::option_index(agent, options, "trailingZeroDisplay", TRAILING)?;

    let has_sd = min_sd != Value::UNDEFINED || max_sd != Value::UNDEFINED;
    let has_fd = min_fd != Value::UNDEFINED || max_fd != Value::UNDEFINED;
    let mut significant = if has_sd || priority != 0 {
        let min = number_option(agent, min_sd, "minimumSignificantDigits", 1, 21, 1)?;
        let max = number_option(agent, max_sd, "maximumSignificantDigits", min, 21, 21)?;
        Some((min, max))
    } else {
        None
    };
    let mut fraction = if priority != 0 || (!has_sd && (has_fd || !compact)) {
        let min = number_option(agent, min_fd, "minimumFractionDigits", 0, 100, 0)?;
        let default_max = if increment == 1 { min.max(3) } else { min };
        let max = number_option(agent, max_fd, "maximumFractionDigits", 0, 100, default_max)?;
        if min > max {
            return Err(agent.throw(
                ErrorKind::RangeError,
                "minimumFractionDigits exceeds maximumFractionDigits",
            ));
        }
        Some((min, max))
    } else {
        None
    };
    if fraction.is_none() && significant.is_none() {
        fraction = Some((0, 0));
        significant = Some((1, 2));
        priority = 1;
    }
    if increment != 1 {
        if significant.is_some() {
            return agent.type_error("roundingIncrement requires fractional rounding");
        }
        if fraction.is_some_and(|(minimum, maximum)| minimum != maximum) {
            return Err(agent.throw(
                ErrorKind::RangeError,
                "roundingIncrement requires equal fraction digit bounds",
            ));
        }
    }
    Ok(Options {
        integer,
        fraction,
        significant,
        increment,
        mode,
        priority,
        trailing,
    })
}

/// Leading and trailing zero digits are implicit. The point counts positions
/// before the decimal separator, relative to the first stored digit.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Decimal {
    pub(super) digits: Vec<u8>,
    pub(super) point: i32,
    pub(super) negative: bool,
}

impl Decimal {
    pub(super) fn new(mut digits: Vec<u8>, mut point: i32, negative: bool) -> Self {
        let Some(first) = digits.iter().position(|&value| value != 0) else {
            return Self {
                digits: Vec::new(),
                point: 1,
                negative,
            };
        };
        let end = digits
            .iter()
            .rposition(|&value| value != 0)
            .expect("nonzero digits")
            + 1;
        digits.truncate(end);
        digits.drain(..first);
        point -= i32::try_from(first).expect("bounded digit count");
        Self {
            digits,
            point,
            negative,
        }
    }

    fn at(&self, position: i32) -> u8 {
        usize::try_from(position)
            .ok()
            .and_then(|index| self.digits.get(index))
            .copied()
            .unwrap_or(0)
    }

    pub(super) fn digit_positions(&self, from: i32, to: i32) -> impl Iterator<Item = u8> + '_ {
        (from..to).map(|position| self.at(position))
    }

    pub(super) fn fraction_digits(&self) -> i32 {
        if self.digits.is_empty() {
            0
        } else {
            (i32::try_from(self.digits.len()).expect("bounded digit count") - self.point).max(0)
        }
    }
}

fn update_decimal(digits: &mut Vec<u8>, mut adjustment: i32) {
    for digit in digits.iter_mut().rev() {
        let value = i32::from(*digit) + adjustment;
        *digit = u8::try_from(value.rem_euclid(10)).expect("decimal remainder");
        adjustment = value.div_euclid(10);
        if adjustment == 0 {
            return;
        }
    }
    assert!(
        adjustment >= 0,
        "rounding subtracts at most the integer coefficient"
    );
    if adjustment > 0 {
        digits.splice(
            ..0,
            adjustment.to_string().bytes().map(|digit| digit - b'0'),
        );
    }
}

fn nearest(remainder: i32, increment: i32, tail_first: u8, tail_after_first: bool) -> Ordering {
    // Examine two times the distance to the lower multiple. After the
    // first fractional digit, a nonzero tail can only break an exact tie.
    (20 * remainder + 2 * i32::from(tail_first))
        .cmp(&(10 * increment))
        .then_with(|| tail_after_first.cmp(&false))
}

fn round_up(mode: usize, negative: bool, half: Ordering, lower_is_odd: bool) -> bool {
    match mode {
        1 => !negative,
        2 => negative,
        3 => true,
        4 => false,
        _ => match half {
            Ordering::Less => false,
            Ordering::Greater => true,
            Ordering::Equal => match mode {
                5 => !negative,
                6 => negative,
                7 => false,
                8 => lower_is_odd,
                _ => true,
            },
        },
    }
}

impl Options {
    pub(super) fn round(&self, decimal: &Decimal) -> (Decimal, i32) {
        let fraction_place = self.fraction.map_or(0, |(_, maximum)| -maximum);
        let significant_place = self
            .significant
            .map_or(0, |(_, maximum)| decimal.point - maximum);
        let significant = match (self.fraction, self.significant, self.priority) {
            (None, Some(_), _) => true,
            (Some(_), Some(_), 1) => significant_place <= fraction_place,
            (Some(_), Some(_), 2) => significant_place > fraction_place,
            _ => false,
        };
        let place = if significant {
            significant_place
        } else {
            fraction_place
        };
        let mut rounded = self.round_at(decimal, place);
        let minimum_fraction = if significant {
            let (minimum, _) = self.significant.expect("selected significant rounding");
            (minimum - rounded.point).max(0)
        } else {
            self.fraction.expect("selected fraction rounding").0
        };
        // Keep the original sign for range identity, including negative zero.
        let fraction = rounded.fraction_digits();
        let visible = if self.trailing == 1 && fraction == 0 {
            0
        } else {
            fraction.max(minimum_fraction)
        };
        if rounded.digits.is_empty() {
            rounded.point = 1;
        }
        (rounded, visible)
    }

    fn round_at(&self, decimal: &Decimal, place: i32) -> Decimal {
        if decimal.digits.is_empty() {
            return decimal.clone();
        }
        let keep = decimal.point - place;
        let mut coefficient: Vec<_> = decimal.digit_positions(0, keep).collect();
        let increment = self.increment;
        let residue = coefficient.iter().fold(0, |rest, &digit| {
            (rest * 10 + i32::from(digit)) % (2 * increment)
        });
        let remainder = residue % increment;
        let last = i32::try_from(decimal.digits.len()).expect("bounded digit count");
        let tail_first = decimal.at(keep);
        let tail_after = last > keep + 1;
        if remainder != 0 || last > keep {
            let half = nearest(remainder, increment, tail_first, tail_after);
            let up = round_up(self.mode, decimal.negative, half, residue >= increment);
            update_decimal(
                &mut coefficient,
                if up {
                    increment - remainder
                } else {
                    -remainder
                },
            );
        }
        let point = i32::try_from(coefficient.len()).expect("bounded digit count") + place;
        Decimal::new(coefficient, point, decimal.negative)
    }
}
