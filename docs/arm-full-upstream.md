# ARM 上流回帰の調査

対象は STM32F072B Discovery、Armv6-M の Cortex-M0/M0+、および使用する STM32 の周辺機器。既存受入の対象と結果は README の「固定した上流 ARM 回帰の対応表」に記録している。

| 参照ツリー | 固定コミット |
| --- | --- |
| Renode | `c38ac3babcd38a7144b9498e3ca4c2226b76aa37` |
| `src/Infrastructure` | `09ac7c563095c5410258ecc1b1a6b841a013db85` |
| Infrastructure の `src/Emulator/Cores/tlib` | `c04522a9c886ba743c77a86b0c879103a2ef46d0` |

[ケース台帳](arm-upstream-cases.tsv)は調査候補を上流パス・ケース・ソース行で記録する。例外復帰のキーワード引数行は独立させた。その他の `see source case body` の行は、パラメータの展開と照合が残っている。Infrastructure の時計、GPIO、バス、CRC 等の NUnit 候補も収録した。固定 tlib ツリーには Robot と C# テストファイルがなかった。

`existing translation` は既存の移植または今回追加した移植を示し、全ケース受入の判定には使用しない。`partial` は対応する観測条件の一部、`open applicability` は適用の根拠の確認が残る候補を示す。台帳は調査途中であり、全件合格ゲートの根拠としては未完成。

今回の NMI 回帰では、上流のリセットループ `0x20`、NMI ハンドラー `0x100`、`NOP` 後の `0x102`、`BX LR` 後の `0x20` と初期 SP `0x800` を Armv6-M で検査する。上流のモニター操作 `nvic SetPendingIRQ 2` に対応するゲスト側の操作として、SCB ICSR の NMIPENDSET を使う。TrustZone 行の `BXNS` は Armv8-M Security Extension に依存する。

## 再現

```sh
export CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=gcc
cargo test --locked
cargo test --locked renode_nmi_bx_lr_restores_interrupted_thread
scripts/verify-stm32f072.sh ../renode /tmp/新規証跡ディレクトリ
```

上記スクリプトは固定 Renode ツリーの公開イメージ7点を取得し、サイズ・SHA-1 と照合してから STM32F072 の ELF・HEX 回帰を実行する。

## 残件

共有テストの適用可否とパラメータを、上流と同じ入力・タイミング・失敗判定まで突合する。例外復帰のうち BLX の戻り先差、Armv6-M に適用できる HardFault/ロックアップ、CPUWAIT のリセット境界、Robot UART テスター、STM32 USART のボーレート・IDLE・パリティ、GPIO とタイマーの NUnit 条件が未完了。既存の公開イメージ回帰と部分移植は個別の証拠として維持する。
