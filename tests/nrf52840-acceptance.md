# nRF52840 受け入れ対応表

対象は nRF52 作業キューで指定された Shell、リセット、TWI、SPI、タイマー、音声、IRQ、RADIO、RNG、ECB、EGU と BLE の回帰です。上流ケース名は Renode の `tests/platforms/NRF52840.robot` に対応します。

| 上流ケース／対象機能 | ログ（成果物ディレクトリからの相対パス） | 判定 |
| --- | --- | --- |
| Should Run ZephyrRTOS Shell On UART / On UARTE | `audio/shell-uart.log`、`audio/shell-uarte.log` | それぞれリセット前後に `demo ping` が応答 |
| Should Restart After Watchdog Timeout | `audio/watchdog.log` | CPU 1 MIPS で上流の起動行が 2 回 |
| Should Run Task Watchdog Subsystem | `audio/task-watchdog.log` | 上流と同じ順序で UART 20 行を照合。各待機は仮想 8 秒 |
| Should Handle I2C / SPI | `audio/i2c-*.log`、`audio/spi-*.log` | 各バスで 0.00、1.00、2.83、5.20 g。軸入力ごとに別の起動 |
| Should Echo I2S Audio | `audio/i2s.log`、`audio/i2s-{input,output}.pcm` | 上流の 32 バイトの PCM が完全一致 |
| Should Detect Yes Pattern | `audio/speech.log` | 上流 PCM で実 ELF が `Heard yes` を表示 |
| Should Detect Passed Interrupt In WFE | `audio/wfe.log` | 割り込みを処理した後に WFE を通過 |
| Should Run Zephyr's kernel.interrupt Test | `audio/interrupt.log` | `PROJECT EXECUTION SUCCESSFUL` |
| Should Run Bluetooth sample | `ble-pair.log`、`ble-central.log` | 上流の 2 ELF が仮想無線上で接続し、購読後に 2 バイトの心拍通知を受信。central 単体でもスキャン開始 |
| レガシー TWI / SPI | `twi.log`、`spi.log` | 検証 ELF で TWI の NACK、再 START、IRQ 3/4、SPI の READY、バッファ、IRQ 35 |
| TIMER / I²S / PDM | `timer.log`、`i2s.log`、`pdm.log` | 検証 ELF で比較・PPI・IRQ、SRAM バッファ、PCM、転送終了を確認 |
| RADIO / RNG / ECB / EGU | `radio.log`、`rng.log`、`ecb.log`、`egu.log` | 検証 ELF で送受信、乱数開始停止、AES 既知答え、イベントと各 IRQ |

## 再現

```sh
cargo test --locked
cargo build --release --locked
tests/nrf52840-acceptance.sh target/release/penv ../renode /tmp/nrf52840-acceptance
```

ELF のコンパイルに `clang` と `ld.lld`、公開素材の取得・確認に `curl` と `sha1sum`、Watchdog の UART 照合に `python3` が必要です。各グループの終了コードとログを保存します。上流素材の SHA-1 は各スクリプトで照合します。

## 残課題

上流 Robot の Alarm、LED/Button、CMSIS DSP Q15、Bluetooth HCI UART、BME280 のチップ ID とバススキャンは、この作業キューの対象外です。ADXL372 の走行中の入力変更、CCM による BLE 暗号化、物理 RF、SPIM/TWIM EasyDMA、FPU の遅延保存にも制限があります。詳細は [README](../README.md) を参照してください。
