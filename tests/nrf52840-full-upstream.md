# nRF52840 上流全件台帳

Renode は `c38ac3babcd38a7144b9498e3ca4c2226b76aa37`、Infrastructure は `09ac7c563095c5410258ecc1b1a6b841a013db85`、tlib は `c04522a9c886ba743c77a86b0c879103a2ef46d0`。[ケースとソース行の索引](nrf52840-full-upstream-index.tsv) を併記する。共有ケースの調査には、固定版の Renode UI、dts2repl、models-analyzer も別ディレクトリへ取得した。Renode の作業ツリーとサブモジュールは読み取り専用で参照した。

`nrf52-ready` は従来の範囲の結果。全件ゲートは本台帳の対象ケースすべてについて上流等価の合格証拠が揃ってから作成する。

## 機種固有スイート

出典はすべて `tests/platforms/NRF52840.robot`。Skipped と non-critical のタグも対象に含む。

| 行・ケース | 上流入力またはパラメータ | 判定・現状 |
|---|---|---|
| 89 Should Run ZephyrRTOS Shell On UART | NO_DMA、legacy Shell ELF、`demo ping` | `uart:~$` から `pong`。既存回帰で確認 |
| 92 Should Run ZephyrRTOS Shell On UARTE | DMA、module Shell ELF、`demo ping` | 同じプロンプトと応答。既存回帰で確認 |
| 97 Should Run Alarm Sample | NO_DMA、`zephyr_alarm_nRF52840.elf` | `!!! Alarm !!!` の出力が仮想時刻の 2 秒台と 6 秒台。追加回帰 |
| 111 Should Handle LED and Button | GPIO0 11 の反転ボタン、13 の LED、Press/Release の各後に 1 秒 | 起動と操作案内の後に LED が on → off → on。追加回帰 |
| 130 Should Detect Passed Interrupt In WFE | DMA、custom WFE ELF | 起動後に `First WFE should finish`。既存回帰 |
| 138 Should Handle SPI | ADXL372 @ spi2、CS GPIO0 22。軸入力は (0,0,0) → (1,0,0) → (2,2,0) → (3,3,3) | 同一起動中に 0.00 → 1.00 → 2.83 → 5.20 g。追加の同一起動回帰で確認。従来の別起動比較は部分移植 |
| 168 Should Handle I2C | ADXL372 @ twi1 0x11、SPI ケースと同じ 4 組の軸入力 | 同一起動中の 4 出力。追加の同一起動回帰で確認。従来の別起動比較は部分移植 |
| 198 Should Echo I2S Audio | Nordic I²S master ELF、上流のステレオ PCM 32 バイト、仮想時刻 0.01 s | サイズと全バイト一致。既存回帰 |
| 221 Should Detect Yes Pattern | micro_speech ELF、`audio_yes_1s.s16le.pcm` | `Heard yes`。既存回帰 |
| 232 Should Run Zephyr's kernel.interrupt Test | Zephyr kernel.interrupt ELF | `PROJECT EXECUTION SUCCESSFUL`。既存回帰 |
| 240 Should Run Zephyr's cmsis_dps.transform.rq15 Test | CMSIS transform_rq15 ELF | `PROJECT EXECUTION SUCCESSFUL`。追加回帰 |
| 249 Should Restart After Watchdog Timeout | Zephyr WDT ELF、上流 monitor で CPU を 1 MIPS に設定 | 元の RESC コマンドで 1 MIPS を設定し、元の ELF の起動行を 2 回照合 |
| 262 Should Run Task Watchdog Subsystem | task_wdt ELF | 既定 100 MIPS と元の ELF。Robot と同じ 20 件の順次 UART 照合。各待機は仮想 8 秒 |
| 291 Should Run Bluetooth sample | HR central/peripheral 各 ELF、2 台、同じ RF 媒体、global quantum 10 µs | 起動、初期化、アドレス C0:00:AA:BB:CC:DD へ接続、購読、通知。10 µs で同期し、上流の11件を照合。2バイト通知を確認 |
| 332 Should Run Bluetooth Hci Uart Sample | `scripts/multi-node/nrf52840-ble-hci-uart-zephyr.resc` の 4 台。UART1 HCI ペア 2 本、BLE 媒体、controller 先行 1 s、quantum 10 µs | Zephyr 3.6.0 の両 host 起動・接続・購読・通知。元の 3 ELF を 4 台で起動し、UART1 で HCI フレーム、RADIO で接続・購読・通知を確認。CLI で同じ接続を構成し、10 µs で同期。上流の11件を照合 |
| 355 Should Return The Chip Id With I2C | MicroPython ELF、BME280 @ twi0 0x76、`readfrom_mem(0x76, 0xD0, 1)` | `ID: 0x60` と上流指定の 3 診断が出ないこと。追加回帰 |
| 372 Should Scan The I2C Bus Correctly | 同じ ELF、別起動の同じ REPL、`i.scan()` | `S: [118]`。追加回帰 |

## ボードを使う共有スイート

