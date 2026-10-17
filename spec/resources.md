# Shared resources

Status: done. A source often ships files its items read but never run: a JSON
allowlist, a prompt template, a reference document, a directory of examples.
This document specifies the `resource` item kind, which installs such a file or
directory to the store, and the `{{resources:name}}` token an item uses to name
its installed path.

## Overview

A `tool` (tooling.md) covers helpers that run. A resource is the same idea for
data: a store-only item reached by a path token, never linked into an agent
home. It differs from a tool in three ways:

- One entry is one item, file or directory. `resources/allowlist.json` is the
  resource `allowlist.json`; `resources/templates/` is the resource `templates`.
  A tool is always a directory.
- A resource has no metadata file and no entrypoint. Nothing about it is read
  from frontmatter.
- Convention discovery offers a resource only when another item in the source
  references it. `resources/` is a common directory name in arbitrary repos (a
  project's install script, images for its README), and an entry nothing
  references has no reader once installed.

Path tokens already expand to store paths (tooling.md TOOL-10..20). This
document adds one token family and reuses the rest of that machinery.

## The `resource` kind

- `RES-1` `resource` is an item kind. By convention, every immediate entry of a
  `resources/` directory under each scan root is a resource: a regular file or a
  directory, classified without following symlinks (DSC-108). The whole entry is
  the item. An entry whose name begins with `.` is not a resource. A symlink
  entry is not a resource.
- `RES-2` A resource's bare name is the entry's full file name, extension
  included: `resources/allowlist.json` is `resource:allowlist.json`, and
  `resources/templates/` is `resource:templates`. The name is validated like any
  convention-derived name (DSC-96): an unsafe name is skipped with a warning.
- `RES-3` Under convention discovery (no authoritative `mind.toml`), a resource
  is offered only when it is referenced. After the scan, a convention resource
  stays in the source's catalog when an item of the same source that is not a
  convention resource depends on it (DEP-1, DEP-4, DEP-8), or when a convention
  resource already kept does. A convention resource no such item references is
  dropped from the catalog without a warning. This applies to the resources an
  `--add-root` root contributes (DSC-84) as well, since those are convention
  resources too.
- `RES-4` A resource declared by an authoritative `mind.toml`, through an
  `[[items]]` entry with `kind = "resource"` or a `[discover].resources` glob, is
  always offered, referenced or not. A `[discover].resources` glob matches the
  resource entry itself (a file or a directory), and the matched entry's full
  file name is its bare name (RES-2).
- `RES-5` A resource has no anchor file and no frontmatter: `mind` does not read
  `description`, `requires`, `expand`, or the lifecycle hook scalars from a
  resource's content, whatever its extension. Its description comes only from an
  `[[items]].description` override, and its hooks only from its `[[items]]`
  entry (HOOK-132). The `bin` and `build` item fields are a `mind.toml` schema
  error on a resource (TOOL-7), and so is `link` (RES-6).
- `RES-6` A resource installs to the store at
  `~/.mind/store/resource/<effective_name>` (a file for a file resource, a
  directory for a directory resource) and is never linked into an agent home: its
  manifest entry records the store path with an empty `links` set, as a tool's
  does (TOOL-3). Unlike a tool (TOOL-4) it cannot declare a `link`. `forget`
  removes the store copy, `recall` reports it installed, and `upgrade` and
  `introspect` track its content hash like any item.
- `RES-7` Namespacing applies as to any kind (namespacing.md): a prefix gives
  the effective name `<prefix>:<name>` (`k:allowlist.json`), and the stable
  identity is `(source, kind, bare_name)`. `resource` is a reserved prefix word
  (NS-25), so it reads as a kind qualifier in an item ref (`resource:x`).
- `RES-8` A resource is never an unmanaged lobe item (unmanaged.md), since
  nothing places it in an agent home, and a plugin manifest has no resource
  component (MKT-3): a plugin source offers resources only through `--add-root`.

## The `{{resources:name}}` token

- `RES-10` `{{resources:name}}` in an item's text expands to the store path of
  the sibling resource `name` (RES-6): the file itself for a file resource, the
  directory for a directory resource, so `{{resources:templates}}/pr.md` names a
  file inside one. `{{path:resource:name}}` (TOOL-11) expands to the same path.
- `RES-11` A `name` that matches no sibling resource is a `BadReference` with
  the miss wording (TOOL-17), at install and as a `review` `bad-reference`
  finding (CLI-135).
- `RES-12` `{{resources:name}}` follows every rule the other path tokens follow:
  the expansion gate (TOOL-14, TOOL-19), the `expand:` override with an
  absolute rendering (TOOL-20), the `~` rendering of the store root (TOOL-16),
  expansion in staging with the token form hashed (TOOL-13), and sibling scope
  only (TOOL-15).
- `RES-13` `review` and `init-source` map a hardcoded store path to a resource
  (`~/.mind/store/resource/<name>/...`) to `{{resources:name}}` plus the rest of
  the path, the way they map a tool's entrypoint to `{{tools:name}}` (CLI-136,
  CLI-138).

## Authoring feedback

- `RES-14` `review` reports each convention resource that RES-3 drops as an
  advisory `unreferenced-resource` finding naming the entry, so an author whose
  prose mentions `resources/<name>` without a token learns why the file is not
  offered and which token to write.
