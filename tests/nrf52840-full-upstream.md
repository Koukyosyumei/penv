# nRF52840 上流全件台帳

Renode は `c38ac3babcd38a7144b9498e3ca4c2226b76aa37`、Infrastructure は `09ac7c563095c5410258ecc1b1a6b841a013db85`、tlib は `c04522a9c886ba743c77a86b0c879103a2ef46d0`。[ケースとソース行の索引](nrf52840-full-upstream-index.tsv) を併記する。共有ケースの調査には、固定版の Renode UI、dts2repl、models-analyzer も別ディレクトリへ取得した。Renode の作業ツリーとサブモジュールは読み取り専用で参照した。

`nrf52-ready` は従来の範囲の結果。全件ゲートは本台帳の未実装・部分移植・調査中の行を含む全ケースの等価な合格証拠が揃ってから作成する。

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
| 249 Should Restart After Watchdog Timeout | Zephyr WDT ELF、上流 monitor で CPU を 1 MIPS に設定 | 起動行が 2 回。既存回帰は標準のステップ時刻を使用。実行条件差は要照合 |
| 262 Should Run Task Watchdog Subsystem | task_wdt ELF | スレッドとコールバックを含む指定行の系列を 2 周。既存回帰は起動・コールバック・リセットの部分一致。現行ログでは `Control thread getting stuck...` とコールバックの間の指定行が欠落 |
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
- `tests/unit-tests/arm-cortex-m-fpu.robot` の `Should Pass Context Stacking Preservation Test v7-M` は元の ELF と fixture で `Test succeeded` を確認。FRDM-K64F 用 ELF の generic sharing は固定 NXP プラットフォーム、v8-M stacking は TrustZone を要求する。2 件の NOCP ケースは Cortex-M33 フィクスチャのため Armv7-M への等価移植を要照合。
- `tests/unit-tests/arm-ge-flag.robot` は同じ Armv7E-M 命令列を Cortex-M4 で実行し、R4 の期待値 6 件を確認。usub8 という名前の行も、上流本文が実際に実行する ssub8 を使用。
- `tests/unit-tests/it-status.robot` は元の Cortex-M4 命令列と判定を Rust に移植し、条件コード、IT 状態とシフト、命令の条件実行、NMI 復帰、IT ブロック内フラグ更新の 6 ケースが合格。NMI は上流の monitor 注入と同じ例外番号を ICSR でペンドする。保存・復元と block begin/end hook の 3 ケースは未移植。
- `tests/unit-tests/page-access-via-io.robot` の Cortex-M4 ケースは対象。monitor 操作とパラメータを照合中。
- `tests/unit-tests/python-engine.robot` の nRF RADIO は RISC-V fixture 内の単体部品。`tests/external-control/external-control.robot` の nRF SPI も RISC-V fixture。ボードとしては別構成。共有 API・部品テストとしての適用性は調査中。
- Infrastructure の `src/Emulator/Main/Tests/UnitTests/GPIOTests.cs` と `GPIODetachingTests.cs` は汎用 GPIO 接続モデル用の NUnit。nRF モデル名を直接指定する fixture は PeripheralsTests、UnitTests、MonitorTests からは見つかっていない。共有 GPIO 操作の適用性と未分類 NUnit の調査は継続。
- dts2repl の `ci/load_repls_with_diffs.robot` / `Should Load Repls` は外部生成の REPL 一覧をパラメータとして読み込む。固定 Git リビジョンのソースだけでは nRF52840 の入力行を確定できず、台帳化は未完了。

## 調査範囲と未分類

- Renode の `tests` では、nRF52840・nRF52・Nano 33 BLE の直接参照と、Cortex-M・Armv7-M・Thumb-2 の共有フィクスチャを検索した。直接参照の 13 ファイルを記録し、機種固有の 17 ケースは上流の行と入力で照合した。
- 汎用テストは関連が確認できたスイートと適用性の判定待ちを含む。共有 Robot/NUnit の調査には未分類があり、上流 Robot のテンプレート展開と NUnit のパラメータ照合も残る。
- Armv8-M TrustZone と MVE の機種固有スイートは、その命令・セキュリティ状態を前提にするため対象外。別の Cortex-M をフィクスチャに使う共有ケースは、命令や入出力の適用性を個別に確認する。

## 再現

```sh
cargo test --locked
cargo build --release --locked
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
tests/nrf52840-task-wdt-upstream.sh /tmp/nrf52-acceptance/audio/task-watchdog.log
```

追加テストの ELF は公開ファイルの SHA-1 を照合する。Alarm の `--uart-steps` は仮想ステップを表示し、100,000,000 ステップを仮想 1 秒として判定する。全件ランナーと全件合格ログは未完成。

Nano 33 BLE は未変更の上流 REPL を使用する。ArduCAM の 4 画像は最後のスコア行と差分の閾値、LSM9DS1 の 2 入力は起動後の認識行が必要。スコアや認識行を欠く実行はランナーが失敗として報告する。
