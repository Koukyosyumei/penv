import argparse
import csv
import itertools
import pathlib
import re

parser = argparse.ArgumentParser(description='Generate the initial pinned STM32H7 upstream inventory')
parser.add_argument('renode', type=pathlib.Path)
parser.add_argument('infrastructure', type=pathlib.Path, help='extracted infrastructure tree with packetnet/ audit checkout')
parser.add_argument('output', type=pathlib.Path)
arguments = parser.parse_args()
renode = arguments.renode
infra = arguments.infrastructure
peripherals = 'tests/peripherals/'
robot = {
    'tests/platforms/nucleo_h753zi.robot': 'board',
    **{peripherals + name + '.robot': 'STM32H7' for name in ('STM32H7_RCC','STM32LTDC','IIS2MDC','VEML7700','LPS25HB','LSM6DSO_IMU','INA228')},
    **{peripherals + name + '.robot': 'mixed' for name in ('STM32SDMMC','MCAN','UT32_CAN')},
    peripherals + 'SDCard.robot': 'STM32HSDMMC',
    peripherals + 'SynopsysDWCEthernetQualityOfService.robot': 'Zynq',
    'tests/platforms/NPCX9.robot': 'NPCX',
    'tests/unit-tests/monitor-tests.robot': 'review',
    **{'tests/unit-tests/' + name + '.robot': 'review' for name in ('arm-cortex-m-fpu','arm-cortex-m-interrupts','arm-dsp-examples-cortex-m4','arm-ge-flag','InterruptHooks/arm-m','emulation-environment','platform_descriptions','sysbus','uart-tester','resd-arbitrary-timestamp','resd-serialization')},
}
nunit = {
    **{'src/Emulator/Peripherals/Test/PeripheralsTests/' + name + '.cs': kind for name,kind in (
        ('STM32H7_RCC','STM32H7'), ('SDCardEmmcTest','eMMC'), ('STMCANTest','bxCAN')
    )},
    **{'src/Emulator/Main/Tests/UnitTests/' + name + '.cs': 'review' for name in (
        'DmaEngineTests','GPIODetachingTests','GPIOTests','StorageTests','PacketTests',
        'LimitTimerTests','ComparingTimerTests','TimeSourceTests','SystemBusTests','MultiCPUTests'
    )},
}
def fixed_robot_fixture(lines, body):
    """Collect fixed fixture inputs referenced by a case and its local keywords."""
    variables = {}
    keywords = {}
    section = ''
    name = None
    for line in lines:
        if line.startswith('***'):
            section = line.strip('* ').lower()
            name = None
        elif section == 'variables':
            match = re.match(r'([$@&]\{[^}]+\})=?\s{2,}(.*)', line)
            if match:
                name, value = match.groups()
                variables[name] = [value]
            elif name and line.startswith('...'):
                variables[name].append(line[3:].strip())
        elif section == 'keywords':
            if line and not line[0].isspace() and not line.startswith('#'):
                name = line.strip()
                keywords[name] = []
            elif name and line.strip() and not line.lstrip().startswith('#'):
                keywords[name].append(line.strip())

    records = list(body)
    seen = set()
    for line in records:
        cells = re.split(r'\s{2,}|\t+', line)
        for cell in cells[:2]:
            if cell in keywords and cell not in seen:
                records.extend(keywords[cell])
                seen.add(cell)
    values = []
    seen.clear()
    for line in records:
        for key in re.findall(r'[$@&]\{[^}]+\}', line):
            if key in variables and key not in seen:
                records.extend(variables[key])
                # Retain fixed file paths and scalar bindings, not entire inline programs.
                value = variables[key][0]
                if not value.startswith(('SEPARATOR=', 'Catenate', 'REGEXP:')):
                    values.append(f'{key}={value}')
                seen.add(key)
        if re.match(r'Execute Command\s+(machine LoadPlatformDescription @|sysbus LoadELF |include @)', line):
            values.append(line)
    return ' | '.join(dict.fromkeys(values))


