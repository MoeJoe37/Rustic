"""Read CLDR locale XML with inheritance and aliases, and write data packs.

Shared by the generators of Rustic's Intl service data. A locale's files are
resolved the way LDML describes: a value missing from a locale comes from its
parent chain, and a path missing from the whole chain follows the alias that
`root` gives for it or for one of its ancestors, starting again at the
requested locale.
"""

import re
import struct
import xml.etree.ElementTree as ET
from io import BytesIO
from zipfile import ZipFile

STEP = re.compile(r"\[@([A-Za-z0-9_:-]+)=['\"]([^'\"]*)['\"]\]")
IGNORED = {"draft", "references", "cp"}
UNCONFIRMED = {"unconfirmed", "provisional"}
VALUE = ""
ALIAS = "@alias"
NONE = 0xFFFF_FFFF


def step(tag, attributes):
    return tag + "".join(f"[@{key}='{attributes[key]}']" for key in sorted(attributes))


def parse_step(text):
    tag = text.split("[", 1)[0]
    return step(tag, dict(STEP.findall(text)))


def path(*steps):
    """A path from readable steps: `path("numbers", "symbols[@numberSystem='latn']")`."""
    return tuple(parse_step(part) for part in steps)


class Cldr:
    """The `common/` tree of a CLDR core.zip."""

    def __init__(self, raw):
        self.archive = ZipFile(BytesIO(raw))
        self.trees = {}
        self.memo = {}
        self.locales = sorted(
            name[len("common/main/"):-4].replace("_", "-")
            for name in self.archive.namelist()
            if name.startswith("common/main/") and name.endswith(".xml")
        )
        supplemental = self.supplemental("supplementalData")
        self.parents = {}
        for table in supplemental.findall("parentLocales"):
            if "component" in table.attrib:
                continue
            for entry in table.findall("parentLocale"):
                for tag in entry.attrib["locales"].split():
                    self.parents[tag.replace("_", "-")] = entry.attrib["parent"].replace("_", "-")

    def supplemental(self, name):
        return ET.fromstring(self.archive.read(f"common/supplemental/{name}.xml"))

    def xml(self, name):
        return ET.fromstring(self.archive.read(f"common/{name}"))

    def parent(self, locale):
        if locale == "root":
            return None
        if locale in self.parents:
            return self.parents[locale]
        return locale.rpartition("-")[0] or "root"

    def chain(self, locale):
        result = []
        while locale is not None:
            result.append(locale)
            locale = self.parent(locale)
        return result

    def tree(self, locale):
        if locale not in self.trees:
            name = f"common/main/{locale.replace('-', '_')}.xml"
            tree = {}
            if name in self.archive.namelist():
                self._load(ET.fromstring(self.archive.read(name)), tree, ())
            self.trees[locale] = tree
        return self.trees[locale]

    def _load(self, element, node, here):
        for child in element:
            if child.get("draft") in UNCONFIRMED:
                continue
            if child.tag == "alias":
                target = list(here)
                for part in child.attrib["path"].split("/"):
                    if part == "..":
                        target.pop()
                    else:
                        target.append(parse_step(part))
                node[ALIAS] = tuple(target)
                continue
            attributes = {k: v for k, v in child.attrib.items() if k not in IGNORED}
            name = step(child.tag, attributes)
            below = node.setdefault(name, {})
            if len(child) == 0:
                text = child.text or ""
                if text != "↑↑↑":
                    below[VALUE] = text
            else:
                self._load(child, below, here + (name,))

    @staticmethod
    def _walk(node, prefix):
        for part in prefix:
            node = node.get(part)
            if node is None:
                return None
        return node

    @staticmethod
    def _flatten(node, here, out):
        for name, below in node.items():
            if name == VALUE:
                out[here] = below
            elif name != ALIAS:
                Cldr._flatten(below, here + (name,), out)

    def _aliases_below(self, node, here, out):
        for name, below in node.items():
            if name == ALIAS:
                if here:
                    out.append((here, below))
            elif name != VALUE:
                self._aliases_below(below, here + (name,), out)

    def subtree(self, locale, prefix):
        """Every value at or below `prefix` for `locale`, by relative path."""
        key = (locale, prefix)
        if key in self.memo:
            return self.memo[key]
        self.memo[key] = {}  # A cycle of aliases contributes nothing.
        result = {}
        root = self.tree("root")
        node = root
        for index, part in enumerate(prefix):
            node = node.get(part) if node is not None else None
            if node is not None and ALIAS in node:
                result.update(self.subtree(locale, node[ALIAS] + prefix[index + 1:]))
        below = self._walk(root, prefix)
        if below is not None:
            aliases = []
            self._aliases_below(below, (), aliases)
            for relative, target in aliases:
                for rest, value in self.subtree(locale, target).items():
                    result[relative + rest] = value
        for member in reversed(self.chain(locale)):
            below = self._walk(self.tree(member), prefix)
            if below is not None:
                self._flatten(below, (), result)
        self.memo[key] = result
        return result

    def get(self, locale, *steps):
        return self.subtree(locale, path(*steps)).get(())


class Pack:
    """A string pool and named tables of 32-bit cells, little-endian.

    Layout: magic `RJP1`, string count, table count, string offsets
    (count + 1), the UTF-8 pool padded to four bytes, then per table its
    name's string id, row count, column count and first cell index, then
    the cells.
    """

    def __init__(self):
        self.ids = {"": 0}
        self.strings = [""]
        self.tables = []

    def s(self, text):
        if text is None:
            return NONE
        if text not in self.ids:
            self.ids[text] = len(self.strings)
            self.strings.append(text)
        return self.ids[text]

    def table(self, name, columns, rows):
        rows = [list(row) for row in rows]
        assert all(len(row) == columns for row in rows), name
        self.tables.append((self.s(name), columns, rows))

    def bytes(self):
        pool = bytearray()
        offsets = []
        for text in self.strings:
            offsets.append(len(pool))
            pool += text.encode("utf-8")
        offsets.append(len(pool))
        pool += b"\0" * (-len(pool) % 4)
        out = bytearray(b"RJP1")
        out += struct.pack("<II", len(self.strings), len(self.tables))
        out += struct.pack(f"<{len(offsets)}I", *offsets)
        out += pool
        start = 0
        cells = []
        for name, columns, rows in self.tables:
            out += struct.pack("<IIII", name, len(rows), columns, start)
            for row in rows:
                cells.extend(row)
            start += len(rows) * columns
        out += struct.pack(f"<{len(cells)}I", *cells)
        return bytes(out)
