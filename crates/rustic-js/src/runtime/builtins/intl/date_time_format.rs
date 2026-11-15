//! Intl.DateTimeFormat (ECMA-402 §11) over pinned CLDR patterns and IANA
//! time-zone transitions.

use std::sync::Arc;

use super::super::{
    arg, date, install_methods, method, new_constructor, prototype_from_new_target,
};
use super::date_time_patterns::{self as patterns, Field, Fields, Token, Width};
use super::number_data::{Symbol, data as numbers};
use super::number_format::{charge_parts, is_type};
use super::numfmt::{self, Part, Source};
use super::service::{
    self, Resolution, best_fit, choice, choice_option, coerce_options, new_service, number_option,
    option, range_error, set, set_text, string_option, text_of,
};
use super::time_zone::{Offset, Zone};
use super::{canonicalize_locale_list, locale, slot};
use crate::runtime::{Agent, JsResult, ObjectId, PropertyDescriptor, PropertyKey, Value};

const STYLES: &[&str] = &["full", "long", "medium", "short"];
const CYCLES: &[&str] = &["h11", "h12", "h23", "h24"];
const CYCLE_LETTERS: [char; 4] = ['K', 'h', 'H', 'k'];

pub(super) const CALENDARS: &[&str] = &["buddhist", "gregory", "islamic-civil", "iso8601"];

#[derive(Clone, Copy)]
pub(in super::super) enum Usage {
    Date,
    Time,
    Both,
}

struct DateTimeFormat {
    locale: String,
    calendar: String,
    row: u32,
    system: u32,
    number_row: u32,
    zone: Zone,
    cycle: usize,
    pattern: Vec<Token>,
    fields: Fields,
    date_style: Option<usize>,
    time_style: Option<usize>,
}

pub(super) fn install(agent: &mut Agent, intl: ObjectId) -> JsResult<()> {
    let prototype = agent.new_object()?;
    agent.intrinsics.intl[slot::DATE_TIME_FORMAT_PROTOTYPE] = prototype;
    let constructor = new_constructor(agent, "DateTimeFormat", construct, 0, prototype)?;
    agent.intrinsics.intl[slot::DATE_TIME_FORMAT] = constructor;
    let key = agent.heap.key("DateTimeFormat")?;
    agent.define_property_or_throw(
        intl,
        key,
        PropertyDescriptor::data_with(Value::object(constructor), true, false, true),
    )?;
    let tag = agent.string("Intl.DateTimeFormat")?;
    agent.define_property_or_throw(
        prototype,
        PropertyKey::Symbol(agent.symbols.to_string_tag),
        PropertyDescriptor::data_with(tag, false, false, true),
    )?;
    install_methods(
        agent,
        constructor,
        &[method("supportedLocalesOf", supported_locales, 1)],
    )?;
    let getter = agent.new_native_function(format_getter, "get format", 0)?;
    let key = agent.heap.key("format")?;
    agent.define_property_or_throw(
        prototype,
        key,
        PropertyDescriptor::accessor(Value::object(getter), Value::UNDEFINED, false, true),
    )?;
    install_methods(
        agent,
        prototype,
        &[
            method("formatRange", format_range, 2),
            method("formatRangeToParts", format_range_to_parts, 2),
            method("formatToParts", format_to_parts, 1),
            method("resolvedOptions", resolved_options, 0),
        ],
    )
}

fn field_option(
    agent: &mut Agent,
    options: Option<ObjectId>,
    field: Field,
) -> JsResult<Option<Width>> {
    use Width::*;
    let values: &[Width] = match field {
        Field::Weekday | Field::Era | Field::Period => &[Narrow, Short, Long],
        Field::Month => &[Numeric, TwoDigit, Narrow, Short, Long],
        Field::Zone => &[
            Short,
            Long,
            ShortOffset,
            LongOffset,
            ShortGeneric,
            LongGeneric,
        ],
        Field::Fraction => {
            let digits = number_option(agent, options, field.property(), 1, 3, None)?;
            return Ok(digits.map(|digits| Fraction(u8::try_from(digits).expect("1 to 3 digits"))));
        }
        _ => &[Numeric, TwoDigit],
    };
    let names: Vec<_> = values.iter().map(|width| width.name()).collect();
    Ok(choice_option(agent, options, field.property(), &names)?.map(|index| values[index]))
}

