# Hello example

The hello-world target for `mind meld jaemk/mind`. The `mind` repo's own
root `mind.toml` sets `roots = ["examples/hello"]` (DSC-50), so melding the
repo scans this directory by convention (DSC-1) and offers the single
`hello-mind` skill. This is what the docs landing page runs.

## Layout

```
skills/hello-mind/SKILL.md    skill, description in frontmatter
```

There is no `mind.toml` in this directory: it is a plain convention source. The
repo-root `mind.toml` points convention discovery here via `roots`.

## Try it

Melding the real repo registers it and, on the default-yes prompt, installs its
items (just `hello-mind`):

```
mind meld jaemk/mind         # prompts [Y/n] to install hello-mind; Enter accepts
mind recall                  # shows hello-mind as installed
```

Then invoke it in a Claude session:

```
/hello-mind
```

`--link-only` registers without installing, and `mind learn hello-mind` then
installs it explicitly (also the path a non-TTY `meld` prints instead of
prompting).

This directory is also a standalone convention source, so you can meld it on its
own. It lives inside the mind repo, so copy it out and init a repo first:

```
cp -r examples/hello /tmp/hello
cd /tmp/hello && git init -q && git add -A && git commit -qm init

mind meld /tmp/hello         # installs hello-mind on the default-yes prompt
```

### Teardown

```
mind forget hello-mind
mind unmeld mind             # or local/tmp/hello for the standalone copy
```

## See also

`../../spec/discovery.md` - feature IDs demonstrated here: DSC-1 (zero-config
convention discovery) and DSC-50 (`[source].roots` convention scan roots).

## Verified

`tests/cli.rs::root_mindfile_exposes_hello` melds a repo carrying the real
root `mind.toml` and this directory, then asserts `hello-mind` is discovered and
links into the agent home, so the landing-page command stays correct.
