//! Grid properties (CSS Grid Layout 2 §7 grid definition and §8 placement,
//! Editor's Draft reviewed 2026-11-26) and their shorthands.
//!
//! Track lists keep their written structure (line names, `repeat()`) so they
//! serialize as declared; layout expands them. `subgrid` keeps its line name
//! list the same way; `masonry` is not parsed (TD-209). Line names are
//! `<custom-ident>`s other than `span` and `auto`.

use std::fmt;

use crate::color::write_number;
use crate::properties::{CssWideKeyword, ident, integer, nonnegative};
use crate::serialize::{serialize_identifier, serialize_string};
use crate::{BlockKind, ComponentValue, LengthPercentage, PropertyId, PropertyValue, Token, Value};

/// The largest `repeat()` count kept; larger counts clamp (CSS Grid 2 §7.2.3
/// lets implementations clamp, and layout clamps the whole grid further).
pub const MAX_REPEAT: u32 = 10_000;

/// `<track-breadth>`: `<length-percentage [0,∞]> | <flex [0,∞]> |
/// min-content | max-content | auto`.
#[derive(Clone, Debug, PartialEq)]
pub enum TrackBreadth {
    /// A nonnegative length or percentage.
    Length(LengthPercentage),
    /// A flexible length in `fr`.
    Flex(f64),
    /// `min-content`.
    MinContent,
    /// `max-content`.
    MaxContent,
    /// `auto`.
    Auto,
}

impl TrackBreadth {
    /// Whether this is a `<fixed-breadth>` (a length or percentage).
    pub fn is_fixed(&self) -> bool {
        matches!(self, Self::Length(_))
    }

    fn map(&self, f: &dyn Fn(&LengthPercentage) -> LengthPercentage) -> Self {
        match self {
            Self::Length(lp) => Self::Length(f(lp)),
            other => other.clone(),
        }
    }
}

impl fmt::Display for TrackBreadth {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Length(lp) => write!(f, "{lp}"),
            Self::Flex(n) => {
                write_number(f, *n)?;
                f.write_str("fr")
            }
            Self::MinContent => f.write_str("min-content"),
            Self::MaxContent => f.write_str("max-content"),
            Self::Auto => f.write_str("auto"),
        }
    }
}

/// `<track-size>`: a breadth, `minmax()` or `fit-content()`.
#[derive(Clone, Debug, PartialEq)]
pub enum TrackSize {
    /// A single breadth (both sizing functions).
    Breadth(TrackBreadth),
    /// `minmax(<inflexible-breadth>, <track-breadth>)`.
    MinMax(TrackBreadth, TrackBreadth),
    /// `fit-content(<length-percentage [0,∞]>)`.
    FitContent(LengthPercentage),
}

impl TrackSize {
    /// The `auto` track size (the initial `grid-auto-*` value).
    pub const AUTO: Self = Self::Breadth(TrackBreadth::Auto);

    /// The minimum and maximum track sizing functions (CSS Grid 2 §11.1):
    /// a flexible minimum is `auto`; `fit-content()` is `auto` /
    /// `max-content` clamped by its argument, which callers read separately.
    pub fn functions(&self) -> (TrackBreadth, TrackBreadth) {
        match self {
            Self::Breadth(TrackBreadth::Flex(n)) => (TrackBreadth::Auto, TrackBreadth::Flex(*n)),
            Self::Breadth(b) => (b.clone(), b.clone()),
            Self::MinMax(min, max) => (min.clone(), max.clone()),
            Self::FitContent(_) => (TrackBreadth::Auto, TrackBreadth::MaxContent),
        }
    }

    /// Whether this is a `<fixed-size>` (CSS Grid 2 §7.2.1).
    pub fn is_fixed(&self) -> bool {
        match self {
            Self::Breadth(b) => b.is_fixed(),
            Self::MinMax(min, max) => {
                min.is_fixed() || (max.is_fixed() && !matches!(min, TrackBreadth::Flex(_)))
            }
            Self::FitContent(_) => false,
        }
    }

    fn map(&self, f: &dyn Fn(&LengthPercentage) -> LengthPercentage) -> Self {
        match self {
            Self::Breadth(b) => Self::Breadth(b.map(f)),
            Self::MinMax(a, b) => Self::MinMax(a.map(f), b.map(f)),
            Self::FitContent(lp) => Self::FitContent(f(lp)),
        }
    }
}

