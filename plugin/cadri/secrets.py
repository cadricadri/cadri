"""Remove credential values before Cadri stores a session or a memory.

Cadri keeps transcripts and lessons, not secrets. The local transcript file is
left as the harness wrote it. Callers redact the copy they are about to upload,
digest, or commit.

Two checks, applied in order:

- Known credential shapes. These are removed wherever they appear: provider
  tokens, JWTs, passwords embedded in URLs, and crypto key material. That
  includes PEM and PGP private keys, BIP39 seed phrases, Bitcoin WIF and xprv
  keys, Ethereum private keys, keystore ciphertexts, and Solana keypair arrays.
- An assignment to a secret-like name (`password`, `api_key`, `token`, …) whose
  value looks like a literal credential rather than a placeholder, a variable,
  or ordinary code.

A password written as a normal word, with no such shape and no assignment, is
not detectable. The memory instructions cover that case.
"""

from __future__ import annotations

import math
import re
from collections import Counter
from pathlib import Path

REDACTED = "[redacted]"
PRIVATE_KEY = "[redacted private key]"

_PLACEHOLDERS = {
    "changeme",
    "change-me",
    "password",
    "passwd",
    "secret",
    "token",
    "example",
    "placeholder",
    "redacted",
    REDACTED,
    PRIVATE_KEY,
    "todo",
    "fixme",
    "dummy",
    "fake",
    "sample",
    "testing",
    "test",
    "none",
    "null",
    "undefined",
    "false",
    "true",
    "xxx",
    "xxxx",
    "xxxxx",
    "xxxxxx",
    "xxxxxxxx",
    "your-api-key",
    "your_api_key",
    "your-password",
    "your_password",
    "your-token",
    "your_token",
    "insert-token",
    "replace-me",
    "replace_me",
    "password123",
    "secret123",
    "token123",
}

# WIF (5, K, L mainnet; 9, c testnet) and BIP32 extended private keys.
# xpub and a public address are not spending keys and are left in place.
_WIF = r"[5KL9c][1-9A-HJ-NP-Za-km-z]{50,51}"
_XPRV = r"[xtuyvz]prv[1-9A-HJ-NP-Za-km-z]{100,112}"

_TOKEN = re.compile(
    "(?<![A-Za-z0-9])(?:"
    + "|".join(
        (
            r"github_pat_[A-Za-z0-9_]{22,}",
            r"gh[pousr]_[A-Za-z0-9]{36,}",
            r"gl(?:pat|ptt|dt|rt)-[A-Za-z0-9_\-]{16,}",
            r"xox[baprs]-[A-Za-z0-9\-]{10,}",
            r"https://hooks\.slack\.com/services/T[A-Z0-9]+/B[A-Z0-9]+/[A-Za-z0-9]+",
            r"(?:sk|rk)_(?:live|test)_[A-Za-z0-9]{10,}",
            r"whsec_[A-Za-z0-9]{10,}",
            r"sk-(?:ant-|proj-|svcacct-|admin-)?[A-Za-z0-9_\-]{20,}",
            r"(?:AKIA|ASIA|AIDA|AROA)[0-9A-Z]{16}",
            r"AIza[0-9A-Za-z_\-]{35}",
            r"ya29\.[0-9A-Za-z_\-]{20,}",
            r"npm_[A-Za-z0-9]{36,}",
            r"hf_[A-Za-z0-9]{30,}",
            r"dop_v1_[a-f0-9]{64}",
            r"SG\.[A-Za-z0-9_\-]{22}\.[A-Za-z0-9_\-]{43}",
            r"shp(?:at|ca|ss)_[a-fA-F0-9]{32,}",
            r"pypi-AgEIcHlwaS5vcmc[A-Za-z0-9_\-]{20,}",
            r"eyJ[A-Za-z0-9_\-]{10,}\.[A-Za-z0-9_\-]{10,}\.[A-Za-z0-9_\-]{10,}",
            r"sbp_[A-Za-z0-9]{20,}",
            r"lin_api_[A-Za-z0-9]{20,}",
            r"ntn_[A-Za-z0-9]{40,}",
            _XPRV,
            _WIF,
        )
    )
    + ")(?![A-Za-z0-9])"
)

_PEM = re.compile(
    r"-----BEGIN [A-Z0-9 ]*PRIVATE KEY-----.*?-----END [A-Z0-9 ]*PRIVATE KEY-----",
    re.DOTALL,
)
_PGP = re.compile(
    r"-----BEGIN PGP PRIVATE KEY BLOCK-----.*?-----END PGP PRIVATE KEY BLOCK-----",
    re.DOTALL,
)
_HEX64 = re.compile(r"(?i)(?<![A-Za-z0-9])((?:0x)?[a-f0-9]{64})(?![A-Za-z0-9])")
_SECRET_CONTEXT = re.compile(
    r"(?i)\b(?:(?:private|priv|secret)[\s_-]*keys?|seeds?(?:[\s_-]*phrase)?|mnemonics?|keystores?|keypairs?)\b"
)
_PUBLIC_FIELD = re.compile(
    r"(?i)(?:hash|tx|txid|txhash|transaction|signature|digest|block|proof|root|address|contract|calldata|input|topic)$"
)
_SOLANA_KEYPAIR = re.compile(r"\[\s*(?:\d{1,3}\s*,\s*){63}\d{1,3}\s*\]")
_KEYSTORE = re.compile(r'(?i)("ciphertext"\s*:\s*")([0-9a-f]{32,})(")')
_MNEMONIC_WORD = re.compile(r"\b[A-Za-z]{3,8}\b")
_MNEMONIC_LENGTHS = (24, 21, 18, 15, 12)
_BIP39 = frozenset(Path(__file__).with_name("bip39_english.txt").read_text(encoding="utf-8").split())

