//! The pattern grammar of ECMA-262 §22.2.1 with the Annex B extensions
//! (§B.1.2) for patterns without the `u` and `v` flags, and its early
//! errors. The parser builds a [`Node`] tree; it never recurses deeper
//! than [`MAX_DEPTH`] groups and classes.

use super::Flags;
use super::class::CharSet;

/// Groups and nested classes deeper than this are a SyntaxError rather
/// than a risk to the Rust stack.
pub(crate) const MAX_DEPTH: usize = 256;

/// The largest repetition count kept exactly; larger counts mean "no
/// limit" in practice and are clamped.
pub(crate) const MAX_COUNT: u32 = u32::MAX;

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Node {
    Empty,
    /// One character (a code point with `u`/`v`, else a code unit).
    Char(u32),
    /// `.`
    Any,
    /// A class, or a class escape such as `\d`.
    Class(Box<CharSet>),
    /// A `v`-mode class that may match strings: alternatives of
    /// sequences (longest first), then the single characters.
    StringClass(Vec<Vec<u32>>, Box<CharSet>),
    /// `^`
    Start,
    /// `$`
    End,
    /// `\b`, or `\B` when negated.
    WordBoundary(bool),
    /// A capturing group with its index (from 1).
    Capture(Box<Node>, u32),
    /// `(?=…)`, `(?!…)`, `(?<=…)`, `(?<!…)`.
    Look {
        node: Box<Node>,
        behind: bool,
        negative: bool,
    },
    /// `\1` or `\k<name>`: the groups it may refer to (several for a
    /// duplicated name, of which at most one participates).
    BackRef(Vec<u32>),
    /// `\k<name>` until the names are resolved after parsing.
    NamedRef(Vec<u16>),
    Concat(Vec<Node>),
    Alt(Vec<Node>),
    Repeat {
        node: Box<Node>,
        min: u32,
        max: u32,
        greedy: bool,
        /// The captures inside, cleared at each iteration.
        captures: (u32, u32),
    },
    /// `(?ims-ims:…)`: the flags added and removed inside.
    Modified {
        node: Box<Node>,
        add: Flags,
        remove: Flags,
    },
}

/// A parsed pattern.
#[derive(Debug)]
pub(crate) struct Parsed {
    pub node: Node,
    /// Capturing groups, not counting the whole match.
    pub groups: u32,
    /// Each group name and the groups that carry it, in order.
    pub names: Vec<(Vec<u16>, Vec<u32>)>,
    /// The pattern uses a Unicode property escape, which cannot be
    /// matched yet.
    pub uses_properties: bool,
}

/// Why a pattern cannot be used.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PatternError {
    /// What is wrong.
    pub message: &'static str,
    /// The pattern is valid but uses something not implemented yet
    /// (Unicode property escapes, TD-110).
    pub unsupported: bool,
}

fn syntax(message: &'static str) -> PatternError {
    PatternError {
        message,
        unsupported: false,
    }
}

type PResult<T> = Result<T, PatternError>;

fn fail<T>(message: &'static str) -> PResult<T> {
    Err(syntax(message))
}

/// Parse `source` (the pattern's code units) under `flags`.
pub(crate) fn parse(source: &[u16], flags: Flags) -> PResult<Parsed> {
    let unicode = flags.unicode || flags.sets;
    let chars = if unicode {
        decode(source)
    } else {
        source.iter().map(|&u| u32::from(u)).collect()
    };
    let (groups, has_names) = prescan(&chars, flags.sets);
    let mut p = Parser {
        chars,
        pos: 0,
        unicode,
        ignore_case: flags.ignore_case,
        sets: flags.sets,
        named: has_names || unicode,
        groups_total: groups,
        group: 0,
        names: Vec::new(),
        name_paths: Vec::new(),
        path: Vec::new(),
        next_disjunction: 0,
        backref_names: Vec::new(),
        depth: 0,
        uses_properties: false,
    };
    let node = p.disjunction()?;
    if p.pos < p.chars.len() {
        return fail(if p.peek() == Some(u32::from(b')')) {
            "unmatched `)`"
        } else {
            "unexpected character"
        });
    }
    for name in std::mem::take(&mut p.backref_names) {
        if !p.names.iter().any(|(n, _)| *n == name) {
            return fail("reference to an undefined group name");
        }
    }
    let mut node = node;
    resolve_names(&mut node, &p.names);
    Ok(Parsed {
        node,
        groups: p.group,
        names: p.names,
        uses_properties: p.uses_properties,
    })
}

