#!/usr/bin/env python3
from pathlib import Path
from itertools import product
import csv
import re
import subprocess
import sys
if len(sys.argv) != 3:
    sys.exit("usage: inventory-arm-upstream.py RENODE_ROOT INFRASTRUCTURE_ROOT")
renode, infra = map(Path, sys.argv[1:])
expected_pin = "c38ac3babcd38a7144b9498e3ca4c2226b76aa37"
actual_pin = subprocess.check_output(
    ["git", "-C", str(renode), "rev-parse", "HEAD"], text=True
).strip()
if actual_pin != expected_pin:
    sys.exit(f"expected Renode {expected_pin}, got {actual_pin}")
expected_infra = "09ac7c563095c5410258ecc1b1a6b841a013db85"
if (infra / ".git").exists():
    actual_infra = subprocess.check_output(
        ["git", "-C", str(infra), "rev-parse", "HEAD"], text=True
    ).strip()
    if actual_infra != expected_infra:
        sys.exit(f"expected Infrastructure {expected_infra}, got {actual_infra}")
if not (infra / "src/Emulator/Main/Tests/UnitTests/LimitTimerTests.cs").is_file():
    sys.exit("expected the pinned Renode Infrastructure source tree")
paths = {}
robot_prefixes = (
    "arm-cortex-m", "arm-thumb", "stm32", "uart-tester", "host-uart",
    "it-status", "cpu-pause", "per-core-registration", "hex", "profiler-trace",
    "timer-reconfiguration", "real-time-clock-mode",
)
arm_fixture = re.compile(
    r"cortex-m0|stm32f0|stm32_uart|stm32f7_usart|GPIOPort\.STM32|Timers\.STM32_Timer", re.I
)
for path in (renode / "tests").rglob("*.robot"):
    name = path.stem.lower()
    if (name.startswith(robot_prefixes) or arm_fixture.search(path.read_text())
            or name in ("thumb2-data-processing-instr", "nucleo_wba52cg")):
        paths[path] = "Renode"

FAULT_PORTS = {
    'Should Enter Lockup On Invalid Reset State': 'renode_invalid_reset_state_enters_lockup_after_two_steps',
    'Should Escalate Disabled BusFault To HardFault': 'renode_precise_load_hardfault_then_lockup_nmi_and_reset, first fault',
    'Should Enter Lockup When BusFault Cannot Escalate From HardFault': 'renode_precise_load_hardfault_then_lockup_nmi_and_reset, repeated LDR; ITSTATE not applicable',
    'NMI Should Preempt Instruction-Time Lockup': 'renode_precise_load_hardfault_then_lockup_nmi_and_reset, NMI and stacked LOCKUP PC',
    'Should Enter Lockup On NMI Vector BusFault': 'renode_nmi_and_hardfault_faulting_vectors_lock_up, exception 2',
    'Should Enter Lockup On HardFault Vector BusFault': 'renode_nmi_and_hardfault_faulting_vectors_lock_up, exception 3',
    'Should Pend HardFault On NMI Stacking BusFault From Thread Mode': 'renode_thread_stacking_fault_pends_or_escalates_to_hardfault, NMI',
    'Should Replace External IRQ With HardFault On Stacking BusFault': 'renode_thread_stacking_fault_pends_or_escalates_to_hardfault, IRQ 16',
    'Should Enter Lockup On NMI Stacking BusFault': 'renode_nmi_stacking_fault_over_hardfault_allocates_frame_and_locks_up',
    'Should Record Ignored Vector BusFault Before Stacking Lockup': 'renode_nmi_stacking_fault_over_hardfault_allocates_frame_and_locks_up, faulting VTOR case',
    'Should Enter Lockup On Exception Unstacking BusFault': 'renode_unstack_fault_over_hardfault_consumes_frame_and_locks_up',
    'Should Tail Chain On Exception Unstacking BusFault': 'renode_unstack_fault_tail_chains_using_existing_frame, fault escalates to HardFault on Armv6-M',
    'Should Enter Lockup On RETPSR Integrity Failure': 'renode_invalid_nmi_return_into_hardfault_locks_up',
    'Should Enter Lockup On Reset Vector BusFault': 'renode_fault_on_initial_msp_or_reset_pc_locks_up, fault offset 4',
    'Should Enter Lockup On Initial MSP BusFault': 'renode_fault_on_initial_msp_or_reset_pc_locks_up, fault offset 0',
}