_BEARER = re.compile(r"(?i)\b(Bearer|Basic)\s+([A-Za-z0-9\-._~+/]{8,}={0,3})")

_URL = re.compile(
    r"(?P<scheme>[a-z][a-z0-9+.-]*://)(?P<user>[^/\s:@]+):(?P<password>[^@\s]{6,})@"
)

_ASSIGN = re.compile(
    r"""(?x)
    (?<![A-Za-z0-9])
    ["']?
    (?P<name>[A-Za-z][A-Za-z0-9_.-]{0,60}?)
    ["']?
    \s*[:=]\s*
    ["']?
    (?P<value>[^"'\\\s,}\]]{6,200})
    """
)

_PASSWORD_WORDS = {"password", "passwords", "passwd", "pwd"}
_KEY_HINTS = {"api", "secret", "access", "private", "priv", "signing", "auth", "wallet", "mnemonic", "seed"}


def redact(text: str) -> str:
    """Return `text` with credential values replaced by `[redacted]`."""
    if not text:
        return text
    text = _PEM.sub(PRIVATE_KEY, text)
    text = _PGP.sub(PRIVATE_KEY, text)
    text = _TOKEN.sub(_token, text)
    text = _redact_hex_keys(text)
    text = _SOLANA_KEYPAIR.sub(_solana_keypair, text)
    text = _redact_keystore(text)
    text = _redact_mnemonics(text)
    text = _BEARER.sub(_bearer, text)
    text = _URL.sub(_url, text)
    text = _ASSIGN.sub(_assign, text)
    return text


def _redact_hex_keys(text: str) -> str:
    """Remove a 32-byte hex key. A transaction hash and a public address stay.

    An Ethereum private key is 32 bytes. So is a transaction hash, so a hash
    named as one (`hash`, `tx`, `signature`, …) is kept, and so is a 20-byte
    address. A key pasted on its own, labeled as a private key, or placed in a
    message body is removed.
    """

    def repl(match: re.Match[str]) -> str:
        value = match.group(1)
        if _low_diversity(value):
            return match.group(0)
        line = _line_around(text, match.start(), match.end())
        if line.strip().strip("`'\"") == value or _SECRET_CONTEXT.search(line):
            return REDACTED
        # A chat message that is only the key, or a field that names one.
        # A hash mentioned inside a longer message stays.
        field = _field_before(text, match.start())
        exact = _quoted_value_is_exact(text, match.start(), match.end())
        if exact and field and not _PUBLIC_FIELD.search(field):
            if _SECRET_CONTEXT.search(field) or field.lower() in {"content", "text", "message", "body", "value", "key"}:
                return REDACTED
        return match.group(0)

    return _HEX64.sub(repl, text)


def _solana_keypair(match: re.Match[str]) -> str:
    numbers = [int(item) for item in re.findall(r"\d+", match.group(0))]
    if len(numbers) == 64 and all(0 <= number <= 255 for number in numbers):
        return REDACTED
    return match.group(0)


def _redact_keystore(text: str) -> str:
    """Remove the ciphertext of an Ethereum keystore (UTC JSON)."""

    def repl(match: re.Match[str]) -> str:
        window = text[max(0, match.start() - 800) : match.end() + 800].lower()
        if any(marker in window for marker in ("kdf", "scrypt", "pbkdf", '"crypto"', '"cipher"')):
            return f"{match.group(1)}{REDACTED}{match.group(3)}"
        return match.group(0)

    return _KEYSTORE.sub(repl, text)


def _redact_mnemonics(text: str) -> str:
    """Remove a BIP39 seed phrase of 12, 15, 18, 21, or 24 words."""
    words = list(_MNEMONIC_WORD.finditer(text))
    if len(words) < 12:
        return text
    spans: list[tuple[int, int]] = []
    index = 0
    while index <= len(words) - 12:
        taken = 0
        for length in _MNEMONIC_LENGTHS:
            if index + length > len(words):
                continue
            chunk = words[index : index + length]
            if all(word.group(0).lower() in _BIP39 for word in chunk) and _words_are_consecutive(text, chunk):
                taken = length
                break
        if taken:
            spans.append((words[index].start(), words[index + taken - 1].end()))
            index += taken
        else:
            index += 1
    for start, end in reversed(spans):
        text = text[:start] + REDACTED + text[end:]
    return text