fn calendar_option(
    agent: &mut Agent,
    options: Option<ObjectId>,
    name: &str,
) -> JsResult<Option<String>> {
    let value = string_option(agent, options, name, &[])?;
    if value.as_deref().is_some_and(|value| !is_type(value)) {
        return range_error(agent, "invalid Intl.DateTimeFormat locale option");
    }
    Ok(value.map(|value| value.to_ascii_lowercase()))
}

fn choose_zone(agent: &mut Agent, options: Option<ObjectId>) -> JsResult<Zone> {
    let value = option(agent, options, "timeZone")?;
    if value == Value::UNDEFINED {
        let minutes = (agent.vm.time_zone_offset_ms / 60_000.0) as i32;
        return Ok(if minutes == 0 {
            Zone::parse("UTC").expect("UTC data")
        } else {
            Zone::Fixed(minutes)
        });
    }
    let text = text_of(agent, value)?;
    match Zone::parse(&text) {
        Some(zone) => Ok(zone),
        None => range_error(agent, "invalid time zone"),
    }
}

fn create(
    agent: &mut Agent,
    locales: Value,
    options: Value,
    required: Usage,
    defaults: Usage,
) -> JsResult<DateTimeFormat> {
    let requested = canonicalize_locale_list(agent, locales)?;
    let options = coerce_options(agent, options)?;
    let matcher = best_fit(agent, options)?;
    let calendar = calendar_option(agent, options, "calendar")?.map(|value| {
        if value == "islamicc" {
            "islamic-civil".to_owned()
        } else {
            value
        }
    });
    let numbering = calendar_option(agent, options, "numberingSystem")?;
    let hour12 = option(agent, options, "hour12")?;
    let hour12 = (hour12 != Value::UNDEFINED).then(|| agent.to_boolean(hour12));
    let hour_cycle = choice_option(agent, options, "hourCycle", CYCLES)?;

    let data = patterns::data();
    let mut resolution = Resolution::new(&data.locales, &requested, matcher);
    let likely = locale::adjust_likely_subtags(resolution.base, false);
    let [_, _, region, _] = locale::base_fields(&likely);
    let default_calendar = if region == "TH" {
        "buddhist"
    } else {
        "gregory"
    };
    let calendar = resolution.key(
        "ca",
        calendar.as_deref(),
        |key| CALENDARS.contains(&key),
        default_calendar,
    );
    let number_resolution = Resolution::new(&numbers().locales, &requested, matcher);
    let number_row = number_resolution.row;
    let default_system =
        numbers().numbering_system_name(numbers().default_numbering_system(number_row));
    let system = resolution.key(
        "nu",
        numbering.as_deref(),
        |key| numbers().numbering_system(key).is_some(),
        default_system,
    );
    let system = numbers()
        .numbering_system(&system)
        .expect("resolved decimal numbering system");
    let default_cycle = patterns::default_cycle(resolution.row);
    let default_cycle = CYCLE_LETTERS
        .iter()
        .position(|&letter| letter == default_cycle)
        .unwrap_or(2);
    let cycle = match hour12 {
        Some(hour12) => {
            if (default_cycle < 2) == hour12 {
                default_cycle
            } else {
                let alternate = data.alternate_cycle(region, hour12);
                CYCLE_LETTERS
                    .iter()
                    .position(|&letter| letter == alternate)
                    .expect("CLDR hour cycle")
            }
        }
        None => {
            let chosen = resolution.key(
                "hc",
                hour_cycle.map(|index| CYCLES[index]),
                |key| CYCLES.contains(&key),
                CYCLES[default_cycle],
            );
            CYCLES
                .iter()
                .position(|&key| key == chosen)
                .expect("resolved hour cycle")
        }
    };

    let zone = choose_zone(agent, options)?;
    let mut requested_fields = [None; 11];
    for field in Field::ALL {
        requested_fields[field as usize] = field_option(agent, options, field)?;
    }
    let best_fit = choice(agent, options, "formatMatcher", &["basic", "best fit"], 1)? == 1;
    let date_style = choice_option(agent, options, "dateStyle", STYLES)?;
    let time_style = choice_option(agent, options, "timeStyle", STYLES)?;
    if (date_style.is_some() && matches!(required, Usage::Time))
        || (time_style.is_some() && matches!(required, Usage::Date))
    {
        return agent.type_error("date or time style does not match the Date method");
    }
    let row = resolution.row;
    let has_style = date_style.is_some() || time_style.is_some();
    let pattern = if has_style {
        if requested_fields.iter().any(Option::is_some) {
            return agent
                .type_error("date or time styles cannot be combined with component options");
        }
        style_pattern(row, date_style, time_style, CYCLE_LETTERS[cycle])
    } else {
        default_fields(&mut requested_fields, required, defaults);
        if cycle >= 2 {
            requested_fields[Field::Period as usize] = None;
        }
        component_pattern(row, requested_fields, CYCLE_LETTERS[cycle], best_fit)
    };
    let mut fields = patterns::fields(&pattern);
    // A pattern may contain an implicit AM/PM marker; resolvedOptions only
    // reports an explicit dayPeriod option.
    if requested_fields[Field::Period as usize].is_none() {
        fields[Field::Period as usize] = None;
    }
    Ok(DateTimeFormat {
        locale: resolution.locale(),
        calendar,
        row,
        system,
        number_row,
        zone,
        cycle,
        pattern,
        fields,
        date_style,
        time_style,
    })
}

