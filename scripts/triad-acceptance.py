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
# `amount` is the buy input in token base units (default 1e18), and
# `buy_floor` is the least PT, in base units, that input must buy. A chain with
# no active Pendle market has only `redeem`: a matured PT redeemed for its SY.
# Every slot below was verified on a fork by writing a balance and reading it
# back through balanceOf.
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
        # A market that matured on 22 Oct 2026, to redeem PT for its
        # underlying srUSDe. `pt_balance_slot` is the PT's balance mapping.
        "redeem": {
            "market": "0x66ec657c59cdcaf171ab43b83da3942758bf8a97",
            "pt": "0x59bc9fae5d62b19d4f8d07d758047acb9ee19d34",
            "token": "0x3d7d6fdf07ee548b939a80edbc9b2256d0cdc003",
            # Verified on the fork: PT-srUSDe keeps balances in slot 0.
            "pt_balance_slot": "0",
        },
    },
    # The chains below buy PT with the market's own SY and sell it back, or,
    # where Pendle has no active market, redeem a matured PT for its SY.
    "base": {
        "chain_id": 8453,
        # PT-yUTY, expiring 17 Dec 2026. Base's tokenized-stock SYs, such as
        # GOOGLc's, price through a Base precompile that Anvil cannot run.
        "market": "0x858539e155d870ecbbd27f5fb9d9521aa39251f1",
        "pt": "0x0112e4d09bc228534eef9d52d5a06d6b94244e8e",
        "token": "0x797f9d67a878250e0aa572c41a8a4e862ad09d5c",
        "balance_slot": "2",
        "buy_floor": 10**18,
    },
    "bsc": {
        "chain_id": 56,
        # PT-clisBNB, expiring 15 Apr 2027.
        "market": "0x3974b474f33e00fe177b54206726a8d33fc87a57",
        "pt": "0x7dfbebc2bcbc3e754d64c12f0fbb64f59d9e96e0",
        "token": "0x27faf900007b4cba7803000251ec96bc69ff1bea",
        "balance_slot": "2",
        "buy_floor": 10**18,
    },
    "hyperliquid": {
        "chain_id": 999,
        # PT-kHYPE, expiring 25 Mar 2027.
        "market": "0x697dd29eed8e926205a08400a60d1578154c1a60",
        "pt": "0x9ccac9566fba516a5df00a23e81801b1f1cb57a8",
        "token": "0x57fc55dff8ceca86ee94a6bf255af2f0ed90eb9e",
        "balance_slot": "2",
        "buy_floor": 10**18,
    },
    "optimism": {
        "chain_id": 10,
        "redeem": {
            # PT-wstETH, matured 26 Sep 2024, redeemed for SY-wstETH.
            "market": "0x24fb77c8c776c75f869bc65e6856af56f57d919f",
            "pt": "0xf4225f061e5e01aa59de5e615729a9180301eb07",
            "token": "0x96a528f4414ac3ccd21342996c93f2ecdec24286",
            "pt_balance_slot": "0",
        },
    },
    "robinhood": {
        "chain_id": 4663,
        # PT-PFE, expiring 10 Dec 2026, trades close to 1:1 with its SY.
        "market": "0x892defbf510d9baa96dbd2a51b13e879a857a79b",
        "pt": "0xf9cd484f7e7799ae32b7f9a75e60c478fd1f0b6e",
        "token": "0x8c7454d4d64abd04fbf5f5ffe0dbcd5e3abcecf6",
        "balance_slot": "2",
        "buy_floor": 99 * 10**16,
    },
    "monad": {
        "chain_id": 143,
        # PT-sUSDE, expiring 28 Jan 2027.
        "market": "0x9bf15c1824a4158d6f9c4ba8a19819e700f5e88b",
        "pt": "0x39148079bca3abf3f3b40f6f1c1b1ae1f2927d13",
        "token": "0xd1279361528c3c4588506e1746f81175dfa5f64e",
        "balance_slot": "0",
        "buy_floor": 10**18,
    },
    "sonic": {
        "chain_id": 146,
        "redeem": {
            # PT-x33, matured 26 Feb 2026, redeemed for SY-x33.
            "market": "0xc9646f2a8a1d2f12a7fbf0274ec2d3ecc65f1468",
            "pt": "0x22fe64c10f482004c48872c98d1f03cbdcaddce9",
            "token": "0x69a23f16974024a1b6a5362ac2a11bfeeefd5827",
            "pt_balance_slot": "0",
        },
    },
    "mantle": {
        "chain_id": 5000,
        "redeem": {
            # PT-cmETH, matured 18 Sep 2025, redeemed for SY-cmETH.
            "market": "0x6304ccbda63a7fb94919c705de54f9889f3ce047",
            "pt": "0x698eb002a4ec013a33286f7f2ba0be3970e66455",
            "token": "0x2ab88ac7458faec2e952bb79cc1be6577bf63e70",
            "pt_balance_slot": "0",
        },
    },
    "berachain": {
        "chain_id": 80094,
        "redeem": {
            # PT-iBGT, matured 25 Dec 2025, redeemed for SY-iBGT.
            "market": "0x850a9f099de3081fe88f8be6207492c52754acb5",
            "pt": "0xc2c7ded5837f911b7efe5984b52182036fd66a20",
            "token": "0xc61c866b6fdc36f88ec368b969c6d7cb810cafa9",
            "pt_balance_slot": "0",
        },
    },
    "xlayer": {
        "chain_id": 196,
        # PT-USDG, expiring 25 Feb 2027. USDG and its SY and PT have 6 decimals.
        "market": "0xb70be417526707c8ebf5f1a79e4876557639b01d",
        "pt": "0x5ea1f184af5ced57725213d8267b5c4c834557d4",
        "token": "0x1f336f899f77b084133bc14a81170837ed618d1b",
        "balance_slot": "0",
        "amount": 10**6,
        "buy_floor": 10**6,
    },
    "plasma": {
        "chain_id": 9745,
        # PT-sUSDE, expiring 28 Jan 2027.
        "market": "0xdb96b529d19ff4a7d711e6e082172c77cfe88ed8",
        "pt": "0x257561233419039bec3a28be717d14b4f490883d",
        "token": "0x9838b0450e311962aed969ba7d0fe0bb9d8620eb",
        "balance_slot": "0",
        "buy_floor": 10**18,
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
    # Redeem-only chains have no market to buy on.
    MARKET, PT, TOKEN = config.get("market"), config.get("pt"), config.get("token")
    redeem = config.get("redeem")
    amount = config.get("amount", 10**18)
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
        print(f"ceremony {count}: {url}", flush=True)
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
    if MARKET:
        assert any(m["address"] == MARKET for m in markets["results"])
    report["active_markets"] = markets["total"]
    shown = MARKET or redeem["market"]
    assert json.loads(read(f"/petals/pendle/{chain}/markets/{shown}/info.json"))["address"] == shown
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
    # The token approval goes to the input token for a buy and to PT for a sell
    # or a redemption.
    destinations = {ROUTER} | ({TOKEN, PT} if MARKET else set()) | ({redeem["pt"]} if redeem else set())
    policy["allowed_destinations"] = [{"chain": chain, "destination": d} for d in sorted(destinations)]
    policy_file = work / (wallet + "-policy.json")
    policy_file.write_text(json.dumps(policy))
    launch = cli("wallet", "update-policy", wallet, "--file", str(policy_file)).stdout
    complete(launch_field(launch, "ceremony_url"))
    cli("wallet", "commit-policy", launch_field(launch, "operation_id"))
    passed("package and exact destinations authorized by owner policy ceremony")
    rpc("anvil_setBalance", [owner, hex(10**20)])
    if MARKET:
        # Fork-only test balance, written to the token's balance mapping. Assert
        # the effect before using it. No changes are sent to the public chain.
        index = command(["cast", "index", "address", owner, config["balance_slot"]]).stdout.strip()
        assert balance(TOKEN, owner) == 0
        rpc("anvil_setStorageAt", [TOKEN, index, "0x" + hex(10 * amount)[2:].rjust(64, "0")])
        assert balance(TOKEN, owner) == 10 * amount
        passed("disposable account funded only on local fork")
    portfolio = json.loads(read(f"/petals/pendle/{chain}/accounts/{wallet}/1/portfolio.json"))
    assert all(p["chainId"] == config["chain_id"] for p in portfolio["positions"])
    passed("selected-account live portfolio API")

    def execute(op, name, input_amount, floor, market=MARKET, token=TOKEN):
        base = f"/petals/pendle/{chain}/{op}/{wallet}/1/{name}"
        request = {"market": market, "token": token, "amount": str(input_amount), "min_output": str(floor), "slippage_bps": 50, "max_price_impact_bps": 100}
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
            assert observed["state"] not in ("reverted", "staging_unknown", "failed", "broadcast_unknown", "expired"), observed["state"]
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
            # A step still in flight refuses confirm, and may have mined
            # before the status read that follows.
            in_flight = "transaction submitted; poll status before advancing" in result.stderr
            if result.returncode and not in_flight and observed["state"] not in ("approval_required", "submitted"):
                raise RuntimeError(result.stderr[-1800:])
            time.sleep(0.3)
        raise RuntimeError(op + " timed out")

    if MARKET:
        pt_before = balance(PT, owner)
        bought = execute("buy-pt", "accept-buy", amount, config["buy_floor"])
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
    if redeem:
        # Fork-only matured PT, written to its balance mapping like the token.
        index = command(["cast", "index", "address", owner, redeem["pt_balance_slot"]]).stdout.strip()
        assert balance(redeem["pt"], owner) == 0
        rpc("anvil_setStorageAt", [redeem["pt"], index, "0x" + hex(10**18)[2:].rjust(64, "0")])
        assert balance(redeem["pt"], owner) == 10**18
        output_before = balance(redeem["token"], owner)
        redeemed = execute("redeem-pt", "accept-redeem", 10**18, 1, redeem["market"], redeem["token"])
        output_delta = balance(redeem["token"], owner) - output_before
        assert balance(redeem["pt"], owner) == 0
        assert output_delta >= int(redeemed["encoded_minimum"])
        report["redeem_output"] = str(output_delta)
        passed("matured PT redeemed through Broker/Signer; output meets encoded floor")
    report["software_owner_ceremonies"] = count
    report["scope"] = "Local fork with real Pendle contracts, compiled WASM and real triad processes; fixture balances and software passkeys. No mainnet broadcast or physical-passkey acceptance."
    report["result"] = "passed"
    args.report.parent.mkdir(parents=True, exist_ok=True)
    args.report.write_text(json.dumps(report, indent=2) + "\n")
    print("Evidence: " + str(args.report), flush=True)


if __name__ == "__main__":
    main()
