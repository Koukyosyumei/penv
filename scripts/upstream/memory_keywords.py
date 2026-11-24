"""Synchronous Renode Robot memory keywords on the live penv system bus."""

import operator

from penv_robot import AssertionFailed, Unsupported


WIDTHS = {"Byte": 1, "Word": 2, "DoubleWord": 4, "QuadWord": 8}
COMPARISONS = {
    "Should Be Equal": (operator.eq, "equal to"),
    "Should Not Be Equal": (operator.ne, "unequal to"),
    "Should Be Less Than": (operator.lt, "less than"),
    "Should Be Less Or Equal": (operator.le, "less or equal"),
    "Should Be Greater Than": (operator.gt, "greater than"),
    "Should Be Greater Or Equal": (operator.ge, "greater or equal"),
    "Should Be Zero": (operator.eq, "equal to"),
    "Should Be Nonzero": (operator.ne, "unequal to"),
}
FAMILIES = ("Memory", "Symbol", "All Symbols", "Any Symbol")


def integer(value):
    value = str(value)
    if value.lstrip("+-").lower().startswith("0x"):
        return int(value, 16)
    return int(value, 10)


def current_keyword(endpoint):
    return endpoint.calls[-1]["keyword"]


class MemoryKeywords:
    memory_keywords = {"Add Symbol": "add_monitor_symbol"}
    memory_keywords.update({"Store " + " ".join(name.replace("Word", " Word").split()):
                            "store_monitor_value" for name in WIDTHS})
    memory_keywords.update({f"{family} {comparison}": "compare_monitor_memory"
                            for family in FAMILIES for comparison in COMPARISONS})

    def add_monitor_symbol(self, name, start, size=4, *args):
        if args or not name:
            raise Unsupported("symbol attributes and CPU-specific symbol tables are unavailable")
        start, size = integer(start), integer(size)
        if not 0 <= start < (1 << 64) or not 0 <= size <= (1 << 64) - start:
            raise Unsupported("symbol range exceeds the address space")
        self.ensure_running()
        self.memory_symbols.setdefault(name, set()).add((start, size))

    def symbol_addresses(self, name):
        entries = sorted(self.memory_symbols.get(name, ()))
        if not entries:
            raise Unsupported(f"symbol {name} is not registered in the Robot monitor symbol table")
        return [start for start, _ in entries]

    def store_monitor_value(self, target, value, *args):
        width = current_keyword(self).removeprefix("Store ").replace(" ", "")
        size = WIDTHS[width]
        options = self.log_options(args, {"offset", "index"}, ("offset", "index"))
        try:
            address = integer(target)
        except ValueError:
            addresses = self.symbol_addresses(target)
            if len(addresses) > 1 and "index" not in options:
                raise Unsupported(f"multiple symbols named {target} require an index")
            index = integer(options.get("index", 0))
            if index < 0 or index >= len(addresses):
                raise Unsupported("symbol index outside the registered table")
            address = addresses[index] + integer(options.get("offset", 0)) * size
        else:
            if options:
                raise Unsupported("offset and index require a symbol")
        bits = 8 * size
        value = integer(value)
        if not -(1 << (bits - 1)) <= value < (1 << bits):
            raise Unsupported(f"value does not fit {width}")
        address &= (1 << 64) - 1
        self.operation(f"sysbus Write{width} {address:#x} {value & ((1 << bits) - 1):#x}")

    def compare_monitor_memory(self, target, *args):
        family, comparison = current_keyword(self).split(" Should", 1)
        comparison = "Should" + comparison
        compare, description = COMPARISONS[comparison]
        if comparison in {"Should Be Zero", "Should Be Nonzero"}:
            expected = 0
        elif args:
            expected, args = integer(args[0]), args[1:]
        else:
            raise Unsupported("memory comparison requires an expected value")
        positional = ("width", "signed") if family == "Memory" else ("width", "signed", "offset")
        options = self.log_options(args, set(positional), positional)
        width = options.get("width", "DoubleWord")
        if width not in WIDTHS:
            raise Unsupported(f"unknown system bus access width {width}")
        size = WIDTHS[width]
        signed = self.log_bool(options.get("signed", "false"))
        offset = size * integer(options.get("offset", 0))
        if family == "Memory":
            addresses = [integer(target)]
        else:
            addresses = self.symbol_addresses(target)
            if family == "Symbol" and len(addresses) != 1:
                raise Unsupported(f"symbol {target} is ambiguous")

        def normalize(value):
            bits = 8 * size
            value &= (1 << bits) - 1
            if signed and value & (1 << (bits - 1)):
                value -= 1 << bits
            return value

        def evaluate():
            for address in addresses:
                address = (address + offset) & ((1 << 64) - 1)
                raw = self.operation(f"sysbus Read{width} {address:#x}", assertion=True)
                yield compare(normalize(integer(raw.split(maxsplit=1)[0])), normalize(expected))

        check = any if family == "Any Symbol" else all
        if not check(evaluate()):
            raise AssertionFailed(f"{family} {target} is not {description} {expected:#x}")
