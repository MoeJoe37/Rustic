//! `Date` (§21.4): time values, the constructor, `Date.parse`,
//! `Date.UTC`, `Date.now` and the methods of `Date.prototype`, with the
//! Annex B `getYear`, `setYear` and `toGMTString`.
//!
//! A time value is milliseconds since 1970-01-01T00:00:00Z, kept in the
//! object's primitive slot. Calendar fields come from integer
//! civil-date arithmetic on the day number, so every field getter is a
//! few divisions.
//!
//! Local time is UTC plus one fixed offset that the embedder sets
//! ([`Agent::set_time_zone_offset`]); there is no time zone database, so
//! no daylight saving time (TD-108). The clock is the system clock unless
//! the embedder fixes it ([`Agent::set_clock`]), which tests and fuzzing
//! do to be reproducible.

use super::{Method, arg, install_constructor, install_methods, method, prototype_from_new_target};
use crate::runtime::ErrorKind;
use crate::runtime::agent::{Agent, JsResult};
use crate::runtime::convert::PreferredType;
use crate::runtime::number::to_integer_or_infinity;
use crate::runtime::object::{Class, Object, PropertyDescriptor};
use crate::runtime::shape::PropertyKey;
use crate::runtime::value::Value;

const MS_PER_SECOND: f64 = 1000.0;
const MS_PER_MINUTE: f64 = 60_000.0;
const MS_PER_HOUR: f64 = 3_600_000.0;
const MS_PER_DAY: f64 = 86_400_000.0;
/// The largest time value magnitude (§21.4.1.31): 100,000,000 days.
const MAX_TIME: f64 = 8.64e15;

const DAY_NAMES: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
const MONTH_NAMES: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

const STATIC_METHODS: &[Method] = &[
    method("UTC", date_utc, 7),
    method("now", date_now, 0),
    method("parse", date_parse, 1),
];

const PROTOTYPE_METHODS: &[Method] = &[
    method("getDate", get_date, 0),
    method("getDay", get_day, 0),
    method("getFullYear", get_full_year, 0),
    method("getHours", get_hours, 0),
    method("getMilliseconds", get_milliseconds, 0),
    method("getMinutes", get_minutes, 0),
    method("getMonth", get_month, 0),
    method("getSeconds", get_seconds, 0),
    method("getTime", get_time, 0),
    method("getTimezoneOffset", get_timezone_offset, 0),
    method("getUTCDate", get_utc_date, 0),
    method("getUTCDay", get_utc_day, 0),
    method("getUTCFullYear", get_utc_full_year, 0),
    method("getUTCHours", get_utc_hours, 0),
    method("getUTCMilliseconds", get_utc_milliseconds, 0),
    method("getUTCMinutes", get_utc_minutes, 0),
    method("getUTCMonth", get_utc_month, 0),
    method("getUTCSeconds", get_utc_seconds, 0),
    method("getYear", get_year, 0),
    method("setDate", set_date, 1),
    method("setFullYear", set_full_year, 3),
    method("setHours", set_hours, 4),
    method("setMilliseconds", set_milliseconds, 1),
    method("setMinutes", set_minutes, 3),
    method("setMonth", set_month, 2),
    method("setSeconds", set_seconds, 2),
    method("setTime", set_time, 1),
    method("setUTCDate", set_utc_date, 1),
    method("setUTCFullYear", set_utc_full_year, 3),
    method("setUTCHours", set_utc_hours, 4),
    method("setUTCMilliseconds", set_utc_milliseconds, 1),
    method("setUTCMinutes", set_utc_minutes, 3),
    method("setUTCMonth", set_utc_month, 2),
    method("setUTCSeconds", set_utc_seconds, 2),
    method("setYear", set_year, 1),
    method("toDateString", to_date_string, 0),
    method("toISOString", to_iso_string, 0),
    method("toJSON", to_json, 1),
    method("toLocaleDateString", to_date_string, 0),
    method("toLocaleString", to_string, 0),
    method("toLocaleTimeString", to_time_string, 0),
    method("toString", to_string, 0),
    method("toTimeString", to_time_string, 0),
    method("toUTCString", to_utc_string, 0),
    method("valueOf", get_time, 0),
];

