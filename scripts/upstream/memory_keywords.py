"""Synchronous Renode Robot memory keywords on the live penv system bus."""

from penv_robot import Unsupported


WIDTHS = {"Byte": 1, "Word": 2, "DoubleWord": 4, "QuadWord": 8}
COMPARISONS = {
    "Should Be Equal": "eq",
    "Should Not Be Equal": "ne",
    "Should Be Less Than": "lt",
    "Should Be Less Or Equal": "le",
    "Should Be Greater Than": "gt",
    "Should Be Greater Or Equal": "ge",
    "Should Be Zero": "eq",
    "Should Be Nonzero": "ne",
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
    memory_keywords = {"Add Symbol": "add_monitor_symbol",
                       "Store Assembled Code": "store_assembled_code"}
    memory_keywords.update({"Store " + " ".join(name.replace("Word", " Word").split()):
                            "store_monitor_value" for name in WIDTHS})
    memory_keywords["All Symbols Should Independently Be Equal"] = "compare_monitor_memory"
    memory_keywords.update({f"{family} {comparison}": "compare_monitor_memory"
                            for family in FAMILIES for comparison in COMPARISONS})

    def store_assembled_code(self, address, assembly, *args):
        positional = ("triple", "alternateDialect", "machine", "cpu")
        options = self.log_options(args, set(positional), positional)
        if self.log_bool(options.get("alternateDialect", "false")):
            raise Unsupported("alternate assembly dialect is unavailable")
        triple = options.get("triple") or "default"
        if triple == "null":
            triple = "default"
        if not triple.isascii() or any(not (character.isalnum() or character in "-_.") for character in triple):
            raise Unsupported(f"invalid assembler triple {triple!r}")
        if machine := options.get("machine"):
            self.operation(f"mach set {self.literal(machine)}")
        cpu = "-" if "cpu" not in options else f"index:{integer(options['cpu'])}"
        source = assembly.strip("\"'").encode().hex()
        self.operation(f"AssembleMonitorBlock {cpu} {integer(address):#x} {triple} {source}")

    def add_monitor_symbol(self, name, start, size=4, *args):
        if args or not name:
            raise Unsupported("symbol attributes and CPU-specific symbol tables are unavailable")
        start, size = integer(start), integer(size)
        if not 0 <= start < (1 << 64) or not 0 <= size <= (1 << 64) - start:
            raise Unsupported("symbol range exceeds the address space")
        self.operation(f"addMemorySymbol {self.literal(name)} {start:#x} {size:#x}")

    def symbol_addresses(self, name, single):
        mode = "single" if single else "all"
        addresses = self.operation(f"resolveMemorySymbols {self.literal(name)} {mode}")
        return [integer(value) for value in addresses.split()]

    def store_monitor_value(self, target, value, *args):
        width = current_keyword(self).removeprefix("Store ").replace(" ", "")
        size = WIDTHS[width]
        options = self.log_options(args, {"offset", "index"}, ("offset", "index"))
        kind, _, specified = target.partition("=")
        if kind in {"address", "symbol"}:
            target = specified
        if kind == "symbol":
            address = None
        elif kind == "address":
            address = integer(target)
        else:
            try:
                address = integer(target)
            except ValueError:
                address = None
        if address is None:
            addresses = self.symbol_addresses(target, "index" not in options)
            index = integer(options.get("index", 0))
            if index < 0 or index >= len(addresses):
                raise Unsupported("symbol index outside the registered table")
            address = addresses[index] + integer(options.get("offset", 0)) * size
        elif options:
            raise Unsupported("offset and index require a symbol")
        value = value.removeprefix("value=")
        bits = 8 * size
        value = integer(value)
        if not -(1 << (bits - 1)) <= value < (1 << bits):
            raise Unsupported(f"value does not fit {width}")
        address &= (1 << 64) - 1
        self.operation(f"sysbus Write{width} {address:#x} {value & ((1 << bits) - 1):#x}")

    def compare_monitor_memory(self, target, *args):
        family, comparison = current_keyword(self).split(" Should", 1)
        independent = comparison.startswith(" Independently ")
        comparison = "Should" + comparison.replace(" Independently ", " ", 1)
        relation = COMPARISONS[comparison]
        if comparison in {"Should Be Zero", "Should Be Nonzero"}:
            expected = 0
        elif args:
            expected, args = integer(args[0]), args[1:]
        else:
            raise Unsupported("memory comparison requires an expected value")
        positional = ("width", "signed") if family == "Memory" else ("width", "signed", "offset")
        positional += ("timeout", "machine")
        if family != "Memory":
            positional += ("context",)
        positional += ("pauseEmulation",)
        options = self.log_options(args, set(positional), positional)
        if options.get("context"):
            raise Unsupported("CPU-specific symbol tables are unavailable")
        if machine := options.get("machine"):
            self.operation(f"mach set {self.literal(machine)}")
        width = options.get("width", "DoubleWord")
        if width not in WIDTHS:
            raise Unsupported(f"unknown system bus access width {width}")
        size = WIDTHS[width]
        signed = self.log_bool(options.get("signed", "false"))
        offset = size * integer(options.get("offset", 0))
        if family == "Memory":
            addresses = [integer(target.removeprefix("address="))]
        else:
            addresses = self.symbol_addresses(target.removeprefix("symbol="), family == "Symbol")

        addresses = [(address + offset) & ((1 << 64) - 1) for address in addresses]
        group = "independently" if independent else "any" if family == "Any Symbol" else "all"
        attributes = f"{group} {relation} {size} {str(signed).lower()} {expected:#x}"
        targets = " ".join(f"{address:#x}" for address in addresses)
        if "timeout" in options:
            pause = str(self.log_bool(options.get("pauseEmulation", "false"))).lower()
            timeout = self.duration(options["timeout"])
            self.operation(f"waitForMemory {attributes} {timeout} {pause} {targets}", assertion=True)
        else:
            self.operation(f"checkMemory {attributes} {targets}", assertion=True)
