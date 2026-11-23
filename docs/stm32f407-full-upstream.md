# STM32F407 Discovery 上流ケース台帳

[ケース台帳](stm32f407-upstream-cases.tsv) は固定上流のリポジトリ、ファイル、ソース行、
ケース名、パラメーター、フィクスチャ、判定根拠または証拠を記録します。

| 出典 | 対象実行ケース | 上流等価合格 | 部分移植 | 未達 | 対象外 |
|---|---:|---:|---:|---:|---:|
| Renode | 510 | 164 | 0 | 346 | 240 |
| Infrastructure | 217 | 50 | 0 | 167 | 7 |
| **合計** | **727** | **214** | **0** | **513** | **247** |

分母は対象の具象行です。未判定行は 0 件です。BusFault の命令別行への参照を
保持する親行 1 件（`expanded`）には実行件数を割り当てません。
`ported-pass` は既存の等価検証、`partial` は観測条件または検査項目に差がある移植、
`pending` は等価検証がまだない対象ケースです。
今回の回帰で実行した FreeRTOS と Contiki のローカルスモークは、この集計に追加していません。

## 適用・展開の監査

- 実フィクスチャからボード、CPU、ISA、実行に必要な配線を確認しました。F429 の
  名前を持つネットワーク ELF 2 本は実際には Discovery REPL 2 台と仮想スイッチで
  動くため、TCP と両 UART を検査する 1 構成として残しました。ADC の外付け
  AD4011 は、上流オーバーレイが F407 SPI1 に接続する 4 ケースを対象としました。
- 単一 CPU の排他アクセス、Thumb DSP、セミホスティング、MPU、UART テスター、
  診断やパーサー、バス、タイマー、GPIO、DMA、CRC の共有テストを残しました。
  合成メモリーやモックはフィクスチャの一部です。異機種の CPU を使うケースでも、
  同じ刺激と観測を M4F で検査できるものは対象です。固定された TrustZone の別名、
  MVE 命令、A/R プロファイル固有命令、複数コアの同一バス上の隔離などを
  必要とする場合は、台帳行に別構成と対象外の理由を記載しました。
- Robot の独立した入力組は展開しました。例として Cortex-M の LLVM 往復は
  4 命令、セミホスティングの一括実行は 22 組に分かれます。状態を共有する
  検査手順は 1 構成に保持しました。NUnit は DMA の 3×3×4×4、SystemBus の
  2×2、REPL 数値パーサーの 3・8・4 の組を個別行にしています。
- `Should Load Repls` は固定上流の列挙とブラックリストを照合しました。
  CPU、Discovery、Discovery kit の 3 REPL が対象です。
  `stm32f4_discovery-additional_gpios.repl` と `stm32f4_discovery-bb.repl` は
  上流の単独ロードの列挙から除かれ、kit の `using` で読まれます。
  F412/F429 SoC の各変種には対象外の根拠を記載しました。列挙に含まれる
  その他の REPL は F407 をロードしない別のボードまたは SoC の構成です。
- Robot の継続行 `...` をケースとして拾った 32 行と、同じ BXNS / TrustZone
  構成の重複を除きました。異なる上流ファイルのケースや異なるパラメーターは、
  同じ移植ランナーを使う場合でも出典を保持しています。

## 参照リビジョン

| リポジトリ | コミット | 調査内容 |
|---|---|---|
| Renode | `c38ac3babcd38a7144b9498e3ca4c2226b76aa37` | Discovery 固有の全 13 件、Cortex-M・STM32 と REPL 関連の Robot/NUnit 候補 |
| Infrastructure | `09ac7c563095c5410258ecc1b1a6b841a013db85` | Peripheral Map 実装、STMCAN NUnit 3 件、タイマー・バス・DMA・GPIO・CRC の共有候補 |
| tlib | `c04522a9c886ba743c77a86b0c879103a2ef46d0` | Cortex-M エミュレーターの関連サブモジュール。このツリーに Robot/NUnit スイートはない |
| Berkeley SoftFloat | `6a9815f99719a24705e7a834f0ef34ac57441904` | 固定リビジョンを確認。このツリーに Robot/NUnit スイートはない |

元の Renode チェックアウトは読み取り専用で参照し、Infrastructure と tlib は別ディレクトリに
取得しました。台帳では同じ STM32F4 系でも F412G Tock のボード固有 ELF や
Cortex-M33 TrustZone 専用ケースに機種上の対象外理由を付けています。共有ケースでは
実装未達、ビルド依存、長い実行時間は対象外理由にしません。

## 再現

```sh
export CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=gcc
export RENODE_ROOT="$(realpath ../renode)"
test "$(git -C "$RENODE_ROOT" rev-parse HEAD)" = c38ac3babcd38a7144b9498e3ca4c2226b76aa37
cargo test
tests/stm32f407-upstream-smoke.sh target/full-upstream-stm32f407
```

個別の Peripheral Export は次の通りです。生成する JSON は Peripheral Map 1.0 の
ヘッダー、システムバス配下の別名、モデル名、登録アドレスを持ちます。

```sh
cargo build --release
target/release/penv peripherals export target/stm32f407-peripherals.json \
  --platform "$RENODE_ROOT/platforms/cpus/stm32f4.repl" --platform-root "$RENODE_ROOT"
```

スモークの出力には `Peripheral Export File Created` と
`Cortex-M4F context stacking: Test succeeded` が現れます。
元の公開 ELF は `fpu-v7m.log` に S0–S15 の復元結果を出力します。
`cargo test renode_stmcan_init_test` は、上流の初期化モード、送受信メールボックス、
フィルター制御と 28 バンクの比較を再現します。

## 既存スモークの時刻条件

- PWM 2 件は未変更の CPU REPL に上流と同じ PA6 → PD12 を追加します。
  CPU は固定 Renode の既定 100 MIPS、タイマーは REPL の 10 MHz、SysTick は 72 MHz です。
  周波数で最初に一致した行を読み飛ばし、次の測定値を Robot と同様に整数部で比較します。
  確認値は通常版が 1000 Hz・33.0%、4 チャネル版が 1001 Hz・32.9% でした。
- RTC は 32 kHz の上流オーバーレイを使用し、プロンプト行の次の実行ステップで `s` を渡します。
  `rtc-alarm.tsv` の行末時刻から Renode の `Elapsed Virtual Time` の秒フィールドを再現し、
  2・4・6・8 秒を確認します。4 行目の後は UART の未改行出力も含め、3 秒間無出力を要求します。
- FAULTMASK 2 件は RCC の事前書き込みを除き、上流 REPL の TIM3 周波数を使用します。
  `faultmask.tsv` の `Sleeping` 行末から次の UART バイトまで 1 秒以上と、
  マスク解除後 3 回、例外復帰テストで 4 回の割り込み出力を確認します。

`--uart-transcript PATH` の各行は仮想開始ステップ、終了ステップ、UART テキストの TSV です。
RTC と FAULTMASK は既定の 1 ステップ/µs で上流の仮想時刻と UART 観測条件を検査します。
この 2 スイートの CPU 命令実行速度は固定 Renode の既定 100 MIPS と異なります。
100 MIPS の個別検証には下記の時刻条件付きランナーを使います。
スモークは既知 ELF の実行を仮想ステップ上限で区切ります。Robot の端末待機の既定タイムアウト
（待機ごとに 8 秒）は汎用機能として再実装していません。

## 未達

未達 513 件には、共有フォールト配送、汎用 UART テスター、GDB・モニター・
スナップショットの操作、実ファームウェアの DSP/FPU/MPU、Ethernet、NUnit
フィクスチャの等価検証が残ります。全件合格ゲートの条件は未達です。