def _words_are_consecutive(text: str, words: list[re.Match[str]]) -> bool:
    """True when nothing but separators sits between the words.

    A numbered list (`1. abandon`) and a comma-separated phrase count. A
    sentence with other words between them does not.
    """
    for left, right in zip(words, words[1:]):
        if re.search(r"[A-Za-z]", text[left.end() : right.start()]):
            return False
    return True


def _line_around(text: str, start: int, end: int) -> str:
    line_start = text.rfind("\n", 0, start) + 1
    line_end = text.find("\n", end)
    if line_end < 0:
        line_end = len(text)
    return text[line_start:line_end]


def _quoted_value_is_exact(text: str, start: int, end: int) -> bool:
    before = text[:start].rstrip()
    after = text[end:].lstrip()
    if not before or before[-1] not in "\"'":
        return False
    return bool(after) and after[0] in "\"'"


def _field_before(text: str, start: int) -> str | None:
    before = text[max(0, start - 80) : start]
    field = re.search(r"""["']?([A-Za-z][A-Za-z0-9_.-]*)["']?\s*[:=]\s*["']?\s*$""", before)
    return field.group(1) if field else None


def _low_diversity(value: str) -> bool:
    body = value.lower().removeprefix("0x")
    return len(body) >= 8 and len(set(body)) <= 2


def _token(match: re.Match[str]) -> str:
    value = match.group(0)
    if _is_placeholder(value) or "example" in value.lower():
        return value
    body = value.rsplit("_", 1)[-1]
    if len(body) >= 8 and len(set(body)) <= 2:
        return value
    return REDACTED


def _bearer(match: re.Match[str]) -> str:
    token = match.group(2)
    if _is_placeholder(token) or "example" in token.lower():
        return match.group(0)
    if match.group(1).lower() == "basic" or _entropy(token) >= 3.0:
        return f"{match.group(1)} {REDACTED}"
    return match.group(0)


def _url(match: re.Match[str]) -> str:
    password = match.group("password")
    if _is_placeholder(password):
        return match.group(0)
    return f"{match.group('scheme')}{match.group('user')}:{REDACTED}@"


def _assign(match: re.Match[str]) -> str:
    name = match.group("name")
    value = match.group("value")
    words = _words(name)
    if not _secret_name(words) or not _literal_secret(words, value):
        return match.group(0)
    start = match.start("value") - match.start()
    whole = match.group(0)
    return whole[:start] + REDACTED + whole[start + len(value) :]


def _words(name: str) -> list[str]:
    spaced = re.sub(r"([a-z0-9])([A-Z])", r"\1_\2", name)
    return [word for word in re.split(r"[^A-Za-z0-9]+", spaced.lower()) if word]


def _secret_name(words: list[str]) -> bool:
    if not words:
        return False
    if words[-1] in _PASSWORD_WORDS:
        return True
    if any(
        word in words
        for word in ("secret", "secrets", "credential", "credentials", "apikey", "apikeys", "mnemonic", "keystore", "seedphrase")
    ):
        return True
    if words[-1] in {"token", "tokens"}:
        return True
    if words[-1] in {"key", "keys"}:
        return any(word in words for word in _KEY_HINTS)
    return False


def _literal_secret(words: list[str], value: str) -> bool:
    if not _plausible_literal(value):
        return False
    # OpenCode records the first streamed token as `time.firstToken`, a millisecond
    # timestamp. A bare integer assigned to a token-shaped name is a count or a
    # clock, and replacing it (or the comma after it) makes the session JSON unreadable.
    if words[-1] in {"token", "tokens"} and value.isdigit():
        return False
    if words[-1] in _PASSWORD_WORDS or len(value) >= 20:
        return True
    # A hyphenated name (`the-deploy-token`) is not a credential. A digit or a
    # symbol other than hyphen is.
    return bool(re.search(r"\d", value) or re.search(r"[^A-Za-z0-9_\-]", value))


def _plausible_literal(value: str) -> bool:
    if len(value) < 6 or len(value) > 200 or _is_placeholder(value):
        return False
    if re.search(r"[()\[\]{}]", value):
        return False
    if "." in value or "://" in value or value.startswith(("/", ".", "~", "$", "http")):
        return False
    lowered = value.lower()
    if "environ" in lowered or "process.env" in lowered or "getenv" in lowered:
        return False
    if re.fullmatch(r"[A-Za-z_][A-Za-z0-9_]*", value) and not re.search(r"\d", value) and len(value) < 24:
        return False
    return True


def _is_placeholder(value: str) -> bool:
    raw = value.strip().strip("'\"`")
    lowered = raw.lower()
    if lowered in _PLACEHOLDERS:
        return True
    if any(marker in lowered for marker in ("example", "placeholder", "redacted", "changeme")):
        return True
    if lowered.startswith(("your-", "your_")):
        return True
    if re.fullmatch(r"[\*.xX\-_]{3,}", raw) or re.fullmatch(r"<[^>\n]{0,40}>", raw):
        return True
    return raw.startswith("${") or raw.startswith("$")


def _entropy(value: str) -> float:
    if not value:
        return 0.0
    counts = Counter(value)
    length = len(value)
    return -sum((count / length) * math.log2(count / length) for count in counts.values())
