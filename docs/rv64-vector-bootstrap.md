# RV64 ACT とベクトル命令の初期対応

固定 Renode の `tests/unit-tests/riscv-arch-tests.robot:54` が設定する `rv64gcv_zba_zbb_zbc_zbs_zfh_zacas_smepmp` を Penv で構築できるようにしました。RAM 1 MiB、NS16550、ロード後の CPU ローカル 4 KiB `tohost` 監視は、従来の固定構成ランナーを使います。結果は [ELF 別の照合台帳](rv64-act-prepare.tsv) に記録します。

## 実装範囲

固定セレクターの 242 ELF は、D、F、I、M、Zaamo、Zalrsc、Zca、Zcd、Zifencei、Sm と Sm 署名生成用の ELF でした。最終 ELF の逆アセンブルにはベクトル構成命令や下記のベクトルデータ命令がなく、Sm は特権 CSR の署名を比較します。ベクトル命令そのものは Renode の `tests/unit-tests/riscv-vector-csr.robot` を出典として検証しました。

- 固定 tlib の既定値に合わせて VLEN 512、ELEN 64 とし、`misa.V`、`mstatus/sstatus.VS` と SD、`vstart`、`vxsat`、`vxrm`、`vcsr`、`vl`、`vtype`、`vlenb` を実装しました。
- `vsetvli`、`vsetivli`、`vsetvl` で AVL、SEW、LMUL を処理します。無効な構成は `vill` と `vl=0` を返します。
- `vmv.v.x` と `vsadd.vv` を実装しました。飽和の保持、VS のアクセス制御、`vstart`、マスク、グループ境界を処理します。テールと非アクティブ要素は保持します。
- 上流 Robot の VCSR 上位ビットと VSADD 飽和のケースを直接実行しました。Penv では Rust テストと、VSADD ケースから移植した ELF で `vcsr=1` と終了通知を確認しました。元の Robot のモニター・CLINT 等のフィクスチャとの差分があるため、移植は部分扱いです。

出典は固定 tlib `arch/riscv/helper.c`、`vector_helper.c`、`op_helper.c` と RISC-V の V 拡張仕様です。[RISC-V V 拡張](https://docs.riscv.org/reference/isa/unpriv/v-st-ext.html)

## 固定 ACT の結果と残課題

| 固定構成での Penv 結果 | ELF 数 |
| --- | ---: |
| 自己署名の合格と終了値 0 | 214 |
| 1 MiB の RAM 範囲外でロード拒否 | 26 |
| LR の非整列例外で終了通知なし | 2 |
| CPU 構成で拒否 | 0 |

214 本のうち 205 本は固定 Renode でも合格し、9 本は固定 Renode で CPU abort でした。上流の直接実行結果と Penv の結果は別列です。固定構成に一致する ISA 文字列を使用していても、V の未対応命令、ロード境界の処理差分、終了前の例外判定やステップ上限が残るため、上流等価の合格は保留です。

- 範囲外 26 本のうち 1 本は `Sm-00.sig.elf` で、通常の自己署名比較を行わない署名生成用です。ELF の物理セグメントと上流のロード処理の差分は [ロード範囲の調査](rv64-act-equivalence.md#1-mib-のロード範囲) にあります。
- `Zalrsc-sc.d-00.elf` と `Zalrsc-sc.w-00.elf` は例外 4 を返します。次の比較対象は固定 tlib のアラインメントと LR/SC 予約の意味です。
- `Sm-00.elf` を同じ ISA と監視で RAM だけ 4 MiB にした診断では終了値 1 でした。mstatus 全ビット書き込みの自己署名でビット 23 と 41 の差が残りました。この診断は固定構成の件数に含めていません。
- 一般のベクトルロード・ストア、残りの算術・順列命令、REPL の VLEN 変更は未実装です。

## 再現

`$act_work` は [固定 ACT の生成手順](rv64-act-and-failures.md) で作成した `work`、`$renode_work` はビルド済み固定 Renode です。`cargo test` は 110 件合格しました。ログと各プロセスの終了状態は `target/rv64-act-prepare/` に保存しました。

```sh
out=target/rv64-act-prepare
mkdir -p "$out"
cargo test
cargo build --release
python3 scripts/probe-rv64-isa.py --act --robot-isa "$act_work" \
  --logs "$out/robot" > "$out/robot.tsv"
python3 scripts/summarize-rv64-act.py docs/rv64-act-equivalence.tsv "$out" > "$out/act-cases.tsv"
cmp docs/rv64-act-prepare.tsv "$out/act-cases.tsv"

"$renode_work/renode-test" "$renode_work/tests/unit-tests/riscv-vector-csr.robot" \
  --fixture 'Should Reflect VSADD.VV Saturation In VCSR' --results-dir "$out/upstream-vsadd" -P 0
riscv-none-elf-gcc -march=rv64gcv -mabi=lp64d -nostdlib -static \
  -Wl,-n,-Ttext=0x80000000,-e,_start,--section-start=.tohost=0x80001000 \
  tests/fixtures/rv64-vector-bootstrap.S -o "$out/vector-bootstrap.elf"
target/release/penv run --platform "$out/robot/isa.repl" --robot-tohost 0x80001000 \
  --max-steps 1000 "$out/vector-bootstrap.elf"
```
