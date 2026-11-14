//! Unicode BCP 47 locale identifier structure and canonical casing.

use std::collections::{BTreeMap, BTreeSet};

use super::data::{
    EXTENSION_ALIASES, LANGUAGES, LIKELY_REGIONS, REGIONS, SCRIPTS, SUBDIVISIONS,
    VARIANT_LANGUAGES, VARIANTS,
};

fn alpha(s: &str) -> bool {
    s.bytes().all(|b| b.is_ascii_alphabetic())
}

fn region(s: &str) -> bool {
    (s.len() == 2 && alpha(s)) || (s.len() == 3 && s.bytes().all(|b| b.is_ascii_digit()))
}

fn variant(s: &str) -> bool {
    (5..=8).contains(&s.len()) || (s.len() == 4 && s.as_bytes()[0].is_ascii_digit())
}

fn alias(table: &[(&'static str, &'static str)], key: &str) -> Option<&'static str> {
    table
        .binary_search_by_key(&key, |&(name, _)| name)
        .ok()
        .map(|i| table[i].1)
}

struct Language<'a> {
    name: &'a str,
    script: Option<&'a str>,
    region: Option<&'a str>,
    variants: BTreeSet<&'a str>,
}

impl<'a> Language<'a> {
    fn parse(parts: &mut std::iter::Peekable<impl Iterator<Item = &'a str>>) -> Option<Self> {
        let name = parts.next()?;
        if !matches!(name.len(), 2 | 3 | 5..=8) || !alpha(name) {
            return None;
        }
        let script = parts.next_if(|s| s.len() == 4 && alpha(s));
        let region = parts.next_if(|s| region(s));
        let mut variants = BTreeSet::new();
        while let Some(subtag) = parts.next_if(|s| variant(s)) {
            if !variants.insert(subtag) {
                return None;
            }
        }
        Some(Self {
            name,
            script,
            region,
            variants,
        })
    }

    fn canonicalize(mut self) -> String {
        for &(name, variants, replacement) in VARIANT_LANGUAGES {
            if (name != self.name && name != "und")
                || !variants
                    .iter()
                    .all(|variant| self.variants.contains(variant))
            {
                continue;
            }
            let replacement = Self::parse(&mut replacement.split('-').peekable())
                .expect("generated CLDR language replacement");
            for variant in variants {
                self.variants.remove(variant);
            }
            if name != "und" {
                self.name = replacement.name;
            }
            self.script = self.script.or(replacement.script);
            self.region = self.region.or(replacement.region);
            self.variants.extend(replacement.variants);
        }
        let base = self.render().to_ascii_lowercase();
        if let Some(replacement) = alias(LANGUAGES, &base) {
            if let Some(language) = Self::parse(&mut replacement.split('-').peekable()) {
                self = language;
            }
        } else if let Some(replacement) = alias(LANGUAGES, self.name) {
            if let Some(language) = Self::parse(&mut replacement.split('-').peekable()) {
                self.name = language.name;
                self.script = self.script.or(language.script);
                self.region = self.region.or(language.region);
                self.variants.extend(language.variants);
            }
        }
        self.script = self
            .script
            .map(|script| alias(SCRIPTS, script).unwrap_or(script));
        if let Some(replacement) = self.region.and_then(|r| alias(REGIONS, r)) {
            let likely = self
                .script
                .and_then(|script| alias(LIKELY_REGIONS, &format!("{}-{script}", self.name)))
                .or_else(|| alias(LIKELY_REGIONS, self.name));
            self.region = likely
                .filter(|r| {
                    replacement
                        .split_ascii_whitespace()
                        .any(|candidate| candidate == *r)
                })
                .or_else(|| replacement.split_ascii_whitespace().next());
        }
        self.variants = self
            .variants
            .into_iter()
            .map(|v| alias(VARIANTS, v).unwrap_or(v))
            .collect();
        self.render()
    }