### 適用監査時の再検証

`target/stm32f407-scope-audit/` に回帰ログを保存しました。
`cargo test` は 76 件が通過しました。`faults.log` は Cortex-M4F ELF 16 件の
UART 判定を、`freertos.log` と `fpu.log` は未変更 Discovery 定義での TIM2 キュー、
UART エコー、CAN ループバック、IWDG 再起動と FPU コンテキスト判定を記録しています。
`contiki.log` には `Contiki 3.x started.` と `UDP server started`、
`zephyr.log` には `Hello World! stm32f4_disco`、
`upstream-fpu.log` には公開 ELF の `Test succeeded` が出ました。
Contiki、Zephyr、公開 Cortex-M4 ELF の起動は上記のスモークの同名手順を個別に再実行しました。
PWM など今回再実行していない既存合格は台帳の既存証拠を参照します。

```sh
export CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=gcc
export RENODE_ROOT="$(realpath ../renode)"
work=target/stm32f407-scope-audit
mkdir -p "$work"
cargo test >"$work/cargo-test.log" 2>&1
tests/cortex-m4f-faults.sh "$work/faults" >"$work/faults.log" 2>&1
tests/stm32f407-freertos-smoke.sh "$work/freertos" --upstream >"$work/freertos.log" 2>&1
tests/stm32f407-freertos-smoke.sh "$work/fpu" --fpu >"$work/fpu.log" 2>&1
```

### Cortex-M4F の精密 BusFault と NOCP

```sh
export CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=gcc
cargo test
tests/cortex-m4f-faults.sh target/cortex-m4f-faults
tests/stm32f407-freertos-smoke.sh target/stm32f407-fault-upstream --upstream
tests/stm32f407-freertos-smoke.sh target/stm32f407-fault-cm4f --fpu
```

Clang と LLD で上流のインラインアセンブリを Cortex-M4F 用 ELF に組み立てます。
CPU モデルを ARMv7E-M に変更し、Robot がモニターから行っていた設定と検査を ELF 内で
行います。13 個の ELF はそれぞれ UART に `BusFault OK`、`NOCP`、または `OK` を
出し、レジスター・スタック値が違えば FAIL を出します。

- 通常の精密 BusFault は命令アドレス `0x200`、SP `0xfe0`、R1 `0x11`、
  R2 `0xa5a5a5a5`、CFSR `0x8200` を確認します。BFAR はモック周辺機器の
  `0x100000`、未マップ読み出しでは `0x200000` です。
- 無効化した BusFault と同じ優先度の実行中ハンドラーからの BusFault は HardFault に
  エスカレーションし、HFSR は `0x40000000` です。後者は `0xfd8` に
  BusFault ハンドラーの命令アドレス `0x300` が保存されます。前者は HFSR の W1C も確認します。
- 連続実行の読み書きは R10 と R11 の PC が一致することを照合します。FPv4-SP の
  `VSTR/VLDR`（S0/D0）は拡張フレームの SP `0xf98` と同じ精密フォールト値を検査します。
- CPACR が初期値のときは UsageFault に入り、`0xe000ed2a` の半ワードが `0x0008`、
  スタックに障害命令の PC が保存されることを確認します。フルアクセス設定時は
  `VLDR → VCVT.S32.F32 → VCVT.F32.S32 → VSTR` が完了し、`OK` を出します。

未実装の FP オペコードと特権限定 CPACR アクセスは残件です。

### Cortex-M4F の命令フェッチと例外ベクター

上記の `tests/cortex-m4f-faults.sh` はフェッチと IRQ ベクターの ELF 3 件を追加で実行し、
各 ELF の UART に `Fetch fault OK` が出ることを照合します。
Rust テストはリセットベクター、初期 MSP、NMI・HardFault の取得失敗と、
NMI 進入時にベクター取得とスタック書き込みが同時に失敗するケースの状態を検査します。

- Thumb-2 の第1・第2半ワードの取得失敗は `CFSR.IBUSERR=1` を記録し、
  命令先頭の PC を保存します。BFAR と BFARVALID は変更しません。BusFault が無効な場合は
  HardFault に配送し、`HFSR.FORCED=1` を記録します。
- IRQ16 のベクター取得失敗は `HFSR.VECTTBL=1` を記録して HardFault に配送します。
  SP は `0xfe0`、CFSR は `0`、元の IRQ は ISPR に残り IABR では非アクティブです。
- NMI・HardFault のベクター取得失敗では `DHCSR.S_LOCKUP` と例外番号、SP、LR、
  `HFSR.VECTTBL` を検査します。同時スタック障害では `CFSR.STKERR` も記録します。
  リセットベクターまたは初期 MSP の取得失敗は SP `0`、LR `0xffffffff`、IPSR `0` と
  HardFault のアクティブ状態を検査します。

上流 Robot の Cortex-M55 フィクスチャは、Cortex-M4F の EXC_RETURN
（スレッド・MSP の基本フレームでは `0xfffffff9`）に置き換えました。
上流の SHCSR に現れる NMI・HardFault のアクティブ状態は Rust テストで内部状態を照合します。
初期 MSP がゼロのリセットでは、2 ステップ目の INVSTATE ロックアップと DHCSR の
CPU・デバッガー別の読出しを確認します。命令実行時のロックアップ解除は下記を参照してください。

## Robot の時間条件を移植した 7 件

`--uart-check PATH` はタブ区切りの `wait 秒 文字列`、`send 文字列`、
`second 秒`、`idle 秒` を順に実行します。`wait` は完了行の部分一致、
`send` は次の実行ステップで改行なしの入力、`second` は経過仮想時刻の秒整数部、
`idle` は未改行バイトも含む無出力を検査します。テスト計画が全行通過すると終了コード
0、未一致・タイムアウト・無通信中の出力では非 0 です。診断ログには仮想時刻を記録します。

```sh
export CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=gcc
export RENODE_ROOT="$(realpath ../renode)"
export PENV_F407_TIMED_RENODE_ROOT="$RENODE_ROOT"
test "$(git -C "$RENODE_ROOT" rev-parse HEAD)" = c38ac3babcd38a7144b9498e3ca4c2226b76aa37
work=target/stm32f407-timed-tests
mkdir -p "$work"
(
    cargo test -- --nocapture >"$work/cargo-test.log" 2>&1
    echo "$?" >"$work/cargo-test.status"
    cargo test renode_f407_flash_ignores_erase_without_mer_or_ser -- --nocapture >"$work/flash.log" 2>&1 &&
        rg -qF 'Tried to erase flash, but MER and SER are reset. This should be forbidden, ignoring...' "$work/flash.log" &&
        rg -qF 'FLASH[0x08000000] = 0xDEADBEEF before and after erase request' "$work/flash.log"
    echo "$?" >"$work/flash.status"
    tests/stm32f407-timed-tests.sh "$work" >"$work/timed.log" 2>&1
    echo "$?" >"$work/timed.status"
) &
```

`PENV_F407_TIMED_RENODE_ROOT` を指定すると WFE・BASEPRI・フラッシュの Rust テストは
固定 upstream REPL をロードします。ゼロ MSP は上流の合成フィクスチャを Cortex-M4F に
移植します。期待ログは `WFE: 0 at 0.001 s`、`BASEPRI: priority boost 0x50 before reset, 0x100 after reset`、
`zero MSP: two steps` と、対象フラッシュ要求と同時に出る警告です。フラッシュの
Robot は要求の直後に既に出ているログを最長 3 仮想秒待ちます。移植は同期書き込みの
その診断を照合し、同じ書き込みの前後で `0xDEADBEEF` を確認します。