| 上流ファイル・ケース | 対象パラメータ・判定 | 現状 |
|---|---|---|
| `tests/peripherals/NRF52840_GPIO.robot` / Should Respect PullUp Configuration | GPIO0 IN、PIN_CNF[5]=0xC、ボタン接続と Press/Release。読み出し順は 0, 0x20, 0, 0x20, 0 | 同じ MMIO と動的ボタン接続を Rust のバステストへ移植 |
| `tests/peripherals/LIS2DS12.robot` / Should Read Acceleration | twi1 0x1c、IRQ GPIO0 28、入力 (10,5,-5) と出力 (9.997213,4.997410,-4.999803) | 元の ELF で確認 |
| `tests/peripherals/SPIMultiplexer.robot` / Should Talk to Two SPI Devices | spi2、CS GPIO0 22/23、dummy0 = 02 B0、dummy1 = FF FF FF FF FF 01 00 02 00 01 00 | 21.50 C と (0.008750,0.017500,0.008750)。元の ELF と 2 台分の応答キューで確認 |
| `tests/peripherals/ArduCAMMini2MPPlus.robot` / Should Detect Person | Nano 33 BLE、`person_image_0.jpg` | person−no person >100。元の ELF と JPEG で +131 |
| 同 / Should Detect Person | `person_image_1.jpg` | 同上。元の ELF と JPEG で +207 |
| 同 / Should Not Detect Person | `no_person_image_0.jpg` | person−no person <−100。元の ELF と JPEG で −170 |
| 同 / Should Not Detect Person | `no_person_image_1.jpg` | 同上。元の ELF と JPEG で −181 |
| `tests/peripherals/LSM330.robot` / Should Read Acceleration | twi0 0x1d、入力 (1,-1,2) | 生 UART 値 (16383,49153,32766)。元の ELF で確認 |
| 同 / Should Read Rotation | twi0 0x6a、入力 (100,-100,250) | 上流の UART 部分一致 (1300,52536,32500)。元の ELF で確認 |
| `tests/peripherals/LSM303.robot` / Should Read MagneticField | twi1 0x1e、入力 (20,5,-5) | 出力 (20.000000,5.000000,-5.000000)。元の ELF で確認 |
| 同 / Should Read Acceleration | twi1 0x19、IRQ GPIO0 26/27、入力 (12,4,-2) | 出力 (11.998728,3.993192,-2.001384)。元の ELF で確認 |
| `tests/peripherals/BMP180.robot` / Should Read Temperature | twi0 0x77、24 °C | `Temperature: 24.00 degC`。元の ELF で確認 |
| 同 / Should Read Pressure | twi0 0x77、未補償圧力 1100 | `Pressure: 231048.00 Pa`。元の ELF で確認 |
| `tests/peripherals/LSM9DS1.robot` / Should Detect RING Motion | Nano 33 BLE、`circle_rotated.data` | `Magic starts` → `RING:`。元の REPL・ELF・入力で確認 |
| 同 / Should Detect SLOPE Motion | Nano 33 BLE、`angle_rotated.data` | `Magic starts` → `SLOPE:`。元の REPL・ELF・入力で確認 |
| `tests/platforms/imxrt500.robot` / Should Run Pigweed Bluetooth Advertiser | mixed RESC の nRF52840 observer と i.MX RT500 beacon、HCI UART と BLE | nRF 側で RSSI −10 の広告受信。混成構成を対象に残す。未移植。上流 RESC は `emulation CreateUARTHub "hci_beacon"` で停止 |
| `tests/unit-tests/monitor-tests.robot` / Should Call GPIO Set | nRF52840 RESC、`uart0 IRQ Set true` | 元の RESC 読み込み後、同じ monitor コマンドで `GPIO: unset` → `GPIO: set` を確認 |

## 共有 CPU・周辺部品の適用性

- `tests/unit-tests/arm-dsp-examples-cortex-m4.robot` は Cortex-M4F の汎用フィクスチャのため対象。13 ケースで上流の UART 判定を確認。ELF と期待値は `nrf52840-dsp-upstream.sh` を参照。
- `tests/unit-tests/arm-cortex-m-fpu.robot` の `Should Pass Context Stacking Preservation Test v7-M` は元の ELF と fixture で `Test succeeded` を確認。FRDM-K64F 用 ELF の generic sharing は固定 NXP プラットフォーム、v8-M stacking は TrustZone を要求する。NOCP の FPU 有効側は Cortex-M4 で元の命令列から `OK` を確認。無効側はゲスト UsageFault と `NOCP` を確認。上流の UFSR 読み出しは Cortex-M4 の整列半語アクセスに移植した。
- `tests/unit-tests/arm-ge-flag.robot` は同じ Armv7E-M 命令列を Cortex-M4 で実行し、R4 の期待値 6 件を確認。usub8 という名前の行も、上流本文が実際に実行する ssub8 を使用。
- `tests/unit-tests/it-status.robot` は元の Cortex-M4 命令列と判定を Rust に移植し、条件コード、IT 状態とシフト、命令の条件実行、NMI 復帰、IT ブロック内フラグ更新の 6 ケースが合格。NMI は上流の monitor 注入と同じ例外番号を ICSR でペンドする。保存・復元と block begin/end hook の 3 ケースは未移植。
- `tests/unit-tests/arm-cortex-m-exception-return.robot` の非 TrustZone `bx lr` と通常分岐の 2 入力は、CPU が同じ PC に到達することを確認した。TrustZone の命令を指定するパラメータは対象外とした。
- `page-access-via-io.robot` は固定 Cortex-M4 のページ単位 I/O 切替。monitor 操作とアクセスログの回帰が未達。
- `python-engine.robot` の nRF RADIO 構文エラーと `external-control.robot` の nRF SPI 転送は Cortex-M4 へ移植できる。RISC-V カスタム命令による CPU abort は固定の別 ISA を必要とする。
- Infrastructure の `GPIOTests.cs` と `GPIODetachingTests.cs` は、汎用 GPIO と IRQ に適用できる固定 NUnit ケース。両 fixture の `[Test]` 全14件を登録し、`[SetUp]` は各ケースの準備として扱う。NUnit の等価な実行は未達。

## 固定構成の集計

監査結果は TSV の `scope` と `result` に記録した。 `in` は対象で、`pass` は上流等価の合格、`partial` は部分移植または最終モデルで等価性の証拠が不足、`unmet` は対象だが上流等価の合格証拠がない状態。 `out` は固定構成上の対象外で、`parameter` に別構成、`applicability` に該当する命令・レジスタ・ELF・周辺機器などの根拠を記録した。