    fn render(&self) -> String {
        let mut result = self.name.to_owned();
        if let Some(script) = self.script {
            result.push('-');
            result.push_str(&script[..1].to_ascii_uppercase());
            result.push_str(&script[1..]);
        }
        if let Some(region) = self.region {
            result.push('-');
            result.push_str(&region.to_ascii_uppercase());
        }
        for variant in &self.variants {
            result.push('-');
            result.push_str(variant);
        }
        result
    }
}

fn unicode_extension(parts: &[&str]) -> Option<String> {
    let mut rest = parts.iter().copied().peekable();
    let mut attributes = BTreeSet::new();
    while let Some(attribute) = rest.next_if(|s| s.len() >= 3) {
        attributes.insert(attribute);
    }
    let mut fields = BTreeMap::new();
    while let Some(key) = rest.next() {
        if key.len() != 2 || !key.as_bytes()[1].is_ascii_alphabetic() {
            return None;
        }
        let mut values = Vec::new();
        while let Some(value) = rest.next_if(|s| s.len() >= 3) {
            values.push(value);
        }
        let mut value = extension_alias(key, &values.join("-"));
        if value == "true" {
            value.clear();
        }
        fields.entry(key).or_insert(value);
    }
    let mut output = attributes
        .into_iter()
        .map(str::to_owned)
        .collect::<Vec<_>>();
    for (key, value) in fields {
        output.push(key.to_owned());
        if !value.is_empty() {
            output.push(value);
        }
    }
    Some(output.join("-"))
}

fn extension_alias(key: &str, value: &str) -> String {
    if matches!(key, "rg" | "sd") {
        if let Some(replacement) = alias(SUBDIVISIONS, value) {
            let replacement = replacement
                .split_ascii_whitespace()
                .next()
                .expect("CLDR replacement");
            if region(replacement) {
                return format!("{replacement}zzzz");
            }
            return replacement.to_owned();
        }
    }
    alias(EXTENSION_ALIASES, &format!("{key}-{value}"))
        .unwrap_or(value)
        .to_owned()
}

fn transform_key(s: &str) -> bool {
    s.len() == 2 && s.as_bytes()[0].is_ascii_alphabetic() && s.as_bytes()[1].is_ascii_digit()
}

fn transformed_extension(parts: &[&str]) -> Option<String> {
    let mut rest = parts.iter().copied().peekable();
    let language = if rest.peek().is_some_and(|s| !transform_key(s)) {
        Some(
            Language::parse(&mut rest)?
                .canonicalize()
                .to_ascii_lowercase(),
        )
    } else {
        None
    };
    let mut fields = BTreeMap::new();
    while let Some(key) = rest.next() {
        if !transform_key(key) {
            return None;
        }
        let mut values = Vec::new();
        while let Some(value) = rest.next_if(|s| s.len() >= 3) {
            values.push(value);
        }
        if values.is_empty()
            || fields
                .insert(key, extension_alias(key, &values.join("-")))
                .is_some()
        {
            return None;
        }
    }
    let mut output: Vec<String> = language.into_iter().collect();
    for (key, value) in fields {
        output.push(key.to_owned());
        output.push(value);
    }
    Some(output.join("-"))
}