pub(super) fn install(agent: &mut Agent) -> JsResult<()> {
    let object_prototype = agent.intrinsics.object_prototype;
    let prototype = agent.new_object_with(Some(object_prototype))?;
    let date = install_constructor(agent, "Date", date_constructor, 7, prototype)?;
    install_methods(agent, date, STATIC_METHODS)?;
    install_methods(agent, prototype, PROTOTYPE_METHODS)?;
    // `toGMTString` is the same function object as `toUTCString` (§B.2.3).
    let utc_key = agent.heap.key("toUTCString")?;
    let to_utc = agent.get(prototype, utc_key)?;
    let gmt_key = agent.heap.key("toGMTString")?;
    agent.define_property_or_throw(
        prototype,
        gmt_key,
        PropertyDescriptor::data_with(to_utc, true, false, true),
    )?;
    let f = agent.new_native_function(to_primitive, "[Symbol.toPrimitive]", 1)?;
    agent.define_property_or_throw(
        prototype,
        PropertyKey::Symbol(agent.symbols.to_primitive),
        PropertyDescriptor::data_with(Value::object(f), false, false, true),
    )
}

// ---- time arithmetic (§21.4.1) ------------------------------------------

/// Days from 1970-01-01 to the proleptic Gregorian date `y`-`m`-`d`
/// (`m` from 1). Exact for any year the heap can name.
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// The date of day number `z`: year, month from 1, day of month.
fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

