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
        ('STM32H7_RCC','STM32H7'), ('SDCardEmmcTest','eMMC'), ('STMCANTest','bxCAN'),
        ('FT5336Test','FT5336')
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
            if configuration in {
                'platforms/boards/stm32h747-disco.repl',
                'platforms/boards/nucleo_h753zi.repl',
                'platforms/boards/stm32h7_renode_reference_board.repl',
                'platforms/cpus/stm32h7.repl',
                'platforms/cpus/stm32h743.repl',
                'platforms/cpus/stm32h747.repl',
                'platforms/cpus/stm32h753.repl',
            }:
                yield configuration, 'PASS', '固定 REPL を新規マシンで読み込み、破棄。上流 Try Load Platform と同じ入力・ロード失敗条件。tests/stm32h7-platform-load.sh'
            else:
                yield configuration, 'OPEN', '固定 REPL が cpus/stm32h7.repl を継承。等価なロード検証なし'
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
            status, note = 'PARTIAL', '診断 REPL でボタン・ゼロ行・フレームを確認。固定 H743 REPL はボタン宣言を読み込むが公開 ELF が DSI 未割り当ての 0x50000430 で停止'
        elif file.endswith('STM32H7_RCC.robot') and name.startswith('Should Pass Zephyr'):
            if name.endswith('With CSI 4'):
                note = '固定 H743 REPL と ELF で USART3 が無出力のまま8仮想秒でタイムアウト。別の実行で PC=0x0800479c を観測'
            else:
                status, note = 'PASS', '固定 H743 REPL と同一 ELF・USART3・8秒期限で PROJECT EXECUTION SUCCESSFUL を照合。tests/stm32h7-rcc-robot.sh'
        elif file.endswith('STM32H7_RCC.robot') and name in {
            'Should Reset Peripheral', 'Should Preserve Peripheral Frequency', 'Should Enable And Disable Peripheral',
        }:
            status, note = 'PASS', {
                'Should Reset Peripheral': '固定 H743 REPL、RCC 0x98 に 1 を書き、実リセットのログ Resetting TIM1 を照合',
                'Should Preserve Peripheral Frequency': '固定 H743 REPL、RCC 0x98 に 1 を書く前後の TIM1 Frequency が 64000000',
                'Should Enable And Disable Peripheral': '固定 H743 REPL、APB2ENR への 0xFFFFFFFE・0x1 の書込みを挟み TIM1 の true/false/true を照合',
            }[name] + '。tests/stm32h7-rcc-timer-robot.sh'
        elif file.endswith('IIS2MDC.robot'):
            status, note = 'PASS', '固定 stm32h753 REPL・I2C1 と PG11・同一 ELF。上流 csv2resd の 9 ブロックを供給し、USART2 の9組を同順・各8秒以内に照合'
        elif file.endswith('INA228.robot'):
            status, note = 'PARTIAL', '固定 Nucleo REPL・同一 ELF・同じ RESD・3秒期限。電圧・電流・電力・温度は許容差内。電力量と電荷が上流許容差外'
        elif file.endswith('VEML7700.robot'):
            if name in {'Should Read Illuminance', 'Should Work After Errata Driver Update'}:
                status, note = 'PASS', '固定上流 ELF・Nucleo REPL と同じ als 登録。照度変更を挟んで USART3 の期待値を照合。tests/stm32h7-veml7700-robot.sh'
            elif name in {'Should Read Samples From RESD', 'Should Read Samples From Timestamped RESD'}:
                status, note = 'PASS', '同一 ELF・上流 CSV と csv2resd による同じ RESD を供給。USART3 で 0/499/12000/12000 を順に照合。tests/stm32h7-veml7700-robot.sh'
            elif name == 'Should Work From Zephyr Shell':
                status, note = 'PARTIAL', '同一上流 ELF の USART3 プロンプト後に 500 lx と上流コマンドを送信。8 仮想秒以内にセンサー応答が出ない'
        elif name == 'Should Talk Over Ethernet':
            status, note = 'PARTIAL', '同一 ELF・未変更 Nucleo REPL・仮想スイッチ。10 仮想秒で成功カウンター 0x2c とサーバーの応答 10 回を確認。クライアント UART の比較行と Robot の順序条件が未達'
        elif file == 'tests/platforms/nucleo_h753zi.robot' and name == 'Should Have Ethernet as Link Up in CubeMX':
            note = '同一 ELF を未変更 Nucleo REPL で試行。ARM EABI5 hard-float ELF のローダー拒否で UART 観測前に停止'
        elif file == 'tests/platforms/nucleo_h753zi.robot' and name == 'Should Pass ADC DMA Test':
            status, note = 'PASS', '同一 ELF・未変更 Nucleo REPL・USART3。固定上流と同じ順序で 7 件の PASS 行を照合。tests/stm32h7-nucleo-ztest-robot.sh'
        elif file == 'tests/platforms/nucleo_h753zi.robot' and name == 'Should Pass Zephyr Encryption Sample':
            status, note = 'PASS', '同一 ELF・未変更 Nucleo REPL・USART3。ECB/CBC/CTR の暗号化と復号の Match 行を上流順に照合。tests/stm32h7-nucleo-ztest-robot.sh'
        elif file == 'tests/platforms/nucleo_h753zi.robot' and name == 'Should Not Run Out Of Ring Buffer Space When Writing Long Line To Uart':
            status, note = 'PARTIAL', '同一 ELF・長文・CR・50 ms 後のプロンプトを観測。ただし追加観測で RX ring buffer full が出るため未達'
        elif file == 'tests/platforms/nucleo_h753zi.robot' and name == 'Should Hear Loopback CAN Messages':
            note = '同一 ELF は初期化を完了。最初の Counter received: 0 が 8 仮想秒以内に出ない。FDCAN の送信・FIFO ループバック未実装'
        elif file == 'tests/platforms/nucleo_h753zi.robot' and name == 'Should Pass Interrupt Test':
            status, note = 'PARTIAL', '同一 ELF で test_arm_esf_collection が PASS。test_arm_interrupt は reason 2 の検証で FAIL、test_arm_user_interrupt は 0x08001c0c で Thumb 状態を持たない 0 への分岐で停止'
        elif file == 'tests/platforms/nucleo_h753zi.robot' and name == 'Should Blink Led':
            status, note = 'PASS', '同一 ELF・未変更 Nucleo REPL・USART3。UART 一致で停止し、GreenLED の変化を 8 仮想秒観測、点灯・消灯各 1 秒 ±5%。tests/stm32h7-nucleo-led-robot.sh'
        elif file == 'tests/platforms/nucleo_h753zi.robot' and name == 'Should See Button Press':
            status, note = 'PASS', '同一 ELF・未変更 Nucleo REPL・USART3・UserButton1 Press/Release。上流順に UART と GreenLED を観測し、各 LED 期待値を 1 仮想秒以内に照合。tests/stm32h7-nucleo-led-robot.sh'
        if file == 'tests/unit-tests/platform_descriptions.robot' and name in {
            'Should Gracefully Fail At Invalid Using',
            'Should Not Crash On Empty Platform String',
            'Should Gracefully Fail At Abstract Class Construction',
        }:
            status, note = 'PASS', '同じインライン REPL を check-platform --from-string に渡し、固定上流の成功・エラー条件と診断文字列を照合。tests/stm32h7-platform-errors-robot.sh'
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
        if kind == 'STM32H7':
            rows.append((repository, source, f'{i+1}-{k+1}', name.group(1), args, 'PASS',
                         '固定上流と同じ RCC 初期化、レジスター書込み順、readback の期待値を照合。src/stm32h7_net_control.rs'))
            continue
        if kind == 'FT5336':
            fixtures = {
                'NormalTest': ('false,false,false', '(50,60)', 'normal_test'),
                'RotatedTest': ('true,false,false', '(60,50)', 'rotated_test'),
                'InvertedTest': ('false,true,false', '(60,60)', 'inverted_test'),
                'RotatedInvertedTest': ('true,false,true', '(90,50)', 'rotated_inverted_test'),
            }
            flags, coordinates, test = fixtures[name.group(1)]
            binding = f'rotated,invertedX,invertedY={flags}; MaxX=110,MaxY=150; MoveTo(50,60),Press,Write([3]),Read(4)'
            rows.append((repository, source, f'{i+1}-{k+1}', name.group(1), binding, 'PASS',
                         f'上流と同じ状態値 2 と座標 {coordinates} を照合。ft5336::tests::{test}'))
            continue
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
        if name in {'Should Count Down SysTick With Reload Set Before Reset', 'Should Count Down SysTick With Reload Set After Reset'}:
            status, reason = 'PASS', '共通 SysTick モデルで上流と同じ 100 Hz・RELOAD=10・レジスター書込み順と 0/90/100/110 ms の CVR=10/1/10/9 を照合。src/systick.rs'
        elif name == 'Should Wake Up From WFE When SEVONPEND Is Set':
            status, reason = 'PASS', '共通 Cortex-M/NVIC で同じ WFE/STR/B、SCR=0x10、IRQ5、1 ms 後のメモリー値 0 と IRQ 後 1 秒以内の 0xDEADBEEF を照合。src/thumb.rs'
        elif name == 'NVIC Base Priority Mask Setting Should Not Survive Reset':
            status, reason = 'PASS', '共通 Cortex-M の BASEPRI=0x50、優先度ブーストとの一致、CPU リセット後の不一致を同じ順序で照合。src/thumb.rs'
    elif suite == 'arm-dsp-examples-cortex-m4.robot':
        status, reason = 'OPEN', '汎用 Cortex-M4F と TrivialUart の CMSIS-DSP ELF。STM32H747 の副コアに適用'
    elif suite == 'arm-ge-flag.robot':
        status, reason = 'PASS', '固定 Cortex-M7 と同じ 10 命令と R4 の期待値を照合。src/thumb/ge_upstream.rs'
        if '(usub8)' in name:
            reason += '。タイトルは usub8、固定フィクスチャの実命令は ssub8'
    elif suite == 'ComparingTimerTests.cs':
        if name == 'ShouldThrowOnCompareHigherThanLimit':
            status, reason = 'OPEN', '汎用 ComparingTimer の Compare setter 拒否条件は STM32 TIM の CCR で観測できない。汎用 API の等価移植が残る'
        else:
            status, reason = 'PASS', 'STM32 タイマーで上流と同じ 1 MHz・周期・比較値・CNT 書込みを実行。各区間の CNT/CCR1 と CC1 イベント数を照合。src/stm32h7_timer.rs'
    elif suite == 'ParserTests.cs' and name in {
        'ShouldParseEntryWithSimpleRegistrationInfo',
        'ShouldParseEntryWithRangeRegistrationPoint',
        'ShouldParseEntryWithStringRegistrationPoint',
        'ShouldParseSingleRegistrationPointInBraces',
        'ShouldParseSingleRegistrationPointInBracesWithSemicolon',
        'ShouldParseEntryWithManyRegistrationPoints',
        'ShouldParseEntryWithManyRegistrationPointsWithSemicolons',
        'ShouldFailOnNonIdentifierRegistrationPointInBraces',
        'ShouldFailOnNonIdentifierRegistrationPointInBracesWithSemicolon',
        'ShouldFailOnNonIdentifierRegistrationPointInBracesWithLeadingSemicolon',
        'ShouldFailOnMultipleRegistrationPointsWithNonIdentifierInBracesWithSemicolons',
        'ShouldParseEntryWithOneAttributeInBraces',
        'ShouldParseEntryWithOneAttributeInBracesWithSemicolon',
        'ShouldParseEntryWithTwoAttributesInBracesAndOneSemicolon',
        'ShouldParseEntryWithTwoAttributesInBracesAndAllSemicolons',
        'ShouldFailOnEntryWithOneAttributeInBracesWithMultipleSemicolons',
        'ShouldFailOnNonIdentifierWithWithMultipleSemicolons',
        'ShouldFailOnEntryWithEmptyStatementAndOneAttributeInBracesWithSemicolon',
        'ShouldFailOnEntryWithAttributeAndNonIdentifierInBraces',
        'ShouldFailOnEntryWithTwoAttributesInBracesWithTrailingSemicolon',
    }:
        status, reason = 'PASS', '固定上流の REPL 入力で解析結果または構文エラーを照合。src/platform.rs'
    elif suite == 'PreLexerTests.cs' and name in {
        'ShouldFailOnUnterminatedString',
        'ShouldFailIfTheMultilineCommentFinishesBeforeEndOfLine',
        'ShouldFailOnSingleLineMultilineComment',
    }:
        status, reason = 'PASS', '固定上流の入力文字列でコメント・文字列処理の構文エラーを照合。src/platform.rs'
    elif suite == 'UsingTests.cs':
        if name in {f'ShouldFollowAttributeOverrideOrder{index}' for index in range(5)}:
            status, reason = 'PASS', '固定上流と同じ REPL・インクルード順・Placeholder 期待値を合成結果で照合。MockCPU の Placeholder は単純な文字列プロパティ。実 H743 GPIO でも合成値を観測'
        elif name in {
            'ShouldFailOnDuplicateUsingEntry', 'ShouldFailOnRecurringUsings',
            'ShouldFailOnDirectlyRecurringUsings', 'ShouldFailOnNonExistingUsingFile',
        }:
            status, reason = 'PASS', '固定上流と同じインクルードのグラフ・ファイル別定義で、対応する診断を照合。src/platform.rs'
        elif name in {
            'ShouldHandleAbsolutePathUnix', 'ShouldHandleRelativePathInSameDirectoryUnix',
            'ShouldHandleRelativePathInParentDirectoryUnix', 'BackslashShouldNotBePathSeparatorOnUnix',
        }:
            status, reason = 'PASS', '固定上流と同じ Unix パス引数、呼出元パス、解決結果を照合。src/platform.rs'
        elif name in {
            'ShouldHandleAbsolutePathWindows', 'ShouldHandleRelativePathInSameDirectoryWindows',
            'ShouldHandleRelativePathInParentDirectoryWindows',
        }:
            status, reason = 'OUT', '上流 RuntimeInfo.IsWindows() ガードが非 Windows で Assert.Ignore。Windows ホストとドライブパスを要求'
        else:
            status, reason = 'OPEN', 'ファイル単位の local シンボル表と前後方向の型付き参照解決が未実装。MockCPU.OtherCpu の固定フィクスチャは対象に維持'
    elif suite == 'MultiCPUTests.cs':
        status, reason = 'OPEN', '汎用 ICPU モックのシステムバス契約。台数は実機コア数の要求ではないため全構成を維持'
    elif suite == 'MCAN.robot' and name in {
        'Should Boot Linux And Login With MCAN',
        'Should Handle CAN Messages Issued Through CAN Utils Tools',
    }:
        status, reason = 'OUT', '固定 Zynq Linux ゲストで実行し、CAN Utils は Zynq ログイン状態を要求'
    elif suite == 'LSM6DSO_IMU.robot':
        if name == 'Should Read Samples From RESD (Polling)':
            status, reason = 'PARTIAL', '固定 ELF・Nucleo REPL・上流 RESD (12 Hz、開始 100 ms)。第3組の加速度までは一致し、角速度は終端後の既定値。2秒の期限で未達'
        elif name == 'Should Read Samples From RESD (Interrupt)':
            status, reason = 'PASS', '固定 ELF・Nucleo REPL・PC13・上流 RESD (12 Hz、開始 100 ms)。全刺激と USART3 の期待値を上流順、各2秒以内に照合'
        else:
            status, reason = 'PASS', '固定 ELF・Nucleo REPL・PC13。上流と同じプロパティ変更、USART3 の順序と2秒期限を照合'
    elif suite == 'LPS25HB.robot':
        status, reason = 'PASS', {
            'Should Read Temperature And Pressure': '固定 v4.2 ELF・Nucleo REPL と同じ I2C 登録。既定値・正負・範囲外を含む全刺激と USART3 の全期待値を上流順に照合',
            'Should Correctly Calculate Pressure': 'Pressure=1023.346923828125 の getter と直接 Read(0x28, 3) の 8D F5 3F を照合。src/lps25hb.rs',
            'Should Read Samples From RESD': '固定 v4.2 ELF と上流 CSV から同じ 25 Hz・開始 70 ms の RESD を生成。3 サンプルと終端復帰 2 回を USART3 で照合',
            'Should Work After Errata Driver Update': '固定 v4.4 ELF、Nucleo REPL と同じ I2C 登録および実行前の温度・気圧設定で USART3 の上流期待値を照合',
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
