//! Install profiles (spec/profiles.md): named sets of a source's items,
//! declared in the root `mind.toml` `[profiles]` table and selected at `meld` /
//! `learn` with `--profile`, `--no-default-profile`, and `--all-profiles`.
//!
//! This module is the pure core. It parses and validates the table (PROF-2..4),
//! turns a consumer selection into the enabled set (PROF-7, PROF-9..11,
//! PROF-26), expands the enabled set against a catalog (PROF-1, PROF-3, PROF-5),
//! and computes the bookkeeping the verbs record and act on: the install reasons
//! on a manifest entry (PROF-20), the source's enabled set and snapshot
//! (PROF-19), the snapshot diff `upgrade` reports (PROF-21..23), and what
//! disabling a profile removes (PROF-25). It does no I/O.
//!
//! Keys throughout are bare `kind:name` strings in source truth (the namespace
//! is an install-time transform, PROF-3), the same shape a `[discover].sources`
//! `install-items` entry uses (DSC-63).

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use crate::catalog::CatalogItem;
use crate::error::{BadRefReason, ItemKind, MindError, Result};
use crate::manifest::InstalledItem;
use crate::sanitize::strip_ansi;
use crate::source::Source;

/// The profile enabled when the consumer names none and does not opt out
/// (PROF-7).
pub const DEFAULT: &str = "default";

/// A `[profiles]` table: profile name to its members as written (PROF-1).
pub type ProfileTable = BTreeMap<String, Vec<String>>;

/// The longest profile name accepted, in bytes (PROF-2).
const MAX_NAME_LEN: usize = 64;

/// Why `name` is not a valid profile name (PROF-2), as a clause that reads
/// after the name ("'X' is empty"), or `None` when it is valid.
pub fn name_problem(name: &str) -> Option<&'static str> {
    let bytes = name.as_bytes();
    let Some(&first) = bytes.first() else {
        return Some("is empty");
    };
    if bytes.len() > MAX_NAME_LEN {
        return Some("is longer than 64 bytes");
    }
    if name.contains(':') {
        return Some("contains ':', which marks an item pattern ('kind:name'), not a profile");
    }
    if !(first.is_ascii_lowercase() || first.is_ascii_digit()) {
        return Some("must start with a lowercase letter or digit");
    }
    let ok = |b: &u8| b.is_ascii_lowercase() || b.is_ascii_digit() || *b == b'_' || *b == b'-';
    if !bytes.iter().all(ok) {
        return Some("may contain only lowercase letters, digits, '_', and '-'");
    }
    None
}

/// Whether `name` is a valid profile name: `[a-z0-9][a-z0-9_-]*`, at most 64
/// bytes (PROF-2). Production callers use [`name_problem`] for the reason.
#[cfg(test)]
pub fn is_valid_name(name: &str) -> bool {
    name_problem(name).is_none()
}

/// One parsed profile member (PROF-3).
#[derive(Debug, Clone)]
pub enum Member {
    /// `"*"`: every item the source offers.
    All,
    /// `kind:name`, where `name` is a bare name or, when `glob` is set, a glob
    /// over bare names.
    Item {
        kind: ItemKind,
        name: String,
        glob: Option<glob::Pattern>,
    },
    /// A profile name (no `:`), including that profile's members.
    Profile(String),
}

impl Member {
    /// Whether this member selects `item` (an item or `*` member). A profile
    /// member selects nothing directly.
    fn matches(&self, item: &CatalogItem) -> bool {
        match self {
            Member::All => true,
            Member::Item { kind, name, glob } => {
                item.kind == *kind
                    && match glob {
                        Some(p) => p.matches(&item.name),
                        None => item.name == *name,
                    }
            }
            Member::Profile(_) => false,
        }
    }
}

/// Parse one member string (PROF-3). The error is a short clause naming the
/// defect; [`validate_table`] wraps it with the profile and member.
///
/// The kind is split at the FIRST `:`, so an item name that itself carries a
/// `:` (a command group, CMD-2) parses as `command:frontend:build`.
pub fn parse_member(raw: &str) -> std::result::Result<Member, String> {
    if raw == "*" {
        return Ok(Member::All);
    }
    if let Some((kind_word, name)) = raw.split_once(':') {
        let kind = ItemKind::parse(kind_word).ok_or_else(|| {
            format!(
                "has unknown kind '{}' (expected one of: {})",
                strip_ansi(kind_word),
                ItemKind::word_list()
            )
        })?;
        if name.is_empty() {
            return Err("has an empty item name".to_string());
        }
        let glob = if crate::resolve::is_glob(name) {
            Some(glob::Pattern::new(name).map_err(|e| format!("is not a valid glob ({e})"))?)
        } else {
            None
        };
        return Ok(Member::Item {
            kind,
            name: name.to_string(),
            glob,
        });
    }
    if let Some(reason) = name_problem(raw) {
        return Err(format!(
            "is neither 'kind:name', '*', nor a profile name: it {reason}"
        ));
    }
    Ok(Member::Profile(raw.to_string()))
}

/// Validate a `[profiles]` table at `mind.toml` load (PROF-2, PROF-3, PROF-4):
/// every key is a valid name, every member parses, every profile member is a
/// key of the table, and inclusion is acyclic. Every error is a `MindToml`
/// whose message starts with `[profiles]: `. Whether an exact item member
/// names a real item needs the catalog and is [`expand`]'s check (PROF-5).
pub fn validate_table(table: &ProfileTable, toml_path: &Path) -> Result<()> {
    let err = |msg: String| MindError::MindToml {
        path: toml_path.to_path_buf(),
        msg: format!("[profiles]: {msg}"),
    };
    for (name, members) in table {
        if let Some(reason) = name_problem(name) {
            return Err(err(format!("profile name '{}' {reason}", strip_ansi(name))));
        }
        for raw in members {
            match parse_member(raw) {
                Ok(Member::Profile(inc)) if !table.contains_key(&inc) => {
                    return Err(err(format!(
                        "profile '{name}': member '{inc}' names a profile the table does not declare"
                    )));
                }
                Ok(_) => {}
                Err(reason) => {
                    return Err(err(format!(
                        "profile '{name}': member '{}' {reason}",
                        strip_ansi(raw)
                    )));
                }
            }
        }
    }
    if let Some(cycle) = find_cycle(table) {
        return Err(err(format!(
            "profile inclusion cycle: {}",
            cycle.join(" -> ")
        )));
    }
    Ok(())
}

/// The profiles `name` includes directly, in declaration order. Unparseable
/// members are skipped (a validated table has none).
fn includes<'a>(table: &'a ProfileTable, name: &str) -> impl Iterator<Item = String> + 'a {
    table
        .get(name)
        .into_iter()
        .flatten()
        .filter_map(|raw| match parse_member(raw) {
            Ok(Member::Profile(p)) => Some(p),
            _ => None,
        })
}

/// The first inclusion cycle in `table`, as the path that closes it
/// (`["full", "basic", "full"]`), or `None` (PROF-4). Depth-first, keys in
/// table order, so the reported cycle is deterministic.
fn find_cycle(table: &ProfileTable) -> Option<Vec<String>> {
    fn dfs(
        table: &ProfileTable,
        node: &str,
        stack: &mut Vec<String>,
        done: &mut BTreeSet<String>,
    ) -> Option<Vec<String>> {
        if let Some(at) = stack.iter().position(|s| s == node) {
            let mut cycle = stack[at..].to_vec();
            cycle.push(node.to_string());
            return Some(cycle);
        }
        if done.contains(node) || !table.contains_key(node) {
            return None;
        }
        stack.push(node.to_string());
        for next in includes(table, node) {
            if let Some(c) = dfs(table, &next, stack, done) {
                return Some(c);
            }
        }
        stack.pop();
        done.insert(node.to_string());
        None
    }
    let mut done = BTreeSet::new();
    for name in table.keys() {
        let mut stack = Vec::new();
        if let Some(c) = dfs(table, name, &mut stack, &mut done) {
            return Some(c);
        }
    }
    None
}

