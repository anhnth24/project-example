#!/usr/bin/env python3
"""Dependency-free encrypt-then-MAC envelope for O03 backup artifacts.

Algorithm id: ``aes-256-ctr-hmac-sha256-v1``

Construction (NOT GCM, NOT AEAD):
1. Random 32-byte salt + 16-byte IV.
2. HKDF-SHA256 (RFC 5869, stdlib HMAC) from master key + salt derives two
   independent 32-byte keys with distinct info labels:
   ``markhand-enc-v1`` / ``markhand-mac-v1``.
3. Host OpenSSL ``enc -aes-256-ctr`` encrypts plaintext with the enc key + IV.
4. HMAC-SHA256 over length-prefixed canonical fields
   (algorithm, keyId, salt, iv, AAD, ciphertext) using the MAC key.
5. Decrypt verifies MAC with ``hmac.compare_digest`` before CTR decrypt.

No third-party crypto libraries. Temp key material uses mode-0600 files and is
zeroed before unlink.
"""

from __future__ import annotations

import argparse
import hashlib
import hmac
import json
import os
import subprocess
import sys
import tempfile
from pathlib import Path
from typing import Any


ALGORITHM = "aes-256-ctr-hmac-sha256-v1"
KDF = "hkdf-sha256"
KEY_LEN = 32
SALT_LEN = 32
IV_LEN = 16
MAC_LEN = 32
ENC_INFO = b"markhand-enc-v1"
MAC_INFO = b"markhand-mac-v1"


class CryptoError(ValueError):
    """Fail-closed crypto error."""


def _sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def _load_key(env_name: str) -> bytes:
    raw = os.environ.get(env_name, "").strip()
    if len(raw) != 64 or any(ch not in "0123456789abcdef" for ch in raw):
        raise CryptoError(f"{env_name} must be 64 lowercase hex chars")
    key = bytes.fromhex(raw)
    if len(key) != KEY_LEN:
        raise CryptoError(f"{env_name} must decode to {KEY_LEN} bytes")
    return key


def hkdf_sha256(ikm: bytes, salt: bytes, info: bytes, length: int) -> bytes:
    """RFC 5869 HKDF-SHA256 (stdlib only)."""
    if length <= 0 or length > 255 * hashlib.sha256().digest_size:
        raise CryptoError("HKDF length invalid")
    if not salt:
        salt = b"\x00" * hashlib.sha256().digest_size
    prk = hmac.new(salt, ikm, hashlib.sha256).digest()
    okm = b""
    previous = b""
    counter = 1
    while len(okm) < length:
        previous = hmac.new(
            prk, previous + info + bytes([counter]), hashlib.sha256
        ).digest()
        okm += previous
        counter += 1
    return okm[:length]


def derive_keys(master: bytes, salt: bytes) -> tuple[bytes, bytes]:
    enc_key = hkdf_sha256(master, salt, ENC_INFO, KEY_LEN)
    mac_key = hkdf_sha256(master, salt, MAC_INFO, KEY_LEN)
    if hmac.compare_digest(enc_key, mac_key):
        raise CryptoError("enc/mac key collision (fail closed)")
    return enc_key, mac_key


def canonical_mac_message(
    *,
    algorithm: str,
    key_id: str,
    salt: bytes,
    iv: bytes,
    aad: bytes,
    ciphertext: bytes,
) -> bytes:
    """Length-prefixed canonical field binding for HMAC."""
    parts = [
        algorithm.encode("utf-8"),
        key_id.encode("utf-8"),
        salt,
        iv,
        aad,
        ciphertext,
    ]
    out = bytearray()
    for part in parts:
        out.extend(len(part).to_bytes(8, "big"))
        out.extend(part)
    return bytes(out)


def _private_temp(prefix: str) -> tuple[tempfile.TemporaryDirectory[str], Path]:
    tmp = tempfile.TemporaryDirectory(prefix=prefix)
    path = Path(tmp.name)
    path.chmod(0o700)
    return tmp, path


