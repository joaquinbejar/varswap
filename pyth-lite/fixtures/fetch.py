#!/usr/bin/env python3
"""Fetch the committed PriceUpdateV2 fixtures from Solana mainnet (stdlib only).

For each feed this derives the Pyth push oracle price feed account (shard 0, seeds
[shard u16 LE, feed id]), adds the explicit accounts below, calls getAccountInfo, and writes
next to this script:
  <name>.json      the raw RPC response, for provenance
  <name>.data.bin  the decoded account data
  <name>.owner.bin the 32-byte owner program id
Usage: python3 fetch.py [rpc_url]
"""
import base64
import hashlib
import json
import os
import sys
import urllib.request

RPC = sys.argv[1] if len(sys.argv) > 1 else "https://api.mainnet-beta.solana.com"
PUSH_ORACLE = "pythWSnswVUd12oZpeFP8e9CVaEqJg25g1Vtc2biRsT"
SHARD = 0
FEEDS = {
    # Feed ids from Hermes /v2/price_feeds (Metal.XAU/USD, Metal.XAG/USD, Crypto.SOL/USD).
    "xau_usd": "765d2ba906dbc32ca17cc11f5310a89e9ee1f6420508c63861f2f8ba4ee34bb2",
    "xag_usd": "f2fb02c32b055c805e7238d628e5e9dadef274376114eb1f012337cabe93871e",
    "sol_usd": "ef0d8b6fda2ceba41da15d4095d1da392a0d2f8ed0c6c7bc0f4cfac8c280b56d",
}
# A Partial-verification account (post_update with a subset of guardian signatures), found with
# getProgramAccounts on the receiver: dataSize 134, memcmp offset 40 == 0x00 (Partial tag).
EXPLICIT = {"partial": "1C5M4gperNytKTT7V5Ut8yYKa7fa2Awrd581wdZfoBw"}

B58 = "123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz"


def b58decode(s):
    n = 0
    for c in s:
        n = n * 58 + B58.index(c)
    raw = n.to_bytes((n.bit_length() + 7) // 8, "big")
    pad = len(s) - len(s.lstrip("1"))
    return b"\0" * pad + raw


def b58encode(b):
    n = int.from_bytes(b, "big")
    out = ""
    while n:
        n, r = divmod(n, 58)
        out = B58[r] + out
    pad = len(b) - len(b.lstrip(b"\0"))
    return "1" * pad + out


P = 2**255 - 19
D = (-121665 * pow(121666, P - 2, P)) % P


def on_curve(b):
    """True if b decompresses to an ed25519 point (curve25519-dalek semantics)."""
    y = int.from_bytes(b, "little") & ((1 << 255) - 1)
    y %= P
    u = (y * y - 1) % P
    v = (D * y * y + 1) % P
    x2 = u * pow(v, P - 2, P) % P
    return x2 == 0 or pow(x2, (P - 1) // 2, P) == 1


def find_pda(seeds, program):
    for bump in range(255, -1, -1):
        h = hashlib.sha256(b"".join(seeds) + bytes([bump]) + program + b"ProgramDerivedAddress").digest()
        if not on_curve(h):
            return h, bump
    raise RuntimeError("no bump")


def rpc(method, params):
    body = json.dumps({"jsonrpc": "2.0", "id": 1, "method": method, "params": params}).encode()
    req = urllib.request.Request(RPC, body, {"Content-Type": "application/json"})
    with urllib.request.urlopen(req, timeout=30) as r:
        return json.load(r)


def main():
    here = os.path.dirname(os.path.abspath(__file__))
    program = b58decode(PUSH_ORACLE)
    targets = []
    for name, feed in FEEDS.items():
        addr, bump = find_pda([SHARD.to_bytes(2, "little"), bytes.fromhex(feed)], program)
        targets.append((name, b58encode(addr), {"bump": bump, "feed_id": feed, "shard": SHARD}))
    for name, address in EXPLICIT.items():
        targets.append((name, address, {}))
    for name, address, extra in targets:
        resp = rpc("getAccountInfo", [address, {"encoding": "base64", "commitment": "finalized"}])
        value = resp["result"]["value"]
        data = base64.b64decode(value["data"][0])
        resp["request"] = dict(extra, address=address, rpc=RPC)
        with open(os.path.join(here, name + ".json"), "w") as f:
            json.dump(resp, f, indent=2, sort_keys=True)
            f.write("\n")
        with open(os.path.join(here, name + ".data.bin"), "wb") as f:
            f.write(data)
        with open(os.path.join(here, name + ".owner.bin"), "wb") as f:
            f.write(b58decode(value["owner"]))
        print(name, address, "owner", value["owner"], "len", len(data), "slot", resp["result"]["context"]["slot"])


if __name__ == "__main__":
    main()
