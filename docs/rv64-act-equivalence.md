# 固定 RV64 ACT の上流照合

このページは `4302d2d` の比較結果です。Penv の [Smepmp 対応と再検証](rv64-smepmp.md) では、固定 ISA の未実装拡張は V が残っています。

対象は Renode `c38ac3babcd38a7144b9498e3ca4c2226b76aa37` の `tests/unit-tests/riscv-arch-tests.robot:54` と、固定 ACT の手順で生成した 242 ELF です。[ELF ごとの判定](rv64-act-equivalence.tsv) [生成手順](rv64-act-and-failures.md)

## 終了と署名の確認

Robot は `LoadELF` の後、`tohost` に CPU ローカルの 4 KiB の `ArrayMemoryRiscVTestWatcher` を登録します。Penv の `--robot-tohost` はロード済み RAM の上にゼロ初期化した別バッファーを置きます。オフセット 0 に書く値の下位ビットが 1 のとき、同位置の 64 ビット値を読み返して右に 1 ビットシフトし、最初の終了値として保存します。1・2・4・8 バイトの通常データアクセスと、終了後の上書きによる終了値の保持を Rust で検証しました。従来の `--tohost` は RAM 上の監視を使います。

ACT の最終 ELF は生成時に埋め込まれた期待署名と実行時の結果を自身で照合します。今回のランナーは対象ファイル名の `RVCP-SUMMARY … PASSED` を 1 件確認し、終了通知および終了値 0 も確認した場合にだけ合格候補として数えます。上流の実行では固定 Robot の判定も必須です。上流セレクターが拾う `Sm-00.sig.elf` は署名**生成用**で自己検証の通知を出さないため、台帳に保持し、通常の最終 ELF と同じ合格には数えません。

Penv で固定 Robot の `rv64gcv_zba_zbb_zbc_zbs_zfh_zacas_smepmp` をそのまま指定すると、未実装の V および Smepmp が残るため 242 本すべて構成段階で拒否されます。Penv の別プロファイルではサポート済みの ISA を指定し、1 MiB で 214 本、4 MiB・非整列許可で 238 本の自己検証と終了通知を確認しました。メモリ拡大、非整列許可、Penv の命令上限や未実装拡張がある実行は、上流と同等の Penv 合格には含めていません。

## 実行結果

| 実行 | 署名と終了を確認 | それ以外 |
| --- | ---: | --- |
| 固定 Renode、固定 Robot | 208 | CPU abort 33、終了値非ゼロ 1 |
| Penv、固定 ISA 指定 | 0 | CPU 構成拒否 242 |
| Penv、対応 ISA と RAM 1 MiB | 214 | ロード拒否 26、実行時例外 2 |
| Penv、対応 ISA と RAM 4 MiB、非整列許可 | 238 | 失敗 4 |

固定 Robot の一括実行は、`Zca-c.add-00.elf` の CPU abort で途中終了しました。残りを含めて判定するため、同じ Robot ファイル・キーワード・フィクスチャを使い、セレクターが参照する入力をプロセスごとに元 ELF 1 本へのシンボリックリンクにしました。直接実行のログと Robot XML は `target/full-rv64-act-equivalence/upstream-cases` に保存しました。Penv の等価合格は保留です。

`cargo test` は 101 件通過し、RV64 virt の公開素材で OpenSBI と Linux の起動、UART の root ログイン、`id -u` と `uname -m` を確認しました。

## 1 MiB のロード範囲

Robot の RAM は `[0x80000000, 0x80100000)` です。固定 ACT の `config/spike/spike-rv64-max/link.ld` は `0x80000000` から `.text`、`.tohost`、`.data`、`.bss` を配置し、各セクションの間をアラインします。RAM 上限を指定する `MEMORY` 領域や `ASSERT` はありません。生成した 26 ELF は、データおよび BSS を含む物理 `PT_LOAD` の終端が `0x80100000` を越えます。[物理セグメント一覧](rv64-act-load-ranges.tsv)

固定 Renode の `LoadELF` は物理セグメントを `SystemBus.WriteBytes` に渡します。`FindTargets` はマップされていない範囲の警告を出し、書き込み先からその範囲を除きます。Penv の標準ローダーはセグメント全体を格納できるか先に検証し、不足時には ELF のロードを拒否します。固定 Renode が出した 26 件すべての警告範囲は ELF の表と一致しました。この 26 本の直接実行では CPU abort 24 本、非ゼロ終了 1 本、署名と終了を確認したもの 1 本（`Sm-00.elf`）でした。メモリを拡大した結果はこの差の診断に使い、固定 1 MiB の結果とは別に記録します。

## 再現

固定 Renode を読み取り専用の参照ツリーから別チェックアウトに作り、同じコミットのサブモジュールを展開しました。ビルドでは .NET 10.0.100、CMake 3.31.8 を使用し、`lib/resources` は `9d7824771f3db67663950cae06f5bb7f89fbf33b` でした。Robot の実行用 Python 環境は固定ツリーの `tests/requirements.txt` から用意します。以下の `$renode_work` はビルド済みチェックアウト、`$act_work` は生成済み ACT の `work` ディレクトリです。Python と .NET は PATH に追加します。

```sh
cargo test
cargo build --release
out=target/full-rv64-act-equivalence
mkdir -p "$out"
python3 scripts/run-rv64-act-robot.py "$renode_work" "$act_work" --jobs 8 \
  --logs "$out/upstream-cases" > "$out/upstream-cases.tsv"
python3 scripts/probe-rv64-isa.py --act --robot-isa "$act_work" \
  --logs "$out/robot" > "$out/robot.tsv"
python3 scripts/probe-rv64-isa.py --act "$act_work" --logs "$out/1mib" > "$out/1mib.tsv"
python3 scripts/probe-rv64-isa.py --act "$act_work" --ram-size 0x400000 --allow-unaligned \
  --logs "$out/expanded" > "$out/expanded.tsv"
python3 scripts/verify-rv64-virt.py "$virt_work" --logs "$out/virt"
```
