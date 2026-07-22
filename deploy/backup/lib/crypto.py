#!/usr/bin/env python3
"""Streaming encrypt-then-MAC for O03 (stdlib HMAC/HKDF + libcrypto AES-CTR).

Algorithm id: ``aes-256-ctr-hmac-sha256-v1``

Construction (NOT GCM, NOT AEAD):
1. Random 32-byte salt + 16-byte IV.
2. HKDF-SHA256 derives independent enc/mac keys (info labels markhand-enc-v1 /
   markhand-mac-v1).
3. AES-256-CTR via host libcrypto (ctypes) streams plaintext→ciphertext without
   whole-file reads; raw derived keys never appear on argv.
4. HMAC-SHA256 streams over length-prefixed canonical header fields then
   ciphertext chunks (encrypt-then-MAC).
5. Decrypt verifies MAC with ``hmac.compare_digest`` before exposing plaintext
   via atomic temp+fsync+rename.

Limits: Python ``bytes``/``str`` are immutable — secure erasure of key material
is best-effort only (overwrite mutable ``bytearray`` copies and drop references).
Do not claim cryptographic wipe of immutable objects.
"""

from __future__ import annotations

import argparse
import ctypes
import ctypes.util
import hashlib
import hmac
import json
import os
import sys
import tempfile
from pathlib import Path
from typing import Any, BinaryIO


ALGORITHM = "aes-256-ctr-hmac-sha256-v1"
KDF = "hkdf-sha256"
KEY_LEN = 32
SALT_LEN = 32
IV_LEN = 16
MAC_LEN = 32
CHUNK = 1024 * 1024
ENC_INFO = b"markhand-enc-v1"
MAC_INFO = b"markhand-mac-v1"


class CryptoError(ValueError):
    """Fail-closed crypto error."""


def _load_libcrypto() -> ctypes.CDLL:
    name = ctypes.util.find_library("crypto") or "libcrypto.so.3"
    try:
        return ctypes.CDLL(name)
    except OSError as error:
        raise CryptoError(f"libcrypto unavailable: {error}") from error


class _AesCtr:
    """Minimal EVP AES-256-CTR wrapper (streaming)."""

    def __init__(self, key: bytes, iv: bytes, *, decrypt: bool) -> None:
        if len(key) != KEY_LEN or len(iv) != IV_LEN:
            raise CryptoError("AES-CTR key/iv length invalid")
        self._lib = _load_libcrypto()
        self._lib.EVP_CIPHER_CTX_new.restype = ctypes.c_void_p
        self._lib.EVP_aes_256_ctr.restype = ctypes.c_void_p
        self._ctx = self._lib.EVP_CIPHER_CTX_new()
        if not self._ctx:
            raise CryptoError("EVP_CIPHER_CTX_new failed")
        cipher = self._lib.EVP_aes_256_ctr()
        key_buf = (ctypes.c_ubyte * KEY_LEN).from_buffer_copy(key)
        iv_buf = (ctypes.c_ubyte * IV_LEN).from_buffer_copy(iv)
        if decrypt:
            rc = self._lib.EVP_DecryptInit_ex(
                ctypes.c_void_p(self._ctx),
                ctypes.c_void_p(cipher),
                None,
                key_buf,
                iv_buf,
            )
        else:
            rc = self._lib.EVP_EncryptInit_ex(
                ctypes.c_void_p(self._ctx),
                ctypes.c_void_p(cipher),
                None,
                key_buf,
                iv_buf,
            )
        if rc != 1:
            self.close()
            raise CryptoError("EVP_*Init_ex failed")
        self._decrypt = decrypt
        # Zero local ctypes copies best-effort.
        for i in range(KEY_LEN):
            key_buf[i] = 0
        for i in range(IV_LEN):
            iv_buf[i] = 0

    def update(self, data: bytes) -> bytes:
        if not data:
            return b""
        out = (ctypes.c_ubyte * (len(data) + 32))()
        out_len = ctypes.c_int(0)
        inp = (ctypes.c_ubyte * len(data)).from_buffer_copy(data)
        if self._decrypt:
            rc = self._lib.EVP_DecryptUpdate(
                ctypes.c_void_p(self._ctx),
                out,
                ctypes.byref(out_len),
                inp,
                len(data),
            )
        else:
            rc = self._lib.EVP_EncryptUpdate(
                ctypes.c_void_p(self._ctx),
                out,
                ctypes.byref(out_len),
                inp,
                len(data),
            )
        if rc != 1:
            raise CryptoError("EVP_*Update failed")
        return bytes(out[: out_len.value])

    def finalize(self) -> bytes:
        out = (ctypes.c_ubyte * 32)()
        out_len = ctypes.c_int(0)
        if self._decrypt:
            rc = self._lib.EVP_DecryptFinal_ex(
                ctypes.c_void_p(self._ctx), out, ctypes.byref(out_len)
            )
        else:
            rc = self._lib.EVP_EncryptFinal_ex(
                ctypes.c_void_p(self._ctx), out, ctypes.byref(out_len)
            )
        if rc != 1:
            raise CryptoError("EVP_*Final_ex failed")
        return bytes(out[: out_len.value])

    def close(self) -> None:
        if getattr(self, "_ctx", None):
            self._lib.EVP_CIPHER_CTX_free(ctypes.c_void_p(self._ctx))
            self._ctx = None

    def __del__(self) -> None:
        try:
            self.close()
        except Exception:
            pass