def platform_variants():
    """Expand the Robot template for the Linux x64 host used by penv/network."""
    suite = renode / 'tests/unit-tests/platform_descriptions.robot'
    text = suite.read_text()
    # The conditional blacklist keeps all three KVM fixtures on Linux x64.
    condition = "'{system}' ${eq} 'Linux' and '{arch}' ${eq} 'x64'"
    if text.count(condition) != 3:
        raise ValueError('review changed platform fixture host conditions')
    block = text.split('@{blacklist}=', 1)[1].split('*** Keywords ***', 1)[0]
    excluded = {
        path.replace('${/}', '/')
        for path in re.findall(r'\$\{platforms_path\}\$\{/\}(\S+\.repl)', block)
    }
    root = renode / 'platforms'
    files = sorted(path for path in root.rglob('*.repl') if path.relative_to(root).as_posix() not in excluded)

    def inherits_h7(path, visited):
        path = path.resolve()
        if path == (root / 'cpus/stm32h7.repl').resolve():
            return True
        if path in visited or not path.is_file():
            return False
        visited.add(path)
        for include in re.findall(r'^\s*using\s+"([^"]+)"', path.read_text(), re.M):
            target = path.parent / include if include.startswith('.') else renode / include
            if inherits_h7(target, visited):
                return True
        return False

    for path in files:
        configuration = path.relative_to(renode).as_posix()
        if inherits_h7(path, set()):
            yield configuration, 'OPEN', '固定 REPL が cpus/stm32h7.repl の Cortex-M7 構成を継承'
        else:
            yield configuration, 'OUT', f'テンプレートが {configuration} 自体のロードを要求。STM32H7 REPL 継承なし'


def deduplicate(entries):
    """Keep the upstream case identity distinct from its execution configuration."""
    seen = {}
    for row in entries:
        repo, source, span, case, bindings, status, reason = row
        key = repo, source, case, bindings
        decision = status, reason
        if key in seen:
            if seen[key] != decision:
                raise ValueError(f'conflicting entries for {key!r}')
            continue
        seen[key] = decision
        yield row


rows = []
for file, kind in robot.items():
    lines = (renode/file).read_text().splitlines()
    section = next(i for i,l in enumerate(lines) if l.lower().startswith('*** test cases'))
    end = next((i for i in range(section+1,len(lines)) if lines[i].startswith('***')),len(lines))
    cases = [i for i in range(section+1,end) if lines[i] and not lines[i][0].isspace() and not lines[i].startswith('#')]
    settings = [l.strip() for l in lines[:section] if l.startswith('Test Template')]
    for start, stop in zip(cases,cases[1:]+[end]):
        name = re.split(r'\s{2,}|\t+',lines[start])[0].strip()
        body = [l.strip() for l in lines[start+1:stop] if l.strip() and not l.lstrip().startswith('#')]
        props = [l for l in body if re.search(r'\$\{|\b(?:FOR|IN RANGE)\b|^\[',l)]
        status, note = 'OPEN', '上流と等価な実行記録なし'
        if kind == 'review':
            status, note = 'REVIEW', 'ケースの機種・実行環境への適用性を要確認'
        elif kind == 'Zynq':
            status, note = 'OUT', 'Zynq-7000、GIC と Linux の固定ゲスト・配線を要求。STM32H7 の EQoS は nucleo_h753zi.robot で対象化'
        elif kind == 'NPCX':
            status, note = 'OUT', {
                'Should Run TMP108': 'NPCX9M6F EVB の Zephyr ELF と SMBus0 上の TMP108 を固定',
            }.get(name, 'NPCX9 REPL の EC ファームウェアとボード配線を固定')
        elif file.endswith('STM32SDMMC.robot') and 'STM32F' in name:
            status, note = 'OUT', 'STM32F746G ELF と F7 コントローラーを固定'
        elif file.endswith('STM32SDMMC.robot'):
            status, note = 'PARTIAL', '診断 REPL / 読取専用のスモークで UART 一致。元の SdCardFromFile と容量設定が未移植'
        elif file.endswith('STM32LTDC.robot'):
            status, note = 'PARTIAL', 'ボタン対照・ゼロ行・仮想フレームは実 ELF で確認。元の構成と Robot 操作の等価性の確定待ち'
        elif file.endswith('IIS2MDC.robot'):
            status, note = 'PARTIAL', '同一 ELF・全サンプル。RESD の代わりに CSV を供給'
        elif file.endswith('VEML7700.robot') and name == 'Should Work From Zephyr Shell':
            status, note = 'PARTIAL', '参照デモ Shell で両照度を確認。上流の同一 ELF と実行時変更が未確認'
        elif name == 'Should Talk Over Ethernet':
            status, note = 'PARTIAL', '同一 ELF。成功カウンターの照合まで確認。上流 UART 期待値と供給条件が未一致'
        if file.endswith('/platform_descriptions.robot') and name == 'Should Load Repls':
            for configuration, decision, reason in platform_variants():
                rows.append(('renode', file, f'{start+1}-{stop}', name, configuration, decision, reason))
            continue
        fixture = fixed_robot_fixture(lines, body)
        bindings = ' | '.join(settings + props + ([fixture] if fixture else []))
        rows.append(('renode', file, f'{start+1}-{stop}', name, bindings or 'ケース本文・キーワード参照', status, note))
