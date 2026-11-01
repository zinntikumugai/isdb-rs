# isdb rx（ISDB-S3 ストリーム受信）のパイプライン化 — 設計

日付: 2026-11-01 / ブランチ: `perf/rx-pipeline`（`local-build` から分岐）

## 背景

`isdb rx --symbols FILE`（words ファイル → TLV）は、32 vCPU の VM（tv-enc,
Xeon Gold 6140）で 1 プロセス 0.40× 実時間、CPU 使用率は全体の 2〜3 割に
とどまった。4 コアの開発機で 3.9 s 分を復号すると wall 22.2 s / CPU 63.5 s
（286 %）で、Amdahl で分けると逐次 ≈ 8 s・並列 ≈ 55 CPU 秒。32 コアに
当てはめると 0.39×・使用率 20 % となり、tv-enc の観測と合う。

原因は `symstream::run` → `rxs3::decode` → `rx::receive` の構造にある。
24 フレーム（≈ 0.8 s）のチャンクごとに

1. 逐次: derotate、slot sync（`synchronise_at`）×2、TMCC と等化器の学習
2. 並列: 等化（`eq::apply`）、フレームごとの下ごしらえ（`prepare`）
3. 並列: デマップ + LDPC（`decode_frames`）
4. 逐次: TLV 組み立て・書き出し

を実行し、チャンクの間に fork-join の区切りがある。逐次部分の間は他の
コアが全部待つ。

upstream `c4a39d2`（2026-11-01 取り込み）で derotate の並列化、
`s3sync::measure_with` の入力コピー廃止、出力のバックグラウンド書き出しが
入った（当初設計の「段階 1」にあたる）。本設計はその上に段階 0 と段階 2 を
載せる。

## 目標と合格条件

- 1 プロセスあたりの速度と CPU 使用率を上げる（tv-enc で 0.8× 以上が目安。
  推定値であり、段階 0 の計測で見直す）。
- 出力 TLV が変更前とバイト単位で一致すること。集計行（フレーム数、TMCC、
  スロット数、LDPC/BCH 失敗、MER、TLV パケット数、lost slots）は所要時間を
  除いて一致すること。
- 基準ファイル: BS-14 の 1 分（`20261101-003935_BS14_1m`）、10 分
  （`20261101-004046_BS14_10m`、595–598 s に LDPC 失敗 15）。基準出力は
  upstream 取り込み後・変更前のバイナリで作る。
- ライブ（`--radio … --offload`）の経路の挙動は変えない（同じ `run_stream`
  を通るので、順序・ロス処理・Ctrl-C の意味を保つ）。
- メモリ増加は 1 GB 以内。f64 → f32 は出力が変わるので対象外。

## 段階 0: 計測

`rxs3::Stats` に段階別の累積時間を持たせ、最後の `time:` 行の後に 1 行
出す（info）。

- `front`: derotate + sync + TMCC/等化学習 + 等化 + 再 sync + prepare
- `ldpc`: noise_of + `decode_frames`
- `emit`: TLV 組み立てと書き出し（`decode` の後半）
- `wait`: 復号側がパーサを待った時間（`symstream::run` の受信ループ）

`rx::receive` 内の時間は `Rx` に `front_s` / `back_s` として返す（ログの
ためのフィールドで、出力には影響しない）。tv-enc で 1 回計測してもらい、
段階 2 の効果見込みを数字で確かめる。

## 段階 2: パイプライン化

### 受信処理の分割（isdbs3-phy）

- `rx::receive_front(z, opt) -> Front`: 現在の `receive` の sync から
  `ready` / `order` の構築まで。`Front` はチャンクの解析結果を**所有**する
  （等化済みシンボルは `prepare` が切り出した `ms` / `pil` として各フレームが
  持つので、`z` への借用は残らない）。`frame_starts`、`residual_hz`、
  `sync_snr_db`、`sync_snr_eq_db`、`taps` もここで決まる。
- `rx::receive_back(front, opt) -> Rx`: `pts` / `noise_of` / `decode_frames`
  と `Rx` の組み立て。
- `receive(z, opt) = receive_back(receive_front(z, opt), opt)`。他の呼び出し
  元（`isdb rx --iq`、例、テスト）はそのまま。

### ストリーム側（isdb-cli `rxs3::run_stream`）

- 現在のデコードクロージャを 2 段に分ける。
  - 前段（`symstream::run` を呼ぶスレッド）: `receive_front` を実行し、
    切り口 `upto`、`aligned`、`tracked`（搬送波追従）をその場で更新して
    `used` を返す。`Front` と `keep_last` 情報を深さ 1 の `sync_channel` で
    後段へ送る。
  - 後段（新しいスレッド）: `receive_back`、統計の加算、TLV 組み立て、
    書き出しをチャンク順に行う。
- 区間の終わり（ロス・ロック喪失・終端での `close(asm)`）は同じチャネルに
  `End` メッセージとして流し、後段が順番どおりに処理する。
- 状態表示（status コールバック）の数字は後段が更新する `Stats` を
  `Mutex` 越しに読む。`residual_hz` / `sync_snr_db` / `tracked_hz` は
  前段で書く。
- 失敗時: 後段のエラー（書き込み失敗など）はチャネルを閉じ、前段の次の
  送信がエラーになって `run` が `Err` で終わる。最終的なエラーは後段の
  `join` の結果を優先する。後段の panic は再送出する。Ctrl-C は現在どおり
  前段のループで止まり、後段は送られた分を処理して終わる。
- メモリ: 処理中のチャンクが 1 つ増える（`Front` 1 個 ≈ 0.5 GB）。

### 対象外

- LDPC のスレッドをチャンクをまたいで使い回す共有プール化（段階 2 の結果を
  見て判断）。
- 1 プロセス内でのファイル分割並列（途中開始時の捕捉の不具合を先に直す
  必要があり、別件）。

## 試験

- 単体: 合成・実データの短いチャンクで `receive(z)` と
  `receive_back(receive_front(z))` の `Rx` が等しい（`Rx` に `PartialEq`
  がなければ比較用の要約で比べる）。
- 結合: 基準ファイル 2 本の TLV を `cmp`、集計行を比較。
- 既存の `cargo test`（isdbs3-phy, isdb-cli）と clippy が通る。
- 速度: 開発機（4 コア）で wall / CPU% を前後比較、tv-enc で利用者が計測。
