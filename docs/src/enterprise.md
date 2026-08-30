# Restricted networks and enterprise

`mind` makes network calls in two places. Source operations (`meld`, `sync`,
`upgrade`) are `git` subprocesses spawned with the parent environment inherited,
so your existing git proxy, CA, and credential configuration applies unchanged.
`mind evolve` speaks HTTPS in-process (as of 0.26.0; it previously shelled out to
`curl` or `wget`), reading proxy settings from the environment and verifying
against the machine's certificate store. `install.sh` still uses `curl` or
`wget`, since it runs before `mind` exists. Where the three differ, each section
below says so.

`mind` sets or strips nothing from the environment (in TUI mode it sets
`GIT_TERMINAL_PROMPT=0` and wraps `GIT_SSH_COMMAND` to suppress interactive git
prompts; immaterial for non-interactive CLI use), and there is no telemetry or
phone-home. This page lists the egress endpoints a firewall allowlist needs and
the knobs for proxies, custom CAs, private repos, IT-managed binaries, and
air-gapped installs.

For machine-wide policy enforcement (trusted-source allowlist, pinning, lobe
lock), see [Managed policy](policy.md); this page shows how those controls fit an
enterprise deployment.

## Egress endpoints

A corporate allowlist needs the hosts each operation contacts:

- **Melding and syncing sources.** `git clone` / `git fetch` against whatever
  hosts you meld (GitHub, GitHub Enterprise, an internal GitLab, an SSH remote,
  or a local path). No fixed host: it is exactly the source URLs you register.
- **`mind evolve` (binary self-update).**
  - `https://api.github.com/repos/jaemk/mind/releases/latest` (skipped with
    `--to <v>`, see below).
  - `https://github.com/jaemk/mind/releases/download/...` for the release asset
    and its `SHA256SUMS`. These redirect to
    `https://release-assets.githubusercontent.com/...`; allowlist that host.
    `*.githubusercontent.com` is the most resilient pattern: GitHub has migrated
    release assets between CDN subdomains. `objects.githubusercontent.com` may
    still be needed for older GitHub Enterprise Server deployments.
- **`install.sh` (install script).** The same two `github.com` /
  `release-assets.githubusercontent.com` download URLs, plus
  `https://raw.githubusercontent.com/jaemk/mind/main/resources/install.sh` for
  the script itself, and `https://api.github.com/...` unless you pin
  `MIND_VERSION`.

## Proxies

`mind` reads proxy environment variables in-process for `evolve`, and the `git`,
`curl`, and `wget` subprocesses read them from the parent process. Behavior
differs by tool:

- **git** honors `HTTPS_PROXY`, `HTTP_PROXY`, `NO_PROXY`, and their lowercase
  forms, plus its own config:

  ```
  git config --global http.proxy http://proxy.corp.example:8080
  ```

  For NTLM/Kerberos proxy auth, set `http.proxyAuthMethod`:

  ```
  git config --global http.proxyAuthMethod negotiate
  ```

- **`mind evolve`** honors `ALL_PROXY`, `HTTPS_PROXY`, and `HTTP_PROXY` (first
  one set wins, in that order), each in either case, plus `NO_PROXY` /
  `no_proxy`. A proxy needing credentials must carry them in the URL
  (`http://user:pass@proxy.corp.example:8080`).

- **curl** (used by `install.sh`) intentionally ignores uppercase `HTTP_PROXY`.
  Set `HTTPS_PROXY` or the lowercase form `https_proxy`.

- **wget** (fallback when curl is absent) reads only lowercase forms
  (`https_proxy`, `http_proxy`, `no_proxy`); uppercase variables are ignored.

The safest approach is to export both cases:

```
export HTTPS_PROXY=http://proxy.corp.example:8080
export https_proxy=http://proxy.corp.example:8080
export NO_PROXY=localhost,127.0.0.1
export no_proxy=localhost,127.0.0.1
```

For NTLM or Kerberos proxy authentication during `install.sh`, `curl` reads
`~/.curlrc` (mind exposes no curl argument knob). Add `proxy-negotiate` there:

```
proxy = http://proxy.corp.example:8080
proxy-negotiate
```