公開 ELF 3 本は SHA-1 を照合し、100 MIPS、上流の周辺クロック、USART2、行ごとの
8 仮想秒の待機で実行します。RTC はプロンプト後に `s` を 1 バイト送信し、
アラームの時刻 2・4・6・8 秒とその後 3 秒の無通信を検査します。FAULTMASK は
`Sleeping` の後 1 秒の無通信と解除後 3 回の IRQ、例外復帰のケースは 4 回の IRQ を検査します。
実行は `rtc` または `faultmask` を第 2 引数に渡すと個別に選択できます。

今回の検証ログでは Rust の 81 テスト（`cargo-test-final.log`）、Cortex-M4F フォールト ELF 16 件、FreeRTOS
FPU 回帰、フラッシュ診断の終了状態が 0 でした。`terminal/` には UART チェック
の小型 ELF と、成功、未改行バイトによるアイドル違反、8 秒の待ち時間切れのログがあります。
同じ成功・失敗条件は CLI 統合テストにも追加しました。

FAULTMASK 2 件のタイムスタンプは `faultmask-check.log` と
`faultmask-exit-check.log` にあります。1 秒の無通信、解除後の IRQ 3 回、例外復帰後の
IRQ 4 回を確認しました。最初の実行ではシェルが動作中のファイル編集の影響を受け、
両 ELF が計画を完了した後にラッパーが 127 で終了しました。各 ELF の SHA-1、全操作、
タイムスタンプを突き合わせた `faultmask-evidence.log` の終了状態は 0 です。
最終版のラッパーは `bash -n` でも確認しました。

最後の部分移植だった RTC アラームは、公開 RIOT ELF の SHA-1 と固定上流の
フィクスチャを照合し、100 MIPS で全操作の合格を確認しました。
アラームは 2.000070、4.000070、6.000070、8.000070 仮想秒に出て、
11.000070 仮想秒まで UART 出力がありませんでした。実行と証拠照合はともに終了状態 0 です。
`target/stm32f407-timed-tests-rtc/` の `rtc-check.log`、`rtc.log`、`rtc.status`、
`rtc-evidence.log`、`rtc-evidence.status` に結果があります。台帳のみの更新のため、
完了した ELF 実行を再利用しました。

同じケースだけをビルドから再現するには、上記の環境変数を設定して次を実行します。

```sh
work=target/stm32f407-timed-tests-rtc-repro
mkdir -p "$work"
(
    tests/stm32f407-timed-tests.sh "$work" rtc >"$work/run.log" 2>&1
    rc=$?
    printf '%s\n' "$rc" >"$work/rtc.status"
    exit "$rc"
) &
```

## Cortex-M4F の例外スタック保存・復元フォールト

固定上流の `tests/unit-tests/arm-cortex-m-bus-fault.robot` から 9 ケースを Rust に
移植しました。上流の RAM、ベクター、ハンドラー、故障する周辺機器のアドレスを
使用します。復帰トークンは ARMv7-M の Thread/MSP 基本フレーム `0xfffffff9`、
上流で使う NMI/HardFault アクティブビットは Cortex-M4F の内部例外状態を照合します。

- 保存では `CFSR.STKERR`、NMI による HardFault の保留、IRQ16 の pending/active、
  同優先度での BusFault 選択、HardFault を中断した NMI のロックアップと IT 状態を検査します。
- 復元では読み出しのみを故障させ、`CFSR.UNSTKERR`、NMI 復帰時のアクティブ状態、
  元のフレームを使うテールチェーンと FPCA クリアを検査します。
- 遅延 FP 保存では `FPCAR=0x100020`、`HFRDY`、`LSPACT`、`CFSR.LSPERR`、
  `HFSR.FORCED` と HardFault の保留を照合します。Thread と HardFault からの NMI で
  保留とロックアップをそれぞれ再現します。

```sh
export CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=gcc
export RENODE_ROOT="$(realpath ../renode)"
export PENV_F407_TIMED_RENODE_ROOT="$RENODE_ROOT"
work=target/stm32f407-stack-faults
mkdir -p "$work"
cargo test thumb::stack_fault_tests
(
    cargo test >"$work/cargo-test.log" 2>&1
    echo "$?" >"$work/cargo-test.status"
    tests/cortex-m4f-faults.sh "$work/faults" >"$work/fault-elf.log" 2>&1
    echo "$?" >"$work/fault-elf.status"
    tests/stm32f407-freertos-smoke.sh "$work/freertos" --fpu >"$work/freertos-fpu.log" 2>&1
    echo "$?" >"$work/freertos-fpu.status"
) &
```

`cargo test` は 90 件、フォールト ELF は 16 件が合格しました。FreeRTOS では
`FPU context OK: A`、`FPU context OK: B`、`IWDG reboot: SRAM retained` を確認しました。
公開の Cortex-M4 拡張フレーム ELF は上記の上流スモークの `fpu-v7m` 手順を単独で実行し、
`upstream-fpu.log` に `Test succeeded` を記録しました。実行上限での終了コード 2 と
UART 成功行を合わせて検証しています。TrustZone の追加スタック、共有のフォールト配送の
残件、一般の遅延 FP 準備ビットの組み合わせは今回の 9 ケースの外に残ります。

## STM32F407 USART の受信時刻

優先グループは固定上流 `tests/peripherals/STM32_UART.robot` の全 6 件です。
CPU を持たない上流フィクスチャと同様に、Rust のテストはアドレス `0x40000000` に
F4 UART を配置し、同じバスアクセスと 1 µs の時間刻みで検査します。16 MHz の入力と
`BRR=0x10` で、9 µs 後に RXNE が立たず、10 µs 後にデータを読めることを確認します。
パリティーを有効にしても 8 ビットのワードにパリティーを含めるため、フレーム長は
10 µs です。IDLE の検出・取り消し、BRR がゼロになったときのバイト順序、
UE/RE を無効にしたときの受信破棄も上流と同じ順序で検査します。

```sh
export CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=gcc
export RENODE_ROOT="$(realpath ../renode)"
export PENV_F407_TIMED_RENODE_ROOT="$RENODE_ROOT"
work=target/stm32f407-uart-timing
mkdir -p "$work"
cargo test stm32_uart::
(
    cargo test >"$work/cargo-test.log" 2>&1
    echo "$?" >"$work/cargo-test.status"
    tests/stm32f407-freertos-smoke.sh "$work/freertos" --upstream >"$work/freertos.log" 2>&1
    echo "$?" >"$work/freertos.status"
    tests/stm32f407-freertos-smoke.sh "$work/freertos-fpu" --fpu >"$work/freertos-fpu.log" 2>&1
    echo "$?" >"$work/freertos-fpu.status"
) &
```

FreeRTOS の期待値は `UART4 echo=X/Y/Z`、TIM2 のキュー通知、CAN ループバックと
IWDG 再起動です。`--fpu` では `FPU context OK: A` と `FPU context OK: B` も照合します。
送信タイミング、パリティーエラー、汎用端末テスターの未移植ケースは、この受信スイートの
合格件数には含めていません。

`cargo test` は 96 件、FreeRTOS の通常版と FPU 版はともに通過しました。
UART 入力変更に対する追加の短い回帰として、先の RTC 手順で取得した RIOT ELF に
同じ UART 計画を既定の 1 MIPS で適用し、4 回のアラームと最後の 3 秒の無通信を
確認しました。今回の短い回帰には 100 MIPS での再実行を含めていません。

