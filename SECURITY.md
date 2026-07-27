# Security policy

## Reporting a vulnerability

Report vulnerabilities privately through GitHub: on
[github.com/jaemk/mind](https://github.com/jaemk/mind), open the "Security"
tab and use "Report a vulnerability" (GitHub private vulnerability reporting).
Do not open a public issue for a security report, and do not include a working
exploit in a public issue.

Include the `mind` version (`mind --version`), OS, the source repo or spec that
triggers the issue if applicable, and reproduction steps.

## Trust model

`mind` melds arbitrary git repos, executes shell hooks a source declares, and
self-updates its own binary. Understand what each of those means before you
meld a source you do not control:

- **Melding a source is a trust decision.** A melded repo's `mind.toml` and
  item files are read during discovery, and its declared install/build hooks
  are shell commands `mind` can run. `mind` prompts for explicit consent
  before running a hook (skippable only with an explicit
  `--dangerously-skip-*` flag); there is no sandbox around a hook once
  consent is given. Only meld and consent to hooks from sources you trust,
  the same way you would before running their code directly.
- **Managed policy is the enterprise control.** An organization can lock a
  `mind` client to a trusted-source allowlist, require every source to be
  pinned, and control self-update, via a managed policy file the user cannot
  edit. See [docs/src/enterprise.md](docs/src/enterprise.md) and
  [spec/policy.md](spec/policy.md) for the full mechanism and schema.
- **No HTTP client, by design.** `mind` has no `reqwest`/`hyper`/`tokio`
  dependency. Every network touch is a `git`, `curl`, or `wget` subprocess
  with explicit, unit-tested argument vectors (e.g. `curl --proto '=https'
  --proto-redir '=https' --tlsv1.2`). This is a deliberate trade: shelling
  out to two well-audited tools with a small, reviewable argument surface
  instead of pulling in roughly 80 transitive crates of an HTTP stack, for a
  tool whose entire risk surface is supply chain.
- **Discovery metadata reads are size-capped.** `mind.toml`, item frontmatter
  (`SKILL.md`/`TOOL.md`, agent and rule `.md` files), and Claude plugin and
  marketplace manifests are capped at 8 MiB. An oversized file is refused,
  naming the file and the limit, and is never fully read into memory: the
  reader takes at most cap-plus-one bytes, so the cost of an oversized file is
  bounded by the cap rather than by the file itself. Recorded as `DSC-91` in
  [spec/discovery.md](spec/discovery.md).
- **Item content reads are not size-capped.** Beyond the metadata cap above, no
  other read of source-controlled content has a size or nesting-depth limit:
  every file in an item tree during `{{ns:}}` expansion at install, the
  reference scan, `review`, the TUI preview, and content hashing. A source
  could ship an oversized or deeply nested file to make `mind` allocate
  heavily while scanning or installing it. This is self-inflicted (you chose
  to meld that source) and bounded to the current invocation; the reasoning is
  recorded as `DSC-90` in
  [spec/discovery.md](spec/discovery.md#accepted-risks). Reports on this are
  welcome but will likely be closed as accepted risk.

## Supported versions

Only the latest released version is supported with security fixes.