# A fixture can be shared when its required stimulus and observable contract
# can be represented on ARMv6-M or by a CPU-independent component.
V6 = "Armv6-M single core"
F072 = "STM32F072B Discovery / Cortex-M0"
COMMON = "CPU-independent fixture"
PASS = "equivalent pass"
PARTIAL = "partial"
OPEN = "unmet"
OUT = "out of scope"

F072_PORTS = {
    "Should Run Zephyr Hello Sample": (PASS, "hello_world_boots_elf_and_hex (ELF); USART1 Hello World"),
    "Should Run Zephyr Console Echo Sample": (PASS, "console_echo_accepts_resc_uart_input; USART1 LF banner, Echo working? + CR, unfinished-line reply and two 8 s wait bounds"),
    "Should Run Zephyr Blinky Sample": (PASS, "blinky_alternates_led_states; six ordered LED states with 1 s waits"),
    "Should Run Zephyr Button Sample": (PARTIAL, "button_updates_led_and_reports_press; 0.3 s guest-time press vs Robot wall-clock Sleep"),
    "Should Read ADC": (PASS, "adc_waits_for_uart_between_voltage_changes; channel 1 at 0.6 V and 1.2 V, readings 745 and 1489"),
    "Should Run stm32f0-crc-test Application": (PASS, "crc_firmware_passes_840_cases; USART1 result 840 passed, 0 failed"),
}

NUNIT_FIXTURES = {
    "CRCTests": "CRCEngine polynomial, initial value and input/output reversal",
    "ComparingTimerTests": "ComparingTimer against a BaseClockSource",
    "LimitTimerTests": "LimitTimer against a BaseClockSource",
    "GPIODetachingTests": "GPIO connect/disconnect and endpoint levels",
    "GPIOTests": "GPIO endpoint connection and signal propagation",
    "TimeHandleTests": "TimeHandle producer/consumer fixture and simulated thread synchronization",
    "TimeSourceTests": "TimeSource master/slave, quantum and sink scheduling",
    "SystemBusTests": "SystemBus registration, access width and memory operations",
}

for path in (infra / "src/Emulator/Main/Tests").rglob("*Tests.cs"):
    if path.stem in NUNIT_FIXTURES or path.stem == "MultiCPUTests":
        paths[path] = "Infrastructure"

BOARD_FIXTURES = {
    "STM32F103": ("STM32F103 / Cortex-M3", "stm32f103.repl and the STM32F103 Zephyr button ELF"),
    "STM32F4_Discovery": ("STM32F4 Discovery / Cortex-M4", "stm32f4.repl (or stm32f4_discovery.resc) and F4-specific ELF or peripheral wiring"),
    "STM32F4_Tock": ("STM32F4 Discovery / Cortex-M4", "stm32f4_tock.resc and the Tock image"),
    "STM32F7": ("STM32F746 Discovery / Cortex-M7", "stm32f746 RESC family and board images; display/modem cases use their attached devices"),
    "STM32L072": ("STM32L072 LRWAN1 / Cortex-M0+", "stm32l072.resc or stm32l072.repl and the L072 firmware/peripheral map"),
    "STM32W108": ("STM32W108 / Cortex-M3", "stm32w108.repl and the MB951 firmware"),
    "nucleo_wba52cg": ("NUCLEO-WBA52CG / Cortex-M33", "NUCLEO-WBA52CG platform and its STM32Cube firmware/pin configuration"),
    "STM32H7_RCC": ("STM32H743 / Cortex-M7", "stm32h743.repl and H7 RCC clock tree and registers"),
    "STM32LTDC": ("STM32H743 / Cortex-M7", "stm32h743.repl and LTDC register/interrupt fixture"),
}

# Keep shared observations credited for these foreign-board cases. Their
# original register maps or executable images are recorded in the evidence.
BOARD_PORTS = {
    ("STM32F4_Discovery", "PWM Capture"): (
        "renode_stm32_pwm_input_capture_routes_period_and_pulse_to_gpio; synthetic GPIO capture, F4 ELF/UART frequency and duty checks absent"),
    ("STM32F4_Discovery", "Should Fire Update Event When Counting Up"): (
        "renode_tim2_update_irq_repeats_in_both_directions; up-count MMIO/IRQ, F4 ELF/UART callbacks absent"),
    ("STM32F4_Discovery", "Should Fire Update Event When Counting Down"): (
        "renode_tim2_update_irq_repeats_in_both_directions; down-count MMIO/IRQ, F4 ELF/UART callbacks absent"),
    ("STM32L072", "PWM Should Support GPIO Output"): (
        "renode_stm32_pwm_duty_reaches_gpio_for_all_four_upstream_pulses; shared timer/GPIO checks, L072 ELF and shell/PWMTester absent"),
}