pub(super) fn canonicalize(tag: &str) -> Option<String> {
    let tag = tag.to_ascii_lowercase();
    let mut parts = tag.split('-').peekable();
    if parts
        .clone()
        .any(|s| !(1..=8).contains(&s.len()) || !s.bytes().all(|b| b.is_ascii_alphanumeric()))
    {
        return None;
    }
    let language = Language::parse(&mut parts)?;
    let mut extensions = BTreeMap::new();
    let mut private = None;
    while let Some(singleton) = parts.next() {
        if singleton.len() != 1 {
            return None;
        }
        if singleton == "x" {
            let tail = parts.collect::<Vec<_>>();
            if tail.is_empty() {
                return None;
            }
            private = Some(tail.join("-"));
            break;
        }
        let mut extension = Vec::new();
        while let Some(subtag) = parts.next_if(|s| s.len() > 1) {
            extension.push(subtag);
        }
        if extension.is_empty() {
            return None;
        }
        let value = match singleton {
            "u" => unicode_extension(&extension)?,
            "t" => transformed_extension(&extension)?,
            _ => extension.join("-"),
        };
        if extensions.insert(singleton, value).is_some() {
            return None;
        }
    }
    let mut output = language.canonicalize();
    for (singleton, value) in extensions {
        output.push('-');
        output.push_str(singleton);
        output.push('-');
        output.push_str(&value);
    }
    if let Some(private) = private {
        output.push_str("-x-");
        output.push_str(&private);
    }
    Some(output)
}

pub(super) fn split_base(tag: &str) -> (&str, &str) {
    tag.match_indices('-')
        .find(|(offset, _)| {
            tag[*offset + 1..]
                .split('-')
                .next()
                .is_some_and(|s| s.len() == 1)
        })
        .map_or((tag, ""), |(offset, _)| tag.split_at(offset))
}

// Canonical base names have disjoint script, region and variant productions.
pub(super) fn base_fields(base: &str) -> [&str; 4] {
    let (language, mut rest) = base.split_once('-').unwrap_or((base, ""));
    let mut fields = [language, "", "", ""];
    for (field, predicate) in [
        (1, (|s: &str| s.len() == 4 && alpha(s)) as fn(&str) -> bool),
        (2, region),
    ] {
        let (part, tail) = rest.split_once('-').unwrap_or((rest, ""));
        if predicate(part) {
            fields[field] = part;
            rest = tail;
        }
    }
    fields[3] = rest;
    fields
}

pub(super) fn valid_option(index: usize, value: &str) -> bool {
    match index {
        0 => matches!(value.len(), 2 | 3 | 5..=8) && alpha(value),
        1 => value.len() == 4 && alpha(value),
        2 => region(value),
        _ => {
            let lowercase = value.to_ascii_lowercase();
            let mut seen = BTreeSet::new();
            lowercase
                .split('-')
                .all(|s| variant(s) && alnum(s) && seen.insert(s))
        }
    }
}

fn alnum(value: &str) -> bool {
    value.bytes().all(|byte| byte.is_ascii_alphanumeric())
}

pub(super) fn valid_type(value: &str) -> bool {
    value
        .split('-')
        .all(|s| (3..=8).contains(&s.len()) && alnum(s))
}

fn split_unicode(suffix: &str) -> (&str, &str, &str) {
    let mut singletons = suffix.match_indices('-').filter_map(|(offset, _)| {
        let part = suffix[offset + 1..].split('-').next()?;
        (part.len() == 1).then_some((offset, part))
    });
    while let Some((start, singleton)) = singletons.next() {
        if singleton == "x" {
            break;
        }
        if singleton == "u" {
            let end = singletons.next().map_or(suffix.len(), |(offset, _)| offset);
            return (&suffix[..start], &suffix[start + 3..end], &suffix[end..]);
        }
    }
    (suffix, "", "")
}

fn keywords(extension: &str) -> impl Iterator<Item = (&str, &str)> {
    let mut offsets = extension
        .split('-')
        .scan(0, |offset, part| {
            let start = *offset;
            *offset += part.len() + 1;
            Some((start, part))
        })
        .filter(|(_, part)| part.len() == 2)
        .peekable();
    std::iter::from_fn(move || {
        let (start, key) = offsets.next()?;
        let end = offsets
            .peek()
            .map_or(extension.len(), |(offset, _)| offset - 1);
        let value = extension[start + 2..end]
            .strip_prefix('-')
            .unwrap_or_default();
        Some((key, value))
    })
}