```sh
rtc=target/stm32f407-timed-tests-rtc
target/release/penv run --script "$rtc/rtc.resc" --platform-root "$RENODE_ROOT" \
    --f4-repl-timing --uart-check "$rtc/rtc.tsv"
```

## Cortex-M4 Thumb DSP の GE フラグ

優先グループは固定上流 `tests/unit-tests/arm-ge-flag.robot` の 6 件です。
Cortex-M7 フィクスチャの各 10 命令を取り出し、同じオペランド・開始アドレス
`0x0` の Cortex-M4 ELF に組み立てます。最初の 10 命令が終わった直後の R4 を
追加した UART 出力で取得し、上流の期待値から生成した 4 バイトと照合します。

`sadd8`、`sadd16`、`ssub8`、`ssub16`、`usub8`、`usub16` の順に、
R4 が `0xff00ff00`、`0xffff0000`、`0x00ff00ff`、`0x0000ffff`、
`0xff0000ff`、`0x0000ffff` になります。上流の `usub8` というケース名の
実命令は `ssub8` です。上流の命令列を保持し、符号付きと符号なしの結果が
異なるオペランドで `USUB8` 自体も別の ELF で検査しました。この追加 ELF は
上流台帳の分母に含めていません。

```sh
export CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=gcc
export RENODE_ROOT="$(realpath ../renode)"
export PENV_F407_TIMED_RENODE_ROOT="$RENODE_ROOT"
work=target/stm32f407-ge
mkdir -p "$work"
(
    tests/cortex-m4-ge.sh "$work/elf" >"$work/ge-elf.log" 2>&1
    echo "$?" >"$work/ge-elf.status"
    cargo test >"$work/cargo-test.log" 2>&1
    echo "$?" >"$work/cargo-test.status"
    tests/cortex-m4f-faults.sh "$work/faults" >"$work/fault-elf.log" 2>&1
    echo "$?" >"$work/fault-elf.status"
    tests/stm32f407-freertos-smoke.sh "$work/freertos" --fpu >"$work/freertos.log" 2>&1
    echo "$?" >"$work/freertos.status"
) &
```

`cargo test` 96 件、フォールト ELF 16 件、FreeRTOS の FPU 回帰が通過しました。
残件には CMSIS-DSP の公開 ELF と、飽和・半減など今回の命令群に含まれない
DSP 命令の検証があります。

## Cortex-M4 の IT ブロックと条件コード

固定上流 `tests/unit-tests/it-status.robot` から CPU の単一命令ステップだけで
再現できる 6 ケースを移植しました。Clang で上流アセンブリを Cortex-M4 に
組み立て、4 プログラムの命令バイトをテストの固定配列と照合しました。
上流と同じリセット／NMI アドレス `0x100`／`0x200`、初期 SP `0x800`、
ステップ数、レジスター条件を使用します。

AL の条件照会に対応し、例外保存と検査で xPSR の組み立てを共用します。
8 種類の条件照会と 4 組の xPSR、ITSTATE の `05 → 0a → 14 → 08 → 00`、
条件が偽の命令のスキップ、ブロック内部のフラグ更新と NMI 前後の ITSTATE を検査します。
保存・再読込の 1 件には永続スナップショット、残る 2 件にはブロック開始・終了フックと
ログ待機のモニター機能が必要です。この 3 件は未達です。

```sh
export CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=gcc
export RENODE_ROOT="$(realpath ../renode)"
export PENV_F407_TIMED_RENODE_ROOT="$RENODE_ROOT"
work=target/stm32f407-it
mkdir -p "$work"
cargo test thumb::it_status_tests
(
    cargo test >"$work/cargo-test.log" 2>&1
    echo "$?" >"$work/cargo-test.status"
    tests/cortex-m4f-faults.sh "$work/faults" >"$work/fault-elf.log" 2>&1
    echo "$?" >"$work/fault-elf.status"
    tests/stm32f407-freertos-smoke.sh "$work/freertos" --fpu >"$work/freertos.log" 2>&1
    echo "$?" >"$work/freertos.status"
) &
```

期待値は IT テスト 6 件と全 Rust テスト 102 件の合格、フォールト ELF 16 件の
判定行、FreeRTOS の `FPU context OK: A`、`FPU context OK: B` と
`IWDG reboot: SRAM retained` です。

## STM32F407 の共有 CRC エンジン

固定 Infrastructure の `src/Emulator/Main/Tests/UnitTests/CRCTests.cs` 全 15 件を
Rust に移植しました。STM32 の CRC DR が使う共通エンジンで、入力・出力反転、
初期値と出力 XOR、空入力と分割更新、CRC の各幅を照合します。上流の引数例外は
Rust の型付きエラーで照合します。移植は共有エンジンを対象とし、F407 上の
レジスターで設定できる幅と多項式は機種の定義に従います。

```sh
export CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=gcc
export RENODE_ROOT="$(realpath ../renode)"
export PENV_F407_TIMED_RENODE_ROOT="$RENODE_ROOT"
work=target/stm32f407-crc
mkdir -p "$work"
cargo test crc
(
    tests/stm32f407-crc.sh "$work/elf" >"$work/crc-elf.log" 2>&1
    echo "$?" >"$work/crc-elf.status"
    cargo test >"$work/cargo-test.log" 2>&1
    echo "$?" >"$work/cargo-test.status"
    tests/stm32f407-freertos-smoke.sh "$work/freertos" --fpu >"$work/freertos.log" 2>&1
    echo "$?" >"$work/freertos.status"
) &
```

Discovery の未変更 kit REPL で実行する追加 ELF は、2 ワード更新、CR による
累積値のリセット、IDR の保持を確認すると、USART2 に
`STM32F407 CRC DR=49E3C2FB RESET=FFFFFFFF IDR=5A` を出力します。
この ELF は共有ケースの追加回帰であり、上流台帳の分母には含めていません。
DMA、GPIO、SystemBus など共有インフラのケースは別群として残っています。

## Cortex-M4F の精密 BusFault ロックアップと抑止

固定上流 `tests/unit-tests/arm-cortex-m-bus-fault.robot` の 3 件を、
同じ RAM、ベクター、故障アドレス `0x100000` と Thumb 命令で移植しました。
HardFault 中に IT ブロックのデータアクセスが失敗すると、CFSR と BFAR を記録し、
PC を `0xeffffffe` に固定します。ロックアップ中に 10 ステップを要求しても ITSTATE と
命令の完了数は変化せず、CPU リセットで CPU と NVIC のロックアップ状態が解除されます。
NMI は命令実行時ロックアップを中断でき、元の PC を `0xfd8` に保存します。

`CCR.BFHFNMIGN` は負の優先度のデータフォルトを抑止し、CFSR と BFAR の更新は行います。
Thread モードからの最初の失敗は HardFault に配送され、そのハンドラー内の同じ LDR
から継続して R1 が `0x22` になることを確認しました。Rust フィクスチャは例外配送と
命令の完了を別の呼び出しで観測します。追加検査では、同じ CCR 設定でも命令フェッチの
失敗がロックアップすることと BFAR が保持されることを確認します。

```sh
export CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=gcc
export RENODE_ROOT="$(realpath ../renode)"
export PENV_F407_TIMED_RENODE_ROOT="$RENODE_ROOT"
work=target/stm32f407-lockup
mkdir -p "$work"
cargo test thumb::stack_fault_tests
(
    cargo test >"$work/cargo-test.log" 2>&1
    echo "$?" >"$work/cargo-test.status"
    tests/cortex-m4f-faults.sh "$work/faults" >"$work/fault-elf.log" 2>&1
    echo "$?" >"$work/fault-elf.status"
    tests/stm32f407-freertos-smoke.sh "$work/freertos" --fpu >"$work/freertos.log" 2>&1
    echo "$?" >"$work/freertos.status"
) &
```