| 対象範囲 | 固定実行ケース | 上流等価の合格 | 部分移植 | 未達 |
|---|---:|---:|---:|---:|
| `tests/platforms/NRF52840.robot` | 17 | 17 | 0 | 0 |
| 監査した共有・混成 Robot | 190 | 112 | 5 | 73 |
| Infrastructure GPIO NUnit | 14 | 0 | 0 | 14 |
| 合計 | **221** | **129** | **5** | **87** |

固定台帳には対象外の記録489行もある。対象外の MVE 命令単体スイートの一部は、同じ ISA 要件を持つパラメータ群として記録した。対象分母は固定の実行構成を数える。再起動・再生成される条件の組を展開し、同一実行の途中で繰り返すレジスタ確認や UART 行は元のケース内の判定として保持する。同じ上流ケース、同じ CPU・UART・バイナリの組は一つにまとめる。

- CMSIS-NN の127 URLは、`hello_world` と `micro_speech` の同 SHA-1 別名を統合すると125構成。Cortex-M85 DSP 用 ELF と合わせて137個の異なる ELF の SHA-1 と逆アセンブルを確認した。固定 ELF には MVE 命令、またはリセット時の `MSPLIM` / `PSPLIM` 設定がある。別ケースや別 UART 構成で使う同一 ELF は独立に扱う。
- semihosting の22 ELF は EABI 属性が Armv8.1-M でも、Cortex-M4 指定の逆アセンブルで未知命令が出なかった。固定の Thumb BKPT とホスト ABI のテストを対象に残した。実行による判定は台帳に記録する。
- Cortex-M55 の BusFault はTrustZone を用いない Armv7-M でも成立する例外のケースを対象に残した。Secure stack integrity、NVIC のバンク化状態、MVE といった追加機能を要求するパラメータは対象外にした。
- dts2repl の `Should Load Repls` は1件の `dynamic/unexpanded` 行として保持した。固定 gitlink は Dashboard の `LATEST_SIM`、`LATEST_SIM_RENODE`、`replkit.tar.xz`、`*.repl.diff` 一覧を固定しない。参照 CI 実行の不変アーティファクトが必要なため、件数は固定分母に含めていない。221件はこの台帳で確定した固定構成の分母。

## 再現

```sh
cargo test --locked
cargo build --release --locked
awk -F '\t' 'NR > 1 { count[$9]++ } END { for (result in count) print result, count[result] }' \
    tests/nrf52840-full-upstream-index.tsv
tests/nrf52840-acceptance.sh target/release/penv ../renode /tmp/nrf52-acceptance
tests/nrf52840-board-extra.sh target/release/penv ../renode /tmp/nrf52-board-extra
tests/nrf52840-dsp-upstream.sh target/release/penv ../renode /tmp/nrf52-dsp
tests/nrf52840-adxl-upstream.sh target/release/penv ../renode /tmp/nrf52-adxl
tests/nrf52840-fpu-upstream.sh target/release/penv ../renode /tmp/nrf52-fpu
tests/nrf52840-spi-mux-upstream.sh target/release/penv ../renode /tmp/nrf52-spi-mux
tests/nrf52840-lis2ds12-upstream.sh target/release/penv ../renode /tmp/nrf52-lis
tests/nrf52840-sensors-upstream.sh target/release/penv ../renode /tmp/nrf52-sensors
tests/nrf52840-ble-pair.sh target/release/penv ../renode CENTRAL_HR_ELF PERIPHERAL_HR_ELF
tests/nrf52840-hci-upstream.sh target/release/penv ../renode /tmp/nrf52-hci
tests/nrf52840-nano33-upstream.sh target/release/penv ../renode /tmp/nrf52-nano33
tests/nrf52840-monitor-upstream.sh target/release/penv ../renode /tmp/nrf52-monitor
tests/nrf52840-watchdogs-upstream.sh target/release/penv ../renode /tmp/nrf52-watchdogs
```

追加テストの ELF は公開ファイルの SHA-1 を照合する。Alarm の `--uart-steps` は仮想ステップを表示し、100,000,000 ステップを仮想 1 秒として判定する。全件ランナーと全件合格ログは未完成。監査時は Zephyr UARTE Shell のリセット前後の応答と、Q15 の `PROJECT EXECUTION SUCCESSFUL` を元の ELF で再確認した。Q15 は `nrf52840-board-extra.sh` と同じ 45,000,000 ステップ上限を使用した。

Nano 33 BLE は未変更の上流 REPL を使用する。ArduCAM の 4 画像は最後のスコア行と差分の閾値、LSM9DS1 の 2 入力は起動後の認識行が必要。スコアや認識行を欠く実行はランナーが失敗として報告する。上表の画像スコアは SPI の CS 判定を追加する前のログに基づく。追加後は `person_image_0.jpg` でキャプチャからデコードまで確認し、4 画像のスコア再確認は残る。

## Watchdog の時刻と UART 判定

`PerformanceInMips` は単一 nRF52840 ノードの RESC ブート設定で指定する。既定は 100 MIPS。1 MIPS では CPU の 1 実行スロットが仮想 1 µs となり、周辺回路には同じ仮想時間を進める。Watchdog は周辺の 32.768 kHz 低周波クロックで進む。非既定の速度を複数ノードで使う構成は未対応。

固定上流の `UartKeywords.cs` と `TerminalTester.cs` は、既定で次の一致行までを消費する。UART の探索は再起動後の行にも続く。Renode 1.17.0 で固定チェックアウトの Robot 本文を実行したところ、両ケースが通った。Task Watchdog の実ログでは `Control thread getting stuck...` の直後にコールバックとリセットが出る。その後の起動で次の生存ログを照合し、さらに続くコールバックを照合する。回帰ではファームウェアの実出力をこの順序で照合する。

`nrf52840-watchdogs-upstream.sh` は上流 ELF の SHA-1 を照合し、各マーカーを前回一致から仮想 8 秒以内に探す。改行済みの UART 行を使う。時刻は penv の `--uart-steps` が付ける各行の出力開始スロットを秒に換算する。最後に一致した行で停止し、UART と一致時刻を保存する。

