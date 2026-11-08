//! Background values and the `background` shorthand (CSS Backgrounds and
//! Borders Level 3 §3, Candidate Recommendation reviewed 2026-11-08), plus the
//! `<position>` type they use (CSS Values 4 §9).
//!
//! Every background longhand except `background-color` is a comma-separated
//! list with one entry per layer. Images are `none` or `url()`; gradients and
//! other `<image>` functions are not supported yet.

use std::fmt;

use crate::properties::{ident, keywords, length_percentage, nonnegative};
use crate::serialize::serialize_string;
use crate::{Calc, Color, ComponentValue, LengthPercentage, PropertyValue, Token, Value};

/// One `background-image` layer.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Image {
    /// `none`.
    None,
    /// `url(...)`, unresolved (relative URLs resolve once stylesheets know
    /// their base URL; see TD-26).
    Url(String),
}

impl fmt::Display for Image {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::None => f.write_str("none"),
            Self::Url(url) => {
                f.write_str("url(")?;
                serialize_string(f, url)?;
                f.write_str(")")
            }
        }
    }
}

/// The side a `<position>` component is measured from.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PositionSide {
    /// `left`.
    Left,
    /// `right`.
    Right,
    /// `top`.
    Top,
    /// `bottom`.
    Bottom,
}

impl PositionSide {
    fn name(self) -> &'static str {
        match self {
            Self::Left => "left",
            Self::Right => "right",
            Self::Top => "top",
            Self::Bottom => "bottom",
        }
    }

    fn horizontal(self) -> bool {
        matches!(self, Self::Left | Self::Right)
    }
}

/// One axis of a `<position>`.
#[derive(Clone, Debug, PartialEq)]
pub enum PositionComponent {
    /// `center`.
    Center,
    /// A side keyword with an optional offset from it.
    Side(PositionSide, Option<LengthPercentage>),
    /// An offset from the left or top edge. Computed positions always use
    /// this form.
    Offset(LengthPercentage),
}

impl PositionComponent {
    /// The offset from the left/top edge (`right 10px` is
    /// `calc(100% - 10px)`).
    pub fn to_offset(&self) -> LengthPercentage {
        match self {
            Self::Center => LengthPercentage::Percentage(50.0),
            Self::Offset(lp) => lp.clone(),
            Self::Side(PositionSide::Left | PositionSide::Top, offset) => {
                offset.clone().unwrap_or(LengthPercentage::Percentage(0.0))
            }
            Self::Side(PositionSide::Right | PositionSide::Bottom, offset) => match offset {
                None => LengthPercentage::Percentage(100.0),
                Some(offset) => Calc::hundred_percent_minus(offset),
            },
        }
    }
}

impl fmt::Display for PositionComponent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Center => f.write_str("center"),
            Self::Side(side, None) => f.write_str(side.name()),
            Self::Side(side, Some(offset)) => write!(f, "{} {offset}", side.name()),
            Self::Offset(lp) => write!(f, "{lp}"),
        }
    }
}

/// A `<position>` (as `<bg-position>`): a horizontal and a vertical component.
#[derive(Clone, Debug, PartialEq)]
pub struct PositionValue {
    /// The horizontal component.
    pub x: PositionComponent,
    /// The vertical component.
    pub y: PositionComponent,
}

impl PositionValue {
    /// `0% 0%`, the initial `background-position`.
    pub fn top_left() -> Self {
        Self {
            x: PositionComponent::Offset(LengthPercentage::Percentage(0.0)),
            y: PositionComponent::Offset(LengthPercentage::Percentage(0.0)),
        }
    }
}

impl fmt::Display for PositionValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} {}", self.x, self.y)
    }
}

/// A position token: a keyword or a length-percentage.
enum Item {
    Keyword(Option<PositionSide>),
    Offset(LengthPercentage),
}

fn item(value: &Value) -> Option<Item> {
    if let Some(name) = ident(value) {
        return Some(Item::Keyword(Some(
            match name.to_ascii_lowercase().as_str() {
                "center" => return Some(Item::Keyword(None)),
                "left" => PositionSide::Left,
                "right" => PositionSide::Right,
                "top" => PositionSide::Top,
                "bottom" => PositionSide::Bottom,
                _ => return None,
            },
        )));
    }
    length_percentage(value).map(Item::Offset)
}