期待結果は全 Rust テスト 120 件、フォールト ELF 16 件、FreeRTOS の FPU・IWDG 回帰の
合格です。Fault 後の NMI からロックアップ元に戻る経路や、TrustZone の診断別名を使う
上流ケースはこの 3 件の範囲外です。

## Cortex-M4F の BX・BLX と例外復帰

固定上流 `tests/unit-tests/arm-cortex-m-exception-return.robot` の非 TrustZone
適用ケース 4 件を移植しました。上流と同じ RAM、初期 SP、リセット／NMI ハンドラー
アドレス、Thumb 命令列とステップ順序を使用します。NMI の LR には ARMv7-M の
`0xfffffff9` が入るため、`BLX LR` 後の PC は `0xfffffff8` です。
`BX LR` は `0x20` に戻り、元のスタックを復元します。
Thread モードの `BX 0xffffffb8` では指定 PC を観測した後、次のフェッチで
`INVSTATE` とスタック上の障害 PC を検査します。`BX 0xfeffffff` は
通常の Thumb 分岐として `0xfefffffe` に到達します。

```sh
export CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=gcc
export RENODE_ROOT="$(realpath ../renode)"
export PENV_F407_TIMED_RENODE_ROOT="$RENODE_ROOT"
work=target/stm32f407-exc-return
mkdir -p "$work"
cargo test thumb::exception_return_tests
(
    cargo test >"$work/cargo-test.log" 2>&1
    echo "$?" >"$work/cargo-test.status"
    tests/cortex-m4f-faults.sh "$work/faults" >"$work/fault-elf.log" 2>&1
    echo "$?" >"$work/fault-elf.status"
    tests/stm32f407-freertos-smoke.sh "$work/freertos" --fpu >"$work/freertos.log" 2>&1
    echo "$?" >"$work/freertos.status"
) &
```

期待結果は対象 4 件、全 Rust テスト 124 件、フォールト ELF 16 件、
FreeRTOS の FPU・IWDG 回帰の合格です。Armv8-M 固有の PMSAv8 ベクターケース、
`BXNS`／`BLXNS`、`FNC_RETURN` は台帳の対象外理由に従います。
汎用的な例外復帰トークンの検証や MPU の上流群は別の未達として残ります。

## Cortex-M4 の単一 CPU 排他アクセス

固定上流 `tests/unit-tests/arm-atomic.robot` の単一 CPU ケース 3 件を移植しました。
Cortex-R8 の A32 命令を Cortex-M4 の Thumb-2 で組み立て、同じ初期レジスター、
メモリーアドレス `0x1000`、データ値、操作順序で実行します。対象命令列の直後に
R2 とメモリーの値を UART へ出力し、32 ビット値を照合します。期待値は順に
`(0, 0x0a)`、`(1, 0x00)`、`(1, 0x0a)` です。

ワード排他命令の LR オペランドのデコード範囲を修正し、LDREX のワード境界を
バスアクセス前に検査します。LR の追加 ELF は上流台帳の分母に含めていません。
同一バス上の複数 CPU が必要な排他モニターとその保存・復元は台帳の対象外理由に従います。
GPIO の動的な切り離しには、周辺機器登録解除や外部コネクターのライフサイクルが
必要です。この共有群は未達に残ります。

```sh
export CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=gcc
export RENODE_ROOT="$(realpath ../renode)"
export PENV_F407_TIMED_RENODE_ROOT="$RENODE_ROOT"
test "$(git -C "$RENODE_ROOT" rev-parse HEAD)" = c38ac3babcd38a7144b9498e3ca4c2226b76aa37
work=target/stm32f407-exclusive
mkdir -p "$work"
(
    tests/cortex-m4-exclusive.sh "$work/elf" >"$work/exclusive-elf.log" 2>&1
    echo "$?" >"$work/exclusive-elf.status"
    cargo test >"$work/cargo-test.log" 2>&1
    echo "$?" >"$work/cargo-test.status"
    tests/stm32f407-freertos-smoke.sh "$work/freertos" --fpu >"$work/freertos.log" 2>&1
    echo "$?" >"$work/freertos.status"
) &
```

## 共有タイマーの周期境界

固定 Infrastructure の `src/Emulator/Main/Tests/UnitTests/LimitTimerTests.cs` 全 9 件を
移植しました。SysTick が使用する周期境界計算とイベント選択を、固定上流と同じ
周波数・上限・経過時間で検証します。上昇カウントは上限までの残距離から値を取得し、
方向切り替えでは切り替え時の値を保持します。1 MHz で 10 µs、10 MHz で 1 µs
進む各ケースで、期限到達 2 回と残り 2 クロックを確認します。
STM32 TIM は SysTick とクロック分数の繰り越しを共用し、TIM 固有の ARR と
更新・比較イベントの条件は各レジスターモデルに保持します。

```sh
export CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=gcc
export RENODE_ROOT="$(realpath ../renode)"
export PENV_F407_TIMED_RENODE_ROOT="$RENODE_ROOT"
work=target/stm32f407-limit-timer
mkdir -p "$work"
cargo test limit_timer
(
    cargo test >"$work/cargo-test.log" 2>&1
    echo "$?" >"$work/cargo-test.status"
    tests/stm32f407-freertos-smoke.sh "$work/freertos" --fpu >"$work/freertos.log" 2>&1
    echo "$?" >"$work/freertos.status"
) &
```

TIM2 の公開 ELF 回帰は `tests/stm32f407-upstream-smoke.sh` の TIM2 段を
個別に実行します。上流の SHA-1 に一致する上昇・下降ファームウェアを
各 100000 ステップ、未変更の CPU REPL で実行し、バナー、IRQ 有効化、開始、
3 回以上の周期コールバックの順序を照合します。ログは `tim2-up.log` と
`tim2-down.log` に保存しました。比較値変更と比較イベントを扱う共有
`ComparingTimerTests.cs` は別群に残っています。

## STM32 GPIO の静的接続

固定 Infrastructure の `src/Emulator/Main/Tests/UnitTests/GPIOTests.cs` 全 4 件を
Rust に移植しました。STM32 REPL のポート間配線も同じ接続登録を通ります。
受信側の識別子と入力ピン番号、未接続時の空リスト、5 入力の受信側のピン 2 を
確認しました。ピン 10 では上流と同じエラー文を検査します。モックオブジェクトの
参照同一性は、Rust のバス内で一意な受信側アドレスで照合します。

比較タイマー群は未達です。上流 `ComparingTimer` は上限超過の比較値を拒否し、
比較値や現在値を書いたときに期限を再計算します。STM32 TIM の CCR には ARR より
大きな値も書けるため、そのレジスター規則を維持した上で共有比較タイマーを扱う必要が
あります。GPIO の動的な切り離しも別群に残ります。

```sh
export CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=gcc
export RENODE_ROOT="$(realpath ../renode)"
export PENV_F407_TIMED_RENODE_ROOT="$RENODE_ROOT"
work=target/stm32f407-gpio-connections
mkdir -p "$work"
cargo test renode_gpio
(
    cargo test >"$work/cargo-test.log" 2>&1
    echo "$?" >"$work/cargo-test.status"
    tests/stm32f407-freertos-smoke.sh "$work/freertos" --fpu >"$work/freertos.log" 2>&1
    echo "$?" >"$work/freertos.status"
) &
```

