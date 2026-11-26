"""Monitor token, variable and include-path handling for the Robot adapter."""

from pathlib import Path
import re

from penv_robot import Unsupported


TOKEN = re.compile(r'''"""[\s\S]*?"""|@?"(?:\\.|[^"\\])*"|'(?:\\.|[^'\\])*'|(?:\\[ \t]|[^\s])+''')
NAME = r"[A-Za-z_][\w.]*"
VARIABLE = re.compile(rf"\$({NAME})")
PUBLIC_ASSET = re.compile(r"https://dl\.antmicro\.com/projects/renode/(?:[\w.-]+/)*[\w.-]+")


def words(text):
    return TOKEN.findall(text)


def unquote(value):
    if value.startswith('"""') and value.endswith('"""'):
        return value[3:-3]
    if len(value) >= 2 and value[0] == value[-1] and value[0] in ('"', "'"):
        return value[1:-1].replace("\\" + value[0], value[0])
    return value.replace(r"\ ", " ")


def at_path(path):
    return "@" + str(path).replace(" ", r"\ ")


class MonitorInput:
    def __init__(self, root, out, assets):
        self.root, self.out, self.assets = root, out, assets
        self.global_values = {}
        self.machine_values = {}
        self.machine = None

    def name_machine(self, name):
        self.machine = re.sub(r"[^\w]", "_", name)

    def get(self, name):
        if name in {"CWD", "global.CWD"}:
            return at_path(self.out)
        if name in {"ORIGIN", "global.ORIGIN"} and name not in self.global_values:
            return at_path(self.root)
        if name.startswith("global."):
            return self.global_values.get(name[7:])
        if self.machine and name.startswith(self.machine + "."):
            return self.machine_values.get(name[len(self.machine) + 1:])
        return self.machine_values.get(name, self.global_values.get(name))

    def expand(self, token):
        seen = set()
        while match := VARIABLE.fullmatch(token):
            name = match[1]
            if name in seen:
                raise Unsupported(f"recursive monitor variable ${name}")
            seen.add(name)
            token = self.get(name)
            if token is None:
                raise Unsupported(f"unknown monitor variable ${name}")
        for name in ("$CWD/", "$ORIGIN/", "$global.ORIGIN/"):
            if token.startswith(name):
                prefix = self.get(name[1:-1])
                return prefix + "/" + token[len(name):]
        return token

    def substitute(self, command):
        # Replace only whole tokens. In particular, a quoted REPL/Python body
        # containing a dollar sign is passed through as text.
        return TOKEN.sub(lambda match: self.expand(match[0]), command)

    def assign(self, command):
        if re.match(r"set\s", command):
            tokens = words(command)
            if len(tokens) != 3 or not re.fullmatch(rf"\$?{NAME}", tokens[1]):
                raise Unsupported("set requires a variable and a single monitor token")
            name, token, conditional = tokens[1].removeprefix("$"), tokens[2], False
        elif match := re.fullmatch(rf"\$({NAME})\s*(\??=)\s*(.+)", command, re.DOTALL):
            name, token, conditional = match[1], match[3], match[2] == "?="
            if len(words(token)) != 1:
                raise Unsupported("variable assignment requires a single monitor token")
        else:
            return None
        if conditional and self.get(name) is not None:
            return name, self.get(name)
        if name.startswith("global."):
            self.global_values[name[7:]] = token
        elif self.machine:
            self.machine_values[name.removeprefix(self.machine + ".")] = token
        else:
            self.global_values[name] = token
        return name, token

    def path(self, token):
        value = unquote(self.expand(token).lstrip("@"))
        path = Path(value)
        for directory in (self.out, self.root, self.assets):
            candidate = (directory / path).resolve()
            if candidate.is_file() and any(candidate.is_relative_to(allowed)
                                           for allowed in (self.root, self.out, self.assets)):
                return candidate
        raise FileNotFoundError(f"File does not exist: {value}")

    def script_prefix(self):
        values = dict(self.global_values, **self.machine_values)
        lines = []
        for name, token in values.items():
            if "\n" in token or "\r" in token:
                raise Unsupported("multiline values in RESC preamble are not supported by penv")
            lines.append(f"set {name} {token}")
        return lines


def script_asset_urls(script, root, depth=0, seen=None):
    """Index candidate URLs without downloading branches the loader never uses."""
    if depth >= 64:
        return set()
    seen = set() if seen is None else seen
    if script in seen:
        return set()
    seen.add(script)
    source = script.read_text(errors="replace")
    urls = set(PUBLIC_ASSET.findall(source))
    for line in source.splitlines():
        tokens = words(line.strip())
        if len(tokens) != 2 or tokens[0] not in {"include", "i"}:
            continue
        value = unquote(tokens[1].removeprefix("@").removeprefix("$ORIGIN/"))
        for directory in (script.parent, root):
            candidate = (directory / value).resolve()
            if candidate.is_relative_to(root) and candidate.is_file() and candidate.suffix == ".resc":
                urls.update(script_asset_urls(candidate, root, depth + 1, seen))
                break
    return urls
