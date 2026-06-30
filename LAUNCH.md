# Launch and adoption plan

Working doc for taking `mind` public. Adoption model: **both, ecosystem-led** -
pitch a curated public library as the hook, personal/team sync as the deeper use.

Check items off as they land. Re-read the "Strategic frame" before reprioritizing.

## Strategic frame

`mind` is a package manager, and a package manager is only as valuable as its
packages. The headline risk at launch: with only one author's sources, it reads
as "dotfiles with extra steps." The launch must show the *seed of plurality* - a
flagship source worth melding plus a frictionless authoring path - so an
ecosystem is believable.

A first-time visitor's question is "meld what?". Their first command must produce
working tooling, not an empty `recall`. That is the bar.

The differentiator vs. "just copy files": drift detection, install/upgrade/
uninstall, cross-harness linking. Lead with that, not with "it manages files."

## Versus the built-in marketplace

Claude Code ships a first-party plugin marketplace. It will own default
distribution, so do not pitch `mind` as another store. The framing that holds:
the marketplace is a store, `mind` is a package manager (Homebrew vs the App
Store). They coexist.

Confirmed structural gaps in the first-party system (current docs, 2026-06-30):

- No zero-config discovery. An author must add `.claude-plugin/plugin.json` and
  list the plugin in a `marketplace.json`. `mind` melds an arbitrary unpackaged
  repo. This is the wedge: addressable supply is every repo with a `SKILL.md`, not
  every repo whose owner published a marketplace.
- Whole-plugin install only. The marketplace install unit is the entire plugin
  bundle; a user cannot cherry-pick one skill. `mind` installs a single item.
- Claude-only. Plugins target Claude Code; `mind` links into Gemini/Codex/
  Antigravity lobes too.
- Consumer-side, optional namespacing vs the native mandatory `plugin:skill`.

Where the marketplace is at parity or ahead (do not claim these as gaps): it has
semver versioning, git-tag release channels, inter-plugin dependency resolution
with version constraints, auto-update, and a renames migration map. So demote the
"lifecycle ops" angle to what `mind` actually wins on: drift reporting
(`introspect`/`upgrade` source-hash + commit deltas, which the marketplace lacks),
transactional rollback, `absorb`, and `dump`.

Cross-harness is contested, not owned. Community generators (ECC, wshobson/agents)
already emit multi-harness artifacts from one source. Differentiate against them on
meld-arbitrary-repos (they still require their own packaged source), not on "we
support many harnesses" alone.

Interop, do not compete. A marketplace repo is a git repo with a JSON manifest, so
`mind` can consume it as one source type. That turns the marketplace's growth into
`mind`'s supply. Spec: [spec/marketplace.md](spec/marketplace.md) (MKT-1..11,
status planned).

Install-model decision (settled): keep `mind`'s own store + symlink model; treat a
marketplace as a source (input), never a sink. The native plugin system is itself
an own-store copy cache (`~/.claude/plugins/cache/...`), not canonical
`~/.claude/skills` files, so conforming to it buys neither a shared format nor
cleaner uninstall while losing namespacing, `{{ns:}}` expansion, the `rule`/`tool`
kinds, and the source-hash drift model. This is also the dominant pattern among
real package managers (Homebrew kegs, Nix store, pipx venvs, Stow): own store, link
into the host's discovery path. Native-format output, if ever wanted, is a
`dump`-style export, kept separate from the install path.

## Critical path (gates the launch)

The flagship is the curated `jaemk/mind` super-source itself, not an own-authored
skills repo. `~/dev/agents` is no longer in the plan. The pitch is "meld this one
registry, get a vetted set of skills" - curation as the product, drift/upgrade as
the differentiator.

- [x] **Curated super-source is the flagship.** The `mind` repo's `mind.toml`
      curates `anthropics/skills` and `ComposioHQ/awesome-claude-skills` as
      register-only `[discover].sources`, so `mind meld jaemk/mind` + `mind probe`
      surfaces real third-party skills on the first command (not just
      `hello-mind`). This is the cold-start answer and the thing to polish.
- [ ] **Productize the curated registry.** Make `mind meld jaemk/mind` a
      compelling first run end to end.
  - [x] Both curated sources surface real items via `mind probe`:
        `anthropics/skills` (17) and `ComposioHQ/awesome-claude-skills` (28, a
        flat-layout repo wired with `flat-skills = true` on its entry). 46 skills
        total including `hello-mind`.
  - [ ] Curate a few more high-signal sources so probe shows breadth, and decide
        per entry whether it stays register-only or is `install = true`.
  - [ ] Harden the flagship meld: `mind meld jaemk/mind` now hard-fails if any
        curated remote is unreachable (network failures are not auth failures, so
        `on-auth-failure` can't soften them). Decide whether the headline command
        should degrade gracefully or whether the registry lives in a separate file
        from the `hello-mind` landing source.
- [ ] **Authoring path is visibly easy.** A prominent "publish your own source in
      2 minutes" tutorial built on `init-source` + `review`. Ecosystem growth =
      third parties authoring sources, so this is the flywheel, not an afterthought.
      (Flat skill layout shipped this cycle: `[source].flat-skills` / `meld
      --flat-skills` lets a bare skill-dirs repo meld with no `skills/` container,
      one less authoring hurdle.)

## Showcase assets

- [ ] **Demo recording** (asciinema or GIF), 20-30s: `mind meld jaemk/mind ->
      probe -> learn -> recall` against the curated registry so viewers see real
      third-party skills (not `hello-mind`). Embed at the top of the README.
      Highest-leverage single asset for a CLI tool. (Capture needs a real terminal
      - James runs it.)
- [ ] **README rework**: problem-first hero (skills/agents are copy-pasted between
      repos and machines, drift silently, no install/upgrade/uninstall story) ->
      demo -> one-line `mind meld jaemk/mind` -> "author your own in 2 min."
      Today's README opens with *what* it is; open with the *pain*.

## Distribution (launch day, same day, be present to answer)

- [ ] **Show HN** - title leads with the pain + Homebrew analogy, e.g. "Show HN:
      mind - Homebrew for Claude/Gemini agent skills". Post with the demo GIF and
      the flagship to meld.
- [ ] **r/ClaudeAI, r/LocalLLaMA, Anthropic Discord** - exact-fit users; the
      cross-harness, vendor-neutral angle plays well here.
- [ ] **Launch blog post / X thread** - walk the cold-start problem -> meld ->
      drift detection -> upgrade. The `upgrade`/`introspect` drift story is the
      differentiator; lead with it.
- [ ] **awesome-claude-code / awesome-* list PRs** - low effort, durable discovery.

## Sequencing

1. Curated-registry productization: vet/expand the curated sources (drop or fix
   the 0-item entry), set per-entry install defaults, harden the flagship meld
   against an unreachable remote. Critical path, gates everything.
2. Demo recording against `jaemk/mind`.
3. README rework.
4. Launch: Show HN + Reddit/Discord + awesome-list PRs, same day.

## Readiness notes (already in good shape)

0.8.0, 227 commits, ~650 test fns, normative spec with an enforced coverage gate,
deployed mdbook docs site + landing page, curl|sh + Homebrew install, MIT license,
tag-driven release pipeline, cross-harness lobes. Engineering polish is not the
bottleneck - ecosystem cold-start is.
