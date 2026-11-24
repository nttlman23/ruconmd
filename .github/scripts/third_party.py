"""Собирает лицензии всего, что попадает в исполняемый файл для заданной цели."""

import json
import os
import subprocess
import sys

NAMES = ("LICENSE", "LICENCE", "COPYING", "UNLICENSE", "NOTICE", "COPYRIGHT")
MIT = open("LICENSE-MIT", encoding="utf-8").read().split("\n\n", 2)[2]


def output(*command: str) -> str:
    return subprocess.run(
        command, check=True, stdout=subprocess.PIPE, encoding="utf-8"
    ).stdout


def license_files(folder: str) -> list[str]:
    found = [n for n in sorted(os.listdir(folder)) if n.upper().startswith(NAMES)]
    return [os.path.join(folder, n) for n in found if os.path.isfile(os.path.join(folder, n))]


def read(path: str) -> str:
    with open(path, encoding="utf-8", errors="replace") as file:
        return file.read().replace("\r\n", "\n").strip()


def main(target: str, destination: str) -> None:
    metadata = json.loads(
        output("cargo", "metadata", "--format-version", "1", "--locked", "--filter-platform", target)
    )
    packages = {(p["name"], p["version"]): p for p in metadata["packages"]}
    tree = output(
        "cargo", "tree", "--locked", "-e", "normal", "--target", target, "--prefix", "none", "-f", "{p}"
    )
    used = set()
    for line in tree.splitlines():
        words = line.split()
        if len(words) >= 2:
            used.add((words[0], words[1].removeprefix("v")))

    parts = [
        "ruconmd is distributed under MIT OR Apache-2.0 (LICENSE-MIT, LICENSE-APACHE).",
        "The executable also contains the following third-party software.",
        "",
        "Rust standard library (MIT OR Apache-2.0), The Rust Project Contributors.",
    ]
    if "musl" in target:
        parts.append("musl libc (MIT), Copyright (c) 2005-2020 Rich Felker, et al.")
    missing = []
    for key in sorted(used):
        package = packages[key]
        if package["name"] == "ruconmd":
            continue
        rule = "=" * 78
        parts += ["", rule, f"{key[0]} {key[1]}", f"License: {package['license']}"]
        if package.get("repository"):
            parts.append(f"Source: {package['repository']}")
        files = license_files(os.path.dirname(package["manifest_path"]))
        for path in files:
            parts += ["", f"--- {os.path.basename(path)}", read(path)]
        if files:
            continue
        # В пакете нет текста лицензии: берём общий текст и авторов.
        authors = ", ".join(package["authors"]) or f"the {key[0]} authors"
        if "MIT" in package["license"]:
            parts += ["", f"Copyright (c) {authors}", "", MIT.strip()]
        elif package["license"] == "BSL-1.0":
            boost = next(p for (n, _), p in packages.items() if n == "error-code")
            text = read(license_files(os.path.dirname(boost["manifest_path"]))[0])
            parts += ["", f"Copyright (c) {authors}", "", text]
        else:
            missing.append(key)
    if missing:
        sys.exit(f"no license text: {missing}")

    syntaxes = output("cargo", "run", "--quiet", "--locked", "--example", "acknowledgements")
    if not syntaxes.strip():
        sys.exit("no acknowledgements for syntax definitions")
    parts += ["", "=" * 78, "Syntax definitions embedded through two-face", "", syntaxes.strip(), ""]
    with open(destination, "w", encoding="utf-8", newline="\n") as file:
        file.write("\n".join(parts))
    print(f"{len(used) - 1} packages, {os.path.getsize(destination)} bytes")


main(sys.argv[1], sys.argv[2])
