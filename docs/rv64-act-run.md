# 固定 ACT 242 ELF の実行

固定 Renode を実行した対照結果と終了通知・署名の追加検証は [上流照合](rv64-act-equivalence.md) にあります。

固定 Renode の `tests/unit-tests/riscv-arch-tests.robot:54` に対応する 242 パラメータを、付属の Python セレクターで取得し、生成 ELF を全件実行しました。中間署名 ELF もセレクターの結果に含まれるため実行対象に保持しました。[実行台帳](rv64-act-run.tsv)

| 構成 | 終了値 0 | 失敗 |
| --- | ---: | ---: |
| 修正前・後ともに RAM 1 MiB、非整列アクセス無効 | 214 | 28 |
| 修正後、RAM 4 MiB、非整列アクセス無効 | 239 | 3 |
| 修正前、RAM 4 MiB、非整列アクセス有効 | 230 | 12 |
| 修正後、RAM 4 MiB、非整列アクセス有効 | 238 | 4 |

固定 Robot の RAM は `0x80000000` から 1 MiB、UART は NS16550 の `0x10000000` です。指定 ISA は `rv64gcv_zba_zbb_zbc_zbs_zfh_zacas_smepmp`、CPU 上の `tohost` を `ArrayMemoryRiscVTestWatcher` で監視します。Penv の 1 MiB プロファイルはメモリ配置、UART、CPU の既定の非整列アクセス禁止、ELF、奇数の終了通知と終了値 0 の判定を揃えています。Penv のサポート済み ISA、RAM ストアの監視、200 万命令の上限には差があります。全 242 行の上流等価判定は `pending` です。RAM 4 MiB と非整列アクセス許可は、ロード範囲と失敗ハンドラー内の非整列ロードを越えて原因を確認する診断専用の変更です。

## 失敗の内訳

- RAM 1 MiB では 26 ELF のロードセグメントがマップ外です。残る 2 件は Zalrsc のテスト失敗を報告する途中で、診断ハンドラーの非整列 `ld` が例外 4 を起こしました。
- 拡張プロファイルの修正前の 12 件のうち、F/D の融合積和 8 件は最小正規化数に丸められる境界で UF フラグを落としていました。指数範囲を制限せずに目的精度へ丸めた値で微小性を判定するよう修正しました。8 ELF が終了値 0 に変わり、テストには境界の両側を追加しました。[Berkeley SoftFloat の微小性判定](https://www.jhauser.us/arithmetic/SoftFloat-3/doc/SoftFloat-FAQ.html)
- 修正後に残る 4 件は、`Sm-00.elf` の `mstatus.VS`、`Sm-00.sig.elf` の署名生成用通信、Zalrsc の異なる幅をまたぐ LR/SC 予約の 2 件です。`Sm-00.elf` は 4 MiB・非整列アクセス無効では終了値 0 でしたが、非整列許可時の `mstatus.VS` 不一致は残ります。署名 ELF は通常の自己検証 ELF と異なる値を `tohost` に書きますが、固定セレクターの規則に従い台帳に残しています。

## 再現

固定 ACT の生成手順は [ACT と ISA 診断](rv64-act-and-failures.md) を参照してください。今回の `$act_work` は `/tmp/penv-rv64-full/arch-git/work` です。`$virt_work` は README の RV64 virt 用の OpenSBI・Linux・DTB を用意したディレクトリです。

```sh
cargo test
cargo build --release
out=target/full-rv64-act-run
mkdir -p "$out"
python3 scripts/probe-rv64-isa.py --act "$act_work" --logs "$out/current" > "$out/current.tsv"
python3 scripts/probe-rv64-isa.py --act "$act_work" --ram-size 0x400000 \
  --logs "$out/current-large" > "$out/current-large.tsv"
python3 scripts/probe-rv64-isa.py --act "$act_work" --ram-size 0x400000 --allow-unaligned \\
  --logs "$out/current-expanded" > "$out/current-expanded.tsv"
python3 scripts/verify-rv64-virt.py "$virt_work" --logs "$out/virt"
```