GDB_SETUPS = {
    "HiFive Unleashed": "SiFive FU540 RV64 SMP / Linux",
    "Versatile Express": "Versatile Express / Cortex-A9 A32",
    "Versatile": "Versatile / ARM926 A32",
    "Leon3": "LEON3 / SPARC",
    "MPC5567": "MPC5567 / PowerPC",
    "Microwatt": "Microwatt / little-endian PowerPC",
    "VM Demo": "RV64 virtual memory program and RISC-V page table",
    "RISC-V Vectored Machine": "RV32 with vector register state",
}
GDB_PORTABLE = {"Should Write And Read Memory", "Handle Long Command", "Commands Should Work Properly With High Addresses"}


def robot_cases(data):
    """Read Robot test tables, retaining case bodies and exact source lines."""
    in_cases = False
    current = None
    for number, line in enumerate(data.splitlines(), 1):
        if line.startswith("***"):
            if current:
                yield current
                current = None
            in_cases = bool(re.fullmatch(r"\*\*\*\s+(Test Cases|Tasks)\s+\*\*\*", line, re.I))
        elif in_cases and line and not line[0].isspace() and not line.startswith(("#", "...")):
            if current:
                yield current
            current = (number, line.strip(), [])
        elif current and line.strip() and not line.lstrip().startswith("#"):
            current[2].append(line)
    if current:
        yield current


def nunit_cases(data):
    pattern = re.compile(
        r"^\s*\[(Test(?:\]|,)[^\n]*)\n\s*public\s+\w+\s+(\w+)\(([^\n]*)\)",
        re.M,
    )
    for match in pattern.finditer(data):
        attrs, case, args = match.groups()
        line = data.count("\n", 0, match.start(2)) + 1
        yield line, case, [attrs, args]


def robot_parameters(name, case, body):
    rows = [line.strip() for line in body.splitlines()]
    assets = list(dict.fromkeys(re.findall(r"[^\s/@]+\.(?:elf|hex)(?:-[\w]+)*-s_\d+-[a-f0-9]{40}", body)))
    asset = "; ".join(assets)
    if name == "arm-cortex-m-exception-return":
        invocations = [re.split(r"\s{2,}", row)[1:] for row in rows
                       if row.startswith("Execution Of Instruction In ")]
        if invocations:
            return ["; ".join(args) for args in invocations]
        return {
            "Should Not Return From Exception When Loading PC From Vector Table": ["HardFault vector=0xffffffb0; MPU RBAR/RLAR; MSP=0x2000"],
            "Should Not Return From Exception When In Thread Mode": ["bx r0; r0=0xffffffb8; Thread mode"],
            "Should Not Return From Insecure Mode When Not Using TrustZone": ["bx r0; r0=0xfeffffff; TrustZone disabled"],
            "Should Not Restore Reserved Bits From FncReturn Partial Retpsr": ["TrustZone enabled; FNC_RETURN; partial RETPSR=0x200"],
        }[case]
    if name == "cpu-pause-halt":
        loops = [re.fullmatch(r"FOR\s+\$\{(\w+)\}\s+IN\s+True\s+False", row) for row in rows]
        names = [m[1] for m in loops if m]
        arch = "is_cortex_m=False" if "is_cortex_m=${False}" in body else "is_cortex_m=True"
        return ["; ".join([arch, *(f"{key}={value}" for key, value in zip(names, values))])
                for values in product(("True", "False"), repeat=len(names))]
    if name == "STM32SDMMC":
        return ["; ".join(re.split(r"\s{2,}", rows[0]))]
    if name == "STM32H7_RCC" and (m := re.search(r"Create Machine\s+(\$\{\w+\})", body)):
        return [m[1]]
    if name == "per-core-registration" and (m := re.search(r"Create Machine With Hex File\s+(sysbus\.cpu[12])", body)):
        return [m[1] + "; STM32F072B Hello HEX"]
    if name == "arm-cortex-m-interrupts":
        return {
            "Should Wake Up From WFE When SEVONPEND Is Set": ["SCR=0x10; GPIO IRQ=5; RAM=0x20000000"],
            "NVIC Base Priority Mask Setting Should Not Survive Reset": ["BASEPRI=0x50; machine reset"],
            "Should Count Down SysTick With Reload Set Before Reset": ["100 Hz; CVR=0 before RVR=10; CSR=5"],
            "Should Count Down SysTick With Reload Set After Reset": ["100 Hz; RVR=10 before CVR=0; CSR=5"],
        }[case]
    if (name, case) == ("STM32L072", "PWM Should Support GPIO Output"):
        return [(asset + "; channel=3; period=256; pulse=5,85,127,250")]
    if name == "STM32_UART":
        return ["16 MHz; BRR=0x10; 1 us quantum; " + (
            "CR1=0x2404 (parity)" if "Parity" in case else "CR1 initially 0x2004 (8N1)")]
    if name == "arm-cortex-m-bus-fault":
        settings = re.findall(r"(?:FaultOffset|FaultOnReads|FaultOnWrites|SetPendingIRQ)\s+([^\s#]+)", body)
        switches = [" ".join(re.split(r"\s{2,}", row)) for row in rows
                    if re.search(r"\b(?:FaultOffset|FaultOnReads|FaultOnWrites|SetPendingIRQ)\b", row)]
        return ["; ".join(switches) if settings else "RAM 0x0; fault peripheral 0x100000; vectors and assembly at source line"]
    if name == "uart-tester":
        return ["; ".join(" ".join(row.split()) for row in rows if
                          re.search(r"(?:Create Terminal Tester|Set Default Tester|testerId=|binaryMode=|Register Failing Uart String|Unregister Failing Uart String)", row, re.I))
                or "TrivialUart 0x1000; case-specific terminal operation"]
    if name == "tlib-thumb2":
        return ["cortex-m-thumb2-data-processing-instr-test.elf-s_17032-6adacbffaba09470a834f59b7e0ba21319b546c2; run_result=0xffff"]
    if name == "stm32f4-erase":
        return {
            "Should Ignore Erase When MER And SER Are Reset": ["FLASH_CR=0x00010000; no erase selection"],
            "Should Perform Mass Erase": ["FLASH_CR=0x00010004; MER"],
            "Should Perform Sector Erase": ["FLASH_CR=0x0001005a; SER and sector 11"],
        }[case]
    if name == "arm-cortex-m-signals":
        return [asset or ("CPU clock, NVIC clock and DWT clock" if "Clocked" in case else "CPUWAIT and reset sequence")]
    return [asset or "single sequence (case body)"]


