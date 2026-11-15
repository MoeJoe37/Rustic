//! Number, currency, unit and plural data of every CLDR locale, read
//! from the `number.bin` pack (tools/generate_js_intl_services.py). A row
//! holds only what a locale does not inherit, so each lookup walks the
//! locale's parents, and then the fallback CLDR's root locale defines:
//! numbering systems without data of their own use the Latin data, long
//! compact forms default to short ones, and long and narrow unit names
//! default to short ones.

use std::sync::OnceLock;

use super::pack::{Pack, Table};

static PACK: Pack = Pack::new(include_bytes!("../../../../data/intl/number.bin"));

/// The number of the plural category `other`; the categories are
/// numbered in CLDR's order: zero, one, two, few, many, other.
pub(super) const OTHER: usize = 5;

/// Symbol fields, in the generator's order.
#[derive(Clone, Copy)]
pub(super) enum Symbol {
    Decimal,
    Group,
    PercentSign,
    PlusSign,
    MinusSign,
    ApproximatelySign,
    Exponential,
    PerMille = 8,
    Infinity,
    Nan,
    CurrencyDecimal,
    CurrencyGroup,
    TimeSeparator,
}

/// Pattern kinds, in the generator's numbering.
pub(super) mod pattern {
    pub(in super::super) const DECIMAL: u32 = 0;
    pub(in super::super) const PERCENT: u32 = 1;
    pub(in super::super) const CURRENCY: u32 = 3;
    pub(in super::super) const ACCOUNTING: u32 = 4;
    /// Added to a currency kind: the form for a currency sign that is
    /// spelled with letters.
    pub(in super::super) const ALPHA_NEXT_TO_NUMBER: u32 = 2;
    pub(in super::super) const RANGE: u32 = 9;
    pub(in super::super) const SPACE_BEFORE_CURRENCY: u32 = 10;
    pub(in super::super) const SPACE_AFTER_CURRENCY: u32 = 11;
    /// Plus a plural category: how a currency's name joins a number.
    pub(in super::super) const CURRENCY_UNIT: u32 = 16;
}

/// Currency fields.
pub(super) mod currency {
    pub(in super::super) const SYMBOL: u32 = 0;
    pub(in super::super) const NARROW: u32 = 1;
    pub(in super::super) const NAME: u32 = 2;
    /// Plus a plural category.
    pub(in super::super) const PLURAL_NAME: u32 = 3;
}

pub(super) struct Data {
    pub(super) locales: Table,
    systems: Table,
    numbers: Table,
    symbols: Table,
    patterns: Table,
    compact: Table,
    currency_codes: Table,
    currencies: Table,
    unit_ids: Table,
    units: Table,
    plurals: [Table; 2],
    latn: u32,
}

pub(super) fn data() -> &'static Data {
    static DATA: OnceLock<Data> = OnceLock::new();
    DATA.get_or_init(|| {
        let systems = PACK.table("nsys");
        Data {
            locales: PACK.table("locales"),
            systems,
            numbers: PACK.table("numbers"),
            symbols: PACK.table("symbols"),
            patterns: PACK.table("patterns"),
            compact: PACK.table("compact"),
            currency_codes: PACK.table("currency_codes"),
            currencies: PACK.table("currencies"),
            unit_ids: PACK.table("unit_ids"),
            units: PACK.table("units"),
            plurals: [PACK.table("plural_0"), PACK.table("plural_1")],
            latn: systems.find_text("latn").expect("the Latin digits"),
        }
    })
}

impl Data {
    pub(super) fn system_names(&self) -> impl Iterator<Item = &'static str> + '_ {
        (0..self.systems.rows()).map(|row| self.systems.text(row, 0))
    }

    pub(super) fn currency_codes(&self) -> impl Iterator<Item = &'static str> + '_ {
        (0..self.currency_codes.rows()).map(|row| self.currency_codes.text(row, 0))
    }

    /// The number of a numbering system with decimal digits.
    pub(super) fn numbering_system(&self, name: &str) -> Option<u32> {
        self.systems.find_text(name)
    }

