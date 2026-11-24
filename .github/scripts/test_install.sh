#!/bin/sh
# Проверяет install.sh в отдельном домашнем каталоге, без сети.
# Неудача выводится так, чтобы GitHub показал её в сводке.
# Первый аргумент: оболочка, которой выполнять (sh, dash, bash).

set -eu

shell=${1:-sh}
script=$(cd "$(dirname "$0")/../.." && pwd)/install.sh
root=$(mktemp -d)
trap 'rm -rf "$root"' EXIT
export HOME="$root/home" SHELL=/bin/bash
unset XDG_DATA_HOME
mkdir "$HOME" "$root/dist"

check() { eval "$2" || { echo "::error::FAILED: $1"; exit 1; }; echo "ok: $1"; }
refuses() {
    title=$1; shift
    if "$shell" "$script" "$@" >"$root/out" 2>&1; then
        cat "$root/out"; echo "::error::FAILED: $title"; exit 1
    fi
    echo "ok: $title"
}

# Выпуск версии $1 из поддельной программы.
release() {
    name=ruconmd-v$1-linux-x86_64
    mkdir "$root/dist/$name"
    printf '#!/bin/sh\necho ruconmd %s\n' "$1" >"$root/dist/$name/ruconmd"
    chmod 755 "$root/dist/$name/ruconmd"
    echo license >"$root/dist/$name/LICENSE-MIT"
    (cd "$root/dist" && tar czf "$name.tar.gz" "$name" && sha256sum ./*.tar.gz | sed 's|\./||' >SHA256SUMS)
}
release 1.0.0
release 1.1.0

bin=$HOME/.local/bin/ruconmd
install() { "$shell" "$script" --from "$root/dist" "$@" >"$root/out"; }

install --version 1.0.0
check 'installs' '[ "$("$bin")" = "ruconmd 1.0.0" ]'
check 'license is kept' '[ -f "$HOME/.local/share/doc/ruconmd/LICENSE-MIT" ]'
check 'PATH is added' 'HOME="$HOME" sh -c ". ~/.profile; command -v ruconmd" | grep -qx "$bin"'
install --version v1.1.0
check 'updates' '[ "$("$bin")" = "ruconmd 1.1.0" ]'
check 'PATH is added once' '[ "$(grep -c "ruconmd: PATH" "$HOME/.profile")" = 1 ]'
check 'no leftovers' '[ "$(ls -A "$HOME/.local/bin")" = ruconmd ]'

echo '# user line' >>"$HOME/.profile"
"$shell" "$script" --uninstall >"$root/out"
check 'uninstalls' '[ ! -e "$bin" ] && [ ! -e "$HOME/.local/share/doc/ruconmd" ]'
check 'own lines only' '! grep -q ruconmd "$HOME/.profile" && grep -q "user line" "$HOME/.profile"'

install --version 1.0.0 --dir "$root/other place" --no-path
check 'other folder' '[ "$("$root/other place/ruconmd")" = "ruconmd 1.0.0" ]'
check 'PATH is left alone' '! grep -q ruconmd "$HOME/.profile"'

printf x >>"$root/dist/ruconmd-v1.1.0-linux-x86_64.tar.gz"
refuses 'damaged archive' --from "$root/dist" --version 1.1.0
check 'nothing is installed' '[ ! -e "$bin" ]'
sed -i /1.0.0/d "$root/dist/SHA256SUMS"
refuses 'missing checksum' --from "$root/dist" --version 1.0.0
refuses 'strange version' --from "$root/dist" --version '../x'
refuses 'unknown option' --wrong
refuses 'option without a value' --dir