/// Every profile reachable from `name` by inclusion, `name` itself included.
/// Guarded by the visited set, so it terminates on an unvalidated table too.
fn closure(table: &ProfileTable, name: &str) -> BTreeSet<String> {
    let mut seen = BTreeSet::new();
    let mut todo = vec![name.to_string()];
    while let Some(p) = todo.pop() {
        if !table.contains_key(&p) || !seen.insert(p.clone()) {
            continue;
        }
        todo.extend(includes(table, &p));
    }
    seen
}

/// A consumer's profile selection, from the flags (PROF-9..11) or a curator
/// entry (PROF-26).
#[derive(Debug, Clone, Default)]
pub struct Selection {
    /// Enable `default` when the source declares it (false under
    /// `--no-default-profile` / `default-profile = false`, PROF-10).
    pub include_default: bool,
    /// Profiles named with `--profile` / `profiles = [...]` (PROF-9).
    pub named: Vec<String>,
    /// `--all-profiles` (PROF-11).
    pub all: bool,
}

impl Selection {
    /// The selection a curator's `[discover].sources` entry directs (PROF-26):
    /// its `profiles` as `--profile`, and `default-profile = false` as
    /// `--no-default-profile`. Names were syntax-checked by
    /// [`crate::mindfile::NestedSource::validate`].
    pub fn from_nested(entry: &crate::mindfile::NestedSource) -> Selection {
        Selection {
            include_default: entry.default_profile.unwrap_or(true),
            named: entry.profiles.clone().unwrap_or_default(),
            all: false,
        }
    }
}

/// The profile flags set on a `meld`/`learn`, spelled as typed, in a fixed
/// order (PROF-8 names the first in its usage error).
pub fn flags_given(names: &[String], no_default: bool, all: bool) -> Vec<&'static str> {
    let mut flags = Vec::new();
    if !names.is_empty() {
        flags.push("--profile");
    }
    if no_default {
        flags.push("--no-default-profile");
    }
    if all {
        flags.push("--all-profiles");
    }
    flags
}

/// Parse `--profile` values (PROF-9, PROF-14): each value may be a
/// comma-separated list; names are trimmed and deduped in first-seen order.
/// A malformed name (including an empty segment) is `InvalidProfileName`, a
/// pure syntax check that runs before any clone.
pub fn parse_cli_names(values: &[String]) -> Result<Vec<String>> {
    let mut out: Vec<String> = Vec::new();
    for value in values {
        for part in value.split(',') {
            let name = part.trim();
            if let Some(reason) = name_problem(name) {
                return Err(MindError::InvalidProfileName {
                    name: name.to_string(),
                    reason: format!("it {reason}"),
                });
            }
            if !out.iter().any(|n| n == name) {
                out.push(name.to_string());
            }
        }
    }
    Ok(out)
}

/// The enabled set for `selection` against the source's table, sorted and
/// deduped (PROF-7, PROF-9..11). `default` is enabled only when the table
/// declares it and `include_default` holds; `all` enables every declared
/// profile. A named profile the table does not declare is `ProfileNotFound`
/// listing the declared ones (PROF-14).
pub fn enabled_set(
    table: Option<&ProfileTable>,
    selection: &Selection,
    source_name: &str,
) -> Result<Vec<String>> {
    enabled_with(table, selection, |name, declared| {
        MindError::ProfileNotFound {
            profile: name.to_string(),
            source_name: source_name.to_string(),
            declared,
        }
    })
}

/// [`enabled_set`] for a curator's `[discover].sources` entry (PROF-26): an
/// undeclared profile name is `BadReference` naming the curator, the profile,
/// and the nested source, rather than the consumer-facing `ProfileNotFound`.
pub fn enabled_set_for_curator(
    table: Option<&ProfileTable>,
    selection: &Selection,
    source_name: &str,
    curator: &str,
) -> Result<Vec<String>> {
    enabled_with(table, selection, |name, _| MindError::BadReference {
        item: format!("curator '{}' [discover].sources entry", strip_ansi(curator)),
        referent: format!("profile '{}'", strip_ansi(name)),
        reason: BadRefReason::NoMatch,
        in_source: strip_ansi(source_name),
    })
}

fn enabled_with(
    table: Option<&ProfileTable>,
    selection: &Selection,
    undeclared: impl Fn(&str, Vec<String>) -> MindError,
) -> Result<Vec<String>> {
    let empty = ProfileTable::new();
    let table = table.unwrap_or(&empty);
    let mut out: BTreeSet<String> = BTreeSet::new();
    if selection.include_default && table.contains_key(DEFAULT) {
        out.insert(DEFAULT.to_string());
    }
    if selection.all {
        out.extend(table.keys().cloned());
    }
    for name in &selection.named {
        if !table.contains_key(name) {
            return Err(undeclared(name, table.keys().cloned().collect()));
        }
        out.insert(name.clone());
    }
    Ok(out.into_iter().collect())
}

/// The bare `kind:name` key of a catalog item.
fn bare_key(item: &CatalogItem) -> String {
    format!("{}:{}", item.kind.as_str(), item.name)
}

/// The expansion of an enabled set (PROF-5): each bare `kind:name` key it
/// reaches, mapped to the ENABLED profiles it came through. A key reached via
/// an included profile is attributed to the enabled profile that included it
/// (enabling `full`, which includes `basic`, tags `basic`'s items `full`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Expansion {
    pub keys: BTreeMap<String, BTreeSet<String>>,
}

impl Expansion {
    /// The expanded keys, sorted.
    pub fn key_list(&self) -> Vec<String> {
        self.keys.keys().cloned().collect()
    }
}

/// Expand `enabled` against `items` (PROF-1, PROF-3, PROF-5). Only items whose
/// `source` is `source_name` are considered, matched by BARE name, so a
/// profile never reaches past the source's own offer. An exact member that
/// matches nothing is `BadReference` naming the profile that lists it; a glob
/// that matches nothing is fine. An enabled name the table does not declare
/// contributes nothing (reporting it is the caller's, PROF-24).
pub fn expand(
    table: &ProfileTable,
    enabled: &[String],
    items: &[CatalogItem],
    source_name: &str,
) -> Result<Expansion> {
    let own: Vec<&CatalogItem> = items.iter().filter(|i| i.source == source_name).collect();
    let mut out = Expansion::default();
    for root in enabled {
        for profile in closure(table, root) {
            for raw in &table[&profile] {
                let Ok(member) = parse_member(raw) else {
                    continue;
                };
                if matches!(member, Member::Profile(_)) {
                    continue;
                }
                let mut hit = false;
                for item in own.iter().filter(|i| member.matches(i)) {
                    hit = true;
                    out.keys
                        .entry(bare_key(item))
                        .or_default()
                        .insert(root.clone());
                }
                if !hit && matches!(member, Member::Item { glob: None, .. }) {
                    // spec: PROF-5
                    return Err(MindError::BadReference {
                        item: format!("profile '{profile}'"),
                        referent: strip_ansi(raw),
                        reason: BadRefReason::NoMatch,
                        in_source: strip_ansi(source_name),
                    });
                }
            }
        }
    }
    Ok(out)
}