## BLE の量子と UART 観測

両ケースで、既定の Cortex-M クロックの 1,000 スロットを仮想 10 µs の同期量子として使う。各ノードの RADIO はローカルのクロックで進み、別ノードへの RF フレームと HCI UART の受信は同期点で反映される。4台構成ではコントローラを仮想 1 秒先行させてからホストを実行する。

BLE 実行時の `--uart-steps` は、改行済みの UART 行を観測した同期点を全ノード共通のステップで表示する。UART0 のテスターごとに行を保持し、Robot 本文と同じ順序で11件を照合する。一致行までをそのテスターで消費し、もう一方のテスターの行を保持する。各待機の上限は仮想 8 秒。心拍ケースでは従来の2バイト通知の確認も行う。心拍サンプルの ELF は `nrf52840-acceptance.sh` に記載の SHA-1 で照合できる。

HCI の回帰は上流 RESC と同じ4台・2本の HCI 接続と無線媒体を CLI から構成する。任意の複数マシン RESC 構文と物理 RF は別の対応範囲。

固定 ELF で両ケースの11件が合格した。心拍ケースの中央通知行は仮想 1.02091 秒、周辺側の HRS 確認後に照合が完了したのは 1.62088 秒。HCI UART の最後の通知は仮想 2.37966 秒に照合した。Renode 1.17.0 でも固定チェックアウトの Robot ケース本文を走らせ、両ケースが通過した。心拍 peripheral の取得 URL は同じ SHA-1 のハイフン表記を用いた。

## Cortex-M4 NOCP と例外フレーム

`nrf52840-fpu-upstream.sh` は上流の Armv7-M スタック確認 ELF の SHA-1 を照合し、`Test succeeded` を確認する。同じランナーで固定 `arm-cortex-m-fpu.robot` の `NOCP_ASSEMBLY` を Cortex-M4 向け ELF に組み立て、`NOCP` を確認する。上流の `0xe000ed2a` への非整列ワード `LDR` は、同じ UFSR と NOCP ビットを読む整列半語 `LDRH` に置換する。試験には Python 3、clang、ld.lld が必要。

CPACR で拒否された命令を実行すると、許可された UsageFault または昇格先の HardFault に入り、CFSR.NOCP と該当時の HFSR.FORCED を記録する。Cortex-M4 のテストは、故障命令の PC、MSP/PSP と EXC_RETURN、スタックの整列、復帰時の ITSTATE と命令再試行を確認する。FP コンテキストがアクティブなら、CPACR を閉じた後の例外でも拡張フレームから S レジスタと FPSCR を復元する。

lazy FP 保存、スタックまたはベクタアクセス自体の BusFault、lockup 状態の継続実行は残課題。NOCP が HardFault に昇格できず lockup に達した場合は診断を返す。

## Cortex-M4 の同期データ BusFault

固定 Renode の `tests/unit-tests/arm-cortex-m-bus-fault.robot` から5ケースを移植した。単一命令の3ケースは元の Thumb 命令、アドレス、初期値と SCB の判定を使用する。実 ELF の2ケースは元の E2E アセンブリを抽出して Cortex-M4 向けにリンクし、ハンドラによる R11 保存後に上流の R10/R11、CFSR、BFAR の判定を実行する。UART の `PASS` は判定がすべて成立した場合に出力する。

```sh
cargo test
tests/nrf52840-bus-fault-upstream.sh "$PWD/target/debug/penv" ../renode "$ARTIFACT_DIR/bus-fault"
tests/nrf52840-fpu-upstream.sh "$PWD/target/debug/penv" ../renode "$ARTIFACT_DIR/fpu"
tests/nrf52840-shell.sh "$PWD/target/debug/penv" ../renode "$SHELL_ELF"
```

今回のログと終了状態は `~/.local/state/penv-dispatch/evidence/nrf52/bus-fault.WbA98rJg/` の `cargo-test`、`elf`、`fpu`、`shell` に保存した。Shell は固定 `NRF52840.robot` の Zephyr Shell ELF を使い、リセット前後で応答を確認した。

同スイートの残課題は下記の FP 転送の回帰に続けて記録した。例外入口・復帰時のバス障害、lockup と NMI、遅延 FP 保存、モックのオフセット指定と FP フレームの検証、連続 fault に対する monitor step 境界が残る。

## Cortex-M4 semihosting の出力と終了

`tests/unit-tests/arm-semihosting.robot` の9構成が合格した。簡易呼び出しは固定の命令列と引数アドレスを ELF に移し、出力 `SUCCESS` と R0 = 0 を照合した。残る8構成では SHA-1 を照合した上流 ELF を使用した。同じ WRITE-TTY ELF を共用出力と分離出力の両構成で実行し、分離時は `SUCCESS` と `ERROR` がそれぞれの出力先にだけ現れることを確認した。終了理由と拡張終了コードは上流のログ文言で照合した。移植用 REPL は Cortex-M4 を選び、使用されるアドレスを維持して RAM の割当範囲を縮めた。分離出力では CLI の標準エラーを第2 UART の観測先として使う。

```sh
cargo test
tests/nrf52840-semihosting-upstream.sh "$PWD/target/debug/penv" ../renode "$ARTIFACT_DIR/semihosting" "$ELF_CACHE"
tests/nrf52840-bus-fault-upstream.sh "$PWD/target/debug/penv" ../renode "$ARTIFACT_DIR/bus-fault"
tests/nrf52840-shell.sh "$PWD/target/debug/penv" ../renode "$SHELL_ELF"
```

ログと終了状態は `~/.local/state/penv-dispatch/evidence/nrf52/semihosting.v5bABUKm/` の `cargo-test`、`elf`、`bus-fault`、`shell` に保存した。各最終実行の終了状態は `*.final.status` に記録した。