配線の回帰には `tests/stm32f407-upstream-smoke.sh` の Quadrature Decoder 段を
使います。公開 ELF の SHA-1 を照合し、上流 REPL に PD12→PA6 と PD13→PA7 を
追加して 2100000 ステップ実行します。期待値は最初の 2 出力が 10±1 度と 20±1 度です。
この実行の UART と判定結果は `qdec.uart` と `qdec.log` にあります。

## Cortex-M4 の SystemBus 未登録アクセスとタグ

固定 Infrastructure の `SystemBusTests.cs` から、未登録アドレスと範囲タグに関する
8 件を移植しました。REPL の `sysbus: init:` で `UnhandledAccessBehaviour Report`、
`DoNotReport`、`ThrowException`、`ThrowExceptionIfNotTagged` を選べます。
上流の既定モードを検証するケースでは `Report` を明示します。既存プラットフォーム用に
設定省略時の Penv のアクセス規則を保持しています。

`Tag` の `throwException` はログ抑止時にも有効です。子タグで範囲を分割しても親タグの
両側に値と例外指定が残ります。上流の `AddressError` は Cortex-M の
`BusError::Unmapped` として検査します。その他のログモード・タグオプションは残件です。

```sh
export CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=gcc
export RENODE_ROOT="$(realpath ../renode)"
export PENV_F407_TIMED_RENODE_ROOT="$RENODE_ROOT"
work=target/stm32f407-unhandled-bus
mkdir -p "$work"
cargo test renode_sysbus
(
    tests/cortex-m4-sysbus.sh "$work/elf" >"$work/sysbus-elf.log" 2>&1
    echo "$?" >"$work/sysbus-elf.status"
    cargo test >"$work/cargo-test.log" 2>&1
    echo "$?" >"$work/cargo-test.status"
    tests/cortex-m4f-faults.sh "$work/faults" >"$work/fault-elf.log" 2>&1
    echo "$?" >"$work/fault-elf.status"
    tests/stm32f407-freertos-smoke.sh "$work/freertos" --fpu >"$work/freertos.log" 2>&1
    echo "$?" >"$work/freertos.status"
) &
```

追加 ELF は REPL を読み、未登録アドレスの 0、子タグの `0x42`、親タグで発生した
HardFault の `CFSR=0x8200` と `BFAR=0x1000` を検査して UART に `OK` を出します。
この ELF は台帳の分母に加えていません。

## SystemBus のマップ済みメモリー区間

固定 Infrastructure の `SystemBusTests.cs` からメモリー転送の 6 構成を移植しました。
2 アドレスに登録した 16 バイトの同一 RAM を使い、区間内、片端、両端、未登録区間への
書き込みを検査します。2 つの独立した RAM を使う `ShouldReadBytes` の
`true; true` は、350 バイトのうち登録区間だけに値が残り、隙間が 0 になることを
確認します。バイト周辺機器を含む残り 3 構成の結果は次節に記載します。

REPL は同一メモリーへの複数の `sysbus` 登録を受け付けます。`Report` または
`DoNotReport` を明示したスクリプトの `LoadBinary` では、対象データのうち
登録済みの RAM とバイト周辺機器へ書き込みます。追加 ELF は、ロード開始前半の
未登録区間が 0、後半の 4 バイトがエイリアスと主アドレスの双方で
`0x07060504` になることを検査し、UART に `OK` を出します。

```sh
export CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=gcc
export RENODE_ROOT="$(realpath ../renode)"
export PENV_F407_TIMED_RENODE_ROOT="$RENODE_ROOT"
work=target/stm32f407-mapped-bytes
mkdir -p "$work"
cargo test renode_mapped_bytes
(
    tests/cortex-m4-mapped-bytes.sh "$work/elf" >"$work/mapped-elf.log" 2>&1
    echo "$?" >"$work/mapped-elf.status"
    cargo test >"$work/cargo-test.log" 2>&1
    echo "$?" >"$work/cargo-test.status"
    tests/stm32f407-freertos-smoke.sh "$work/freertos" --fpu >"$work/freertos.log" 2>&1
    echo "$?" >"$work/freertos.status"
) &
```

## SystemBus の RAM・バイト周辺機器混在転送

固定 Infrastructure の `SystemBusTests.cs` の `ShouldReadBytes` で残っていた
`true; false`、`false; true`、`false; false` の 3 構成を移植しました。
C# のモックはバイト単位でアクセスする `Mocks.BytePeripheral` に対応させています。
バスは転送範囲を登録区間ごとに処理し、RAM へは区間転送し、バイト周辺機器へは
個別のバイト読み書きを行います。上流と同じ 50・200 番地の 100 バイト区間に対し、
350 バイトを入力し、全バイトを検査します。読出しの隙間は 0 です。
隣接領域を横断する検索は次節に記載します。

```sh
export CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=gcc
export RENODE_ROOT="$(realpath ../renode)"
export PENV_F407_TIMED_RENODE_ROOT="$RENODE_ROOT"
work=target/stm32f407-mixed-bytes
mkdir -p "$work"
cargo test renode_mapped_bytes
(
    tests/cortex-m4-mapped-bytes.sh "$work/elf" >"$work/mixed-elf.log" 2>&1
    echo "$?" >"$work/mixed-elf.status"
    cargo test >"$work/cargo-test.log" 2>&1
    echo "$?" >"$work/cargo-test.status"
    tests/stm32f407-freertos-smoke.sh "$work/freertos" --fpu >"$work/freertos.log" 2>&1
    echo "$?" >"$work/freertos.status"
) &
```

ELF スクリプトでは、既存のメモリーエイリアスに加え、RAM とバイト周辺機器の
混在 REPL を `LoadBinary` で初期化します。ELF が `LDRB` で 350 バイトすべてを
検査し、UART に `OK` を出力します。台帳は Rust の 3 構成を合格として数えます。

## SystemBus のバイトパターン検索

固定 Infrastructure `SystemBusTests.cs` の `ShouldFindBytePattern` を移植しました。
隣接する 1 MiB の RAM 2 区間に `00 01 02 03 04 05 06 07` を配置し、
アドレス `0x00000000` と `0x000ffffc` の 2 件だけが見つかることを確認します。
後者は RAM 登録境界と検索バッファー境界をまたぎます。未登録区間、バイト周辺機器、
フォールト注入用周辺機器は検索から除きます。エイリアスを含むアドレス順で結果を返します。

`penv run --script PATH --find-bytes HEX` はイメージをロードしてから検索し、
マッチしたアドレスを標準出力に 1 行ずつ出します。上流ケースの既定検索範囲に
対応します。検索開始・終了アドレスを指定する上流 API は別件に残ります。

```sh
export CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=gcc
export RENODE_ROOT="$(realpath ../renode)"
export PENV_F407_TIMED_RENODE_ROOT="$RENODE_ROOT"
work=target/stm32f407-find-bytes
mkdir -p "$work"
cargo test renode_find_byte_pattern
(
    tests/cortex-m4-find-bytes.sh "$work/cli" >"$work/find-cli.log" 2>&1
    echo "$?" >"$work/find-cli.status"
    tests/cortex-m4-mapped-bytes.sh "$work/elf" >"$work/mixed-elf.log" 2>&1
    echo "$?" >"$work/mixed-elf.status"
    cargo test >"$work/cargo-test.log" 2>&1
    echo "$?" >"$work/cargo-test.status"
    tests/stm32f407-freertos-smoke.sh "$work/freertos" --fpu >"$work/freertos.log" 2>&1
    echo "$?" >"$work/freertos.status"
) &
```

