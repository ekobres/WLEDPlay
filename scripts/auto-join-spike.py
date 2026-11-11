#!/usr/bin/env python3
"""Manual Auto Join spike against a live AirPlay 2 network.

Not used by the operator UI. Proves whether pyatv can see a speaker start
playback and add this Shairport Sync receiver to that group.

Requires the `atvremote` CLI from pyatv 0.16.x on a Linux host that can
see both the speaker and WLEDPlay via mDNS:

    pip install 'pyatv==0.16.1'
    python3 scripts/auto-join-spike.py scan --storage /host-config/pyatv/pyatv.json
    python3 scripts/auto-join-spike.py watch --storage /host-config/pyatv/pyatv.json \\
        --target-id <speaker-id> --receiver-name WLEDPlay

Success is not the MRP return code. Confirm WLEDPlay logs "PCM stream
connected" (or the UI shows a live session). An iPhone-as-sender session
often fails here; that is a Join failed, not a broken receiver.
"""

from __future__ import annotations

import argparse
import os
import shutil
import subprocess
import sys
import time


def atvremote() -> str:
    path = shutil.which("atvremote")
    if not path:
        print(
            "atvremote not found. Install pyatv on the AirPlay host:\n"
            "  pip install 'pyatv==0.16.1'",
            file=sys.stderr,
        )
        sys.exit(2)
    return path


def run(cmd: list[str]) -> subprocess.CompletedProcess[str]:
    print("+", " ".join(cmd), flush=True)
    return subprocess.run(cmd, text=True, capture_output=True)


def settings_file(storage: str) -> str:
    """atvremote --storage-filename wants a file. A directory becomes pyatv.json inside it."""
    if not storage:
        return ""
    if storage.endswith((".json", ".conf")) and not os.path.isdir(storage):
        return storage
    return os.path.join(storage, "pyatv.json")


def base_args(storage: str) -> list[str]:
    cmd = [atvremote()]
    filename = settings_file(storage)
    if filename:
        cmd.extend(["--storage-filename", filename])
    return cmd


def cmd_scan(args: argparse.Namespace) -> int:
    proc = run(base_args(args.storage) + ["scan"])
    sys.stdout.write(proc.stdout)
    sys.stderr.write(proc.stderr)
    print(
        "\nPersist pyatv's identifier (prefer the AirPlay output device id).\n"
        "Do not persist the display name. Pair once with:\n"
        f"  atvremote --storage-filename {settings_file(args.storage) or '<file>'} wizard"
    )
    return proc.returncode


def playing_now(storage: str, target_id: str) -> bool:
    proc = run(base_args(storage) + ["--id", target_id, "playing"])
    text = (proc.stdout + proc.stderr).lower()
    sys.stdout.write(proc.stdout)
    if proc.returncode != 0:
        sys.stderr.write(proc.stderr)
        return False
    # pyatv prints "Device state: Playing" (also accept a bare Playing token).
    return "device state: playing" in text or "\nplaying\n" in f"\n{text}\n"


def cmd_watch(args: argparse.Namespace) -> int:
    print(
        f"Watching {args.target_id} for playback. Receiver id {args.receiver_id or '(lookup ' + args.receiver_name + ')'}.\n"
        "Start music on the speaker itself (HomePod / Apple TV as leader) for the reliable case."
    )
    while True:
        if playing_now(args.storage, args.target_id):
            receiver = args.receiver_id
            if not receiver:
                print(
                    "Target is playing. Re-run with --receiver-id set to WLEDPlay's "
                    "scan identifier (the AirPlay output id, not the display name).",
                    file=sys.stderr,
                )
                return 1
            proc = run(
                base_args(args.storage)
                + ["--id", args.target_id, f"add_output_devices={receiver}"]
            )
            sys.stdout.write(proc.stdout)
            sys.stderr.write(proc.stderr)
            if proc.returncode == 0:
                print(
                    "MRP add_output_devices returned. Auto Join is successful only if "
                    "WLEDPlay then receives PCM (Shairport: PCM stream connected)."
                )
            else:
                print(
                    "add_output_devices failed. If an iPhone is the session leader, "
                    "pyatv often cannot modify that group. Direct AirPlay to WLEDPlay "
                    "is unaffected."
                )
            return proc.returncode
        time.sleep(args.interval)


def main() -> int:
    parser = argparse.ArgumentParser(description="WLEDPlay Auto Join pyatv spike")
    parser.add_argument(
        "--storage",
        default="",
        help="pyatv settings file, or a directory that contains pyatv.json",
    )
    sub = parser.add_subparsers(dest="cmd", required=True)

    scan = sub.add_parser("scan", help="print AirPlay devices and identifiers")
    scan.set_defaults(func=cmd_scan)

    watch = sub.add_parser("watch", help="add WLEDPlay when the target starts playing")
    watch.add_argument("--target-id", required=True)
    watch.add_argument("--receiver-name", default="WLEDPlay")
    watch.add_argument("--receiver-id", default="")
    watch.add_argument("--interval", type=float, default=2.0)
    watch.set_defaults(func=cmd_watch)

    args = parser.parse_args()
    return args.func(args)


if __name__ == "__main__":
    raise SystemExit(main())