`~/.curlrc` does not apply to `evolve`, which no longer shells out to curl; it
speaks HTTP itself and supports only URL-embedded proxy credentials.

`mind` neither sets nor unsets any of these, so whatever works for a bare
`git clone` works for every `mind` git operation.

## Custom CA / TLS-intercepting proxy

Behind a proxy that re-signs TLS with a corporate root CA, the CA has to be
trusted by whatever makes the connection. Installing it in the **system trust
store** covers everything below at once and is the recommended fix.

- **`mind evolve`** verifies against the system trust store directly: the
  keychain on macOS, the certificate stores on Windows, and on Linux the
  OpenSSL-convention bundle, which means `SSL_CERT_FILE` (a bundle file) and
  `SSL_CERT_DIR` (a directory of hashed certs) also work:

  ```
  SSL_CERT_FILE=/path/to/corp-ca.pem mind evolve
  ```

  It does not read `CURL_CA_BUNDLE`, and it does not shell out to `curl` or
  `wget` as of 0.26.0. A failure here reports `invalid peer certificate` and
  points at this setting.

- **git** (every `meld` / `sync` / `upgrade` fetch):
  `git config --global http.sslCAInfo /path/to/corp-ca.pem`, or the system trust
  store.

- **`install.sh`** still uses `curl` or `wget`, since it runs before `mind`
  exists. `curl` reads `CURL_CA_BUNDLE` or `SSL_CERT_FILE`; the `wget` fallback
  honors no CA environment variable at all, so behind a custom root CA the CA
  must be in the system trust store (or configured in `wgetrc`). That same
  `wget` fallback reads only lowercase proxy environment variables
  (`https_proxy`, not `HTTPS_PROXY`); see [Proxies](#proxies) above.

## Private repos

Private sources work with no `mind`-specific configuration: it shells out to
`git`, so your credential helpers and SSH agents apply untouched.

- **SSH.** Meld the `git@host:owner/repo` form, or set `ssh = true` in
  `~/.mind/config.toml` so the `owner/repo` shorthand clones over SSH (see
  [Configuration](configuration.md#ssh-cloning)). The running `ssh-agent`
  supplies keys.
- **HTTPS.** A configured git credential helper supplies the token or password
  git would normally prompt for.
- **Any host.** GitHub Enterprise, a full clone URL, the `git@` SSH form, and a
  local path or `file://` remote all meld the same way. Source identity is
  `host/owner/repo`, so a GHE `github.example.com/...` source and a
  `github.com/...` source never collide. When GHES runs on a non-standard port,
  the identity includes the port (e.g. `github.example.com:8443/owner/repo`);
  allow patterns in a managed policy must include the port to match.

## IT-managed binaries (no self-update)

To stop end users from updating the binary, either install `mind` to a
root-owned path (so an in-place `evolve` cannot write it) or set the managed
policy `[binary].self-update` knob:

```toml
[binary]
self-update = false
```

With `self-update = false`, both `mind evolve` and `mind evolve --check` fail
fast with a policy error (`self-update is disabled by the managed policy`) before
any network call. To pin a rollout to a specific version instead of blocking
outright, set a version string:

```toml
[binary]
self-update = "0.14.0"
```

`mind evolve` then resolves to that exact version offline (no `api.github.com`
call), and `evolve --to` with any other value is refused. Absent or
`self-update = true` leaves `evolve` unrestricted. See [Managed
policy](policy.md) for where the policy file lives and how it is enforced.

**The pin is an upper bound, not a fleet version enforcement.** When IT
distributes a binary newer than the pin (e.g. the policy says `0.14.0` but the
installed binary is `0.15.0`), `evolve` does not downgrade. Instead it prints a
human-readable warning and exits 0:

```
warning: running 0.15.0 differs from the managed policy pin 0.14.0; the policy
pin is an upper bound and does not downgrade
```

To detect this skew in a script or monitoring job, run `evolve --check --json`
and inspect the `outcome` field: a value of `not-downgrading` means the running
binary is above the pin. The exit code is 0 in all cases so cron jobs are not
broken.

### Trust model for binary updates

`SHA256SUMS` is fetched over the same HTTPS channel as the release artifact
itself. The checksums verify integrity (no corruption or truncation in transit)
but not origin: a host-trusted TLS-terminating proxy can substitute both the
artifact and its matching checksums without triggering a mismatch. Releases are
not code-signed.

Origin is covered separately, and only when a `gh` binary is on `PATH`: `evolve`
verifies the downloaded archive's GitHub build-provenance attestation before
extracting it, and aborts the swap when `gh` reports the artifact does not
verify. That check is anchored in a public transparency log rather than in the
TLS channel, so an intercepting proxy cannot forge it. Without `gh` the step is
skipped silently and checksums are the only gate.

For environments where egress goes through an intercepting proxy you do not fully
trust, either require `gh` on the hosts that run `evolve`, or set
`self-update = false` and distribute IT-built binaries through a separately
audited channel.

Note that `evolve` trusts the machine's certificate store (so that it works
behind an intercepting proxy at all). On a host whose store an untrusted party
can write to, that party can terminate the TLS connection. The provenance check
above is what closes that gap; a policy pin does not.

**`GH_TOKEN`/`GITHUB_TOKEN` visibility on shared hosts.** `evolve` sends the token
as a bearer header on `api.github.com` requests (see
[Troubleshooting](troubleshooting.md) for why you'd set one). Since 0.26.0 it
builds the request in-process, so the token never reaches a subprocess command
line and is not visible to other local users through the process table. Before
0.26.0 the `wget` fallback did expose it there; that path is gone. The token is
sent only to the configured API host, never forwarded to the artifact download
across a redirect.

## Air-gapped and api-blocked installs

When only `github.com` is allowlisted and `api.github.com` is blocked, pin the
version so `install.sh` skips the API call:

```
curl --proto '=https' --tlsv1.2 -fsSL \
  https://raw.githubusercontent.com/jaemk/mind/main/resources/install.sh \
  | MIND_VERSION=0.14.0 sh
```

`install.sh` resolves the latest tag from `api.github.com` only when
`MIND_VERSION` is unset; pinning it downloads the release asset from `github.com`
directly. `mind evolve --to <v>` is the same: the version bypasses the API.

For fully air-gapped mirrors, meld from a local path or a `file://` remote
pointing at an internal clone. Source melding accepts any git remote git can
reach, including internal hosts.

**Air-gapped installs under a managed policy lock.** A local-path or `file://`
meld gets identity `local/<parent-dir>/<repo>`. Under `lock = true` with a
GHES-scoped allowlist (e.g. `github.example.com/platform/*`), every local meld
is refused because the identity does not match. There are two ways to reconcile
this:

- Add an allow pattern that covers the mirror directory:

  ```toml
  [sources]
  lock = true
  allow = ["github.example.com/platform/*", "local/mirrors/*"]
  ```

  This allows melds from `/srv/mirrors/*` (identity `local/mirrors/<repo>`) while
  still blocking other local paths. Note that admitting any local-path pattern
  delegates control to whoever can write that directory: a user who can clone any
  repo into a matching path bypasses the network restriction. Combine this with
  filesystem permissions that allow reads but not writes to the mirror directory.

- Set `allow-local = false` to block all local-path and `file://` melds under
  lock, regardless of allow patterns:

  ```toml
  [sources]
  lock = true
  allow-local = false
  allow = ["github.example.com/platform/*"]
  ```

  Use this when the lock is meant to enforce source origin and local-path melds
  should never be permitted. See [Managed policy](policy.md#local-path-control-allow-local)
  for details.

## Network timeouts

`mind evolve` reads its request budget from `MIND_HTTP_TIMEOUT_SECS` (seconds,
default 15). It is a whole-request budget, not a connect timeout, and it applies
to the release lookup; the artifact download gets 600 seconds, or the knob's
value when that is higher. Raise it for a slow proxy:

```
MIND_HTTP_TIMEOUT_SECS=60 mind evolve
```

`install.sh` uses a fixed 15-second connect timeout and does not read
`MIND_HTTP_TIMEOUT_SECS` (it runs before `mind` is on `PATH`).

For slow or blackholed `git` clones and fetches, use git's own knobs, for
example an abort when throughput stays under a floor:

```
git config --global http.lowSpeedLimit 1000
git config --global http.lowSpeedTime 30
```

## Managed-policy provisioning

A managed policy file constrains `mind` machine-wide: it restricts the client to
a trusted source allowlist, can require every source to be pinned, and can
auto-provision a base set of sources on `sync`. The file lives at a fixed system
path an administrator controls (see [Managed policy](policy.md) for the per-OS
paths and full schema). A worked example:

```toml
[sources]
# Only sources under these identities may be melded.
allow = ["github.example.com/platform/*"]
# Refuse any meld outside the allowlist (without lock, allow is advisory).
lock = true
# Every source must resolve to a tag or ref; no floating branches.
pinned = true

# Provision a baseline source automatically during `sync` and install its items.
# `install = true` runs the item install pass after provisioning (headless, --yes).
# `run-build-hooks = true` also runs item build hooks (arbitrary code; only for
# sources you control).
[[sources.auto_meld]]
repo = "https://github.example.com/platform/agent-baseline"
tag = "v1.4.0"
install = true

[binary]
# Block user-initiated binary updates.
self-update = false

[lobes]
# The effective agent home is exactly ~/.claude; config lobes edits are refused.
lock = true
targets = ["~/.claude"]
```

Auto-meld runs during `sync`, melding any listed source not already present at
its declared pin (it is idempotent). Provisioning failures are soft: an
`auto_meld` entry that fails to provision does not block the rest, already-melded
sources still sync normally, and `sync` exits non-zero after the run if any
provisioning entry failed. Validate a policy before deploying it with
`mind review --policy <path>`.

**Policy schema and deployment ordering.** A policy file must only use keys the
oldest deployed binary understands. Upgrading binaries before deploying a policy
that uses new keys prevents "unknown field" errors on old binaries (which fail
closed per POL-5). Starting with 0.15.0, a policy can declare
`min-mind-version = "0.15.0"` at the top level; an old binary that reads it
reports a clear version error instead of an opaque field error. See
[Schema evolution](policy.md#schema-evolution-and-deployment-ordering) in the
policy reference for the full constraint and the `min-mind-version` key.

**Policy file permissions.** `mind` warns to stderr when the system policy file
or its parent directory is group/world-writable or not root-owned. See
[Deploying the policy file](policy.md#deploying-the-policy-file) for the
recommended `chown`/`chmod` steps and an Ansible snippet.

## Team / CI provisioning recipe

To provision a fleet of machines from a curated source, commit a `mind.toml`
super-source to a private repo and meld it in CI. See [The mind.toml
file](mind-toml.md) for the super-source format.

1. In a private repo, commit a `mind.toml` listing the sources and items the team
   should have (a super-source with a `[discover].sources` list, or a single
   source's own inventory). `mind dump` can generate one from a reference
   machine.

2. In CI, meld it non-interactively and install its items. If this is the first
   SSH connection to the host in this environment, pre-seed `known_hosts` first;
   a headless clone fails on host-key verification otherwise:

   ```
   ssh-keyscan -H github.example.com >> ~/.ssh/known_hosts
   mind meld git@github.example.com:platform/agent-config --yes
   ```

   `--yes` installs without prompting. A non-TTY `meld` without `--yes` registers
   the source only and exits 0 without installing anything, so CI that wants items
   installed must pass `--yes`. If the sources are trusted and you
   want their install and build hooks to run unattended, add
   `--dangerously-skip-install-hook-check` and
   `--dangerously-skip-build-hook-check` (both execute arbitrary code from the
   source; only for sources you trust).

3. Verify the result as structured JSON:

   ```
   mind recall --json
   ```

   `recall --json` nests items under each source; use `jq` to find uninstalled
   items across all sources:

   ```
   mind recall --json | jq '.items[].items[] | select(.installed | not)'
   ```

   Branch on that output (or a non-zero exit) to fail the job if an expected item
   is missing.

Under a managed policy with `auto_meld` and `install = true`, a CI step is as
small as `mind sync` followed by `mind recall --json`. The `sync` provisions each
listed source and installs its items headlessly; the recall confirms the result.
See [Managed-policy provisioning](#managed-policy-provisioning) for the schema and
`install`/`run-build-hooks` field reference.