同スイートの残課題は下記の入力と引数の回帰に続けて記録した。

### semihosting の入力と起動引数

上記の `nrf52840-semihosting-upstream.sh` に5構成を追加した。READ-TTY と READC は上流 TerminalTester の `WriteLine` と同じ `hello, world\r` を、既存の `--uart-input` から供給する。GET-CMDLINE と ARGV は RESC の `cpu.semihosting ProgramArguments "hello world"` を使う。NO-ARGV は上流の既定プログラム名 `program-name` を使う。各固定 ELF の SHA-1 と `ADP_Stopped_ApplicationExit(0x20026)`、戻りコード 0 を照合した。上記と同じコマンドで再現できる。

今回の `cargo-test`、`elf`、`shell` のログと終了状態は `~/.local/state/penv-dispatch/evidence/nrf52/semihosting-input.SGkGZfKh/` に保存した。

同スイートの残る11構成は下記のファイル操作の回帰で扱う。

### semihosting のファイル操作と一時ディレクトリ

上記のスクリプトに固定 ELF の11構成を追加した。各実行は個別の作業ディレクトリを RESC の `cpu.semihosting WorkingDirectory` で指定する。OPEN、CLOSE、ERRNO、REMOVE、READ、SEEK、FLEN、ISTTY、ISERROR、TMPNAM の既定と指定ディレクトリの各構成で、上流の終了理由とコード 0 を照合した。指定構成では `TemporaryFilesDirectory "my-temp"` と、上流と同じ `/semi/my-temp` の作成ログも照合した。上記と同じコマンドで再現できる。

ホスト側で明示的に作成した作業ディレクトリを使う。ゲストが指定できるのは、直下の通常ファイルと semihosting の一時ディレクトリ直下の通常ファイル。生成する一時ディレクトリ名の接尾辞はホスト側で割り当てる。上流の固定ケースは接尾辞を判定しない。

今回のログと終了状態は `~/.local/state/penv-dispatch/evidence/nrf52/semihosting-files.H1B886nQ/` の `cargo-test.log`、`elf.log`、`shell.log` と対応する `cargo-test.final.status`、`elf.final.status`、`shell.status` に保存した。semihosting の対象25構成はすべて合格した。台帳全体の残数は固定構成の集計を参照。

### FP 転送の同期データ BusFault

`arm-cortex-m-bus-fault.robot:1360–1377` の Cortex-M4 に適用できる VSTR/VLDR の S0・D0 の4構成を移植した。固定の命令、フォールトアドレス、`FaultOffset = 0`、R1/R2 の初期値を保持した。単体テストと生成 ELF で、BusFault ハンドラ、SP = `0xf98`、保存された R1/R2 と PC、CFSR・BFAR・HFSR を照合した。上流の monitor によるオフセット指定は、検証 ELF の REPL で同じ値を設定した。

上記の `cargo test`、`nrf52840-bus-fault-upstream.sh`、`nrf52840-fpu-upstream.sh`、`nrf52840-shell.sh` で再現できる。今回のログと対応する終了状態は `~/.local/state/penv-dispatch/evidence/nrf52/fp-bus-fault.WoqOVB6v/` の `cargo-test`、`elf`、`fpu`、`shell` に保存した。

BusFault の適用対象31ケースのうち30件が合格、1件が部分移植、未達0件。例外入口・復帰時のバス障害、lockup と NMI、遅延 FP 保存、連続 fault に対する monitor step 境界などの阻害点は TSV の `verification` 欄を参照。

### 未マップのデータアクセス

`arm-cortex-m-bus-fault.robot:336–343` を移植した。`nrf52840-bus-fault-upstream.sh` の `unmapped.elf` は、元の命令列とアドレス `0x200000` を使い、RESC で `sysbus UnhandledAccessBehaviour ThrowException` を指定する。ハンドラ PC、SP、R1/R2、その保存値、保存 PC、CFSR・BFAR・HFSR を照合する。同じ ELF を設定なしでも実行し、従来の未マップエラーを確認する。今回のコマンド対応は Cortex-M4 の未マップデータアクセスを対象とする。命令フェッチの例外化と、RENODE のタグに対するポリシーは別途対応が必要。

上記の `cargo test`、`nrf52840-bus-fault-upstream.sh`、`nrf52840-shell.sh` で再現できる。今回のログと終了状態は `~/.local/state/penv-dispatch/evidence/nrf52/unmapped-bus-fault.105kBUr7/` の `cargo-test`、`elf`、`shell` に保存した。

### 例外ハンドラ内の同期データ BusFault

`arm-cortex-m-bus-fault.robot:1212–1228` に合わせ、`CCR.BFHFNMIGN` を設定した場合、HardFault/NMI 内のデータ BusFault は SCB に記録して命令実行を継続する。生成した `ignored-in-hardfault.elf` で、Thread モードから HardFault に昇格した際の R1 保存値、ハンドラの再アクセス後の R1 と CFSR・BFAR・HFSR を照合した。BFHFNMIGN の適用範囲はデータアクセスであり、命令フェッチ・例外スタック処理は別経路。

同スイートの352–368行は、`nested-handler.elf` で同優先度の BusFault から HardFault へ昇格すること、保存 PC、R1/R2 と SCB の期待値を照合した。上流 monitor の1回の `cpu Step` で二つの fault を観測する条件は未検証のため、部分移植として記録した。

上記の `cargo test`、`nrf52840-bus-fault-upstream.sh`、`nrf52840-shell.sh` で再現できる。ログと終了状態は `~/.local/state/penv-dispatch/evidence/nrf52/handler-bus-fault.9Ii3pGU8/` の `cargo-test`、`elf`、`shell` に保存した。

### 命令実行中の BusFault lockup

