# Offer install profiles

A large source often has a core most people want and extras some people want.
Profiles let you name those sets, like cargo features, so a consumer picks
`full` instead of choosing items one by one.

```toml
# mind.toml
[profiles]
default = ["basic"]
basic   = ["skill:review", "skill:test", "rule:style"]
full    = ["basic", "agent:*", "skill:deploy"]
all     = ["*"]
```

With this file:

```
mind meld owner/repo                     # installs basic (the default)
mind meld owner/repo --profile full      # basic plus full
mind learn owner/repo --profile all      # later: everything
```

## Writing profiles

Each key is a profile name; each value lists its members. A member is one of:

- `kind:name`, an exact item: `skill:review`.
- `kind:glob`, every matching item: `agent:*`, `skill:pdf-*`.
- `"*"`, every item the source offers.
- the name of another profile, which includes its members: `basic`.

Rules:

- Names match `[a-z0-9][a-z0-9_-]*`, at most 64 bytes.
- Write bare item names. A namespace in effect is applied at install, so
  `skill:review` still matches when the source installs as `jk:review`.
- A member splits at its first `:`, so `command:frontend:build` is the command
  named `frontend:build`.
- A profile may not include itself, directly or through others.
- An exact member naming an item the source does not offer is an error when the
  profile is used. A glob matching nothing is not, so you can list `agent:*`
  before you ship an agent.
- Profiles are read from the repo's root `mind.toml` only.

`[profiles]` does not change discovery. It only selects among items the source
already offers, so adding it to a convention-layout repo keeps every item
available to `learn`.

## The default profile

`default` is what a plain `meld` installs, without asking which profiles to
enable.

Without a `default`, a first meld on a terminal asks which profiles to enable;
Enter offers every item. Profiles are opt-in. Choose deliberately: adding
`default` to an existing source changes what new consumers get by default.

## How changes reach consumers

Consumers who enabled a profile are tracked against it:

- An item you add to an enabled profile is offered to them on their next
  `mind upgrade`.
- An item you remove from a profile is reported, not uninstalled.
- An item a consumer forgot is not offered again while it stays in the profile.
- Renaming or removing a profile drops it from consumers' enabled sets with a
  warning.

So treat profile names as part of your public interface, as you would item
names.

## Hooks

Hooks can read the enabled profiles from `MIND_PROFILES`, and an item hook can
read the profiles its item was installed for from `MIND_ITEM_PROFILES`. See
[Profile variables](install-hooks.md#profile-variables).

## Check them

`mind review` reports invalid profile names, cycles, and exact members that
name no item as hard errors, and globs that match nothing as advisories. See
[Validate before publishing](validate.md).

Curators can select your profiles for their users with `profiles` and
`default-profile` in a `[discover].sources` entry; see
[Curate other repos](curating.md).
