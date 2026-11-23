"""Run the fixed RV32 PMP Robot programs as ELFs."""

import argparse
import json
from pathlib import Path
import re
import subprocess


def assembly(block, name):
    lines = re.search(
        rf"(?m)^    \$\{{{name}\}}=.*\n((?:    \.\.\..*\n)+)", block,
    )[1]
    return "\n".join(line.split("...", 1)[1].strip() for line in lines.splitlines())


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("penv", type=Path)
    parser.add_argument("renode", type=Path)
    parser.add_argument("output", type=Path)
    args = parser.parse_args()
    robot = (args.renode / "tests/unit-tests/pmp.robot").read_text()
    pc, mepc, mtvec, dummy, fault = (
        int(re.search(rf"^\$\{{{name}\}}\s+(0x[0-9a-f]+)", robot, re.M)[1], 16)
        for name in ("PC", "MEPC", "MTVEC", "DUMMY_VALUE", "INSTRUCTION_ACCESS_FAULT")
    )
    cases = re.findall(
        r"(?ms)^(Should (?:Not Throw Exception After MRET|Deny User Mode Access|Allow User Mode Access)[^\n]*)\n"
        r"(.*?)(?=^\S|\Z)", robot,
    )
    if len(cases) != 3:
        parser.error(f"expected three fixed RV32 PMP cases, found {len(cases)}")
    plain_machine = assembly(robot.split("Enter User Mode Without PMP Configuration\n", 1)[1], "machine_assembly")
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    linker = output / "layout.ld"
    linker.write_text(
        "ENTRY(_start)\nPHDRS { image PT_LOAD FLAGS(7); }\n"
        f"SECTIONS {{ .machine {pc:#x} : {{ *(.machine) }} :image "
        f".user {mepc:#x} : {{ *(.user) }} :image .data 0x80002000 : {{ *(.data) }} :image "
        f".text 0x80003000 : {{ *(.text) }} :image .trap {mtvec:#x} : {{ *(.trap) }} :image }}\n"
    )
    results = []
    for index, (name, body) in enumerate(cases):
        napot = "NAPOT" in name
        deny = name.startswith("Should Deny")
        fixture = "RV32_NO_PMP" if "Create RV32 No PMP Machine" in body else "RV32_PRIV10"
        template = robot.split(f"${{{fixture}}}=", 1)[1].split('\n...  """', 2)[1]
        repl = "\n".join(
            line.removeprefix("...  ").replace("${SPACE*4}", "    ")
            for line in template.splitlines() if line.startswith("...  ")
        ) + "\n"
        source = output / f"{index}.S"
        user = assembly(body, "user_assembly") if napot else "nop"
        if napot:
            user += f"\nlw t0, 4(s0)\nli t1, {dummy}\nbeq t0, t1, 1f\nj fail\n1:"
        machine = assembly(body, "machine_assembly") if napot else plain_machine
        trap = (
            f"csrr t1, mcause\nli t2, {fault}\nbne t1, t2, trap_fail\n"
            f"csrr t1, mepc\nli t2, {mepc}\nbne t1, t2, trap_fail\n"
            "j pass\ntrap_fail: j fail"
        ) if deny else "j fail"
        source.write_text(
            f'.option norvc\n.section .machine, "ax"\nmachine:\n{machine}\n'
            f'.section .user, "ax"\n{user}\nj {"fail" if deny else "pass"}\n'
            f'.data\n.word {dummy}, 0\n'
            f".text\n.global _start\n_start:\nli t0, {mtvec}\ncsrw mtvec, t0\n"
            f"li t0, {mepc}\ncsrw mepc, t0\nj machine\n"
            "pass: li t1, 1\nj done\nfail: li t1, 3\n"
            "done: li t2, 0x80002008\nsw t1, 0(t2)\nj .\n"
            f'.section .trap, "ax"\n{trap}\n'
        )
        elf = source.with_suffix(".elf")
        subprocess.run(
            ["clang", "--target=riscv32-unknown-elf", "-march=rv32imac", "-mabi=ilp32",
             "-nostdlib", "-fuse-ld=lld", f"-Wl,-T,{linker}", source, "-o", elf],
            check=True, timeout=15,
        )
        for mode, config in (("diagnostic", repl.replace('"rv32gcv"', '"rv32gc"')), ("fixed", repl)):
            platform = output / f"{index}-{mode}.repl"
            platform.write_text(config)
            run = subprocess.run(
                [args.penv.resolve(), "run", "--platform", platform, "--tohost", "0x80002008",
                 "--max-steps", "64", elf],
                capture_output=True, text=True, timeout=15,
            )
            log = platform.with_suffix(".log")
            log.write_text(run.stdout + run.stderr)
            passed = run.returncode == 0 and "penv: tohost exit code 0" in run.stderr
            results.append(dict(case=name, mode=mode, passed=passed, exit_code=run.returncode, log=log.name))
            print(f"{'PASS' if passed else 'FAIL'} {mode}: {name}")
    (output / "results.jsonl").write_text("".join(json.dumps(result) + "\n" for result in results))
    return 0 if all(result["passed"] for result in results if result["mode"] == "diagnostic") else 1


if __name__ == "__main__":
    raise SystemExit(main())