fn days_in_month(y: i64, m: i64) -> i64 {
    match m {
        2 if y % 4 == 0 && (y % 100 != 0 || y % 400 == 0) => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

/// The calendar fields of a finite time value.
#[derive(Clone, Copy)]
struct Fields {
    year: i64,
    /// From 0.
    month: i64,
    date: i64,
    hour: i64,
    minute: i64,
    second: i64,
    ms: i64,
    /// 0 is Sunday.
    weekday: i64,
}

impl Fields {
    // Time values are integers within ±8.64e15 plus an offset, so the
    // day number and the time within the day fit `i64` exactly.
    fn of(t: f64) -> Fields {
        let days = (t / MS_PER_DAY).floor() as i64;
        let within = t.rem_euclid(MS_PER_DAY) as i64;
        let (year, month, date) = civil_from_days(days);
        Fields {
            year,
            month: month - 1,
            date,
            hour: within / 3_600_000,
            minute: within / 60_000 % 60,
            second: within / 1000 % 60,
            ms: within % 1000,
            weekday: (days + 4).rem_euclid(7),
        }
    }
}

/// MakeTime (§21.4.1.28).
fn make_time(hour: f64, min: f64, sec: f64, ms: f64) -> f64 {
    if !(hour.is_finite() && min.is_finite() && sec.is_finite() && ms.is_finite()) {
        return f64::NAN;
    }
    let h = to_integer_or_infinity(hour);
    let m = to_integer_or_infinity(min);
    let s = to_integer_or_infinity(sec);
    let milli = to_integer_or_infinity(ms);
    h * MS_PER_HOUR + m * MS_PER_MINUTE + s * MS_PER_SECOND + milli
}

/// MakeDay (§21.4.1.29).
fn make_day(year: f64, month: f64, date: f64) -> f64 {
    if !(year.is_finite() && month.is_finite() && date.is_finite()) {
        return f64::NAN;
    }
    let y = to_integer_or_infinity(year);
    let m = to_integer_or_infinity(month);
    let dt = to_integer_or_infinity(date);
    let ym = y + (m / 12.0).floor();
    // Beyond ±400,000 years every result is past TimeClip's range, and
    // the bound keeps the integer arithmetic exact.
    if !ym.is_finite() || ym.abs() > 400_000.0 {
        return f64::NAN;
    }
    let mn = m.rem_euclid(12.0);
    let day = days_from_civil(ym as i64, mn as i64 + 1, 1) as f64;
    day + dt - 1.0
}

/// MakeDate (§21.4.1.30).
fn make_date(day: f64, time: f64) -> f64 {
    if !(day.is_finite() && time.is_finite()) {
        return f64::NAN;
    }
    let tv = day * MS_PER_DAY + time;
    if tv.is_finite() { tv } else { f64::NAN }
}

/// TimeClip (§21.4.1.31).
fn time_clip(t: f64) -> f64 {
    if !t.is_finite() || t.abs() > MAX_TIME {
        return f64::NAN;
    }
    // Adding +0 turns -0 into +0.
    to_integer_or_infinity(t) + 0.0
}

fn local_time(agent: &Agent, t: f64) -> f64 {
    t + agent.vm.time_zone_offset_ms
}

/// UTC(t) (§21.4.1.26).
fn utc(agent: &Agent, t: f64) -> f64 {
    if t.is_finite() {
        t - agent.vm.time_zone_offset_ms
    } else {
        f64::NAN
    }
}

/// The current time value.
pub(crate) fn now(agent: &Agent) -> f64 {
    if let Some(t) = agent.vm.clock {
        return t;
    }
    match std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
        Ok(d) => d.as_millis() as f64,
        Err(e) => -(e.duration().as_millis() as f64),
    }
}

// ---- receivers -----------------------------------------------------------

/// thisTimeValue (§21.4.4): the time value of a Date receiver.
fn this_time(agent: &mut Agent, this: Value, name: &str) -> JsResult<f64> {
    if let Some(o) = this.as_object() {
        if agent.class(o) == Class::Date {
            if let Some(t) = agent.primitive_of(o).and_then(|v| v.as_number()) {
                return Ok(t);
            }
        }
    }
    agent.type_error(&format!("Date.prototype.{name} called on a non-Date"))
}

fn set_this_time(agent: &mut Agent, this: Value, t: f64) -> Value {
    let o = this.as_object().expect("this_time accepted a Date");
    let v = Value::number(t);
    agent.heap.object_mut(o).extra_mut().primitive = Some(v);
    v
}

// ---- the constructor and statics -----------------------------------------

fn date_constructor(agent: &mut Agent, _: Value, args: &[Value]) -> JsResult<Value> {
    if agent.new_target().is_none() {
        let t = now(agent);
        return string_value(agent, t, Format::Full);
    }
    let tv = match args.len() {
        0 => now(agent),
        1 => {
            let value = args[0];
            let date_value = value
                .as_object()
                .filter(|&o| agent.class(o) == Class::Date)
                .and_then(|o| agent.primitive_of(o))
                .and_then(|v| v.as_number());
            match date_value {
                Some(t) => t,
                None => {
                    let v = agent.to_primitive(value, None)?;
                    match v.as_string() {
                        Some(s) => {
                            let text = agent.heap.string(s).to_text();
                            parse(agent, &text)
                        }
                        None => agent.to_number(v)?,
                    }
                }
            }
        }
        _ => {
            let t = from_components(agent, args)?;
            utc(agent, t)
        }
    };
    let default = own_prototype(agent)?;
    let prototype = prototype_from_new_target(agent, default)?;
    let mut object = Object::new(Class::Date, Some(prototype));
    object.extra_mut().primitive = Some(Value::number(time_clip(tv)));
    agent
        .heap
        .charge(std::mem::size_of::<crate::runtime::object::Extra>())?;
    Ok(Value::object(agent.heap.alloc_object(object)?))
}

/// `%Date.prototype%`, through the running constructor.
fn own_prototype(agent: &mut Agent) -> JsResult<crate::runtime::value::ObjectId> {
    let f = agent.active_function().expect("a constructor is running");
    let key = PropertyKey::String(agent.names.prototype);
    match agent.get(f, key)?.as_object() {
        Some(p) => Ok(p),
        None => Ok(agent.intrinsics.object_prototype),
    }
}

/// The date of `year, month[, date, hours, minutes, seconds, ms]` as the
/// constructor and `Date.UTC` read them, before any time zone.
fn from_components(agent: &mut Agent, args: &[Value]) -> JsResult<f64> {
    let mut n = [f64::NAN, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0];
    for (i, slot) in n.iter_mut().enumerate() {
        if i == 0 || i < args.len() {
            *slot = agent.to_number(arg(args, i))?;
        }
    }
    let mut year = n[0];
    if !year.is_nan() {
        let yi = to_integer_or_infinity(year);
        if (0.0..=99.0).contains(&yi) {
            year = 1900.0 + yi;
        }
    }
    Ok(make_date(
        make_day(year, n[1], n[2]),
        make_time(n[3], n[4], n[5], n[6]),
    ))
}

fn date_utc(agent: &mut Agent, _: Value, args: &[Value]) -> JsResult<Value> {
    let t = from_components(agent, args)?;
    Ok(Value::number(time_clip(t)))
}

fn date_now(agent: &mut Agent, _: Value, _: &[Value]) -> JsResult<Value> {
    Ok(Value::number(now(agent)))
}

fn date_parse(agent: &mut Agent, _: Value, args: &[Value]) -> JsResult<Value> {
    let s = agent.to_string(arg(args, 0))?;
    let text = agent.heap.string(s).to_text();
    Ok(Value::number(time_clip(parse(agent, &text))))
}

// ---- getters ---------------------------------------------------------------

fn field(
    agent: &mut Agent,
    this: Value,
    name: &str,
    local: bool,
    pick: fn(&Fields) -> i64,
) -> JsResult<Value> {
    let t = this_time(agent, this, name)?;
    if t.is_nan() {
        return Ok(Value::number(f64::NAN));
    }
    let t = if local { local_time(agent, t) } else { t };
    // Calendar fields are far below 2⁵³.
    Ok(Value::number(pick(&Fields::of(t)) as f64))
}

macro_rules! getters {
    ($($f:ident $name:literal $local:literal $field:ident;)*) => {$(
        fn $f(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
            field(agent, this, $name, $local, |f| f.$field)
        }
    )*};
}

getters! {
    get_date "getDate" true date;
    get_day "getDay" true weekday;
    get_full_year "getFullYear" true year;
    get_hours "getHours" true hour;
    get_milliseconds "getMilliseconds" true ms;
    get_minutes "getMinutes" true minute;
    get_month "getMonth" true month;
    get_seconds "getSeconds" true second;
    get_utc_date "getUTCDate" false date;
    get_utc_day "getUTCDay" false weekday;
    get_utc_full_year "getUTCFullYear" false year;
    get_utc_hours "getUTCHours" false hour;
    get_utc_milliseconds "getUTCMilliseconds" false ms;
    get_utc_minutes "getUTCMinutes" false minute;
    get_utc_month "getUTCMonth" false month;
    get_utc_seconds "getUTCSeconds" false second;
}

fn get_time(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    Ok(Value::number(this_time(agent, this, "getTime")?))
}

fn get_timezone_offset(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    let t = this_time(agent, this, "getTimezoneOffset")?;
    if t.is_nan() {
        return Ok(Value::number(f64::NAN));
    }
    Ok(Value::number((t - local_time(agent, t)) / MS_PER_MINUTE))
}

/// `getYear` (§B.2.3.1).
fn get_year(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    let t = this_time(agent, this, "getYear")?;
    if t.is_nan() {
        return Ok(Value::number(f64::NAN));
    }
    Ok(Value::number(
        (Fields::of(local_time(agent, t)).year - 1900) as f64,
    ))
}

// ---- setters ---------------------------------------------------------------

/// The fields a setter writes, in the order of the time value.
#[derive(Clone, Copy, PartialEq)]
enum First {
    Year = 0,
    Month = 1,
    Date = 2,
    Hour = 3,
    Minute = 4,
    Second = 5,
    Ms = 6,
}

/// The shared body of the `set…` methods (§21.4.4.20–28): converts every
/// argument present (the first always), then rebuilds the time value
/// from the fields with the given ones replaced.
fn set_fields(
    agent: &mut Agent,
    this: Value,
    args: &[Value],
    name: &str,
    local: bool,
    first: First,
) -> JsResult<Value> {
    let t = this_time(agent, this, name)?;
    let start = first as usize;
    // Year, month and date run to the date; the time fields to the ms.
    let end = if start <= 2 { 3 } else { 7 };
    let mut given = [None; 4];
    for (i, slot) in given.iter_mut().enumerate().take(end - start) {
        if i == 0 || i < args.len() {
            *slot = Some(agent.to_number(arg(args, i))?);
        }
    }
    let t = if t.is_nan() {
        if first != First::Year {
            return Ok(Value::number(f64::NAN));
        }
        0.0
    } else if local {
        local_time(agent, t)
    } else {
        t
    };
    let f = Fields::of(t);
    let mut n = [f.year, f.month, f.date, f.hour, f.minute, f.second, f.ms].map(|v| v as f64);
    for (i, v) in given.iter().enumerate() {
        if let Some(v) = v {
            n[start + i] = *v;
        }
    }
    let date = make_date(
        make_day(n[0], n[1], n[2]),
        make_time(n[3], n[4], n[5], n[6]),
    );
    let u = time_clip(if local { utc(agent, date) } else { date });
    Ok(set_this_time(agent, this, u))
}

macro_rules! setters {
    ($($f:ident $name:literal $local:literal $first:ident;)*) => {$(
        fn $f(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
            set_fields(agent, this, args, $name, $local, First::$first)
        }
    )*};
}

setters! {
    set_date "setDate" true Date;
    set_full_year "setFullYear" true Year;
    set_hours "setHours" true Hour;
    set_milliseconds "setMilliseconds" true Ms;
    set_minutes "setMinutes" true Minute;
    set_month "setMonth" true Month;
    set_seconds "setSeconds" true Second;
    set_utc_date "setUTCDate" false Date;
    set_utc_full_year "setUTCFullYear" false Year;
    set_utc_hours "setUTCHours" false Hour;
    set_utc_milliseconds "setUTCMilliseconds" false Ms;
    set_utc_minutes "setUTCMinutes" false Minute;
    set_utc_month "setUTCMonth" false Month;
    set_utc_seconds "setUTCSeconds" false Second;
}

fn set_time(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    this_time(agent, this, "setTime")?;
    let t = agent.to_number(arg(args, 0))?;
    Ok(set_this_time(agent, this, time_clip(t)))
}

/// `setYear` (§B.2.3.2).
fn set_year(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let t = this_time(agent, this, "setYear")?;
    let y = agent.to_number(arg(args, 0))?;
    if y.is_nan() {
        return Ok(set_this_time(agent, this, f64::NAN));
    }
    let yi = to_integer_or_infinity(y);
    let year = if (0.0..=99.0).contains(&yi) {
        1900.0 + yi
    } else {
        y
    };
    let t = if t.is_nan() {
        0.0
    } else {
        local_time(agent, t)
    };
    let f = Fields::of(t);
    let d = make_day(year, f.month as f64, f.date as f64);
    let within = t.rem_euclid(MS_PER_DAY);
    let date = make_date(d, within);
    let u = time_clip(utc(agent, date));
    Ok(set_this_time(agent, this, u))
}

// ---- strings ---------------------------------------------------------------

#[derive(Clone, Copy)]
enum Format {
    Full,
    Date,
    Time,
}

/// A year as DateString writes it: at least four digits, `-` if negative.
fn year_text(year: i64) -> String {
    if year < 0 {
        format!("-{:04}", -year)
    } else {
        format!("{year:04}")
    }
}

/// `+hhmm` or `-hhmm` for the local offset, then the zone's name.
fn zone_text(agent: &Agent) -> String {
    let offset = agent.vm.time_zone_offset_ms;
    let minutes = (offset / MS_PER_MINUTE) as i64;
    let sign = if minutes < 0 { '-' } else { '+' };
    let m = minutes.abs();
    let name = if minutes == 0 {
        " (Coordinated Universal Time)"
    } else {
        ""
    };
    format!("GMT{sign}{:02}{:02}{name}", m / 60, m % 60)
}

/// ToDateString (§21.4.4.41.4) and its date-only and time-only parts.
fn string_value(agent: &mut Agent, tv: f64, format: Format) -> JsResult<Value> {
    if tv.is_nan() {
        return agent.string("Invalid Date");
    }
    let f = Fields::of(local_time(agent, tv));
    let date = format!(
        "{} {} {:02} {}",
        DAY_NAMES[f.weekday as usize],
        MONTH_NAMES[f.month as usize],
        f.date,
        year_text(f.year)
    );
    let time = format!(
        "{:02}:{:02}:{:02} {}",
        f.hour,
        f.minute,
        f.second,
        zone_text(agent)
    );
    let text = match format {
        Format::Full => format!("{date} {time}"),
        Format::Date => date,
        Format::Time => time,
    };
    agent.string(&text)
}

fn to_string(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    let t = this_time(agent, this, "toString")?;
    string_value(agent, t, Format::Full)
}

fn to_date_string(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    let t = this_time(agent, this, "toDateString")?;
    string_value(agent, t, Format::Date)
}

fn to_time_string(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    let t = this_time(agent, this, "toTimeString")?;
    string_value(agent, t, Format::Time)
}

fn to_utc_string(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    let t = this_time(agent, this, "toUTCString")?;
    if t.is_nan() {
        return agent.string("Invalid Date");
    }
    let f = Fields::of(t);
    let text = format!(
        "{}, {:02} {} {} {:02}:{:02}:{:02} GMT",
        DAY_NAMES[f.weekday as usize],
        f.date,
        MONTH_NAMES[f.month as usize],
        year_text(f.year),
        f.hour,
        f.minute,
        f.second
    );
    agent.string(&text)
}

fn to_iso_string(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    let t = this_time(agent, this, "toISOString")?;
    if !t.is_finite() {
        return Err(agent.throw(ErrorKind::RangeError, "Invalid time value"));
    }
    let f = Fields::of(t);
    let year = if (0..=9999).contains(&f.year) {
        format!("{:04}", f.year)
    } else if f.year < 0 {
        format!("-{:06}", -f.year)
    } else {
        format!("+{:06}", f.year)
    };
    let text = format!(
        "{year}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}Z",
        f.month + 1,
        f.date,
        f.hour,
        f.minute,
        f.second,
        f.ms
    );
    agent.string(&text)
}

/// `toJSON` (§21.4.4.37): generic, through `toISOString`.
fn to_json(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    let o = agent.to_object(this)?;
    let tv = agent.to_primitive(Value::object(o), Some(PreferredType::Number))?;
    if let Some(n) = tv.as_number() {
        if !n.is_finite() {
            return Ok(Value::NULL);
        }
    }
    let key = agent.heap.key("toISOString")?;
    agent.invoke(Value::object(o), key, &[])
}

/// `Date.prototype[@@toPrimitive](hint)` (§21.4.4.45).
fn to_primitive(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    if this.as_object().is_none() {
        return agent.type_error("Date.prototype[Symbol.toPrimitive] called on a non-object");
    }
    let hint = arg(args, 0);
    let text = hint.as_string().map(|s| agent.heap.string(s).to_text());
    let preferred = match text.as_deref() {
        Some("string" | "default") => PreferredType::String,
        Some("number") => PreferredType::Number,
        _ => return agent.type_error("Invalid hint for Date.prototype[Symbol.toPrimitive]"),
    };
    agent.ordinary_to_primitive(this, preferred)
}

// ---- parsing (§21.4.3.2) ---------------------------------------------------

/// The time value of `text`, or NaN. Accepts the Date Time String Format
/// (§21.4.1.32), including expanded years, and the forms `toString` and
/// `toUTCString` produce.
fn parse(agent: &Agent, text: &str) -> f64 {
    let text = text.trim();
    if let Some((t, local)) = parse_iso(text) {
        return if local { utc(agent, t) } else { t };
    }
    match parse_legacy(text) {
        Some((t, None)) => utc(agent, t),
        Some((t, Some(offset))) => t - offset,
        None => f64::NAN,
    }
}

struct Cursor<'a> {
    s: &'a [u8],
    at: usize,
}

