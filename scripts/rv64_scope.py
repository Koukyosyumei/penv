"""Resolve concrete CPU fixtures reached through local Robot keywords."""

import re


CELLS = re.compile(r"(?: {2,}|\t+)")
VARIABLE = re.compile(r"\$\{([^}]+)\}")
PLATFORM = re.compile(r'(?<![\w/])(?:@)?([\w./-]+\.(?:repl|resc))\b', re.I)
CPU = re.compile(r"\bCPU\.(RiscV32|RiscV64|VexRiscv|CV32E40P)\b")
ISA = re.compile(r'\bcpuType:\s*\\?"(rv(?:32|64)[^"\\;]+)', re.I)
EMBEDDED = re.compile(r"\$\{([^}:]+)(?::([^}]+))?\}")


class RobotFixtures:
    def __init__(self, source, path, root):
        self.path = path
        self.root = root
        self.globals = {}
        self.variables = {}
        self.keywords = []
        self.settings = {}
        section = ""
        current = []
        for number, line in enumerate(source.splitlines(), 1):
            if line.startswith("***"):
                section = line.strip("* ").casefold()
                continue
            cells = CELLS.split(line.strip())
            first = cells[0]
            if not first or first.startswith("#"):
                continue
            if section == "variables":
                if first == "..." and current:
                    current.extend(cells[1:])
                elif match := VARIABLE.fullmatch(first.rstrip("=")):
                    current = cells[1:]
                    self.variables[match[1]] = current
            elif section == "keywords":
                if line[0].isspace():
                    current.append(line)
                else:
                    current = []
                    pieces = []
                    keys = []
                    position = 0
                    for match in EMBEDDED.finditer(first):
                        pieces.extend((re.escape(first[position:match.start()]), f"({match[2] or '.+?'})"))
                        keys.append(match[1])
                        position = match.end()
                    pieces.append(re.escape(first[position:]))
                    self.keywords.append((number, re.compile("".join(pieces), re.I), keys, current))
            elif section == "settings" and first in {"Test Setup", "Suite Setup", "Test Template"}:
                self.settings[first] = cells[1:]

        self.globals = {name.casefold(): " ".join(cells) for name, cells in self.variables.items()}
        self.globals.update({"curdir": str(path.parent.resolve()), "/": "/"})

    def resolve(self, text, arguments):
        def substitute(value, active):
            def replace(match):
                name = match[1]
                if name in active:
                    return match[0]
                resolved = arguments.get(name, self.globals.get(name.casefold(), match[0]))
                return substitute(resolved, active | {name})

            return VARIABLE.sub(replace, value)

        return substitute(text, set())

    def evidence(self, body, parameter=None):
        found = set()
        settings = dict(self.settings)
        records = [CELLS.split(line.strip()) for line in body]
        for cells in records:
            overrides = {"[Setup]": "Test Setup", "[Template]": "Test Template"}
            if cells[0] in overrides:
                settings[overrides[cells[0]]] = cells[1:]
        for name in ("Test Setup", "Suite Setup"):
            self.walk(settings.get(name, []), {}, set(), found, "settings")
        if parameter is not None:
            self.walk(settings.get("Test Template", []) + CELLS.split(parameter), {}, set(), found, "template")
        else:
            for cells in records:
                if not cells[0].startswith(("[", "#")):
                    self.walk(cells, {}, set(), found, "case body")
        return sorted(found)

    def walk(self, cells, arguments, visited, found, origin):
        if not cells:
            return
        cells = [self.resolve(cell, arguments) for cell in cells]
        statement = " ".join(cells)
        self.scan(statement, found, origin, set(), self.path.parent)

        for index, cell in enumerate(cells):
            for number, pattern, keys, body in self.keywords:
                match = pattern.fullmatch(cell)
                if not match or number in visited:
                    continue
                local = dict(zip(keys, match.groups()))
                records = [CELLS.split(line.strip()) for line in body]
                for record in records:
                    if record[0] != "[Arguments]":
                        continue
                    provided = cells[index + 1:]
                    named = {key: value for arg in provided if "=" in arg for key, value in [arg.split("=", 1)]}
                    positional = iter(arg for arg in provided if "=" not in arg)
                    for arg in record[1:]:
                        variable, _, default = arg.partition("=")
                        if key := VARIABLE.fullmatch(variable):
                            local[key[1]] = named[key[1]] if key[1] in named else next(positional, default)
                for record in records:
                    if not record[0].startswith(("[", "#")):
                        self.walk(record, local, visited | {number}, found, f"keyword line {number}")

    def scan(self, statement, found, origin, visited, parent):
        isa = ISA.search(statement)
        for cpu in CPU.findall(statement):
            found.add((cpu, isa[1] if isa else "", origin))
        for reference in PLATFORM.findall(statement):
            if "$" in reference or "://" in statement:
                continue
            for candidate in (self.root / reference, parent / reference):
                path = candidate.resolve()
                if path in visited or not path.is_relative_to(self.root) or not path.is_file():
                    continue
                relative = path.relative_to(self.root)
                for line in path.read_text(encoding="utf-8-sig", errors="replace").splitlines():
                    if line.lstrip().startswith(("#", "//")):
                        continue
                    self.scan(line, found, str(relative), visited | {path}, path.parent)
                break
