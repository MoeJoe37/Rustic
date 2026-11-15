//! Intl.Segmenter (ECMA-402 §19): Segments objects and their iterators
//! over the boundaries `segment.rs` finds.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use super::super::{
    arg, install_methods, iterator_result, method, new_constructor, prototype_from_new_target,
};
use super::number_data::data as locale_data;
use super::segment::{self, Properties};
use super::service::{
    self, Resolution, best_fit, bound, choice, new_service, options_object, set, set_bound,
    set_text,
};
use super::{canonicalize_locale_list, slot};
use crate::runtime::{
    Agent, JsResult, JsStr, ObjectId, OutOfMemory, PropertyDescriptor, PropertyKey, StringId,
    Value, to_integer_or_infinity,
};

const GRANULARITIES: &[&str] = &["grapheme", "word", "sentence"];

struct Segmenter {
    locale: String,
    granularity: usize,
}

/// The boundaries of one string, found when it was segmented.
struct Segments {
    granularity: usize,
    /// The code unit offsets of the boundaries, from 0 to the length.
    boundaries: Vec<u32>,
    /// For word granularity: whether the segment that starts at the
    /// boundary of the same index is word-like.
    word_like: Vec<bool>,
}

/// A position in the boundaries of a [`Segments`].
struct SegmentIterator {
    segments: Arc<Segments>,
    next: AtomicUsize,
}

pub(super) fn install(agent: &mut Agent, intl: ObjectId) -> JsResult<()> {
    let prototype = agent.new_object()?;
    agent.intrinsics.intl[slot::SEGMENTER_PROTOTYPE] = prototype;
    let constructor = new_constructor(agent, "Segmenter", construct, 0, prototype)?;
    let key = agent.heap.key("Segmenter")?;
    agent.define_property_or_throw(
        intl,
        key,
        PropertyDescriptor::data_with(Value::object(constructor), true, false, true),
    )?;
    let tag_key = PropertyKey::Symbol(agent.symbols.to_string_tag);
    let tag = agent.string("Intl.Segmenter")?;
    agent.define_property_or_throw(
        prototype,
        tag_key,
        PropertyDescriptor::data_with(tag, false, false, true),
    )?;
    install_methods(
        agent,
        constructor,
        &[method("supportedLocalesOf", supported_locales, 1)],
    )?;
    install_methods(
        agent,
        prototype,
        &[
            method("resolvedOptions", resolved_options, 0),
            method("segment", segment_string, 1),
        ],
    )?;

    // %IntlSegmentsPrototype% (§19.5.2).
    let segments = agent.new_object()?;
    agent.intrinsics.intl[slot::SEGMENTS_PROTOTYPE] = segments;
    install_methods(agent, segments, &[method("containing", containing, 1)])?;
    let iterator_key = PropertyKey::Symbol(agent.symbols.iterator);
    agent.define_method(segments, iterator_key, iterate, "[Symbol.iterator]", 0)?;

    // %IntlSegmentIteratorPrototype% (§19.6.2).
    let iterator_prototype = agent
        .get_prototype_of(agent.intrinsics.array_iterator_prototype)?
        .expect("%ArrayIteratorPrototype% inherits from %IteratorPrototype%");
    let iterator = agent.new_object_with(Some(iterator_prototype))?;
    agent.intrinsics.intl[slot::SEGMENT_ITERATOR_PROTOTYPE] = iterator;
    install_methods(agent, iterator, &[method("next", next, 0)])?;
    let tag = agent.string("Segmenter String Iterator")?;
    agent.define_property_or_throw(
        iterator,
        tag_key,
        PropertyDescriptor::data_with(tag, false, false, true),
    )
}

fn construct(agent: &mut Agent, _: Value, args: &[Value]) -> JsResult<Value> {
    if agent.new_target().is_none() {
        return agent.type_error("Intl.Segmenter requires new");
    }
    let default = agent.intrinsics.intl[slot::SEGMENTER_PROTOTYPE];
    let prototype = prototype_from_new_target(agent, default)?;
    let requested = canonicalize_locale_list(agent, arg(args, 0))?;
    let options = options_object(agent, arg(args, 1))?;
    let best = best_fit(agent, options)?;
    let granularity = choice(agent, options, "granularity", GRANULARITIES, 0)?;
    // The rules are Unicode's for every locale; the locales are those
    // the other services have data for.
    let locale = Resolution::new(&locale_data().locales, &requested, best).locale();
    let state = Segmenter {
        locale,
        granularity,
    };
    let bytes = state.locale.len();
    Ok(Value::object(new_service(agent, prototype, state, bytes)?))
}

fn supported_locales(agent: &mut Agent, _: Value, args: &[Value]) -> JsResult<Value> {
    service::supported_locales(agent, &locale_data().locales, arg(args, 0), arg(args, 1))
}

fn resolved_options(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    let Some(state) = service::service::<Segmenter>(agent, this) else {
        return agent.type_error("expected an Intl.Segmenter");
    };
    let object = agent.new_object()?;
    set_text(agent, object, "locale", &state.locale)?;
    set_text(
        agent,
        object,
        "granularity",
        GRANULARITIES[state.granularity],
    )?;
    Ok(Value::object(object))
}