impl Cursor<'_> {
    fn peek(&self) -> Option<u8> {
        self.s.get(self.at).copied()
    }

    fn eat(&mut self, c: u8) -> bool {
        if self.peek() == Some(c) {
            self.at += 1;
            true
        } else {
            false
        }
    }

    /// Exactly `n` digits.
    fn digits(&mut self, n: usize) -> Option<i64> {
        let digits = self.s.get(self.at..self.at + n)?;
        if !digits.iter().all(u8::is_ascii_digit) {
            return None;
        }
        self.at += n;
        Some(digits.iter().fold(0, |v, d| v * 10 + i64::from(d - b'0')))
    }
}

/// The Date Time String Format: the time value and whether it is local
/// time (a date-time form without an offset).
fn parse_iso(text: &str) -> Option<(f64, bool)> {
    let mut c = Cursor {
        s: text.as_bytes(),
        at: 0,
    };
    let year = match c.peek()? {
        b'+' | b'-' => {
            let negative = c.peek() == Some(b'-');
            c.at += 1;
            let y = c.digits(6)?;
            if negative && y == 0 {
                return None;
            }
            if negative { -y } else { y }
        }
        _ => c.digits(4)?,
    };
    let (mut month, mut day) = (1, 1);
    if c.eat(b'-') {
        month = c.digits(2)?;
        if c.eat(b'-') {
            day = c.digits(2)?;
        }
    }
    if !(1..=12).contains(&month) || day < 1 || day > days_in_month(year, month) {
        return None;
    }
    let (mut hour, mut minute, mut second, mut ms) = (0, 0, 0, 0);
    let mut local = false;
    let mut offset = 0;
    if c.eat(b'T') {
        hour = c.digits(2)?;
        if !c.eat(b':') {
            return None;
        }
        minute = c.digits(2)?;
        if c.eat(b':') {
            second = c.digits(2)?;
            if c.eat(b'.') {
                let start = c.at;
                while c.peek().is_some_and(|b| b.is_ascii_digit()) {
                    c.at += 1;
                }
                let fraction = &c.s[start..c.at];
                if fraction.is_empty() {
                    return None;
                }
                ms = fraction
                    .iter()
                    .chain(std::iter::repeat(&b'0'))
                    .take(3)
                    .fold(0, |v, d| v * 10 + i64::from(d - b'0'));
            }
        }
        match c.peek() {
            Some(b'Z') => c.at += 1,
            Some(sign @ (b'+' | b'-')) => {
                c.at += 1;
                let h = c.digits(2)?;
                if !c.eat(b':') {
                    return None;
                }
                let m = c.digits(2)?;
                if h > 23 || m > 59 {
                    return None;
                }
                let minutes = h * 60 + m;
                offset = if sign == b'-' { -minutes } else { minutes };
            }
            _ => local = true,
        }
        if hour > 24 || minute > 59 || second > 59 {
            return None;
        }
        if hour == 24 && (minute != 0 || second != 0 || ms != 0) {
            return None;
        }
    }
    if c.at != c.s.len() {
        return None;
    }
    let date = make_date(
        days_from_civil(year, month, day) as f64,
        make_time(hour as f64, minute as f64, second as f64, ms as f64),
    );
    Some((date - offset as f64 * MS_PER_MINUTE, local))
}

