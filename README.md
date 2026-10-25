# dugrow

*Disk usage growth tracker — find the directories that GREW, not just the ones that are big.*

ディスク容量の増加を追跡する小さな CLI ツールです。

## なにこれ

`du` や `ncdu` は「今、大きいフォルダ」を教えてくれます。でもディスクが満杯になったときに本当に知りたいのはそれじゃなくて、**「最近太ったフォルダはどこか」**のはずです。

キャッシュが静かに膨らんでいた、ログがローテーションされていなかった、ダウンロードしたままのファイルが溜まっていた——原因はいつも「最近増えたもの」の中にあります。

dugrow はディレクトリサイズのスナップショットを SQLite に保存しておき、過去と比較して「増えた順」に表示します。

## インストール

Rust のツールチェーンがあれば:

```sh
cargo build --release
# バイナリは target/release/dugrow
cp target/release/dugrow ~/.local/bin/
```

## 使い方

まずスナップショットを撮ります:

```sh
dugrow scan /home/you          # 対象は複数指定可
dugrow scan / /home /var --min-size 10MiB   # 小さいフォルダは記録しない
```

しばらく経ってから（次のスナップショットを撮ってから）増加分を見ます:

```sh
dugrow growth            # 直近2回のスナップショットを比較
dugrow growth --days 7   # 7日以上前のスナップショットと比較
dugrow growth --top 5    # 上位5件だけ
```

出力はこんな感じです:

```
        GREW      SIZE NOW  PATH
    +5.0 MiB       5.0 MiB  /home/you/.cache/something
  +120.3 MiB     812.0 MiB  /home/you/Downloads
```

その他のコマンド:

```sh
dugrow top          # 最新スナップショットで大きいフォルダ
dugrow snapshots    # 保存済みスナップショットの一覧
```

スナップショット DB は既定で `~/.local/share/dugrow/dugrow.db` に作られます。`--db <path>` で変更できます。

## 定期実行

cron の例（毎日深夜に自宅ディレクトリを撮る）:

```cron
0 3 * * *  /home/you/.local/bin/dugrow scan /home/you --min-size 1MiB >/dev/null
```

systemd timer の例:

```ini
# ~/.config/systemd/user/dugrow-scan.service
[Unit]
Description=dugrow snapshot

[Service]
Type=oneshot
ExecStart=%h/.local/bin/dugrow scan %h --min-size 1MiB

# ~/.config/systemd/user/dugrow-scan.timer
[Unit]
Description=Take a dugrow snapshot daily

[Timer]
OnCalendar=daily
Persistent=true

[Install]
WantedBy=timers.target
```

```sh
systemctl --user enable --now dugrow-scan.timer
```

## 仕様メモ

- シンボリックリンクは辿りません
- 読めないディレクトリは警告を出してスキップします（落としません）
- サイズはディレクトリの累積値（配下すべてを含む）です
- `--min-size` には `500KB`（1000進）や `1MiB`（1024進）のような表記が使えます

## ライセンス

MIT