/// Problems in a profile table against a catalog, collected rather than
/// failing on the first (PROF-30, for `review`). Pairs are `(profile, member
/// as written)`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProfileLint {
    /// Exact item members naming an item the source does not offer (hard).
    pub missing_exact: Vec<(String, String)>,
    /// Glob members (and `*`) matching nothing (advisory,
    /// `empty-profile-member`).
    pub empty_globs: Vec<(String, String)>,
    /// Profiles whose whole expansion is empty (advisory, `empty-profile`).
    pub empty_profiles: Vec<String>,
}

/// Lint `table` against the source's items (PROF-30). Tolerates an
/// unvalidated table: unparseable members are skipped (the PROF-2..4 findings
/// come from [`validate_table`]).
pub fn lint(table: &ProfileTable, items: &[CatalogItem], source_name: &str) -> ProfileLint {
    let own: Vec<&CatalogItem> = items.iter().filter(|i| i.source == source_name).collect();
    let mut out = ProfileLint::default();
    // Per profile, whether its own (non-inclusion) members matched anything.
    let mut direct_hit: BTreeMap<&str, bool> = BTreeMap::new();
    for (profile, members) in table {
        let mut any = false;
        for raw in members {
            let Ok(member) = parse_member(raw) else {
                continue;
            };
            if matches!(member, Member::Profile(_)) {
                continue;
            }
            let hit = own.iter().any(|i| member.matches(i));
            any |= hit;
            if !hit {
                let pair = (profile.clone(), raw.clone());
                match member {
                    Member::Item { glob: None, .. } => out.missing_exact.push(pair),
                    _ => out.empty_globs.push(pair),
                }
            }
        }
        direct_hit.insert(profile, any);
    }
    for profile in table.keys() {
        let reaches = closure(table, profile)
            .iter()
            .any(|p| direct_hit.get(p.as_str()).copied().unwrap_or(false));
        if !reaches {
            out.empty_profiles.push(profile.clone());
        }
    }
    out
}

/// How a fresh expansion differs from a source's snapshot (PROF-21, PROF-22).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SnapshotDiff {
    /// Keys in the expansion and not in the snapshot, with the profiles each
    /// came through: pending additions (PROF-21).
    pub additions: Vec<(String, BTreeSet<String>)>,
    /// Keys in the snapshot that left the expansion (PROF-22).
    pub departures: Vec<String>,
}

/// Diff `snapshot` against `expansion` (PROF-21..23). A key the consumer
/// forgot is still in the snapshot, so it is never an addition while it
/// remains a member (PROF-23).
pub fn diff(snapshot: &[String], expansion: &Expansion) -> SnapshotDiff {
    let snap: BTreeSet<&str> = snapshot.iter().map(String::as_str).collect();
    let additions = expansion
        .keys
        .iter()
        .filter(|(k, _)| !snap.contains(k.as_str()))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    let departures = snap
        .into_iter()
        .filter(|k| !expansion.keys.contains_key(*k))
        .map(str::to_string)
        .collect();
    SnapshotDiff {
        additions,
        departures,
    }
}

/// Carry the install reasons of `previous` onto `fresh` (PROF-20): the
/// install path builds a new manifest entry on every (re)install, so a caller
/// replacing an existing entry calls this before [`tag_reasons`] to keep what
/// the old entry recorded.
pub fn carry_reasons(fresh: &mut InstalledItem, previous: &InstalledItem) {
    fresh.profiles = previous.profiles.clone();
    fresh.explicit = previous.explicit;
}

/// Record that `item` was installed for `profiles` (PROF-20), unioning the
/// names into its reasons.
///
/// `pre_existing` says whether the item was already installed before this
/// profile install touched it (the caller knows; the entry alone cannot tell
/// a fresh install from a by-name one, both having no reasons). A pre-existing
/// item with no reasons was installed by name or before profiles existed, so
/// it is marked explicit first: enabling a profile must not make a
/// hand-installed item removable by disabling that profile later (PROF-25).
pub fn tag_reasons(item: &mut InstalledItem, profiles: &BTreeSet<String>, pre_existing: bool) {
    if profiles.is_empty() {
        return;
    }
    if pre_existing && item.profiles.is_empty() {
        item.explicit = true;
    }
    let mut set: BTreeSet<String> = item.profiles.drain(..).collect();
    set.extend(profiles.iter().cloned());
    item.profiles = set.into_iter().collect();
}

/// A by-name `learn` of `item` (PROF-20): mark it explicit, keeping its
/// profile names. An item with no profile reasons is already explicit
/// ([`InstalledItem::is_explicit`]), so the flag is left unset there to keep
/// the manifest entry unchanged.
pub fn mark_explicit(item: &mut InstalledItem) {
    if !item.profiles.is_empty() {
        item.explicit = true;
    }
}

/// Record an applied profile install on `source` (PROF-16, PROF-19): union
/// `enabled` into its recorded set and `applied_keys` into its snapshot, both
/// kept sorted. Pass only the keys actually installed or already present: a
/// declined addition stays out of the snapshot so it is offered again
/// (PROF-21).
pub fn record_applied(source: &mut Source, enabled: &[String], applied_keys: &[String]) {
    let union = |cur: Option<Vec<String>>, add: &[String]| -> Vec<String> {
        let mut set: BTreeSet<String> = cur.unwrap_or_default().into_iter().collect();
        set.extend(add.iter().cloned());
        set.into_iter().collect()
    };
    source.profiles = Some(union(source.profiles.take(), enabled));
    source.profile_snapshot = Some(union(source.profile_snapshot.take(), applied_keys));
}

/// What disabling one profile does (PROF-25), computed by [`disable_plan`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DisablePlan {
    /// Whether the profile was in the source's recorded set at all. When
    /// false the plan is a no-op and the caller decides how to report it.
    pub was_enabled: bool,
    /// The recorded set after the disable; `None` when nothing stays enabled,
    /// which also clears the snapshot (the source stops being
    /// profile-tracked).
    pub remaining_enabled: Option<Vec<String>>,
    /// Manifest keys of items whose every recorded reason is now disabled and
    /// that are not explicit: forget candidates (subject to DEP-60, the
    /// caller's check).
    pub removable: Vec<String>,
    /// Manifest keys of kept items that lose some reasons, with their new
    /// reason list.
    pub strip: Vec<(String, Vec<String>)>,
    /// The snapshot after the disable: the remaining expansion's keys, or
    /// `None` alongside `remaining_enabled == None`.
    pub new_snapshot: Option<Vec<String>>,
}

