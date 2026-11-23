# リリース統合のケース対応

Renode 固定リビジョンは `c38ac3babcd38a7144b9498e3ca4c2226b76aa37`。各行の出典は担当の合格コミットに含まれる。個別ゲートの証跡は対象ケースの特定に使用し、統合結果には加算しない。

| 担当 | 合格コミット | ケース対応の出典と再実行範囲 |
| --- | --- | --- |
| ARM | `45e874f8fcdc11135788f9aa677c3732b735ecfa` | README の「固定した上流 ARM 回帰の対応表」。Cortex-M と STM32F072 の Rust/CLI 回帰、Hello World ELF と HEX、Blinky と SysTick ゲート、Button、Console Echo とプロンプト、ADC、CRC |
| RV32 | `22777ec2d3e2352b5c3a436c9cf5bee9e8cc8f1a` | README の「上流 RV32 テストの対応表」と `scripts/validate-riscv-acceptance.sh`。148 ISA ELF、virt/特権プローブ5件、上流 Bare/PMP MRET、LiteX Linux・SD・Ethernet 等 |
| RV64 | `0edb80bd79fced68f8eab7b40b949ba6ab2dc4ed` | README の「上流 Renode の RV64 ケース対応」。ISA 25ケースの移植、StarLight 上流4件と外部 PMP 3件、GPIO 経由再起動と Ethernet、RV64 virt、Fire SD のセクター照合 |
| STM32H7 | `21624b76fabbfee9151003a849b967d65356b27c` | README の `close-h7-io` と `close-h7-acceptance-gaps`、`tests/stm32h7-*-smoke.sh` 3本。Zephyr Shell、I²C、VEML7700、DMA2D/LTDC、GPIO、SD、Ethernet TCP 44応答、IIS2MDC 9ベクトル、PWM |
| nRF52 | `049e0ecf5d5c647a3bdcdc4303d4342e63686047` | `tests/nrf52840-acceptance.md` と `tests/nrf52840-acceptance.sh`。Radio、EGU、SPI、PDM、RNG、TWI、I2S、Timer、ECB、BLE Central、BLE Pair、上流 Audio の12グループ |
| STM32F407 | `c4960230193b12a5dc36fd31c52d6ad346fdb52a` | README の「STM32F407 上流ケースの棚卸し」と `tests/stm32f407-upstream-smoke.sh`。Discovery FreeRTOS、Contiki、Cortex-M4F FreeRTOS、PWM キャプチャと直交位相、および共有 STM32H7 の Zephyr 再確認 |

リリース検証では `scripts/verify-release.sh` を統合コミットのクリーンな worktree から実行する。引数は固定 Renode worktree、新規ログディレクトリ、STM32F072 公開素材、RV32 ISA アーカイブ、StarLight 公開素材、再起動用 StarLight FIT 素材、RV64 virt 起動素材、Fire 公開素材の順。素材の入手と生成手順は各 README 節を参照する。ログには統合 SHA と共有 release 実行ファイルの SHA-256 を記録する。

同じホスト上で並行する検証では、`PENV_RELEASE_PORT_BASE` に空いている連続9個のループバック UDP ポートの先頭を指定する。既定値は 30800。実行ファイルのコピーと SHA-256 は検証ディレクトリに残す。