    pub(super) fn numbering_system_name(&self, system: u32) -> &'static str {
        self.systems.text(system, 0)
    }

    /// The ten digits of a numbering system, zero first.
    pub(super) fn digits(&self, system: u32) -> &'static str {
        self.systems.text(system, 1)
    }

    /// The numbering system a locale uses by default.
    pub(super) fn default_numbering_system(&self, locale: u32) -> u32 {
        self.numbers
            .inherited(&self.locales, locale, 0, &[])
            .unwrap_or(self.latn)
    }

    /// The fewest digits a number needs before its first group is split
    /// off (2 turns 1,000 into 1000 but keeps 10,000).
    pub(super) fn minimum_grouping_digits(&self, locale: u32) -> u32 {
        self.numbers
            .inherited(&self.locales, locale, 2, &[])
            .unwrap_or(1)
    }

    pub(super) fn symbol(&self, locale: u32, system: u32, symbol: Symbol) -> &'static str {
        let field = symbol as u32;
        let id = self
            .symbols
            .inherited(&self.locales, locale, system << 8 | field, &[])
            .or_else(|| {
                self.symbols
                    .inherited(&self.locales, locale, self.latn << 8 | field, &[])
            });
        id.map_or("", |id| PACK.text(id))
    }

    pub(super) fn pattern(&self, locale: u32, system: u32, kind: u32) -> Option<&'static str> {
        self.patterns
            .inherited(&self.locales, locale, system << 8 | kind, &[])
            .or_else(|| {
                self.patterns
                    .inherited(&self.locales, locale, self.latn << 8 | kind, &[])
            })
            .map(|id| PACK.text(id))
    }

    /// The compact pattern for numbers with `zeros + 1` integer digits:
    /// `kind` is 0 short, 1 long, 2 short currency. `"0"` and `None`
    /// both mean the magnitude has no compact form.
    pub(super) fn compact(
        &self,
        locale: u32,
        system: u32,
        kind: u32,
        zeros: u32,
        category: usize,
    ) -> Option<&'static str> {
        let mut keys = vec![system << 8 | kind, self.latn << 8 | kind];
        if kind == 1 {
            keys.push(self.latn << 8);
        }
        for category in [category, OTHER] {
            for &key in &keys {
                let rest = [zeros << 8 | category as u32];
                if let Some(id) = self.compact.inherited(&self.locales, locale, key, &rest) {
                    return Some(PACK.text(id));
                }
            }
        }
        None
    }

    fn currency_row(&self, code: &str) -> Option<u32> {
        self.currency_codes.find_text(code)
    }

    /// `CurrencyDigits(currency)` (§16.5.1): the minor unit digits of
    /// ISO 4217 as CLDR records them, and 2 for an unknown code.
    pub(super) fn currency_digits(&self, code: &str) -> u32 {
        self.currency_row(code)
            .map_or(2, |row| self.currency_codes.get(row, 1))
    }

    pub(super) fn currency(&self, locale: u32, code: &str, field: u32) -> Option<&'static str> {
        let row = self.currency_row(code)?;
        self.currencies
            .inherited(&self.locales, locale, row << 4 | field, &[])
            .map(|id| PACK.text(id))
    }

    /// The number of a unit CLDR has patterns for: a sanctioned single
    /// unit, or a compound such as `kilometer-per-hour` that it
    /// translates as a whole.
    pub(super) fn unit(&self, name: &str) -> Option<u32> {
        self.unit_ids.find_text(name)
    }

    /// A unit's pattern: `width` is 0 long, 1 short, 2 narrow and
    /// `field` a plural category or 6 for the "per unit" pattern.
    pub(super) fn unit_pattern(
        &self,
        locale: u32,
        unit: u32,
        width: u32,
        field: u32,
    ) -> Option<&'static str> {
        [width, 1]
            .into_iter()
            .find_map(|width| {
                self.units
                    .inherited(&self.locales, locale, unit << 5 | width << 3 | field, &[])
            })
            .map(|id| PACK.text(id))
    }

    /// The pattern that joins two units into "X per Y".
    pub(super) fn per_pattern(&self, locale: u32, width: u32) -> &'static str {
        self.unit_pattern(locale, self.unit_ids.rows(), width, 0)
            .unwrap_or("{0}/{1}")
    }

    /// The plural rules of a locale: `kind` is 0 cardinal, 1 ordinal.
    /// The row lists which categories have rules and each rule's text.
    fn plural_row(&self, kind: usize, locale: &str) -> Option<u32> {
        let table = &self.plurals[kind];
        let mut base = locale;
        loop {
            if let Some(row) = table.find_text(base) {
                return Some(row);
            }
            match base.rsplit_once('-') {
                Some((prefix, _)) => base = prefix,
                None => return table.find_text("root"),
            }
        }
    }

    /// Which categories a locale's rules can select, as a bit per
    /// category, always with `other`.
    pub(super) fn plural_categories(&self, kind: usize, locale: &str) -> u32 {
        self.plural_row(kind, locale)
            .map_or(0, |row| self.plurals[kind].get(row, 1))
            | 1 << OTHER
    }

    /// The rule that selects `category`, empty when there is none.
    pub(super) fn plural_rule(&self, kind: usize, locale: &str, category: usize) -> &'static str {
        self.plural_row(kind, locale)
            .map_or("", |row| self.plurals[kind].text(row, 2 + category as u32))
    }
}
