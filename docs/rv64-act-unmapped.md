# 固定 ACT の実行時バスと LR/SC

固定 Renode の `SystemBus.ReportNonExistingRead` と `ReportNonExistingWrite` を照合し、RV64 のアライン済みデータアクセスに `--renode-unmapped-data` を追加しました。未マップのロードは警告と値 0、ストアは警告と破棄になります。アドレス変換、PMP、アラインメントと命令フェッチの検査は通常どおり実行されます。

固定 tlib の `arch/riscv/translate.c` の `gen_sc` と `hash-table-store-test.c` の `gen_store_table_check` では、単一 CPU の場合は共有ストアテーブルを参照せず、ゲストアドレスで予約を照合します。`--renode-lrsc` でこの単一ハート動作を選び、同一ハートの途中のストアとアクセス幅の変更でも予約を保持します。SC による予約の消費は保持します。固定 ACT のランナーは両方の互換モードを使い、通常 CLI での動作は既定のままです。

## 結果

- `Zalrsc-sc.d-00.elf` と `Zalrsc-sc.w-00.elf` は、固定 Renode Robot と Penv の両方で、同じ ELF、固定 CPU 文字列、1 MiB RAM、CPU ローカル監視を使い、自己署名の成功と監視の終了通知・値 0 を確認しました。以前の非整列 `ld` は失敗時の署名診断への分岐後に実行されていました。
- 固定 ACT 全体は 242 本中 216 本で自己署名と値 0 を確認しました。上流等価の合格に更新したのは今回直接照合した Zalrsc 2 本です。[ELF 別台帳](rv64-act-unmapped.tsv)
- ロード境界を越える 26 本のうち、24 本は最初のストア例外を越え、非整列ロード例外 4 で止まります。固定 Renode 側もこの 24 本は CPU abort です。中断同士を合格には数えていません。
- `Sm-00.elf` は未マップ書き込み後に終了通知まで進みました。自己署名の `mstatus` 全ビット書き込みの比較でビット 23・41 が異なり、終了値 1 を返します。残りの CSR 差分を固定 tlib と生成時の参照構成の両方で照合する必要があります。署名生成用 `Sm-00.sig.elf` は非ゼロ通知まで進み、通常の自己署名プロトコルとは区別しています。

`cargo test` は 114 件合格しました。検証ログと終了状態は `target/backlog-rv64-unmapped/` にあります。

## 再現

固定 CPU と ACT 素材の準備は [ACT とベクトル命令の初期対応](rv64-vector-bootstrap.md) を参照してください。`$renode_work` はビルド済み固定 Renode、`$act_work` は生成済み ELF の絶対パスです。

```sh
out=target/backlog-rv64-unmapped
mkdir -p "$out"
cargo test
cargo build --release
python3 scripts/probe-rv64-isa.py --act --robot-isa "$act_work" \
  --logs "$out/robot" > "$out/robot.tsv"
cases=spike-rv64-max/elfs/rv64i/Zalrsc
mkdir -p "$out/upstream-input/$cases"
for size in d w; do
  ln -sf "$act_work/$cases/Zalrsc-sc.$size-00.elf" "$out/upstream-input/$cases/"
done
python3 scripts/run-rv64-act-robot.py "$renode_work" "$out/upstream-input" \
  --jobs 2 --logs "$out/upstream-zalrsc" > "$out/upstream-zalrsc.tsv"
python3 scripts/summarize-rv64-act.py docs/rv64-act-equivalence.tsv "$out" \
  --verified-zalrsc "$out/upstream-zalrsc.tsv" > "$out/cases.tsv"
cmp docs/rv64-act-unmapped.tsv "$out/cases.tsv"
```
