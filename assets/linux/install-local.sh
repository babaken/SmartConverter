#!/bin/sh
# SmartConverter をこのユーザーだけに入れる（~/.local）。
#
# 配布物（tar.gz）を展開したフォルダで：
#   sh install.sh
#   sh install.sh --uninstall
# リポジトリで（ビルドしたものを入れる）：
#   sh assets/linux/install-local.sh [ビルドしたフォルダ]   （省略時は target/release）
#   sh assets/linux/install-local.sh --uninstall
#
# 入れるもの：
#   ~/.local/bin/SmartConverter、~/.local/bin/smartconverter-cli
#   ~/.local/bin/Transcoding.ini（配布物から入れるとき、まだ無ければ。プリセット例。
#     前からあって中身が違うときは、上書きせずに Transcoding.ini.new として置く）
#   ~/.local/share/applications/smartconverter.desktop
#   ~/.local/share/icons/hicolor/<大きさ>/apps/smartconverter.png
# Wayland ではウィンドウのアイコンが .desktop とアイコンから決まるので、GUI を確かめる前に入れておく。
set -eu

here=$(cd "$(dirname "$0")" && pwd)
bin_dir="$HOME/.local/bin"
app_dir="$HOME/.local/share/applications"
icon_root="$HOME/.local/share/icons/hicolor"
sizes="16 24 32 48 64 128 256 512"

# 配布物を展開したフォルダか、リポジトリの assets/linux か
if [ -d "$here/share/applications" ]; then
    dist=1
    desktop="$here/share/applications/smartconverter.desktop"
    icon() { echo "$here/share/icons/hicolor/${1}x${1}/apps/smartconverter.png"; }
else
    dist=0
    root=$(cd "$here/../.." && pwd)
    desktop="$here/smartconverter.desktop"
    icon() { echo "$root/assets/icon/png/smartconverter-$1.png"; }
fi

refresh() {
    command -v update-desktop-database >/dev/null 2>&1 && update-desktop-database -q "$app_dir" || true
    command -v gtk-update-icon-cache >/dev/null 2>&1 && gtk-update-icon-cache -q -t "$icon_root" || true
    # KDE Plasma のメニューを作り直す
    command -v kbuildsycoca6 >/dev/null 2>&1 && kbuildsycoca6 >/dev/null 2>&1 || true
}

if [ "${1:-}" = "--uninstall" ]; then
    rm -f "$bin_dir/SmartConverter" "$bin_dir/smartconverter-cli" "$app_dir/smartconverter.desktop" \
        "$bin_dir/Transcoding.ini.new"
    for s in $sizes; do
        rm -f "$icon_root/${s}x${s}/apps/smartconverter.png"
    done
    refresh
    echo "削除しました（設定 ~/.config/smartconverter/ と ~/.local/bin/Transcoding.ini は残しています）"
    exit 0
fi

if [ "$dist" = 1 ]; then
    build="$here"
else
    build=${1:-"$root/target/release"}
fi
for b in SmartConverter smartconverter-cli; do
    if [ ! -x "$build/$b" ]; then
        echo "見つかりません: $build/$b（リポジトリでは先に cargo build --release を実行してください）" >&2
        exit 1
    fi
done

mkdir -p "$bin_dir" "$app_dir"
install -m 755 "$build/SmartConverter" "$build/smartconverter-cli" "$bin_dir/"
if [ "$dist" = 1 ]; then
    if [ ! -e "$bin_dir/Transcoding.ini" ]; then
        install -m 644 "$here/Transcoding.ini" "$bin_dir/"
        echo "プリセット例を入れました: $bin_dir/Transcoding.ini"
    elif ! cmp -s "$here/Transcoding.ini" "$bin_dir/Transcoding.ini"; then
        # 自分で編集したプリセットかもしれないので上書きしない。新しい版の例は隣に置く。
        install -m 644 "$here/Transcoding.ini" "$bin_dir/Transcoding.ini.new"
        echo "※ $bin_dir/Transcoding.ini は前からあるので、そのままにしました。"
        echo "   この版のプリセット例は $bin_dir/Transcoding.ini.new です（使うときは名前を変えてください）"
    fi
fi
if [ -e "$bin_dir/SmartConverter.ini" ]; then
    # 実行ファイルの隣に設定ファイルがあると、ポータブルとして扱われる（設定とログがそこに置かれる）。
    echo "※ $bin_dir/SmartConverter.ini があるので、設定とログはそのフォルダに置かれます。"
    echo "   ~/.config/smartconverter/ を使うには、このファイルを移すか消してください"
fi
install -m 644 "$desktop" "$app_dir/"
for s in $sizes; do
    mkdir -p "$icon_root/${s}x${s}/apps"
    install -m 644 "$(icon "$s")" "$icon_root/${s}x${s}/apps/smartconverter.png"
done
refresh

echo "入れました: $bin_dir/SmartConverter"
case ":$PATH:" in
    *":$bin_dir:"*) ;;
    *) echo "※ $bin_dir が PATH に入っていません。.desktop から起動するには PATH に加えてください" ;;
esac