def _load_key(env_name: str) -> bytearray:
    raw = os.environ.get(env_name, "").strip()
    if len(raw) != 64 or any(ch not in "0123456789abcdef" for ch in raw):
        raise CryptoError(f"{env_name} must be 64 lowercase hex chars")
    return bytearray(bytes.fromhex(raw))


def hkdf_sha256(ikm: bytes, salt: bytes, info: bytes, length: int) -> bytes:
    if length <= 0 or length > 255 * 32:
        raise CryptoError("HKDF length invalid")
    if not salt:
        salt = b"\x00" * 32
    prk = hmac.new(salt, ikm, hashlib.sha256).digest()
    okm = b""
    previous = b""
    counter = 1
    while len(okm) < length:
        previous = hmac.new(prk, previous + info + bytes([counter]), hashlib.sha256).digest()
        okm += previous
        counter += 1
    return okm[:length]


def derive_keys(master: bytes | bytearray, salt: bytes) -> tuple[bytearray, bytearray]:
    enc = bytearray(hkdf_sha256(bytes(master), salt, ENC_INFO, KEY_LEN))
    mac = bytearray(hkdf_sha256(bytes(master), salt, MAC_INFO, KEY_LEN))
    if hmac.compare_digest(bytes(enc), bytes(mac)):
        raise CryptoError("enc/mac key collision")
    return enc, mac


def _lp(data: bytes) -> bytes:
    return len(data).to_bytes(8, "big") + data


def canonical_header(
    *,
    algorithm: str,
    key_id: str,
    salt: bytes,
    iv: bytes,
    aad: bytes,
) -> bytes:
    return b"".join(
        [
            _lp(algorithm.encode()),
            _lp(key_id.encode()),
            _lp(salt),
            _lp(iv),
            _lp(aad),
        ]
    )


def _sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(CHUNK), b""):
            digest.update(chunk)
    return digest.hexdigest()


def _private_dir(prefix: str) -> tempfile.TemporaryDirectory[str]:
    tmp = tempfile.TemporaryDirectory(prefix=prefix)
    Path(tmp.name).chmod(0o700)
    return tmp


def _atomic_replace(tmp_path: Path, final_path: Path) -> None:
    final_path.parent.mkdir(parents=True, exist_ok=True)
    with tmp_path.open("r+b") as handle:
        handle.flush()
        os.fsync(handle.fileno())
    os.replace(tmp_path, final_path)
    # Best-effort dir fsync.
    dir_fd = os.open(str(final_path.parent), os.O_RDONLY)
    try:
        os.fsync(dir_fd)
    finally:
        os.close(dir_fd)
    final_path.chmod(0o600)


def _zero_mutable(buf: bytearray) -> None:
    for i in range(len(buf)):
        buf[i] = 0


