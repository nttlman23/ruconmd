#!/bin/sh
# Установка ruconmd в каталог пользователя, без прав суперпользователя.
#
#   install.sh [--version X.Y.Z] [--dir КАТАЛОГ] [--from КАТАЛОГ] [--no-path]
#   install.sh --uninstall [--dir КАТАЛОГ]
#
# --from берёт архив и SHA256SUMS из каталога, без сети.

set -eu

REPO=nttlman23/ruconmd
PLATFORM=linux-x86_64
MARK='# ruconmd: PATH'

say() { printf '%s\n' "$*"; }
fail() { printf 'install.sh: %s\n' "$*" >&2; exit 1; }
need() { command -v "$1" >/dev/null 2>&1; }

# Загрузить $1 в файл $2, только по HTTPS.
fetch() {
    if need curl; then
        curl --proto '=https' --tlsv1.2 -fsSL --retry 2 -o "$2" "$1"
    elif need wget; then
        wget -q --https-only -O "$2" "$1"
    else
        fail 'curl or wget is required'
    fi
}

# Последний окончательный выпуск, а пока таких нет, последний предварительный.
latest() {
    api="https://api.github.com/repos/$REPO/releases"
    fetch "$api?per_page=30" "$tmp/releases" || fail "cannot read $api"
    tags=$(sed 's/,/\
/g' "$tmp/releases" | sed -n 's/.*"tag_name" *: *"v\([0-9A-Za-z.-]*\)".*/\1/p')
    stable=$(printf '%s\n' "$tags" | grep -v -- - | head -n 1 || true)
    if [ -n "$stable" ]; then
        printf '%s' "$stable"
    else
        printf '%s\n' "$tags" | head -n 1
    fi
}

add_path() {
    case ":$PATH:" in *":$dir:"*) return ;; esac
    case "$dir" in *[!A-Za-z0-9_./+@%-]*)
        say "Add $dir to PATH yourself: its name has unusual characters."
        return ;;
    esac
    profile=$HOME/.profile
    case "${SHELL:-}" in */zsh) profile=$HOME/.zprofile ;; esac
    # ~/.bash_profile не даёт bash читать ~/.profile.
    case "${SHELL:-}" in */bash)
        if [ -f "$HOME/.bash_profile" ]; then profile=$HOME/.bash_profile; fi ;;
    esac
    if ! grep -qF "$MARK" "$profile" 2>/dev/null; then
        printf '\nexport PATH="%s:%s" %s\n' "$dir" "\$PATH" "$MARK" >>"$profile"
    fi
    say "Added $dir to PATH in $profile, log in again to use it."
}

remove_path() {
    for profile in "$HOME/.profile" "$HOME/.zprofile" "$HOME/.bash_profile"; do
        if [ -f "$profile" ] && grep -qF "$MARK" "$profile"; then
            # Содержимое переписывается на место: файл может быть ссылкой.
            grep -vF "$MARK" "$profile" >"$tmp/profile" || true
            cat "$tmp/profile" >"$profile"
            say "Removed PATH from $profile"
        fi
    done
}

uninstall() {
    rm -f "$dir/ruconmd"
    rm -rf "$docs"
    remove_path
    say "Removed $dir/ruconmd. Settings and recovery copies are kept."
}

main() {
    version='' dir="${HOME:?}/.local/bin" from='' path=yes action=install
    while [ $# -gt 0 ]; do
        case "$1" in
            --version | --dir | --from)
                [ $# -ge 2 ] || fail "$1 needs a value"
                case "$1" in
                    --version) version=${2#v} ;;
                    --dir) dir=$2 ;;
                    --from) from=$2 ;;
                esac
                shift 2 ;;
            --no-path) path=no; shift ;;
            --uninstall) action=uninstall; shift ;;
            *) fail "unknown option $1" ;;
        esac
    done
    case "$dir" in /*) ;; *) dir=$PWD/$dir ;; esac
    docs="${XDG_DATA_HOME:-$HOME/.local/share}/doc/ruconmd"

    tmp=$(mktemp -d)
    trap 'rm -rf "$tmp"' EXIT
    trap 'exit 1' HUP INT TERM

    if [ "$action" = uninstall ]; then
        uninstall
        return
    fi
    case "$(uname -s)-$(uname -m)" in
        Linux-x86_64 | Linux-amd64) ;;
        *) fail "no build for $(uname -s) $(uname -m), only Linux x86_64" ;;
    esac
    need sha256sum || fail 'sha256sum is required'
    need tar || fail 'tar is required'

    if [ -z "$version" ]; then
        [ -z "$from" ] || fail '--from needs --version'
        version=$(latest)
    fi
    case "$version" in
        '' | *[!0-9A-Za-z.-]*) fail "bad version '$version'" ;;
    esac
    name=ruconmd-v$version-$PLATFORM
    archive=$name.tar.gz
    if [ -n "$from" ]; then
        cp "$from/$archive" "$from/SHA256SUMS" "$tmp/"
    else
        base=https://github.com/$REPO/releases/download/v$version
        say "Downloading ruconmd $version"
        fetch "$base/$archive" "$tmp/$archive" || fail "cannot download $base/$archive"
        fetch "$base/SHA256SUMS" "$tmp/SHA256SUMS" || fail "cannot download $base/SHA256SUMS"
    fi

    awk -v f="$archive" '$2 == f || $2 == "*" f' "$tmp/SHA256SUMS" >"$tmp/sum"
    [ "$(wc -l <"$tmp/sum")" -eq 1 ] || fail "no checksum for $archive"
    (cd "$tmp" && sha256sum -c sum >/dev/null 2>&1) || fail 'checksum mismatch, nothing is installed'

    # Извлекается только ожидаемое, имена заданы здесь.
    (cd "$tmp" && tar xzf "$archive" "$name")
    program=$tmp/$name/ruconmd
    { [ -f "$program" ] && [ ! -L "$program" ]; } || fail 'no program in the archive'

    mkdir -p "$dir" "$docs"
    # Рядом и переименованием: работающую программу нельзя перезаписать.
    cp "$program" "$dir/.ruconmd.new"
    chmod 755 "$dir/.ruconmd.new"
    mv -f "$dir/.ruconmd.new" "$dir/ruconmd"
    for file in LICENSE-MIT LICENSE-APACHE THIRD-PARTY.txt README.md CHANGELOG.md config.example.toml; do
        if [ -f "$tmp/$name/$file" ]; then cp "$tmp/$name/$file" "$docs/"; fi
    done
    say "Installed $dir/ruconmd ($("$dir/ruconmd" --version))"
    if [ "$path" = yes ]; then add_path; fi
}

# Вызов в последней строке: недокачанный скрипт не выполнит ничего.
main "$@"
