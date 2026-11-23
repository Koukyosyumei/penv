"""Run a paired Renode/penv benchmark manifest with GNU time on Linux."""

import argparse
import hashlib
import json
import os
from pathlib import Path
import signal
import statistics
import subprocess
import sys


def fingerprint(path):
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        while chunk := stream.read(65536):
            digest.update(chunk)
    return digest.hexdigest()


def run_trial(engine, trial, spec, artifacts, inputs, marker, timeout):
    log = artifacts / f"{engine}-{trial}"
    stdout, stderr, timing = (log.with_suffix(ext) for ext in (".uart", ".stderr", ".time"))
    argv = [str(inputs.get(arg, arg)) for arg in spec["argv"]]
    result = {"engine": engine, "trial": trial, "argv": argv, "uart": stdout.name,
              "stderr": stderr.name, "timing": timing.name}
    env = dict(os.environ, LC_ALL="C")
    with stdout.open("wb") as uart, stderr.open("wb") as errors:
        with subprocess.Popen(
            ["/usr/bin/time", "-q", "-o", str(timing), "-f", "%e %M", "--", *argv],
            cwd=spec["cwd"], env=env, stdout=uart, stderr=errors,
            stdin=subprocess.DEVNULL, start_new_session=True,
        ) as proc:
            try:
                result["exit_code"] = proc.wait(timeout=timeout)
            except subprocess.TimeoutExpired:
                try:
                    os.killpg(proc.pid, signal.SIGKILL)
                except ProcessLookupError:
                    pass
                proc.wait()
                result.update(status="timeout", exit_code=None)
                return result
    try:
        seconds, rss = timing.read_text().strip().split()
        result.update(elapsed_seconds=float(seconds), max_rss_kib=int(rss))
    except (OSError, ValueError):
        pass
    if result["exit_code"] != spec.get("success_exit_code", 0):
        result["status"] = "failed"
    elif not contains_marker(stdout, marker):
        result["status"] = "marker_missing"
    elif "elapsed_seconds" not in result:
        result["status"] = "measurement_missing"
    else:
        result["status"] = "ok"
    return result


def contains_marker(path, marker):
    tail = b""
    with path.open("rb") as stream:
        while chunk := stream.read(65536):
            block = tail + chunk
            if marker in block:
                return True
            tail = block[-(len(marker) - 1):] if len(marker) > 1 else b""
    return False


def benchmark(manifest_path, artifacts):
    manifest = json.loads(manifest_path.read_text())
    base = manifest_path.resolve().parent
    inputs = {f"{{{key}}}": (base / manifest[key]).resolve() for key in ("firmware", "input")}
    hashes = {key: {"path": str(path), "sha256": fingerprint(path)} for key, path in inputs.items()}
    marker = manifest["uart_marker"].encode("utf-8")
    repeats = manifest["repeats"]
    timeout = manifest["timeout_seconds"]
    if not marker or type(repeats) is not int or repeats < 2 or not 0 < timeout < float("inf"):
        raise ValueError("expected a nonempty UART marker, at least two repeats and a finite positive timeout")
    engines = {}
    for engine in ("renode", "penv"):
        spec = manifest["engines"][engine]
        argv = spec["argv"]
        if not isinstance(argv, list) or any(not isinstance(arg, str) for arg in argv):
            raise ValueError(f"{engine}: argv must be a list of strings")
        if not argv or not inputs.keys() <= set(argv):
            raise ValueError(f"{engine}: argv must include standalone {{firmware}} and {{input}} arguments")
        engines[engine] = dict(spec, cwd=str((base / spec.get("cwd", ".")).resolve()))
    artifacts = artifacts.resolve()
    artifacts.mkdir(parents=True, exist_ok=False)
    (artifacts / "manifest.json").write_text(json.dumps(manifest, indent=2) + "\n")
    (artifacts / "inputs.json").write_text(json.dumps(hashes, indent=2) + "\n")

    trials = []
    with (artifacts / "trials.jsonl").open("w") as raw:
        for trial in range(1, repeats + 1):
            order = ("renode", "penv") if trial % 2 else ("penv", "renode")
            for engine in order:
                result = run_trial(engine, trial, engines[engine], artifacts, inputs, marker, timeout)
                trials.append(result)
                raw.write(json.dumps(result) + "\n")
                raw.flush()
    if any(fingerprint(path) != hashes[key]["sha256"] for key, path in inputs.items()):
        raise ValueError("shared firmware or input changed during the run; trial data is invalid")
    summary = {}
    for engine in engines:
        valid = [trial for trial in trials if trial["engine"] == engine and trial["status"] == "ok"]
        summary[engine] = {"valid": len(valid), "excluded": repeats - len(valid)}
        for metric in ("elapsed_seconds", "max_rss_kib"):
            values = [trial[metric] for trial in valid]
            summary[engine][metric] = (
                {"min": min(values), "median": statistics.median(values), "max": max(values)}
                if values else None
            )
    (artifacts / "summary.json").write_text(json.dumps(summary, indent=2) + "\n")
    return 0 if all(engine["valid"] >= 2 for engine in summary.values()) else 1


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("manifest", type=Path)
    parser.add_argument("artifacts", type=Path)
    args = parser.parse_args()
    try:
        sys.exit(benchmark(args.manifest, args.artifacts))
    except (OSError, ValueError, KeyError, TypeError) as error:
        sys.exit(f"benchmark: {error}")