/// Whether a component may be horizontal (`vertical` false) or vertical.
fn fits(component: &PositionComponent, vertical: bool) -> bool {
    match component {
        PositionComponent::Side(side, _) => side.horizontal() != vertical,
        _ => true,
    }
}

/// Parse a `<bg-position>` that spans exactly `parts` (1 to 4 values).
pub(crate) fn position(parts: &[&Value]) -> Option<PositionValue> {
    let items: Vec<Item> = parts.iter().map(|v| item(v)).collect::<Option<_>>()?;
    let component = |item: &Item| match item {
        Item::Keyword(None) => PositionComponent::Center,
        Item::Keyword(Some(side)) => PositionComponent::Side(*side, None),
        Item::Offset(lp) => PositionComponent::Offset(lp.clone()),
    };
    let (x, y) = match items.as_slice() {
        [one] => {
            let c = component(one);
            match c {
                PositionComponent::Side(side, _) if !side.horizontal() => {
                    (PositionComponent::Center, c)
                }
                _ => (c, PositionComponent::Center),
            }
        }
        [a, b] => {
            let (mut x, mut y) = (component(a), component(b));
            // Two keywords may come in either order.
            if matches!((a, b), (Item::Keyword(_), Item::Keyword(_)))
                && (!fits(&x, false) || !fits(&y, true))
            {
                std::mem::swap(&mut x, &mut y);
            }
            (x, y)
        }
        [_, _, _] | [_, _, _, _] => {
            // `[center | side offset?] && [center | side offset?]`.
            let mut groups = Vec::with_capacity(2);
            let mut i = 0;
            while i < items.len() {
                let Item::Keyword(keyword) = &items[i] else {
                    return None;
                };
                let offset = match (keyword, items.get(i + 1)) {
                    (Some(_), Some(Item::Offset(lp))) => Some(lp.clone()),
                    _ => None,
                };
                i += if offset.is_some() { 2 } else { 1 };
                groups.push(match keyword {
                    None => PositionComponent::Center,
                    Some(side) => PositionComponent::Side(*side, offset),
                });
            }
            let [mut x, mut y]: [PositionComponent; 2] = groups.try_into().ok()?;
            if !fits(&x, false) || !fits(&y, true) {
                std::mem::swap(&mut x, &mut y);
            }
            (x, y)
        }
        _ => return None,
    };
    (fits(&x, false) && fits(&y, true)).then_some(PositionValue { x, y })
}

/// `background-size` for one layer.
#[derive(Clone, Debug, PartialEq)]
pub enum BackgroundSize {
    /// `cover`.
    Cover,
    /// `contain`.
    Contain,
    /// A width and height, each `auto` (`None`) or a nonnegative
    /// length-percentage.
    Explicit(Option<LengthPercentage>, Option<LengthPercentage>),
}

impl BackgroundSize {
    /// `auto auto`, the initial value.
    pub const AUTO: Self = Self::Explicit(None, None);
}

impl fmt::Display for BackgroundSize {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let side = |f: &mut fmt::Formatter<'_>, v: &Option<LengthPercentage>| match v {
            None => f.write_str("auto"),
            Some(lp) => write!(f, "{lp}"),
        };
        match self {
            Self::Cover => f.write_str("cover"),
            Self::Contain => f.write_str("contain"),
            Self::Explicit(w, h) => {
                side(f, w)?;
                // A single value means `<w> auto`.
                if h.is_some() {
                    f.write_str(" ")?;
                    side(f, h)?;
                }
                Ok(())
            }
        }
    }
}

fn size_side(value: &Value) -> Option<Option<LengthPercentage>> {
    if ident(value).is_some_and(|i| i.eq_ignore_ascii_case("auto")) {
        return Some(None);
    }
    nonnegative(value).map(Some)
}

/// Parse a `<bg-size>` that spans exactly `parts`.
pub(crate) fn size(parts: &[&Value]) -> Option<BackgroundSize> {
    match parts {
        [one] => match ident(one).map(str::to_ascii_lowercase).as_deref() {
            Some("cover") => Some(BackgroundSize::Cover),
            Some("contain") => Some(BackgroundSize::Contain),
            _ => Some(BackgroundSize::Explicit(size_side(one)?, None)),
        },
        // `<w> auto` is the same value as `<w>`, and serializes as it.
        [w, h] => Some(BackgroundSize::Explicit(size_side(w)?, size_side(h)?)),
        _ => None,
    }
}

