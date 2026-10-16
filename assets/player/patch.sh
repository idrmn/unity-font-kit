#!/bin/sh
set -eu
action=$1
patch=$(cd "$(dirname "$0")" && pwd)
folder=$(head -n 1 "$patch/folder.txt" | tr -d '\r')
remembered="$patch/game.txt"
backup="$patch/backup"
decoder="$patch/xdelta3decode"
tab=$(printf '\t')

fail() {
    echo "$1"
    exit 1
}

sha() {
    if [ -f "$1" ]; then sha256sum "$1" | cut -d ' ' -f 1; fi
}

find_data() {
    saved=$(head -n 1 "$remembered" 2>/dev/null || true)
    # An install stopped while writing game.txt leaves it empty, and a moved game leaves a stale path.
    if [ -n "$saved" ] && [ -d "$saved" ]; then
        echo "$saved"
        return
    fi
    next="$(dirname "$patch")/$folder"
    if [ -d "$next" ]; then
        echo "$next"
        return
    fi
    echo "Cannot find the game folder next to this file." >&2
    echo "Drag the game folder, the one that has $folder inside, into this window, then press Enter." >&2
    printf 'Game folder: ' >&2
    read -r answer
    # Terminals quote a dragged path, or escape its spaces with \.
    answer=$(printf '%s' "$answer" | sed 's/^ *//; s/ *$//')
    case $answer in
    \"*|\'*) answer=$(printf '%s' "$answer" | sed "s/^['\"]//; s/['\"]\$//") ;;
    *) answer=$(printf '%s' "$answer" | sed 's/\\\(.\)/\1/g') ;;
    esac
    for candidate in "$answer/$folder" "$answer"; do
        if [ "$(basename "$candidate")" = "$folder" ] && [ -d "$candidate" ]; then
            echo "$candidate"
            return
        fi
    done
    echo "There is no $folder in $answer. Choose the folder of the game this patch is for." >&2
    return 1
}

move_checked() {
    if [ "$(sha "$1")" != "$2" ]; then
        rm -f "$1"
        fail "Could not write $3."
    fi
    # Next to the game file, so mv renames it in one step instead of copying across disks.
    mv -f "$1" "$3"
}

check_install() {
    wrong=""
    while IFS="$tab" read -r original patched delta file; do
        current=$(sha "$data/$file")
        if [ "$current" != "$original" ] && [ "$current" != "$patched" ]; then wrong="$wrong  $file
"; fi
    done < "$patch/files.tsv"
    if [ -n "$wrong" ]; then
        printf 'These game files are not the version this patch is for:\n%s' "$wrong"
        fail "Nothing was changed. Ask the author of the patch which game version it is for."
    fi
}

install_fonts() {
    data=$(find_data) || exit 1
    check_install
    chmod +x "$decoder"
    # Written first, so restore works even if the install stops halfway.
    echo "$data" > "$remembered"
    mkdir -p "$backup"
    while IFS="$tab" read -r original patched delta file; do
        [ "$(sha "$data/$file")" = "$original" ] || continue
        name=$(basename "$delta" .vcdiff)
        cp "$data/$file" "$backup/$name"
        "$decoder" -d -f -s "$data/$file" "$patch/$delta" "$data/$file.ufk-partial" < /dev/null
        move_checked "$data/$file.ufk-partial" "$patched" "$data/$file"
        echo "Patched $file"
    done < "$patch/files.tsv"
    report_unsaved
    echo "Done. Start the game and check the text."
}

report_unsaved() {
    unsaved=""
    while IFS="$tab" read -r original patched delta file; do
        name=$(basename "$delta" .vcdiff)
        if [ "$(sha "$data/$file")" = "$patched" ] && [ "$(sha "$backup/$name")" != "$original" ]; then
            unsaved="$unsaved  $file
"
        fi
    done < "$patch/files.tsv"
    if [ -n "$unsaved" ]; then
        printf 'These files were already patched, and ufk-patch has no copy of their originals:\n%s' "$unsaved"
        echo "Restore cannot put them back. To undo, let Steam or your game launcher check the game files."
    fi
}

check_restore() {
    changed=""
    while IFS="$tab" read -r original patched delta file; do
        current=$(sha "$data/$file")
        name=$(basename "$delta" .vcdiff)
        if [ "$current" != "$original" ] && { [ "$current" != "$patched" ] || [ "$(sha "$backup/$name")" != "$original" ]; }; then
            changed="$changed  $file
"
        fi
    done < "$patch/files.tsv"
    if [ -n "$changed" ]; then
        printf 'These game files were changed after the install, for example by a game update:\n%s' "$changed"
        fail "Nothing was changed. Let Steam or your game launcher check the game files instead."
    fi
}

restore_files() {
    [ -f "$remembered" ] || fail "The fonts are not installed, nothing to restore."
    data=$(find_data)
    check_restore
    while IFS="$tab" read -r original patched delta file; do
        [ "$(sha "$data/$file")" = "$patched" ] || continue
        name=$(basename "$delta" .vcdiff)
        cp "$backup/$name" "$data/$file.ufk-partial"
        move_checked "$data/$file.ufk-partial" "$original" "$data/$file"
        echo "Restored $file"
    done < "$patch/files.tsv"
    rm -rf "$backup" "$remembered"
    echo "Done. The game has its original files again."
}

command -v sha256sum > /dev/null || fail "This script needs the sha256sum command, which comes with coreutils."
if [ "$action" = install ]; then install_fonts; else restore_files; fi
