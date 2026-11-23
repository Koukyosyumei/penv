"""Exercise the benchmark runner with local dummy processes only."""

import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest


class BenchmarkTest(unittest.TestCase):
    def test_only_successful_marker_runs_are_aggregated(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "fw").write_bytes(b"firmware")
            (root / "input").write_bytes(b"completion")
            (root / "dummy.py").write_text("""
from pathlib import Path
import sys
import time
engine, firmware, input_file = sys.argv[1:]
assert Path(firmware).read_bytes() == b"firmware"
marker = Path(input_file).read_bytes()
count = Path(engine + ".count")
trial = int(count.read_text()) + 1 if count.exists() else 1
count.write_text(str(trial))
if trial == 5:
    time.sleep(60)
if trial != 4:
    sys.stdout.buffer.write(b"A" * 65534 + marker)
sys.exit(3 if trial == 3 else 2 if engine == "penv" else 0)
""")
            engines = {
                engine: {"argv": [sys.executable, "dummy.py", engine, "{firmware}", "{input}"],
                         "success_exit_code": exit_code}
                for engine, exit_code in (("renode", 0), ("penv", 2))
            }
            manifest = root / "bench.json"
            manifest.write_text(json.dumps({
                "firmware": "fw", "input": "input", "uart_marker": "completion",
                "repeats": 5, "timeout_seconds": 0.5, "engines": engines,
            }))
            output = root / "results"
            runner = Path(__file__).with_name("benchmark-riscv.py")
            subprocess.run([sys.executable, runner, manifest, output], check=True)
            trials = [json.loads(line) for line in (output / "trials.jsonl").read_text().splitlines()]
            summary = json.loads((output / "summary.json").read_text())
            self.assertEqual(len(trials), 10)
            for name in engines:
                selected = [trial for trial in trials if trial["engine"] == name]
                self.assertEqual([trial["status"] for trial in selected],
                                 ["ok", "ok", "failed", "marker_missing", "timeout"])
                self.assertEqual(summary[name]["valid"], 2)
                self.assertEqual(summary[name]["excluded"], 3)
                for metric in ("elapsed_seconds", "max_rss_kib"):
                    values = [trial[metric] for trial in selected[:2]]
                    self.assertEqual(summary[name][metric],
                                     {"min": min(values), "median": sum(values) / 2, "max": max(values)})


if __name__ == "__main__":
    unittest.main()