/// Plan disabling profile `name` on `source` (PROF-25). `items_of_source` are
/// the source's manifest entries; `remaining_expansion` is [`expand`] of the
/// remaining enabled set (the caller computes it against the current catalog).
pub fn disable_plan(
    source: &Source,
    items_of_source: &[&InstalledItem],
    name: &str,
    remaining_expansion: &Expansion,
) -> DisablePlan {
    let current: Vec<String> = source.profiles.clone().unwrap_or_default();
    if !source.is_profile_tracked() || !current.iter().any(|p| p == name) {
        return DisablePlan {
            was_enabled: false,
            remaining_enabled: source.profiles.clone(),
            removable: Vec::new(),
            strip: Vec::new(),
            new_snapshot: source.profile_snapshot.clone(),
        };
    }
    let remaining: Vec<String> = current.into_iter().filter(|p| p != name).collect();
    let mut plan = DisablePlan {
        was_enabled: true,
        ..DisablePlan::default()
    };
    for item in items_of_source {
        if item.profiles.is_empty() {
            continue;
        }
        let kept: Vec<String> = item
            .profiles
            .iter()
            .filter(|p| remaining.contains(p))
            .cloned()
            .collect();
        if kept.len() == item.profiles.len() {
            continue;
        }
        let key: String = item.key().into();
        if kept.is_empty() && !item.is_explicit() {
            plan.removable.push(key);
        } else {
            plan.strip.push((key, kept));
        }
    }
    if remaining.is_empty() {
        plan.remaining_enabled = None;
        plan.new_snapshot = None;
    } else {
        plan.remaining_enabled = Some(remaining);
        plan.new_snapshot = Some(remaining_expansion.key_list());
    }
    plan
}