def _write_private(path: Path, data: bytes) -> None:
    fd = os.open(str(path), os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
    try:
        with os.fdopen(fd, "wb") as handle:
            handle.write(data)
            handle.flush()
            os.fsync(handle.fileno())
    except Exception:
        try:
            os.close(fd)
        except OSError:
            pass
        raise
    path.chmod(0o600)


def _zero_file(path: Path) -> None:
    if not path.is_file():
        return
    size = path.stat().st_size
    with path.open("r+b") as handle:
        handle.write(b"\x00" * size)
        handle.flush()
        os.fsync(handle.fileno())


def _openssl_ctr(
    *,
    decrypt: bool,
    key: bytes,
    iv: bytes,
    infile: Path,
    outfile: Path,
) -> None:
    if len(key) != KEY_LEN or len(iv) != IV_LEN:
        raise CryptoError("OpenSSL CTR key/iv length invalid")
    tmp, tmp_dir = _private_temp("markhand-openssl-")
    try:
        key_file = tmp_dir / "key.hex"
        iv_file = tmp_dir / "iv.hex"
        _write_private(key_file, key.hex().encode("ascii"))
        _write_private(iv_file, iv.hex().encode("ascii"))
        # OpenSSL enc requires -K/-iv hex on argv; keep material only in 0600
        # temp files until the short-lived subprocess starts, then zero files.
        key_hex = key_file.read_text(encoding="ascii").strip()
        iv_hex = iv_file.read_text(encoding="ascii").strip()
        args = [
            "openssl",
            "enc",
            "-aes-256-ctr",
            "-K",
            key_hex,
            "-iv",
            iv_hex,
            "-in",
            str(infile),
            "-out",
            str(outfile),
        ]
        if decrypt:
            args.insert(2, "-d")
        completed = subprocess.run(
            args,
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            check=False,
        )
        key_hex = "0" * len(key_hex)
        iv_hex = "0" * len(iv_hex)
        if completed.returncode != 0:
            raise CryptoError(
                f"openssl aes-256-ctr failed: {completed.stderr.strip() or completed.returncode}"
            )
        outfile.chmod(0o600)
    finally:
        for name in ("key.hex", "iv.hex"):
            _zero_file(tmp_dir / name)
        tmp.cleanup()


def encrypt_file(
    plaintext: Path,
    ciphertext: Path,
    *,
    key_env: str,
    key_id: str,
    aad: bytes,
) -> dict[str, Any]:
    if not key_id or any(ord(ch) < 32 for ch in key_id):
        raise CryptoError("key_id must be non-empty without control chars")
    try:
        aad_text = aad.decode("utf-8")
    except UnicodeDecodeError as error:
        raise CryptoError("AAD must be UTF-8") from error
    if plaintext.is_symlink() or ciphertext.is_symlink():
        raise CryptoError("symlink rejected for envelope IO")
    master = _load_key(key_env)
    salt = os.urandom(SALT_LEN)
    iv = os.urandom(IV_LEN)
    enc_key, mac_key = derive_keys(master, salt)
    data = plaintext.read_bytes()
    ciphertext.parent.mkdir(parents=True, exist_ok=True)
    tmp, tmp_dir = _private_temp("markhand-enc-")
    try:
        raw_ct = tmp_dir / "ct.bin"
        _openssl_ctr(decrypt=False, key=enc_key, iv=iv, infile=plaintext, outfile=raw_ct)
        ct = raw_ct.read_bytes()
        message = canonical_mac_message(
            algorithm=ALGORITHM,
            key_id=key_id,
            salt=salt,
            iv=iv,
            aad=aad,
            ciphertext=ct,
        )
        mac = hmac.new(mac_key, message, hashlib.sha256).digest()
        _write_private(ciphertext, ct)
        return {
            "algorithm": ALGORITHM,
            "keyId": key_id,
            "kdf": KDF,
            "saltHex": salt.hex(),
            "ivHex": iv.hex(),
            "macHex": mac.hex(),
            "aad": aad_text,
            "ciphertextSha256": _sha256_file(ciphertext),
            "plaintextSha256": hashlib.sha256(data).hexdigest(),
        }
    finally:
        _zero_file(tmp_dir / "ct.bin")
        enc_key = b"\x00" * KEY_LEN
        mac_key = b"\x00" * KEY_LEN
        master = b"\x00" * KEY_LEN
        tmp.cleanup()


def decrypt_file(
    ciphertext: Path,
    plaintext: Path,
    *,
    key_env: str,
    meta: dict[str, Any],
    aad: bytes,
    expected_key_id: str | None = None,
) -> None:
    if meta.get("algorithm") != ALGORITHM:
        raise CryptoError(f"unsupported algorithm: {meta.get('algorithm')}")
    if meta.get("kdf") != KDF:
        raise CryptoError(f"unsupported kdf: {meta.get('kdf')}")
    key_id = str(meta.get("keyId") or "")
    if not key_id:
        raise CryptoError("envelope keyId missing")
    if expected_key_id is not None and key_id != expected_key_id:
        raise CryptoError("envelope keyId mismatch")
    if meta.get("aad") != aad.decode("utf-8"):
        raise CryptoError("AAD mismatch")
    master = _load_key(key_env)
    try:
        salt = bytes.fromhex(str(meta["saltHex"]))
        iv = bytes.fromhex(str(meta["ivHex"]))
        mac = bytes.fromhex(str(meta["macHex"]))
    except (KeyError, ValueError) as error:
        raise CryptoError(f"envelope metadata incomplete: {error}") from error
    if len(salt) != SALT_LEN or len(iv) != IV_LEN or len(mac) != MAC_LEN:
        raise CryptoError("salt/iv/mac length invalid")
    actual_digest = _sha256_file(ciphertext)
    if actual_digest != meta.get("ciphertextSha256"):
        raise CryptoError("ciphertext digest mismatch")
    ct = ciphertext.read_bytes()
    if not ct:
        raise CryptoError("ciphertext truncated/empty")
    enc_key, mac_key = derive_keys(master, salt)
    message = canonical_mac_message(
        algorithm=ALGORITHM,
        key_id=key_id,
        salt=salt,
        iv=iv,
        aad=aad,
        ciphertext=ct,
    )
    expected_mac = hmac.new(mac_key, message, hashlib.sha256).digest()
    if not hmac.compare_digest(expected_mac, mac):
        raise CryptoError("HMAC verification failed (tamper/wrong key)")
    plaintext.parent.mkdir(parents=True, exist_ok=True)
    tmp, tmp_dir = _private_temp("markhand-dec-")
    try:
        ct_path = tmp_dir / "ct.bin"
        _write_private(ct_path, ct)
        out_tmp = tmp_dir / "pt.bin"
        _openssl_ctr(decrypt=True, key=enc_key, iv=iv, infile=ct_path, outfile=out_tmp)
        plain = out_tmp.read_bytes()
        if hashlib.sha256(plain).hexdigest() != meta.get("plaintextSha256"):
            raise CryptoError("plaintext digest mismatch after decrypt")
        _write_private(plaintext, plain)
    finally:
        _zero_file(tmp_dir / "ct.bin")
        _zero_file(tmp_dir / "pt.bin")
        enc_key = b"\x00" * KEY_LEN
        mac_key = b"\x00" * KEY_LEN
        master = b"\x00" * KEY_LEN
        tmp.cleanup()


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="cmd", required=True)
    enc = sub.add_parser("encrypt")
    enc.add_argument("--in", dest="infile", type=Path, required=True)
    enc.add_argument("--out", dest="outfile", type=Path, required=True)
    enc.add_argument("--meta-out", type=Path, required=True)
    enc.add_argument("--key-env", required=True)
    enc.add_argument("--key-id", required=True)
    enc.add_argument("--aad", default="")
    dec = sub.add_parser("decrypt")
    dec.add_argument("--in", dest="infile", type=Path, required=True)
    dec.add_argument("--out", dest="outfile", type=Path, required=True)
    dec.add_argument("--meta", type=Path, required=True)
    dec.add_argument("--key-env", required=True)
    dec.add_argument("--aad", default="")
    dec.add_argument("--expected-key-id")
    args = parser.parse_args(argv)
    try:
        if args.cmd == "encrypt":
            meta = encrypt_file(
                args.infile,
                args.outfile,
                key_env=args.key_env,
                key_id=args.key_id,
                aad=args.aad.encode(),
            )
            args.meta_out.write_text(json.dumps(meta, indent=2) + "\n", encoding="utf-8")
            print(args.meta_out)
            return 0
        meta = json.loads(args.meta.read_text(encoding="utf-8"))
        decrypt_file(
            args.infile,
            args.outfile,
            key_env=args.key_env,
            meta=meta,
            aad=args.aad.encode(),
            expected_key_id=args.expected_key_id,
        )
        print(args.outfile)
        return 0
    except (CryptoError, OSError, json.JSONDecodeError) as error:
        print(f"crypto error: {error}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
