# Restricted networks and enterprise

`mind` has no built-in HTTP client. Every network touch is a `git`, `curl`, or
`wget` subprocess spawned with the parent environment inherited, so the same
proxy, CA, and credential configuration you already use for those tools applies
unchanged. `mind` sets or strips nothing, and there is no telemetry or
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
    `--version`, see below).
  - `https://github.com/jaemk/mind/releases/download/...` for the release asset
    and its `SHA256SUMS`. These redirect to
    `https://objects.githubusercontent.com/...`, so allowlist that host too.
- **`install.sh` (install script).** The same two `github.com` /
  `objects.githubusercontent.com` download URLs, plus
  `https://raw.githubusercontent.com/jaemk/mind/main/resources/install.sh` for
  the script itself, and `https://api.github.com/...` unless you pin
  `MIND_VERSION`.

## Proxies

The `git`, `curl`, and `wget` subprocesses honor the standard proxy environment
variables of the parent process: `HTTP_PROXY`, `HTTPS_PROXY`, and `NO_PROXY`
(and their lowercase forms). `git` additionally honors its own `http.proxy`
config:

```
git config --global http.proxy http://proxy.corp.example:8080
```

`mind` neither sets nor unsets any of these, so whatever works for a bare
`git clone` or `curl` works for `mind`.

## Custom CA / TLS-intercepting proxy

Behind a proxy that re-signs TLS with a corporate root CA, point each tool at the
CA bundle the way you already do:

- **git:** `git config --global http.sslCAInfo /path/to/corp-ca.pem`, or install
  the CA into the system trust store.
- **curl** (used by `evolve` and `install.sh`): `CURL_CA_BUNDLE` or
  `SSL_CERT_FILE` in the environment.

The `wget` fallback path (used by `install.sh` and `evolve` only when `curl` is
absent) honors no CA environment variable. Behind a custom root CA, the CA must
be in the system trust store (or configured in `wgetrc`); an env var alone will
not help. Prefer `curl`, which reads `CURL_CA_BUNDLE` / `SSL_CERT_FILE`.

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
  `github.com/...` source never collide.

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
call), and `evolve --version` with any other value is refused. Absent or
`self-update = true` leaves `evolve` unrestricted. See [Managed
policy](policy.md) for where the policy file lives and how it is enforced.

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
directly. `mind evolve --version <v>` is the same: the version bypasses the API.

For fully air-gapped mirrors, meld from a local path or a `file://` remote
pointing at an internal clone. Source melding accepts any git remote git can
reach, including internal hosts.

## Network timeouts

`mind evolve` reads the connect timeout from `MIND_HTTP_TIMEOUT_SECS` (seconds,
default 15). Raise it for a slow proxy:

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

# Provision a baseline source automatically during `sync`.
[[sources.auto_meld]]
repo = "https://github.example.com/platform/agent-baseline"
tag = "v1.4.0"

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

## Team / CI provisioning recipe

To provision a fleet of machines from a curated source, commit a `mind.toml`
super-source to a private repo and meld it in CI. See [The mind.toml
file](mind-toml.md) for the super-source format.

1. In a private repo, commit a `mind.toml` listing the sources and items the team
   should have (a super-source with a `[discover].sources` list, or a single
   source's own inventory). `mind dump` can generate one from a reference
   machine.

2. In CI, meld it non-interactively and install its items:

   ```
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

   Branch on the `items` array (or a non-zero exit) to fail the job if an
   expected item is missing.

Under a managed policy with `auto_meld`, `sync` provisions the baseline sources
itself, so a CI step can be as small as `mind sync` followed by `mind recall
--json`. See [Managed-policy provisioning](#managed-policy-provisioning) above.