impl fmt::Display for TrackSize {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Breadth(b) => write!(f, "{b}"),
            Self::MinMax(min, max) => write!(f, "minmax({min}, {max})"),
            Self::FitContent(lp) => write!(f, "fit-content({lp})"),
        }
    }
}

/// A `repeat()` count.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RepeatCount {
    /// A positive integer, clamped to [`MAX_REPEAT`].
    Count(u32),
    /// `auto-fill`.
    AutoFill,
    /// `auto-fit`.
    AutoFit,
}

impl fmt::Display for RepeatCount {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Count(n) => write!(f, "{n}"),
            Self::AutoFill => f.write_str("auto-fill"),
            Self::AutoFit => f.write_str("auto-fit"),
        }
    }
}

/// One component of a track list, in written order.
#[derive(Clone, Debug, PartialEq)]
pub enum TrackComponent {
    /// `[<custom-ident>*]`.
    Names(Vec<String>),
    /// A track size.
    Size(TrackSize),
    /// `repeat(<count>, <names and sizes>)`; the inner list has no repeats.
    Repeat(RepeatCount, Vec<TrackComponent>),
}

impl TrackComponent {
    fn map(&self, f: &dyn Fn(&LengthPercentage) -> LengthPercentage) -> Self {
        match self {
            Self::Names(n) => Self::Names(n.clone()),
            Self::Size(s) => Self::Size(s.map(f)),
            Self::Repeat(count, inner) => {
                Self::Repeat(*count, inner.iter().map(|c| c.map(f)).collect())
            }
        }
    }
}

fn write_names(f: &mut fmt::Formatter<'_>, names: &[String]) -> fmt::Result {
    f.write_str("[")?;
    for (i, name) in names.iter().enumerate() {
        if i > 0 {
            f.write_str(" ")?;
        }
        serialize_identifier(f, name)?;
    }
    f.write_str("]")
}

fn write_components(f: &mut fmt::Formatter<'_>, list: &[TrackComponent]) -> fmt::Result {
    for (i, component) in list.iter().enumerate() {
        if i > 0 {
            f.write_str(" ")?;
        }
        match component {
            TrackComponent::Names(names) => write_names(f, names)?,
            TrackComponent::Size(size) => write!(f, "{size}")?,
            TrackComponent::Repeat(count, inner) => {
                write!(f, "repeat({count}, ")?;
                write_components(f, inner)?;
                f.write_str(")")?;
            }
        }
    }
    Ok(())
}

/// `grid-template-rows`/`-columns`: `none | <track-list> | <auto-track-list>`.
#[derive(Clone, Debug, PartialEq)]
pub enum GridTemplate {
    /// `none`: no explicit tracks.
    None,
    /// A track list (at most one auto repeat, validated when parsed).
    Tracks(Vec<TrackComponent>),
    /// `subgrid <line-name-list>?` (CSS Grid 2 §9): the axis adopts the
    /// parent grid's tracks; the list names the subgrid's own lines.
    Subgrid(Vec<SubgridNames>),
}

/// One entry of a subgrid's `<line-name-list>`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SubgridNames {
    /// `[<custom-ident>*]`, one line (an empty set still counts).
    Names(Vec<String>),
    /// `repeat(<integer> | auto-fill, <line-names>+)`; `None` is
    /// `auto-fill`.
    Repeat(Option<u32>, Vec<Vec<String>>),
}

impl SubgridNames {
    fn fmt_list(f: &mut fmt::Formatter<'_>, list: &[Vec<String>]) -> fmt::Result {
        for (i, names) in list.iter().enumerate() {
            if i > 0 {
                f.write_str(" ")?;
            }
            write_names(f, names)?;
        }
        Ok(())
    }
}

impl fmt::Display for SubgridNames {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Names(names) => write_names(f, names),
            Self::Repeat(count, list) => {
                match count {
                    Some(n) => write!(f, "repeat({n}, ")?,
                    None => f.write_str("repeat(auto-fill, ")?,
                }
                Self::fmt_list(f, list)?;
                f.write_str(")")
            }
        }
    }
}

impl fmt::Display for GridTemplate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::None => f.write_str("none"),
            Self::Tracks(list) => write_components(f, list),
            Self::Subgrid(list) => {
                f.write_str("subgrid")?;
                for entry in list {
                    write!(f, " {entry}")?;
                }
                Ok(())
            }
        }
    }
}

/// A named grid area from `grid-template-areas`, as zero-based track ranges.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NamedArea {
    /// The area name.
    pub name: String,
    /// Rows `start..end`.
    pub rows: (usize, usize),
    /// Columns `start..end`.
    pub columns: (usize, usize),
}

