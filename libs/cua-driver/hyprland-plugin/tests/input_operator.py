r"""Host-only operator for the nonshipping isolated-input experiment.

Requires the optional Python ``cryptography`` package for key operations.
Keep the private key outside the VM and outside any agent/MCP-accessible path.
Transfer only the public key and the JSON output of ``sign`` to the guest.
This helper has no network or compositor access and never approves implicitly.

Usage (operator shell on the host):
  python3 input_operator.py keygen --key /operator/private/input.pem
  python3 input_operator.py public-key --key /operator/private/input.pem
  python3 input_operator.py sign --key /operator/private/input.pem \
    --epoch HEX --challenge HEX --target HEX --capabilities 1 --ttl-ms 30000
"""

import argparse
import json
import os
import re
import stat
import sys
import time
from pathlib import Path

MAX_UINT64 = (1 << 64) - 1
MAX_LEASE_MS = 60_000


def hex_token(value):
    if not isinstance(value, str) or re.fullmatch(r"[0-9a-f]{32}", value) is None:
        raise ValueError("token must be exactly 32 lowercase hexadecimal digits")
    return value


def unsigned(value, maximum=MAX_UINT64):
    if type(value) is int:
        number = value
    elif isinstance(value, str) and re.fullmatch(r"0|[1-9][0-9]*", value):
        if len(value) > 20:
            raise ValueError("integer is out of range")
        number = int(value)
    else:
        raise ValueError("integer must be canonical unsigned decimal")
    if number < 0 or number > maximum:
        raise ValueError("integer is out of range")
    return number


def canonical_message(epoch, challenge, target, expires_unix_ms, capabilities):
    tokens = [hex_token(value) for value in (epoch, challenge, target)]
    expiry = unsigned(expires_unix_ms)
    caps = unsigned(capabilities, 15)
    if not caps:
        raise ValueError("at least one capability is required")
    return ("CUA_TEST_LEASE_1\n" + "\n".join(tokens + [str(expiry), str(caps)]) + "\n").encode("ascii")


def validate_expiry(expires_unix_ms, now_ms):
    expiry, now = unsigned(expires_unix_ms), unsigned(now_ms)
    if not now < expiry <= now + MAX_LEASE_MS:
        raise ValueError("grant must expire in the next 1 to 60000 milliseconds")
    return expiry


def crypto():
    try:
        from cryptography.hazmat.primitives import serialization
        from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PrivateKey
    except ImportError:
        raise RuntimeError("key operations require the optional cryptography package") from None
    return serialization, Ed25519PrivateKey


def public_key_hex(key):
    serialization, _ = crypto()
    return key.public_key().public_bytes(
        serialization.Encoding.Raw, serialization.PublicFormat.Raw
    ).hex()


def keygen(path):
    serialization, key_type = crypto()
    key = key_type.generate()
    pem = key.private_bytes(
        serialization.Encoding.PEM,
        serialization.PrivateFormat.PKCS8,
        serialization.NoEncryption(),
    )
    # Exclusive creation prevents overwriting a key or following a symlink.
    with os.fdopen(os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600), "wb") as stream:
        stream.write(pem)
    return public_key_hex(key)


def load_key(path):
    serialization, key_type = crypto()
    with os.fdopen(os.open(path, os.O_RDONLY | os.O_NOFOLLOW), "rb") as stream:
        metadata = os.fstat(stream.fileno())
        if (not stat.S_ISREG(metadata.st_mode) or metadata.st_mode & 0o077
                or metadata.st_uid != os.getuid()):
            raise ValueError("private key must be an owner-only regular file")
        pem = stream.read(4097)
        if len(pem) > 4096:
            raise ValueError("invalid private key file")
    try:
        key = serialization.load_pem_private_key(pem, password=None)
    except (ValueError, TypeError):
        raise ValueError("invalid private key file") from None
    if not isinstance(key, key_type):
        raise ValueError("private key must be Ed25519")
    return key


def sign_grant(key, epoch, challenge, target, expires_unix_ms, capabilities, now_ms):
    expiry = validate_expiry(expires_unix_ms, now_ms)
    message = canonical_message(epoch, challenge, target, expiry, capabilities)
    caps = unsigned(capabilities, 15)
    signature = key.sign(message).hex()
    return {
        "epoch": epoch,
        "challenge": challenge,
        "target": target,
        "expires_unix_ms": expiry,
        "capabilities": caps,
        "packet": f"APPROVE {challenge} {target} {expiry} {caps} {signature}",
    }


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    commands = parser.add_subparsers(dest="command", required=True)
    for command in ("keygen", "public-key", "sign"):
        subparser = commands.add_parser(command)
        subparser.add_argument("--key", type=Path, required=True)
        if command == "sign":
            for name in ("epoch", "challenge", "target", "capabilities"):
                subparser.add_argument("--" + name, required=True)
            expiry = subparser.add_mutually_exclusive_group()
            expiry.add_argument("--expires-unix-ms")
            expiry.add_argument("--ttl-ms", default="30000")
    args = parser.parse_args(argv)
    try:
        if args.command == "keygen":
            output = {"public_key": keygen(args.key)}
        elif args.command == "public-key":
            output = {"public_key": public_key_hex(load_key(args.key))}
        else:
            now = time.time_ns() // 1_000_000
            expiry = args.expires_unix_ms
            if expiry is None:
                expiry = now + unsigned(args.ttl_ms, MAX_LEASE_MS)
            # Validate all public input before opening the private key.
            validate_expiry(expiry, now)
            canonical_message(args.epoch, args.challenge, args.target, expiry, args.capabilities)
            output = sign_grant(load_key(args.key), args.epoch, args.challenge,
                                args.target, expiry, args.capabilities, now)
    except (OSError, ValueError, RuntimeError):
        # Neither exception text nor private-key paths/content belong in logs.
        print("operator failed: check public arguments, owner-only key file, and cryptography dependency", file=sys.stderr)
        return 1
    print(json.dumps(output, separators=(",", ":")))
    return 0


if __name__ == "__main__":
    sys.exit(main())