pub(super) fn unicode_value<'a>(tag: &'a str, key: &str) -> Option<&'a str> {
    let (_, extension, _) = split_unicode(split_base(tag).1);
    keywords(extension).find_map(|(name, value)| (key == name).then_some(value))
}

pub(super) fn insert_unicode_values(tag: &str, overrides: &[(&str, String)]) -> String {
    if overrides.is_empty() {
        return tag.to_owned();
    }
    let (base, suffix) = split_base(tag);
    let (mut before, extension, mut after) = split_unicode(suffix);
    if extension.is_empty() {
        if let Some(offset) = suffix.find("-x-") {
            (before, after) = suffix.split_at(offset);
        }
    }
    let mut values: BTreeMap<&str, &str> = keywords(extension).collect();
    values.extend(overrides.iter().map(|(key, value)| (*key, value.as_str())));
    let mut output = format!("{base}{before}-u");
    for attribute in extension.split('-').take_while(|part| part.len() >= 3) {
        output.push('-');
        output.push_str(attribute);
    }
    for (key, value) in values {
        output.push('-');
        output.push_str(key);
        if !value.is_empty() {
            output.push('-');
            output.push_str(value);
        }
    }
    output.push_str(after);
    output
}

fn join_fields(fields: impl IntoIterator<Item = impl AsRef<str>>) -> String {
    let mut output = String::new();
    for field in fields {
        let field = field.as_ref();
        if !field.is_empty() {
            if !output.is_empty() {
                output.push('-');
            }
            output.push_str(field);
        }
    }
    output
}

fn maximize_base([language, script, region, _]: [&str; 4]) -> String {
    let script = if script == "Zzzz" { "" } else { script };
    let region = if region == "ZZ" { "" } else { region };
    let lookup = |name, script, region| {
        let key = join_fields([name, script, region]).to_ascii_lowercase();
        alias(super::data::LIKELY_SUBTAGS, &key)
    };
    let resolved = lookup(language, script, region)
        .or_else(|| lookup(language, script, ""))
        .or_else(|| lookup(language, "", region))
        .or_else(|| lookup(language, "", ""))
        .or_else(|| {
            (!script.is_empty())
                .then(|| lookup("und", script, ""))
                .flatten()
        });
    let Some(resolved) = resolved else {
        return join_fields([language, script, region]);
    };
    let [default_language, default_script, default_region, _] = base_fields(resolved);
    join_fields([
        if language == "und" {
            default_language
        } else {
            language
        },
        if script.is_empty() {
            default_script
        } else {
            script
        },
        if region.is_empty() {
            default_region
        } else {
            region
        },
    ])
}

pub(super) fn adjust_likely_subtags(tag: &str, minimize: bool) -> String {
    let (base, suffix) = split_base(tag);
    let fields = base_fields(base);
    let maximal = maximize_base(fields);
    let minimal = if minimize {
        let [language, script, region, _] = base_fields(&maximal);
        [
            [language, "", "", ""],
            [language, "", region, ""],
            [language, script, "", ""],
        ]
        .into_iter()
        .find(|candidate| maximize_base(*candidate) == maximal)
        .map(join_fields)
    } else {
        None
    };
    let mut maximal = minimal.unwrap_or(maximal);
    if !fields[3].is_empty() {
        maximal.push('-');
        maximal.push_str(fields[3]);
    }
    maximal.push_str(suffix);
    maximal
}

pub(super) fn subdivision_region(tag: &str, key: &str) -> Option<String> {
    let value = unicode_value(tag, key)?;
    let length = if value.as_bytes().first()?.is_ascii_digit() {
        3
    } else {
        2
    };
    let (prefix, suffix) = value.split_at_checked(length)?;
    if !region(prefix) || !(1..=4).contains(&suffix.len()) || !alnum(suffix) {
        return None;
    }
    let tag = canonicalize(&format!("und-{prefix}"))?;
    Some(base_fields(&tag)[2].to_owned())
}
