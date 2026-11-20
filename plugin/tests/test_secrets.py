"""Credential values are removed before Cadri stores a session or a memory."""

from __future__ import annotations

import io
import json
import os
import tempfile
import unittest
from contextlib import redirect_stderr
from pathlib import Path
from unittest import mock

from support import PLUGIN

from cadri import capture
from cadri.install import bundles
from cadri.memory import digests
from cadri.secrets import redact
from cadri.upload import without_secrets

GITHUB = "ghp_" + "A1b2C3d4E5f6G7h8I9j0K1l2M3n4O5p6Q7r8"
OPENAI = "sk-proj-" + "a1B2c3D4e5F6g7H8i9J0"
STRIPE = "sk_live_" + "abcDEFghiJklMN1234"
AWS = "AKIA" + "1234567890ABCDEF"
AWS_EXAMPLE = "AKIAIOSFODNN7EXAMPLE"


class RedactTests(unittest.TestCase):
    def test_known_tokens_are_removed_and_the_sentence_stays(self):
        text = f"Use {GITHUB} and {STRIPE} and {OPENAI} and {AWS} for the deploy."
        cleaned = redact(text)
        for secret in (GITHUB, STRIPE, OPENAI, AWS):
            self.assertNotIn(secret, cleaned)
        self.assertIn("Use [redacted] and [redacted]", cleaned)
        self.assertIn("for the deploy.", cleaned)

    def test_a_private_key_block_is_removed(self):
        text = "-----BEGIN OPENSSH PRIVATE KEY-----\nb3BlbnNzaC1rZXkAAAAA\n-----END OPENSSH PRIVATE KEY-----\n"
        cleaned = redact(text)
        self.assertNotIn("b3BlbnNzaC1rZXkAAAAA", cleaned)
        self.assertIn("[redacted private key]", cleaned)

    def test_a_url_password_is_removed_and_the_host_stays(self):
        cleaned = redact("postgres://app:s3cret-pass@db.internal:5432/app")
        self.assertEqual(cleaned, "postgres://app:[redacted]@db.internal:5432/app")

    def test_an_assigned_password_is_removed_inside_json(self):
        raw = '{"password": "hunter2", "note": "keep the host"}\n'
        cleaned = redact(raw)
        self.assertNotIn("hunter2", cleaned)
        parsed = json.loads(cleaned)
        self.assertEqual(parsed["password"], "[redacted]")
        self.assertEqual(parsed["note"], "keep the host")

    def test_an_opencode_first_token_timestamp_stays_so_the_session_json_parses(self):
        raw = json.dumps(
            {
                "info": {"id": "ses_1"},
                "messages": [
                    {
                        "time": {"created": 1795191015032, "firstToken": 1795191017586},
                        "text": "rename the button",
                    }
                ],
            }
        )
        cleaned = redact(raw)
        parsed = json.loads(cleaned)
        self.assertEqual(parsed["messages"][0]["time"]["firstToken"], 1795191017586)
        self.assertEqual(parsed["messages"][0]["text"], "rename the button")
        self.assertEqual(
            redact('{"api_token": "not-a-provider-token-value"}'),
            '{"api_token": "[redacted]"}',
        )

    def test_a_bearer_token_and_a_jwt_are_removed(self):
        jwt = "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiJ1c2VyIn0.signaturevalue1"
        cleaned = redact(f"Authorization: Bearer {jwt}")
        self.assertNotIn(jwt, cleaned)
        self.assertIn("Bearer [redacted]", cleaned)

    def test_ordinary_text_names_and_examples_stay(self):
        samples = [
            "Set API_KEY in the environment. Do not commit .env.",
            "Refer to it as $API_KEY.",
            "https://github.com/cadri/cadri",
            f"The sample key is {AWS_EXAMPLE}.",
            "password = os.environ[\"PASSWORD\"]",
            "const token = getToken()",
            "token=cadri-dev",
            "secret: the-deploy-token",
            '{"type":"user","message":{"content":"When do we deploy?"}}',
            "Deploys happen on Wednesday, before the release cut.",
        ]
        for sample in samples:
            self.assertEqual(redact(sample), sample, sample)

    def test_upload_drops_the_secret_and_says_so(self):
        payload = {"thread_id": "t1", "content": f"the key is {GITHUB}", "harness": "cursor"}
        notes = io.StringIO()
        with redirect_stderr(notes):
            cleaned = without_secrets(payload)
        self.assertEqual(payload["content"], f"the key is {GITHUB}")
        self.assertNotIn(GITHUB, cleaned["content"])
        self.assertIn("[redacted]", cleaned["content"])
        self.assertEqual(cleaned["thread_id"], "t1")
        self.assertIn("removed secrets", notes.getvalue())

    def test_capture_leaves_the_local_transcript_and_uploads_without_the_secret(self):
        token = GITHUB
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "session.jsonl"
            path.write_text(json.dumps({"type": "user", "text": token}) + "\n", encoding="utf-8")
            payload = capture.build_payload(
                "claude",
                {"session_id": "s1", "transcript_path": str(path), "hook_event_name": "Stop"},
            )
            self.assertIn(token, payload["content"])
            response = mock.MagicMock()
            response.status = 201
            response.read.return_value = b"{}"
            response.__enter__.return_value = response
            with (
                mock.patch("cadri.capture.auth.server_url", return_value="http://127.0.0.1:9"),
                mock.patch("cadri.upload.cadri_auth.auth_headers", return_value={}),
                mock.patch("cadri.upload.urllib.request.urlopen", return_value=response) as opened,
            ):
                capture.upload(payload)
            body = json.loads(opened.call_args.args[0].data.decode())
            self.assertNotIn(token, body["content"])
            self.assertIn("[redacted]", body["content"])
            self.assertIn(token, path.read_text(encoding="utf-8"))

    def test_a_digest_does_not_keep_a_pasted_key(self):
        with tempfile.TemporaryDirectory() as tmp:
            with mock.patch.dict(os.environ, {"CADRI_HOME": tmp}):
                path = digests.save(
                    "claude",
                    "s1",
                    f"the key is {STRIPE}",
                    "I will remember it",
                    0,
                    [],
                    plans=[{"title": "Auth", "body": f"send {STRIPE} in the header", "slug": "auth"}],
                )
                raw = path.read_text(encoding="utf-8")
                self.assertNotIn(STRIPE, raw)
                self.assertIn("[redacted]", raw)
                shown = digests.render([json.loads(raw)])
                self.assertNotIn(STRIPE, shown)

    def test_the_keeper_is_told_never_to_store_a_secret(self):
        agent = (PLUGIN / "agents" / "cadri-memory-keeper.md").read_text(encoding="utf-8")
        maintain = (PLUGIN / "skills" / "cadri-memory" / "references" / "maintain.md").read_text(encoding="utf-8")
        self.assertIn("Never store a secret.", agent)
        self.assertIn("## Secrets", maintain)
        self.assertIn("Never store a secret.", maintain)
        self.assertIn("Never store a secret.", bundles.keeper_body())


