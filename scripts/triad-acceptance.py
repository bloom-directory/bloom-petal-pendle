#!/usr/bin/env python3
"""Acceptance against an already-running DISPOSABLE triad and a local fork of one chain.

Creates a new wallet through software owner ceremonies. No production funds or
wallet secrets are used. The supplied RPC must identify itself as local Anvil.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import shlex
import subprocess
import time
import urllib.parse
import urllib.request
import urllib.error

ROUTER = "0x888888888889758f76e7103c6cbf23abbf58f946"
ROOT = Path(__file__).resolve().parents[1]
# One market per chain. `token` is what the test buys PT with and sells PT for;
# `balance_slot` is that token's ERC-20 balance mapping slot, for `cast index`.
# `buy_floor` is in PT base units for a 1e18 input.
CHAINS = {
    "arbitrum": {
        "chain_id": 42161,
        "market": "0xf86119a39f8654f38acbbd5488bd83f3f51983c8",
        "pt": "0xe9d07c2a3588b9a25edd55664be44ecfe5f92fce",
        # SY-sUSDai itself; its verified balance mapping is slot 2.
        "token": "0x30ccf4bbee313fcd19f3e295b3ba2920a24e2f62",
        "balance_slot": "2",
        "buy_floor": 10**18,
    },
    "ethereum": {
        "chain_id": 1,
        "market": "0x13285bcbc27f92b47b4edb99d744c07b48c977c0",
        # PT-reUSD has 6 decimals.
        "pt": "0xecfafdc7741323a945a163ed068b5a3c43483957",
        # reUSD, the only token the reUSD SY accepts. OpenZeppelin v5
        # ERC20Upgradeable keeps balances in its ERC-7201 namespace.
        "token": "0x5086bf358635b81d8c47c66d1c8b9e567db70c72",
        "balance_slot": "0x52c63247e1f47db19d5ce0460030c497f067ca4cebf71ba98eeadabe20bace00",
        "buy_floor": 10**6,
    },
}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--triad-env", required=True, type=Path)
    parser.add_argument("--driver", required=True, type=Path)
    parser.add_argument("--rpc", required=True)
    parser.add_argument("--report", required=True, type=Path)
    parser.add_argument("--chain", choices=sorted(CHAINS), default="arbitrum")
    args = parser.parse_args()
    chain = args.chain
    config = CHAINS[chain]
    MARKET, PT, TOKEN = config["market"], config["pt"], config["token"]
    assert urllib.parse.urlparse(args.rpc).hostname in ("localhost", "127.0.0.1"), "local fork only"
    assert not args.report.resolve().is_relative_to(ROOT), "evidence must live outside the package"
    env = os.environ.copy()
    # The launcher documents triad.env as public connection settings. Do not
    # source arbitrary shell or enumerate the calling environment's secrets.
    for line in args.triad_env.read_text().splitlines():
        match = re.fullmatch(r"export (BLOOM_[A-Z_]+)=(.*)", line)
        if match:
            parts = shlex.split(match[2])
            env[match[1]] = parts[0] if parts else ""
    home = Path(env["BLOOM_HOME"]).resolve()
    assert home.is_relative_to(Path(env["BLOOM_TRIAD_DEVELOPER_ROOT"]).resolve())
    assert home != Path.home() / ".bloom"
    binary = env["BLOOM_BIN"]
    count = 0
    wallet = "pendle-" + str(time.time_ns())[-12:]
    auth_seed = "pendle-disposable-software-owner-" + wallet
    work = args.triad_env.resolve().parent.parent
    report = {"sdk_revision": "7dd2722db5de305dd63ca4ed03591f3ab82fa40b", "chain": chain, "wallet": wallet, "account": 1, "checks": [], "transactions": []}

    def command(argv, check=True):
        p = subprocess.run(argv, env=env, capture_output=True, text=True, timeout=90)
        if check and p.returncode:
            raise RuntimeError(p.stderr[-1800:] or p.stdout[-1800:])
        return p

    def cli(*argv, check=True):
        return command([binary, *argv], check=check)

    def read(path):
        return cli("vfs", "cat", path).stdout

    def write(path, value, check=True):
        body = value if isinstance(value, str) else json.dumps(value)
        return cli("vfs", "write", path, "--data", body, check=check)

    def rpc(method, params):
        # These calls are reads or idempotent Anvil fixture mutations. Retrying
        # an upstream transport reset does not submit an Ethereum transaction.
        for attempt in range(4):
            req = urllib.request.Request(args.rpc, data=json.dumps({"jsonrpc": "2.0", "id": 1, "method": method, "params": params}).encode(), headers={"Content-Type": "application/json"})
            try:
                with urllib.request.urlopen(req, timeout=30) as response:
                    data = json.load(response)
                if "error" not in data:
                    return data["result"]
                message = str(data["error"]).lower()
                if attempt == 3 or not any(word in message for word in ("connection", "timeout", "sending request")):
                    raise RuntimeError(data["error"])
            except (urllib.error.URLError, TimeoutError):
                if attempt == 3:
                    raise
            time.sleep(0.5 * (attempt + 1))
        raise RuntimeError("fork RPC did not return a result")

    def complete(url):
        nonlocal count
        assert url.startswith(env["BLOOM_TRIAD_DEV_CEREMONY_ORIGIN"] + "/ceremony/")
        count += 1
        command([str(args.driver), "complete", url, auth_seed, "--sign-count", str(count)])

    def launch_field(text, name):
        return next(line.split(": ", 1)[1] for line in text.splitlines() if line.startswith(name + ": "))

    def balance(token, owner):
        data = "0x70a08231" + owner[2:].rjust(64, "0")
        return int(rpc("eth_call", [{"to": token, "data": data}, "latest"]), 16)

    def passed(label):
        report["checks"].append(label)
        print("PASS " + label, flush=True)

    assert "anvil" in rpc("web3_clientVersion", []).lower()
    assert int(rpc("eth_chainId", []), 16) == config["chain_id"]
    # A fresh fork refuses eth_call ("Excess blob gas not set") until it has
    # mined a block of its own.
    rpc("evm_mine", [])
    report["fork_start_block"] = int(rpc("eth_blockNumber", []), 16)
    status = json.loads(read("/petals/pendle/status.json"))
    assert status["sdk_revision"] == report["sdk_revision"]
    built = cli("petals", "build", str(ROOT)).stdout
    report["package_hash"] = launch_field(built, "hash")
    report["machine_binary_sha256"] = hashlib.sha256(Path(binary).read_bytes()).hexdigest()
    passed("compiled SDK42 package reachable through real triad")
    markets = json.loads(read(f"/petals/pendle/{chain}/markets.json"))
    assert markets["chainId"] == config["chain_id"] and not markets["truncated"]
    assert markets["count"] == markets["total"]
    assert any(m["address"] == MARKET for m in markets["results"])
    report["active_markets"] = markets["total"]
    assert json.loads(read(f"/petals/pendle/{chain}/markets/{MARKET}/info.json"))["address"] == MARKET
    passed("live Pendle discovery and market API through WASM")
    complete(launch_field(cli("wallet", "new", wallet).stdout, "ceremony_url"))
    write(f"/wallets/{wallet}/new", {"request_id": "account-one"})
    creation = json.loads(read(f"/wallets/{wallet}/new"))
    complete(next(r["ceremony_url"] for r in creation["requests"] if r["request_id"] == "account-one"))
    for _ in range(30):
        creation = json.loads(read(f"/wallets/{wallet}/new"))
        if any(r["state"] == "created" and r["number"] == 1 for r in creation["requests"]):
            break
        time.sleep(0.2)
    # The creation receipt and the VFS inventory projection are asynchronous.
    # Refresh the authenticated public projection and wait for the address leaf.
    for _ in range(200):
        cli("wallet", "accounts", wallet)
        projected = cli("vfs", "cat", f"/wallets/{wallet}/1/address.evm", check=False)
        if projected.returncode == 0:
            owner = projected.stdout.strip().lower()
            break
        time.sleep(0.25)
    else:
        raise RuntimeError("account-one public projection did not become ready")
    owner_zero = read(f"/wallets/{wallet}/0/address.evm").strip().lower()
    assert owner != owner_zero
    report["owner"] = owner
    passed("Signer-created account one differs from account zero")
    policy = json.loads(read(f"/wallets/{wallet}/policy.json"))
    policy["allowed_petal_packages"] = [report["package_hash"]]
    # The token approval goes to the input token for a buy and to PT for a sell.
    policy["allowed_destinations"] = [{"chain": chain, "destination": d} for d in sorted({TOKEN, PT, ROUTER})]
    policy_file = work / (wallet + "-policy.json")
    policy_file.write_text(json.dumps(policy))
    launch = cli("wallet", "update-policy", wallet, "--file", str(policy_file)).stdout
    complete(launch_field(launch, "ceremony_url"))
    cli("wallet", "commit-policy", launch_field(launch, "operation_id"))
    passed("package and exact destinations authorized by owner policy ceremony")
    rpc("anvil_setBalance", [owner, hex(10**20)])
    # Fork-only test balance, written to the token's balance mapping. Assert the
    # effect before using it. No changes are sent to the public chain.
    index = command(["cast", "index", "address", owner, config["balance_slot"]]).stdout.strip()
    assert balance(TOKEN, owner) == 0
    rpc("anvil_setStorageAt", [TOKEN, index, "0x" + hex(10**19)[2:].rjust(64, "0")])
    assert balance(TOKEN, owner) == 10**19
    passed("disposable account funded only on local fork")
    portfolio = json.loads(read(f"/petals/pendle/{chain}/accounts/{wallet}/1/portfolio.json"))
    assert all(p["chainId"] == config["chain_id"] for p in portfolio["positions"])
    passed("selected-account live portfolio API")

    def execute(op, name, input_amount, floor):
        base = f"/petals/pendle/{chain}/{op}/{wallet}/1/{name}"
        request = {"market": MARKET, "token": TOKEN, "amount": str(input_amount), "min_output": str(floor), "slippage_bps": 50, "max_price_impact_bps": 100}
        write(base + "/draft.json", request)
        original = json.loads(read(base + "/status.json"))
        assert original["account"] == 1 and original["owner"].lower() == owner
        assert cli("vfs", "cat", base.replace(f"/{wallet}/1/", f"/{wallet}/0/") + "/status.json", check=False).returncode != 0
        assert cli("vfs", "cat", base.replace(f"/{wallet}/1/", f"/{wallet}/01/") + "/status.json", check=False).returncode != 0
        write(base + "/draft.json", request)
        changed = dict(request, amount=str(input_amount + 1))
        assert write(base + "/draft.json", changed, check=False).returncode != 0
        passed(op + " account isolation and immutable id")
        for _ in range(100):
            observed = json.loads(read(base + "/status.json"))
            if observed["state"] == "execution_confirmed":
                nonce = rpc("eth_getTransactionCount", [owner, "latest"])
                write(base + "/confirm.json", "confirm")
                assert nonce == rpc("eth_getTransactionCount", [owner, "latest"])
                for step in observed["steps"]:
                    receipt = rpc("eth_getTransactionReceipt", [step["tx_hash"]])
                    assert receipt["status"] == "0x1" and receipt["from"].lower() == owner
                    report["transactions"].append({"operation": op, "step": step["kind"], "hash": step["tx_hash"], "receipt_status": receipt["status"]})
                return observed
            assert observed["state"] not in ("reverted", "staging_unknown", "failed"), observed["state"]
            result = write(base + "/confirm.json", "confirm", check=False)
            observed = json.loads(read(base + "/status.json"))
            for step in observed["steps"]:
                if not step["outbox_id"] or step["state"] == "confirmed":
                    continue
                challenge = cli("vfs", "cat", f"/wallets/{wallet}/1/chains/{chain}/outbox/pending/{step['outbox_id']}/approval_challenge.json", check=False)
                if challenge.returncode == 0:
                    value = json.loads(challenge.stdout)
                    if value.get("ceremony_url"):
                        complete(value["ceremony_url"])
            if result.returncode and observed["state"] not in ("approval_required", "submitted"):
                raise RuntimeError(result.stderr[-1800:])
            time.sleep(0.3)
        raise RuntimeError(op + " timed out")

    pt_before = balance(PT, owner)
    bought = execute("buy-pt", "accept-buy", 10**18, config["buy_floor"])
    pt_delta = balance(PT, owner) - pt_before
    assert pt_delta >= int(bought["encoded_minimum"])
    assert balance(PT, owner_zero) == 0
    report["pt_received"] = str(pt_delta)
    passed("PT buy mined through Broker/Signer; account-one output meets encoded floor")
    token_before = balance(TOKEN, owner)
    sold = execute("sell-pt", "accept-sell", pt_delta // 2, 1)
    token_delta = balance(TOKEN, owner) - token_before
    assert token_delta >= int(sold["encoded_minimum"])
    report["token_received_on_sell"] = str(token_delta)
    passed("PT sell mined through Broker/Signer; output meets encoded floor")
    report["software_owner_ceremonies"] = count
    report["scope"] = "Local fork with real Pendle contracts, compiled WASM and real triad processes; fixture balances and software passkeys. No mainnet broadcast or physical-passkey acceptance."
    report["result"] = "passed"
    args.report.parent.mkdir(parents=True, exist_ok=True)
    args.report.write_text(json.dumps(report, indent=2) + "\n")
    print("Evidence: " + str(args.report), flush=True)


if __name__ == "__main__":
    main()