for file in sorted((infra/'packetnet/Test').rglob('*.cs')):
    if re.search(r'\[Test(?:\]|,|Case\()',file.read_text(encoding='utf-8-sig')):
        name=file.relative_to(infra).as_posix()
        nunit[name]='wifi' if '/Ieee80211/' in name else 'review'
for file in sorted((renode/'tests/unit-tests/RenodeTests/PlatformDescription').glob('*Tests.cs')):
    nunit[file.relative_to(renode).as_posix()]='review'
for file,kind in nunit.items():
    repository, root, source = 'infrastructure', infra, file
    if file.startswith('packetnet/'):
        repository, root, source = 'packetnet', infra/'packetnet', file.removeprefix('packetnet/')
    elif file.startswith('tests/'):
        repository, root = 'renode', renode
    lines = (root/source).read_text(encoding='utf-8-sig').splitlines()
    for i,line in enumerate(lines):
        if not re.search(r'\[Test(?:\]|,|Case\(|CaseSource\()',line):
            continue
        j=i+1
        while j<len(lines) and not re.search(r'\bpublic\b',lines[j]):
            j+=1
        if j==len(lines): continue
        name=re.search(r'(\w+)\s*\(',lines[j])
        if not name: continue
        k=j
        while k+1<len(lines) and not lines[k].rstrip().endswith(')'):
            k+=1
        args=line.strip()+' '+ ' '.join(l.strip() for l in lines[j:k+1])
        if kind == 'eMMC':
            status,note='OUT','eMMC ハードウェアパーティションまたは eMMC 固有コマンド。対象の SDHC ソケットと異なる'
        elif kind == 'bxCAN':
            status,note='OUT','STM32 bxCAN ペリフェラル単体。H7 ボードは MCAN/FDCAN'
        elif kind == 'wifi':
            status,note='OUT','IEEE 802.11 無線フレーム固有。対象 MAC と仮想スイッチは IEEE 802.3 有線 Ethernet'
        elif kind == 'review':
            status,note='REVIEW','共有 NUnit 契約の適用性とパラメータの確認待ち'
        else:
            status,note='OPEN','上流 NUnit と等価な実行記録なし'
        ranges = re.findall(r'\[Range\((\d+),\s*(\d+)\)\]\s+int\s+(\w+)', args)
        if ranges:
            for combination in itertools.product(*(range(int(first), int(last) + 1) for first, last, _ in ranges)):
                binding = ', '.join(f'{key}={value}' for (_, _, key), value in zip(ranges, combination))
                rows.append((repository, source, f'{i+1}-{k+1}', name.group(1), binding, status, note))
            continue
        param = re.findall(r'\[Values\(([^()]*)\)\]\s+\w+\s+(\w+)', args)
        if param:
            for combination in itertools.product(*(spec.split(',') for spec, _ in param)):
                binding = ', '.join(f'{key}={value.strip()}' for value, (_, key) in zip(combination, param))
                rows.append((repository,source,f'{i+1}-{k+1}',name.group(1),binding,status,note))
        else:
            rows.append((repository,source,f'{i+1}-{k+1}',name.group(1),args,status,note))