`arm-cortex-m-bus-fault.robot:369–423` の2構成を移植した。固定の命令列を実行して、再フォールト後の PC、R1/R2、CFSR・BFAR・HFSR・SHCSR、CPU/NVIC の lockup 状態と非休止状態を確認した。続く10ステップで命令が実行されず ITSTATE が保持されること、CPU リセットで lockup が解除されることを確認した。上流 monitor の NMI 注入は ICSR の NMI ペンドで移植し、NMI 入口と保存 PC、lockup の解除を照合した。`lockup.elf` でも CLI 実行時に PC が `0xEFFFFFFE` に留まることを確認した。

上記の `cargo test`、`nrf52840-bus-fault-upstream.sh`、`nrf52840-shell.sh` で再現できる。ログと終了状態は `~/.local/state/penv-dispatch/evidence/nrf52/bus-lockup.AF3ru2Uw/` の `cargo-test.final`、`elf`、`shell` に保存した。リセット時のベクタアクセスと DHCSR の検証は次節に記録した。

### リセットベクタと初期 MSP の BusFault

`arm-cortex-m-bus-fault.robot:1042–1073` の2構成を移植した。ベクタテーブルを `0x100000` に置き、上流と同じオフセット 4 または 0 だけでフォールトさせた。CPU/NVIC の lockup、PC・SP・LR・IPSR、CFSR・HFSR・SHCSR、DHCSR の CPU/デバッガ別の可視性と書き込み無視を照合した。上流の `RunFor` は実行予算を与えてから固定状態を確認するテストに移植し、予算中に命令が実行されないことも検証した。CLI は同じ起動条件を REPL に指定して実行した。

上記の `cargo test`、`nrf52840-bus-fault-upstream.sh`、`nrf52840-shell.sh` で再現できる。ログと終了状態は `~/.local/state/penv-dispatch/evidence/nrf52/reset-vector-fault.w13sYiUd/` の `cargo-test.final`、`elf.final`、`shell` に保存した。例外入口・復帰時のバス障害と不正な初期 Thumb 状態は引き続き未達。

### 例外入口のベクタ BusFault

`arm-cortex-m-bus-fault.robot:601–628, 685–709` を Rust の実行回帰に移植した。元の Thumb 命令、VTOR、モックのフォールトアドレスを使用する。外部 IRQ は NVIC の ISPR、NMI は ICSR のペンドビットで注入した。外部 IRQ のケースは HardFault 入口・単一フレーム・元の IRQ の保留と非アクティブ状態を照合した。NMI のケースは lockup・IPSR・SCB・DHCSR と HFSR の書き込みクリアを照合した。EXC_RETURN の LR は Cortex-M4 の Thread/MSP 対応値 `0xFFFFFFF9` を使う。

`cargo test` と上記の BusFault ELF・Zephyr Shell スクリプトで再現できる。ログと終了状態は `~/.local/state/penv-dispatch/evidence/nrf52/exception-vectors.hSLLTaY9/` の `cargo-test.final`、`elf`、`shell` に保存した。HardFault の直接ペンドは次節に記録した。スタックアクセスとベクタアクセスが同時に失敗した場合の優先順位も未達。

### 直接ペンドした HardFault のベクタ BusFault

`arm-cortex-m-bus-fault.robot:710–732` を移植した。上流と同じ Thumb 命令、VTOR とモックを使い、上流の `nvic SetPendingIRQ 3` と同じ NVIC 経路で HardFault をペンドした。次のステップで lockup、単一の例外フレーム、Armv7-M の Thread/MSP EXC_RETURN、IPSR・ITSTATE、CFSR・HFSR・SHCSR、CPU とデバッガの DHCSR を照合した。RESC コマンド自体は別の読み出し可能なベクタを持つ ELF で確認した。

`cargo test` と上記の BusFault ELF・Zephyr Shell スクリプトで再現できる。ログと終了状態は `~/.local/state/penv-dispatch/evidence/nrf52/pending-hardfault.VonZkMGx/` の `cargo-test`、`elf`、`shell` に保存した。例外スタックとベクタの同時障害、例外復帰時の障害、遅延 FP 保存は未達。

### 例外フレーム保存時の BusFault

`arm-cortex-m-bus-fault.robot:424–491` の3構成を移植した。上流と同じ SP と命令列を使い、NMI 入口で HardFault が保留されること、外部 IRQ 入口で HardFault または同優先度の BusFault が選ばれることを照合した。外部 IRQ は保留を維持し、アクティブにはならない。生成 ELF の `stacking-hardfault` と `stacking-busfault` でも、スタックの割当位置、SCB、IRQ の状態を確認した。

`cargo test` と上記の BusFault ELF・Zephyr Shell スクリプトで再現できる。ログと終了状態は `~/.local/state/penv-dispatch/evidence/nrf52/stacking-bus-fault.a5Oiy5sV/` の `cargo-test.verified`、`elf.verified`、`shell` に保存した。同時に発生するスタックとベクタの障害の優先順位、選択されなかった設定可能 BusFault の保留、例外復帰時の障害、遅延 FP 保存は残る。

### HardFault を中断する NMI のスタック障害

`arm-cortex-m-bus-fault.robot:544–600` の2ケースを移植した。元の命令、SP、NMI の注入とベクタ位置を再現し、スタック障害後の SP、NMI の IPSR、ITSTATE、HardFault/NMI のアクティブ状態、CPU とデバッガの DHCSR を照合した。スタック障害と NMI ベクタ障害が重なる構成では、`STKERR` と `VECTTBL` の両方を記録する。追加 ELF は HardFault ハンドラから NMI をペンドし、CLI でも lockup に留まることを確認する。

`cargo test` と上記の BusFault ELF・Zephyr Shell スクリプトで再現できる。ログと終了状態は `~/.local/state/penv-dispatch/evidence/nrf52/nmi-stacking-lockup.YyxdHtTN/` の `cargo-test`、`elf`、`shell` に保存した。今回の同時障害対応は NMI 入口の選択済みケースを対象とする。例外復帰時のスタック障害と遅延 FP 保存は未達。