/// Replace each `\k<name>` by a reference to the groups of that name.
fn resolve_names(node: &mut Node, names: &[(Vec<u16>, Vec<u32>)]) {
    match node {
        Node::NamedRef(name) => {
            let groups = names
                .iter()
                .find(|(n, _)| n == name)
                .map(|(_, g)| g.clone())
                .unwrap_or_default();
            *node = Node::BackRef(groups);
        }
        Node::Capture(inner, _)
        | Node::Look { node: inner, .. }
        | Node::Repeat { node: inner, .. }
        | Node::Modified { node: inner, .. } => resolve_names(inner, names),
        Node::Concat(nodes) | Node::Alt(nodes) => {
            for n in nodes {
                resolve_names(n, names);
            }
        }
        _ => {}
    }
}

/// Code points from UTF-16, pairing surrogates.
fn decode(units: &[u16]) -> Vec<u32> {
    char::decode_utf16(units.iter().copied())
        .map(|r| r.map_or_else(|e| u32::from(e.unpaired_surrogate()), u32::from))
        .collect()
}

/// Count the capturing groups and note whether any is named, before
/// parsing (a `\N` escape needs the total; `\k` depends on names).
fn prescan(chars: &[u32], sets: bool) -> (u32, bool) {
    let (mut groups, mut named) = (0, false);
    let mut i = 0;
    let mut class_depth = 0usize;
    while i < chars.len() {
        match chars[i] {
            0x5C => i += 1,
            0x5B => {
                if sets || class_depth == 0 {
                    class_depth += 1;
                }
            }
            0x5D if class_depth > 0 => class_depth -= 1,
            0x28 if class_depth == 0 => {
                if chars.get(i + 1) != Some(&0x3F) {
                    groups += 1;
                } else if chars.get(i + 2) == Some(&0x3C)
                    && !matches!(chars.get(i + 3), Some(&0x3D | &0x21))
                {
                    groups += 1;
                    named = true;
                }
            }
            _ => {}
        }
        i += 1;
    }
    (groups, named)
}

/// One step from the root to a group: (disjunction, alternative).
type AltStep = (u32, u32);

struct Parser {
    chars: Vec<u32>,
    pos: usize,
    /// `u` or `v`.
    unicode: bool,
    /// `i` as modifier groups leave it at the current position: with `u`
    /// or `v`, `\w` gains the characters that fold into it, and a
    /// negated class is closed under case before it is complemented.
    ignore_case: bool,
    /// `v`.
    sets: bool,
    /// Group names are in play: `\k` is a reference, not an identity
    /// escape.
    named: bool,
    groups_total: u32,
    /// Groups opened so far.
    group: u32,
    names: Vec<(Vec<u16>, Vec<u32>)>,
    /// For each name's groups: the alternatives they sit in, as
    /// (disjunction, alternative) pairs from the root.
    name_paths: Vec<(Vec<u16>, Vec<AltStep>)>,
    path: Vec<(u32, u32)>,
    next_disjunction: u32,
    backref_names: Vec<Vec<u16>>,
    depth: usize,
    uses_properties: bool,
}

const SYNTAX_CHARACTERS: &[u8] = b"^$\\.*+?()[]{}|";

fn is_syntax_character(c: u32) -> bool {
    c < 0x80 && SYNTAX_CHARACTERS.contains(&(c as u8))
}

fn hex_value(c: u32) -> Option<u32> {
    char::from_u32(c)?.to_digit(16)
}

impl Parser {
    fn peek(&self) -> Option<u32> {
        self.chars.get(self.pos).copied()
    }

    fn peek_at(&self, offset: usize) -> Option<u32> {
        self.chars.get(self.pos + offset).copied()
    }

