# RV64 のアセンブルと逆アセンブル往復

固定 Renode の `tests/unit-tests/llvm-asm-disasm.robot` から、RV64 の往復4ケースとアセンブルエラー1ケースを移植した。Penv の `--assemble CPU@BASE:INSTRUCTION` は指定 CPU の ISA に合わせて PATH 上の `llvm-mc` を呼び、完成した命令バイト列を指定 CPU から見える RAM に書く。検証には LLVM 15.0.7 を使用した。

4構成は `rv64imac`、`rv64imafdc`、`rv64gc`、`rv64gcv`。固定 Robot と同じ RAM、アドレス0、ニーモニック、オペランドを使い、生成バイト列と逆アセンブル結果を照合した。圧縮と非圧縮、浮動小数点、原子命令、ベクトル命令を含む。RV64G のエラーケースでは、不明なニーモニック、不正なレジスター名、V 拡張が必要な命令の3入力がそれぞれ期待された診断を返した。失敗時は RAM に書き込む前に終了する。

## 再現

```sh
out=target/backlog-rv64-assemble
ref=/var/tmp/penv-rv64-act-upstream
work=/tmp/penv-rv64-full
elf=target/backlog-rv64-per-core/multibus.elf
mkdir -p "$out"
cargo test > "$out/cargo-test.log" 2>&1
cargo build --release > "$out/build.log" 2>&1
for fixture in 'Should Assemble And Disassemble RISCV64' 'Should Handle Illegal Instruction When Assembling'; do
  PATH="$ref/dotnet:$ref/robot-venv/bin:$PATH" \
    "$ref/renode/renode-test" "$ref/renode/tests/unit-tests/llvm-asm-disasm.robot" \
    --fixture "$fixture" --results-dir "$PWD/$out/upstream-${fixture// /-}" -P 0
done
python3 scripts/verify-rv64-assembly.py ../renode/tests/unit-tests/llvm-asm-disasm.robot \
  --logs "$out/penv" > "$out/penv.log" 2>&1
python3 scripts/verify-rv64-disassembly.py "$elf" --logs "$out/disassembly" > "$out/disassembly.log" 2>&1
python3 scripts/verify-rv64-per-core.py "$elf" --logs "$out/firmware" > "$out/firmware.log" 2>&1
python3 scripts/probe-rv64-isa.py --act --robot-isa "$work/arch-git/work" \
  --jobs 4 --logs "$out/act" > "$out/act.tsv" 2> "$out/act.stderr.log"
```

固定 Renode と ACT の準備は [ACT 終了通知と署名](rv64-act-equivalence.md)、公開 ELF の入手とハッシュは [ハート別 RAM の記録](rv64-per-core.md) を参照。ケース別ログと終了状態は `out` に保存した。

固定 Robot の5ケースと Penv の対応する7チェックが合格した。前回の逆アセンブル3件、公開 ELF の UART・メモリ回帰4件、`cargo test` 125件も合格。固定 ACT の診断は前回と同じ216本成功・26本失敗だった。

## 残課題

CLI のアセンブルは単一命令に対応する。ラベル、再配置、ディレクティブ、複数命令、Renode Monitor の任意の triple・方言指定は未対応。LLVM の版により追加の ISA 拡張や診断文が異なる場合がある。