fn default_fields(fields: &mut Fields, required: Usage, defaults: Usage) {
    let date = [Field::Weekday, Field::Year, Field::Month, Field::Day];
    let clock = [
        Field::Period,
        Field::Hour,
        Field::Minute,
        Field::Second,
        Field::Fraction,
    ];
    let includes_date = !matches!(required, Usage::Time);
    let includes_clock = !matches!(required, Usage::Date);
    let any = |group: &[Field]| group.iter().any(|&field| fields[field as usize].is_some());
    if (includes_date && any(&date)) || (includes_clock && any(&clock)) {
        return;
    }
    if !matches!(defaults, Usage::Time) {
        for field in [Field::Year, Field::Month, Field::Day] {
            fields[field as usize] = Some(Width::Numeric);
        }
    }
    if !matches!(defaults, Usage::Date) {
        for field in [Field::Hour, Field::Minute, Field::Second] {
            fields[field as usize] = Some(Width::Numeric);
        }
    }
}

fn style_pattern(row: u32, date: Option<usize>, clock: Option<usize>, cycle: char) -> Vec<Token> {
    let data = patterns::data();
    let date_pattern = date.map(|style| {
        patterns::parse(data.value(row, style as u32).unwrap_or("y-MM-dd")).unwrap_or_default()
    });
    let time_pattern = clock.map(|style| {
        let mut tokens = patterns::parse(data.value(row, 4 + style as u32).unwrap_or("HH:mm:ss"))
            .unwrap_or_default();
        patterns::clock(&mut tokens, cycle, false);
        tokens
    });
    match (date_pattern, time_pattern) {
        (Some(date_pattern), Some(time_pattern)) => {
            let key = 8 + date.unwrap_or(2) as u32;
            patterns::join(
                data.value(row, key).unwrap_or("{1}, {0}"),
                [time_pattern, date_pattern],
                Token::Literal,
            )
        }
        (Some(pattern), None) | (None, Some(pattern)) => pattern,
        (None, None) => Vec::new(),
    }
}

