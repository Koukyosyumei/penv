#!/usr/bin/env python3
"""Resume the two Renode Codex sessions when their TUI returns to an idle prompt."""

import re
import subprocess
import time
from datetime import datetime, timezone

TASKS = {
    "penv-renode-ci": (
        "Renode互換モニターの意味論をpenv本体へ移す実装を最優先で継続。"
        "まず変数・include・マシン選択・プラットフォーム起動を共通Rustセッションに移し、"
        "続いて仮想時間、UART/ログ待機、状態保存と復帰を本体へ寄せる。"
        "Robot側はRPCと引数・結果の変換を担当し、通常CLIと同じ実装を使う。"
        "固定Renodeとのコマンド列比較と未変更の代表Robotケースで検証し、区切りごとにコミット。"
    ),
    "penv-renode-nunit": (
        "固定上流NUnitの全対象ケースのRust移植を継続。未移植fixtureを選び、"
        "元の入力とアサーションを維持して公式IDを対応付け、区切りごとにコミット。"
        "全対象の移植が揃うまでNUnit全件の一括実行はせず、必要なfixtureだけ確認。"
    ),
}
CONTROL = {"/R" + "ENODE-DONE", "/R" + "ENODE-BLOCKED"}
INTERVAL = 10


def tmux(*args):
    return subprocess.run(["tmux", *args], text=True, capture_output=True)


def idle(screen):
    return (
        re.search(r"^\s*› Ask Codex to do anything\s*$", screen, re.M)
        and not re.search(
            r"esc to interrupt|Compacting context|Messages to be submitted|tab to queue message",
            screen,
        )
    )


def main():
    previous = {}
    submitted = {}
    stopped = set()
    for name in TASKS:
        print(f"Watching {name}", flush=True)
    while True:
        for name, task in TASKS.items():
            if name in stopped:
                continue
            target = f"{name}:0.0"
            pane = tmux("display-message", "-p", "-t", target, "#{pane_current_command}")
            captured = tmux("capture-pane", "-p", "-t", target)
            screen = captured.stdout
            if pane.returncode or pane.stdout.strip() != "codex" or captured.returncode or not idle(screen):
                previous.pop(name, None)
                continue
            if previous.get(name) != screen or submitted.get(name) == screen:
                previous[name] = screen
                continue
            if CONTROL.intersection(line.strip() for line in screen.splitlines()):
                stopped.add(name)
                print(f"{datetime.now(timezone.utc).isoformat()} {name}: terminal marker", flush=True)
                continue
            prompt = (
                task + " 作業の区切りの報告だけで止まらず次の項目へ進めてください。"
                "全対象の完了を証明した場合、または人の対応が必要な外部ブロッカーがあり"
                "独立に進められる作業も残っていない場合のみ、最終行をそれぞれ "
                "/RENODE-DONE または /RENODE-BLOCKED としてください。"
            )
            if tmux("send-keys", "-t", target, "-l", prompt).returncode == 0:
                time.sleep(1)
                result = tmux("send-keys", "-t", target, "Enter")
                if result.returncode == 0:
                    submitted[name] = screen
                    print(f"{datetime.now(timezone.utc).isoformat()} {name}: continuation submitted", flush=True)
        if len(stopped) == len(TASKS):
            return
        time.sleep(INTERVAL)


if __name__ == "__main__":
    main()