_B58 = "123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz"
_EVM_KEY = "0x" + "a1b2c3d4" * 8
_TX = "0x" + "deadbeef" * 8
_ADDRESS = "0x" + "ab" * 20
_WIF = "K" + (_B58 * 2)[:50]
_XPRV = "xprv" + (_B58 * 3)[:107]
_XPUB = "xpub" + (_B58 * 3)[:107]
_SEED = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about"


def _pem(kind: str, body: str) -> str:
    return f"-----BEGIN {kind}-----\n{body}\n-----END {kind}-----\n"


class CryptoRedactTests(unittest.TestCase):
    """What a crypto team pastes by accident: keys that can spend, sign, or decrypt.

    A public address, an xpub, and a transaction hash are not spending keys.
    They stay, so a session about a contract or a revert is still readable.
    """

    def test_certificate_private_keys_are_removed_and_the_certificate_stays(self):
        certificate = _pem("CERTIFICATE", "MIIBtjCCAVugAwIBAgIUFAKE")
        bundle = "\n".join(
            [
                certificate,
                _pem("EC PRIVATE KEY", "MHCCAQEEIGnotarealkey"),
                _pem("RSA PRIVATE KEY", "MIIEowIBAAKCAQEAnotarealkey"),
                _pem("ENCRYPTED PRIVATE KEY", "MIIFHDBOBgkqhkiG9w0BBQ0wQTAp"),
            ]
        )
        cleaned = redact(bundle)
        self.assertIn("MIIBtjCCAVugAwIBAgIUFAKE", cleaned)
        self.assertNotIn("MHCCAQEEIGnotarealkey", cleaned)
        self.assertNotIn("MIIEowIBAAKCAQEAnotarealkey", cleaned)
        self.assertNotIn("MIIFHDBOBgkqhkiG9w0BBQ0wQTAp", cleaned)
        self.assertIn("[redacted private key]", cleaned)

    def test_a_private_key_pasted_into_json_is_removed(self):
        escaped = "-----BEGIN EC PRIVATE KEY-----\\nMHCCAQEEIGnotarealkey\\n-----END EC PRIVATE KEY-----"
        cleaned = redact(json.dumps({"content": escaped}))
        self.assertNotIn("MHCCAQEEIGnotarealkey", cleaned)
        self.assertIn("[redacted private key]", cleaned)

    def test_a_pgp_private_key_is_removed(self):
        block = _pem("PGP PRIVATE KEY BLOCK", "lQOYBGFAKEfake")
        cleaned = redact(block)
        self.assertNotIn("lQOYBGFAKEfake", cleaned)
        self.assertIn("[redacted private key]", cleaned)

    def test_an_ethereum_private_key_is_removed_and_a_tx_hash_and_address_stay(self):
        self.assertNotIn(_EVM_KEY, redact(_EVM_KEY))
        self.assertNotIn(_EVM_KEY, redact(f"private key: {_EVM_KEY}"))
        self.assertNotIn(_EVM_KEY, redact(json.dumps({"content": _EVM_KEY})))
        self.assertNotIn(_EVM_KEY, redact(json.dumps({"privateKey": _EVM_KEY})))
        self.assertIn(_EVM_KEY, redact(json.dumps({"hash": _EVM_KEY})))
        self.assertEqual(redact(f"the revert is on tx {_TX}"), f"the revert is on tx {_TX}")
        self.assertEqual(redact(f"send the fee to {_ADDRESS}"), f"send the fee to {_ADDRESS}")
        self.assertEqual(redact("0x" + "00" * 32), "0x" + "00" * 32)

    def test_bitcoin_wif_and_xprv_are_removed_and_xpub_stays(self):
        cleaned = redact(f"import {_WIF} and {_XPRV} but watch {_XPUB}")
        self.assertNotIn(_WIF, cleaned)
        self.assertNotIn(_XPRV, cleaned)
        self.assertIn(_XPUB, cleaned)

    def test_a_bip39_seed_phrase_is_removed(self):
        words = (PLUGIN / "cadri" / "bip39_english.txt").read_text(encoding="utf-8").split()
        self.assertEqual(len(words), 2048)
        numbered = "\n".join(f"{index}. {word}" for index, word in enumerate(_SEED.split(), start=1))
        long = " ".join(words[:24])
        for phrase in (_SEED, numbered, long):
            cleaned = redact(f"the backup is {phrase}")
            self.assertNotIn("abandon", cleaned)
            self.assertIn("the backup is", cleaned)
        almost = " ".join(words[:11] + ["deploy"])
        self.assertEqual(redact(almost), almost)
        self.assertEqual(redact("You can abandon one key if you are about to ship."), "You can abandon one key if you are about to ship.")

    def test_a_solana_keypair_and_an_ethereum_keystore_are_removed(self):
        keypair = "[" + ", ".join(str(number) for number in range(64)) + "]"
        self.assertNotIn("63", redact(f"id.json is {keypair}"))
        self.assertEqual(redact("[1, 2, 3, 4]"), "[1, 2, 3, 4]")
        secret = "a1b2c3d4e5f67890a1b2c3d4e5f67890"
        store = json.dumps({"crypto": {"cipher": "aes-128-ctr", "ciphertext": secret, "kdf": "scrypt"}})
        self.assertNotIn(secret, redact(store))
        self.assertIn("scrypt", redact(store))
        unrelated = json.dumps({"ciphertext": secret})
        self.assertIn(secret, redact(unrelated))