/// `grid-template-areas` strings, validated as rectangles.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GridAreas {
    /// Cells per row (`None` is a null cell token).
    pub cells: Vec<Vec<Option<String>>>,
    /// Every named area, in first-appearance order.
    pub areas: Vec<NamedArea>,
}

impl GridAreas {
    /// Number of columns the strings define.
    pub fn columns(&self) -> usize {
        self.cells.first().map_or(0, Vec::len)
    }
}

impl fmt::Display for GridAreas {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (i, row) in self.cells.iter().enumerate() {
            if i > 0 {
                f.write_str(" ")?;
            }
            let text: Vec<&str> = row.iter().map(|c| c.as_deref().unwrap_or(".")).collect();
            serialize_string(f, &text.join(" "))?;
        }
        Ok(())
    }
}

/// `<grid-line>`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GridLine {
    /// `auto`.
    Auto,
    /// A line by number and/or name, or a span.
    Line {
        /// `span` was given.
        span: bool,
        /// The integer (never 0; positive for spans).
        integer: Option<i32>,
        /// The line or area name.
        name: Option<String>,
    },
}

impl GridLine {
    /// A bare `<custom-ident>`, which shorthands copy into omitted ends.
    pub fn ident(&self) -> Option<&str> {
        match self {
            Self::Line {
                span: false,
                integer: None,
                name: Some(name),
            } => Some(name),
            _ => None,
        }
    }
}

impl fmt::Display for GridLine {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let Self::Line {
            span,
            integer,
            name,
        } = self
        else {
            return f.write_str("auto");
        };
        let mut words = 0;
        if *span {
            f.write_str("span")?;
            words += 1;
        }
        // `span 1 <name>` serializes without the default count.
        if let Some(n) = integer.filter(|n| !(*span && *n == 1 && name.is_some())) {
            if words > 0 {
                f.write_str(" ")?;
            }
            write!(f, "{n}")?;
            words += 1;
        }
        if let Some(name) = name {
            if words > 0 {
                f.write_str(" ")?;
            }
            serialize_identifier(f, name)?;
        }
        Ok(())
    }
}

/// `grid-auto-flow`: `[row | column] || dense`.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct AutoFlow {
    /// `column` (otherwise `row`).
    pub column: bool,
    /// `dense` packing.
    pub dense: bool,
}

impl fmt::Display for AutoFlow {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match (self.column, self.dense) {
            (false, false) => "row",
            (true, false) => "column",
            (false, true) => "dense",
            (true, true) => "column dense",
        })
    }
}

/// A grid longhand's value.
#[derive(Clone, Debug, PartialEq)]
pub enum GridValue {
    /// `grid-template-rows`, `grid-template-columns`.
    Template(GridTemplate),
    /// `grid-template-areas` (`None` for `none`).
    Areas(Option<GridAreas>),
    /// `grid-auto-rows`, `grid-auto-columns`.
    AutoTracks(Vec<TrackSize>),
    /// `grid-auto-flow`.
    AutoFlow(AutoFlow),
    /// `grid-row-start`, `grid-row-end`, `grid-column-start`, `grid-column-end`.
    Line(GridLine),
}

impl GridValue {
    /// The value with `f` applied to every length (computed values).
    pub fn map_lengths(&self, f: &dyn Fn(&LengthPercentage) -> LengthPercentage) -> Self {
        match self {
            Self::Template(GridTemplate::Tracks(list)) => Self::Template(GridTemplate::Tracks(
                list.iter().map(|c| c.map(f)).collect(),
            )),
            Self::AutoTracks(list) => Self::AutoTracks(list.iter().map(|s| s.map(f)).collect()),
            other => other.clone(),
        }
    }
}

impl fmt::Display for GridValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Template(t) => write!(f, "{t}"),
            Self::Areas(None) => f.write_str("none"),
            Self::Areas(Some(a)) => write!(f, "{a}"),
            Self::AutoTracks(list) => {
                for (i, size) in list.iter().enumerate() {
                    if i > 0 {
                        f.write_str(" ")?;
                    }
                    write!(f, "{size}")?;
                }
                Ok(())
            }
            Self::AutoFlow(a) => write!(f, "{a}"),
            Self::Line(l) => write!(f, "{l}"),
        }
    }
}

