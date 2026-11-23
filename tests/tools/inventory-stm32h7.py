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
            status, note = 'OUT', 'NPCX9 の EC ファームウェアとボード配線が対象'
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
        rows.append(('renode',file,f'{start+1}-{stop}',name,' | '.join(settings+props) or 'ケース本文・キーワード参照',status,note))
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
    if suite == 'MCAN.robot' and name in {
        'Should Boot Linux And Login With MCAN',
        'Should Handle CAN Messages Issued Through CAN Utils Tools',
    }:
        status, reason = 'OUT', '固定 Zynq Linux ゲストで実行し、CAN Utils は Zynq ログイン状態を要求'
    elif suite == 'LPS25HB.robot' and name != 'Should Read Samples From RESD':
        status, reason = 'PARTIAL', {
            'Should Read Temperature And Pressure': '同一 v4.2 ELF の初期値と正負の入力を別実行で照合。稼働中の入力変更と順序は未移植',
            'Should Correctly Calculate Pressure': 'I²C サブアドレスで上流の 3 バイトを確認。Robot のプロパティ getter と直接 Read は未移植',
            'Should Work After Errata Driver Update': '同一 v4.4 ELF の UART 期待値を確認。元のプラットフォーム構成と操作順の等価性は未確定',
        }[name]
    elif status == 'REVIEW':
        if suite == 'arm-cortex-m-fpu.robot' and name != 'Should Pass Context Stacking Preservation Test v7-M':
            status, reason = 'OUT', 'FPU 共有は NXP K6xF 固定。残りのスタック・NOCP ケースは v8-M/M33 固定'
        elif suite == 'monitor-tests.robot' and name == 'Should Fail to Find Unix-only Method on Windows':
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
    out.writerows(map(triage, rows))