keywords! {
    /// One axis of a `<repeat-style>`.
    RepeatStyle {
        Repeat => "repeat",
        Space => "space",
        Round => "round",
        NoRepeat => "no-repeat",
    }
}

/// `background-repeat` for one layer.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BackgroundRepeat {
    /// Horizontal repetition.
    pub x: RepeatStyle,
    /// Vertical repetition.
    pub y: RepeatStyle,
}

impl BackgroundRepeat {
    /// `repeat`, the initial value.
    pub const REPEAT: Self = Self {
        x: RepeatStyle::Repeat,
        y: RepeatStyle::Repeat,
    };
}

impl fmt::Display for BackgroundRepeat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match (self.x, self.y) {
            (x, y) if x == y => f.write_str(x.name()),
            (RepeatStyle::Repeat, RepeatStyle::NoRepeat) => f.write_str("repeat-x"),
            (RepeatStyle::NoRepeat, RepeatStyle::Repeat) => f.write_str("repeat-y"),
            (x, y) => write!(f, "{x} {y}"),
        }
    }
}

/// Parse a `<repeat-style>` that spans exactly `parts`.
pub(crate) fn repeat(parts: &[&Value]) -> Option<BackgroundRepeat> {
    match parts {
        [one] => {
            let name = ident(one)?.to_ascii_lowercase();
            match name.as_str() {
                "repeat-x" => Some(BackgroundRepeat {
                    x: RepeatStyle::Repeat,
                    y: RepeatStyle::NoRepeat,
                }),
                "repeat-y" => Some(BackgroundRepeat {
                    x: RepeatStyle::NoRepeat,
                    y: RepeatStyle::Repeat,
                }),
                _ => {
                    let style = RepeatStyle::from_name(&name)?;
                    Some(BackgroundRepeat { x: style, y: style })
                }
            }
        }
        [x, y] => Some(BackgroundRepeat {
            x: RepeatStyle::from_name(ident(x)?)?,
            y: RepeatStyle::from_name(ident(y)?)?,
        }),
        _ => None,
    }
}

keywords! {
    /// `background-attachment` for one layer.
    Attachment {
        Scroll => "scroll",
        Fixed => "fixed",
        Local => "local",
    }
}

keywords! {
    /// A `<visual-box>`, for `background-origin` and `background-clip`.
    VisualBox {
        BorderBox => "border-box",
        PaddingBox => "padding-box",
        ContentBox => "content-box",
    }
}

/// Parse an `<image>` (`none` or a URL) from one value.
pub(crate) fn image(value: &Value) -> Option<Image> {
    match &value.value {
        ComponentValue::Token(Token::Ident(name)) if name.eq_ignore_ascii_case("none") => {
            Some(Image::None)
        }
        ComponentValue::Token(Token::Url(url)) => Some(Image::Url(url.clone())),
        ComponentValue::Function { name, arguments } if name.eq_ignore_ascii_case("url") => {
            let mut args = arguments.iter().filter(|v| !v.is_whitespace());
            match (args.next().and_then(Value::token), args.next()) {
                (Some(Token::String(url)), None) => Some(Image::Url(url.clone())),
                _ => None,
            }
        }
        _ => None,
    }
}

/// Split a value list into its comma-separated entries, rejecting empty ones.
pub(crate) fn layers<'a, 'b>(parts: &'b [&'a Value]) -> Option<Vec<&'b [&'a Value]>> {
    let pieces: Vec<&[&Value]> = parts.split(|v| v.token() == Some(&Token::Comma)).collect();
    pieces.iter().all(|p| !p.is_empty()).then_some(pieces)
}

/// Parse a list-valued background longhand with `entry` per layer.
pub(crate) fn list<T>(parts: &[&Value], entry: impl Fn(&[&Value]) -> Option<T>) -> Option<Vec<T>> {
    layers(parts)?.into_iter().map(entry).collect()
}

/// One parsed layer of the `background` shorthand.
struct Layer {
    image: Option<Image>,
    position: Option<PositionValue>,
    size: Option<BackgroundSize>,
    repeat: Option<BackgroundRepeat>,
    attachment: Option<Attachment>,
    origin: Option<VisualBox>,
    clip: Option<VisualBox>,
    color: Option<Color>,
}