### SystemBus の周辺機器例外

固定 Infrastructure の `ShouldPropagateBusAccessExceptionFromPeripheral` を移植しました。
上流と同じくベース `0x1000`、サイズ `0x100` の周辺機器に対してオフセット 0 の
32 ビット読み出しを行い、周辺機器が発生させた `CommandError` と同じインスタンスが
呼び出し元に戻ることを `Arc::ptr_eq` で確認します。補足の Cortex-M4 ELF は
`CFSR=0x8200` と `BFAR=0x1000` を確認し、UART に `OK` を出します。

上記の未登録アクセス検証手順で `work=target/stm32f407-command-error` に設定すると
再現できます。フォールト検証の実行ログは `faults.log`、ほかのログは手順と同じ名前で、
各終了状態は対応する `.status` に記録しています。

### SystemBus の 32 ビット周辺機器登録

固定 Infrastructure の `ShouldFindAfterRegistration` と
`ShouldFindAfterManyRegistrations` を Cortex-M4 ELF に移植しました。
`Mocks.DoubleWordPeripheral` を既存の固定値 MMIO と重複検査に接続し、
上流と同じ十進アドレス、登録サイズ、読み出し順序、応答値を使います。
各 ELF は全比較が一致したときだけ UART に `K` を出し、スクリプトが完全一致を検査します。
上流で `Ignore` 指定の登録解除ケースは、動的な登録解除と上流の乱数フィクスチャが必要なため未達です。

```sh
export CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=gcc
export RENODE_ROOT="$(realpath ../renode)"
work=target/stm32f407-doubleword-registration
mkdir -p "$work"
(
    run() {
        local name=$1
        shift
        "$@" >"$work/$name.log" 2>&1
        echo "$?" >"$work/$name.status"
    }
    run sysbus-elf tests/cortex-m4-sysbus.sh "$work/elf"
    run cargo-test cargo test
    run freertos tests/stm32f407-freertos-smoke.sh "$work/freertos" --fpu
) &
```

`sysbus-elf.log` には単独登録と複数登録の `OK` が出ます。各 `.status` の期待値は 0 です。

### 上流プラットフォーム定義の読み込み

`platform_descriptions.robot` から Discovery の独立した REPL 3 パラメーターと、
文字列読み込み 3 件を移植しました。各入力は独立プロセスで、通常の起動と共通の
REPL ローダーに渡します。プロセス終了が上流の `Reset Emulation` に相当します。
空文字列の成功と、不正な `using` および抽象型を指定した場合の終了コード 1 と
診断文字列を検査します。

```sh
export CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=gcc
export RENODE_ROOT="$(realpath ../renode)"
work=target/stm32f407-platform-check
mkdir -p "$work"
(
    run() {
        local name=$1
        shift
        "$@" >"$work/$name.log" 2>&1
        echo "$?" >"$work/$name.status"
    }
    run build cargo build --release
    run platform tests/stm32f407-platform-check.sh
    run cargo-test cargo test
    run freertos tests/stm32f407-freertos-smoke.sh "$work/freertos" --fpu
) &
```

`platform.log` の期待値は REPL 3 件のロード成功、空文字列の成功、指定の診断 2 件です。
各 `.status` の期待値は 0 です。CAN の上流で無効化されたループバックケースは、
初期化モード外で CAN_BTR を書いて読み戻す期待値が同スイートの `InitTest` と矛盾し、
FINIT も解除しないため、元の条件を台帳に記録して未達に残しました。

### REPL の using エラー

固定上流の `UsingTests.cs` から、同一ソース内の重複、間接循環、直接循環、
存在しないファイルの 4 件を移植しました。上流のテスト用リゾルバーが作る
A と B を一時ファイルで再現し、元のソース文字列を `platform check --string`
に渡します。終了コード 1 に加え、上流と同じエラー種別
`DuplicateUsing`、`RecurringUsing`、`UsingFileNotFound` を検査します。

上記の REPL 読み込み手順で `work=target/stm32f407-repl-using` に設定し、
ビルド後に `run using tests/stm32f407-repl-using.sh` を追加すると再現できます。
`using.log` の期待値は重複 1 件、循環 2 件、欠落 1 件の診断です。
参照の可視性と宣言前の属性上書きは未達です。
初回の `cargo test` は循環エラーの従来文言を要求する CLI テストで失敗しました。
循環診断に従来文言を残して再実行した結果は `cargo-test.log`、初回結果は
`cargo-test-initial.log` に保存しています。

### REPL の Unix パス解決

固定上流の `UsingTests.cs` から、絶対パス、同一ディレクトリー、親ディレクトリー、
バックスラッシュをファイル名の文字として扱う 4 件を移植しました。
通常の REPL ローダーが使うリゾルバーに元と同じ引数・空の検索パスを渡し、
返るパスを完全一致で検査します。明示的なパスは字句的に解決します。
修飾のない名前の検索では、既存の同階層優先を保持します。

Windows 専用の 3 件は、上流の `RuntimeInfo.IsWindows()` ガードとテストが要求する
ドライブ・パス区切りを根拠に、Linux 対象の分母から外しました。

```sh
export CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=gcc
export RENODE_ROOT="$(realpath ../renode)"
work=target/stm32f407-using-paths
mkdir -p "$work"
(
    run() {
        local name=$1
        shift
        "$@" >"$work/$name.log" 2>&1
        echo "$?" >"$work/$name.status"
    }
    run cargo-test cargo test
    run freertos tests/stm32f407-freertos-smoke.sh "$work/freertos" --fpu
    run using tests/stm32f407-repl-using.sh
    run platform tests/stm32f407-platform-check.sh
) &
```

`cargo-test.log` に `renode_using_` で始まるケース 4 件と全体の合格が出ます。
各 `.status` の期待値は 0 です。

### REPL のコメントと文字列

固定上流の `PreLexerTests.cs` から 7 件を移植しました。通常の REPL ローダーが使う
コメント処理に元の入力を渡し、行コメント 4 件の出力を完全一致で検査します。
未終端文字列とブロックコメントの配置違反 2 件は構文エラーを検査します。
インデントから補う波括弧・セミコロンを含む全行変換とインデント診断は未達です。

直前の再現手順で `work=target/stm32f407-repl-comments` に変更すると、
関連回帰をまとめて実行できます。絞った検証は `cargo test renode_prelexer` です。
`cargo-test.log` の期待値は移植した 7 件の合格、各 `.status` の期待値は 0 です。
初回のコンパイルで検出されたファイル名の借用寿命は、重複検査がファイル名を所有するよう修正しました。
初回ログは `cargo-test-initial.log` に保存しています。

### REPL のインデント変換

固定上流の `PreLexerTests.cs` の残り 14 件を移植しました。
`penv platform prelex` は標準入力から REPL を読み込み、通常のローダーと共通の
コメント処理を通した後、インデントから波括弧・セミコロンを補ったテキストを返します。
空行、明示的な波括弧、文字列中の波括弧、複数行コメントの配置も検査しました。
先頭行の不正なインデントは `WrongIndent` を返します。

直前の再現手順で `work=target/stm32f407-repl-prelex` を使い、最初に
`run focused cargo test renode_prelexer` を実行します。`focused.log` の期待値は
このスイートの 21 件合格です。CLI 単体では
`printf 'parent\n    child' | target/release/penv platform prelex`
を実行すると、`parent{` に続いて `    child}` が出ます。
通常の REPL ローダーは既存の構文解析を使います。他の REPL パーサースイートは引き続き対象です。

### REPL の sysbus 範囲と十六進数

