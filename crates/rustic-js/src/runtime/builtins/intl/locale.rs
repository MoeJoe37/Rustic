//! Unicode BCP 47 locale identifier structure and canonical casing.

use std::collections::{BTreeMap, BTreeSet};

use super::data::{
    EXTENSION_ALIASES, LANGUAGES, LIKELY_REGIONS, REGIONS, SCRIPTS, SUBDIVISIONS, VARIANTS,
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
        if self.variants.contains("hepburn") && self.variants.remove("heploc") {
            self.variants.remove("hepburn");
            self.variants.insert("alalc97");
        }
        self.variants.remove("arevela");
        self.variants.remove("arevmda");
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
            return replacement
                .split_ascii_whitespace()
                .next()
                .expect("CLDR subdivision replacement")
                .to_owned();
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
