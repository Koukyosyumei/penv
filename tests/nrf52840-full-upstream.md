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
| 291 Should Run Bluetooth sample | HR central/peripheral 各 ELF、2 台、同じ RF 媒体、global quantum 10 µs | 起動、初期化、アドレス C0:00:AA:BB:CC:DD へ接続、購読、通知。起動行と期待アドレスの判定を追加。現在の BLE 再検証を参照 |
| 332 Should Run Bluetooth Hci Uart Sample | `scripts/multi-node/nrf52840-ble-hci-uart-zephyr.resc` の 4 台。UART1 HCI ペア 2 本、BLE 媒体、controller 先行 1 s、quantum 10 µs | Zephyr 3.6.0 の両 host 起動・接続・購読・通知。元の 3 ELF を 4 台で起動し、UART1 で HCI フレーム、RADIO で接続・購読・通知を確認。CLI で同じ接続を構成。上流の 10 µs 量子条件は照合待ち |
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
- `tests/unit-tests/arm-cortex-m-fpu.robot` の `Should Pass Context Stacking Preservation Test v7-M` は元の ELF と fixture で `Test succeeded` を確認。FRDM-K64F 用 ELF の generic sharing は固定 NXP プラットフォーム、v8-M stacking は TrustZone を要求する。NOCP の FPU 有効側は Cortex-M4 で元の命令列から `OK` を確認。無効側では CPACR で拒否された命令がシミュレータを止めるため、ゲスト UsageFault と `NOCP` 出力が未検証。
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
| `tests/platforms/NRF52840.robot` | 17 | 15 | 2 | 0 |
| 監査した共有・混成 Robot | 190 | 41 | 4 | 145 |
| Infrastructure GPIO NUnit | 14 | 0 | 0 | 14 |
| 合計 | **221** | **56** | **6** | **159** |

固定台帳には対象外の記録489行もある。対象外の MVE 命令単体スイートの一部は、同じ ISA 要件を持つパラメータ群として記録した。対象分母は固定の実行構成を数える。再起動・再生成される条件の組を展開し、同一実行の途中で繰り返すレジスタ確認や UART 行は元のケース内の判定として保持する。同じ上流ケース、同じ CPU・UART・バイナリの組は一つにまとめる。

- CMSIS-NN の127 URLは、`hello_world` と `micro_speech` の同 SHA-1 別名を統合すると125構成。Cortex-M85 DSP 用 ELF と合わせて137個の異なる ELF の SHA-1 と逆アセンブルを確認した。固定 ELF には MVE 命令、またはリセット時の `MSPLIM` / `PSPLIM` 設定がある。別ケースや別 UART 構成で使う同一 ELF は独立に扱う。
- semihosting の22 ELF は EABI 属性が Armv8.1-M でも、Cortex-M4 指定の逆アセンブルで未知命令が出なかった。固定の Thumb BKPT とホスト ABI のテストを対象に残し、実行による等価判定は未達とした。
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