def nunit_parameters(case, attrs, args):
    internal_parameters = {
        "ShouldThrowArgumentException": [f"polynomial=0xff; width={width}" for width in (7, 0, 33)],
        "ShouldHandleReflections": ["reflectIn=false; reflectOut=true", "reflectIn=true; reflectOut=false", "reflectIn=false; reflectOut=false"],
        "ShouldInitConfigProperly": ["CRC32", "CRC16_CCITT", "CRC16"],
        "ShouldTestEnableAllTranslations": [f"native access width={width}" for width in (8, 16, 32, 64)],
    }
    if case in internal_parameters:
        return internal_parameters[case]
    if not args:
        return ["Repeat(5), same configuration" if "Repeat(5)" in attrs else "fixture defaults"]
    if case == "ShouldReadBytes" and args.count("[Values(true, false)]") == 2:
        return [f"isFirstMemory={first}; isSecondMemory={second}"
                for first, second in product(("true", "false"), repeat=2)]
    if case == "ShouldGuardPeripheralReads" and args == "[Range(1, 4)] int cpuCount":
        return [f"cpuCount={count}" for count in range(1, 5)]
    raise ValueError(f"Unexpanded NUnit arguments for {case}: {args}")


def classify_nunit(name, case, attrs, parameter):
    if "Ignore(" in attrs:
        return OUT, COMMON, "Disabled by upstream NUnit Ignore attribute"
    if name == "MultiCPUTests":
        if parameter == "cpuCount=1":
            return OPEN, V6, "One ActivelyAskingCPU with ActivelyAskedPeripheral; reproduce concurrent bus-read guard with the single-core bus"
        count = 10 if not parameter.startswith("cpuCount=") else parameter.split("=")[1]
        return OUT, f"{count} concurrent mock CPUs", "Requires multiple CPU registration slots or multiple independent requesters of one peripheral"
    if name == "CRCTests":
        if case == "ShouldCalculateCRC31Philips":
            return OUT, "Generic CRCEngine with arbitrary polynomial width", "31-bit width cannot be selected on STM32F072 CRC"
        if case in {"ShouldCalculateCRC32MPEG2", "ShouldCalculateCRC8"}:
            return PASS, F072, "supports_crc32_mpeg2_and_crc8_smbus_check_values; same polynomial, initial value, 123456789 input and check value"
        if case == "ShouldThrowArgumentException":
            return OPEN, COMMON, "Host CRCConfig constructor distinguishes width out of range from an oversized polynomial; STM32 POLYSIZE encodes only 7/8/16/32 and MMIO has no constructor error API"
        checks = {
            "ShouldCalculateMultipleUpdates": "renode_crc_multiple_updates; empty, 0x31, then 0x32 with each intermediate value",
            "ShouldHandleReflections": "renode_crc_reflection_combinations; same 0x32/0x33 input and all three reflection configurations",
            "ShouldInitConfigProperly": "renode_crc_default_configurations; default CRCEngine parameters programmed through CR/POL/INIT, same input and check values",
        }
        if case in checks:
            return PASS, F072, checks[case]
        if case in {
            "ShouldCalculateCRC32", "ShouldCalculateCRC32BZIP", "ShouldCalculateCRC32JAMCRC",
            "ShouldCalculateCRC32POSIX", "ShouldCalculateCRC32SATA", "ShouldCalculateCRC16CCITT",
            "ShouldCalculateCRC16", "ShouldCalculateCRC7",
        }:
            return PASS, F072, "renode_crc_polynomial_check_values; same polynomial, init, reflections, 123456789 and final XOR (applied after DR read)"
    return OPEN, COMMON, NUNIT_FIXTURES[name] + "; exact NUnit observations still to port"