/// The initial value of a grid longhand.
pub(crate) fn initial(property: PropertyId) -> Option<GridValue> {
    use PropertyId as P;
    Some(match property {
        P::GridTemplateRows | P::GridTemplateColumns => GridValue::Template(GridTemplate::None),
        P::GridTemplateAreas => GridValue::Areas(None),
        P::GridAutoRows | P::GridAutoColumns => GridValue::AutoTracks(vec![TrackSize::AUTO]),
        P::GridAutoFlow => GridValue::AutoFlow(AutoFlow::default()),
        P::GridRowStart | P::GridRowEnd | P::GridColumnStart | P::GridColumnEnd => {
            GridValue::Line(GridLine::Auto)
        }
        _ => return None,
    })
}

fn is_keyword(value: &Value, expected: &str) -> bool {
    ident(value).is_some_and(|i| i.eq_ignore_ascii_case(expected))
}

/// A `<custom-ident>` usable as a line name (not `span`, `auto`, a CSS-wide
/// keyword or `default`).
fn line_name(value: &Value) -> Option<String> {
    let name = ident(value)?;
    let reserved = ["span", "auto", "default"]
        .iter()
        .any(|r| name.eq_ignore_ascii_case(r))
        || CssWideKeyword::from_name(name).is_some();
    (!reserved).then(|| name.to_owned())
}

/// `[<custom-ident>*]`.
fn line_names(value: &Value) -> Option<Vec<String>> {
    let ComponentValue::Block {
        kind: BlockKind::Square,
        contents,
    } = &value.value
    else {
        return None;
    };
    contents
        .iter()
        .filter(|v| !v.is_whitespace())
        .map(line_name)
        .collect()
}

fn breadth(value: &Value) -> Option<TrackBreadth> {
    if let Some(Token::Dimension(n, unit)) = value.token() {
        if unit.eq_ignore_ascii_case("fr") {
            return (n.value >= 0.0).then_some(TrackBreadth::Flex(n.value));
        }
    }
    if let Some(name) = ident(value) {
        return match name.to_ascii_lowercase().as_str() {
            "min-content" => Some(TrackBreadth::MinContent),
            "max-content" => Some(TrackBreadth::MaxContent),
            "auto" => Some(TrackBreadth::Auto),
            _ => None,
        };
    }
    nonnegative(value).map(TrackBreadth::Length)
}

/// Function arguments split at commas, whitespace removed.
fn arguments<'a>(value: &'a Value, expected: &str) -> Option<Vec<Vec<&'a Value>>> {
    let ComponentValue::Function { name, arguments } = &value.value else {
        return None;
    };
    if !name.eq_ignore_ascii_case(expected) {
        return None;
    }
    let parts: Vec<&Value> = arguments.iter().filter(|v| !v.is_whitespace()).collect();
    Some(
        parts
            .split(|v| v.token() == Some(&Token::Comma))
            .map(<[&Value]>::to_vec)
            .collect(),
    )
}

/// `<track-size>`.
fn track_size(value: &Value) -> Option<TrackSize> {
    if let Some(args) = arguments(value, "minmax") {
        let [min, max] = args.as_slice() else {
            return None;
        };
        let ([min], [max]) = (min.as_slice(), max.as_slice()) else {
            return None;
        };
        let (min, max) = (breadth(min)?, breadth(max)?);
        if matches!(min, TrackBreadth::Flex(_)) {
            return None;
        }
        return Some(TrackSize::MinMax(min, max));
    }
    if let Some(args) = arguments(value, "fit-content") {
        let [limit] = args.as_slice() else {
            return None;
        };
        let [limit] = limit.as_slice() else {
            return None;
        };
        return nonnegative(limit).map(TrackSize::FitContent);
    }
    breadth(value).map(TrackSize::Breadth)
}

/// Names and track sizes without repeats; `None` if two name groups are
/// adjacent or there is no track.
fn simple_list(parts: &[&Value]) -> Option<Vec<TrackComponent>> {
    let mut out = Vec::new();
    let mut names_last = false;
    for v in parts {
        if let Some(names) = line_names(v) {
            if names_last {
                return None;
            }
            // `[]` names nothing and serializes away.
            if !names.is_empty() {
                out.push(TrackComponent::Names(names));
            }
            names_last = true;
        } else {
            out.push(TrackComponent::Size(track_size(v)?));
            names_last = false;
        }
    }
    out.iter()
        .any(|c| matches!(c, TrackComponent::Size(_)))
        .then_some(out)
}