固定上流の `ParserPrimitivesTest.cs` から 8 パラメーターを移植しました。
sysbus の範囲登録で `<開始, 終端>` と `<開始, +長さ>` を受け付け、
十進数と十六進数の組み合わせを解釈します。逆順の境界は拒否します。
パーサーは 64 ビットの値を読み取り、STM32F407 の登録時には
開始と長さを既存の 32 ビット型に変換します。

```sh
export CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=gcc
export RENODE_ROOT="$(realpath ../renode)"
work=target/stm32f407-repl-ranges
mkdir -p "$work"
(
    run() {
        local name=$1
        shift
        "$@" >"$work/$name.log" 2>&1
        echo "$?" >"$work/$name.status"
    }
    run focused cargo test renode_ranges
    run cargo-test cargo test
    run freertos tests/stm32f407-freertos-smoke.sh "$work/freertos" --fpu
    run platform tests/stm32f407-platform-check.sh
) &
```

`focused.log` は 3 テスト関数で上流の 8 入力を検証します。
各 `.status` の期待値は 0 です。符号付き・小数の汎用トークン、IRQ リスト、
引用文字列のケースは未達です。

### REPL の IRQ 範囲とソースリスト

固定上流の `ParserPrimitivesTest.cs` から IRQ 構文 3 件を移植しました。
昇順・降順の範囲を指定順に展開し、番号と名前付きソースのリストを読み取ります。
既存の番号配線はこのパーサーを使います。名前付きソースを含む任意のリストの
デバイス別配線は今後の対応範囲です。

直前の範囲検証手順で `work=target/stm32f407-repl-irq` とし、絞った検証を
`run focused cargo test renode_irq_primitive` に置き換えると再現できます。
`focused.log` の期待値は 3 件合格で、各 `.status` の期待値は 0 です。

### REPL の数値トークンと単行引用文字列

固定上流の `ParserPrimitivesTest.cs` の残り 14 入力を移植しました。
符号付き十六進数、整数、小数のトークン出力、無効な数値の拒否、
および空白やセミコロンを含む引用文字列の復号を検証します。
数値トークン処理は既存のアドレス・長さの変換で使い、登録時には符号なし整数に
変換します。単行引用文字列は通常の `using` で使います。

直前の範囲検証手順で `work=target/stm32f407-repl-scalars` とし、最初に
`run focused cargo test renode_scalar`、最後に
`run using tests/stm32f407-repl-using.sh` を実行します。
`focused.log` の期待値は 4 テスト関数に含まれる上流 14 入力の合格です。
各 `.status` の期待値は 0 です。型付きプロパティへの汎用バインドと
複数行引用文字列は別の上流ケースとして未達です。

### REPL の複数行引用区切り

`platform prelex` が三重引用符を含む物理行をまとめ、エスケープされた区切りを
区別します。固定上流 `MergeAndCreationTests.cs` の元の入力 4 件で、
`SyntaxError` と `At 11:17:`、`At 12:9:`、`At 3:37:`、`At 3:17:` を検査します。
オブジェクト生成時の文字列の値、引用符の後の構文、`using` の構文は残課題です。

直前の範囲検証手順で `work=target/stm32f407-repl-multiline` に設定し、
`run focused cargo test renode_multiline` に置き換えます。
`focused.log` の期待値は 4 件合格で、各 `.status` の期待値は 0 です。

### REPL ローダーでの複数行文字列エラー

通常の REPL ローダーで、固定上流の複数行文字列エラーを 2 件移植しました。
`using` に複数行の引用を渡すと `SyntaxError` になり、プロパティの閉じ区切り後の
余分な文字列には `SyntaxError` と `At 4:23:` を返します。
成功系でのプレースホルダーへの値のバインドと、開始行の文字列内部にあるコメント記号の
扱いは未達です。

直前の手順で `work=target/stm32f407-multiline-loader` に設定して、
`run focused cargo test renode_multiline_loader` を実行します。
`focused.log` の期待値は 2 件合格です。加えて
`run using tests/stm32f407-repl-using.sh` を実行します。
各 `.status` の期待値は 0 です。

### REPL の複数行文字列の値

固定上流の成功ケース 7 件を移植しました。通常の REPL ローダーで引用内容を保護し、
複数行の値を読み取ります。実際の Python 周辺機器のスクリプトも共通の復号を経由します。
上流の `MockCPU` 相当のテスト用フィクスチャは、通常のパースとマージで得た
登録名・プロパティを使い、`sysbus.cpu` と `sysbus.cpu2` の文字列を完全一致で検証します。
物理行末のエスケープ 2 件の検証結果は次節に記録します。

範囲検証手順で `work=target/stm32f407-multiline-binding` とし、
`run focused cargo test renode_multiline` を最初に実行します。
`focused.log` の期待値は関連する 13 件合格、各 `.status` の期待値は 0 です。

### REPL の行末にあるエスケープ済み区切り

固定上流の残り 2 件を元の REPL と期待値で移植しました。上流の C# 入力は
逐語的文字列なのでバックスラッシュを含みます。期待値の通常の C# 文字列では
同じ表記がアポストロフィに復号されます。既存のローダーが三重引用符と改行を保持し、
復号後の値が一致することを検証しました。

直前の手順で `work=target/stm32f407-multiline-escaped-eol` として
`run focused cargo test renode_multiline` を実行します。`focused.log` の期待値は
関連 15 件の合格です。上流で無効化された引用ケースと他の生成ケースは未達です。

### REPL のプロパティ上書き順序

固定上流の単一ファイルのプロパティ更新 1 件と `using` の順序 5 件を移植しました。
通常のローダーは全入力から宣言先を把握し、取り込んだファイルとルートのブロックを
読み込み順に適用します。後のブロックが同名のスカラー値を上書きします。
上流の入力と同じインポートグラフを一時ファイルで再現し、最終値と保持される
別プロパティを検査します。登録先の差し替えとインポートをまたぐ参照の可視性は未達です。

既存の範囲検証手順で `work=target/stm32f407-repl-overrides` とし、最初に
`run focused cargo test renode_override`、最後に
`run using tests/stm32f407-repl-using.sh` を実行します。
期待値は絞った検証 6 件合格と各 `.status` の 0 です。

### REPL の登録先変更と解除

固定上流の登録先変更と `@none` による解除の 2 件を移植しました。
宣言のマージは後続ブロックの登録先を適用します。型付き `BusRangeRegistration` の
インライン波括弧も登録式として処理します。
上流の模擬機器に相当するテスト用フィクスチャで登録先の名前を照合し、
補足テストでは実際の SystemBus に移動先のみがマップされることを読み出しで確認します。
一般の型付き登録や機器生成時の依存順序は残課題です。

範囲検証手順で `work=target/stm32f407-repl-registration` とし、
最初に `run focused cargo test renode_registration` を実行します。
期待値は絞った検証 3 件合格と各 `.status` の 0 です。
初回は型付き範囲の波括弧が機器のプロパティと解釈されました。修正前のログは
`focused-initial.log` に保存しています。

### REPL の登録別名

固定上流の別名登録 1 件と不正な登録 2 件を移植しました。宣言の識別子を配線に、
別名を登録名と Peripheral Export に使います。登録なしと `@ none` を伴う別名は
それぞれ `AliasWithoutRegistration` と `AliasWithNoneRegistration` を返します。
名前付き親の依存順序と、インポートのスコープにまたがる参照は残課題です。

範囲検証手順で `work=target/stm32f407-repl-alias` とし、
最初に `run focused cargo test renode_alias` を実行します。
期待値は絞った検証 3 件合格と各 `.status` の 0 です。