/// The forms `toString` and `toUTCString` write, and the common variants
/// of them: `[Www[,]] Mmm DD YYYY` or `[Www[,]] DD Mmm YYYY`, then an
/// optional `HH:mm[:ss]`, then an optional `GMT`/`UTC`/`Z` with an
/// optional `±hhmm`, then an optional parenthesized comment. Returns the
/// time before any zone and the zone's offset in milliseconds, if given.
fn parse_legacy(text: &str) -> Option<(f64, Option<f64>)> {
    let (mut year, mut month, mut day) = (None, None, None);
    let mut time = None;
    let mut offset = None;
    let mut rest = text;
    while let Some(c) = rest.chars().next() {
        if c.is_whitespace() || c == ',' {
            rest = &rest[c.len_utf8()..];
            continue;
        }
        if c == '(' {
            let close = rest.find(')')?;
            rest = &rest[close + 1..];
            continue;
        }
        let end = rest
            .find(|c: char| c.is_whitespace() || c == ',' || c == '(')
            .unwrap_or(rest.len());
        let token = &rest[..end];
        rest = &rest[end..];
        let lower = token.to_ascii_lowercase();
        if let Some(zone) = ["gmt", "utc", "z"].iter().find(|z| lower.starts_with(**z)) {
            let tail = &token[zone.len()..];
            let minutes = if tail.is_empty() {
                0
            } else {
                parse_offset(tail)?
            };
            offset = Some(minutes as f64 * MS_PER_MINUTE);
        } else if lower.len() >= 3 && lower.bytes().all(|b| b.is_ascii_alphabetic()) {
            if let Some(m) = MONTH_NAMES
                .iter()
                .position(|m| lower.starts_with(&m.to_ascii_lowercase()))
            {
                month = Some(m as i64);
            } else if !DAY_NAMES
                .iter()
                .any(|d| lower.starts_with(&d.to_ascii_lowercase()))
            {
                return None;
            }
        } else if token.contains(':') {
            let mut parts = token.split(':');
            let h: i64 = parts.next()?.parse().ok()?;
            let m: i64 = parts.next()?.parse().ok()?;
            let s: i64 = match parts.next() {
                Some(s) => s.parse().ok()?,
                None => 0,
            };
            if parts.next().is_some() || h > 24 || m > 59 || s > 59 {
                return None;
            }
            time = Some((h, m, s));
        } else if offset.is_some() || time.is_some() && token.starts_with('+') {
            offset = Some(parse_offset(token)? as f64 * MS_PER_MINUTE);
        } else {
            let negative = token.starts_with('-');
            let digits = token.strip_prefix(['-', '+']).unwrap_or(token);
            if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
            let n: i64 = digits.parse().ok()?;
            if day.is_none() && !negative && n <= 31 && year.is_none() {
                day = Some(n);
            } else if year.is_none() {
                year = Some(if negative { -n } else { n });
            } else {
                return None;
            }
        }
    }
    let (year, month, day) = (year?, month?, day?);
    let (h, m, s) = time.unwrap_or((0, 0, 0));
    let date = make_date(
        make_day(year as f64, month as f64, day as f64),
        make_time(h as f64, m as f64, s as f64, 0.0),
    );
    if date.is_nan() {
        return None;
    }
    Some((date, offset))
}

