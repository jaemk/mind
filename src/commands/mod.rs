//! Command implementations: one submodule per verb, plus the helpers they share.
//!
//! Submodules re-export into this module, and each one imports it with
//! `use super::*`, so a helper is reachable from every verb without a path.

mod absorb;
mod auto_update;
mod config;
mod forget;
mod homes;
mod hooks;
mod init_source;
mod introspect;
mod learn;
mod link;
mod lobes;
mod meld;
mod pin;
mod probe;
mod profiles;
mod recall;
mod refresh;
mod review;
mod shared;
mod sync;
#[cfg(test)]
mod test_support;
mod tracking;
mod unmeld;
mod upgrade;
mod warnings;

pub use absorb::*;
pub use auto_update::*;
pub use config::*;
pub use forget::*;
pub(crate) use homes::*;
pub(crate) use hooks::*;
pub use init_source::*;
pub use introspect::*;
pub use learn::*;
pub use link::*;
pub use lobes::*;
pub use meld::*;
pub use pin::*;
pub use probe::*;
pub use profiles::*;
pub use recall::*;
pub use review::*;
pub use shared::*;
pub use sync::*;
use tracking::*;
pub use unmeld::*;
pub use upgrade::*;
use warnings::*;

#[cfg(test)]
use refresh::parse_max_parallel_fetches;
use refresh::refresh_sources;
use std::collections::{BTreeSet, HashSet};
use std::io::Write;

use serde::Serialize;

use crate::catalog::{self, CatalogItem};
use crate::config::Config;
use crate::error::{ItemKind, MindError, Result};
use crate::git;
use crate::install;
use crate::manifest::Manifest;
use crate::mindfile::AuthFailureAction;
use crate::mindfile::HookEvent;
use crate::mindfile::MindToml;
use crate::paths::Paths;
use crate::plugin_manifest;
use crate::policy::Policy;
use crate::resolve::{
    is_glob, parse_item_ref, resolve, select, select_by_bare_refs, select_installed,
    source_matches, source_matches_glob,
};
use crate::source::{
    HookOrigin, ManifestOrigin, Pin, RecordedEvent, Registry, parse_spec, parse_spec_quiet,
};
// spec: DSC-69/CLI-224 -- `strip_ansi` used to be a private copy here, out of
// step with the shared, hardened `crate::sanitize::strip_ansi` (it also
// blocks Unicode directional marks and zero-width characters, and collapses a
// control-character run to one space instead of deleting it outright). Both
// copies applied to the exact same class of untrusted, source/curator-derived
// strings (plugin/marketplace descriptions, collision-prompt identities, auth-
// failure messages, git stderr), so the divergence meant only half of those
// call sites got the newer hardening. Every `commands` call site now routes
// through the one shared, hardened implementation.
use crate::sanitize::{display_path, strip_ansi};
