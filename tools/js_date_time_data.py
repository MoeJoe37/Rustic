"""Generate DateTimeFormat's Gregorian LDML patterns and names."""

from cldr_data import STEP, Pack, path

STYLES = ("full", "long", "medium", "short")
WIDTHS = ("abbreviated", "wide", "narrow")
WEEKDAYS = ("sun", "mon", "tue", "wed", "thu", "fri", "sat")
PERIODS = ("am", "pm", "midnight", "noon", "morning1", "morning2", "afternoon1",
           "afternoon2", "evening1", "evening2", "night1", "night2")
GREGORIAN = ("dates", "calendars", "calendar[@type='gregorian']")
SKELETON_START = 512
INTERVAL_START = 4096
DIFFERENCES = {"E": 0, "G": 1, "y": 2, "M": 3, "d": 4, "a": 5,
               "B": 5, "h": 6, "H": 6, "m": 7, "s": 8, "S": 9, "z": 10}


def clock_minutes(text):
    hour, minute = map(int, text.split(":"))
    return hour * 60 + minute


def pack_date_time(cldr, locale_rows):
    """Record sparse values keyed by locale, and supplemental period rules."""
    pack = Pack()
    locales = locale_rows(cldr, pack)
    prefixes = path(*GREGORIAN, "dateTimeFormats", "availableFormats")
    intervals_path = path(*GREGORIAN, "dateTimeFormats", "intervalFormats")
    available, intervals = {}, {}
    skeletons = set()
    for locale in locales.names:
        values = {}
        for tail, pattern in cldr.subtree(locale, prefixes).items():
            if len(tail) != 1:
                continue
            attrs = dict(STEP.findall(tail[0]))
            if attrs.keys() == {"id"}:
                key = attrs["id"]
                values[key] = pattern
                skeletons.add(key)
        available[locale] = values
        values = {}
        for tail, pattern in cldr.subtree(locale, intervals_path).items():
            if len(tail) != 2 or not tail[0].startswith("intervalFormatItem["):
                continue
            skeleton = dict(STEP.findall(tail[0])).get("id")
            difference = DIFFERENCES.get(dict(STEP.findall(tail[1])).get("id"))
            if skeleton is not None and difference is not None:
                values[(skeleton, difference)] = pattern
                skeletons.add(skeleton)
        intervals[locale] = values
    skeletons = {key: index for index, key in enumerate(sorted(skeletons))}
    pack.table("skeletons", 1, ((pack.s(key),) for key in skeletons))

    def rows(locale):
        out = {}

        def add(index, text):
            if text:
                out[index] = pack.s(text)

        calendar = cldr.subtree(locale, path(*GREGORIAN))

        def date(index, *nodes):
            add(index, calendar.get(path(*nodes)))

        for index, style in enumerate(STYLES):
            date(index, "dateFormats", f"dateFormatLength[@type='{style}']",
                 "dateFormat", "pattern")
            date(index + 4, "timeFormats", f"timeFormatLength[@type='{style}']",
                 "timeFormat", "pattern")
            folder = ("dateTimeFormats", f"dateTimeFormatLength[@type='{style}']")
            base = calendar.get(path(*folder, "dateTimeFormat", "pattern"))
            # CLDR also supplies a join pattern for an explicit time of day.
            at_time = calendar.get(path(*folder, "dateTimeFormat[@type='atTime']", "pattern"))
            add(index + 8, at_time if index < 2 and at_time else base)
            add(index + 16, base)
        date(12, "dateTimeFormats", "intervalFormats", "intervalFormatFallback")

        zones = cldr.subtree(locale, path("dates", "timeZoneNames"))
        for index, name in enumerate(("gmtFormat", "gmtZeroFormat", "hourFormat"), start=13):
            add(index, zones.get((name,)))
        add(20, zones.get(path("zone[@type='Etc/UTC']", "long", "standard")))

        buddhist = cldr.subtree(locale, path("dates", "calendars", "calendar[@type='buddhist']", "eras"))
        islamic = cldr.subtree(locale, path("dates", "calendars", "calendar[@type='islamic']", "months"))
        islamic_eras = cldr.subtree(locale, path("dates", "calendars", "calendar[@type='islamic']", "eras"))
        for width, style in enumerate(WIDTHS):
            for context, mode in enumerate(("format", "stand-alone")):
                for number in range(12):
                    date(64 + 36 * context + 12 * width + number,
                         "months", f"monthContext[@type='{mode}']",
                         f"monthWidth[@type='{style}']", f"month[@type='{number + 1}']")
                    add(288 + 36 * context + 12 * width + number, islamic.get(path(
                        f"monthContext[@type='{mode}']", f"monthWidth[@type='{style}']",
                        f"month[@type='{number + 1}']")))
            for number, day in enumerate(WEEKDAYS):
                date(144 + 7 * width + number, "days", "dayContext[@type='format']",
                     f"dayWidth[@type='{style}']", f"day[@type='{day}']")
            era_width = ("eraAbbr", "eraNames", "eraNarrow")[width]
            add(252 + width, buddhist.get(path(era_width, "era[@type='0']")))
            add(256 + width, islamic_eras.get(path(era_width, "era[@type='0']")))
            for number in range(2):
                date(192 + 2 * width + number, "eras", era_width, f"era[@type='{number}']")
            for number, period in enumerate(PERIODS):
                date(216 + 12 * width + number, "dayPeriods", "dayPeriodContext[@type='format']",
                     f"dayPeriodWidth[@type='{style}']", f"dayPeriod[@type='{period}']")

        for key, pattern in available[locale].items():
            add(SKELETON_START + skeletons[key], pattern)
        for (key, difference), pattern in intervals[locale].items():
            add(INTERVAL_START + 16 * skeletons[key] + difference, pattern)
        return out

    pack.table("values", 2, sorted(
        (row << 16 | key, value) for row, key, value in locales.sparse(rows)
    ))

    clock_periods = []
    rules = cldr.supplemental("dayPeriods").find("dayPeriodRuleSet")
    for rule in rules.findall("dayPeriodRules"):
        for name in rule.attrib["locales"].split():
            for item in rule.findall("dayPeriodRule"):
                if "at" in item.attrib:
                    begin = end = clock_minutes(item.attrib["at"])
                else:
                    begin = clock_minutes(item.attrib["from"])
                    end = clock_minutes(item.attrib["before"])
                clock_periods.append((name.replace("_", "-"), begin, end, PERIODS.index(item.attrib["type"])))
    clock_periods.sort()
    pack.table("periods", 4, ((pack.s(name), begin, end, period)
                              for name, begin, end, period in clock_periods))
    cycle_options = {}
    for entry in cldr.supplemental("supplementalData").findall("timeData/hours"):
        allowed = [value[0] for value in entry.attrib["allowed"].split() if value[0] in "KhHk"]
        half_day = next((cycle for cycle in allowed if cycle in "Kh"), "h")
        full_day = next((cycle for cycle in allowed if cycle in "Hk"), "H")
        for region in entry.attrib["regions"].split():
            cycle_options[region] = (ord(half_day), ord(full_day))
    pack.table("cycles", 3, ((pack.s(region), *cycles)
                             for region, cycles in sorted(cycle_options.items())))
    return pack.bytes()