fn repeat(value: &Value) -> Option<TrackComponent> {
    let args = arguments(value, "repeat")?;
    let [count, list] = args.as_slice() else {
        return None;
    };
    let [count] = count.as_slice() else {
        return None;
    };
    let count = if is_keyword(count, "auto-fill") {
        RepeatCount::AutoFill
    } else if is_keyword(count, "auto-fit") {
        RepeatCount::AutoFit
    } else {
        let n = integer(count)?;
        if n < 1 {
            return None;
        }
        RepeatCount::Count(u32::try_from(n).ok()?.min(MAX_REPEAT))
    };
    let inner = simple_list(list)?;
    if !matches!(count, RepeatCount::Count(_))
        && inner
            .iter()
            .any(|c| matches!(c, TrackComponent::Size(s) if !s.is_fixed()))
    {
        return None;
    }
    Some(TrackComponent::Repeat(count, inner))
}

/// `<track-list> | <auto-track-list>`, or `<explicit-track-list>` (no auto
/// repeat) when `explicit`.
fn track_list(parts: &[&Value], explicit: bool) -> Option<Vec<TrackComponent>> {
    let mut out = Vec::new();
    let mut names_last = false;
    for v in parts {
        if let Some(names) = line_names(v) {
            if names_last {
                return None;
            }
            if !names.is_empty() {
                out.push(TrackComponent::Names(names));
            }
            names_last = true;
            continue;
        }
        names_last = false;
        if matches!(&v.value, ComponentValue::Function { name, .. } if name.eq_ignore_ascii_case("repeat"))
        {
            out.push(repeat(v)?);
        } else {
            out.push(TrackComponent::Size(track_size(v)?));
        }
    }
    let autos = out
        .iter()
        .filter(|c| {
            matches!(
                c,
                TrackComponent::Repeat(RepeatCount::AutoFill | RepeatCount::AutoFit, _)
            )
        })
        .count();
    let tracks = out
        .iter()
        .filter(|c| !matches!(c, TrackComponent::Names(_)))
        .count();
    if tracks == 0 || autos > 1 || (explicit && autos > 0) {
        return None;
    }
    // With an auto repeat every other track must be a fixed size.
    if autos == 1 {
        let fixed = out.iter().all(|c| match c {
            TrackComponent::Size(s) => s.is_fixed(),
            TrackComponent::Repeat(_, inner) => inner.iter().all(|c| match c {
                TrackComponent::Size(s) => s.is_fixed(),
                _ => true,
            }),
            TrackComponent::Names(_) => true,
        });
        if !fixed {
            return None;
        }
    }
    Some(out)
}

fn template(parts: &[&Value]) -> Option<GridTemplate> {
    if let [only] = parts {
        if is_keyword(only, "none") {
            return Some(GridTemplate::None);
        }
    }
    if let [first, rest @ ..] = parts {
        if is_keyword(first, "subgrid") {
            return subgrid_names(rest).map(GridTemplate::Subgrid);
        }
    }
    track_list(parts, false).map(GridTemplate::Tracks)
}

/// `<line-name-list>`: `[ <line-names> | <name-repeat> ]+`, at most one
/// `repeat(auto-fill, ...)`.
fn subgrid_names(parts: &[&Value]) -> Option<Vec<SubgridNames>> {
    let mut out = Vec::new();
    let mut auto_fill = false;
    for v in parts {
        if let Some(names) = line_names(v) {
            out.push(SubgridNames::Names(names));
            continue;
        }
        let args = arguments(v, "repeat")?;
        let [count, list] = args.as_slice() else {
            return None;
        };
        let [count] = count.as_slice() else {
            return None;
        };
        let count = if is_keyword(count, "auto-fill") {
            if auto_fill {
                return None;
            }
            auto_fill = true;
            None
        } else {
            let n = integer(count)?;
            if n < 1 {
                return None;
            }
            Some(u32::try_from(n).ok()?.min(MAX_REPEAT))
        };
        let list: Vec<Vec<String>> = list.iter().map(|v| line_names(v)).collect::<Option<_>>()?;
        if list.is_empty() {
            return None;
        }
        out.push(SubgridNames::Repeat(count, list));
    }
    Some(out)
}

/// The cell tokens of one `grid-template-areas` string (CSS Grid 2 §7.3).
fn area_row(text: &str) -> Option<Vec<Option<String>>> {
    let mut cells = Vec::new();
    let mut chars = text.chars().peekable();
    let name_point = |c: char| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c >= '\u{80}';
    while let Some(&c) = chars.peek() {
        if matches!(c, ' ' | '\t' | '\n' | '\r' | '\u{c}') {
            chars.next();
        } else if c == '.' {
            while chars.peek() == Some(&'.') {
                chars.next();
            }
            cells.push(None);
        } else if name_point(c) {
            let mut name = String::new();
            while let Some(&c) = chars.peek().filter(|&&c| name_point(c)) {
                name.push(c);
                chars.next();
            }
            cells.push(Some(name));
        } else {
            return None;
        }
    }
    Some(cells)
}