fn layer(parts: &[&Value], last: bool) -> Option<Layer> {
    let mut l = Layer {
        image: None,
        position: None,
        size: None,
        repeat: None,
        attachment: None,
        origin: None,
        clip: None,
        color: None,
    };
    let mut i = 0;
    'next: while i < parts.len() {
        let rest = &parts[i..];
        if l.image.is_none() {
            if let Some(img) = image(rest[0]) {
                l.image = Some(img);
                i += 1;
                continue;
            }
        }
        if l.position.is_none() {
            for n in (1..=rest.len().min(4)).rev() {
                let Some(pos) = position(&rest[..n]) else {
                    continue;
                };
                l.position = Some(pos);
                i += n;
                if parts.get(i).and_then(|v| v.token()) == Some(&Token::Delim('/')) {
                    let after = &parts[i + 1..];
                    let (s, used) = [2, 1]
                        .into_iter()
                        .filter(|&n| n <= after.len())
                        .find_map(|n| size(&after[..n]).map(|s| (s, n)))?;
                    l.size = Some(s);
                    i += 1 + used;
                }
                continue 'next;
            }
        }
        if l.repeat.is_none() {
            for n in [2, 1] {
                if let Some(r) = rest.get(..n).and_then(repeat) {
                    l.repeat = Some(r);
                    i += n;
                    continue 'next;
                }
            }
        }
        if l.attachment.is_none() {
            if let Some(a) = ident(rest[0]).and_then(Attachment::from_name) {
                l.attachment = Some(a);
                i += 1;
                continue;
            }
        }
        if let Some(b) = ident(rest[0]).and_then(VisualBox::from_name) {
            if l.origin.is_none() {
                l.origin = Some(b);
            } else if l.clip.is_none() {
                l.clip = Some(b);
            } else {
                return None;
            }
            i += 1;
            continue;
        }
        if last && l.color.is_none() {
            if let Some(c) = Color::parse(rest[0]) {
                l.color = Some(c);
                i += 1;
                continue;
            }
        }
        return None;
    }
    Some(l)
}

/// Expand the `background` shorthand into its eight longhands, in the order
/// of [`crate::PropertyId`]'s background longhands: color, image, position,
/// size, repeat, attachment, origin, clip.
pub(crate) fn shorthand(parts: &[&Value]) -> Option<Vec<PropertyValue>> {
    let pieces = layers(parts)?;
    let count = pieces.len();
    let mut images = Vec::with_capacity(count);
    let mut positions = Vec::with_capacity(count);
    let mut sizes = Vec::with_capacity(count);
    let mut repeats = Vec::with_capacity(count);
    let mut attachments = Vec::with_capacity(count);
    let mut origins = Vec::with_capacity(count);
    let mut clips = Vec::with_capacity(count);
    let mut color = Color::named("transparent").expect("named color");
    for (index, piece) in pieces.into_iter().enumerate() {
        let l = layer(piece, index + 1 == count)?;
        images.push(l.image.unwrap_or(Image::None));
        positions.push(l.position.unwrap_or_else(PositionValue::top_left));
        sizes.push(l.size.unwrap_or(BackgroundSize::AUTO));
        repeats.push(l.repeat.unwrap_or(BackgroundRepeat::REPEAT));
        attachments.push(l.attachment.unwrap_or(Attachment::Scroll));
        // One box sets both origin and clip.
        origins.push(l.origin.unwrap_or(VisualBox::PaddingBox));
        clips.push(l.clip.or(l.origin).unwrap_or(VisualBox::BorderBox));
        if let Some(c) = l.color {
            color = c;
        }
    }
    Some(vec![
        PropertyValue::Color(color),
        PropertyValue::BackgroundImage(images),
        PropertyValue::BackgroundPosition(positions),
        PropertyValue::BackgroundSize(sizes),
        PropertyValue::BackgroundRepeat(repeats),
        PropertyValue::BackgroundAttachment(attachments),
        PropertyValue::BackgroundBox(origins),
        PropertyValue::BackgroundBox(clips),
    ])
}

/// Write `items` separated by `, `.
pub(crate) fn write_list<T: fmt::Display>(f: &mut fmt::Formatter<'_>, items: &[T]) -> fmt::Result {
    for (i, item) in items.iter().enumerate() {
        if i > 0 {
            f.write_str(", ")?;
        }
        write!(f, "{item}")?;
    }
    Ok(())
}
