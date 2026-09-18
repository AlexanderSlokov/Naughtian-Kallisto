"""The duck test, through hvac — the Python client most people actually use.

Nothing here knows it is not talking to Vault. Every call is the call an
application would make, and every assertion is about what hvac hands back, not
about the bytes on the wire: if Kallisto's JSON is subtly the wrong shape, hvac
raises or returns None and this fails, which is exactly the failure mode that
matters.
"""

import os
import sys

import hvac
import requests

ADDR = os.environ["VAULT_ADDR"]
TOKEN = os.environ.get("VAULT_TOKEN") or None

failures = []


def check(name, fn):
    try:
        fn()
        print(f"  ok    {name}")
    except Exception as exc:  # noqa: BLE001 - the report is the point
        print(f"  FAIL  {name}: {type(exc).__name__}: {exc}")
        failures.append(name)


client = hvac.Client(url=ADDR, token=TOKEN)

# --- the calls an SDK makes before it does anything useful -------------------

def startup():
    assert client.sys.is_initialized() is True, "is_initialized said no"
    assert client.sys.is_sealed() is False, "a loaded resolver reported sealed"
    health = client.sys.read_health_status(method="GET")
    body = health if isinstance(health, dict) else health.json()
    assert body["sealed"] is False, body
    # The fields ADR-0015 D4 adds so an operator can ask three machines whether
    # they agree. An SDK ignores them; a human needs them.
    assert body["kallisto_file_version"] >= 1, body
    assert body["kallisto_etag"], body


def lookup_self():
    got = client.auth.token.lookup_self()
    assert "policies" in got["data"], got


# --- reads ------------------------------------------------------------------

def read_secret():
    got = client.secrets.kv.v2.read_secret_version(
        path="app/db", mount_point="secret", raise_on_deleted_version=True
    )
    assert got["data"]["data"]["password"] == "hunter2", got
    assert got["data"]["metadata"]["version"] >= 1, got


def read_nested():
    got = client.secrets.kv.v2.read_secret_version(
        path="app/sub/deep", mount_point="secret", raise_on_deleted_version=True
    )
    assert got["data"]["data"]["k"] == "v", got


def read_current_version_explicitly():
    version = client.sys.read_health_status(method="GET")["kallisto_file_version"]
    got = client.secrets.kv.v2.read_secret_version(
        path="app/db", mount_point="secret", version=version,
        raise_on_deleted_version=True,
    )
    assert got["data"]["data"]["password"] == "hunter2", got


def read_other_version_is_absent():
    try:
        client.secrets.kv.v2.read_secret_version(
            path="app/db", mount_point="secret", version=999_999,
            raise_on_deleted_version=True,
        )
    except hvac.exceptions.InvalidPath:
        return
    raise AssertionError("a version this file does not hold was served")


def missing_secret_is_invalid_path():
    # Inside the namespace this token is allowed to read. A path *outside* it
    # answers 403 whether or not it exists, deliberately — a 404 there would be
    # an existence oracle — so testing with one would assert the wrong thing.
    try:
        client.secrets.kv.v2.read_secret_version(
            path="app/nothing-here", mount_point="secret", raise_on_deleted_version=True
        )
    except hvac.exceptions.InvalidPath:
        return
    raise AssertionError("a missing secret did not raise InvalidPath")


def an_unreadable_path_is_403_whether_or_not_it_exists():
    # The anti-oracle property, from the client's side: `other/thing` exists and
    # `other/absent` does not, and the token may read neither. They must be
    # indistinguishable.
    codes = []
    for path in ("other/thing", "other/absent"):
        try:
            client.secrets.kv.v2.read_secret_version(
                path=path, mount_point="secret", raise_on_deleted_version=True
            )
            codes.append(200)
        except hvac.exceptions.Forbidden:
            codes.append(403)
        except hvac.exceptions.InvalidPath:
            codes.append(404)
    assert codes == [403, 403], f"existence leaked through the status code: {codes}"


def list_secrets():
    got = client.secrets.kv.v2.list_secrets(path="app", mount_point="secret")
    keys = got["data"]["keys"]
    assert "db" in keys, keys
    # Vault reports a directory with a trailing slash, once, however many
    # secrets are under it.
    assert "sub/" in keys, keys


def read_metadata():
    got = client.secrets.kv.v2.read_secret_metadata(path="app/db", mount_point="secret")
    assert "versions" in got["data"], got


# --- writes, all of which must be refused -----------------------------------

def every_write_is_refused():
    attempts = {
        "create_or_update": lambda: client.secrets.kv.v2.create_or_update_secret(
            path="app/db", secret={"x": "y"}, mount_point="secret"
        ),
        "patch": lambda: client.secrets.kv.v2.patch(
            path="app/db", secret={"x": "y"}, mount_point="secret"
        ),
        "delete_latest": lambda: client.secrets.kv.v2.delete_latest_version_of_secret(
            path="app/db", mount_point="secret"
        ),
        "delete_versions": lambda: client.secrets.kv.v2.delete_secret_versions(
            path="app/db", versions=[1], mount_point="secret"
        ),
        "undelete": lambda: client.secrets.kv.v2.undelete_secret_versions(
            path="app/db", versions=[1], mount_point="secret"
        ),
        "destroy": lambda: client.secrets.kv.v2.destroy_secret_versions(
            path="app/db", versions=[1], mount_point="secret"
        ),
    }
    for name, call in attempts.items():
        try:
            call()
        except hvac.exceptions.Forbidden:
            continue
        except hvac.exceptions.VaultError as exc:
            raise AssertionError(f"{name} failed, but not with 403: {exc!r}") from exc
        raise AssertionError(f"{name} SUCCEEDED — this server must not write")


def the_refusal_uses_vaults_own_wording():
    # Clients match on this string. It is compatibility surface, not a message
    # we are free to improve.
    response = requests.put(
        f"{ADDR}/v1/secret/data/app/db",
        json={"data": {"x": "y"}},
        headers={"X-Vault-Token": TOKEN} if TOKEN else {},
        timeout=5,
    )
    assert response.status_code == 403, response.status_code
    assert response.json()["errors"] == ["permission denied"], response.text


def metrics_are_scrapeable():
    response = requests.get(f"{ADDR}/v1/sys/metrics", timeout=5)
    assert response.status_code == 200, response.status_code
    assert "kallisto_access_log_dropped_total" in response.text, response.text[:200]


print("python / hvac")
for name, fn in [
    ("startup calls", startup),
    ("auth/token/lookup-self", lookup_self),
    ("read a secret", read_secret),
    ("read a nested secret", read_nested),
    ("read ?version=<current>", read_current_version_explicitly),
    ("read ?version=<other> is 404", read_other_version_is_absent),
    ("missing secret is InvalidPath", missing_secret_is_invalid_path),
    ("an unreadable path hides whether it exists", an_unreadable_path_is_403_whether_or_not_it_exists),
    ("list", list_secrets),
    ("read metadata", read_metadata),
    ("every write is refused", every_write_is_refused),
    ("refusal uses Vault's wording", the_refusal_uses_vaults_own_wording),
    ("sys/metrics", metrics_are_scrapeable),
]:
    check(name, fn)

if failures:
    print(f"\n{len(failures)} failure(s): {', '.join(failures)}")
    sys.exit(1)
print("\nall python checks passed")