/// Validate area strings into rows of cells and rectangular areas.
pub(crate) fn areas_from_strings(strings: &[&str]) -> Option<GridAreas> {
    let cells: Vec<Vec<Option<String>>> =
        strings.iter().map(|s| area_row(s)).collect::<Option<_>>()?;
    let columns = cells.first()?.len();
    if columns == 0 || cells.iter().any(|row| row.len() != columns) {
        return None;
    }
    let mut areas: Vec<NamedArea> = Vec::new();
    for (r, row) in cells.iter().enumerate() {
        for (c, cell) in row.iter().enumerate() {
            let Some(name) = cell else { continue };
            if areas.iter().any(|a| a.name == *name) {
                continue;
            }
            // The first cell is the top-left corner; extend right, then down.
            let mut c_end = c + 1;
            while row.get(c_end) == Some(cell) {
                c_end += 1;
            }
            let mut r_end = r + 1;
            while cells
                .get(r_end)
                .is_some_and(|next| next.get(c) == Some(cell))
            {
                r_end += 1;
            }
            areas.push(NamedArea {
                name: name.clone(),
                rows: (r, r_end),
                columns: (c, c_end),
            });
        }
    }
    // Every cell must lie in its area's rectangle, and the rectangle be full.
    for (r, row) in cells.iter().enumerate() {
        for (c, cell) in row.iter().enumerate() {
            let Some(name) = cell else { continue };
            let area = areas.iter().find(|a| a.name == *name)?;
            if !(area.rows.0..area.rows.1).contains(&r)
                || !(area.columns.0..area.columns.1).contains(&c)
            {
                return None;
            }
        }
    }
    for area in &areas {
        for row in &cells[area.rows.0..area.rows.1] {
            if row[area.columns.0..area.columns.1]
                .iter()
                .any(|cell| cell.as_deref() != Some(area.name.as_str()))
            {
                return None;
            }
        }
    }
    Some(GridAreas { cells, areas })
}

fn areas(parts: &[&Value]) -> Option<Option<GridAreas>> {
    if let [only] = parts {
        if is_keyword(only, "none") {
            return Some(None);
        }
    }
    let strings: Vec<&str> = parts
        .iter()
        .map(|v| match v.token()? {
            Token::String(s) => Some(s.as_str()),
            _ => None,
        })
        .collect::<Option<_>>()?;
    areas_from_strings(&strings).map(Some)
}

fn auto_flow(parts: &[&Value]) -> Option<AutoFlow> {
    let (mut axis, mut dense) = (None, false);
    for v in parts {
        let name = ident(v)?.to_ascii_lowercase();
        match name.as_str() {
            "row" | "column" if axis.is_none() => axis = Some(name == "column"),
            "dense" if !dense => dense = true,
            _ => return None,
        }
    }
    if parts.is_empty() {
        return None;
    }
    Some(AutoFlow {
        column: axis.unwrap_or(false),
        dense,
    })
}

/// `<grid-line>`.
pub(crate) fn grid_line(parts: &[&Value]) -> Option<GridLine> {
    if let [only] = parts {
        if is_keyword(only, "auto") {
            return Some(GridLine::Auto);
        }
    }
    let (mut span, mut number, mut name) = (false, None, None);
    for (i, v) in parts.iter().enumerate() {
        if is_keyword(v, "span") {
            // `span` comes first or last, never between the others.
            if span || (i != 0 && i != parts.len() - 1) {
                return None;
            }
            span = true;
        } else if let Some(n) = integer(v) {
            if number.is_some() {
                return None;
            }
            // A math function's result clamps into `span`'s [1,∞] range
            // (CSS Values 4 §10.9); a literal outside it is invalid.
            let math = crate::properties::math_function(v);
            number = Some(if math && parts.iter().any(|p| is_keyword(p, "span")) {
                n.max(1)
            } else {
                n
            });
        } else {
            let n = line_name(v)?;
            if name.is_some() {
                return None;
            }
            name = Some(n);
        }
    }
    if parts.is_empty() || parts.len() > 3 || (number.is_none() && name.is_none()) {
        return None;
    }
    if number == Some(0) || (span && number.is_some_and(|n| n < 0)) {
        return None;
    }
    Some(GridLine::Line {
        span,
        integer: number,
        name,
    })
}

