#!/usr/bin/env python3
"""Convert a desktop tmuxmux hosts.toml into a tmuxmux-mobile import.json,
routing every host through a single SSH jump box (e.g. a Tailscale-reachable
machine that already has cloudflared + your ~/.ssh/config).

The phone only ever connects to the jump box; the jump box runs the actual
reach-the-target command (cloudflared/ssh-alias) and tmux at the far end.

Usage:
  gen_import.py hosts.toml --jump-host 100.80.173.59 --jump-user luke \
      --jump-key ~/.ssh/phone_key --out import.json

Then: adb push import.json \
  /sdcard/Android/data/xyz.geocam.tmuxmux/files/import.json
"""
import argparse
import json
import sys

try:
    import tomllib  # py3.11+
except ModuleNotFoundError:
    import tomli as tomllib  # type: ignore


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("hosts_toml")
    ap.add_argument("--jump-host", required=True, help="jump box address (Tailscale IP/name)")
    ap.add_argument("--jump-user", required=True)
    ap.add_argument("--jump-port", type=int, default=22)
    ap.add_argument("--jump-key", required=True, help="path to the private key the phone uses")
    ap.add_argument("--out", default="import.json")
    args = ap.parse_args()

    key = open(args.jump_key).read()
    data = tomllib.loads(open(args.hosts_toml).read())

    out = []
    for h in data.get("hosts", []):
        name = h.get("name")
        if not name:
            continue
        if h.get("local"):
            # tmux on the jump box itself.
            command = ""
            label = f"{name} (jump box)"
        elif h.get("command"):
            # cloudflared / custom chain — run verbatim on the jump box.
            command = h["command"]
            label = name
        else:
            # plain ssh-config alias — resolved on the jump box.
            target = f"{h['username']}@{name}" if h.get("username") else name
            command = f"ssh -tt {target}"
            label = name
        out.append({
            "label": label,
            "host": args.jump_host,
            "port": args.jump_port,
            "username": args.jump_user,
            "password": "",
            "private_key": key,
            "key_passphrase": "",
            "command": command,
        })

    json.dump({"hosts": out}, open(args.out, "w"), indent=2)
    print(f"wrote {args.out} with {len(out)} hosts (jump {args.jump_user}@{args.jump_host})")
    return 0


if __name__ == "__main__":
    sys.exit(main())