/// The boundaries of `string` at `granularity`, as code unit offsets.
fn find_boundaries(agent: &mut Agent, string: StringId, granularity: usize) -> JsResult<Segments> {
    let length = agent.heap.string_len(string);
    // Code points, their offsets, flags and the boundary list.
    agent
        .heap
        .charge(length.checked_mul(14).ok_or(OutOfMemory)?)?;
    let units = agent.heap.string(string).to_units();
    let mut text: Vec<Properties> = Vec::with_capacity(length);
    let mut offsets: Vec<u32> = Vec::with_capacity(length + 1);
    let mut at = 0;
    while at < units.len() {
        agent.check_native_work()?;
        offsets.push(at as u32);
        let unit = u32::from(units[at]);
        let low = units.get(at + 1).map_or(0, |&low| u32::from(low));
        if (0xD800..0xDC00).contains(&unit) && (0xDC00..0xE000).contains(&low) {
            text.push(segment::properties(
                0x10000 + ((unit - 0xD800) << 10) + (low - 0xDC00),
            ));
            at += 2;
        } else {
            text.push(segment::properties(unit));
            at += 1;
        }
    }
    offsets.push(units.len() as u32);
    let flags = match granularity {
        0 => segment::grapheme_boundaries(&text),
        1 => segment::word_boundaries(&text),
        _ => segment::sentence_boundaries(&text),
    };
    let mut boundaries = Vec::new();
    let mut word_like = Vec::new();
    for (index, flag) in flags.iter().enumerate() {
        if *flag {
            boundaries.push(offsets[index]);
            word_like.push(false);
        }
        let like = granularity == 1 && text.get(index).is_some_and(|code| code.word_like());
        if let Some(last) = word_like.last_mut().filter(|_| like) {
            *last = true;
        }
    }
    Ok(Segments {
        granularity,
        boundaries,
        word_like,
    })
}

/// `Intl.Segmenter.prototype.segment(string)` (§19.3.3).
fn segment_string(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let Some(state) = service::service::<Segmenter>(agent, this) else {
        return agent.type_error("expected an Intl.Segmenter");
    };
    let string = agent.to_string(arg(args, 0))?;
    let segments = find_boundaries(agent, string, state.granularity)?;
    let bytes = 5 * segments.boundaries.len();
    let prototype = agent.intrinsics.intl[slot::SEGMENTS_PROTOTYPE];
    let object = new_service(agent, prototype, Arc::new(segments), bytes)?;
    // The bound slot keeps the string alive.
    set_bound(agent, object, Value::string(string));
    Ok(Value::object(object))
}

fn this_segments(agent: &mut Agent, this: Value) -> JsResult<(Arc<Segments>, StringId)> {
    if let (Some(state), Some(object)) = (
        service::service::<Arc<Segments>>(agent, this),
        this.as_object(),
    ) {
        if let Some(string) = bound(agent, object).as_string() {
            return Ok((Arc::clone(&state), string));
        }
    }
    agent.type_error("expected a Segments object")
}

/// `CreateSegmentDataObject` (§19.7.1) for the segment that starts at
/// the boundary `index`.
fn segment_data(
    agent: &mut Agent,
    segments: &Segments,
    string: StringId,
    index: usize,
) -> JsResult<Value> {
    let start = segments.boundaries[index] as usize;
    let end = segments.boundaries[index + 1] as usize;
    agent.heap.charge(2 * (end - start))?;
    let units: Vec<u16> = agent
        .heap
        .string(string)
        .units()
        .skip(start)
        .take(end - start)
        .collect();
    let text = agent.string_from(JsStr::from_units(&units))?;
    let object = agent.new_object()?;
    set(agent, object, "segment", text)?;
    set(agent, object, "index", Value::number(start as f64))?;
    set(agent, object, "input", Value::string(string))?;
    if segments.granularity == 1 {
        let like = Value::boolean(segments.word_like[index]);
        set(agent, object, "isWordLike", like)?;
    }
    Ok(Value::object(object))
}

/// `%IntlSegmentsPrototype%.containing(index)` (§19.5.2.1).
fn containing(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let (segments, string) = this_segments(agent, this)?;
    let number = agent.to_number(arg(args, 0))?;
    let position = to_integer_or_infinity(number);
    let length = agent.heap.string_len(string);
    if position < 0.0 || position >= length as f64 {
        return Ok(Value::UNDEFINED);
    }
    let position = position as u32;
    // The last boundary at or before the position starts its segment.
    let index = segments
        .boundaries
        .partition_point(|&boundary| boundary <= position)
        - 1;
    segment_data(agent, &segments, string, index)
}

/// `%IntlSegmentsPrototype% [ %Symbol.iterator% ] ()` (§19.5.2.2).
fn iterate(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    let (segments, string) = this_segments(agent, this)?;
    let prototype = agent.intrinsics.intl[slot::SEGMENT_ITERATOR_PROTOTYPE];
    let state = SegmentIterator {
        segments,
        next: AtomicUsize::new(0),
    };
    let object = new_service(agent, prototype, state, 0)?;
    set_bound(agent, object, Value::string(string));
    Ok(Value::object(object))
}

/// `%IntlSegmentIteratorPrototype%.next()` (§19.6.2.1).
fn next(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    let state = service::service::<SegmentIterator>(agent, this);
    let string = this
        .as_object()
        .and_then(|object| bound(agent, object).as_string());
    let (Some(state), Some(string)) = (state, string) else {
        return agent.type_error("expected a Segmenter String Iterator");
    };
    let index = state.next.load(Ordering::Relaxed);
    if index + 1 >= state.segments.boundaries.len() {
        return iterator_result(agent, Value::UNDEFINED, true);
    }
    state.next.store(index + 1, Ordering::Relaxed);
    let data = segment_data(agent, &state.segments, string, index)?;
    iterator_result(agent, data, false)
}