/// `±hhmm` or `±hh:mm` as signed minutes.
fn parse_offset(text: &str) -> Option<i64> {
    let (sign, digits) = match text.as_bytes().first()? {
        b'+' => (1, &text[1..]),
        b'-' => (-1, &text[1..]),
        _ => return None,
    };
    let digits: String = digits.chars().filter(|&c| c != ':').collect();
    if digits.len() != 4 || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let h: i64 = digits[..2].parse().ok()?;
    let m: i64 = digits[2..].parse().ok()?;
    Some(sign * (h * 60 + m))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn civil_dates_round_trip_across_eras() {
        for z in [-719_468 * 3, -1, 0, 1, 11_016, 2_932_896, 100_000_000] {
            let (y, m, d) = civil_from_days(z);
            assert_eq!(days_from_civil(y, m, d), z, "day {z}");
        }
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(-1), (1969, 12, 31));
        assert_eq!(days_from_civil(2000, 3, 1), 11_017);
    }

    #[test]
    fn iso_strings_parse_with_and_without_zones() {
        assert_eq!(parse_iso("1970-01-01"), Some((0.0, false)));
        assert_eq!(parse_iso("1970-01-01T00:00:00.001Z"), Some((1.0, false)));
        assert_eq!(parse_iso("1970-01-01T01:00+01:00"), Some((0.0, false)));
        assert_eq!(parse_iso("1970-01-01T00:00"), Some((0.0, true)));
        assert_eq!(parse_iso("+002000-01-01").map(|p| p.1), Some(false));
        assert_eq!(parse_iso("-000000-01-01"), None);
        assert_eq!(parse_iso("2000-02-30"), None);
        assert_eq!(parse_iso("2000-01-01T24:00:01"), None);
    }

    #[test]
    fn legacy_strings_parse() {
        let t = parse_legacy("Thu Jan 01 1970 00:00:00 GMT+0000 (Coordinated Universal Time)");
        assert_eq!(t, Some((0.0, Some(0.0))));
        let t = parse_legacy("Thu, 01 Jan 1970 00:00:01 GMT");
        assert_eq!(t, Some((1000.0, Some(0.0))));
        assert_eq!(parse_legacy("hello"), None);
    }
}
