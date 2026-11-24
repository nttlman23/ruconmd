"""Показывает упавшие тесты в сводке запуска: журнал виден не всем."""

import re
import sys

LIMIT = 9  # GitHub показывает не больше 10 сообщений на шаг
LENGTH = 3000


def escape(text: str) -> str:
    return text.replace("%", "%25").replace("\r", "").replace("\n", "%0A")


def main() -> None:
    with open(sys.argv[1], encoding="utf-8", errors="replace") as log:
        text = log.read()
    blocks = re.findall(
        r"^---- (\S+) stdout ----\n(.*?)(?=^---- |^failures:|\Z)", text, re.M | re.S
    )
    for name, body in blocks[:LIMIT]:
        lines = [line.rstrip() for line in body.splitlines() if line.strip()]
        print("::error title=" + escape(name) + "::" + escape("\n".join(lines)[:LENGTH]))
    names = [name for name, _ in blocks]
    if len(names) > LIMIT or not names:
        tail = "\n".join(names) if names else text[-LENGTH:]
        print("::error title=all failures::" + escape(tail[:LENGTH]))


# В Windows вывод по умолчанию не UTF-8.
sys.stdout.reconfigure(encoding="utf-8")
main()