### 例外復帰時のスタック読み出し BusFault

`arm-cortex-m-bus-fault.robot:826–908` の3ケースを移植した。NMI の復帰先が HardFault なら、読み出しに失敗したフレームの分だけ SP を進めて lockup に入る。BusFault にテールチェーンする2ケースでは、既存のフレームと EXC_RETURN を保持し、`UNSTKERR` と FPCA の解除を確認した。生成 ELF の `unstack-tail` と `unstack-tail-fp` では、上流 monitor の `FaultOnWrites false` と同じ値を REPL に指定する。Cortex-M4 の LR は Thread/MSP の対応値を使った。

`cargo test` と上記の BusFault ELF・FPU・Zephyr Shell スクリプトで再現できる。ログと終了状態は `~/.local/state/penv-dispatch/evidence/nrf52/unstack-bus-fault.FJjkkUW4/` の `cargo-test.final`、`elf`、`fpu`、`shell` に保存した。遅延 FP 保存の障害、不正なリセット状態、復帰フレームの整合性エラーが残る。

### 例外復帰時の RETPSR 整合性エラー

`arm-cortex-m-bus-fault.robot:1015–1041` のケースを移植した。HardFault を中断した NMI で保存 xPSR をゼロにする上流の命令列を使い、`INVPC`、フレーム消費後の SP、HardFault の IPSR とアクティブ状態、HFSR、CPU とデバッガの DHCSR を照合した。追加の `retpsr.elf` は HardFault ハンドラから NMI をペンドし、CLI の lockup 到達を確認する。

`cargo test` と上記の BusFault ELF・FPU・Zephyr Shell スクリプトで再現できる。ログと終了状態は `~/.local/state/penv-dispatch/evidence/nrf52/retpsr.oFDlA7Ct/` の `cargo-test.final`、`elf`、`fpu`、`shell` に保存した。遅延 FP 保存の2ケースが未達。初期 Thumb 状態は次節に記録した。連続 BusFault の monitor ステップ境界は部分移植。

### 不正な初期 Thumb 状態の INVSTATE

`arm-cortex-m-bus-fault.robot:311–321` を移植した。ゼロ化したベクタテーブルで初期 T ビットを保持し、1ステップ目の HardFault 入口と、2ステップ目の lockup を個別に確認した。CFSR.INVSTATE、HFSR.FORCED、CPU とデバッガでの DHCSR の違いを照合した。MSP が 0 のこの起動条件では、上流の既定アクセス方針に合わせ、未マップの例外スタック書き込みを破棄する。Cortex-M0 の既存の不正ベクタ診断も回帰確認した。

`cargo test` と上記の BusFault ELF・FPU・Zephyr Shell スクリプトで再現できる。ログと終了状態は `~/.local/state/penv-dispatch/evidence/nrf52/invalid-thumb.gvVTxj2I/` の `cargo-test.final`、`elf`、`fpu`、`shell` に保存した。BusFault スイートでは遅延 FP 保存の2ケースと、連続 fault の monitor ステップ境界の部分移植が残る。

### 遅延 FP 保存時の BusFault

`arm-cortex-m-bus-fault.robot:733–806` の2ケースを移植した。固定命令とアドレスを使い、FPCAR と予約時の HFRDY、保存時の LSPERR、HardFault の保留または lockup、LSPACT と SCB を照合した。Thread 由来のケースは生成 ELF でも検証した。既存の即時 FP 保存テストは LSPEN を明示的に無効化して実行する。

`cargo test` と上記の BusFault ELF・FPU・Zephyr Shell スクリプトで再現できる。ログと終了状態は `~/.local/state/penv-dispatch/evidence/nrf52/lazy-fp.cjsLEghf/` の `cargo-test.verified`、`elf`、`fpu`、`shell` に保存した。BusFault スイートには連続 fault の monitor ステップ境界の部分移植が残る。今回の遅延保存障害処理は、BusFault が無効な NMI 内の障害を対象とする。

### Thumb オペコードカウンタ

`opcodes-counting.robot:179–213` の2ケースを移植した。上流と同じアドレス 0 の命令列、正負のパターン、1 ステップ後の PC とカウントを Rust の実行回帰で照合した。RESC/ELF 回帰ではリセットベクタを置くため命令列を `0x100` に配置し、`InstallOpcodeCounterPattern`、`EnableOpcodesCounting`、実行後の `GetOpcodeCounter` を確認する。現行 RESC は起動設定と実行後の照会に対応する。途中で実行を止めて設定や照会を行う monitor セッションは範囲外。

```sh
cargo test
tests/nrf52840-opcode-count-upstream.sh "$PWD/target/debug/penv" "$ARTIFACT_DIR/opcode-count"
tests/nrf52840-shell.sh "$PWD/target/debug/penv" ../renode "$SHELL_ELF"
```

ログと終了状態は `~/.local/state/penv-dispatch/evidence/nrf52/thumb-opcode-count.01Z0HhOi/` の `cargo-test.final`、`elf`、`shell` に保存した。オペコード実行フックは次節に記録した。BusFault の連続 fault に対する monitor ステップ境界は残る。

### Thumb オペコード実行フック

`opcodes-execution-hooks.robot:36–66` の2ケースを移植した。Rust 回帰は上流と同じアドレス、命令、マスク、正負の登録パターンを使い、1ステップ後の PC と、実行前後の完全なログを照合する。上流の Python コールバックのログ処理は `AddPreOpcodeExecutionLogHook` と `AddPostOpcodeExecutionLogHook` で再現した。任意の Python コールバックは未実装。ELF 回帰は前節のスクリプトを使い、同じログをリセットベクタを避けた `0x100` で確認した。