/// Refuse profile flags on an item link (PROF-8): `flags` are the profile
/// flags the consumer set, spelled as typed (`--profile`, ...). The first one
/// is named in the error.
pub fn reject_on_item_link(flags: &[&str]) -> Result<()> {
    match flags.first() {
        Some(flag) => Err(MindError::ProfileFlagOnItemLink {
            flag: (*flag).to_string(),
        }),
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    const SRC: &str = "local/o/r";

    fn table(rows: &[(&str, &[&str])]) -> ProfileTable {
        rows.iter()
            .map(|(k, v)| (k.to_string(), v.iter().map(|s| s.to_string()).collect()))
            .collect()
    }

    fn cat(kind: ItemKind, name: &str, source: &str) -> CatalogItem {
        CatalogItem {
            kind,
            name: name.to_string(),
            source: source.to_string(),
            prefix: Some("ns".to_string()),
            path: PathBuf::from(name),
            description: None,
            when_to_use: None,
            link_rel: None,
            bin: None,
            build: None,
            requires: Vec::new(),
            ignore: None,
            expand: Vec::new(),
            hooks: Vec::new(),
        }
    }

    fn catalog() -> Vec<CatalogItem> {
        vec![
            cat(ItemKind::Skill, "review", SRC),
            cat(ItemKind::Skill, "test", SRC),
            cat(ItemKind::Skill, "deploy", SRC),
            cat(ItemKind::Rule, "style", SRC),
            cat(ItemKind::Agent, "planner", SRC),
            // Another source's item: never reachable from SRC's profiles.
            cat(ItemKind::Agent, "foreign", "local/x/y"),
        ]
    }

    fn installed(name: &str, profiles: &[&str], explicit: bool) -> InstalledItem {
        InstalledItem {
            kind: ItemKind::Skill,
            name: name.to_string(),
            bare_name: name.to_string(),
            source: SRC.to_string(),
            commit: "c".to_string(),
            hash: "h".to_string(),
            store: format!("store/skill/{name}"),
            links: Vec::new(),
            description: None,
            install_hooks: Vec::new(),
            dropped_requires: Vec::new(),
            profiles: profiles.iter().map(|s| s.to_string()).collect(),
            explicit,
            version: None,
        }
    }

    fn set(names: &[&str]) -> BTreeSet<String> {
        names.iter().map(|s| s.to_string()).collect()
    }

    fn validate_msg(t: &ProfileTable) -> String {
        match validate_table(t, Path::new("mind.toml")) {
            Err(MindError::MindToml { msg, .. }) => msg,
            other => panic!("expected a MindToml error, got {other:?}"),
        }
    }

    // spec: PROF-2
    #[test]
    fn name_grammar() {
        for ok in ["default", "a", "0", "full-set", "x_y", "a1-b_2"] {
            assert!(is_valid_name(ok), "{ok}");
        }
        assert!(is_valid_name(&"a".repeat(64)), "64 bytes is the limit");
        assert!(!is_valid_name(&"a".repeat(65)), "65 bytes is over");
        for bad in [
            "",
            "Full",
            "-a",
            "_a",
            "a b",
            "a.b",
            "skill:review",
            "a:b",
            "é",
        ] {
            assert!(!is_valid_name(bad), "{bad:?} must be invalid");
        }
        let t = table(&[("Bad", &[])]);
        let msg = validate_msg(&t);
        assert!(msg.starts_with("[profiles]: profile name 'Bad'"), "{msg}");
        let t = table(&[("a:b", &[])]);
        assert!(validate_msg(&t).contains("contains ':'"));
    }

    // spec: PROF-3
    #[test]
    fn member_forms() {
        assert!(matches!(parse_member("*"), Ok(Member::All)));
        match parse_member("skill:review").unwrap() {
            Member::Item { kind, name, glob } => {
                assert_eq!(kind, ItemKind::Skill);
                assert_eq!(name, "review");
                assert!(glob.is_none());
            }
            other => panic!("{other:?}"),
        }
        match parse_member("agent:*").unwrap() {
            Member::Item { kind, glob, .. } => {
                assert_eq!(kind, ItemKind::Agent);
                assert!(glob.is_some());
            }
            other => panic!("{other:?}"),
        }
        match parse_member("command:frontend:build").unwrap() {
            Member::Item { kind, name, .. } => {
                assert_eq!(kind, ItemKind::Command);
                assert_eq!(name, "frontend:build", "kind splits at the first ':'");
            }
            other => panic!("{other:?}"),
        }
        assert!(matches!(parse_member("basic"), Ok(Member::Profile(p)) if p == "basic"));
        assert!(parse_member("skill:").is_err());
        assert!(parse_member("skill:[").is_err(), "malformed glob");
        assert!(parse_member("Basic").is_err());
    }

    // spec: PROF-3
    #[test]
    fn unknown_kind_and_undeclared_profile_members_are_mindtoml_errors() {
        let t = table(&[("basic", &["widget:x"])]);
        let msg = validate_msg(&t);
        assert!(
            msg.starts_with("[profiles]: profile 'basic': member 'widget:x'"),
            "{msg}"
        );
        assert!(msg.contains("unknown kind 'widget'"), "{msg}");

        let t = table(&[("full", &["basic"])]);
        let msg = validate_msg(&t);
        assert!(msg.starts_with("[profiles]: "), "{msg}");
        assert!(
            msg.contains("'basic' names a profile the table does not declare"),
            "{msg}"
        );

        let ok = table(&[
            ("default", &["basic"]),
            ("basic", &["skill:review", "rule:style"]),
            ("full", &["basic", "agent:*"]),
            ("all", &["*"]),
        ]);
        validate_table(&ok, Path::new("mind.toml")).expect("the spec's example table is valid");
    }

    // spec: PROF-4
    #[test]
    fn cycles_are_named() {
        let t = table(&[("loop", &["loop"])]);
        assert_eq!(
            validate_msg(&t),
            "[profiles]: profile inclusion cycle: loop -> loop"
        );
        let t = table(&[("basic", &["full"]), ("full", &["basic", "skill:x"])]);
        assert_eq!(
            validate_msg(&t),
            "[profiles]: profile inclusion cycle: basic -> full -> basic"
        );
        let t = table(&[("a", &["b"]), ("b", &["c"]), ("c", &["a"]), ("d", &["a"])]);
        assert_eq!(
            validate_msg(&t),
            "[profiles]: profile inclusion cycle: a -> b -> c -> a"
        );
        // A diamond is not a cycle.
        let t = table(&[("a", &["b", "c"]), ("b", &["d"]), ("c", &["d"]), ("d", &[])]);
        validate_table(&t, Path::new("mind.toml")).expect("diamond is acyclic");
    }

    // spec: PROF-7
    #[test]
    fn enabled_set_default_semantics() {
        let t = table(&[("default", &["basic"]), ("basic", &[]), ("full", &[])]);
        let plain = Selection {
            include_default: true,
            ..Selection::default()
        };
        assert_eq!(enabled_set(Some(&t), &plain, SRC).unwrap(), vec!["default"]);
        let named = Selection {
            include_default: true,
            named: vec!["full".into(), "full".into()],
            all: false,
        };
        assert_eq!(
            enabled_set(Some(&t), &named, SRC).unwrap(),
            vec!["default", "full"],
            "naming adds to default, deduped and sorted"
        );
        let opt_out = Selection {
            include_default: false,
            named: vec!["full".into()],
            all: false,
        };
        assert_eq!(enabled_set(Some(&t), &opt_out, SRC).unwrap(), vec!["full"]);
        let all = Selection {
            include_default: false,
            named: vec![],
            all: true,
        };
        assert_eq!(
            enabled_set(Some(&t), &all, SRC).unwrap(),
            vec!["basic", "default", "full"]
        );
        // No `default` declared: profiles are opt-in only.
        let no_default = table(&[("basic", &[])]);
        assert!(
            enabled_set(Some(&no_default), &plain, SRC)
                .unwrap()
                .is_empty()
        );
        assert!(enabled_set(None, &plain, SRC).unwrap().is_empty());
    }

    #[test]
    fn enabled_set_undeclared_is_profile_not_found() {
        let t = table(&[("basic", &[]), ("default", &[])]);
        let sel = Selection {
            include_default: true,
            named: vec!["nope".into()],
            all: false,
        };
        match enabled_set(Some(&t), &sel, SRC) {
            Err(MindError::ProfileNotFound {
                profile,
                source_name,
                declared,
            }) => {
                assert_eq!(profile, "nope");
                assert_eq!(source_name, SRC);
                assert_eq!(declared, vec!["basic", "default"]);
            }
            other => panic!("{other:?}"),
        }
        match enabled_set(None, &sel, SRC) {
            Err(MindError::ProfileNotFound { declared, .. }) => assert!(declared.is_empty()),
            other => panic!("{other:?}"),
        }
    }

    // spec: PROF-26
    #[test]
    fn curator_undeclared_profile_is_bad_reference() {
        let t = table(&[("basic", &[])]);
        let sel = Selection {
            include_default: true,
            named: vec!["full".into()],
            all: false,
        };
        match enabled_set_for_curator(Some(&t), &sel, "local/n/s", "local/c/c") {
            Err(e @ MindError::BadReference { .. }) => {
                let msg = e.to_string();
                assert!(msg.contains("local/c/c"), "{msg}");
                assert!(msg.contains("profile 'full'"), "{msg}");
                assert!(msg.contains("local/n/s"), "{msg}");
            }
            other => panic!("{other:?}"),
        }
        let ok = Selection {
            include_default: false,
            named: vec!["basic".into()],
            all: false,
        };
        assert_eq!(
            enabled_set_for_curator(Some(&t), &ok, "local/n/s", "local/c/c").unwrap(),
            vec!["basic"]
        );
    }

    #[test]
    fn parse_cli_names_splits_trims_dedupes() {
        let got = parse_cli_names(&["full, basic".into(), "basic".into(), "x".into()]).unwrap();
        assert_eq!(got, vec!["full", "basic", "x"]);
        for bad in ["Full", "a,,b", "", "skill:x"] {
            match parse_cli_names(&[bad.to_string()]) {
                Err(MindError::InvalidProfileName { .. }) => {}
                other => panic!("{bad:?}: {other:?}"),
            }
        }
    }

    // spec: PROF-1 PROF-3 PROF-5
    #[test]
    fn expand_matches_bare_names_of_own_source_and_attributes_to_enabled() {
        let t = table(&[
            ("default", &["basic"]),
            ("basic", &["skill:review", "rule:style"]),
            ("full", &["basic", "agent:*", "skill:deploy"]),
        ]);
        let exp = expand(&t, &["full".into()], &catalog(), SRC).unwrap();
        assert_eq!(
            exp.key_list(),
            vec![
                "agent:planner",
                "rule:style",
                "skill:deploy",
                "skill:review"
            ],
            "bare keys, own source only (agent:foreign excluded), namespace not applied"
        );
        assert_eq!(
            exp.keys["skill:review"],
            set(&["full"]),
            "attributed to the enabled profile"
        );

        let both = expand(&t, &["default".into(), "full".into()], &catalog(), SRC).unwrap();
        assert_eq!(both.keys["skill:review"], set(&["default", "full"]));
        assert_eq!(both.keys["skill:deploy"], set(&["full"]));

        let all = table(&[("all", &["*"])]);
        let exp = expand(&all, &["all".into()], &catalog(), SRC).unwrap();
        assert_eq!(exp.keys.len(), 5, "'*' is every item of this source");

        // A namespaced pattern is not source truth: it does not match.
        let ns = table(&[("p", &["skill:ns:review"])]);
        assert!(expand(&ns, &["p".into()], &catalog(), SRC).is_err());
    }

    // spec: PROF-5
    #[test]
    fn exact_miss_is_bad_reference_glob_miss_is_not() {
        let t = table(&[("basic", &["skill:missing"]), ("full", &["basic"])]);
        match expand(&t, &["full".into()], &catalog(), SRC) {
            Err(e @ MindError::BadReference { .. }) => {
                let msg = e.to_string();
                assert!(
                    msg.contains("profile 'basic'"),
                    "names the listing profile: {msg}"
                );
                assert!(msg.contains("skill:missing"), "{msg}");
                assert!(msg.contains(SRC), "{msg}");
            }
            other => panic!("{other:?}"),
        }
        let globs = table(&[("p", &["command:*", "skill:rev*"])]);
        let exp = expand(&globs, &["p".into()], &catalog(), SRC).unwrap();
        assert_eq!(exp.key_list(), vec!["skill:review"]);
        // An exact name that exists only in another source is still a miss.
        let foreign = table(&[("p", &["agent:foreign"])]);
        assert!(expand(&foreign, &["p".into()], &catalog(), SRC).is_err());
    }

    // spec: PROF-30
    #[test]
    fn lint_collects_every_problem() {
        let t = table(&[
            ("a", &["skill:missing", "skill:review", "command:*"]),
            ("b", &["workflow:*"]),
            ("c", &["b"]),
            ("d", &["a"]),
        ]);
        let l = lint(&t, &catalog(), SRC);
        assert_eq!(l.missing_exact, vec![("a".into(), "skill:missing".into())]);
        assert_eq!(
            l.empty_globs,
            vec![
                ("a".into(), "command:*".into()),
                ("b".into(), "workflow:*".into())
            ]
        );
        assert_eq!(l.empty_profiles, vec!["b".to_string(), "c".to_string()]);
    }

    // spec: PROF-23
    #[test]
    fn diff_reports_additions_and_departures_and_keeps_forgotten_members_out() {
        let mut exp = Expansion::default();
        exp.keys.insert("skill:review".into(), set(&["default"]));
        exp.keys.insert("skill:new".into(), set(&["full"]));
        // skill:forgotten was forgotten by the consumer but is still a member
        // and still in the snapshot: not an addition.
        exp.keys.insert("skill:forgotten".into(), set(&["default"]));
        let snapshot = vec![
            "skill:review".to_string(),
            "skill:forgotten".to_string(),
            "skill:gone".to_string(),
        ];
        let d = diff(&snapshot, &exp);
        assert_eq!(d.additions, vec![("skill:new".to_string(), set(&["full"]))]);
        assert_eq!(d.departures, vec!["skill:gone".to_string()]);
    }

    // spec: PROF-20
    #[test]
    fn tag_reasons_and_explicit_rule() {
        // Fresh profile install: tagged, not explicit.
        let mut fresh = installed("a", &[], false);
        tag_reasons(&mut fresh, &set(&["full"]), false);
        assert_eq!(fresh.profiles, vec!["full"]);
        assert!(!fresh.explicit && !fresh.is_explicit());

        // Union with existing reasons, sorted.
        tag_reasons(&mut fresh, &set(&["default", "full"]), true);
        assert_eq!(fresh.profiles, vec!["default", "full"]);
        assert!(
            !fresh.explicit,
            "an item that already had reasons stays non-explicit"
        );

        // Pre-existing by-name (or pre-feature) item: marked explicit first.
        let mut by_name = installed("b", &[], false);
        assert!(by_name.is_explicit());
        tag_reasons(&mut by_name, &set(&["default"]), true);
        assert_eq!(by_name.profiles, vec!["default"]);
        assert!(by_name.explicit && by_name.is_explicit());

        // A by-name learn marks explicit, keeping the names.
        let mut tagged = installed("c", &["full"], false);
        mark_explicit(&mut tagged);
        assert!(tagged.explicit);
        assert_eq!(tagged.profiles, vec!["full"]);
        let mut plain = installed("d", &[], false);
        mark_explicit(&mut plain);
        assert!(
            !plain.explicit && plain.is_explicit(),
            "no reasons: already explicit"
        );

        // Reinstall carries reasons over before tagging.
        let mut re = installed("c", &[], false);
        carry_reasons(&mut re, &tagged);
        assert_eq!(re.profiles, vec!["full"]);
        assert!(re.explicit);
    }

    // spec: PROF-19
    #[test]
    fn record_applied_unions_and_sorts() {
        let mut s = crate::source::parse_spec("o/r").unwrap();
        assert!(!s.is_profile_tracked());
        record_applied(
            &mut s,
            &["full".into()],
            &["skill:b".into(), "skill:a".into()],
        );
        record_applied(
            &mut s,
            &["default".into(), "full".into()],
            &["skill:a".into()],
        );
        assert!(s.is_profile_tracked());
        assert_eq!(s.profiles.as_deref().unwrap(), ["default", "full"]);
        assert_eq!(
            s.profile_snapshot.as_deref().unwrap(),
            ["skill:a", "skill:b"]
        );
    }

    // spec: PROF-25
    #[test]
    fn disable_plan_removes_only_items_left_without_reasons() {
        let mut s = crate::source::parse_spec("o/r").unwrap();
        s.profiles = Some(vec!["default".into(), "full".into()]);
        s.profile_snapshot = Some(vec!["skill:only-full".into(), "skill:both".into()]);
        let only_full = installed("only-full", &["full"], false);
        let both = installed("both", &["default", "full"], false);
        let explicit = installed("explicit", &["full"], true);
        let by_name = installed("by-name", &[], false);
        let items = [&only_full, &both, &explicit, &by_name];
        let mut remaining = Expansion::default();
        remaining
            .keys
            .insert("skill:both".into(), set(&["default"]));

        let plan = disable_plan(&s, &items, "full", &remaining);
        assert!(plan.was_enabled);
        assert_eq!(plan.removable, vec!["skill:only-full".to_string()]);
        assert_eq!(
            plan.strip,
            vec![
                ("skill:both".to_string(), vec!["default".to_string()]),
                ("skill:explicit".to_string(), vec![]),
            ]
        );
        assert_eq!(plan.remaining_enabled, Some(vec!["default".to_string()]));
        assert_eq!(plan.new_snapshot, Some(vec!["skill:both".to_string()]));

        // Disabling the last enabled profile untracks the source.
        s.profiles = Some(vec!["default".into()]);
        let only_default = installed("d", &["default"], false);
        let plan = disable_plan(&s, &[&only_default], "default", &Expansion::default());
        assert_eq!(plan.removable, vec!["skill:d".to_string()]);
        assert_eq!(plan.remaining_enabled, None);
        assert_eq!(plan.new_snapshot, None);

        // A profile that is not enabled is a no-op.
        let plan = disable_plan(&s, &[&only_default], "full", &Expansion::default());
        assert!(!plan.was_enabled);
        assert!(plan.removable.is_empty() && plan.strip.is_empty());
        assert_eq!(plan.remaining_enabled, s.profiles);
    }

    // spec: PROF-8
    #[test]
    fn profile_flags_on_item_link_are_refused() {
        reject_on_item_link(&[]).expect("no flags is fine");
        match reject_on_item_link(&["--no-default-profile", "--profile"]) {
            Err(MindError::ProfileFlagOnItemLink { flag }) => {
                assert_eq!(flag, "--no-default-profile")
            }
            other => panic!("{other:?}"),
        }
    }

    // spec: PROF-14
    #[test]
    fn parse_cli_names_rejects_blank_and_empty_segments() {
        for bad in [" ", "a,", ",a", "a, ,b", "\t", "a,\u{1b}[31mb"] {
            match parse_cli_names(&[bad.to_string()]) {
                Err(MindError::InvalidProfileName { .. }) => {}
                other => panic!("{bad:?}: {other:?}"),
            }
        }
        // No values at all is an empty selection, not an error.
        assert!(parse_cli_names(&[]).unwrap().is_empty());
        // Surrounding whitespace is trimmed; order is first-seen across values.
        let got = parse_cli_names(&[" b ,a".into(), "a, b,c".into()]).unwrap();
        assert_eq!(got, vec!["b", "a", "c"]);
        // The error echoes the trimmed name and strips escapes from it.
        let e = parse_cli_names(&["  \u{1b}[31mBad  ".into()]).unwrap_err();
        let msg = e.to_string();
        assert!(!msg.contains('\u{1b}'), "{msg}");
        assert!(msg.starts_with("invalid profile name"), "{msg}");
    }

    // spec: PROF-14
    #[test]
    fn profile_not_found_message_is_sanitized_end_to_end() {
        let t = table(&[("basic", &[])]);
        let sel = Selection {
            include_default: false,
            named: vec!["\u{1b}[31mfull".into()],
            all: false,
        };
        let e = enabled_set(Some(&t), &sel, "local/\u{1b}[2Jo/r").unwrap_err();
        let msg = e.to_string();
        assert!(!msg.contains('\u{1b}'), "{msg:?}");
        assert!(msg.contains("basic"), "{msg}");
    }

    // spec: PROF-26
    #[test]
    fn curator_bad_reference_is_sanitized() {
        let t = table(&[("basic", &[])]);
        let sel = Selection {
            include_default: false,
            named: vec!["\u{1b}[31mfull".into()],
            all: false,
        };
        let e =
            enabled_set_for_curator(Some(&t), &sel, "n/\u{1b}[2Js", "c/\u{1b}[31mc").unwrap_err();
        let msg = e.to_string();
        assert!(!msg.contains('\u{1b}'), "{msg:?}");
    }

    // spec: PROF-2 PROF-3
    #[test]
    fn source_derived_text_in_validation_errors_is_sanitized() {
        let t = table(&[("p", &["\u{1b}[31mwidget:x"])]);
        assert!(!validate_msg(&t).contains('\u{1b}'));
        let t = table(&[("p", &["skill:\u{1b}[31m"]), ("q", &["\u{1b}[31mQ"])]);
        assert!(!validate_msg(&t).contains('\u{1b}'));
        let t = table(&[("\u{1b}[31mbad", &[])]);
        assert!(!validate_msg(&t).contains('\u{1b}'));
        // An exact miss from expand strips the member text too.
        let t = table(&[("p", &["skill:\u{1b}]0;title\u{7}missing"])]);
        let msg = expand(&t, &["p".into()], &catalog(), SRC)
            .unwrap_err()
            .to_string();
        assert!(!msg.contains('\u{1b}'), "{msg:?}");
    }

    // spec: PROF-3
    #[test]
    fn member_edge_forms() {
        assert!(parse_member("").is_err());
        assert!(parse_member(":x").is_err(), "empty kind word");
        assert!(
            parse_member("**").is_err(),
            "bare glob is not a profile name"
        );
        assert!(parse_member("skill:a:b").is_ok(), "colon in name is kept");
        // '?' and '[' trigger glob mode; a lone ']' does not.
        assert!(matches!(
            parse_member("skill:a?").unwrap(),
            Member::Item { glob: Some(_), .. }
        ));
        assert!(matches!(
            parse_member("skill:a]").unwrap(),
            Member::Item { glob: None, .. }
        ));
        // Kind words are case-sensitive.
        assert!(parse_member("Skill:x").is_err());
    }

    // spec: PROF-3 PROF-5
    #[test]
    fn command_group_names_match_exactly_and_by_glob() {
        let items = vec![
            cat(ItemKind::Command, "frontend:build", SRC),
            cat(ItemKind::Command, "frontend:lint", SRC),
            cat(ItemKind::Command, "backend:build", SRC),
        ];
        let t = table(&[
            ("exact", &["command:frontend:build"]),
            ("glob", &["command:frontend:*"]),
        ]);
        let e = expand(&t, &["exact".into()], &items, SRC).unwrap();
        assert_eq!(e.key_list(), vec!["command:frontend:build"]);
        let g = expand(&t, &["glob".into()], &items, SRC).unwrap();
        assert_eq!(
            g.key_list(),
            vec!["command:frontend:build", "command:frontend:lint"]
        );
    }

    // spec: PROF-5
    #[test]
    fn kind_must_match_and_blank_catalog_is_an_exact_miss() {
        // `rule:review` does not match the skill of that name.
        let t = table(&[("p", &["rule:review"])]);
        assert!(expand(&t, &["p".into()], &catalog(), SRC).is_err());
        // Empty catalog: exact misses, glob and '*' do not.
        let t = table(&[("g", &["skill:*", "*"])]);
        assert!(expand(&t, &["g".into()], &[], SRC).unwrap().keys.is_empty());
        let t = table(&[("x", &["skill:a"])]);
        assert!(expand(&t, &["x".into()], &[], SRC).is_err());
    }

    // spec: PROF-5
    #[test]
    fn expand_overlapping_enabled_profiles_attribute_to_each_root() {
        // `full` includes `default`; both enabled: a shared key is attributed
        // to both roots, a key only reachable through `full` to `full` alone.
        let t = table(&[
            ("default", &["skill:review"]),
            ("full", &["default", "skill:deploy"]),
            ("other", &["skill:review", "skill:test"]),
        ]);
        let enabled: Vec<String> = vec!["default".into(), "full".into(), "other".into()];
        let e = expand(&t, &enabled, &catalog(), SRC).unwrap();
        assert_eq!(e.keys["skill:review"], set(&["default", "full", "other"]));
        assert_eq!(e.keys["skill:deploy"], set(&["full"]));
        assert_eq!(e.keys["skill:test"], set(&["other"]));
        // Enabling the same root twice changes nothing.
        let twice: Vec<String> = vec!["full".into(), "full".into()];
        let once: Vec<String> = vec!["full".into()];
        assert_eq!(
            expand(&t, &twice, &catalog(), SRC).unwrap(),
            expand(&t, &once, &catalog(), SRC).unwrap()
        );
        // A diamond still attributes each key once, to the root.
        let d = table(&[
            ("a", &["b", "c"]),
            ("b", &["d"]),
            ("c", &["d"]),
            ("d", &["skill:test"]),
        ]);
        let e = expand(&d, &["a".into()], &catalog(), SRC).unwrap();
        assert_eq!(e.keys["skill:test"], set(&["a"]));
    }

    // spec: PROF-5
    #[test]
    fn expand_undeclared_or_empty_enabled_contributes_nothing_and_terminates_on_cycles() {
        let t = table(&[("a", &["skill:review"])]);
        assert!(
            expand(&t, &["ghost".into()], &catalog(), SRC)
                .unwrap()
                .keys
                .is_empty()
        );
        assert!(expand(&t, &[], &catalog(), SRC).unwrap().keys.is_empty());
        // An unvalidated cyclic table must not hang expansion.
        let cyc = table(&[("a", &["b", "skill:review"]), ("b", &["a", "skill:test"])]);
        let e = expand(&cyc, &["a".into()], &catalog(), SRC).unwrap();
        assert_eq!(e.key_list(), vec!["skill:review", "skill:test"]);
        // A dangling profile include in an unvalidated table is skipped.
        let dangling = table(&[("a", &["nope", "skill:review"])]);
        let e = expand(&dangling, &["a".into()], &catalog(), SRC).unwrap();
        assert_eq!(e.key_list(), vec!["skill:review"]);
    }

    // spec: PROF-30
    #[test]
    fn lint_terminates_on_cycles_and_tolerates_garbage() {
        let cyc = table(&[
            ("a", &["b"]),
            ("b", &["a"]),
            ("self", &["self"]),
            ("ok", &["a", "skill:review"]),
        ]);
        let l = lint(&cyc, &catalog(), SRC);
        assert_eq!(
            l.empty_profiles,
            vec!["a".to_string(), "b".to_string(), "self".to_string()]
        );
        assert!(l.missing_exact.is_empty() && l.empty_globs.is_empty());
        // Unparseable and dangling members are skipped, not reported.
        let junk = table(&[("p", &["widget:x", "Nope", "ghost", "skill:review"])]);
        let l = lint(&junk, &catalog(), SRC);
        assert_eq!(l, ProfileLint::default());
        // A profile of only unparseable members is empty.
        let only_junk = table(&[("p", &["widget:x"]), ("none", &[])]);
        let l = lint(&only_junk, &catalog(), SRC);
        assert_eq!(l.empty_profiles, vec!["none".to_string(), "p".to_string()]);
    }

    // spec: PROF-30
    #[test]
    fn lint_scopes_to_own_source_and_reports_star_on_empty_catalog() {
        let t = table(&[("p", &["agent:foreign", "*"])]);
        let l = lint(&t, &catalog(), "local/other/none");
        assert_eq!(l.missing_exact, vec![("p".into(), "agent:foreign".into())]);
        assert_eq!(l.empty_globs, vec![("p".into(), "*".into())]);
        assert_eq!(l.empty_profiles, vec!["p".to_string()]);
        // A profile reaching items only through an include is not empty.
        let t = table(&[("inner", &["skill:review"]), ("outer", &["inner"])]);
        let l = lint(&t, &catalog(), SRC);
        assert_eq!(l, ProfileLint::default());
    }

    // spec: PROF-23
    #[test]
    fn diff_edge_cases() {
        let none = diff(&[], &Expansion::default());
        assert_eq!(none, SnapshotDiff::default());
        let mut exp = Expansion::default();
        exp.keys.insert("skill:a".into(), set(&["x"]));
        let d = diff(&[], &exp);
        assert_eq!(d.additions.len(), 1);
        assert!(d.departures.is_empty());
        let d = diff(&["skill:a".into()], &Expansion::default());
        assert!(d.additions.is_empty());
        assert_eq!(d.departures, vec!["skill:a".to_string()]);
        // Duplicate snapshot entries report one departure, sorted.
        let d = diff(
            &["skill:z".into(), "skill:b".into(), "skill:z".into()],
            &Expansion::default(),
        );
        assert_eq!(
            d.departures,
            vec!["skill:b".to_string(), "skill:z".to_string()]
        );
    }

    // spec: PROF-20
    #[test]
    fn carry_and_tag_reason_edges() {
        // carry_reasons replaces whatever the fresh entry held.
        let mut fresh = installed("a", &["stale"], true);
        let prev = installed("a", &["full"], false);
        carry_reasons(&mut fresh, &prev);
        assert_eq!(fresh.profiles, vec!["full"]);
        assert!(!fresh.explicit, "explicit is carried too, including false");
        // carry from an entry with no reasons clears them.
        let mut fresh = installed("a", &["x"], false);
        carry_reasons(&mut fresh, &installed("a", &[], false));
        assert!(fresh.profiles.is_empty());
        // carry then tag keeps old reasons and adds the new one, sorted.
        let mut fresh = installed("a", &[], false);
        carry_reasons(&mut fresh, &installed("a", &["zeta"], false));
        tag_reasons(&mut fresh, &set(&["alpha"]), true);
        assert_eq!(fresh.profiles, vec!["alpha", "zeta"]);
        assert!(!fresh.explicit, "had reasons, so not pre-existing by name");

        // Empty profile set is a no-op even when pre-existing.
        let mut by_name = installed("b", &[], false);
        tag_reasons(&mut by_name, &BTreeSet::new(), true);
        assert!(by_name.profiles.is_empty() && !by_name.explicit);

        // Idempotent re-tag: same reasons, no duplicates, never explicit.
        let mut t = installed("c", &[], false);
        tag_reasons(&mut t, &set(&["full"]), false);
        tag_reasons(&mut t, &set(&["full"]), false);
        assert_eq!(t.profiles, vec!["full"]);
        assert!(!t.explicit);

        // Already-explicit stays explicit through a re-tag.
        let mut e = installed("d", &["full"], true);
        tag_reasons(&mut e, &set(&["default"]), true);
        assert!(e.explicit);
        assert_eq!(e.profiles, vec!["default", "full"]);

        // mark_explicit is idempotent.
        mark_explicit(&mut e);
        assert!(e.explicit);
    }

    // spec: PROF-19
    #[test]
    fn record_applied_with_empty_inputs_still_tracks() {
        let mut s = crate::source::parse_spec("o/r").unwrap();
        record_applied(&mut s, &[], &[]);
        assert!(s.is_profile_tracked());
        assert_eq!(s.profiles, Some(vec![]));
        assert_eq!(s.profile_snapshot, Some(vec![]));
        // Never removes: a later smaller application keeps earlier names.
        record_applied(&mut s, &["b".into()], &["skill:b".into()]);
        record_applied(&mut s, &["a".into()], &[]);
        assert_eq!(s.profiles.as_deref().unwrap(), ["a", "b"]);
        assert_eq!(s.profile_snapshot.as_deref().unwrap(), ["skill:b"]);
    }

    // spec: PROF-25
    #[test]
    fn disable_plan_edges() {
        // Untracked source: no-op, nothing carried invented.
        let s = crate::source::parse_spec("o/r").unwrap();
        let it = installed("a", &["full"], false);
        let p = disable_plan(&s, &[&it], "full", &Expansion::default());
        assert!(!p.was_enabled);
        assert_eq!(p.remaining_enabled, None);
        assert_eq!(p.new_snapshot, None);
        assert!(p.removable.is_empty() && p.strip.is_empty());

        // Tracked with Some(empty) set: nothing to disable.
        let mut s = crate::source::parse_spec("o/r").unwrap();
        s.profiles = Some(vec![]);
        s.profile_snapshot = Some(vec!["skill:k".into()]);
        let p = disable_plan(&s, &[&it], "full", &Expansion::default());
        assert!(!p.was_enabled);
        assert_eq!(p.remaining_enabled, Some(vec![]));
        assert_eq!(p.new_snapshot, Some(vec!["skill:k".to_string()]));

        // Snapshot-only tracked source: profiles None, so no-op.
        let mut s = crate::source::parse_spec("o/r").unwrap();
        s.profile_snapshot = Some(vec![]);
        let p = disable_plan(&s, &[&it], "full", &Expansion::default());
        assert!(!p.was_enabled);
        assert_eq!(p.remaining_enabled, None);

        // Disabling the last profile: explicit items are stripped, by-name
        // items untouched, both clear the enabled set and snapshot.
        let mut s = crate::source::parse_spec("o/r").unwrap();
        s.profiles = Some(vec!["full".into()]);
        s.profile_snapshot = Some(vec!["skill:e".into()]);
        let exp = installed("e", &["full"], true);
        let by_name = installed("n", &[], false);
        let p = disable_plan(&s, &[&exp, &by_name], "full", &Expansion::default());
        assert!(p.was_enabled && p.removable.is_empty());
        assert_eq!(p.strip, vec![("skill:e".to_string(), vec![])]);
        assert_eq!(p.remaining_enabled, None);
        assert_eq!(p.new_snapshot, None);

        // Items of other reasons only are ignored; empty item list is fine.
        s.profiles = Some(vec!["a".into(), "b".into()]);
        let other = installed("o", &["b"], false);
        let p = disable_plan(&s, &[&other], "a", &Expansion::default());
        assert!(p.was_enabled && p.removable.is_empty() && p.strip.is_empty());
        assert_eq!(p.remaining_enabled, Some(vec!["b".to_string()]));
        assert_eq!(p.new_snapshot, Some(vec![]));
        let p = disable_plan(&s, &[], "a", &Expansion::default());
        assert!(p.was_enabled);

        // An item whose reasons are all already-disabled profiles is removable.
        let stale = installed("s", &["gone"], false);
        let p = disable_plan(&s, &[&stale], "a", &Expansion::default());
        assert_eq!(p.removable, vec!["skill:s".to_string()]);
    }

    // spec: PROF-9
    #[test]
    fn selection_from_nested_and_all_flag() {
        let sel = Selection::default();
        assert!(!sel.include_default, "Default derive is opt-out");
        let t = table(&[("default", &[]), ("x", &[])]);
        // `all` plus a named duplicate plus default: deduped.
        let s = Selection {
            include_default: true,
            named: vec!["x".into()],
            all: true,
        };
        assert_eq!(
            enabled_set(Some(&t), &s, SRC).unwrap(),
            vec!["default", "x"]
        );
        // Nothing selected, nothing enabled.
        assert!(
            enabled_set(Some(&t), &Selection::default(), SRC)
                .unwrap()
                .is_empty()
        );
        // `all` with no table is empty, not an error.
        let s = Selection {
            all: true,
            ..Selection::default()
        };
        assert!(enabled_set(None, &s, SRC).unwrap().is_empty());
    }
}