    fn eat(&mut self, c: u8) -> bool {
        if self.peek() == Some(u32::from(c)) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    fn looking_at(&self, text: &str) -> bool {
        text.bytes()
            .enumerate()
            .all(|(i, b)| self.peek_at(i) == Some(u32::from(b)))
    }

    fn enter(&mut self) -> PResult<()> {
        self.depth += 1;
        if self.depth > MAX_DEPTH {
            return fail("pattern nested too deeply");
        }
        Ok(())
    }

    fn disjunction(&mut self) -> PResult<Node> {
        let id = self.next_disjunction;
        self.next_disjunction += 1;
        let mut alternatives = Vec::new();
        let mut index = 0;
        loop {
            self.path.push((id, index));
            let alternative = self.alternative();
            self.path.pop();
            alternatives.push(alternative?);
            if !self.eat(b'|') {
                break;
            }
            index += 1;
        }
        Ok(if alternatives.len() == 1 {
            alternatives.pop().expect("one alternative")
        } else {
            Node::Alt(alternatives)
        })
    }

    fn alternative(&mut self) -> PResult<Node> {
        let mut terms = Vec::new();
        while let Some(c) = self.peek() {
            if c == u32::from(b'|') || c == u32::from(b')') {
                break;
            }
            terms.push(self.term()?);
        }
        Ok(match terms.len() {
            0 => Node::Empty,
            1 => terms.pop().expect("one term"),
            _ => Node::Concat(terms),
        })
    }

    fn term(&mut self) -> PResult<Node> {
        let first_group = self.group;
        let c = self.peek().expect("the caller checked");
        let (atom, quantifiable) = match c {
            0x5E => {
                self.pos += 1;
                (Node::Start, false)
            }
            0x24 => {
                self.pos += 1;
                (Node::End, false)
            }
            0x5C if matches!(self.peek_at(1), Some(0x62 | 0x42)) => {
                let negated = self.peek_at(1) == Some(0x42);
                self.pos += 2;
                (Node::WordBoundary(negated), false)
            }
            0x28 if self.looking_at("(?=")
                || self.looking_at("(?!")
                || self.looking_at("(?<=")
                || self.looking_at("(?<!") =>
            {
                let behind = self.peek_at(2) == Some(0x3C);
                let negative = self.peek_at(if behind { 3 } else { 2 }) == Some(0x21);
                self.pos += if behind { 4 } else { 3 };
                self.enter()?;
                let node = self.disjunction()?;
                self.depth -= 1;
                if !self.eat(b')') {
                    return fail("unterminated group");
                }
                // Annex B: a lookahead without `u` may be quantified.
                let quantifiable = !behind && !self.unicode;
                (
                    Node::Look {
                        node: Box::new(node),
                        behind,
                        negative,
                    },
                    quantifiable,
                )
            }
            _ => (self.atom()?, true),
        };
        match self.quantifier()? {
            Some(_) if !quantifiable => fail("nothing to repeat"),
            Some((min, max, greedy)) => Ok(Node::Repeat {
                node: Box::new(atom),
                min,
                max,
                greedy,
                captures: (first_group + 1, self.group + 1),
            }),
            None => Ok(atom),
        }
    }

    /// A quantifier after an atom: `(min, max, greedy)`.
    fn quantifier(&mut self) -> PResult<Option<(u32, u32, bool)>> {
        let (min, max) = match self.peek() {
            Some(0x2A) => {
                self.pos += 1;
                (0, MAX_COUNT)
            }
            Some(0x2B) => {
                self.pos += 1;
                (1, MAX_COUNT)
            }
            Some(0x3F) => {
                self.pos += 1;
                (0, 1)
            }
            Some(0x7B) => match self.braced_quantifier() {
                Some(range) => range,
                None if self.unicode => return fail("incomplete quantifier"),
                // Annex B: a `{` that starts no quantifier is a literal.
                None => return Ok(None),
            },
            _ => return Ok(None),
        };
        if min > max {
            return fail("numbers out of order in quantifier");
        }
        let greedy = !self.eat(b'?');
        Ok(Some((min, max, greedy)))
    }

    /// `{n}`, `{n,}` or `{n,m}` at the cursor, consumed if valid.
    fn braced_quantifier(&mut self) -> Option<(u32, u32)> {
        let start = self.pos;
        self.pos += 1;
        let Some(min) = self.decimal() else {
            self.pos = start;
            return None;
        };
        let max = if self.eat(b',') {
            if self.peek() == Some(0x7D) {
                MAX_COUNT
            } else if let Some(max) = self.decimal() {
                max
            } else {
                self.pos = start;
                return None;
            }
        } else {
            min
        };
        if !self.eat(b'}') {
            self.pos = start;
            return None;
        }
        Some((min, max))
    }

    /// Decimal digits, saturating.
    fn decimal(&mut self) -> Option<u32> {
        let start = self.pos;
        let mut value: u32 = 0;
        while let Some(d) = self.peek().and_then(|c| char::from_u32(c)?.to_digit(10)) {
            value = value.saturating_mul(10).saturating_add(d);
            self.pos += 1;
        }
        (self.pos > start).then_some(value)
    }

    fn atom(&mut self) -> PResult<Node> {
        let c = self.peek().expect("the caller checked");
        match c {
            0x2E => {
                self.pos += 1;
                Ok(Node::Any)
            }
            0x28 => self.group(),
            0x5B => {
                self.pos += 1;
                if self.sets {
                    self.enter()?;
                    let (strings, set) = self.class_set_expression()?;
                    self.depth -= 1;
                    Ok(if strings.is_empty() {
                        Node::Class(Box::new(set))
                    } else {
                        Node::StringClass(strings, Box::new(set))
                    })
                } else {
                    Ok(Node::Class(Box::new(self.class_ranges()?)))
                }
            }
            0x5C => self.atom_escape(),
            0x2A | 0x2B | 0x3F => fail("nothing to repeat"),
            0x7B => {
                if self.unicode || self.braced_quantifier().is_some() {
                    fail("nothing to repeat")
                } else {
                    self.pos += 1;
                    Ok(Node::Char(c))
                }
            }
            0x29 => fail("unmatched `)`"),
            0x5D | 0x7D if self.unicode => fail("lone `]` or `}`"),
            _ => {
                self.pos += 1;
                Ok(Node::Char(c))
            }
        }
    }

    fn group(&mut self) -> PResult<Node> {
        self.pos += 1;
        self.enter()?;
        let node = if self.eat(b'?') {
            if self.eat(b':') {
                self.disjunction()?
            } else if self.peek() == Some(0x3C) {
                self.pos += 1;
                let name = self.group_name()?;
                self.group += 1;
                let index = self.group;
                self.declare_name(name, index)?;
                Node::Capture(Box::new(self.disjunction()?), index)
            } else {
                let (add, remove) = self.modifiers()?;
                let outer = self.ignore_case;
                self.ignore_case = (outer || add.ignore_case) && !remove.ignore_case;
                let node = self.disjunction();
                self.ignore_case = outer;
                Node::Modified {
                    node: Box::new(node?),
                    add,
                    remove,
                }
            }
        } else {
            self.group += 1;
            let index = self.group;
            Node::Capture(Box::new(self.disjunction()?), index)
        };
        self.depth -= 1;
        if !self.eat(b')') {
            return fail("unterminated group");
        }
        Ok(node)
    }

    /// `ims-ims:` after `(?`: the flags a modifier group adds and removes.
    fn modifiers(&mut self) -> PResult<(Flags, Flags)> {
        let mut add = Flags::default();
        let mut remove = Flags::default();
        let mut removing = false;
        let mut any = false;
        loop {
            let Some(c) = self.peek() else {
                return fail("invalid group");
            };
            self.pos += 1;
            let target = if removing { &mut remove } else { &mut add };
            let slot = match c {
                0x69 => &mut target.ignore_case,
                0x6D => &mut target.multiline,
                0x73 => &mut target.dot_all,
                0x2D if !removing => {
                    removing = true;
                    continue;
                }
                0x3A => break,
                _ => return fail("invalid group"),
            };
            if *slot {
                return fail("repeated flag in a modifier group");
            }
            *slot = true;
            any = true;
        }
        let both = (add.ignore_case && remove.ignore_case)
            || (add.multiline && remove.multiline)
            || (add.dot_all && remove.dot_all);
        if both || (removing && !any) {
            return fail("invalid modifier group");
        }
        Ok((add, remove))
    }

    /// A class negated with `^`. Under `i` the spec asks whether any
    /// member has the same canonical form as the input and then inverts,
    /// so the set is closed under case first; the complement of a closed
    /// set is closed, so the compiler's own closure leaves it alone.
    fn complement(&self, set: &CharSet) -> CharSet {
        if self.ignore_case {
            super::case::close(set, self.unicode).negated()
        } else {
            set.negated()
        }
    }

    /// `name>` after `(?<` or `\k<`.
    fn group_name(&mut self) -> PResult<Vec<u16>> {
        let mut name = Vec::new();
        let mut first = true;
        loop {
            let Some(mut c) = self.peek() else {
                return fail("invalid group name");
            };
            self.pos += 1;
            if c == 0x3E {
                break;
            }
            if c == 0x5C {
                if !self.eat(b'u') {
                    return fail("invalid group name");
                }
                c = self.unicode_escape_in_name()?;
            } else if (0xD800..0xDC00).contains(&c) {
                // Without `u` the source is code units: pair them here.
                if let Some(low) = self.peek().filter(|l| (0xDC00..0xE000).contains(l)) {
                    self.pos += 1;
                    c = 0x10000 + ((c - 0xD800) << 10) + (low - 0xDC00);
                }
            }
            let ok = if first {
                crate::unicode::is_identifier_start(c)
            } else {
                crate::unicode::is_identifier_part(c)
            };
            if !ok {
                return fail("invalid group name");
            }
            first = false;
            let mut buf = [0u16; 2];
            let Some(ch) = char::from_u32(c) else {
                return fail("invalid group name");
            };
            name.extend_from_slice(ch.encode_utf16(&mut buf));
        }
        if name.is_empty() {
            return fail("invalid group name");
        }
        Ok(name)
    }

    /// `\u` escapes in a group name: `\uXXXX` (paired), or `\u{…}`.
    fn unicode_escape_in_name(&mut self) -> PResult<u32> {
        if self.eat(b'{') {
            return self.braced_code_point().ok_or(syntax("invalid group name"));
        }
        let Some(high) = self.hex4() else {
            return fail("invalid group name");
        };
        if (0xD800..0xDC00).contains(&high) && self.looking_at("\\u") {
            let save = self.pos;
            self.pos += 2;
            match self.hex4() {
                Some(low) if (0xDC00..0xE000).contains(&low) => {
                    return Ok(0x10000 + ((high - 0xD800) << 10) + (low - 0xDC00));
                }
                _ => self.pos = save,
            }
        }
        Ok(high)
    }

    fn declare_name(&mut self, name: Vec<u16>, index: u32) -> PResult<()> {
        let path = self.path.clone();
        for (other, other_path) in &self.name_paths {
            if *other == name && might_both_participate(&path, other_path) {
                return fail("duplicate group name");
            }
        }
        self.name_paths.push((name.clone(), path));
        match self.names.iter_mut().find(|(n, _)| *n == name) {
            Some((_, groups)) => groups.push(index),
            None => self.names.push((name, vec![index])),
        }
        Ok(())
    }

    fn hex4(&mut self) -> Option<u32> {
        let mut value = 0;
        for i in 0..4 {
            value = value * 16 + hex_value(self.peek_at(i)?)?;
        }
        self.pos += 4;
        Some(value)
    }

    /// `XXXXXX}` after `\u{`: a code point up to 0x10FFFF.
    fn braced_code_point(&mut self) -> Option<u32> {
        let start = self.pos;
        let mut value: u32 = 0;
        while let Some(d) = self.peek().and_then(hex_value) {
            value = value.saturating_mul(16).saturating_add(d);
            self.pos += 1;
        }
        if self.pos == start || value > 0x10FFFF || !self.eat(b'}') {
            self.pos = start;
            return None;
        }
        Some(value)
    }

    fn atom_escape(&mut self) -> PResult<Node> {
        self.pos += 1;
        let Some(c) = self.peek() else {
            return fail("`\\` at end of pattern");
        };
        // A back reference.
        if (0x31..=0x39).contains(&c) {
            let start = self.pos;
            let n = self.decimal().unwrap_or(0);
            if n <= self.groups_total {
                return Ok(Node::BackRef(vec![n]));
            }
            if self.unicode {
                return fail("reference to a group that does not exist");
            }
            // Annex B: a legacy octal escape or an identity escape.
            self.pos = start;
        }
        if c == 0x6B && self.named {
            self.pos += 1;
            if !self.eat(b'<') {
                return fail("invalid named reference");
            }
            let name = self.group_name()?;
            self.backref_names.push(name.clone());
            return Ok(Node::NamedRef(name));
        }
        if let Some(set) = self.class_escape()? {
            let ClassEscape::Set(set) = set;
            return Ok(Node::Class(Box::new(set)));
        }
        Ok(Node::Char(self.character_escape(false)?))
    }

    /// `\d \D \s \S \w \W \p{…} \P{…}` after the `\`, if one is there.
    fn class_escape(&mut self) -> PResult<Option<ClassEscape>> {
        let Some(c) = self.peek() else {
            return Ok(None);
        };
        let set = match c {
            0x64 => CharSet::digits(),
            0x44 => CharSet::digits().negated(),
            0x73 => CharSet::spaces(),
            0x53 => CharSet::spaces().negated(),
            0x77 => CharSet::word(self.unicode && self.ignore_case),
            0x57 => CharSet::word(self.unicode && self.ignore_case).negated(),
            0x70 | 0x50 if self.unicode => {
                // `\p{Name}` or `\p{Name=Value}`: checked for shape only.
                self.pos += 1;
                if !self.eat(b'{') {
                    return fail("invalid property escape");
                }
                let mut parts = 0;
                let mut length = 0;
                loop {
                    match self.peek() {
                        Some(c) if c < 0x80 && (c as u8).is_ascii_alphanumeric() || c == 0x5F => {
                            length += 1;
                            self.pos += 1;
                        }
                        Some(0x3D) if parts == 0 && length > 0 => {
                            parts = 1;
                            length = 0;
                            self.pos += 1;
                        }
                        Some(0x7D) if length > 0 => {
                            self.pos += 1;
                            break;
                        }
                        _ => return fail("invalid property escape"),
                    }
                }
                self.uses_properties = true;
                return Ok(Some(ClassEscape::Set(CharSet::default())));
            }
            _ => return Ok(None),
        };
        self.pos += 1;
        Ok(Some(ClassEscape::Set(set)))
    }

    /// A CharacterEscape after the `\` (and `\b`, `\-` inside a class).
    fn character_escape(&mut self, in_class: bool) -> PResult<u32> {
        let c = self.peek().expect("the caller checked");
        self.pos += 1;
        let value = match c {
            0x66 => 0x0C,
            0x6E => 0x0A,
            0x72 => 0x0D,
            0x74 => 0x09,
            0x76 => 0x0B,
            0x62 if in_class => 0x08,
            0x2D if in_class && self.unicode => 0x2D,
            0x63 => {
                let letter = self.peek().filter(|l| {
                    (0x41..=0x5A).contains(l)
                        || (0x61..=0x7A).contains(l)
                        || (in_class && !self.unicode && (l == &0x5F || (0x30..=0x39).contains(l)))
                });
                match letter {
                    Some(l) => {
                        self.pos += 1;
                        l % 32
                    }
                    None if self.unicode => return fail("invalid control escape"),
                    None => {
                        // Annex B: `\c` is a literal backslash.
                        self.pos -= 1;
                        0x5C
                    }
                }
            }
            0x30 if !self.peek().is_some_and(|d| (0x30..=0x39).contains(&d)) => 0,
            0x30..=0x37 if !self.unicode => {
                // Annex B legacy octal: up to 0o377.
                let mut value = c - 0x30;
                let limit = if c <= 0x33 { 2 } else { 1 };
                for _ in 0..limit {
                    match self.peek().filter(|d| (0x30..=0x37).contains(d)) {
                        Some(d) => {
                            value = value * 8 + (d - 0x30);
                            self.pos += 1;
                        }
                        None => break,
                    }
                }
                value
            }
            0x78 => {
                match (
                    self.peek().and_then(hex_value),
                    self.peek_at(1).and_then(hex_value),
                ) {
                    (Some(a), Some(b)) => {
                        self.pos += 2;
                        a * 16 + b
                    }
                    _ if self.unicode => return fail("invalid hexadecimal escape"),
                    _ => 0x78,
                }
            }
            0x75 => match self.unicode_escape() {
                Some(cp) => cp,
                None if self.unicode => return fail("invalid Unicode escape"),
                None => 0x75,
            },
            _ if self.unicode => {
                if is_syntax_character(c) || c == 0x2F {
                    c
                } else {
                    return fail("invalid escape");
                }
            }
            // Annex B identity escapes: anything but `k` with names.
            0x6B if self.named => return fail("invalid escape"),
            _ => c,
        };
        Ok(value)
    }

    /// `XXXX` or `{…}` after `\u`; with `u`, a surrogate pair written as
    /// two escapes is one code point.
    fn unicode_escape(&mut self) -> Option<u32> {
        if self.unicode && self.eat(b'{') {
            return self.braced_code_point();
        }
        let high = self.hex4()?;
        if self.unicode && (0xD800..0xDC00).contains(&high) && self.looking_at("\\u") {
            let save = self.pos;
            self.pos += 2;
            match self.hex4() {
                Some(low) if (0xDC00..0xE000).contains(&low) => {
                    return Some(0x10000 + ((high - 0xD800) << 10) + (low - 0xDC00));
                }
                _ => self.pos = save,
            }
        }
        Some(high)
    }

    // ---- classes without `v` -----------------------------------------------

    /// `[…]` after the `[`.
    fn class_ranges(&mut self) -> PResult<CharSet> {
        let negated = self.eat(b'^');
        let mut set = CharSet::default();
        loop {
            match self.peek() {
                None => return fail("unterminated character class"),
                Some(0x5D) => {
                    self.pos += 1;
                    break;
                }
                _ => {}
            }
            let first = self.class_atom()?;
            if self.peek() == Some(0x2D) && self.peek_at(1).is_some_and(|c| c != 0x5D) {
                self.pos += 1;
                let second = self.class_atom()?;
                match (&first, &second) {
                    (ClassAtom::Char(a), ClassAtom::Char(b)) => {
                        if a > b {
                            return fail("range out of order in character class");
                        }
                        set.add_range(*a, *b);
                    }
                    _ if self.unicode => return fail("invalid character class range"),
                    _ => {
                        // Annex B: `-` next to a class escape is literal.
                        first.add_to(&mut set);
                        set.add(0x2D);
                        second.add_to(&mut set);
                    }
                }
            } else {
                first.add_to(&mut set);
            }
        }
        Ok(if negated { self.complement(&set) } else { set })
    }

    fn class_atom(&mut self) -> PResult<ClassAtom> {
        let c = self.peek().expect("the caller checked");
        if c != 0x5C {
            self.pos += 1;
            return Ok(ClassAtom::Char(c));
        }
        self.pos += 1;
        if self.peek().is_none() {
            return fail("`\\` at end of pattern");
        }
        if let Some(ClassEscape::Set(set)) = self.class_escape()? {
            return Ok(ClassAtom::Set(set));
        }
        // Annex B: `\8` and `\9` in a class are identity escapes; other
        // digits are octal.
        Ok(ClassAtom::Char(self.character_escape(true)?))
    }

    // ---- classes with `v` --------------------------------------------------

    /// `[…]` after the `[` in `v` mode: the strings it may match and its
    /// single characters.
    fn class_set_expression(&mut self) -> PResult<(Vec<Vec<u32>>, CharSet)> {
        let negated = self.eat(b'^');
        let mut acc = self.class_set_operand_or_range()?;
        let Some(mut acc_value) = acc.take() else {
            // An empty class.
            if !self.eat(b']') {
                return fail("invalid character class");
            }
            let set = CharSet::default();
            return Ok((
                Vec::new(),
                if negated { self.complement(&set) } else { set },
            ));
        };
        let op = if self.looking_at("&&") {
            Some(true)
        } else if self.looking_at("--") {
            Some(false)
        } else {
            None
        };
        match op {
            Some(intersection) => loop {
                let token = if intersection { "&&" } else { "--" };
                if !self.looking_at(token) {
                    if self.eat(b']') {
                        break;
                    }
                    return fail("mixed set operators in a character class");
                }
                self.pos += 2;
                if intersection && self.peek() == Some(0x26) {
                    return fail("invalid set operation");
                }
                let rhs = self.class_set_operand()?;
                acc_value = if intersection {
                    acc_value.intersect(&rhs)
                } else {
                    acc_value.subtract(&rhs)
                };
            },
            None => loop {
                if self.eat(b']') {
                    break;
                }
                if self.looking_at("&&") || self.looking_at("--") {
                    return fail("mixed set operators in a character class");
                }
                match self.class_set_operand_or_range()?.take() {
                    Some(next) => acc_value = acc_value.union(&next),
                    None => return fail("unterminated character class"),
                }
            },
        }
        let ClassValue { strings, set } = acc_value;
        if negated {
            if !strings.is_empty() {
                return fail("negated character class may contain strings");
            }
            return Ok((Vec::new(), self.complement(&set)));
        }
        let mut strings: Vec<Vec<u32>> = strings.into_iter().collect();
        // Longest strings first, as the matcher tries them in order.
        strings.sort_by(|a, b| b.len().cmp(&a.len()).then(a.cmp(b)));
        Ok((strings, set))
    }

    /// One union member: an operand or a `a-b` range; `None` at `]`.
    fn class_set_operand_or_range(&mut self) -> PResult<Option<ClassValue>> {
        match self.peek() {
            None => fail("unterminated character class"),
            Some(0x5D) => Ok(None),
            _ => {
                let start = self.pos;
                if let Some(a) = self.class_set_character()? {
                    if self.peek() == Some(0x2D) && self.peek_at(1) != Some(0x2D) {
                        self.pos += 1;
                        let Some(b) = self.class_set_character()? else {
                            return fail("invalid character class range");
                        };
                        if a > b {
                            return fail("range out of order in character class");
                        }
                        let mut set = CharSet::default();
                        set.add_range(a, b);
                        return Ok(Some(ClassValue::from_set(set)));
                    }
                    self.pos = start;
                }
                self.class_set_operand().map(Some)
            }
        }
    }

    /// A ClassSetOperand: a nested class, a string disjunction, a class
    /// escape or one character.
    fn class_set_operand(&mut self) -> PResult<ClassValue> {
        match self.peek() {
            Some(0x5B) => {
                self.pos += 1;
                self.enter()?;
                let (strings, set) = self.class_set_expression()?;
                self.depth -= 1;
                Ok(ClassValue {
                    strings: strings.into_iter().collect(),
                    set,
                })
            }
            Some(0x5C) if self.peek_at(1) == Some(0x71) => {
                self.pos += 2;
                if !self.eat(b'{') {
                    return fail("invalid escape");
                }
                self.class_string_disjunction()
            }
            Some(0x5C) => {
                self.pos += 1;
                match self.class_escape()? {
                    Some(ClassEscape::Set(set)) => Ok(ClassValue::from_set(set)),
                    None => {
                        self.pos -= 1;
                        let c = self.class_set_character()?.expect("an escape");
                        Ok(ClassValue::from_set(CharSet::single(c)))
                    }
                }
            }
            _ => match self.class_set_character()? {
                Some(c) => Ok(ClassValue::from_set(CharSet::single(c))),
                None => fail("invalid character in character class"),
            },
        }
    }

    /// `a|bc|…}` after `\q{`.
    fn class_string_disjunction(&mut self) -> PResult<ClassValue> {
        let mut value = ClassValue::default();
        let mut current = Vec::new();
        loop {
            match self.peek() {
                None => return fail("unterminated class string disjunction"),
                Some(0x7D) => {
                    self.pos += 1;
                    value.add_string(std::mem::take(&mut current));
                    return Ok(value);
                }
                Some(0x7C) => {
                    self.pos += 1;
                    value.add_string(std::mem::take(&mut current));
                }
                _ => match self.class_set_character()? {
                    Some(c) => current.push(c),
                    None => return fail("invalid character in class string disjunction"),
                },
            }
        }
    }

    /// A ClassSetCharacter at the cursor, consumed; `None` (cursor kept)
    /// for anything else.
    fn class_set_character(&mut self) -> PResult<Option<u32>> {
        let Some(c) = self.peek() else {
            return Ok(None);
        };
        if c == 0x5C {
            let Some(next) = self.peek_at(1) else {
                return fail("`\\` at end of pattern");
            };
            if matches!(
                next,
                0x64 | 0x44 | 0x73 | 0x53 | 0x77 | 0x57 | 0x70 | 0x50 | 0x71
            ) {
                return Ok(None);
            }
            self.pos += 1;
            // ClassSetReservedPunctuator may be escaped.
            if b"&-!#%,:;<=>@`~".contains(&(next as u8)) && next < 0x80 {
                self.pos += 1;
                return Ok(Some(next));
            }
            return self.character_escape(true).map(Some);
        }
        // ClassSetSyntaxCharacter and reserved double punctuators.
        if b"()[]{}/-|".contains(&(c as u8)) && c < 0x80 {
            return Ok(None);
        }
        if c < 0x80 && b"&!#$%*+,.:;<=>?@^`~".contains(&(c as u8)) && self.peek_at(1) == Some(c) {
            return fail("reserved double punctuator in a character class");
        }
        self.pos += 1;
        Ok(Some(c))
    }
}

/// Whether two groups might both participate in a match: unless some
/// disjunction holds them in different alternatives.
fn might_both_participate(a: &[(u32, u32)], b: &[(u32, u32)]) -> bool {
    !a.iter()
        .any(|&(d, i)| b.iter().any(|&(e, j)| d == e && i != j))
}

enum ClassEscape {
    Set(CharSet),
}

enum ClassAtom {
    Char(u32),
    Set(CharSet),
}

impl ClassAtom {
    fn add_to(&self, set: &mut CharSet) {
        match self {
            ClassAtom::Char(c) => set.add(*c),
            ClassAtom::Set(s) => set.add_set(s),
        }
    }
}

/// The value of a `v`-mode class operand: strings of length other than
/// one, and single characters.
#[derive(Default)]
struct ClassValue {
    strings: std::collections::BTreeSet<Vec<u32>>,
    set: CharSet,
}

impl ClassValue {
    fn from_set(set: CharSet) -> Self {
        ClassValue {
            strings: Default::default(),
            set,
        }
    }

    fn add_string(&mut self, s: Vec<u32>) {
        if s.len() == 1 {
            self.set.add(s[0]);
        } else {
            self.strings.insert(s);
        }
    }

    fn union(mut self, other: &ClassValue) -> ClassValue {
        self.set.add_set(&other.set);
        self.strings.extend(other.strings.iter().cloned());
        self
    }

    fn intersect(self, other: &ClassValue) -> ClassValue {
        ClassValue {
            strings: self.strings.intersection(&other.strings).cloned().collect(),
            set: self.set.intersect(&other.set),
        }
    }

    fn subtract(self, other: &ClassValue) -> ClassValue {
        ClassValue {
            strings: self.strings.difference(&other.strings).cloned().collect(),
            set: self.set.subtract(&other.set),
        }
    }
}
