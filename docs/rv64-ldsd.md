# RV64 LD と SD のアラインメント照合

`--set-gpr CPU@xN:VALUE` を追加し、停止中に名前付き RV64 CPU の整数レジスターを設定できるようにしました。`x0` は変更できません。`--read-register` で `mcause` と `mtval` も観測できます。

固定 Renode の `riscv-unit-tests.robot` にある LD/SD 4ケースを直接実行し、同じ `rv64gc_Zfh`、初期 PC `0x2000`、`mtvec=0x1010`、命令語と `a1` で Penv の ELF を1命令実行しました。アライン済みの実効アドレス `0x8` は固定フィクスチャで未マップです。Penv の `--renode-unmapped-data` で Renode と同じ警告継続動作を使い、非整列アクセスは先に例外を返します。

| 固定ケース | a1 | 1命令後の PC | mcause |
| --- | ---: | ---: | ---: |
| Should Handle LD | 3 | `0x2004` | 0 |
| Should Fail LD | 0 | `0x1010` | 4 |
| Should Handle SD | 3 | `0x2004` | 0 |
| Should Fail SD | 1 | `0x1010` | 6 |

`cargo test` は125件合格し、固定 ACT 242本の結果は変更前と一致しました。

固定 ACT の署名・終了通知で未達のケースは [ACT 台帳](rv64-act-unmapped.tsv) に残っています。今回の4ケースの照合は、ACT の署名失敗後に発生する非整列診断まで解決するものではありません。

## 再現

ログと終了状態は `target/backlog-rv64-ldsd/` にあります。`$renode_work` は固定 Renode のビルド済みツリーです。Penv 側の REPL は [浮動小数点リセット検証](rv64-fdiv-reset.md#再現) の `fdiv.repl` と同じ設定です。

```sh
out=target/backlog-rv64-ldsd
mkdir -p "$out"
cargo test
cargo build --release
for op in LD SD; do
  "$renode_work/renode-test" "$renode_work/tests/unit-tests/riscv-unit-tests.robot" \
    --fixture "Should * $op" --results-dir "$out/upstream-$op" -P 0
done
for variant in LOAD STORE; do
  riscv-none-elf-gcc -D"$variant" -march=rv64gc_zfh -mabi=lp64d -nostdlib -static \
    -Wl,-n,-Ttext=0x2000,-e,_start tests/fixtures/rv64-ldsd.S -o "$out/$variant.elf"
done
for row in 'LOAD 3 2004 0' 'LOAD 0 1010 4' 'STORE 3 2004 0' 'STORE 1 1010 6'; do
  read -r variant value pc cause <<< "$row"
  status=0
  target/release/penv run --platform "$out/ldsd.repl" --renode-unmapped-data --mtvec 0x1010 \
    --set-gpr "cpu@x11:$value" --max-steps 1 --read-register cpu@pc --read-register cpu@mcause \
    "$out/$variant.elf" > "$out/$variant-$value.log" 2>&1 || status=$?
  test "$status" -eq 2 || exit 1
  grep -Fx "penv: register cpu@pc = 0x000000000000$pc" "$out/$variant-$value.log" || exit 1
  grep -Fx "penv: register cpu@mcause = 0x000000000000000$cause" "$out/$variant-$value.log" || exit 1
done
```
