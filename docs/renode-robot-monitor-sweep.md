# Robot 全件再実行とモニター対応

対象は Renode `c38ac3babcd38a7144b9498e3ca4c2226b76aa37` の公開マニフェストを
Linux x86-64 で選択した 304 スイート、2,197 インスタンス。各段階で
同じ ID とパラメーターを投入した。上流 Robot ファイル、テストデータ、アサーションは
固定ツリーから読み取る。

```sh
tmux new-session -d -s penv-robot-sweep '
  target/renode-venv/bin/python scripts/upstream/run.py \
    --all --jobs 10 --out target/robot-sweep \
    >target/robot-sweep.log 2>&1
  echo $? >target/robot-sweep.exit
'
```

NUnit discovery を併走する場合は `--nunit-root` を明示する。
Robot 専用の `--all` は指定を省略できる。

## 修正した停止原因

- `logLevel` の数値と名前を上流のレベルに対応させ、記録されるログに適用した。
  CLIC 144件は `logLevel 0` を通過し、`machine UserState` に到達した。
- 文字列の `UserState`、複数行の変数、`echo` のモニター出力を扱った。
  CLIC の次の停止は `rvtest_code_end` に取り付ける Python コールバック。
  既存の停止用フックと一般の Python フックを区別し、後者の非対応を報告する。
- `Create Log Tester` と `Wait For Log Entry` を実際の penv ログと
  仮想時間に接続した。既存のログフックと STM32F4 のフラッシュ警告を記録する。
  STM32F4 の消去拒否ケースは penv と、別途実行した公式 Renode の両方で合格した。
- RISC-V の PC、レジスター、ステップ、実行モード、トラップベクターを
  既存の CPU 実装に接続した。対応する `AssembleBlock` は LLVM の出力を
  penv の RAM へ書く。ラベル・再配置を要するアセンブリーには対応が必要。
- 同期の `Add Symbol`、`Store`、アドレスとシンボルのメモリ比較は、
  指定の幅で実際に penv のバスを読み書きする。アサーション付きの
  `memory-assertions.robot` は8件合格。RV32 の残りは指定された
  `V` 拡張を penv が非対応と報告、または時間付きアサーションなどが未実装。

## 全件再実行の結果

| 実行 | penv SHA | PASS | FAIL | UNSUPPORTED | SKIP | NOT_RUN |
| --- | --- | ---: | ---: | ---: | ---: | ---: |
| 再開前 `upstream-monitor-final` | `c43be89` | 18 | 0 | 1,965 | 133 | 81 |
| ログレベル `robot-loglevel` | `5a2766d` | 18 | 0 | 1,965 | 133 | 81 |
| UserState・レジスター `robot-userstate` | `772593d` | 22 | 12 | 1,949 | 133 | 81 |
| モニターログ・未登録アドレス `robot-logwait-retry` | `a9dc1b0` | 31 | 14 | 1,791 | 133 | 228 |
| 実行モード・LLVM `robot-scheduler` | `6660406` | 32 | 16 | 1,893 | 133 | 123 |
| メモリ・シンボル `robot-memory` | `0f9cec3` | 40 | 16 | 1,876 | 133 | 132 |
| ResetVector・アセンブラー引数 `robot-reset` | `c81f379` | 42 | 14 | 1,876 | 133 | 132 |

最終実行の全ケースは `target/robot-reset/results.json`、再開前との差分は
`target/robot-reset-delta.json`、CLIC 144件の停止点履歴は
`target/robot-resume-audit.json` に保存した。上流 Renode の
STM32F4 ログケースは `target/reference-flash-warning/robot_output.xml`。
モニター・ログの Rust チェックは合計14件合格。
