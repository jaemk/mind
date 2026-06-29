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
