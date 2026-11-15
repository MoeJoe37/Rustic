//! Reader for the data packs that tools/generate_js_intl_services.py
//! writes: a string pool and named tables of 32-bit cells, all
//! little-endian (the layout is in tools/cldr_data.py). A pack is a static
//! byte string, so reading it allocates nothing and costs no heap budget.

/// A cell that holds no value.
pub(super) const NONE: u32 = u32::MAX;

/// A data pack.
pub(super) struct Pack {
    data: &'static [u8],
}

impl Pack {
    pub(super) const fn new(data: &'static [u8]) -> Self {
        Self { data }
    }

    fn word(&self, at: usize) -> u32 {
        let bytes = &self.data[at..at + 4];
        u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])
    }

    fn string_count(&self) -> usize {
        self.word(4) as usize
    }

    fn pool(&self) -> usize {
        12 + 4 * (self.string_count() + 1)
    }

    /// The string with number `id`; empty for [`NONE`].
    pub(super) fn text(&self, id: u32) -> &'static str {
        let id = id as usize;
        if id >= self.string_count() {
            return "";
        }
        let start = self.word(12 + 4 * id) as usize;
        let end = self.word(16 + 4 * id) as usize;
        let pool = self.pool();
        std::str::from_utf8(&self.data[pool + start..pool + end]).unwrap_or("")
    }

    /// The table called `name`. The generator and the reader agree on
    /// the names, so a missing table is a build error caught by tests.
    pub(super) fn table(&'static self, name: &str) -> Table {
        let strings = self.string_count();
        let pool_bytes = self.word(12 + 4 * strings) as usize;
        let directory = self.pool() + pool_bytes.next_multiple_of(4);
        let tables = self.word(8) as usize;
        let cells = directory + 16 * tables;
        for index in 0..tables {
            let entry = directory + 16 * index;
            if self.text(self.word(entry)) == name {
                return Table {
                    pack: self,
                    rows: self.word(entry + 4),
                    columns: self.word(entry + 8),
                    cells: cells + 4 * self.word(entry + 12) as usize,
                };
            }
        }
        panic!("data pack has no table {name}");
    }
}

/// A table of a pack: rows of 32-bit cells.
#[derive(Clone, Copy)]
pub(super) struct Table {
    pack: &'static Pack,
    rows: u32,
    columns: u32,
    cells: usize,
}

impl Table {
    pub(super) fn rows(&self) -> u32 {
        self.rows
    }

    pub(super) fn get(&self, row: u32, column: u32) -> u32 {
        self.pack
            .word(self.cells + 4 * (row as usize * self.columns as usize + column as usize))
    }

    pub(super) fn text(&self, row: u32, column: u32) -> &'static str {
        self.pack.text(self.get(row, column))
    }

    /// The row whose first cell names the string `key`, in a table
    /// sorted by the bytes of that string.
    pub(super) fn find_text(&self, key: &str) -> Option<u32> {
        let (mut low, mut high) = (0, self.rows);
        while low < high {
            let middle = low + (high - low) / 2;
            match self.text(middle, 0).as_bytes().cmp(key.as_bytes()) {
                std::cmp::Ordering::Less => low = middle + 1,
                std::cmp::Ordering::Greater => high = middle,
                std::cmp::Ordering::Equal => return Some(middle),
            }
        }
        None
    }

    /// The first row at or after which the leading cells are not below
    /// `key`, in a table sorted by those cells.
    pub(super) fn lower_bound(&self, key: &[u32]) -> u32 {
        let (mut low, mut high) = (0, self.rows);
        while low < high {
            let middle = low + (high - low) / 2;
            let below = key
                .iter()
                .enumerate()
                .map(|(column, &wanted)| self.get(middle, column as u32).cmp(&wanted))
                .find(|order| order.is_ne())
                .is_some_and(std::cmp::Ordering::is_lt);
            if below {
                low = middle + 1;
            } else {
                high = middle;
            }
        }
        low
    }

    /// The row whose leading cells equal `key`.
    pub(super) fn find(&self, key: &[u32]) -> Option<u32> {
        let row = self.lower_bound(key);
        (row < self.rows
            && key
                .iter()
                .enumerate()
                .all(|(column, &wanted)| self.get(row, column as u32) == wanted))
        .then_some(row)
    }

    /// The last cell of the row keyed `locale << 16 | key` followed by
    /// `rest`, looked up in `locale` and then in each of its parents.
    /// `locales` is the pack's locale table, whose second cell is the
    /// parent's row.
    pub(super) fn inherited(
        &self,
        locales: &Table,
        mut locale: u32,
        key: u32,
        rest: &[u32],
    ) -> Option<u32> {
        let mut cells = [0; 4];
        cells[1..=rest.len()].copy_from_slice(rest);
        loop {
            cells[0] = locale << 16 | key;
            if let Some(row) = self.find(&cells[..=rest.len()]) {
                return Some(self.get(row, self.columns - 1));
            }
            locale = locales.get(locale, 1);
            if locale == NONE {
                return None;
            }
        }
    }
}
