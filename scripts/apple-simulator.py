#!/usr/bin/env python3
"""Pick and boot an Apple simulator for the no-Mac CI test gate.

Usage:
    python3 scripts/apple-simulator.py pick iphone
    python3 scripts/apple-simulator.py pick ipad
    python3 scripts/apple-simulator.py pick appletv
    python3 scripts/apple-simulator.py boot iphone   # boot + print UDID

Only the UDID is written to stdout, so callers can use command
substitution. `pick` chooses the newest available runtime that has a
matching device and prefers the current default models (iPhone 16, iPad
Pro 13-inch, Apple TV 4K). Run `xcrun simctl list devices available`
to see what this Xcode installation provides.
"""

from __future__ import annotations

import json
import re
import subprocess
import sys


PREFERRED = {
    "iphone": ["iPhone 16", "iPhone 16 Pro", "iPhone 15"],
    "ipad": ["iPad Pro 13-inch (M4)", "iPad Pro 13-inch", "iPad Air 13-inch (M2)"],
    "appletv": ["Apple TV 4K (3rd generation)", "Apple TV 4K (2nd generation)", "Apple TV"],
}

DEVICE_MARKERS = {
    "iphone": "iPhone",
    "ipad": "iPad",
    "appletv": "AppleTV",
}


def run(*args: str, quiet: bool = False) -> str:
    result = subprocess.run(
        args,
        check=True,
        text=True,
        stdout=subprocess.PIPE if quiet else None,
        stderr=subprocess.PIPE if quiet else None,
    )
    return result.stdout if quiet else ""


def runtime_rank(runtime: str) -> tuple[int, ...]:
    """Order runtimes by their version digits (iOS-18-5 > iOS-18-4)."""
    numbers = [int(part) for part in re.findall(r"\d+", runtime)]
    return tuple(numbers) if numbers else (0,)


def preferred_rank(kind: str, name: str) -> int:
    for index, preferred in enumerate(PREFERRED[kind]):
        if name == preferred:
            return index
    return len(PREFERRED[kind])


def candidates(kind: str) -> list[dict[str, str]]:
    raw = run("xcrun", "simctl", "list", "devices", "available", "-j", quiet=True)
    data = json.loads(raw)
    marker = DEVICE_MARKERS[kind]
    found: list[dict[str, str]] = []
    for runtime, devices in data.get("devices", {}).items():
        for device in devices:
            if not device.get("isAvailable", True):
                continue
            device_type = device.get("deviceTypeIdentifier", "")
            if marker not in device_type:
                continue
            found.append(
                {
                    "udid": device["udid"],
                    "name": device.get("name", ""),
                    "runtime": runtime,
                }
            )
    found.sort(
        key=lambda item: (
            tuple(-value for value in runtime_rank(item["runtime"])),
            preferred_rank(kind, item["name"]),
            item["name"],
        )
    )
    return found


def pick(kind: str) -> dict[str, str]:
    if kind not in DEVICE_MARKERS:
        raise SystemExit(f"unknown simulator kind {kind!r}; use iphone, ipad or appletv")
    found = candidates(kind)
    if not found:
        available = run("xcrun", "simctl", "list", "runtimes", quiet=True)
        raise SystemExit(
            f"no available {kind} simulator in this Xcode installation.\n{available}"
        )
    return found[0]


def main() -> int:
    if len(sys.argv) != 3 or sys.argv[1] not in {"pick", "boot"}:
        print(__doc__, file=sys.stderr)
        return 2
    command, kind = sys.argv[1], sys.argv[2]
    device = pick(kind)
    if command == "boot":
        # `boot` is idempotent for our purposes; an already-booted simulator
        # reports an error that should not fail the job.
        subprocess.run(
            ["xcrun", "simctl", "boot", device["udid"]],
            check=False,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
        )
        subprocess.run(
            ["xcrun", "simctl", "bootstatus", device["udid"], "-b"],
            check=True,
            stdout=subprocess.DEVNULL,
        )
    print(device["udid"])
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
