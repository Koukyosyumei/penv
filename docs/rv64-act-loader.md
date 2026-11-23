# RV64 固定 ACT の ELF ロード境界

固定 Robot の 1 MiB RAM を越える PT_LOAD を持つ ACT 26 本を対象にしました。固定 Renode の `SystemBus.WriteBytes` と `FindTargets` は、未マップ範囲を警告して飛ばし、マップ済み部分に元のファイルオフセットからロードします。Penv の `--renode-elf-load` は、RV64 ELF の物理セグメントについて同じ境界処理を行います。固定 ACT のランナーは `--robot-isa` のときこのモードを使用します。通常の ELF ローダーは従来どおりセグメント全体のマッピングを要求します。

実行時バスと LR/SC の後続結果は [固定 ACT の実行時バスと LR/SC](rv64-act-unmapped.md) にあります。

## 結果

- 26 本すべてで、固定 Renode の警告範囲と Penv が飛ばした範囲が一致しました。Renode の終端は包含、台帳と Penv ログの終端は排他です。CLI テストでは二つの RAM 区間の間に穴を作り、両側に正しいファイル位置からロードされること、末尾の BSS 範囲と通常モードの拒否を確認しました。
- 固定 ACT 242 本の再実行で、214 本が自己署名と終了値 0 を確認できました。範囲外の 26 本は全件実行へ進み、24 本がストアアクセス例外 7、`Sm-00.elf` と署名生成用 `Sm-00.sig.elf` が命令上限に達しました。固定 Renode では前者 24 本が CPU abort、`Sm-00.elf` が合格、署名生成用 ELF が失敗でした。上流等価の合格は保留です。[範囲別台帳](rv64-act-loader.tsv)
- `Sm-00.elf` の固定 Renode はロード後も範囲外への CPU の書き込みを警告して続行し、終了通知を出します。Penv の今回の変更はロード時の挙動に限定しています。実行時バスの未マップアクセスと例外処理は別の差分として残っています。
- 既存の Zalrsc 2 本は `pc=0x8000bedc` の非整列 `ld` で例外 4 を返します。固定 Robot は非整列許可を指定していないため、条件を変える前に、その命令に到達する制御フローと LR/SC 予約の差分を照合する必要があります。
- 別グループでは固定 Renode の ArrayMemory 向け AMOCAS 6 ケースが合格しました。Penv で `amocas.d` と `amocas.q` の一致・不一致 4 ケースを同じアドレス、初期値、レジスター結果、メモリー結果で確認しました。Rust 検証は RAM バッキングを使うため部分移植として台帳に記録しました。`amocas.w` の旧値の拡張規則には差が残ります。

`cargo test` は 112 件合格しました。ログと終了状態は `target/backlog-rv64-loader/` にあります。固定 Renode の ACT 行別ログは `target/full-rv64-act-equivalence/upstream-cases/` を使用しました。

## 再現

`$act_work` と `$renode_work` の準備は [固定 ACT の上流照合](rv64-act-equivalence.md) を参照してください。

```sh
out=target/backlog-rv64-loader
mkdir -p "$out"
cargo test
cargo build --release
python3 scripts/probe-rv64-isa.py --act --robot-isa "$act_work" \
  --logs "$out/robot" > "$out/robot.tsv"
python3 scripts/summarize-rv64-act.py docs/rv64-act-equivalence.tsv "$out" \
  --load-ranges docs/rv64-act-load-ranges.tsv \
  --upstream-logs target/full-rv64-act-equivalence > "$out/load-cases.tsv"
cmp docs/rv64-act-loader.tsv "$out/load-cases.tsv"
"$renode_work/renode-test" "$renode_work/tests/unit-tests/riscv-amocas.robot" \
  --fixture '*MMIO Memory Location' --results-dir "$out/amocas-upstream" -P 0
```
