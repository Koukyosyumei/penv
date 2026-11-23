# RV64 浮動小数点リセットとゼロ除算

RV64 REPL の `renodeResetMstatus: true` は、固定 tlib の `set_default_mstatus` に合わせて MPP を M にし、F/D 有効時の FS と XS を Initial にします。このモードは CPU 構築時に適用され、Penv がマシンを再構築する場合にも同じ設定を使います。

`--set-freg CPU@fN:VALUE` は起動前に指定した浮動小数点レジスターへ生ビットをセットします。`--read-register` は `f0`～`f31` と `fflags` を読み出せます。

## ケース対応

固定 Renode の `riscv-unit-tests.robot:458` と `:483` は両方合格しました。Penv の同じ ISA、CLINT、RAM、初期 PC を使用し、NaN ボックス化した分子と分母をセットしました。初期の `f2=0` と DZ=0、1命令後の `pc=0x2004`、正の無限大、DZ=1 を両精度で照合しました。Penv でも FS はリセットプロファイルで初期化されるため、元の2ケースを上流等価の合格に更新しました。

`cargo test` は124件合格しました。ログは `target/backlog-rv64-fdiv-reset/` にあります。CPU 単体の Monitor リセット操作と ResetVector の変更・再設定は引き続き未達です。

## 再現

```sh
out=target/backlog-rv64-fdiv-reset
mkdir -p "$out"
cargo test
cargo build --release
"$renode_work/renode-test" "$renode_work/tests/unit-tests/riscv-unit-tests.robot" \
  --fixture 'Should Set Flag On * Precision Floating Point Division By Zero' \
  --results-dir "$out/upstream" -P 0
cat > "$out/fdiv.repl" <<'REPL'
clint: IRQControllers.CoreLevelInterruptor @ sysbus 0x44000000
    frequency: 66000000
cpu: CPU.RiscV64 @ sysbus
    timeProvider: clint
    cpuType: "rv64gc_Zfh"
    renodeResetMstatus: true
mem: Memory.MappedMemory @ sysbus 0x1000
    size: 0x40000
REPL
for variant in SINGLE HALF; do
  riscv-none-elf-gcc -D"$variant" -march=rv64gc_zfh -mabi=lp64d -nostdlib -static \
    -Wl,-n,-Ttext=0x2000,-e,_start tests/fixtures/rv64-fdiv-zero.S -o "$out/$variant.elf"
  if [ "$variant" = SINGLE ]; then
    zero=ffffffff00000000; one=ffffffff3f800000; expected=ffffffff7f800000
  else
    zero=ffffffffffff0000; one=ffffffffffff3c00; expected=ffffffffffff7c00
  fi
  for steps in 0 1; do
    status=0
    target/release/penv run --platform "$out/fdiv.repl" --max-steps "$steps" \
      --set-freg "cpu@f0:0x$zero" --set-freg "cpu@f1:0x$one" --set-freg cpu@f2:0 \
      --read-register cpu@pc --read-register cpu@f2 --read-register cpu@fflags \
      "$out/$variant.elf" > "$out/$variant-$steps.log" 2>&1 || status=$?
    if [ "$steps" = 0 ]; then
      test "$status" -eq 0 || exit 1
      grep -Fx 'penv: register cpu@f2 = 0x0000000000000000' "$out/$variant-$steps.log" || exit 1
      grep -Fx 'penv: register cpu@fflags = 0x0000000000000000' "$out/$variant-$steps.log" || exit 1
    else
      test "$status" -eq 2 || exit 1
      grep -Fx 'penv: register cpu@pc = 0x0000000000002004' "$out/$variant-$steps.log" || exit 1
      grep -Fx "penv: register cpu@f2 = 0x$expected" "$out/$variant-$steps.log" || exit 1
      grep -Fx 'penv: register cpu@fflags = 0x0000000000000008' "$out/$variant-$steps.log" || exit 1
    fi
  done
done
```