fn component_pattern(row: u32, requested: Fields, cycle: char, best: bool) -> Vec<Token> {
    let mut date = requested;
    let mut time = requested;
    date[Field::Period as usize..].fill(None);
    time[..Field::Period as usize].fill(None);
    let date = patterns::components(row, &date, cycle, best);
    let time = patterns::components(row, &time, cycle, best);
    if date.is_empty() {
        return time;
    }
    if time.is_empty() {
        return date;
    }
    let names = requested[Field::Month as usize];
    let key = match names {
        Some(Width::Long) if requested[Field::Weekday as usize].is_some() => 16,
        Some(Width::Long) => 17,
        _ => 18,
    };
    let join = patterns::data().value(row, key).unwrap_or("{1}, {0}");
    patterns::join(join, [time, date], Token::Literal)
}

fn construct(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let default = agent.intrinsics.intl[slot::DATE_TIME_FORMAT_PROTOTYPE];
    let prototype = prototype_from_new_target(agent, default)?;
    let state = create(agent, arg(args, 0), arg(args, 1), Usage::Both, Usage::Date)?;
    let bytes = state.locale.capacity()
        + state.calendar.capacity()
        + state.pattern.capacity() * size_of::<Token>()
        + state
            .pattern
            .iter()
            .map(|token| match token {
                Token::Literal(text) => text.capacity(),
                _ => 0,
            })
            .sum::<usize>();
    let object = new_service(agent, prototype, state, bytes)?;
    service::legacy_result(agent, this, object, slot::DATE_TIME_FORMAT)
}

fn supported_locales(agent: &mut Agent, _: Value, args: &[Value]) -> JsResult<Value> {
    service::supported_locales(agent, &patterns::data().locales, arg(args, 0), arg(args, 1))
}

fn this_format(agent: &mut Agent, this: Value) -> JsResult<Arc<DateTimeFormat>> {
    match service::service(agent, this) {
        Some(state) => Ok(state),
        None => agent.type_error("expected an Intl.DateTimeFormat"),
    }
}

fn unwrap(agent: &mut Agent, this: Value) -> JsResult<(Value, Arc<DateTimeFormat>)> {
    let value = service::unwrap_legacy::<DateTimeFormat>(agent, this, slot::DATE_TIME_FORMAT)?;
    Ok((value, this_format(agent, value)?))
}

fn format_getter(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    let (owner, _) = unwrap(agent, this)?;
    let object = owner.as_object().expect("date formatter object");
    let mut bound = service::bound(agent, object);
    if bound == Value::UNDEFINED {
        bound = Value::object(service::closure(agent, bound_format, 1, owner)?);
        service::set_bound(agent, object, bound);
    }
    Ok(bound)
}

fn clip(agent: &mut Agent, time: f64) -> JsResult<f64> {
    if !time.is_finite() || time.abs() > date::MAX_TIME {
        return range_error(agent, "invalid time value");
    }
    Ok(time.trunc())
}

fn time_argument(agent: &mut Agent, value: Value) -> JsResult<f64> {
    let time = if value == Value::UNDEFINED {
        date::now(agent)
    } else {
        agent.to_number(value)?
    };
    clip(agent, time)
}

fn bound_format(agent: &mut Agent, _: Value, args: &[Value]) -> JsResult<Value> {
    let owner = service::captured(agent);
    let state = this_format(agent, owner)?;
    let date = time_argument(agent, arg(args, 0))?;
    let parts = state.partition(agent, date)?;
    agent.string(&numfmt::text_of(&parts))
}

fn format_to_parts(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let state = this_format(agent, this)?;
    let date = time_argument(agent, arg(args, 0))?;
    let parts = state.partition(agent, date)?;
    parts_array(agent, &parts, false)
}

fn parts_array(agent: &mut Agent, parts: &[Part], range: bool) -> JsResult<Value> {
    agent.heap.charge(parts.len() * size_of::<Value>())?;
    let mut items = Vec::with_capacity(parts.len());
    for part in parts {
        let item = agent.new_object()?;
        set_text(agent, item, "type", part.kind)?;
        set_text(agent, item, "value", &part.text)?;
        if range {
            set_text(agent, item, "source", part.source.name())?;
        }
        items.push(Value::object(item));
    }
    Ok(Value::object(agent.create_array_from_list(&items)?))
}