pub(crate) fn longhand(property: PropertyId, parts: &[&Value]) -> Option<PropertyValue> {
    use PropertyId as P;
    let value = match property {
        P::GridTemplateRows | P::GridTemplateColumns => GridValue::Template(template(parts)?),
        P::GridTemplateAreas => GridValue::Areas(areas(parts)?),
        P::GridAutoRows | P::GridAutoColumns => {
            if parts.is_empty() {
                return None;
            }
            GridValue::AutoTracks(parts.iter().map(|v| track_size(v)).collect::<Option<_>>()?)
        }
        P::GridAutoFlow => GridValue::AutoFlow(auto_flow(parts)?),
        P::GridRowStart | P::GridRowEnd | P::GridColumnStart | P::GridColumnEnd => {
            GridValue::Line(grid_line(parts)?)
        }
        _ => return None,
    };
    Some(PropertyValue::Grid(value))
}

fn split_slash<'a, 'b>(parts: &'b [&'a Value]) -> Vec<&'b [&'a Value]> {
    parts
        .split(|v| v.token() == Some(&Token::Delim('/')))
        .collect()
}

fn line_value(line: GridLine) -> PropertyValue {
    PropertyValue::Grid(GridValue::Line(line))
}

/// The omitted end of a `grid-row`/`grid-column`/`grid-area` value: a copy
/// of a bare `<custom-ident>`, else `auto`.
fn copy_ident(line: &GridLine) -> GridLine {
    match line.ident() {
        Some(_) => line.clone(),
        None => GridLine::Auto,
    }
}

/// `grid-row`, `grid-column`: `<grid-line> [ / <grid-line> ]?`.
pub(crate) fn line_shorthand(parts: &[&Value]) -> Option<Vec<PropertyValue>> {
    let pieces = split_slash(parts);
    let (start, end) = match pieces.as_slice() {
        [start] => {
            let start = grid_line(start)?;
            let end = copy_ident(&start);
            (start, end)
        }
        [start, end] => (grid_line(start)?, grid_line(end)?),
        _ => return None,
    };
    Some(vec![line_value(start), line_value(end)])
}

/// `grid-area`: `<grid-line> [ / <grid-line> ]{0,3}`, longhands in
/// row-start, column-start, row-end, column-end order.
pub(crate) fn area_shorthand(parts: &[&Value]) -> Option<Vec<PropertyValue>> {
    let pieces = split_slash(parts);
    if pieces.len() > 4 {
        return None;
    }
    let lines: Vec<GridLine> = pieces.iter().map(|p| grid_line(p)).collect::<Option<_>>()?;
    let row_start = lines[0].clone();
    let column_start = lines
        .get(1)
        .cloned()
        .unwrap_or_else(|| copy_ident(&row_start));
    let row_end = lines
        .get(2)
        .cloned()
        .unwrap_or_else(|| copy_ident(&row_start));
    let column_end = lines
        .get(3)
        .cloned()
        .unwrap_or_else(|| copy_ident(&column_start));
    Some(
        [row_start, column_start, row_end, column_end]
            .into_iter()
            .map(line_value)
            .collect(),
    )
}

/// The `[ <line-names>? <string> <track-size>? <line-names>? ]+ [ /
/// <explicit-track-list> ]?` form of `grid-template`: (rows, columns,
/// areas).
fn template_with_areas(parts: &[&Value]) -> Option<(GridTemplate, GridTemplate, GridAreas)> {
    let pieces = split_slash(parts);
    let (rows_part, columns) = match pieces.as_slice() {
        [rows] => (*rows, GridTemplate::None),
        [rows, columns] => (*rows, GridTemplate::Tracks(track_list(columns, true)?)),
        _ => return None,
    };
    let mut rows: Vec<TrackComponent> = Vec::new();
    let mut strings: Vec<&str> = Vec::new();
    let mut i = 0;
    while i < rows_part.len() {
        let mut names_before = None;
        if let Some(names) = line_names(rows_part[i]) {
            names_before = Some(names);
            i += 1;
        }
        let Some(Token::String(s)) = rows_part.get(i).and_then(|v| v.token()) else {
            return None;
        };
        strings.push(s);
        i += 1;
        let mut size = TrackSize::AUTO;
        if let Some(v) = rows_part.get(i) {
            if !matches!(v.token(), Some(Token::String(_))) && line_names(v).is_none() {
                size = track_size(v)?;
                i += 1;
            }
        }
        let mut names_after = None;
        if let Some(names) = rows_part.get(i).and_then(|v| line_names(v)) {
            names_after = Some(names);
            i += 1;
        }
        // Names after one row and before the next merge into one line.
        match (rows.last_mut(), names_before) {
            (Some(TrackComponent::Names(prev)), Some(names)) => prev.extend(names),
            (_, Some(names)) => rows.push(TrackComponent::Names(names)),
            _ => {}
        }
        rows.push(TrackComponent::Size(size));
        if let Some(names) = names_after {
            rows.push(TrackComponent::Names(names));
        }
    }
    if strings.is_empty() {
        return None;
    }
    let areas = areas_from_strings(&strings)?;
    Some((GridTemplate::Tracks(rows), columns, areas))
}

