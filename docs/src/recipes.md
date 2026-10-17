# Recipes

Short answers to common tasks. Each links to the page with the details.

## Try a skill from a GitHub link

```
mind learn https://github.com/owner/repo/tree/main/skills/foo
```

Installs just that skill. `mind forget skill:foo` removes it. See
[Install one item from a link](item-links.md).

## Install only some items from a large repo

```
mind meld owner/big-repo --learn 'review*' --learn agent:dev --yes
```

Or register it and pick later:

```
mind meld owner/big-repo --register-only
mind probe --source big-repo
```

See [Choose what a source installs](choosing-items.md).

## Switch a source to its "full" profile

```
mind learn owner/repo --profile full
```

And back: `mind forget owner/repo --profile full`. See
[Install profiles](choosing-items.md#install-profiles).

## Track a branch while testing a change

```
mind meld owner/repo --pin branch=my-feature
mind upgrade 'owner/repo#*'
```

Return to the default branch with `--pin branch=main`, then `upgrade`. See
[Pin a source to a version](pinning.md).

## Undo a bad upgrade

```
mind meld owner/repo --pin <previous-sha>
mind upgrade skill:review --yes
```

See [Roll back an item](pinning.md#roll-back-an-item).

## Use Claude Code and Codex side by side

```
mind config lobes add --preset codex
```

Every installed skill now links into `~/.agents` too, and new installs go to
both. See [Use more than one agent](lobes.md).

## Install skills into one project only

```
cd ~/projects/myapp
mind link-project --preset windsurf      # or: mind config lobes add ./.claude
mind learn skill:review --local
```

See [Project lobes](lobes.md#project-lobes).

## Two repos ship a `review` skill

```
mind meld other/repo --namespace other
```

Its skill installs as `other:review`. See [Fix name collisions](collisions.md).

## Keep a skill you wrote by hand under version control

```
mind absorb skill:my-skill --to ~/dev/my-agent-library
```

Moves it into that repo, commits it, and links it back as a managed item. See
[Adopt items you placed by hand](unmanaged.md).

## Set up a new machine like this one

```
mind dump --output mind.toml          # on the old machine, in a git repo you push
mind meld <that-repo> --yes           # on the new machine
```

See [Back up and reproduce your setup](dump.md).

## Onboard a teammate with one command

```
mind meld git@github.com:acme/agent-setup --yes
```

See [Share a setup with your team](team-setup.md).

## See what changed upstream without applying it

```
mind sync
mind recall            # out-of-date items are marked
```

`mind upgrade` then shows the details and asks before applying. See
[Keep everything up to date](updating.md).

## Find out where an item came from

```
mind recall skill:review
```

Shows its source, commit, profiles, and dependencies.

## Remove everything mind installed

```
mind unmeld '*' --yes
```

See [Uninstall mind entirely](removing.md#uninstall-mind-entirely).

## Publish your own skills

```
mkdir -p my-skills/skills/greet
$EDITOR my-skills/skills/greet/SKILL.md
cd my-skills && git init -q && git add -A && git commit -qm init
mind review            # check it
```

Push it, and `mind meld owner/my-skills` installs it anywhere. See
[Publish your first source](authoring.md).