def classify_robot(name, case, body, parameter):
    if name == "STM32F072b":
        state, note = F072_PORTS[case]
        return state, F072, "upstream_stm32f072_" + note
    if name == "hex":
        return PASS, F072, "upstream_stm32f072_hello_world_boots_elf_and_hex (HEX); VTOR=0x08000000 and same USART1 output"
    if name == "STM32L072" and case == "Should Handle Flash Operations":
        return PARTIAL, F072, "renode_stm32_flash_erase_modes_preserve_the_expected_ranges covers shared erase range; original L072 flash shell ELF, geometry and UART command sequence are absent"
    if name == "STM32F4_Discovery" and case == "PWM Capture Four Channel Capture Support":
        return OPEN, F072, "F4 ELF uses four-channel capture; shared STM32 timer/GPIO frequency and duty checks can be driven on F072; retain distinct capture configuration"
    if (name, case) in BOARD_PORTS:
        return PARTIAL, F072, BOARD_PORTS[name, case] + "; upstream " + BOARD_FIXTURES[name][1]
    if name in BOARD_FIXTURES:
        config, reason = BOARD_FIXTURES[name]
        if "FAULTMASK" in case or "Faultmask" in case:
            reason += "; requires FAULTMASK, absent from Armv6-M"
        return OUT, config, reason
    if name == "STM32SDMMC":
        config = "STM32F746 Discovery / Cortex-M7" if "STM32F_REPL" in parameter else "STM32H743 REPL with H747 firmware / Cortex-M7"
        return OUT, config, "SDMMC and file-backed EXT2 card plus matching Zephyr storage ELF"
    if name == "PL330_DMA":
        config = "Cortex-A53 GICv3 / Zephyr" if "Create Zephyr Machine" in body else "Zedboard Zynq / Cortex-A9 Linux"
        return OUT, config, "PL330 channel program and controller are distinct from STM32F0 channel DMA; Linux branch uses dmatest or DMA-backed UART"
    if name == "ArrayMemory":
        return OPEN, V6, "Synthetic Cortex-M0+ REPL with ArrayMemory and 96b_neonkey ELF; retain save/clear/load and UART contract"
    if name == "STM32_UART":
        checks = {
            "Should Receive Characters At Configured Baud Rate": "receives_at_configured_baud_rate_and_detects_idle; two 9+1 us frames and one 9+1 us idle interval",
            "Should Account For Parity Within Word Length": "parity_is_part_of_the_programmed_word_length; 8-bit word including parity, same RX and idle timing",
            "Should Preserve Character Order When Baud Rate Becomes Zero": "preserves_receive_order_when_brr_becomes_zero; drain in-flight 0x12 before immediate 0x34",
            "Should Cancel Idle Detection When A New Frame Starts": "cancels_idle_when_the_next_frame_starts; start next frame after 9 us idle, no IDLE on old deadline",
            "Should Drop Queued Character When Receiver Is Disabled": "drops_an_in_flight_byte_when_re_is_cleared; no RXNE after 10 us with RE off or 10 us after re-enable",
            "Should Drop Characters When UART Or Receiver Disabled": "drops_input_if_either_enable_is_clear; neither RE-disabled 0x12 nor UE-disabled 0x34 arrives after re-enable",
        }
        return PASS, F072, "renode_stm32_usart_" + checks[case] + "; classic STM32_UART registers translated to F072 CR1/ISR/RDR"
    if name == "stm32f4-erase":
        return PARTIAL, F072, "renode_stm32_flash_erase_modes_preserve_the_expected_ranges; F4 fixture uses 0x40023cxx, SER/SNB and sector 11, F0 uses page/bank erase"
    if name == "arm-cortex-m-bus-fault":
        if case in FAULT_PORTS:
            return PARTIAL, V6, FAULT_PORTS[case] + "; M55 syndrome registers or EXC_RETURN differ"
        if "Create TrustZone Machine" in body:
            return OUT, "Cortex-M55 with Armv8-M Security Extension", "Secure/non-secure exception routing, banked registers, stack signature or secure vector observations"
        if re.search(r"(Lazy FP|FPCA|VLD/VST|VSTR/VLDR)", case):
            return OUT, "Cortex-M55 with FP or MVE", "Fixture performs lazy FP preservation or executes FP/MVE transfers"
        if "Ignore Precise BusFault" in case:
            return OUT, "Cortex-M55 with CCR.BFHFNMIGN", "Tests configurable suppression of a precise bus fault in HardFault; no such Armv6-M control"
        if case in {
            "Should Raise Precise BusFault On Peripheral Read",
            "Should Raise Precise BusFault On Peripheral Write",
            "Should Raise Precise BusFault On Unmapped Read When Configured",
            "Read Access Should Produce Precise Bus Fault Without Single Step",
            "Write Access Should Produce Precise Bus Fault Without Single Step",
        }:
            return OPEN, V6, "Portable faulting LDR/STR and precise saved frame contract; Armv6-M routing must use HardFault and the same access and run mode"
        if "Enable BusFault" in body:
            return OUT, "Cortex-M55 with configurable BusFault", "Asserts BusFault handler, priority or syndrome; HardFault adaptation is represented by the disabled-BusFault cases"
        if case == "Should Replace External IRQ With HardFault On Vector BusFault":
            return PARTIAL, V6, "renode_irq_vector_fault_preserves_pending_and_reuses_frame; same fault offset 0x40, IRQ16 pending, HardFault PC and SP; M55 syndrome registers absent"
        raise ValueError(f"Unreviewed fault case: {case}")
    if name == "arm-cortex-m-exception-return":
        if "trustZone=True" in parameter or "TrustZone enabled" in parameter or "Insecure Code" in body:
            return OUT, "Cortex-M33 with TrustZone", "Uses BXNS/BLXNS or SAU and FNC_RETURN security transition"
        if case == "Should Return From Exception When Using Allowed Instruction":
            return PASS, V6, "renode_nmi_bx_lr_restores_interrupted_thread; same reset/NMI PCs, NOP, BX LR and SetPendingIRQ 2"
        if case == "Should Not Return From Exception When In Thread Mode":
            return PASS, V6, "renode_thread_bx_and_nmi_handler_blx_do_not_unstack; load the same 0xffffffb8 via literal, BX in Thread mode, same PC"
        if case == "Should Not Return From Insecure Mode When Not Using TrustZone":
            return PASS, V6, "renode_nonsecure_return_token_branches_without_trustzone; literal 0xfeffffff, same BX and PC 0xfefffffe"
        ports = {
            "Should Not Return From Exception When Loading PC From Vector Table": "renode_exception_vector_load_does_not_unstack_an_exc_return_value; same NMI and HardFault vector 0xffffffb0, unmapped 0x2000 stack instead of an Armv8-M MPU denial",
            "Should Not Return From Exception When Not Using Allowed Instruction": "renode_thread_bx_and_nmi_handler_blx_do_not_unstack; same NMI and BLX LR, Armv6-M EXC_RETURN produces PC 0xfffffff8",
        }
        return PARTIAL, V6, ports[case]
    if name == "arm-cortex-m-interrupts":
        if "Base Priority" in case:
            return OUT, "STM32F4 / Cortex-M4", "Uses BASEPRI=0x50 and GetPriorityBoost; Armv6-M has PRIMASK"
        if "WFE" in case:
            return PASS, V6, "renode_wfe_wakes_on_new_pending_irq_only_with_sevonpend; same SCR, GPIO IRQ5, WFE/STR and before/after memory; CLI adds a RESC-level observation"
        return PARTIAL, V6, "renode_systick_counts_and_reloads_for_both_cvr_write_orders; explicit ticks replace 100 Hz RunFor timing checks"
    if name == "arm-cortex-m-signals":
        if "DWT" in case or "Clocked State" in case:
            return PARTIAL, F072, "renode_systick_external_clock_gate_survives_reset and upstream Blinky clock variant; original RA8M1/SAMD51 CPU and DWT checkpoints absent"
        if case == "CPU Should Not Read Vector Table Until After CPU Wait Signal Is Deasserted":
            return PARTIAL, V6, "renode_cpuwait_latches_only_at_reset_and_releases_at_sync_point plus upstream_stm32f072_cpuwait_defers_elf_boot_until_released; RA8M1 ELF and exact vector addresses replaced"
        if case == "Emulation Reset Should Halt CPU When CPU Wait Signal Set":
            return PASS, V6, "renode_initial_cpuwait_prevents_execution_after_machine_reset; initial signal asserted, reset, unmapped PC 0xdeadbeef stays halted"
        if case == "CPU Wait Signal Should Unhalt CPU When Deasserted After Reset":
            return PASS, V6, "renode_cpuwait_releases_with_an_explicit_pc; assert/reset/write PC 0x100/deassert/synchronize, halted transitions True to False"
        return PASS, V6, "renode_cpuwait_latches_only_at_reset_and_releases_at_sync_point; retains signal across reset and compares the same counter address and 0.1 s intervals"
    if name == "arm-cortex-m-fpu":
        model = ("K6xF Cortex-M4F" if "nxp-k6xf" in body else
                 "Armv8-M with TrustZone and FPU" if "V8M" in body else "Cortex-M with FP extension")
        return OUT, model, "FPU sharing, FP context stacking or NOCP/FP register stimulus"
    if name == "arm-cortex-m-fpv5":
        return OUT, "Cortex-M with FPv5" + (" and TrustZone" if "trustZoneEnabled=" in body else ""), "Suite setup starts FPU; tests manipulate FP registers, rounding or FP context"
    if name == "arm-cortex-m-mve":
        return OUT, "Cortex-M with Armv8.1-M MVE", "Vector registers, predication or MVE instructions; all nested test-data values require MVE"
    if name == "arm-cortex-m-trustzone":
        return OUT, "Cortex-M with Armv8-M Security Extension", "Secure/non-secure state, SAU, banked stacks or security instructions"
    if name in {"arm-thumb", "arm-thumb2-srs-rfe"}:
        return OUT, "Cortex-A9 / ARMv7-A", "A32/T32 switching or banked CPSR modes with SRS/RFE; no A32 state on Armv6-M"
    if name == "it-status":
        if case == "Should Evaluate Condition Codes Properly":
            return OPEN, V6, "Cortex-M4 fixture's CMP/ADDS flags are portable; EvaluateConditionCode observations need a port"
        return OUT, "Cortex-M4 / Armv7E-M", "Assembles ITTET/ITEET and reads ITSTATE; IT is unavailable on Cortex-M0/M0+"
    if name == "tlib-thumb2":
        return PARTIAL, V6, "renode_thumb2_fixture_branches_and_reads_apsr; extracted BL/MRS, original Cortex-M85 ELF also executes ORRS.W and tests run_result"
    if name == "cpu-pause-halt":
        if "is_cortex_m=${False}" in body:
            return OUT, "Allwinner A20 / Cortex-A7", "A32 fixture and retained halt state across Cortex-A reset"
        return OPEN, V6, "RA8M1 fixture uses portable Thumb reset loop; pause/halt/time flow and reset ordering remain to port"
    if name == "gdb":
        if case == "Should Not Pause Machine Upon Python Peripheral Access":
            return OPEN, V6, "Cortex-M0 REPL, GDB breakpoint and rolling-bit Python peripheral; replace original MOV.W/MOVW with Armv6-M literals"
        setup = re.search(r"\[Setup\]\s+Create (.+)", body)
        if setup:
            config = GDB_SETUPS[setup[1].strip()]
            if case in GDB_PORTABLE:
                return OPEN, V6, f"Source uses {config}; GDB protocol and direct memory stimulus are portable without executing that ISA"
            return OUT, config, "Case runs against this fixture's instruction stream, register file, peripheral addresses, thread identities or address translation"
        if "zynqmp.repl" in body or "cortex-r52_smp_4.repl" in body:
            config = "ZynqMP Cortex-A53/Cortex-R5F clusters" if "zynqmp.repl" in body else "Cortex-R52 four-core SMP"
            return OUT, config, "Asserts GDB CPU/cluster selection and thread list for the named cores"
        raise ValueError(f"Unreviewed GDB setup for {case}")
    if name == "hooks":
        if "CortexM Hook" in case:
            return PARTIAL, V6, "renode_systick_current_write_reloads_without_pend_and_zero_reload_stops; M0 zero reload covered, Python PC hooks and pause log missing"
        if "stm32l072.resc" in body:
            return OUT, "STM32L072 LRWAN1 / Cortex-M0+", "Asserts specific L072 firmware symbol names and translation block addresses"
        if "cortex-a9" in body:
            return OUT, "Cortex-A9 / ARMv7-A", "Executes hard-coded A32 LDRB/B sequence with a Python watchpoint"
        return OPEN, V6, "Mi-V fixture runs monitor.Parse at a CPU hook; hook/monitor/log contract is ISA independent"
    if name == "per-core-registration":
        if "Create Machine With Hex File" in body:
            return OUT, "per-core-registration-hex.repl: two Cortex-M4s", "Same F072 HEX asset, but loader must resolve CPU1 0x07000000 vs CPU2 0x08000000 context"
        if "array_memory_global_above_32bit.resc" in body:
            return OPEN, V6, "Cortex-R8 source fixture; accepts global high-address memory and checks no inappropriate flush of 32-bit translations"
        return OUT, "per-core-registration.repl: RV64 / two CPUs and two clusters", "Assertions depend on per-core or cluster bus aliases, context-specific loads and registration/move rules"
    if name == "profiler-trace":
        if "PLAT_ARM" in body:
            return PARTIAL, V6, "renode_profiler_fixture_pendsv_dispatches_from_icsr; Cortex-M4 MPU nesting and profiler limit log missing"
        return OPEN, V6, "RV32I fixture uses ordinary calls/loops; trace format and file-size controls are ISA independent"
    if name == "real-time-clock-mode":
        model = case.split()[0]
        return OPEN, F072, "Source uses Timers." + model + "; portable machine/RTC date-time agreement through RTC mode changes; F072 RTC integration still missing"
    if name == "timer-reconfiguration":
        return OUT, "RV32IC with CLINT", "Reconfigures RISC-V CLINT frequency and guest time-provider path; no corresponding STM32 timer register"
    if name == "host-uart":
        return OPEN, V6, "Mi-V RESC supplies UART; host serial adapter and bidirectional host I/O contract can use an F072 UART"
    if name == "uart-tester":
        if "Create Trivial Machine" in body:
            fixture = "TrivialUart with injected 0x00..0xff"
        elif "Create Tock Machine" in body:
            fixture = "STM32F4 Tock USART2/3 log fixture"
        else:
            fixture = "SiFive FE310 Zephyr RESC UART0/1 fixture"
        return OPEN, COMMON, fixture + "; terminal API contract is independent of UART model; tester IDs, binary mode, regex and failure handling need exact observations"
    raise ValueError(f"Unreviewed case: {name}/{case}")


