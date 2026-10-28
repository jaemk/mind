# Share a setup with your team

Put the list of sources your team uses in one git repo. Everyone melds that
repo, gets the same sources at the same versions, and stays in step with
`mind curate` when the list changes.

```
mind meld git@github.com:acme/agent-setup --yes     # each teammate, once
mind curate                                         # after the list changes
```

## 1. Create the setup repo

Start from a machine that already has the sources you want:

```
mkdir agent-setup && cd agent-setup
mind dump --output mind.toml
git init -q && git add mind.toml && git commit -qm "team agent setup"
git remote add origin git@github.com:acme/agent-setup
git push -u origin main
```

`dump` pins each source to the commit you have now and records which items are
installed. Edit the file to suit the team. A hand-written one looks like this:

```toml
[source]
description = "Acme team agent setup"

[discover]
sources = [
  { source = "acme/agents", install = true },
  { source = "acme/skills", namespace = "acme", follow-branch = "main", install = true },
  { source = "anthropics/skills", install-items = ["skill:pdf", "skill:docx"] },
  { source = "acme/big-library", profiles = ["basic"] },
]
```

Pin with `pin-ref` for an exact commit everyone shares, or `follow-branch` to
let teammates pick up new commits with `upgrade`. See
[Curate other repos](curating.md) for every entry key.

The setup repo can also ship its own team skills and agents in `skills/` and
`agents/` beside the `mind.toml`.

## 2. Onboard a teammate

```
mind meld git@github.com:acme/agent-setup --yes
```

This registers every listed source and installs what each entry declares.
Without `--yes` they see the list and confirm. To also link into Codex or
Gemini, they add a lobe first (`mind config lobes detect`); see
[Use more than one agent](lobes.md).

## 3. Change the list

Edit `mind.toml`, commit, and push. Teammates run:

```
mind curate            # shows the plan, asks once
```

`curate` registers new entries and installs what they declare, re-pins entries
whose pin changed, upgrades curated sources, and reports entries you removed.
It uninstalls removed entries only with `--prune`. See
[Follow a curated collection](curate.md).

## In CI

The same repo provisions a CI runner or a dev container:

```
ssh-keyscan -H github.com >> ~/.ssh/known_hosts   # first SSH connection only
mind meld git@github.com:acme/agent-setup --yes
mind recall --json | jq '.items[].items[] | select(.installed | not)'
```

The last line lists items offered but not installed; fail the job if anything
you expected appears. Hooks are skipped without a terminal. If the sources are
yours and their install or build hooks must run, add
`--accept-hooks`. See [Scripts and CI](scripting.md).

## Enforcing it

A setup repo is a convenience; teammates can still meld other sources. To
restrict sources, require pins, lock agent homes, or provision a baseline on
every `mind sync` without anyone melding it, an administrator deploys a
[managed policy](policy.md):

```toml
[sources]
allow = ["github.com/acme/*", "github.com/anthropics/skills"]
lock = true

[[sources.auto_meld]]
repo = "acme/agent-setup"
tag = "v1.4.0"
install = true
```