fn ranged(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Vec<Part>> {
    let state = this_format(agent, this)?;
    let from = arg(args, 0);
    let to = arg(args, 1);
    if from == Value::UNDEFINED || to == Value::UNDEFINED {
        return agent.type_error("DateTimeFormat ranges require both endpoints");
    }
    let begin = agent.to_number(from)?;
    let end = agent.to_number(to)?;
    let begin = clip(agent, begin)?;
    let end = clip(agent, end)?;
    let mut first = state.partition(agent, begin)?;
    let mut last = state.partition(agent, end)?;
    if first == last {
        return Ok(first);
    }
    if let Some(parts) = state.format_interval(agent, begin, end, &first, &last)? {
        return Ok(parts);
    }
    for part in &mut first {
        part.source = Source::Start;
    }
    for part in &mut last {
        part.source = Source::End;
    }
    let template = patterns::data()
        .value(state.row, patterns::RANGE)
        .unwrap_or("{0} – {1}");
    // CLDR fallbacks contain both placeholders once and may order them
    // differently. Use the same literal/template handling as date+time.
    let parts = patterns::join(template, [first, last], |text| Part {
        kind: "literal",
        text,
        source: Source::Shared,
    });
    charge_parts(agent, &parts)?;
    Ok(parts)
}

fn format_range(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let parts = ranged(agent, this, args)?;
    agent.string(&numfmt::text_of(&parts))
}

fn format_range_to_parts(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let parts = ranged(agent, this, args)?;
    parts_array(agent, &parts, true)
}

fn resolved_options(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    let (_, state) = unwrap(agent, this)?;
    let object = agent.new_object()?;
    set_text(agent, object, "locale", &state.locale)?;
    set_text(agent, object, "calendar", &state.calendar)?;
    set_text(
        agent,
        object,
        "numberingSystem",
        numbers().numbering_system_name(state.system),
    )?;
    set_text(agent, object, "timeZone", &state.zone.name())?;
    if state.fields[Field::Hour as usize].is_some() {
        set_text(agent, object, "hourCycle", CYCLES[state.cycle])?;
        set(agent, object, "hour12", Value::boolean(state.cycle < 2))?;
    }
    let styled = state.date_style.is_some() || state.time_style.is_some();
    if !styled {
        for field in Field::ALL {
            match state.fields[field as usize] {
                Some(Width::Fraction(length)) => set(
                    agent,
                    object,
                    field.property(),
                    Value::number(f64::from(length)),
                )?,
                Some(width) => set_text(agent, object, field.property(), width.name())?,
                None => {}
            }
        }
    }
    if let Some(style) = state.date_style {
        set_text(agent, object, "dateStyle", STYLES[style])?;
    }
    if let Some(style) = state.time_style {
        set_text(agent, object, "timeStyle", STYLES[style])?;
    }
    Ok(Value::object(object))
}

impl DateTimeFormat {
    fn digits(&self, ascii: &str) -> String {
        if numbers().numbering_system_name(self.system) == "latn" {
            return ascii.to_owned();
        }
        let digits: Vec<_> = numbers().digits(self.system).chars().collect();
        ascii
            .chars()
            .map(|c| {
                if c.is_ascii_digit() {
                    digits[(c as u8 - b'0') as usize]
                } else {
                    c
                }
            })
            .collect()
    }

    fn number(&self, value: i64, width: Width) -> String {
        let text = if width == Width::TwoDigit {
            format!("{value:02}")
        } else {
            value.to_string()
        };
        self.digits(&text)
    }

    fn named(&self, key: u32, fallback: &'static str) -> String {
        patterns::data()
            .value(self.row, key)
            .unwrap_or(fallback)
            .to_owned()
    }

    fn offset_name(&self, seconds: i64, long: bool) -> String {
        let data = patterns::data();
        if seconds == 0 {
            return self.named(patterns::GMT_ZERO, "GMT");
        }
        let (plus, minus) = data
            .value(self.row, patterns::HOUR_OFFSET)
            .unwrap_or("+HH:mm;-HH:mm")
            .split_once(';')
            .unwrap_or(("+HH:mm", "-HH:mm"));
        let template = if seconds < 0 { minus } else { plus };
        let magnitude = seconds.abs();
        let hour = magnitude / 3600;
        let minute = magnitude / 60 % 60;
        let second = magnitude % 60;
        let mut formatted = String::new();
        let mut letters = template.chars().peekable();
        while let Some(letter) = letters.next() {
            match letter {
                'H' => {
                    let padded = letters.next_if_eq(&'H').is_some() && long;
                    formatted.push_str(&self.number(
                        hour,
                        if padded {
                            Width::TwoDigit
                        } else {
                            Width::Numeric
                        },
                    ));
                }
                'm' => {
                    letters.next_if_eq(&'m');
                    if long || minute != 0 || second != 0 {
                        formatted.push_str(&self.number(minute, Width::TwoDigit));
                    }
                }
                ':' | '.'
                    if !long && minute == 0 && second == 0 && letters.peek() == Some(&'m') => {}
                literal => formatted.push(literal),
            }
        }
        if second != 0 {
            formatted.push(':');
            formatted.push_str(&self.number(second, Width::TwoDigit));
        }
        data.value(self.row, patterns::GMT)
            .unwrap_or("GMT{0}")
            .replace("{0}", &formatted)
    }

    fn localized_zone(&self, offset: &Offset, width: Width) -> String {
        if matches!(width, Width::ShortOffset | Width::LongOffset) {
            return self.offset_name(offset.seconds, width == Width::LongOffset);
        }
        let language = patterns::data().locales.text(self.row, 0);
        let english = language == "en" || language.starts_with("en-");
        if offset.seconds == 0 && matches!(offset.abbreviation, "UTC" | "GMT") {
            return if matches!(width, Width::Long | Width::LongGeneric) {
                self.named(patterns::UTC_LONG, "Coordinated Universal Time")
            } else if english {
                "UTC".to_owned()
            } else {
                self.named(patterns::GMT_ZERO, "UTC")
            };
        }
        if width == Width::Short
            && english
            && !offset.abbreviation.is_empty()
            && offset.abbreviation.bytes().all(|c| c.is_ascii_alphabetic())
        {
            return offset.abbreviation.to_owned();
        }
        self.offset_name(
            offset.seconds,
            matches!(width, Width::Long | Width::LongGeneric),
        )
    }

    fn calendar_fields(&self, time: f64, offset: &Offset) -> date::Fields {
        let time = time + offset.seconds as f64 * 1000.0;
        let mut fields = date::Fields::of(time);
        match self.calendar.as_str() {
            "buddhist" => fields.year += 543,
            "islamic-civil" => {
                let day = date::days_from_civil(fields.year, fields.month + 1, fields.date);
                // The Friday epoch is 622-07-19 in the proleptic Gregorian
                // calendar. The civil calendar alternates 30- and 29-day
                // months, with the intercalary day at the end of leap years.
                let elapsed = day - date::days_from_civil(622, 7, 19);
                let year = (30 * elapsed + 10_646).div_euclid(10_631);
                let year_start = (year - 1) * 354 + (3 + 11 * year).div_euclid(30);
                let within_year = elapsed - year_start;
                let month_start = |month: i64| (59 * month + 1).div_euclid(2);
                let month = (0..12)
                    .rev()
                    .find(|&month| month_start(month) <= within_year)
                    .expect("civil Islamic year starts at day zero");
                fields.year = year;
                fields.month = month;
                fields.date = within_year - month_start(month) + 1;
            }
            _ => {}
        }
        fields
    }

    fn format_interval(
        &self,
        agent: &mut Agent,
        start: f64,
        end: f64,
        first: &[Part],
        last: &[Part],
    ) -> JsResult<Option<Vec<Part>>> {
        let ordered = [
            Field::Era,
            Field::Year,
            Field::Month,
            Field::Day,
            Field::Weekday,
            Field::Period,
            Field::Hour,
            Field::Minute,
            Field::Second,
            Field::Fraction,
            Field::Zone,
        ];
        let Some(difference) = ordered.into_iter().find(|kind| {
            let first = first
                .iter()
                .find(|part| part.kind == kind.part())
                .map(|part| &part.text);
            let last = last
                .iter()
                .find(|part| part.kind == kind.part())
                .map(|part| &part.text);
            first != last
        }) else {
            return Ok(None);
        };
        let Some(pattern) = patterns::interval(
            self.row,
            &self.pattern,
            difference,
            CYCLE_LETTERS[self.cycle],
        ) else {
            return Ok(None);
        };
        let first_offset = self.zone.offset_at((start as i64).div_euclid(1000));
        let last_offset = self.zone.offset_at((end as i64).div_euclid(1000));
        let first_fields = self.calendar_fields(start, &first_offset);
        let last_fields = self.calendar_fields(end, &last_offset);
        let mut remaining = [0u8; 11];
        for token in &pattern {
            if let Token::Field { kind, .. } = token {
                remaining[*kind as usize] += 1;
            }
        }
        for kind in Field::ALL {
            let count = remaining[kind as usize];
            let a = first.iter().find(|part| part.kind == kind.part());
            let b = last.iter().find(|part| part.kind == kind.part());
            if count > 2 || (a != b && count < 2) {
                return Ok(None);
            }
        }
        let mut seen = [false; 11];
        let mut parts = Vec::with_capacity(pattern.len());
        for token in pattern {
            agent.check_native_work()?;
            let part = match token {
                Token::Literal(text) => Part {
                    kind: "literal",
                    text,
                    source: Source::Shared,
                },
                Token::Field {
                    kind,
                    letter,
                    width,
                } => {
                    let index = kind as usize;
                    let source = if remaining[index] == 1 {
                        Source::Shared
                    } else if seen[index] {
                        Source::End
                    } else {
                        Source::Start
                    };
                    seen[index] = true;
                    let (fields, offset) = if source == Source::End {
                        (&last_fields, &last_offset)
                    } else {
                        (&first_fields, &first_offset)
                    };
                    let text = self.render(kind, letter, width, fields, offset);
                    Part {
                        kind: kind.part(),
                        text,
                        source,
                    }
                }
            };
            parts.push(part);
        }
        let mut at = 1;
        while at + 1 < parts.len() {
            if parts[at].kind == "literal" && parts[at - 1].source == parts[at + 1].source {
                parts[at].source = parts[at - 1].source;
            }
            at += 1;
        }
        charge_parts(agent, &parts)?;
        Ok(Some(parts))
    }

    fn partition(&self, agent: &mut Agent, time: f64) -> JsResult<Vec<Part>> {
        let epoch_seconds = (time as i64).div_euclid(1000);
        let offset = self.zone.offset_at(epoch_seconds);
        let local = self.calendar_fields(time, &offset);
        let mut parts = Vec::with_capacity(self.pattern.len() + 1);
        for token in &self.pattern {
            agent.check_native_work()?;
            let (kind, text) = match token {
                Token::Literal(text) => ("literal", text.replace('\u{202f}', " ")),
                Token::Field {
                    kind,
                    width,
                    letter,
                } => {
                    if matches!(width, Width::Fraction(_))
                        && parts
                            .last()
                            .is_some_and(|part: &Part| part.kind == "second")
                    {
                        let separator =
                            numbers().symbol(self.number_row, self.system, Symbol::Decimal);
                        parts.push(Part {
                            kind: "literal",
                            text: separator.to_owned(),
                            source: Source::Shared,
                        });
                    }
                    (
                        kind.part(),
                        self.render(*kind, *letter, *width, &local, &offset),
                    )
                }
            };
            if !text.is_empty() {
                parts.push(Part {
                    kind,
                    text,
                    source: Source::Shared,
                });
            }
        }
        charge_parts(agent, &parts)?;
        Ok(parts)
    }

    fn render(
        &self,
        field: Field,
        letter: char,
        width: Width,
        local: &date::Fields,
        offset: &Offset,
    ) -> String {
        let width_index = width.text_index();
        match field {
            Field::Year => {
                let mut year = local.year;
                if year <= 0 {
                    year = 1 - year;
                }
                if width == Width::TwoDigit {
                    year %= 100;
                }
                self.number(year, width)
            }
            Field::Month => {
                if matches!(width, Width::Numeric | Width::TwoDigit) {
                    self.number(local.month + 1, width)
                } else {
                    let base = if self.calendar == "islamic-civil" {
                        patterns::ISLAMIC_MONTHS
                    } else {
                        patterns::MONTHS
                    };
                    let index = base + 12 * width_index + local.month as u32;
                    let standalone = if letter == 'L' { 36 } else { 0 };
                    patterns::data()
                        .value(self.row, index + standalone)
                        .or_else(|| patterns::data().value(self.row, index))
                        .map_or_else(
                            || self.number(local.month + 1, Width::Numeric),
                            str::to_owned,
                        )
                }
            }
            Field::Weekday => self.named(
                patterns::WEEKDAYS + 7 * width_index + local.weekday as u32,
                "",
            ),
            Field::Era if self.calendar == "buddhist" => {
                self.named(patterns::BUDDHIST_ERAS + width_index, "BE")
            }
            Field::Era if self.calendar == "islamic-civil" => {
                if local.year > 0 {
                    self.named(patterns::ISLAMIC_ERAS + width_index, "AH")
                } else {
                    "BH".to_owned()
                }
            }
            Field::Era => self.named(
                patterns::ERAS + 2 * width_index + u32::from(local.year > 0),
                "",
            ),
            Field::Day => self.number(local.date, width),
            Field::Hour => {
                let hour = match letter {
                    'K' => local.hour % 12,
                    'h' => (local.hour + 11) % 12 + 1,
                    'k' => (local.hour + 23) % 24 + 1,
                    _ => local.hour,
                };
                self.number(hour, width)
            }
            Field::Minute => self.number(local.minute, width),
            Field::Second => self.number(local.second, width),
            Field::Fraction => {
                let Width::Fraction(length) = width else {
                    unreachable!("fraction width")
                };
                let length = usize::from(length);
                let digits = format!("{:03}", local.ms);
                self.digits(&digits[..length])
            }
            Field::Period => {
                let first = u32::from(local.hour >= 12);
                let minute = (local.hour * 60 + local.minute) as u32;
                let exact = local.second == 0 && local.ms == 0;
                let locale = patterns::data().locales.text(self.row, 0);
                let [language, ..] = locale::base_fields(locale);
                let period = match letter {
                    'B' => patterns::data().period(language, minute, exact),
                    'b' if exact && minute == 0 => 2,
                    'b' if exact && minute == 720 => 3,
                    _ => first,
                };
                let base = patterns::PERIODS + 12 * width_index;
                patterns::data()
                    .value(self.row, base + period)
                    .or_else(|| patterns::data().value(self.row, patterns::PERIODS + period))
                    .or_else(|| patterns::data().value(self.row, base + first))
                    .unwrap_or(if first == 0 { "AM" } else { "PM" })
                    .to_owned()
            }
            Field::Zone => self.localized_zone(offset, width),
        }
    }
}

pub(in super::super) fn date_locale_string(
    agent: &mut Agent,
    time: f64,
    args: &[Value],
    usage: Usage,
) -> JsResult<Value> {
    if time.is_nan() {
        return agent.string("Invalid Date");
    }
    let state = create(agent, arg(args, 0), arg(args, 1), usage, usage)?;
    let parts = state.partition(agent, time)?;
    agent.string(&numfmt::text_of(&parts))
}
