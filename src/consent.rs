//! The unified install plan's answer (HOOK-180..186).
//!
//! A verb that installs, upgrades, or removes items lists every hook its run
//! would offer and asks once. The answer is recorded here against each listed
//! hook, and the hook decision points (`hook::decide`, the item hook runners in
//! `install.rs`, `upgrade`'s source hook pass) consult it before prompting: a
//! listed hook runs or skips as answered, and only an unlisted one, or one the
//! user chose to review, is prompted on its own (HOOK-185).

use std::cell::RefCell;

use crate::sanitize::strip_ansi;

/// How the user answered a plan's hooks (HOOK-181).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Answer {
    /// `y`: run every listed hook.
    RunAll,
    /// `s` (or `n` on the hooks-only prompt): skip every listed hook.
    SkipAll,
    /// `r`: prompt each listed hook on its own, as before.
    Review,
}

/// A hook's identity within one run: its source, the item it belongs to (by
/// display key; `None` for a source hook), its event, and its command.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct HookKey {
    pub source: String,
    pub item: Option<String>,
    pub event: String,
    pub command: String,
}

impl HookKey {
    pub fn source_hook(source: &str, event: &str, command: &str) -> HookKey {
        HookKey {
            source: source.to_string(),
            item: None,
            event: event.to_string(),
            command: command.to_string(),
        }
    }

    pub fn item_hook(source: &str, item: &str, event: &str, command: &str) -> HookKey {
        HookKey {
            source: source.to_string(),
            item: Some(item.to_string()),
            event: event.to_string(),
            command: command.to_string(),
        }
    }
}

thread_local! {
    static ANSWERED: RefCell<Vec<(HookKey, Answer)>> = const { RefCell::new(Vec::new()) };
}

/// Record `answer` for each of `keys`. A later answer for the same key wins.
pub fn grant<'a>(keys: impl IntoIterator<Item = &'a HookKey>, answer: Answer) {
    ANSWERED.with(|a| {
        let mut a = a.borrow_mut();
        for key in keys {
            a.retain(|(k, _)| k != key);
            a.push((key.clone(), answer));
        }
    });
}

/// The plan answer covering `key`, if a plan listed it (HOOK-185: `None`
/// means the hook was never shown, so it must be prompted on its own).
pub fn answer_for(key: &HookKey) -> Option<Answer> {
    ANSWERED.with(|a| {
        a.borrow()
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, ans)| *ans)
    })
}

/// Forget every recorded answer.
pub fn reset() {
    ANSWERED.with(|a| a.borrow_mut().clear());
}

thread_local! {
    static DEPTH: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// Keeps recorded answers alive for one verb; see [`scope`].
pub struct Scope(());

impl Drop for Scope {
    fn drop(&mut self) {
        let depth = DEPTH.with(|d| {
            let n = d.get().saturating_sub(1);
            d.set(n);
            n
        });
        if depth == 0 {
            reset();
        }
    }
}

/// Enter a verb that asks a plan. Answers live until the outermost scope
/// ends, so a plan's answer never carries into a later verb in the same
/// process (the probe TUI runs several), while a verb that calls another
/// (`meld` installing through `learn`) keeps the answer it already got.
pub fn scope() -> Scope {
    DEPTH.with(|d| d.set(d.get() + 1));
    Scope(())
}

/// One hook a plan lists (HOOK-180), with the HOOK-20 disclosure fields.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedHook {
    pub key: HookKey,
    /// The hook's `name`, or its command when it has none (HOOK-51).
    pub label: String,
    pub optional: bool,
    pub commit: String,
    /// The directory the hook runs in.
    pub run_dir: String,
    /// For an item hook, the item's path in the source's clone: where to read
    /// the code, since `run_dir` does not hold it yet (HOOK-184).
    pub inspect: Option<String>,
    /// A version-control browse URL pinned to `commit` (HOOK-24).
    pub browse_url: Option<String>,
    /// The declared command(s) a consumer override replaced (HOOK-2, HOOK-56).
    pub replaced: Option<String>,
    /// Whether `--accept-hooks` already accepts it (HOOK-150).
    pub accepted: bool,
}

