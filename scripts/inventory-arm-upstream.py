#!/usr/bin/env python3
from pathlib import Path
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
if not (infra / "src/Emulator/Main/Tests/UnitTests/LimitTimerTests.cs").is_file():
    sys.exit("expected the pinned Renode Infrastructure source tree")
paths={}
for path in (renode/'tests').rglob('*.robot'):
    name=path.stem.lower()
    data=path.read_text(errors='replace')
    if (name.startswith(('arm-cortex-m','arm-thumb','stm32','uart-tester','host-uart','it-status','cpu-pause','per-core-registration','hex','profiler-trace','timer-reconfiguration','real-time-clock-mode'))
        or re.search(r'cortex-m0|stm32f0|stm32_uart|stm32f7_usart|GPIOPort\.STM32|Timers\.STM32_Timer', data, re.I)
        or name in ('thumb2-data-processing-instr','nucleo_wba52cg')):
        paths[path]='Renode'
for path in (infra/'src/Emulator/Main/Tests').rglob('*Tests.cs'):
    if path.stem in ('ComparingTimerTests','LimitTimerTests','GPIOTests','GPIODetachingTests','CRCTests','DmaEngineTests','SystemBusTests','MultiCPUTests','TimeSourceTests','TimeHandleTests'):
        paths[path]='Infrastructure'
def status(name,case):
    if name=='stm32f072b':
        checks=['hello_world_boots_elf_and_hex','console_echo_accepts_resc_uart_input','blinky_alternates_led_states','button_updates_led_and_reports_press','adc_waits_for_uart_between_voltage_changes','crc_firmware_passes_840_cases']
        return 'existing translation', 'upstream_stm32f072_'+checks[discovery.index(case)]+'; prior port, audit input and failure equivalence'
    if name=='hex': return 'existing translation','upstream_stm32f072_hello_world_boots_elf_and_hex; HEX branch'
    if name=='arm-cortex-m-exception-return' and case=='Should Return From Exception When Using Allowed Instruction':
        return 'mixed','BX LR with NMI covered by renode_nmi_bx_lr_restores_interrupted_thread; BXNS row requires Armv8-M Security Extension'
    if name=='arm-cortex-m-interrupts':
        if 'Base Priority' in case: return 'not applicable','Armv6-M has PRIMASK and lacks BASEPRI'
        if 'Count Down' in case: return 'existing translation','renode_systick_counts_and_reloads_for_both_cvr_write_orders'
        return 'existing translation','renode_wfe_wakes_on_new_pending_irq_only_with_sevonpend'
    if name in ('arm-cortex-m-fpu','arm-cortex-m-fpv5','arm-cortex-m-trustzone','arm-cortex-m-mve','arm-thumb','arm-thumb2-srs-rfe'):
        return 'not applicable','requires FPU, Security Extension, MVE, or the A/R-profile ARM/CPSR facilities named by this suite'
    if name=='stm32_uart':
        return ('partial','classic STM32_UART bench uses SR/DR/CR1; F072 equivalent still needs timed RX, IDLE and parity boundary audit') if 'Disabled' in case and 'UART Or Receiver' in case else ('open applicability','classic STM32_UART bench; compare shared RX and timing behavior with F072 STM32F7_USART layout')
    if name=='arm-cortex-m-signals': return 'partial' if 'Clocked' in case else 'open','SysTick gating port exists; CPUWAIT, DWT model availability and full observations need per-case audit'
    if name == 'arm-cortex-m-bus-fault':
        ports = {
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
        if case in ports:
            return 'partial', ports[case] + '; Armv6-M has no CFSR/HFSR/BFAR or Armv8-M exception-return token'
        return 'open applicability', 'Cortex-M55 fixture; classify Armv6-M behavior separately from architectural BusFault and security/floating point state'
    if name=='per-core-registration': return 'open applicability','uses per-CPU registration; check exact topology/CPU requirement, including STM32F072 HEX'
    if name=='uart-tester': return 'open','Robot terminal API and exact matching/error semantics require audit; named RESC UART waits cover a subset'
    if name=='stm32f4-erase': return 'partial','F072 page/bank behavior port exists; STM32F4 sector geometry and registers differ'
    if name in ('limittimertests','comparingtimertests','gpiotests','gpiodetachingtests','crctests'):
        return 'open applicability','shared infrastructure NUnit fixture; compare virtual-time, endpoint or CRC assumptions with device behavior'
    return 'open applicability','inspect case setup and exact expectations against F072 or Armv6-M; no acceptance credit'
discovery=['Should Run Zephyr Hello Sample','Should Run Zephyr Console Echo Sample','Should Run Zephyr Blinky Sample','Should Run Zephyr Button Sample','Should Read ADC','Should Run stm32f0-crc-test Application']
def robot_rows(data):
    started=False
    cases=[]
    for i,line in enumerate(data.splitlines(),1):
        if line.startswith('***'):
            started=bool(re.match(r'\*\*\*\s+(Test Cases|Tasks)\s+\*\*\*',line,re.I))
        elif started and line and not line[0].isspace() and not line.startswith(('#', '...')):
            cases.append([i,line.strip(),[]])
        elif started and cases and line.strip() and not line.strip().startswith('#'):
            if re.match(r'\s*(\[Template\]|Execution Of Instruction In .* Should Result In PC\b|\[Arguments\])',line):
                cases[-1][2].append(' '.join(line.split()))
    return cases
def nunit_rows(data):
    rows=[]
    attrs=[]
    for i,line in enumerate(data.splitlines(),1):
        t=line.strip()
        if t.startswith(('[Test]','[TestCase','[Values')):
            attrs.append(t)
        elif m:=re.match(r'public\s+\w+\s+(\w+)\((.*)\)',t):
            if attrs:
                rows.append((i,m[1],attrs))
                attrs=[]
    return rows
with sys.stdout as output:
    writer=csv.writer(output,delimiter='\t',lineterminator='\n')
    writer.writerow(['source','file','line','case','parameter','status','evidence_or_pending'])
    for path,origin in sorted(paths.items(),key=lambda item:(item[1],str(item[0]))):
        data=path.read_text(errors='replace')
        for line,case,params in (robot_rows(data) if origin=='Renode' else nunit_rows(data)):
            state,notes=status(path.stem.lower(),case)
            for param in params or ['see source case body']:
                row_state,row_notes=state,notes
                if path.stem.lower() == "stm32f072b":
                    case_source = data.splitlines()[line:]
                    for source_line in case_source:
                        if source_line and not source_line[0].isspace():
                            break
                        if asset := re.search(r"\b[^\s/]+\.elf-s_\d+-[a-f0-9]{40}\b", source_line):
                            param = asset[0]
                            break
                if path.stem == 'arm-cortex-m-exception-return' and re.search(r'\b(?:bxns|blxns)\b', param):
                    row_state,row_notes='not applicable','BXNS/BLXNS requires Armv8-M Security Extension'
                elif state=='mixed' and 'bx lr' in param:
                    row_state,row_notes='existing translation','renode_nmi_bx_lr_restores_interrupted_thread; same SetPendingIRQ 2 operation, addresses and PC transitions'
                writer.writerow([origin,path.relative_to(renode if origin=='Renode' else infra),line,case,param,row_state,row_notes])