def encrypt_file(
    plaintext: Path,
    ciphertext: Path,
    *,
    key_env: str,
    key_id: str,
    aad: bytes,
) -> dict[str, Any]:
    if not key_id or any(ord(ch) < 32 for ch in key_id):
        raise CryptoError("key_id invalid")
    try:
        aad_text = aad.decode("utf-8")
    except UnicodeDecodeError as error:
        raise CryptoError("AAD must be UTF-8") from error
    if plaintext.is_symlink() or ciphertext.is_symlink():
        raise CryptoError("symlink rejected")
    master = _load_key(key_env)
    salt = os.urandom(SALT_LEN)
    iv = os.urandom(IV_LEN)
    enc_key, mac_key = derive_keys(master, salt)
    tmp = _private_dir("markhand-enc-")
    try:
        out_tmp = Path(tmp.name) / "out.enc"
        mac = hmac.new(bytes(mac_key), digestmod=hashlib.sha256)
        mac.update(canonical_header(algorithm=ALGORITHM, key_id=key_id, salt=salt, iv=iv, aad=aad))
        plain_digest = hashlib.sha256()
        cipher_digest = hashlib.sha256()
        ctr = _AesCtr(bytes(enc_key), iv, decrypt=False)
        try:
            with plaintext.open("rb") as src, out_tmp.open("wb") as dst:
                while True:
                    chunk = src.read(CHUNK)
                    if not chunk:
                        break
                    plain_digest.update(chunk)
                    ct = ctr.update(chunk)
                    mac.update(ct)
                    cipher_digest.update(ct)
                    dst.write(ct)
                tail = ctr.finalize()
                if tail:
                    mac.update(tail)
                    cipher_digest.update(tail)
                    dst.write(tail)
                dst.flush()
                os.fsync(dst.fileno())
        finally:
            ctr.close()
        tag = mac.digest()
        _atomic_replace(out_tmp, ciphertext)
        return {
            "algorithm": ALGORITHM,
            "keyId": key_id,
            "kdf": KDF,
            "saltHex": salt.hex(),
            "ivHex": iv.hex(),
            "macHex": tag.hex(),
            "aad": aad_text,
            "plaintextSha256": plain_digest.hexdigest(),
            "ciphertextSha256": cipher_digest.hexdigest(),
        }
    finally:
        _zero_mutable(enc_key)
        _zero_mutable(mac_key)
        _zero_mutable(master)
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
    try:
        salt = bytes.fromhex(str(meta["saltHex"]))
        iv = bytes.fromhex(str(meta["ivHex"]))
        tag = bytes.fromhex(str(meta["macHex"]))
    except (KeyError, ValueError) as error:
        raise CryptoError(f"envelope metadata incomplete: {error}") from error
    if len(salt) != SALT_LEN or len(iv) != IV_LEN or len(tag) != MAC_LEN:
        raise CryptoError("salt/iv/mac length invalid")
    if _sha256_file(ciphertext) != meta.get("ciphertextSha256"):
        raise CryptoError("ciphertext digest mismatch")
    if ciphertext.stat().st_size <= 0:
        raise CryptoError("ciphertext truncated/empty")

    master = _load_key(key_env)
    enc_key, mac_key = derive_keys(master, salt)
    tmp = _private_dir("markhand-dec-")
    try:
        # Pass 1: authenticate ciphertext stream before exposing plaintext.
        mac = hmac.new(bytes(mac_key), digestmod=hashlib.sha256)
        mac.update(canonical_header(algorithm=ALGORITHM, key_id=key_id, salt=salt, iv=iv, aad=aad))
        with ciphertext.open("rb") as src:
            while True:
                chunk = src.read(CHUNK)
                if not chunk:
                    break
                mac.update(chunk)
        if not hmac.compare_digest(mac.digest(), tag):
            raise CryptoError("HMAC verification failed (tamper/wrong key)")

        # Pass 2: decrypt to private temp, then atomic publish.
        out_tmp = Path(tmp.name) / "out.bin"
        plain_digest = hashlib.sha256()
        ctr = _AesCtr(bytes(enc_key), iv, decrypt=True)
        try:
            with ciphertext.open("rb") as src, out_tmp.open("wb") as dst:
                while True:
                    chunk = src.read(CHUNK)
                    if not chunk:
                        break
                    pt = ctr.update(chunk)
                    plain_digest.update(pt)
                    dst.write(pt)
                tail = ctr.finalize()
                if tail:
                    plain_digest.update(tail)
                    dst.write(tail)
                dst.flush()
                os.fsync(dst.fileno())
        finally:
            ctr.close()
        if plain_digest.hexdigest() != meta.get("plaintextSha256"):
            raise CryptoError("plaintext digest mismatch after decrypt")
        _atomic_replace(out_tmp, plaintext)
    finally:
        _zero_mutable(enc_key)
        _zero_mutable(mac_key)
        _zero_mutable(master)
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