/// Render the hooks section of a plan (HOOK-180, HOOK-184). Every
/// source-derived field is sanitized (HOOK-91); the labels and the warning are
/// static text.
pub fn render(hooks: &[PlannedHook]) -> String {
    let mut out = String::new();
    out.push_str(&format!("====== hooks: {} ======\n", hooks.len()));
    out.push_str(
        "WARNING: each hook below runs arbitrary code from its source with your privileges.\n",
    );
    for (i, h) in hooks.iter().enumerate() {
        let need = if h.optional { "optional" } else { "required" };
        out.push_str(&format!("[{}] {}\n", i + 1, strip_ansi(&h.label)));
        out.push_str(&format!(
            "    Event:   {} ({need})\n",
            strip_ansi(&h.key.event)
        ));
        out.push_str(&format!("    Source:  {}\n", strip_ansi(&h.key.source)));
        if let Some(item) = &h.key.item {
            out.push_str(&format!("    Item:    {}\n", strip_ansi(item)));
        }
        if !h.commit.is_empty() {
            out.push_str(&format!("    Commit:  {}\n", strip_ansi(&h.commit)));
        }
        out.push_str(&format!("    Runs in: {}\n", strip_ansi(&h.run_dir)));
        if let Some(path) = &h.inspect {
            out.push_str(&format!(
                "    Inspect: {} (the source copy, before token expansion)\n",
                strip_ansi(path)
            ));
        }
        if let Some(url) = &h.browse_url {
            out.push_str(&format!("    Browse:  {}\n", strip_ansi(url)));
        }
        out.push_str(&format!("    Command: {}\n", strip_ansi(&h.key.command)));
        if let Some(replaced) = &h.replaced {
            out.push_str(&format!(
                "    NOTE: your --install-hook replaces the declared command: {}\n",
                strip_ansi(replaced)
            ));
        }
        if h.accepted {
            out.push_str("    (accepted by --accept-hooks)\n");
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn planned(item: Option<&str>, accepted: bool) -> PlannedHook {
        PlannedHook {
            key: HookKey {
                source: "local/x/src".into(),
                item: item.map(str::to_string),
                event: "install".into(),
                command: "bash ./bin/build.sh".into(),
            },
            label: "build".into(),
            optional: false,
            commit: "abc1234".into(),
            run_dir: "/store/skill/s".into(),
            inspect: item.map(|_| "/clone/skills/s".to_string()),
            browse_url: None,
            replaced: None,
            accepted,
        }
    }

    #[test]
    fn a_granted_answer_covers_exactly_the_listed_keys() {
        // spec: HOOK-185
        reset();
        let listed = HookKey::source_hook("s", "install", "make");
        let other = HookKey::source_hook("s", "install", "make all");
        grant([&listed], Answer::RunAll);
        assert_eq!(answer_for(&listed), Some(Answer::RunAll));
        assert_eq!(answer_for(&other), None, "an unlisted hook has no answer");
        grant([&listed], Answer::SkipAll);
        assert_eq!(answer_for(&listed), Some(Answer::SkipAll), "later wins");
        reset();
        assert_eq!(answer_for(&listed), None);
    }

    #[test]
    fn answers_last_until_the_outermost_scope_ends() {
        // spec: HOOK-181
        reset();
        let key = HookKey::source_hook("s", "install", "make");
        {
            let _outer = scope();
            grant([&key], Answer::RunAll);
            {
                let _inner = scope();
            }
            assert_eq!(answer_for(&key), Some(Answer::RunAll), "inner end keeps it");
        }
        assert_eq!(answer_for(&key), None, "outer end clears it");
    }

    #[test]
    fn an_item_hook_shows_where_it_runs_and_where_to_inspect_it() {
        // spec: HOOK-180 HOOK-184
        let text = render(&[planned(Some("skill:s"), false)]);
        assert!(text.contains("Runs in: /store/skill/s"), "{text}");
        assert!(
            text.contains("Inspect: /clone/skills/s (the source copy, before token expansion)"),
            "{text}"
        );
        assert!(text.contains("Item:    skill:s"), "{text}");
        assert!(text.contains("Event:   install (required)"), "{text}");
        assert!(text.contains("Command: bash ./bin/build.sh"), "{text}");
    }

    #[test]
    fn a_source_hook_has_no_inspect_line_and_an_accepted_hook_says_so() {
        // spec: HOOK-180 HOOK-184
        let text = render(&[planned(None, true)]);
        assert!(!text.contains("Inspect:"), "{text}");
        assert!(text.contains("(accepted by --accept-hooks)"), "{text}");
    }

    #[test]
    fn source_derived_fields_are_sanitized() {
        // spec: HOOK-91
        let mut h = planned(None, false);
        h.key.command = "echo hi\x1b[2K\x1b[1Arm -rf ~".into();
        h.label = "\u{202e}evil".into();
        let text = render(&[h]);
        assert!(!text.contains('\x1b'), "{text:?}");
        assert!(!text.contains('\u{202e}'), "{text:?}");
    }
}