/// `grid-template`: (rows, columns, areas).
fn template_parts(parts: &[&Value]) -> Option<(GridTemplate, GridTemplate, Option<GridAreas>)> {
    if let [only] = parts {
        if is_keyword(only, "none") {
            return Some((GridTemplate::None, GridTemplate::None, None));
        }
    }
    if parts
        .iter()
        .any(|v| matches!(v.token(), Some(Token::String(_))))
    {
        let (rows, columns, areas) = template_with_areas(parts)?;
        return Some((rows, columns, Some(areas)));
    }
    let pieces = split_slash(parts);
    let [rows, columns] = pieces.as_slice() else {
        return None;
    };
    Some((template(rows)?, template(columns)?, None))
}

/// `grid-template` longhands: rows, columns, areas.
pub(crate) fn template_shorthand(parts: &[&Value]) -> Option<Vec<PropertyValue>> {
    let (rows, columns, areas) = template_parts(parts)?;
    Some(vec![
        PropertyValue::Grid(GridValue::Template(rows)),
        PropertyValue::Grid(GridValue::Template(columns)),
        PropertyValue::Grid(GridValue::Areas(areas)),
    ])
}

/// `[ auto-flow && dense? ] <auto-tracks>?`: (dense, tracks).
fn auto_flow_side(parts: &[&Value]) -> Option<Option<(bool, Vec<TrackSize>)>> {
    let mut i = 0;
    let (mut flow, mut dense) = (false, false);
    while let Some(v) = parts.get(i) {
        if !flow && is_keyword(v, "auto-flow") {
            flow = true;
        } else if !dense && is_keyword(v, "dense") {
            dense = true;
        } else {
            break;
        }
        i += 1;
    }
    if !flow {
        return Some(None);
    }
    let tracks = if i == parts.len() {
        vec![TrackSize::AUTO]
    } else {
        parts[i..]
            .iter()
            .map(|v| track_size(v))
            .collect::<Option<_>>()?
    };
    Some(Some((dense, tracks)))
}

/// `grid` longhands: template rows, columns, areas, auto rows, auto
/// columns, auto flow.
pub(crate) fn grid_shorthand(parts: &[&Value]) -> Option<Vec<PropertyValue>> {
    let grid = |v| PropertyValue::Grid(v);
    let auto = || GridValue::AutoTracks(vec![TrackSize::AUTO]);
    let pieces = split_slash(parts);
    if let [rows, columns] = pieces.as_slice() {
        if let Some((dense, tracks)) = auto_flow_side(rows)? {
            return Some(vec![
                grid(GridValue::Template(GridTemplate::None)),
                grid(GridValue::Template(template(columns)?)),
                grid(GridValue::Areas(None)),
                grid(GridValue::AutoTracks(tracks)),
                grid(auto()),
                grid(GridValue::AutoFlow(AutoFlow {
                    column: false,
                    dense,
                })),
            ]);
        }
        if let Some((dense, tracks)) = auto_flow_side(columns)? {
            return Some(vec![
                grid(GridValue::Template(template(rows)?)),
                grid(GridValue::Template(GridTemplate::None)),
                grid(GridValue::Areas(None)),
                grid(auto()),
                grid(GridValue::AutoTracks(tracks)),
                grid(GridValue::AutoFlow(AutoFlow {
                    column: true,
                    dense,
                })),
            ]);
        }
    }
    let (rows, columns, areas) = template_parts(parts)?;
    Some(vec![
        grid(GridValue::Template(rows)),
        grid(GridValue::Template(columns)),
        grid(GridValue::Areas(areas)),
        grid(auto()),
        grid(auto()),
        grid(GridValue::AutoFlow(AutoFlow::default())),
    ])
}