def triage(row):
    repo, source, span, name, bindings, status, reason = row
    suite = pathlib.PurePosixPath(source).name
    if suite == 'arm-cortex-m-fpu.robot':
        if name == 'Should Pass Context Stacking Preservation Test v7-M':
            status, reason = 'OPEN', '固定 Cortex-M4 の v7-M/FP コンテキスト。STM32H747 の M4 側に移植可能'
        elif name == 'Should Pass Zephyr FPU Sharing Generic Tests':
            status, reason = 'OUT', 'NXP K6xF 固定 REPL、UART0 と K6xF 用 Zephyr ELF を要求'
        elif name == 'Should Pass Context Stacking Preservation Test v8-M with TrustZone':
            status, reason = 'OUT', 'Cortex-M33/v8-M TrustZone と専用のスタック検証 ELF を要求'
        else:
            status, reason = 'OUT', 'Prepare Trivial Platform が Cortex-M33 と v8-M 用 NOCP アセンブリをロード'
    elif source == 'tests/unit-tests/InterruptHooks/arm-m.robot':
        status, reason = 'OUT', 'STM32F746G Discovery 専用 ELF と固定 GPIOI11 のボタン刺激を要求'
    elif suite == 'sysbus.robot' and name == 'Symbols Should Be Dynamically Loaded and Unloaded On Request':
        status, reason = 'OUT', '固定 STM32L072/STM32L07 ELF と main=0x08007644 が観測点'
    elif suite == 'arm-cortex-m-interrupts.robot':
        status, reason = 'OPEN', 'F4 REPL はフィクスチャ。観測対象は H7 と共通の WFE/BASEPRI/SysTick 契約で、専用 ELF なし'
    elif suite == 'arm-dsp-examples-cortex-m4.robot':
        status, reason = 'OPEN', '汎用 Cortex-M4F と TrivialUart の CMSIS-DSP ELF。STM32H747 の副コアに適用'
    elif suite == 'arm-ge-flag.robot':
        status, reason = 'OPEN', '固定 Cortex-M7 の GE フラグ。H7 主コアの命令セットに適用'
    elif suite == 'MultiCPUTests.cs':
        status, reason = 'OPEN', '汎用 ICPU モックのシステムバス契約。台数は実機コア数の要求ではないため全構成を維持'
    elif suite == 'MCAN.robot' and name in {
        'Should Boot Linux And Login With MCAN',
        'Should Handle CAN Messages Issued Through CAN Utils Tools',
    }:
        status, reason = 'OUT', '固定 Zynq Linux ゲストで実行し、CAN Utils は Zynq ログイン状態を要求'
    elif suite == 'LSM6DSO_IMU.robot':
        status = 'PARTIAL'
        if 'RESD' in name:
            reason = '同一 ELF・元の CSV 12 Hz・100 ms オフセット・3 区間と終端復帰を同順 UART で照合。RESD コンテナー、元の REPL、Robot timeout は未移植'
        else:
            reason = '同一 ELF と同順の実行時プロパティ刺激を UART で照合。元の REPL と Robot timeout の等価性は未確認'
    elif suite == 'LPS25HB.robot' and name != 'Should Read Samples From RESD':
        status, reason = 'PARTIAL', {
            'Should Read Temperature And Pressure': '同一 v4.2 ELF の初期値と正負の入力を別実行で照合。稼働中の入力変更と順序は未移植',
            'Should Correctly Calculate Pressure': 'I²C サブアドレスで上流の 3 バイトを確認。Robot のプロパティ getter と直接 Read は未移植',
            'Should Work After Errata Driver Update': '同一 v4.4 ELF の UART 期待値を確認。元のプラットフォーム構成と操作順の等価性は未確定',
        }[name]
    elif status == 'REVIEW':
        if suite == 'monitor-tests.robot' and name == 'Should Fail to Find Unix-only Method on Windows':
            status, reason = 'OUT', 'Windows 固有のメソッド探索。対象の検証ホストは Unix'
        elif repo == 'packetnet' and suite in {'PPPPacketTest.cs', 'PPPoEPPPTest.cs', 'LinuxCookedCaptureTest.cs'}:
            status, reason = 'OUT', 'PPP セッションまたは Linux cooked キャプチャ形式固有。対象スイッチは生の Ethernet フレームを入出力'
        else:
            status = 'OPEN'
            if repo == 'packetnet':
                reason = '共有パケット処理契約として保守的に対象。等価性未確認'
                if '/Performance/' in source:
                    reason = '共有パケット処理の計測契約。明示された性能タスクで実行予定'
            elif repo == 'infrastructure':
                reason = '共有 DMA・バス・時刻・GPIO 等の契約として対象。等価性未確認'
            elif '/PlatformDescription/' in source:
                reason = '上流 H7 REPL の読み込み・検証に関わる共有契約。等価性未確認'
            else:
                reason = 'Cortex-M・バス・UART・モニター・センサーの共有契約として保守的に対象。等価性未確認'
    return repo, source, span, name, bindings, status, reason


with arguments.output.open('w', encoding='utf-8', newline='') as f:
    out=csv.writer(f,delimiter='\t',lineterminator='\n')
    out.writerow(('repository','source','lines','case','parameter_bindings','status','evidence_or_reason'))
    out.writerows(deduplicate(map(triage, rows)))