def inventory():
    for path, origin in sorted(paths.items(), key=lambda item: (item[1], str(item[0]))):
        data = path.read_text()
        name = "tlib-thumb2" if path.stem == "thumb2-data-processing-instr" else path.stem
        if origin == "Infrastructure":
            for line, case, (attrs, args) in nunit_cases(data):
                for param in nunit_parameters(case, attrs, args):
                    state, config, note = classify_nunit(name, case, attrs, param)
                    yield origin, path.relative_to(infra), line, case, param, state, config, note
        else:
            for line, case, lines in robot_cases(data):
                body = "\n".join(lines)
                for param in robot_parameters(name, case, body):
                    state, config, note = classify_robot(name, case, body, param)
                    yield origin, path.relative_to(renode), line, case, param, state, config, note


writer = csv.writer(sys.stdout, delimiter="\t", lineterminator="\n")
writer.writerow(["source", "file", "line", "case", "parameter", "status", "configuration", "evidence_or_pending"])
seen = {}
for row in inventory():
    source, path, _, case, parameter, state, config, note = row
    identity = source, path, case, parameter, config
    if identity in seen:
        if seen[identity] != (state, note):
            raise ValueError(f"Conflicting audit entries: {identity}")
        continue
    seen[identity] = state, note
    writer.writerow(row)