前節のコマンドと `nrf52840-bus-fault-upstream.sh` で再現できる。ログと終了状態は `~/.local/state/penv-dispatch/evidence/nrf52/thumb-opcode-hooks.fhGwoVJf/` の `cargo-test.final`、`elf`、`shell`、`bus-fault` に保存した。

### 例外復帰値への通常分岐

`arm-cortex-m-exception-return.robot:129–149` の Cortex-M4 に適用できる未達2ケースを移植した。Thread の `BX` は固定オペランド `0xFFFFFFB8` に分岐し、次の命令境界で扱う無効 Thumb 状態を保持する。NMI の `BLX LR` は通常分岐として扱う。Cortex-M4 の Thread/MSP 用 LR `0xFFFFFFF9` に対応する PC `0xFFFFFFF8` と、NMI がアクティブなままであることを照合した。TrustZone 構成の判定は既存の対象外根拠を参照。

`cargo test`、上記の `nrf52840-bus-fault-upstream.sh` と `nrf52840-shell.sh` で再現できる。ログ・終了状態は `~/.local/state/penv-dispatch/evidence/nrf52/exception-branch.0gAH2ip8/` の `cargo-test.final`、`elf`、`shell` に保存した。BusFault の連続フォールトに対する monitor ステップ境界は引き続き部分移植。

### LLVM MC の Thumb アセンブルと逆アセンブル

`llvm-asm-disasm.robot:317–324, 522–534, 543–547` の3ケースを移植した。Cortex-M4 固定ケースでは上流の4命令のバイト列、アドレス、逆アセンブルの表記を照合する。複数行とディレクティブのケースは、元の行数と反復数を維持して Thumb NOP に置き換えた。上流 monitor のゲストメモリ経由のバイト列受け渡しは、CLI の16進バイト列で行う。ホストの `llvm-mc` が必要。逆アセンブルは1命令を対象とし、アセンブルは解決済みの命令エンコーディングを返す。

```sh
cargo test
tests/nrf52840-thumb-llvm-upstream.sh "$PWD/target/debug/penv"
tests/nrf52840-shell.sh "$PWD/target/debug/penv" ../renode "$SHELL_ELF"
```

ログと終了状態は `~/.local/state/penv-dispatch/evidence/nrf52/thumb-llvm.01QfxuUa/` の `cargo-test.final`、`llvm`、`shell` に保存した。ゲストメモリからの逆アセンブルは次節に記録した。

### ゲストメモリからの Thumb 逆アセンブル

`llvm-asm-disasm.robot:501–515` の共有2ケースを Cortex-M4 に移植した。`thumb-disassemble-memory REPL ELF ADDRESS SIZE` はファームウェアをロードし、指定アドレスのメモリを読んで1命令を逆アセンブルする。上流の有効アドレス `0x1234` と範囲外アドレス `0x02000000` で、Thumb 命令のデコードと範囲外診断を照合した。上流の RISC-V の不正入力2種は、Thumb の不正な16ビット・32ビットエンコーディングに移植し、各入力で診断、非ゼロ終了状態と空の標準出力を確認した。

```sh
cargo test
tests/nrf52840-thumb-llvm-errors-upstream.sh "$PWD/target/debug/penv" "$ARTIFACT_DIR/thumb-llvm-errors"
tests/nrf52840-thumb-llvm-upstream.sh "$PWD/target/debug/penv"
tests/nrf52840-shell.sh "$PWD/target/debug/penv" ../renode "$SHELL_ELF"
```

ログと終了状態は `~/.local/state/penv-dispatch/evidence/nrf52/thumb-llvm-errors.chK1LR2d/` の `cargo-test`、`llvm-errors`、`llvm`、`shell` に保存した。アセンブルの診断は次節に記録した。

### Thumb アセンブルエラーの診断

`llvm-asm-disasm.robot:535–542` の共有ケースのうち Cortex-M4 に適用できる2入力を移植した。不明命令は上流の `illegalinstruction123`、不正オペランドは Thumb の `str r0, [sp, #65536]` を使用する。LLVM MC の詳細診断、終了コード 1、空の標準出力を照合した。正常命令が不正命令より前にある複数行ソースでも、前半のバイト列が出力されないことを検証した。RISC-V V 拡張の診断は固定の別 ISA 構成であり、既存の対象外に該当する。

前節と同じコマンドで再現できる。ログと終了状態は `~/.local/state/penv-dispatch/evidence/nrf52/thumb-assembler-errors.ImBrXqLR/` の `cargo-test`、`llvm-errors`、`llvm`、`shell` に保存した。監査対象の LLVM アセンブル・逆アセンブル6ケースはすべて合格した。台帳全体の残数は固定構成の集計を参照。

### Thumb 実行トレース

`execution-tracing.robot:600–626, 854–859` の3ケースを移植した。元の4命令を元のアドレスに配置した ELF を実行し、`PCAndOpcode` のテキスト4行と ReTrace v5 の63バイトを比較する。ヘッダーの CPU モデル名は移植先の `thumb cortex-m4` を使う。バイナリの禁止書式 `Disassembly` は設定時に診断し、ファイルを作成しない。

```sh
cargo test
tests/nrf52840-thumb-trace-upstream.sh "$PWD/target/debug/penv" "$ARTIFACT_DIR/trace"
tests/nrf52840-opcode-count-upstream.sh "$PWD/target/debug/penv" "$ARTIFACT_DIR/opcode-count"
tests/nrf52840-shell.sh "$PWD/target/debug/penv" ../renode "$SHELL_ELF"
```

ログと終了状態は `~/.local/state/penv-dispatch/evidence/nrf52/thumb-trace.DU40efTQ/` の `cargo-test`、`trace`、`opcode`、`shell` に保存した。現行 RESC は起動からの単一トレースを実行終了時に閉じる。対話中の有効・無効切り替えと他の形式は残る。
