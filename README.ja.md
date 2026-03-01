# box

[English](README.md)

[![Crates.io](https://img.shields.io/crates/v/box-cli)](https://crates.io/crates/box-cli)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
[![CI](https://github.com/yusukeshib/box/actions/workflows/ci.yml/badge.svg)](https://github.com/yusukeshib/box/actions/workflows/ci.yml)

隔離されたgitワークスペースとターミナルマルチプレクサ。クローンして、ブランチして、壊しても — 元のリポジトリは無傷。

![demo](./demo.gif)

## なぜ box？

Boxは**隔離された**gitワークスペースと**永続的な**ターミナルセッションを提供します。2つの核となるアイデア：

**1. 隔離されたgitワークスペース**

各セッションは独自のワークスペースを取得します。デフォルトでは `git clone --local` がハードリンクを使って完全に独立したリポジトリを作成します — 大きなリポジトリでも高速で、何をしても元のリポジトリには影響しません。また、`--strategy worktree` を使えば `git worktree` によるさらに高速で省スペースなワークスペースも利用できます。

**2. 内蔵ターミナルマルチプレクサによるセッション永続化**

すべてのセッションはスクロールバック、マウスサポート、永続的な接続を備えたターミナルマルチプレクサ内で実行されます。デタッチと再接続を自由に行えます — プロセスはバックグラウンドで実行され続けます。サイドバーで現在のワークスペースの全セッションをすばやく切り替えられます。

## 特徴

- **隔離されたgitワークスペース** — `git clone --local`（デフォルト）または `git worktree` でセッションごとにワークスペースを作成。ホストのファイルは変更されない
- **永続的なセッション** — `Ctrl+P` → `Ctrl+Q` でデタッチ、`box resume` で再接続。プロセスは実行され続ける
- **ターミナルマルチプレクサ** — スクロールバック履歴、マウススクロール、スクロールバー、ナビゲーション用COMMANDモード
- **マルチセッションワークスペース** — ワークスペースごとに複数セッションを実行（例: `my-feature/zsh`、`my-feature/server`）。サイドバーで素早く切り替え
- **Dockerモード** — 任意のDockerイメージによるオプションの完全コンテナ隔離（`BOX_MODE=docker`）

## 必要なもの

- [Git](https://git-scm.com/)
- [Docker](https://www.docker.com/)（macOSでは[OrbStack](https://orbstack.dev/)も可） — `BOX_MODE=docker` 使用時のみ必要

## インストール

### クイックインストール

```bash
curl -fsSL https://raw.githubusercontent.com/yusukeshib/box/main/install.sh | bash
```

### crates.ioから

```bash
cargo install box-cli
```

### ソースから

```bash
cargo install --git https://github.com/yusukeshib/box
```

### Nix

```bash
nix run github:yusukeshib/box
```

### バイナリダウンロード

ビルド済みバイナリは[GitHub Releases](https://github.com/yusukeshib/box/releases)ページからダウンロードできます。

## クイックスタート

```bash
box create my-feature
# 隔離されたgitワークスペースを作成し、その中でシェルを開く
# セッション名はコマンドから自動生成: my-feature/zsh
```

Boxはgitリポジトリ内で実行する必要があります。現在のリポジトリを `~/.box/workspaces/<name>/` にクローンします。

```bash
# 隔離されたワークスペースで作業...
$ git checkout -b experiment
$ make test  # 自由に壊してOK

# デタッチ（Ctrl+PでCOMMANDモードに入り、Ctrl+Q）
# プロセスはバックグラウンドで実行され続ける

# 後で再接続
box resume my-feature/zsh

# 完了？クリーンアップ
box remove my-feature
```

引数なしで `box` を実行すると、最初の実行中セッションを再開します。セッションがない場合は作成プロンプトが表示されます。

## ターミナルマルチプレクサ

すべてのboxセッションは内蔵ターミナルマルチプレクサ内で実行されます。これによりセッションの永続化、スクロールバック、キーボードナビゲーションが可能になります。

### COMMANDモード

`Ctrl+P`（設定変更可能）を押すとCOMMANDモードに入ります：

| キー | 動作 |
|-----|--------|
| `Ctrl+P` | 1行上にスクロール |
| `Ctrl+N` | 1行下にスクロール |
| `Ctrl+U` | 半ページ上にスクロール |
| `Ctrl+D` | 半ページ下にスクロール |
| `矢印キー` | 上下スクロール |
| `PgUp` / `PgDn` | 半ページスクロール |
| `Ctrl+Q` | boxを終了 |
| `Ctrl+X` | セッションを停止/キル |
| `A` | セッションサイドバーにフォーカス（Enterで切替、Escでキャンセル） |
| `N` | 現在のワークスペースに新しいセッションを作成 |
| `Esc` | COMMANDモードを終了（最下部にスナップ） |

マウススクロールは通常モードとCOMMANDモードの両方で動作します。スクロールバックコンテンツがある場合、スクロールバーが表示されます。

### セッションサイドバー

左側のサイドバーに現在のワークスペースの全セッションが表示されます。セッションをクリックして切り替えるか、`Ctrl+P` → `A` でキーボードナビゲーションによるサイドバーフォーカスが可能です。セッションが終了すると（例: シェルでCtrl+D）、同じワークスペース内の別の実行中セッションに自動的に切り替わります。

### プレフィックスキーの設定

COMMANDモードに入るキーは `~/.config/box/config.toml` で変更できます：

```toml
[mux]
prefix_key = "Ctrl+B"   # デフォルト: "Ctrl+P"
```

`Ctrl+A` から `Ctrl+Z` まで対応しています。

## セッション名

セッションは `ワークスペース/セッション` の命名規則を使用します：

```bash
box create my-feature                # → my-feature/zsh（コマンド名からセッション名を生成）
box create my-feature -- python      # → my-feature/python
box create my-feature/server -- node # → my-feature/server
```

複数のセッションがワークスペースを共有できます — それぞれ独自のターミナルを持ちますが、同じgitワークスペースディレクトリを使用します。

## 使い方

```bash
box                                               最初の実行中セッションを再開
box <name> [--local] [--docker] [--strategy <s>]  `box create <name>` のショートカット
box create [name] [--local] [--docker] [--strategy <s>] [options] [-- cmd...]  新しいセッションを作成
box resume <name> [-d] [--docker-args <args>]     既存のセッションを再開
box stop <name>                                   実行中のセッションを停止
box exec <name> -- <cmd...>                       実行中のセッションでコマンドを実行
box list [options]                                セッション一覧を表示（エイリアス: ls）
box remove <name>                                 セッションまたはワークスペースを削除
box cd <name>                                     ホストのプロジェクトディレクトリを表示
box path <name>                                   ワークスペースパスを表示
box origin                                        ワークスペースから元のプロジェクトディレクトリにcd
box config zsh|bash                               シェル補完を出力
box upgrade                                       最新版にアップグレード
```

### セッションの作成

```bash
# ショートカット: 名前を渡すだけ
box my-feature

# 対話型プロンプト（名前、コマンドを入力）
box create

# コマンドを指定して作成
box create my-feature -- make test

# 同じワークスペースに複数セッション
box create my-feature/server -- node server.js
box create my-feature/test -- make test

# cloneの代わりにgit worktreeを使用（より高速、オブジェクトストアを共有）
box create my-feature --strategy worktree
BOX_STRATEGY=worktree box my-feature

# デタッチモードで作成（バックグラウンド）
box create my-feature -d -- long-running-task
```

### セッションの再開

```bash
box resume my-feature

# デタッチモードで再開
box resume my-feature -d
```

### セッションの一覧と管理

```bash
box list                        # 全セッションを一覧表示
box ls                          # エイリアス
box list --running              # 実行中のセッションのみ
box list -q --running           # 名前のみ（スクリプト用途）
box stop my-feature             # セッションを停止
box remove my-feature           # セッション、ワークスペース、データを削除
box stop $(box list -q --running)  # 実行中の全セッションを停止
```

### ワークスペース間のナビゲーション

```bash
box cd my-feature               # ホストプロジェクトディレクトリを表示
cd "$(box path my-feature)"    # ワークスペースにcd
box origin                      # ワークスペースから元のプロジェクトにcd
```

## Dockerモード

完全なコンテナ隔離には `BOX_MODE=docker` を設定します。各セッションはワークスペースをバインドマウントしたDockerコンテナ内で実行されます。

```bash
export BOX_MODE=docker
```

オプションでカスタムイメージとデフォルトを設定：

```bash
export BOX_DEFAULT_IMAGE=mydev              # カスタムイメージ
export BOX_DOCKER_ARGS="--network host"     # 追加のDockerフラグ
export BOX_DEFAULT_CMD="bash"               # デフォルトコマンド
```

```bash
# 明示的なオプション付きDockerセッション
box create my-feature --docker --image ubuntu:latest -- bash
box create my-feature --docker --docker-args "-e KEY=VALUE -v /host:/container"
```

## オプション

### `box create`

| オプション | 説明 |
|--------|-------------|
| `-d` | バックグラウンドで実行（デタッチ） |
| `--local` | ローカルセッションを作成（デフォルト） |
| `--docker` | Dockerセッションを作成（Docker必要） |
| `--image <image>` | 使用するDockerイメージ（デフォルト: `alpine:latest`） |
| `--strategy <strategy>` | ワークスペース戦略: `clone`（デフォルト）または `worktree`。`$BOX_STRATEGY` を上書き |
| `--docker-args <args>` | 追加のDockerフラグ（例: `-e KEY=VALUE`、`-v /host:/container`）。`$BOX_DOCKER_ARGS` を上書き |
| `-- cmd...` | 実行するコマンド（デフォルト: `$BOX_DEFAULT_CMD` が設定されている場合はそれを使用） |

### `box list`

| オプション | 説明 |
|--------|-------------|
| `--running`, `-r` | 実行中のセッションのみ表示 |
| `--stopped`, `-s` | 停止中のセッションのみ表示 |
| `--quiet`, `-q` | セッション名のみ出力（スクリプト用途に便利） |

### `box resume`

| オプション | 説明 |
|--------|-------------|
| `-d` | バックグラウンドで再開（デタッチ） |
| `--docker-args <args>` | 追加のDockerフラグ。`$BOX_DOCKER_ARGS` を上書き |

## 環境変数

| 変数 | 説明 |
|----------|-------------|
| `BOX_DEFAULT_IMAGE` | 新規セッションのデフォルトDockerイメージ（デフォルト: `alpine:latest`） |
| `BOX_DOCKER_ARGS` | デフォルトの追加Dockerフラグ。`--docker-args` が指定されていない場合に使用 |
| `BOX_DEFAULT_CMD` | 新規セッションのデフォルトコマンド。`-- cmd` が指定されていない場合に使用 |
| `BOX_MODE` | セッションモード: `local`（デフォルト）または `docker` |
| `BOX_STRATEGY` | ワークスペース戦略: `clone`（デフォルト）または `worktree` |

## シェル補完

```bash
# Zsh (~/.zshrc)
eval "$(box config zsh)"

# Bash (~/.bashrc)
eval "$(box config bash)"
```

## 仕組み

```
your-repo/          box create my-feature         ~/.box/workspaces/my-feature/
  .git/        ──── git clone --local ────>         .git/  (独立)
  src/                                              src/   (ハードリンク)
  ...                                               ...
```

デフォルトでは、`git clone --local` がハードリンクを使って完全に独立したgitリポジトリを作成します。クローンは独自の `.git` ディレクトリを持つため、ワークスペース内でのコミット、ブランチ操作、リセット、破壊的操作が元のリポジトリに影響することはありません。

`--strategy worktree` を指定すると、代わりに `git worktree add --detach` を使用します。親リポジトリとオブジェクトストアを共有するため、ワークスペースの作成がより高速で省スペースになります。トレードオフとして、worktreeは親リポジトリとrefを共有します — 完全なgit隔離よりも軽量なワークスペースが必要な場合に使用してください。

内蔵ターミナルマルチプレクサは各セッションを以下の機能でラップします：
- **セッション永続化** — プロセスはバックグラウンドサーバーで実行され、中断なくデタッチ・再接続が可能
- **スクロールバック** — 10,000行の履歴をキーボードとマウスでナビゲーション
- **セッションサイドバー** — ワークスペース内の全セッションを表示し、素早く切り替え

| 項目 | 詳細 |
|--------|--------|
| ワークスペースの場所 | `~/.box/workspaces/<name>/` |
| セッションメタデータ | `~/.box/sessions/<name>/` |
| Git隔離 | `clone`（デフォルト）で完全隔離、`worktree` ではオブジェクトストアを共有 |
| セッション永続化 | マルチプレクササーバーがデタッチ・再接続をまたいでプロセスを維持 |
| クリーンアップ | `box remove` でワークスペース、セッションデータ、コンテナ（Docker時）を削除 |

## 設計上の判断

<details>
<summary><strong>なぜ <code>git clone --local</code> がデフォルト？</strong></summary>

| 戦略 | トレードオフ |
|------|-------------|
| **ホストリポジトリをバインドマウント** | 隔離なし — エージェントが実際のファイルを直接変更してしまう |
| **git worktree** | `.git` ディレクトリをホストと共有するため、checkout・reset・rebaseがホストのブランチやrefに影響する |
| **bare-gitマウント** | 状態を共有するため、コンテナ内でのブランチ作成・削除がホストに影響する |
| **ブランチのみの隔離** | 共有refに対する破壊的なgitコマンドを防げない |
| **完全コピー（`cp -r`）** | 完全に隔離されるが、大きなリポジトリでは遅い |

`git clone --local` は完全に独立（独自の `.git`）、高速（ハードリンク）、完全（全履歴）、シンプル（ラッパースクリプト不要）です。

なお、速度やディスク節約が完全な隔離よりも重要な場合には、`--strategy worktree` で `git worktree` を利用できます。

</details>

<details>
<summary><strong>なぜ内蔵マルチプレクサ？</strong></summary>

Boxにはセッション永続化 — 実行中のプロセスからデタッチして後で再接続する機能 — が必要です。tmuxやscreenを外部依存として要求する代わりに、専用のターミナルマルチプレクサを内蔵しています：

- 設定不要 — そのまま動作
- 全セッションで一貫したUI（サイドバー、スクロールバック、COMMANDモード）
- セッション永続化のためのクライアント-サーバーアーキテクチャを透過的に処理
- マウススクロールとビジュアルスクロールバーで出力履歴をナビゲーション
- マルチセッションサイドバーでワークスペースごとに複数ターミナルを実行

</details>

<details>
<summary><strong>なぜ素のDocker？</strong></summary>

一部のツールはDockerサンドボックスを組み込みで提供しています。Boxは素のDockerを直接使用することで、以下を実現しています：

- **自分のツールチェーンを使用** — 必要なツールを含む任意のDockerイメージを使用可能
- **完全なDocker制御** — カスタムネットワーク、ボリューム、環境変数、その他の `docker run` フラグが使用可能
- **任意のワークフローで動作** — 特定のツールやエージェントに縛られない

</details>

## Claude Code連携

Boxは[Claude Code](https://docs.anthropic.com/en/docs/claude-code)と組み合わせて、隔離されたワークスペースでAIエージェントを実行するのに適しています：

```bash
box create ai-experiment -- claude
box create ai-experiment -d -- claude -p "refactor the auth module"
```

エージェントが行うすべての操作はワークスペース内に留まります。完了したらセッションを削除すれば消えます。

## セキュリティに関する注意

`--docker-args` フラグと `BOX_DOCKER_ARGS` 環境変数は引数を `docker run` に直接渡します。`--privileged` や `-v /:/host` のようなフラグはコンテナのサンドボックスを弱める可能性があります。信頼できる値のみを使用してください。

## ライセンス

MIT
