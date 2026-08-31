//! Source namespacing: prefixing every item from a source, and rewriting the
//! intra-source references that prefixing would otherwise break.
//!
//! A source's *effective prefix* is the consumer's `--namespace` alias if set, else the
//! `[source].prefix` declared in its `mind.toml`, else none. When a prefix `p`
//! is in effect, item `name` installs as `p:name` (identity, symlink, ref).
//!
//! References between items in the same source must be written as `{{ns:name}}`
//! tokens so they survive prefixing. [`expand`] rewrites each token to the
//! effective name (`name` when unprefixed, `p:name` when prefixed) and validates
//! that the referent is a real sibling. Sources that instead reference siblings
//! in bare prose can be detected with [`unguarded_refs`].

use std::collections::HashSet;

/// The file extensions (case-insensitive) [`is_markdown`] recognizes as
/// markdown.
const MARKDOWN_EXTENSIONS: &[&str] = &["md", "markdown", "mdown", "mkd"];

/// Whether `path` is a markdown file, by a case-insensitive extension match
/// against [`MARKDOWN_EXTENSIONS`].
///
/// The single chokepoint for "does a token expand here" (NS-53): all four
/// token families (`{{ns:}}`, `{{path:}}`, `{{tools:}}`, `{{self}}`) expand only
/// in a file this returns `true` for. `install.rs` skips any other file before
/// expansion; `review --fix` reports but never rewrites one (NS-54). Every
/// caller that needs this question answered goes through here rather than
/// repeating an extension check.
pub fn is_markdown(path: &std::path::Path) -> bool {
    path.extension().and_then(|e| e.to_str()).is_some_and(|e| {
        MARKDOWN_EXTENSIONS
            .iter()
            .any(|md| e.eq_ignore_ascii_case(md))
    })
}

/// Render `{{ns:name}}` tokens in `text` as their bare `name`, for a display
/// surface (`recall`, `probe`, `dump`) that shows raw source text -- a
/// frontmatter `description:` -- rather than an item's expanded store copy
/// (NS-56). This lets `templatize` wrap a sibling mention in a description
/// (NS-56) without the wrapped token leaking into a human-facing listing. Any
/// other `{{...}}` token is left as written; an unterminated token (no closing
/// `}}`) is left verbatim, mirroring [`expand`].
pub fn flatten_display(text: &str) -> String {
    const OPEN: &str = "{{ns:";
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(pos) = rest.find(OPEN) {
        out.push_str(&rest[..pos]);
        let after = &rest[pos + OPEN.len()..];
        let Some(end) = after.find("}}") else {
            out.push_str(&rest[pos..]);
            return out;
        };
        let name = after[..end].trim();
        out.push_str(name);
        rest = &after[end + 2..];
    }
    out.push_str(rest);
    out
}

/// Apply an effective prefix to a bare item name. An empty prefix is treated as
/// no prefix (the "no prefix" override; see [`prefix_choice`]).
pub fn apply(bare: &str, prefix: &Option<String>) -> String {
    match prefix {
        Some(p) if !p.is_empty() => format!("{p}:{bare}"),
        _ => bare.to_string(),
    }
}

/// Whether `c` is part of an item-name word (alphanumerics plus `-`/`_`), used
/// for whole-word matching when templating bare references.
fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '-' || c == '_'
}

/// Rewrite bare whole-word sibling mentions in `content` into `{{ns:name}}`
/// tokens, returning the new content and the number of replacements. Wrapping is
/// confined to prose (NS-24): text already inside a `{{...}}` brace span, a code
/// block, an inline code span, link syntax (a destination, a title, a reference
/// label; NS-52), the frontmatter `name:` field, or a path-adjacent position is
/// left untouched, so a keyword, a path component, a link, or the item's own
/// identity is never wrapped. A sibling mention in any other frontmatter field
/// (e.g. `description:`) is ordinary prose and is wrapped (NS-56): the
/// `description:` value is what `recall`/`probe`/`dump` display, so a display
/// surface flattens the token back to a bare name for that listing
/// ([`flatten_display`]) while the installed copy still expands it.
/// Still heuristic in prose (a sibling name can be an ordinary word), so callers
/// (init-source) keep it opt-in and reviewable, and apply it only to markdown.
///
/// Reads the document structure once (NS-46/NS-47) through the same
/// [`Structure`] map [`scan_ns_refs`] reads, so wrapping and un-wrapping cannot
/// disagree about what is code, and walks the document as a whole rather than
/// line by line, so a brace span that crosses a line break is copied verbatim
/// instead of being wrapped into (NS-51).
pub fn templatize(content: &str, siblings: &HashSet<String>) -> (String, usize) {
    let doc = Structure::new(content);
    let mut out = String::with_capacity(content.len());
    let mut count = 0usize;
    // Byte offset where the word being accumulated started, if any.
    let mut word: Option<usize> = None;
    // The last non-word character emitted, for the path-adjacency test.
    let mut before: Option<char> = None;
    let mut i = 0usize;
    while i < content.len() {
        let rest = &content[i..];
        // An existing `{{...}}` span is copied verbatim: wrapping never rewrites
        // inside a token, so it can neither nest one nor split one (NS-51).
        if rest.starts_with("{{") {
            let Some(close) = rest.find("}}") else {
                // Unterminated: the remainder is left verbatim, as `expand` does.
                count += emit_word(
                    content,
                    word.take(),
                    i,
                    &doc,
                    siblings,
                    before,
                    None,
                    &mut out,
                );
                out.push_str(rest);
                return (out, count);
            };
            count += emit_word(
                content,
                word.take(),
                i,
                &doc,
                siblings,
                before,
                None,
                &mut out,
            );
            out.push_str(&rest[..close + 2]);
            i += close + 2;
            before = Some('}');
            continue;
        }
        let c = rest.chars().next().expect("non-empty remainder");
        if is_word_char(c) {
            word.get_or_insert(i);
            i += c.len_utf8();
            continue;
        }
        count += emit_word(
            content,
            word.take(),
            i,
            &doc,
            siblings,
            before,
            Some(c),
            &mut out,
        );
        out.push(c);
        before = Some(c);
        i += c.len_utf8();
    }
    count += emit_word(
        content,
        word.take(),
        content.len(),
        &doc,
        siblings,
        before,
        None,
        &mut out,
    );
    (out, count)
}

/// Emit the word spanning `[start, end)` of `content`: wrapped as a `{{ns:}}`
/// token when it is a sibling name in a wrappable position, else verbatim.
/// Returns 1 if wrapped. See [`is_wrappable_mention`] for what counts.
#[allow(clippy::too_many_arguments)]
fn emit_word(
    content: &str,
    start: Option<usize>,
    end: usize,
    doc: &Structure,
    siblings: &HashSet<String>,
    before: Option<char>,
    after: Option<char>,
    out: &mut String,
) -> usize {
    let Some(start) = start else { return 0 };
    let word = &content[start..end];
    if is_wrappable_mention(doc, start, word, siblings, before, after) {
        out.push_str("{{ns:");
        out.push_str(word);
        out.push_str("}}");
        1
    } else {
        out.push_str(word);
        0
    }
}

/// Whether the word `word` starting at byte `start` (with `before`/`after` its
/// neighboring non-word characters, for the path-adjacency test) is a bare
/// sibling mention `templatize` may wrap into a token and [`unguarded_refs`]
/// must report (NS-24, NS-46, NS-47, NS-52, NS-55, NS-56): a real sibling name,
/// in prose or in a frontmatter field other than `name:`, not abutting a path
/// separator (`/` or `~`). The one predicate both read, so wrapping and
/// reporting cannot disagree about what counts.
fn is_wrappable_mention(
    doc: &Structure,
    start: usize,
    word: &str,
    siblings: &HashSet<String>,
    before: Option<char>,
    after: Option<char>,
) -> bool {
    let path_adj = matches!(before, Some('/') | Some('~')) || matches!(after, Some('/'));
    !path_adj
        && matches!(doc.at(start), Cell::Prose | Cell::FmDescription)
        && siblings.contains(word)
}

/// Interpret the user's answer to the meld prefix prompt for a source that
/// declares `[source].prefix` (CLI-24). Returns the alias to set on the source:
/// `None` keeps the declared prefix; `Some("")` is the explicit "no prefix"
/// override; `Some(other)` is a custom prefix. Empty / `y` / `yes` accept the
/// declared prefix, `n` / `no` / `none` drop it, and anything else is taken
/// verbatim (trimmed) as a custom prefix.
pub fn prefix_choice(answer: &str) -> Option<String> {
    let a = answer.trim();
    match a.to_ascii_lowercase().as_str() {
        "" | "y" | "yes" => None,
        "n" | "no" | "none" => Some(String::new()),
        _ => Some(a.to_string()),
    }
}

/// Extended reserved prefix words (NS-29): plausible future item-kind
/// or CLI-subsystem names that are banned pre-emptively. The list is permanent
/// and append-only, so `command` stays here even though it is now a real kind
/// word rejected a step earlier by `ItemKind::parse` (commands.md CMD-9).
const EXTRA_RESERVED: &[&str] = &[
    "command",
    "hook",
    "mcp",
    "plugin",
    "prompt",
    "mode",
    "output-style",
];

/// Return `true` when `prefix` passes the NS-28 path-safety requirement.
///
/// This is the low-level guard used by `validate_prefix` and by catalog code
/// that auto-generates a prefix from an untrusted name (e.g. a marketplace entry
/// name). Unlike `validate_prefix` it does **not** enforce the NS-25/NS-29
/// reserved-word lists: auto-generated prefixes originate from structured
/// content (not user input at a CLI/config ingress) and legitimate names like
/// `"plugin"` must remain usable.
///
/// A safe prefix must not be empty, `.`, or `..`; must not start with `~`; and
/// must not contain `/`, `\`, `:`, NUL, any ASCII control character (0x00-0x1F
/// or 0x7F), or a security-blocked Unicode code point (a bidi override, a
/// directional mark, a zero-width/invisible format character, or a member of
/// the Unicode tag block or the variation-selector block -- NS-72, broadened
/// by NS-73; see [`crate::sanitize::has_blocked_chars`]). The Path component
/// check is belt-and-suspenders: it rejects anything the scans would miss on
/// unusual platforms.
pub(crate) fn is_safe_prefix_component(prefix: &str) -> bool {
    if prefix.is_empty() || prefix == "." || prefix == ".." {
        return false;
    }
    if prefix.starts_with('~') {
        return false;
    }
    for b in prefix.bytes() {
        // Control characters and DEL.
        if b < 0x20 || b == 0x7f {
            return false;
        }
        // Path separators and the namespace colon separator.
        if b == b'/' || b == b'\\' || b == b':' || b == b'\0' {
            return false;
        }
    }
    // NS-72/NS-73: reject multi-byte control/bidi/zero-width/invisible code
    // points the byte scan above cannot see. The live path this changes
    // behavior on is `validate_prefix`: a user-supplied `meld --namespace`/`-N`
    // value or a repo's `[source].prefix` carrying one of these is refused with
    // `UnsafePrefix` rather than silently seeding a spoofing prefix onto every
    // namespaced ref. (An untrusted marketplace/catalog entry name is NOT a
    // live input to this function: `catalog.rs` already runs `strip_ansi` on
    // that name before it ever reaches here, so this guard is defense in depth
    // on that path, not what stands between a raw entry name and a prefix.)
    if crate::sanitize::has_blocked_chars(prefix) {
        return false;
    }
    // Belt-and-suspenders: exactly one Normal path component.
    let mut comps = std::path::Path::new(prefix).components();
    matches!(comps.next(), Some(std::path::Component::Normal(_))) && comps.next().is_none()
}

/// Validate that `prefix` is safe to use as a namespace prefix (NS-25, NS-28, NS-29).
///
/// Rejects any prefix that:
/// - is a reserved item-kind word (`skill`, `agent`, `rule`, `command`, `tool`;
///   NS-25), or
/// - is in the extended reserved list (NS-29), or
/// - is not a single safe path component (NS-28).
///
/// An empty prefix is always accepted -- it means "no prefix in effect" and is
/// handled by [`apply`].
///
/// This is the single chokepoint: every code path that accepts a user-supplied
/// prefix (`meld --namespace`, `[source].prefix`, config) must call this before
/// persisting the value.
pub fn validate_prefix(prefix: &str) -> crate::error::Result<()> {
    if prefix.is_empty() {
        return Ok(());
    }
    // NS-28: must be a single safe path component.
    if !is_safe_prefix_component(prefix) {
        return Err(crate::error::MindError::UnsafePrefix {
            prefix: prefix.to_string(),
        });
    }
    // NS-25: reject reserved item-kind words.
    if crate::error::ItemKind::parse(prefix).is_some() {
        return Err(crate::error::MindError::ReservedPrefix {
            prefix: prefix.to_string(),
        });
    }
    // NS-29: reject extended reserved words.
    if EXTRA_RESERVED.contains(&prefix) {
        return Err(crate::error::MindError::ReservedPrefix {
            prefix: prefix.to_string(),
        });
    }
    Ok(())
}

/// Expand every `{{ns:name}}` token in `content` to its effective name.
///
/// Returns `Err(name)` if a token names something that is not a sibling, so the
/// caller can report the typo. Sources with no tokens pass through unchanged.
///
/// `bare_names` is the set of sibling bare names that must expand to their bare
/// name even when a prefix is in effect (NS-42: agent referents, unless they are
/// also shadowed by a non-agent sibling of the same name). A name in
/// `bare_names` is still validated against `siblings` as normal; only the output
/// form is bare rather than `<prefix>:<name>`.
pub fn expand(
    content: &str,
    prefix: &Option<String>,
    siblings: &HashSet<String>,
    bare_names: &HashSet<String>,
) -> Result<String, String> {
    const OPEN: &str = "{{ns:";
    let mut out = String::with_capacity(content.len());
    let mut rest = content;
    while let Some(pos) = rest.find(OPEN) {
        out.push_str(&rest[..pos]);
        let after = &rest[pos + OPEN.len()..];
        let Some(end) = after.find("}}") else {
            // Unterminated token: leave the rest verbatim.
            out.push_str(&rest[pos..]);
            return Ok(out);
        };
        let name = after[..end].trim();
        if !siblings.contains(name) {
            return Err(name.to_string());
        }
        // spec: NS-42 -- agent referents expand bare; skill/rule/tool referents
        // expand with the prefix as before.
        if bare_names.contains(name) {
            out.push_str(name);
        } else {
            out.push_str(&apply(name, prefix));
        }
        rest = &after[end + 2..];
    }
    out.push_str(rest);
    Ok(out)
}

/// A sibling item the path-token expander can resolve a store path for.
#[derive(Debug, Clone)]
pub struct PathSibling {
    pub kind: crate::error::ItemKind,
    /// Bare name as it appears in the source.
    pub name: String,
    /// Entrypoint relative to the item dir, used by `{{tools:name}}`. Only tools
    /// carry one; `None` when the item is not a tool or declares no entrypoint.
    pub bin: Option<String>,
}

/// Everything [`expand_paths`] needs to turn a path token into a store path.
pub struct PathCtx<'a> {
    /// `~/.mind/store` (honors `MIND_HOME`).
    pub store_root: &'a std::path::Path,
    /// The user's home directory. When the store lies under it, a store path is
    /// rendered with a leading `~` (TOOL-16); `None` renders the absolute path.
    pub home: Option<&'a std::path::Path>,
    /// The source's effective prefix, applied to every referent's effective name.
    pub prefix: &'a Option<String>,
    /// The installing item's own kind and bare name (for `{{self}}`).
    pub self_kind: crate::error::ItemKind,
    pub self_name: &'a str,
    /// Every item in the same source (including self), for sibling lookups.
    pub siblings: &'a [PathSibling],
}

impl PathCtx<'_> {
    /// The store directory of an item of `kind` with bare name `bare`, rendered
    /// with a leading `~` when it lies under `home` (TOOL-16).
    fn store_path(&self, kind: crate::error::ItemKind, bare: &str) -> String {
        let abs = self
            .store_root
            .join(kind.as_str())
            .join(apply(bare, self.prefix));
        render_under_home(&abs, self.home)
    }
}

/// Render `path` with a leading `~` when it lies under `home`, else as the path
/// itself. This keeps a store-path token matchable by a Claude `settings.json`
/// permission glob that uses tilde syntax (`Bash(~/.mind/store/**)`), which an
/// absolute path would not match (TOOL-16).
fn render_under_home(path: &std::path::Path, home: Option<&std::path::Path>) -> String {
    if let Some(home) = home
        && let Ok(rest) = path.strip_prefix(home)
    {
        return std::path::Path::new("~")
            .join(rest)
            .to_string_lossy()
            .into_owned();
    }
    path.to_string_lossy().into_owned()
}

/// Outcome of resolving one `{{...}}` token's inner text.
enum Token {
    /// A path token that resolved to this store path.
    Path(String),
    /// Not a path token (e.g. `{{ns:...}}` or a stray `{{`): leave it verbatim.
    Passthrough,
    /// A path token whose referent does not resolve, tagged with why (miss vs. a
    /// real tool with no entrypoint) so the caller can report the specific cause
    /// (TOOL-17).
    Bad(crate::error::BadRefReason),
}

/// Expand the path tokens `{{self}}`, `{{tools:name}}`, and `{{path:ref}}` in
/// `content` to absolute store paths.
///
/// `{{ns:...}}` tokens are left untouched (handled by [`expand`]); any other
/// `{{...}}` span is passed through verbatim. Returns `Err(token)` with the
/// offending token text when a path token's referent does not resolve, so the
/// caller can report it. Whitespace inside a token is trimmed; an unterminated
/// token (no closing `}}`) leaves the remainder verbatim, mirroring [`expand`].
pub fn expand_paths(
    content: &str,
    ctx: &PathCtx,
) -> Result<String, (String, crate::error::BadRefReason)> {
    let mut out = String::with_capacity(content.len());
    let mut rest = content;
    while let Some(pos) = rest.find("{{") {
        out.push_str(&rest[..pos]);
        let after = &rest[pos + 2..];
        let Some(end) = after.find("}}") else {
            // Unterminated token: leave the rest verbatim.
            out.push_str(&rest[pos..]);
            return Ok(out);
        };
        let inner = after[..end].trim();
        match resolve_token(inner, ctx) {
            Token::Path(p) => {
                out.push_str(&p);
                rest = &after[end + 2..];
            }
            Token::Bad(reason) => {
                // Report the token exactly as written, including the braces, with
                // the specific reason it failed (TOOL-17).
                return Err((rest[pos..pos + 2 + end + 2].to_string(), reason));
            }
            Token::Passthrough => {
                // Leave the `{{` verbatim and resume scanning just after it, so a
                // following `{{ns:}}` or another path token is still seen.
                out.push_str("{{");
                rest = after;
            }
        }
    }
    out.push_str(rest);
    Ok(out)
}

/// Resolve one token's trimmed inner text to a store path.
fn resolve_token(inner: &str, ctx: &PathCtx) -> Token {
    if inner == "self" {
        return Token::Path(ctx.store_path(ctx.self_kind, ctx.self_name));
    }
    if let Some(name) = inner.strip_prefix("tools:") {
        let name = name.trim();
        return match ctx
            .siblings
            .iter()
            .find(|s| s.kind == crate::error::ItemKind::Tool && s.name == name)
        {
            Some(tool) => match &tool.bin {
                Some(bin) => Token::Path(
                    std::path::Path::new(&ctx.store_path(crate::error::ItemKind::Tool, name))
                        .join(bin)
                        .to_string_lossy()
                        .into_owned(),
                ),
                // The tool exists but has no resolvable entrypoint: a distinct
                // cause from a plain miss (TOOL-17).
                None => Token::Bad(crate::error::BadRefReason::ToolNoBin),
            },
            None => Token::Bad(crate::error::BadRefReason::NoMatch),
        };
    }
    if let Some(reference) = inner.strip_prefix("path:") {
        let reference = reference.trim();
        let (want_kind, name) = match reference.split_once(':') {
            Some((k, n)) => match crate::error::ItemKind::parse(k) {
                Some(kind) => (Some(kind), n.trim()),
                None => return Token::Bad(crate::error::BadRefReason::NoMatch),
            },
            None => (None, reference),
        };
        let mut hits = ctx
            .siblings
            .iter()
            .filter(|s| s.name == name && want_kind.is_none_or(|k| s.kind == k));
        return match (hits.next(), hits.next()) {
            (Some(s), None) => Token::Path(ctx.store_path(s.kind, name)),
            // Two matches (only possible for a bare `{{path:name}}` with no
            // qualifier) is a distinct cause from a plain miss (TOOL-18).
            (Some(_), Some(_)) => Token::Bad(crate::error::BadRefReason::AmbiguousKind),
            // No match at all.
            (None, _) => Token::Bad(crate::error::BadRefReason::NoMatch),
        };
    }
    Token::Passthrough
}

/// The home-root spellings a hardcoded reference can start with. A reference is
/// only a hardcoded install path once one of the three install layouts
/// (`.mind/store/`, `.claude/`, `.agents/`) follows the home root, checked in
/// [`canonical_install_path`]. `~/` covers the literal tilde, `$HOME/` /
/// `${HOME}/` the env-var spellings, and `/home/` / `/Users/` an absolute home.
const HOME_MARKERS: [&str; 5] = ["~/", "$HOME/", "${HOME}/", "/home/", "/Users/"];

/// What a hardcoded install path resolves to at runtime, which sets the
/// advisory's severity wording (CLI-145).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HardcodedKind {
    /// The item's own resources (`{{self}}`). Resolves through the symlink mind
    /// links into each agent home, so it works until a prefix renames the item
    /// or a second home is configured.
    OwnResource,
    /// A sibling `tool`. A tool is store-only and never linked into an agent
    /// home (TOOL-3), so a hardcoded reference to it does not resolve.
    SharedTool,
    /// Any other recognized install path (a sibling item, or a foreign/unparsed
    /// name): reached by a token, not by a literal install path.
    OtherItem,
}

/// One hardcoded install-path occurrence found in an item's text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HardcodedPath {
    /// The offending path substring as written.
    pub matched: String,
    /// The token that should replace it, when it maps confidently; else `None`
    /// (the path is still flagged, just without a concrete suggestion).
    pub suggestion: Option<String>,
    /// What the path resolves to, for the advisory's wording (CLI-145).
    pub kind: HardcodedKind,
}

/// Reduce a hardcoded path to its canonical `~/<layout>/...` form, or `None` when
/// it is not a mind install path. Accepts the home root written as `~`, `$HOME`,
/// `${HOME}`, or an absolute `/home/<user>` / `/Users/<user>` path, and requires
/// one of the install layouts (`.mind/store/`, `.claude/`, `.agents/`) to follow.
fn canonical_install_path(path: &str) -> Option<String> {
    let rest = if let Some(r) = path.strip_prefix("~/") {
        r
    } else if let Some(r) = path.strip_prefix("$HOME/") {
        r
    } else if let Some(r) = path.strip_prefix("${HOME}/") {
        r
    } else {
        let r = path
            .strip_prefix("/home/")
            .or_else(|| path.strip_prefix("/Users/"))?;
        // Drop the `<user>` segment of an absolute home path.
        r.split_once('/').map(|(_user, rest)| rest)?
    };
    if rest.starts_with(".mind/store/")
        || rest.starts_with(".claude/")
        || rest.starts_with(".agents/")
    {
        Some(format!("~/{rest}"))
    } else {
        None
    }
}

/// Whether `c` ends a path token in prose (so the scanner knows where the path
/// substring stops).
fn is_path_terminator(c: char) -> bool {
    c.is_whitespace()
        || matches!(
            c,
            '"' | '\'' | '`' | ')' | ']' | '}' | ',' | ';' | '<' | '>'
        )
}

/// Parse a hardcoded install path into `(kind, bare_name, rest)`, where `rest`
/// is the remainder after the item name (no leading slash). Recognizes the
/// `~/.mind/store/<kind>/...`, `~/.claude/<kinddir>/...`, and `~/.agents/<kinddir>/...`
/// layouts. Returns `None` for anything that does not name a kind + item.
fn parse_install_path(path: &str) -> Option<(crate::error::ItemKind, String, String)> {
    let after_kind = if let Some(rest) = path.strip_prefix("~/.mind/store/") {
        let mut it = rest.splitn(2, '/');
        let kind = crate::error::ItemKind::parse(it.next()?)?;
        (kind, it.next()?.to_string())
    } else {
        let rest = path
            .strip_prefix("~/.claude/")
            .or_else(|| path.strip_prefix("~/.agents/"))?;
        let mut it = rest.splitn(2, '/');
        let kind = crate::error::ItemKind::from_dir(it.next()?)?;
        (kind, it.next()?.to_string())
    };
    let (kind, tail) = after_kind;
    let mut seg = tail.splitn(2, '/');
    let first = seg.next()?;
    let rest = seg.next().unwrap_or("").to_string();
    // An agent/rule/command file is `<name>.md`; the store copies it as a bare
    // `<name>`, so stripping a `.md` suffix is correct for both layouts and a
    // no-op for the store form.
    let name = match kind {
        crate::error::ItemKind::Agent
        | crate::error::ItemKind::Rule
        | crate::error::ItemKind::Command => first.strip_suffix(".md").unwrap_or(first).to_string(),
        _ => first.to_string(),
    };
    if name.is_empty() {
        return None;
    }
    Some((kind, name, rest))
}

/// Join a token with a path remainder: `{{self}}` + `resources/x` -> `{{self}}/resources/x`.
fn join_token(token: &str, rest: &str) -> String {
    if rest.is_empty() {
        token.to_string()
    } else {
        format!("{token}/{rest}")
    }
}

/// The token that should replace a hardcoded `path`, or `None` when it does not
/// map confidently (a foreign name, an unrecognized layout like `~/.agents/resources/...`).
fn token_for_path(path: &str, ctx: &PathCtx) -> Option<String> {
    let (kind, name, rest) = parse_install_path(path)?;
    // The item's own directory -> {{self}}.
    if kind == ctx.self_kind && name == ctx.self_name {
        return Some(join_token("{{self}}", &rest));
    }
    // Otherwise it must name a real sibling of that kind.
    let sib = ctx
        .siblings
        .iter()
        .find(|s| s.kind == kind && s.name == name)?;
    if kind == crate::error::ItemKind::Tool {
        // A tool's entrypoint -> {{tools:name}}; anything else in the tool dir ->
        // {{path:tool:name}}/rest.
        if let Some(bin) = &sib.bin
            && rest == *bin
        {
            return Some(format!("{{{{tools:{name}}}}}"));
        }
        return Some(join_token(&format!("{{{{path:tool:{name}}}}}"), &rest));
    }
    Some(join_token(
        &format!("{{{{path:{}:{}}}}}", kind.as_str(), name),
        &rest,
    ))
}

/// Classify a canonical install path by what it resolves to (CLI-145), returning
/// the class and the token that should replace it (if it maps confidently).
fn classify_path(canonical: &str, ctx: &PathCtx) -> (HardcodedKind, Option<String>) {
    let suggestion = token_for_path(canonical, ctx);
    let kind = match parse_install_path(canonical) {
        Some((k, name, _)) => {
            if k == ctx.self_kind && name == ctx.self_name {
                HardcodedKind::OwnResource
            } else if k == crate::error::ItemKind::Tool
                && ctx.siblings.iter().any(|s| s.kind == k && s.name == name)
            {
                HardcodedKind::SharedTool
            } else {
                HardcodedKind::OtherItem
            }
        }
        None => HardcodedKind::OtherItem,
    };
    (kind, suggestion)
}

/// Find every hardcoded install path in `content`, in order, as
/// `(start, end, HardcodedPath)` byte spans. A candidate is a [`HOME_MARKERS`]
/// span that reduces to an install layout via [`canonical_install_path`]; other
/// uses of those markers (an ordinary `/home/<user>/projects/...` path) are
/// skipped.
fn scan_hardcoded(content: &str, ctx: &PathCtx) -> Vec<(usize, usize, HardcodedPath)> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < content.len() {
        let Some((start, marker)) = HOME_MARKERS
            .iter()
            .filter_map(|m| content[i..].find(m).map(|off| (i + off, *m)))
            .min_by_key(|(pos, _)| *pos)
        else {
            break;
        };
        // Scan for the path terminator from after the marker, so a `}` inside a
        // `${HOME}` spelling is not mistaken for the end of the path.
        let scan_from = start + marker.len();
        let mut end = content.len();
        for (idx, c) in content[scan_from..].char_indices() {
            if is_path_terminator(c) {
                end = scan_from + idx;
                break;
            }
        }
        let matched = content[start..end].to_string();
        if let Some(canonical) = canonical_install_path(&matched) {
            let (kind, suggestion) = classify_path(&canonical, ctx);
            out.push((
                start,
                end,
                HardcodedPath {
                    matched,
                    suggestion,
                    kind,
                },
            ));
        }
        i = end.max(start + 1);
    }
    out
}

/// Report every hardcoded install path in `content` that a path token should
/// replace (CLI-136). Read-only: suggests but does not rewrite.
pub fn detect_hardcoded_paths(content: &str, ctx: &PathCtx) -> Vec<HardcodedPath> {
    scan_hardcoded(content, ctx)
        .into_iter()
        .map(|(_, _, hp)| hp)
        .collect()
}

/// Rewrite the confidently-mapped hardcoded install paths in `content` into their
/// tokens (CLI-138). Paths with no confident mapping are left untouched. Returns
/// the new content and the number of rewrites.
pub fn rewrite_hardcoded_paths(content: &str, ctx: &PathCtx) -> (String, usize) {
    let mut out = String::with_capacity(content.len());
    let mut last = 0;
    let mut count = 0;
    for (start, end, hp) in scan_hardcoded(content, ctx) {
        if let Some(token) = hp.suggestion {
            out.push_str(&content[last..start]);
            out.push_str(&token);
            last = end;
            count += 1;
        }
    }
    out.push_str(&content[last..]);
    (out, count)
}

/// Find every well-formed `{{...}}` span in `content`, in document order, as
/// `(start, end, inner)`: `start`/`end` are the byte offsets of the whole span
/// (braces included), `inner` is the text between the braces with surrounding
/// whitespace trimmed. An unterminated span (no closing `}}`) stops the scan,
/// mirroring [`expand`]/[`scan_ns_refs`].
///
/// The one low-level `{{...}}` tokenizer this module has for "any token,
/// whatever family": [`strip_braced`] (masking a token so prose scanning skips
/// it) and [`inert_tokens`] (review's non-markdown-file check, CLI-223) both
/// read it, so they cannot disagree about what counts as a token span.
fn scan_braced(content: &str) -> Vec<(usize, usize, &str)> {
    let mut out = Vec::new();
    let mut pos = 0usize;
    while let Some(rel) = content[pos..].find("{{") {
        let start = pos + rel;
        let after_start = start + 2;
        let Some(rel_end) = content[after_start..].find("}}") else {
            break;
        };
        let inner_end = after_start + rel_end;
        let end = inner_end + 2;
        out.push((start, end, content[after_start..inner_end].trim()));
        pos = end;
    }
    out
}

/// Replace every `{{...}}` span with a space, so prose scanning ignores anything
/// already inside a reference token (any token kind, not just `{{ns:}}`).
///
/// An unterminated `{{` (no closing `}}` anywhere after it) is not a span
/// [`scan_braced`] reports, so it -- and everything from it to the end of
/// `content` -- is left verbatim rather than masked, mirroring [`expand`]'s
/// "leave the rest verbatim" treatment of an unterminated token.
fn strip_braced(content: &str) -> String {
    let mut out = String::with_capacity(content.len());
    let mut last = 0;
    for (start, end, _) in scan_braced(content) {
        out.push_str(&content[last..start]);
        out.push(' ');
        last = end;
    }
    out.push_str(&content[last..]);
    out
}

/// Every `{{...}}` token found in `content`, as written (braces included),
/// deduplicated in first-seen order.
///
/// Used by `review` to flag any token in a non-markdown item file (NS-53): no
/// token family expands outside markdown, so even a token that would resolve
/// if the file were markdown -- e.g. `{{tools:detect}}` in a bundled `.sh` --
/// is left literal at install and is dead/inert text, not a working reference
/// (CLI-223). This closes the gap [`expand_paths`]'s resolution check leaves:
/// that one only reports a token here when it does NOT resolve (still
/// advisory, never hard, since it can never break an install either way); this
/// one reports the token regardless of whether it resolves, since neither case
/// ever actually expands outside markdown.
///
/// For a nested construct (`{{ {{tools:detect}} }}`) [`scan_braced`] reports
/// the outer span, so the string returned here is a superset of the inner
/// token rather than the inner token alone -- cosmetic (the advisory message
/// still names a span that covers the real token; the never-miss invariant in
/// the tests below holds), not a correctness issue, so left as-is.
pub fn inert_tokens(content: &str) -> Vec<String> {
    // O(n) dedup via a seen-set (a hostile source can pack a non-markdown file
    // with an attacker-controlled number of distinct tokens; `review` must stay
    // linear in that count rather than the O(k^2) an `out.iter().any(...)` scan
    // would cost per token). `out` still preserves first-seen order, since a
    // `HashSet` alone would not.
    let mut seen: HashSet<&str> = HashSet::new();
    let mut out: Vec<String> = Vec::new();
    for (start, end, _) in scan_braced(content) {
        let tok = &content[start..end];
        if seen.insert(tok) {
            out.push(tok.to_string());
        }
    }
    out
}

/// Find sibling TOOL names mentioned in `content`'s prose without a token
/// (CLI-137). Unlike [`unguarded_refs`], this is prefix-independent: a tool is
/// reached by a path token, never by name, so a bare tool name is always suspect.
pub fn bare_tool_refs(content: &str, siblings: &[PathSibling]) -> Vec<String> {
    let stripped = strip_braced(content);
    let mut found: Vec<String> = siblings
        .iter()
        .filter(|s| s.kind == crate::error::ItemKind::Tool)
        .map(|s| s.name.clone())
        .filter(|name| whole_word_present(&stripped, name))
        .collect();
    found.sort();
    found.dedup();
    found
}

/// Extract the bare name of every `{{ns:name}}` token in `content`.
///
/// Mirrors [`expand`]'s inline parser: the open delimiter is `{{ns:`, the name
/// is the text up to the next `}}` with surrounding whitespace trimmed, and an
/// unterminated token (no closing `}}`) stops the scan and is not a reference
/// (NS-15). Names are returned in first-seen order, de-duplicated. These are the
/// intra-source dependency edges (DEP-1). Called by [`crate::deps::resolve`].
pub fn referenced_names(content: &str) -> Vec<String> {
    const OPEN: &str = "{{ns:";
    let mut names: Vec<String> = Vec::new();
    let mut rest = content;
    while let Some(pos) = rest.find(OPEN) {
        let after = &rest[pos + OPEN.len()..];
        let Some(end) = after.find("}}") else {
            // Unterminated token: stop, exactly like `expand` does.
            break;
        };
        let name = after[..end].trim();
        if !name.is_empty() && !names.iter().any(|n| n == name) {
            names.push(name.to_string());
        }
        rest = &after[end + 2..];
    }
    names
}

/// One token in an item's text that names a SIBLING item.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SiblingRef {
    /// The token exactly as written, braces included, e.g. `{{ns:dev}}`.
    pub token: String,
    /// The referent's bare name (may be empty for a malformed token).
    pub name: String,
    /// The kind the token narrows to: `Tool` for `{{tools:}}`, the named kind
    /// for a qualified `{{path:kind:name}}`, `None` when the token matches any
    /// kind (`{{ns:}}`, bare `{{path:name}}`).
    pub kind: Option<crate::error::ItemKind>,
}

/// Every token in `content` that resolves against a SIBLING: `{{ns:name}}`
/// (NS-10), `{{tools:name}}` (TOOL-15), and `{{path:[kind:]name}}` (TOOL-18).
///
/// This is the union of what [`expand`] and [`expand_paths`] resolve against the
/// sibling set, so a caller can decide up front whether a given catalog could
/// satisfy an item's references at all (LNK-18). Each family's prefix test
/// mirrors the expander that actually recognizes it: the `ns:` test is against
/// the UNTRIMMED text right after `{{`, matching [`expand`]'s literal, no-space
/// `{{ns:` scan (so `{{ ns:name }}` -- a space before `ns:` -- is not a token
/// here either, since it never expands there); the `tools:`/`path:`/`self`
/// tests are against the trimmed inner text, matching [`expand_paths`], which
/// trims the whole span before testing (so a space there IS still a token, on
/// both sides). `{{self}}` is excluded: it names the item itself and always
/// resolves. Any other `{{...}}` span is not a reference and is skipped, and an
/// unterminated token stops the scan, matching both expanders. Tokens are
/// returned in first-seen order, de-duplicated by token text.
///
/// A malformed token whose name is empty is still returned: [`expand`] rejects
/// an empty referent as a non-sibling, so reporting it here keeps the two in
/// step rather than letting it fall through to a blunter error.
pub fn sibling_reference_tokens(content: &str) -> Vec<SiblingRef> {
    let mut out: Vec<SiblingRef> = Vec::new();
    let mut rest = content;
    while let Some(pos) = rest.find("{{") {
        let after = &rest[pos + 2..];
        let Some(end) = after.find("}}") else {
            // Unterminated token: stop, exactly like both expanders do.
            break;
        };
        // `raw` is the span between the braces, untrimmed; `inner` is its trimmed
        // form. The `ns:` prefix test uses `raw` (matching `expand`'s literal
        // `{{ns:` scan, which tolerates no whitespace before `ns:`); the other
        // arms use `inner` (matching `expand_paths`, which trims the whole span
        // first).
        let raw = &after[..end];
        let inner = raw.trim();
        let token = rest[pos..pos + 2 + end + 2].to_string();
        let parsed: Option<(String, Option<crate::error::ItemKind>)> = if inner == "self" {
            None
        } else if let Some(name) = raw.strip_prefix("ns:") {
            Some((name.trim().to_string(), None))
        } else if let Some(name) = inner.strip_prefix("tools:") {
            Some((name.trim().to_string(), Some(crate::error::ItemKind::Tool)))
        } else if let Some(reference) = inner.strip_prefix("path:") {
            let reference = reference.trim();
            // `{{path:kind:name}}` narrows by kind; a `kind` that does not parse
            // means `resolve_token` fails immediately with `Bad(NoMatch)` (see
            // namespace.rs:525) rather than falling back to treating the whole
            // reference as a plain name. This scanner instead reports the whole
            // reference text as the referent name: it will not match any real
            // sibling either (a colon is not a legal item name), so both paths
            // agree the token cannot resolve -- only the internal rationale for
            // "why not" differs, which is cosmetic for this scanner's purpose.
            match reference.split_once(':') {
                Some((k, n)) => match crate::error::ItemKind::parse(k) {
                    Some(kind) => Some((n.trim().to_string(), Some(kind))),
                    None => Some((reference.to_string(), None)),
                },
                None => Some((reference.to_string(), None)),
            }
        } else {
            None
        };
        match parsed {
            Some((name, kind)) => {
                if !out.iter().any(|r| r.token == token) {
                    out.push(SiblingRef { token, name, kind });
                }
                rest = &after[end + 2..];
            }
            // Not a reference: resume just past the `{{`, not past the span, so
            // a token opening inside it is still seen -- the same passthrough
            // rule `expand_paths` follows.
            None => rest = after,
        }
    }
    out
}

/// Find sibling names referenced in bare prose (outside any `{{...}}` token).
///
/// Heuristic and advisory: used to warn when a source is about to be prefixed
/// but references siblings without the token that would keep them resolvable.
/// A sibling name that already appears inside any token kind (`{{ns:}}`,
/// `{{tools:}}`, `{{path:}}`, `{{self}}`) is correctly guarded and is NOT
/// reported; only names in genuinely bare prose are flagged.
///
/// Structure-aware (NS-55): reads the same [`Structure`] map `templatize` and
/// `scan_ns_refs` read, through the same [`is_wrappable_mention`] predicate
/// `templatize` wraps with, so a mention inside a code span, a fenced or
/// indented code block, link syntax, or a path-adjacent position is never
/// reported (it was never a real reference there), while a mention in prose or
/// in a frontmatter field other than `name:` is (NS-56) -- exactly the set
/// `templatize` can wrap, so `--fix` clears every mention this reports instead
/// of leaving some of them permanently unclearable (NS-48).
///
/// `content` need not be markdown (a script, data): [`Structure`] still reads
/// it as a CommonMark document, which is a heuristic parse rather than a
/// meaningful one for such a file, but it only ever suppresses a report (an
/// indented line reads as code), never adds a false one, so applying it to any
/// text file moves in the right direction rather than the wrong one.
pub fn unguarded_refs(content: &str, siblings: &HashSet<String>) -> Vec<String> {
    let doc = Structure::new(content);
    let mut found: Vec<String> = Vec::new();
    let mut word: Option<usize> = None;
    let mut before: Option<char> = None;
    let mut i = 0usize;
    while i < content.len() {
        let rest = &content[i..];
        // An existing `{{...}}` span is not a bare mention: skip it whole,
        // exactly as `templatize` does, so a name already inside any token kind
        // is never reported (it is correctly guarded, whatever the token).
        if rest.starts_with("{{") {
            note_mention(
                content,
                word.take(),
                i,
                &doc,
                siblings,
                before,
                None,
                &mut found,
            );
            let Some(close) = rest.find("}}") else {
                break;
            };
            i += close + 2;
            before = Some('}');
            continue;
        }
        let c = rest.chars().next().expect("non-empty remainder");
        if is_word_char(c) {
            word.get_or_insert(i);
            i += c.len_utf8();
            continue;
        }
        note_mention(
            content,
            word.take(),
            i,
            &doc,
            siblings,
            before,
            Some(c),
            &mut found,
        );
        before = Some(c);
        i += c.len_utf8();
    }
    note_mention(
        content,
        word.take(),
        content.len(),
        &doc,
        siblings,
        before,
        None,
        &mut found,
    );
    found.sort();
    found.dedup();
    found
}

/// Record the word spanning `[start, end)` of `content` when it is a bare
/// sibling mention ([`is_wrappable_mention`]).
#[allow(clippy::too_many_arguments)]
fn note_mention(
    content: &str,
    start: Option<usize>,
    end: usize,
    doc: &Structure,
    siblings: &HashSet<String>,
    before: Option<char>,
    after: Option<char>,
    found: &mut Vec<String>,
) {
    let Some(start) = start else { return };
    let word = &content[start..end];
    if is_wrappable_mention(doc, start, word, siblings, before, after) {
        found.push(word.to_string());
    }
}

fn whole_word_present(haystack: &str, needle: &str) -> bool {
    if needle.is_empty() {
        return false;
    }
    let mut start = 0;
    while let Some(i) = haystack[start..].find(needle) {
        let idx = start + i;
        let before = haystack[..idx].chars().next_back();
        let after = haystack[idx + needle.len()..].chars().next();
        if !before.is_some_and(is_word) && !after.is_some_and(is_word) {
            return true;
        }
        start = idx + needle.len();
    }
    false
}

fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == '-'
}

/// The structural context a `{{ns:}}` token sits in, for flagging misplaced ones.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NsContext {
    /// Natural-language prose: the only place a name reference belongs.
    Prose,
    /// Inside a fenced ```` ``` ```` code block.
    CodeBlock,
    /// Inside an inline `code span`.
    CodeSpan,
    /// Abutting a path separator (`/` or `~`).
    Path,
    /// The frontmatter `name:` field (an item namespacing its own name).
    FrontmatterName,
}

impl NsContext {
    /// Whether a name token here is misplaced (anything but prose; NS-24).
    pub fn is_misplaced(self) -> bool {
        !matches!(self, NsContext::Prose)
    }
}

/// One `{{ns:name}}` token found in `content`, with its context and byte span.
#[derive(Debug, Clone)]
pub struct NsRef {
    pub name: String,
    pub context: NsContext,
    pub start: usize,
    pub end: usize,
}

/// What one byte of a markdown document is, structurally (NS-46, NS-47, NS-49).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Cell {
    /// Natural-language prose: the only place a name reference belongs, and the
    /// only place wrapping may create one.
    Prose,
    /// Content of a code block, fenced or indented, wherever it sits (top level,
    /// inside a list item, inside a blockquote).
    Code,
    /// Inside an inline code span.
    Span,
    /// Link syntax: everything a link or an image is made of except its visible
    /// text -- the destination, the title, the reference label, and the whole of
    /// a link reference definition (NS-52). Rewriting any of it edits markdown
    /// syntax rather than prose, so wrapping never touches it.
    LinkSyntax,
    /// The frontmatter `name:` field.
    FmName,
    /// The *value* of the frontmatter `description:` field: the one frontmatter
    /// field that is free prose rather than machine-read structure, so the one
    /// wrapping may rewrite (NS-56).
    FmDescription,
    /// Any other frontmatter content: another field's key or value, the
    /// `description:` key itself, or a line with no `key:` at all. A token here
    /// is a reference (prose), but the field is structure mind or the harness
    /// parses out of the *source* file -- `requires:` is a list of item refs
    /// (DEP-4), `build:`/`install:`/`bin:` are shell commands (TOOL-6..) --
    /// and nothing ever expands a token there, so wrapping must never create
    /// one (NS-56).
    FmOther,
    /// Structure carrying no scannable content: a frontmatter delimiter, a code
    /// fence delimiter (info string included), or a container marker prefixing
    /// code-block content. A token here is not a reference and is not reported.
    Skip,
}

/// The structure of a markdown document as a byte map: what every byte of it is
/// (NS-46, NS-47, NS-49, NS-50, NS-52).
///
/// Derived from one CommonMark parse of the document (`pulldown-cmark`), which
/// is what makes this a lookup rather than a re-derivation of markdown's block
/// and inline rules: a code block's own event range is structure, the text
/// events inside it are code, and every inline-code event is a span. A link's
/// own event range is syntax and the text events inside it are its visible
/// prose (NS-52). Everything the parse claims as neither code nor link syntax is
/// prose. The leading `--- ... ---` frontmatter block is not CommonMark, so it
/// is marked by a pre-pass and the parse runs over the body after it (NS-47).
///
/// [`scan_ns_refs`] and [`templatize`] both read this one map, so un-wrapping
/// and wrapping cannot disagree about what is code (NS-51).
struct Structure {
    /// One entry per byte of the document, in order.
    cells: Vec<Cell>,
}

impl Structure {
    fn new(content: &str) -> Self {
        let mut cells = vec![Cell::Prose; content.len()];
        let body = mark_frontmatter(content, &mut cells);
        // Tables are the one non-CommonMark construct mind's own items rely on,
        // and each cell is its own inline run, so a stray backtick in one cell
        // cannot open a span that closes in the next row.
        let opts = pulldown_cmark::Options::ENABLE_TABLES;
        let parser = pulldown_cmark::Parser::new_ext(&content[body..], opts);
        // A link reference definition (`[label]: url "title"`) is resolved during
        // parsing and emits no event, so its span is read off the parser's own
        // definition table rather than from the event stream (NS-52).
        let refdefs: Vec<std::ops::Range<usize>> = parser
            .reference_definitions()
            .iter()
            .map(|(_, def)| (def.span.start + body)..(def.span.end + body))
            .collect();
        for range in refdefs {
            fill(&mut cells, range, Cell::LinkSyntax);
        }
        let mut depth = 0usize;
        // One entry per open link or image, `true` when that link's visible text
        // doubles as its label or its destination (NS-52).
        let mut links: Vec<bool> = Vec::new();
        for (event, range) in parser.into_offset_iter() {
            let range = (range.start + body)..(range.end + body);
            match event {
                // The whole block is structure to begin with (its delimiters and
                // any container markers); the text events inside it fill the
                // content back in below.
                pulldown_cmark::Event::Start(pulldown_cmark::Tag::CodeBlock(_)) => {
                    fill(&mut cells, range, Cell::Skip);
                    depth += 1;
                }
                pulldown_cmark::Event::End(pulldown_cmark::TagEnd::CodeBlock) => {
                    depth = depth.saturating_sub(1);
                }
                // The whole link is syntax to begin with -- brackets, label,
                // destination, title; the text events inside it fill the visible
                // text back in as prose below, unless that text is itself the
                // label or the destination (NS-52).
                pulldown_cmark::Event::Start(
                    pulldown_cmark::Tag::Link { link_type, .. }
                    | pulldown_cmark::Tag::Image { link_type, .. },
                ) => {
                    fill(&mut cells, range, Cell::LinkSyntax);
                    links.push(text_doubles_as_syntax(link_type));
                }
                pulldown_cmark::Event::End(
                    pulldown_cmark::TagEnd::Link | pulldown_cmark::TagEnd::Image,
                ) => {
                    links.pop();
                }
                pulldown_cmark::Event::Text(_) if depth > 0 => {
                    fill(&mut cells, range, Cell::Code);
                }
                pulldown_cmark::Event::Text(_)
                    if !links.is_empty() && links.iter().all(|opaque| !opaque) =>
                {
                    fill(&mut cells, range, Cell::Prose);
                }
                pulldown_cmark::Event::Code(_) => fill(&mut cells, range, Cell::Span),
                _ => {}
            }
        }
        Structure { cells }
    }

    /// What the byte at `pos` is. Past the end of the document (only reachable
    /// from an empty word) it is prose, which wraps nothing.
    fn at(&self, pos: usize) -> Cell {
        self.cells.get(pos).copied().unwrap_or(Cell::Prose)
    }

    /// The context of a `{{ns:}}` token spanning `[start, end)`, or `None` when
    /// the position carries no scannable content (a delimiter line).
    fn context(&self, content: &str, start: usize, end: usize) -> Option<NsContext> {
        Some(match self.at(start) {
            Cell::Skip => return None,
            Cell::FmName => NsContext::FrontmatterName,
            // A token in any other frontmatter field is an ordinary reference.
            Cell::FmDescription | Cell::FmOther => NsContext::Prose,
            Cell::Code => NsContext::CodeBlock,
            Cell::Span => NsContext::CodeSpan,
            // A destination, a label, or a title is a path or an identifier, not
            // prose: the same class of misplacement as a token beside a path
            // separator, and reported as one (NS-52).
            Cell::LinkSyntax => NsContext::Path,
            Cell::Prose if path_adjacent(content, start, end) => NsContext::Path,
            Cell::Prose => NsContext::Prose,
        })
    }
}

/// Whether a link of this type has visible text that is also its reference label
/// or its destination, so rewriting the text breaks the link (NS-52).
///
/// A shortcut (`[label]`) or collapsed (`[label][]`) link resolves by its own
/// text, and an autolink (`<https://x/y>`, `<a@b.c>`) renders its destination as
/// its text. An inline (`[text](url)`) or full reference (`[text][label]`) link
/// keeps the two apart, so its text is ordinary prose and stays wrappable.
fn text_doubles_as_syntax(link_type: pulldown_cmark::LinkType) -> bool {
    use pulldown_cmark::LinkType as L;
    matches!(
        link_type,
        L::Shortcut
            | L::ShortcutUnknown
            | L::Collapsed
            | L::CollapsedUnknown
            | L::Autolink
            | L::Email
            | L::WikiLink { .. }
    )
}

/// Set every byte of `range` to `cell`, clamped to the map.
fn fill(cells: &mut [Cell], range: std::ops::Range<usize>, cell: Cell) {
    let end = range.end.min(cells.len());
    if range.start < end {
        cells[range.start..end].fill(cell);
    }
}

/// Mark the leading `--- ... ---` frontmatter block, returning the byte offset
/// the markdown body starts at (0 when the document has no frontmatter).
///
/// Frontmatter is not CommonMark -- to a parser the opening `---` is a thematic
/// break and the closing one a setext underline -- so it is read here instead,
/// exactly as before: the block opens only on the document's first line, its
/// delimiters carry no content, a `name:` field is the item's own identity
/// (NS-24), and an unterminated block runs to the end of the document.
///
/// A leading UTF-8 BOM is stripped before the delimiter check, exactly as
/// `frontmatter.rs` strips it (DSC-23). A BOM-prefixed item file is one mind
/// discovers and installs normally, so its frontmatter has to be visible here
/// too; without the strip the opening `---` fails the check, the whole file
/// parses as CommonMark, and the block's fields read as prose that wrapping
/// rewrites (NS-47). The BOM is marked with the opening delimiter, and every
/// offset returned or recorded stays document-global.
fn mark_frontmatter(content: &str, cells: &mut [Cell]) -> usize {
    /// The one frontmatter key whose value is free prose (NS-56).
    const DESCRIPTION_KEY: &str = "description:";
    let bom = if content.starts_with('\u{feff}') {
        '\u{feff}'.len_utf8()
    } else {
        0
    };
    // A BOM is never part of the body, whether or not frontmatter follows it.
    // Returning 0 here would hand `U+FEFF` to the parser on line one, and it is
    // not whitespace in CommonMark, so it displaces that line's block structure:
    // a first-line fence stops opening, its closer reads as an opener, and the
    // rest of the file is a code block (NS-47).
    fill(cells, 0..bom, Cell::Skip);
    let mut lines = content[bom..].split_inclusive('\n');
    let Some(first) = lines.next() else {
        return bom;
    };
    if first.trim() != "---" {
        return bom;
    }
    fill(cells, 0..bom + first.len(), Cell::Skip);
    let mut offset = bom + first.len();
    for raw in lines {
        let line = raw.strip_suffix('\n').unwrap_or(raw);
        let end = offset + raw.len();
        if line.trim() == "---" {
            fill(cells, offset..end, Cell::Skip);
            return end;
        }
        // NS-56: `description:` is the one frontmatter field that is free prose,
        // so its *value* is the one frontmatter region wrapping may rewrite. Its
        // key is not (a source with a sibling literally named `description`
        // would otherwise have the key itself wrapped, breaking the field), and
        // neither is any other line: `requires:`, `build:`, `install:`, `bin:`,
        // `link:` and every harness-owned field are structure parsed out of this
        // source file, where no token is ever expanded.
        //
        // The classifier below is line-shape-based (does this line's trimmed
        // text start with `name:` / `description:`), not YAML-nesting-aware: it
        // does not track indentation depth, so a `name:`/`description:`-shaped
        // line that is actually a continuation of a folded/literal block
        // scalar (`>-`/`|-`) or a nested mapping key under some other field
        // would still classify by its own text. Every frontmatter field mind's
        // own convention uses is flat, so this holds in practice; a nested
        // document is outside what this scanner is built to read (a real YAML
        // parser would be needed for that), and this classifier only ever
        // widens or narrows a *wrappable* range, never expands a token, so a
        // false positive here is at most an over-eager `--fix` suggestion, not
        // a install-time defect.
        let trimmed = line.trim_start();
        if trimmed.starts_with("name:") {
            fill(cells, offset..end, Cell::FmName);
        } else if trimmed.starts_with(DESCRIPTION_KEY) {
            let indent = line.len() - trimmed.len();
            let value_start = offset + indent + DESCRIPTION_KEY.len();
            fill(cells, offset..value_start, Cell::FmOther);
            fill(cells, value_start..end, Cell::FmDescription);
        } else {
            fill(cells, offset..end, Cell::FmOther);
        }
        offset = end;
    }
    content.len()
}

/// True when the token spanning `[start, end)` of `content` abuts a path
/// separator.
fn path_adjacent(content: &str, start: usize, end: usize) -> bool {
    let before = content[..start].chars().next_back();
    let after = content[end..].chars().next();
    matches!(before, Some('/') | Some('~')) || matches!(after, Some('/'))
}

/// Find every `{{ns:name}}` token in `content`, each with its structural context
/// (NS-24) and byte span. Reads the document's structure once (NS-46/NS-47) so a
/// token can be classified as misplaced (in code, a path, or `name:`).
///
/// The scan is document-wide, matching [`expand`]: the opener is `{{ns:`, the
/// name runs to the next `}}` with whitespace trimmed, and an unterminated token
/// stops the scan (NS-15). A token on a delimiter line carries no reference and
/// is not reported (NS-47).
pub fn scan_ns_refs(content: &str) -> Vec<NsRef> {
    const OPEN: &str = "{{ns:";
    let mut out = Vec::new();
    let doc = Structure::new(content);
    let mut from = 0usize;
    while let Some(rel) = content[from..].find(OPEN) {
        let start = from + rel;
        let after = &content[start + OPEN.len()..];
        let Some(erel) = after.find("}}") else { break };
        let end = start + OPEN.len() + erel + 2;
        let name = after[..erel].trim();
        if !name.is_empty()
            && let Some(context) = doc.context(content, start, end)
        {
            out.push(NsRef {
                name: name.to_string(),
                context,
                start,
                end,
            });
        }
        from = end;
    }
    out
}

/// Un-wrap misplaced `{{ns:name}}` tokens (NS-24) back to the bare `name`. With
/// `all_code` false, only non-prose tokens are un-wrapped; with it true, every
/// token is un-wrapped. Returns the new content and the count.
///
/// `review --fix` (NS-54) only ever calls this with `all_code = false`, and
/// only on a markdown file ([`is_markdown`]): a token has no reportable
/// "misplaced" reading outside markdown, since none expands there at all, so
/// `--fix` reports it instead of rewriting (see `review.rs`). The `all_code`
/// parameter stays for callers that do want to unwrap every token in a
/// known-all-code text uniformly.
pub fn unwrap_misplaced(content: &str, all_code: bool) -> (String, usize) {
    let mut out = String::with_capacity(content.len());
    let mut last = 0;
    let mut count = 0;
    for r in scan_ns_refs(content) {
        if all_code || r.context.is_misplaced() {
            out.push_str(&content[last..r.start]);
            out.push_str(&r.name);
            last = r.end;
            count += 1;
        }
    }
    out.push_str(&content[last..]);
    (out, count)
}

#[cfg(test)]
mod tests {
    // spec: NS-2, NS-11, NS-12, NS-13, NS-14, NS-20, NS-21, NS-25
    use super::*;

    fn sibs(names: &[&str]) -> HashSet<String> {
        names.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn apply_prefixes_or_passes_through() {
        // spec: NS-2
        assert_eq!(apply("review", &Some("jk".into())), "jk:review");
        assert_eq!(apply("review", &None), "review");
        // An empty prefix is "no prefix" (the override), not a leading colon.
        assert_eq!(apply("review", &Some(String::new())), "review");
    }

    #[test]
    fn is_markdown_matches_every_recognized_extension_case_insensitively() {
        // spec: NS-53
        for name in [
            "SKILL.md",
            "notes.markdown",
            "notes.MARKDOWN",
            "notes.mdown",
            "notes.mkd",
            "notes.Md",
        ] {
            assert!(
                is_markdown(std::path::Path::new(name)),
                "{name} should be recognized as markdown"
            );
        }
        for name in [
            "run.sh",
            "TOOL",
            "lib.py",
            "notes.txt",
            "notes",
            "notes.mdx",
            // The extension is the LAST component, so one appearing mid-name is
            // not one: a generated script and a backup both stay non-markdown.
            "run.md.sh",
            "notes.md.bak",
            // A leading dot is a stem, not an extension (Rust's own rule), so a
            // dotfile named for a markdown extension is not markdown.
            ".md",
            ".markdown",
            // A path is judged by its file name, not by a directory component
            // that happens to carry the extension.
            "docs.md/run.sh",
        ] {
            assert!(
                !is_markdown(std::path::Path::new(name)),
                "{name} should not be recognized as markdown"
            );
        }
    }

    #[test]
    fn flatten_display_renders_ns_tokens_as_their_bare_name() {
        // spec: NS-56
        assert_eq!(
            flatten_display("hand off to {{ns:dev}} when done"),
            "hand off to dev when done"
        );
        // Whitespace inside the token is trimmed, mirroring `expand`.
        assert_eq!(flatten_display("see {{ns: dev }}"), "see dev");
        // No tokens: unchanged.
        assert_eq!(flatten_display("plain text"), "plain text");
        // An unterminated token is left verbatim.
        assert_eq!(flatten_display("see {{ns:dev"), "see {{ns:dev");
        // A non-`ns:` token (e.g. a stray `{{self}}`) is left as written: this
        // helper only flattens the name-reference family.
        assert_eq!(flatten_display("path is {{self}}"), "path is {{self}}");
    }

    #[test]
    fn templatize_wraps_bare_siblings_and_skips_tokens() {
        // spec: INIT-5
        let s = sibs(&["dev", "style"]);
        let (out, n) = templatize("hand off to dev, see {{ns:style}}, not develop", &s);
        assert_eq!(
            out, "hand off to {{ns:dev}}, see {{ns:style}}, not develop",
            "bare `dev` is wrapped; the token and the longer word `develop` are left alone"
        );
        assert_eq!(n, 1, "only the one bare sibling mention is rewritten");

        // Idempotent: a second pass changes nothing (everything is now tokenized).
        let (again, m) = templatize(&out, &s);
        assert_eq!(again, out);
        assert_eq!(m, 0);
    }

    #[test]
    fn prefix_choice_interprets_the_meld_prompt() {
        // spec: CLI-24
        // Empty / yes -> keep the declared prefix (no alias change).
        assert_eq!(prefix_choice(""), None);
        assert_eq!(prefix_choice("y"), None);
        assert_eq!(prefix_choice("YES"), None);
        // no/none -> the explicit "no prefix" override (empty alias).
        assert_eq!(prefix_choice("n"), Some(String::new()));
        assert_eq!(prefix_choice("none"), Some(String::new()));
        // Anything else is a custom prefix, trimmed and verbatim-cased.
        assert_eq!(prefix_choice("  MyPfx "), Some("MyPfx".to_string()));
    }

    #[test]
    fn expand_unprefixed_yields_bare_names() {
        let s = sibs(&["test"]);
        let no_bare: HashSet<String> = HashSet::new();
        let got = expand("hand off to {{ns:test}} now", &None, &s, &no_bare).unwrap();
        assert_eq!(got, "hand off to test now");
    }

    #[test]
    fn expand_prefixed_yields_prefixed_names() {
        // spec: NS-11
        let s = sibs(&["test"]);
        let no_bare: HashSet<String> = HashSet::new();
        let got = expand("see {{ns:test}}.", &Some("jk".into()), &s, &no_bare).unwrap();
        assert_eq!(got, "see jk:test.");
    }

    #[test]
    fn expand_rejects_unknown_referent() {
        let s = sibs(&["test"]);
        let no_bare: HashSet<String> = HashSet::new();
        assert_eq!(
            expand("{{ns:nope}}", &None, &s, &no_bare),
            Err("nope".to_string())
        );
    }

    #[test]
    fn expand_passes_content_without_tokens() {
        let s = sibs(&["test"]);
        let no_bare: HashSet<String> = HashSet::new();
        assert_eq!(
            expand("no tokens here", &None, &s, &no_bare).unwrap(),
            "no tokens here"
        );
    }

    #[test]
    fn expand_trims_token_and_leaves_unterminated_verbatim() {
        // spec: NS-15
        let s = sibs(&["dev"]);
        let no_bare: HashSet<String> = HashSet::new();
        // Whitespace inside the token is trimmed before the sibling lookup.
        assert_eq!(
            expand("{{ns:  dev  }}", &Some("jk".into()), &s, &no_bare).unwrap(),
            "jk:dev"
        );
        // An unterminated token (no closing `}}`) is passed through unchanged.
        assert_eq!(
            expand("see {{ns:dev", &None, &s, &no_bare).unwrap(),
            "see {{ns:dev"
        );
    }

    #[test]
    fn expand_agent_referent_expands_bare_under_prefix() {
        // spec: NS-42
        // An agent referent in bare_names expands to its bare name even under a
        // prefix. A skill referent NOT in bare_names still expands prefixed.
        let s = sibs(&["dev", "review"]);
        let bare = sibs(&["dev"]); // dev is an agent (bare); review is a skill (not bare)
        let pfx = Some("jk".to_string());
        // Agent token: always bare regardless of prefix.
        assert_eq!(
            expand("delegate to {{ns:dev}}", &pfx, &s, &bare).unwrap(),
            "delegate to dev"
        );
        // Skill token: still prefixed.
        assert_eq!(
            expand("use {{ns:review}}", &pfx, &s, &bare).unwrap(),
            "use jk:review"
        );
        // Cross-kind shadow: if "shared" is both an agent AND a skill, it is NOT
        // in bare_names, so it expands prefixed.
        let s2 = sibs(&["shared"]);
        let no_bare: HashSet<String> = HashSet::new(); // "shared" not in bare_names -> prefixed
        assert_eq!(
            expand("{{ns:shared}}", &pfx, &s2, &no_bare).unwrap(),
            "jk:shared"
        );
        // An agent token still validates: a missing name is still Err.
        let bare2 = sibs(&["dev"]);
        let s3 = sibs(&["dev"]);
        assert_eq!(
            expand("{{ns:ghost}}", &pfx, &s3, &bare2),
            Err("ghost".to_string())
        );
    }

    #[test]
    fn unguarded_finds_bare_prose_refs_only() {
        let s = sibs(&["test", "planner"]);
        // 'test' appears in prose; 'planner' only inside a token (guarded).
        let refs = unguarded_refs("run the test, then {{ns:planner}}", &s);
        assert_eq!(refs, vec!["test".to_string()]);
    }

    #[test]
    fn unguarded_respects_word_boundaries() {
        let s = sibs(&["do"]);
        // "doing" must not match the sibling "do".
        assert!(unguarded_refs("doing work", &s).is_empty());
        assert_eq!(unguarded_refs("just do it", &s), vec!["do".to_string()]);
    }

    #[test]
    fn unguarded_does_not_flag_names_already_inside_any_token() {
        // spec: NS-20
        // A sibling name that appears only inside a token of any kind must not
        // be reported as an unguarded prose reference.  Previously, strip_tokens
        // only removed {{ns:...}} spans, so {{tools:detect}} or
        // {{path:skill:other}} would still expose the bare sibling name to the
        // whole-word scan and produce a false-positive advisory.
        let s = sibs(&["detect", "other", "planner"]);

        // Guarded by {{tools:NAME}}: not flagged.
        assert!(
            unguarded_refs("run {{tools:detect}} to start", &s).is_empty(),
            "{{tools:detect}} must not produce an unguarded-reference advisory"
        );

        // Guarded by {{path:kind:NAME}}: not flagged.
        assert!(
            unguarded_refs("see {{path:skill:other}} for details", &s).is_empty(),
            "{{path:skill:other}} must not produce an unguarded-reference advisory"
        );

        // Guarded by {{ns:NAME}}: not flagged (pre-existing behavior preserved).
        assert!(
            unguarded_refs("hand off to {{ns:planner}}", &s).is_empty(),
            "{{ns:planner}} must not produce an unguarded-reference advisory"
        );

        // Bare prose mention is still flagged (true-positive preserved).
        let bare = unguarded_refs("run detect and see other", &s);
        assert_eq!(
            bare,
            vec!["detect".to_string(), "other".to_string()],
            "bare prose sibling names must still be flagged"
        );

        // Mixed: guarded and bare in the same content.
        let mixed = unguarded_refs("use {{tools:detect}} then call other directly", &s);
        assert_eq!(
            mixed,
            vec!["other".to_string()],
            "only the bare mention should be flagged when the same name also appears in a token"
        );
    }

    #[test]
    fn referenced_names_extracts_tokens_in_order_deduped() {
        // spec: DEP-1
        // Bare names from each token, first-seen order, de-duplicated.
        let got = referenced_names("see {{ns:test}} then {{ns:do}} then {{ns:test}}");
        assert_eq!(got, vec!["test".to_string(), "do".to_string()]);
    }

    #[test]
    fn referenced_names_trims_whitespace_inside_token() {
        // spec: DEP-1
        // Whitespace inside a token is trimmed, mirroring `expand`.
        let got = referenced_names("{{ns:  dev  }}");
        assert_eq!(got, vec!["dev".to_string()]);
    }

    #[test]
    fn referenced_names_no_tokens_is_empty() {
        // spec: DEP-1
        assert!(referenced_names("plain prose, no tokens").is_empty());
    }

    #[test]
    fn referenced_names_unterminated_token_is_not_a_reference() {
        // spec: NS-15
        // An unterminated token (no closing `}}`) stops the scan, exactly like
        // `expand` leaves the remainder verbatim. A terminated token before it is
        // still captured; the dangling one is not.
        assert!(referenced_names("see {{ns:dev").is_empty());
        assert_eq!(
            referenced_names("{{ns:test}} then {{ns:dev"),
            vec!["test".to_string()]
        );
    }

    #[test]
    fn referenced_names_empty_token_is_skipped() {
        // spec: NS-15
        // A token with an empty name (`{{ns:}}`) or whitespace-only name
        // (`{{ns:   }}`) trims to "" and is not a reference: it is skipped, but
        // the scan continues past it to any following valid token.
        assert!(referenced_names("{{ns:}}").is_empty());
        assert!(referenced_names("{{ns:   }}").is_empty());
        assert_eq!(
            referenced_names("{{ns:}} then {{ns:dev}}"),
            vec!["dev".to_string()]
        );
    }

    #[test]
    fn referenced_names_valid_then_unterminated_returns_valid_only() {
        // spec: NS-15
        // A valid terminated token followed by an unterminated one yields the
        // valid name then stops at the dangling token (which is not a reference).
        assert_eq!(
            referenced_names("use {{ns:dev}} and then {{ns:planner"),
            vec!["dev".to_string()]
        );
    }

    #[test]
    fn referenced_names_whitespace_or_empty_content_is_empty() {
        // spec: NS-15
        // Whitespace-only or empty content carries no tokens and no references.
        assert!(referenced_names("").is_empty());
        assert!(referenced_names("   \n\t  ").is_empty());
    }

    #[test]
    fn referenced_names_empty_token_does_not_swallow_following_close() {
        // spec: NS-15
        // `{{ns:}}{{ns:dev}}` is two adjacent tokens: the first is empty (skipped)
        // and the scan resumes after its `}}`, so the second is still parsed.
        assert_eq!(
            referenced_names("{{ns:}}{{ns:dev}}"),
            vec!["dev".to_string()]
        );
    }

    // ---- validate_prefix (NS-25) ----------------------------------------------

    #[test]
    fn validate_prefix_rejects_reserved_kind_words() {
        // spec: NS-25 CMD-9
        // `command` is rejected both as a real kind word (NS-25) and by the
        // extended list it was on before the kind existed (NS-29); the error is
        // the same either way, so no source's prefix changes validity.
        for word in ["skill", "agent", "rule", "command", "tool"] {
            let err = validate_prefix(word).unwrap_err();
            assert!(
                matches!(err, crate::error::MindError::ReservedPrefix { ref prefix } if prefix == word),
                "expected ReservedPrefix for '{word}', got: {err:?}"
            );
        }
    }

    #[test]
    fn validate_prefix_accepts_normal_prefix_and_empty() {
        // spec: NS-25
        // A normal user-chosen prefix is fine.
        assert!(validate_prefix("jk").is_ok(), "'jk' must be accepted");
        assert!(
            validate_prefix("my-org").is_ok(),
            "'my-org' must be accepted"
        );
        // Empty is fine: it means "no prefix in effect".
        assert!(validate_prefix("").is_ok(), "empty must be accepted");
    }

    #[test]
    fn validate_prefix_rejects_path_traversal_and_separators() {
        // spec: NS-28 -- a prefix with path traversal, separators, or control
        // characters is unsafe and must be rejected with UnsafePrefix.
        for bad in [
            "../evil", "..", ".", "a/b", "a\\b", "~home", "a:b", "a\x00b",
        ] {
            let err = validate_prefix(bad).unwrap_err();
            assert!(
                matches!(err, crate::error::MindError::UnsafePrefix { .. }),
                "expected UnsafePrefix for {bad:?}, got {err:?}"
            );
        }
    }

    #[test]
    fn validate_prefix_rejects_blocked_unicode_with_unsafe_prefix() {
        // spec: NS-72 NS-73 -- `validate_prefix` (the chokepoint every
        // user/repo-supplied prefix ingress calls) rejects the broadened
        // blocked-Unicode set with the same structured `UnsafePrefix` error as
        // the ASCII path-safety violations, not a silent pass-through or a
        // different error variant.
        for bad in [
            "pay\u{202E}oot", // pre-NS-73 baseline (a bidi override)
            "acme\u{E0041}",  // NS-73: Unicode tag block
            "acme\u{FE0F}",   // NS-73: variation selector
        ] {
            let err = validate_prefix(bad).unwrap_err();
            assert!(
                matches!(err, crate::error::MindError::UnsafePrefix { .. }),
                "expected UnsafePrefix for {bad:?}, got {err:?}"
            );
        }
    }

    #[test]
    fn is_safe_prefix_component_rejects_multibyte_control_and_bidi() {
        // spec: NS-72 NS-73 -- the byte scan catches ASCII controls, but a
        // multi-byte bidi override / directional mark / zero-width / C1
        // control / tag-block / variation-selector code point must be
        // rejected too, so a user-supplied `meld --namespace`/`-N` prefix or a
        // repo's declared `[source].prefix` (the live paths this guard
        // affects via `validate_prefix`; end-to-end coverage in
        // tests/cli_prefix_guard.rs) cannot carry a spoofing or invisibly
        // smuggled payload.
        for ok in ["plugin", "my-plugin", "caf\u{00e9}"] {
            assert!(is_safe_prefix_component(ok), "{ok:?} should be accepted");
        }
        for bad in [
            "pay\u{202E}",   // bidi override
            "a\u{2066}b",    // isolate
            "a\u{200B}b",    // zero-width space
            "a\u{200E}b",    // LRM
            "a\u{FEFF}b",    // BOM
            "a\u{0085}b",    // NEL, a C1 control
            "acme\u{E0041}", // NS-73: Unicode tag block
            "acme\u{FE0F}",  // NS-73: variation selector
        ] {
            assert!(!is_safe_prefix_component(bad), "{bad:?} should be rejected");
        }
    }

    #[test]
    fn validate_prefix_rejects_extended_reserved_words() {
        // spec: NS-29 -- future kind words are banned even though they are not
        // item-kind words (so NS-25 alone would not catch them).
        for word in [
            "command",
            "hook",
            "mcp",
            "plugin",
            "prompt",
            "mode",
            "output-style",
        ] {
            let err = validate_prefix(word).unwrap_err();
            assert!(
                matches!(err, crate::error::MindError::ReservedPrefix { .. }),
                "expected ReservedPrefix for {word:?}, got {err:?}"
            );
        }
    }

    // ---- path-reference tokens ({{self}}, {{tools:}}, {{path:}}) -------------

    use crate::error::ItemKind;
    use std::path::Path;

    fn psib(kind: ItemKind, name: &str, bin: Option<&str>) -> PathSibling {
        PathSibling {
            kind,
            name: name.to_string(),
            bin: bin.map(|s| s.to_string()),
        }
    }

    fn ctx<'a>(
        store: &'a Path,
        prefix: &'a Option<String>,
        self_kind: ItemKind,
        self_name: &'a str,
        siblings: &'a [PathSibling],
    ) -> PathCtx<'a> {
        PathCtx {
            store_root: store,
            home: None,
            prefix,
            self_kind,
            self_name,
            siblings,
        }
    }

    #[test]
    fn self_token_resolves_to_own_store_dir() {
        // spec: TOOL-10
        let store = Path::new("/m/store");
        let none = None;
        let c = ctx(store, &none, ItemKind::Skill, "review", &[]);
        assert_eq!(
            expand_paths("run {{self}}/resources/pr.py here", &c).unwrap(),
            "run /m/store/skill/review/resources/pr.py here"
        );
    }

    #[test]
    fn self_token_is_prefix_aware() {
        // spec: TOOL-10 TOOL-13
        let store = Path::new("/m/store");
        let pfx = Some("jk".to_string());
        let c = ctx(store, &pfx, ItemKind::Skill, "review", &[]);
        assert_eq!(
            expand_paths("{{self}}", &c).unwrap(),
            "/m/store/skill/jk:review"
        );
    }

    #[test]
    fn store_paths_render_with_tilde_when_under_home() {
        // spec: TOOL-16
        let home = Path::new("/home/jk");
        let store = Path::new("/home/jk/.mind/store");
        let none = None;
        let sibs = vec![psib(ItemKind::Tool, "shard-plan", Some("shard-plan"))];
        let c = PathCtx {
            store_root: store,
            home: Some(home),
            prefix: &none,
            self_kind: ItemKind::Skill,
            self_name: "review",
            siblings: &sibs,
        };
        // Every token kind keeps the leading `~` instead of spelling out home.
        assert_eq!(
            expand_paths("{{self}}/resources/pr.py", &c).unwrap(),
            "~/.mind/store/skill/review/resources/pr.py"
        );
        assert_eq!(
            expand_paths("{{tools:shard-plan}}", &c).unwrap(),
            "~/.mind/store/tool/shard-plan/shard-plan"
        );
        assert_eq!(
            expand_paths("{{path:tool:shard-plan}}/lib.sh", &c).unwrap(),
            "~/.mind/store/tool/shard-plan/lib.sh"
        );
    }

    #[test]
    fn store_paths_stay_absolute_when_store_not_under_home() {
        // spec: TOOL-16
        let home = Path::new("/home/jk");
        // A MIND_HOME pointing outside home (or no home) yields an absolute path.
        let store = Path::new("/srv/mind/store");
        let none = None;
        let c = PathCtx {
            store_root: store,
            home: Some(home),
            prefix: &none,
            self_kind: ItemKind::Skill,
            self_name: "review",
            siblings: &[],
        };
        assert_eq!(
            expand_paths("{{self}}", &c).unwrap(),
            "/srv/mind/store/skill/review"
        );
        // With no home at all, also absolute.
        let c = PathCtx { home: None, ..c };
        assert_eq!(
            expand_paths("{{self}}", &c).unwrap(),
            "/srv/mind/store/skill/review"
        );
    }

    #[test]
    fn tools_token_resolves_to_entrypoint() {
        // spec: TOOL-12
        let store = Path::new("/m/store");
        let none = None;
        let sibs = vec![psib(ItemKind::Tool, "shard-plan", Some("shard-plan"))];
        let c = ctx(store, &none, ItemKind::Skill, "review", &sibs);
        assert_eq!(
            expand_paths("pipe to {{tools:shard-plan}} --max 5", &c).unwrap(),
            "pipe to /m/store/tool/shard-plan/shard-plan --max 5"
        );
    }

    #[test]
    fn tools_token_is_prefix_aware() {
        // spec: TOOL-12 TOOL-13
        let store = Path::new("/m/store");
        let pfx = Some("jk".to_string());
        let sibs = vec![psib(ItemKind::Tool, "shard-plan", Some("shard-plan"))];
        let c = ctx(store, &pfx, ItemKind::Skill, "review", &sibs);
        assert_eq!(
            expand_paths("{{tools:shard-plan}}", &c).unwrap(),
            "/m/store/tool/jk:shard-plan/shard-plan"
        );
    }

    #[test]
    fn tools_token_errors_on_missing_or_binless_or_non_tool() {
        // spec: TOOL-12 TOOL-17
        use crate::error::BadRefReason::{NoMatch, ToolNoBin};
        let store = Path::new("/m/store");
        let none = None;
        // No such sibling: a plain miss.
        let c = ctx(store, &none, ItemKind::Skill, "review", &[]);
        assert_eq!(
            expand_paths("{{tools:nope}}", &c),
            Err(("{{tools:nope}}".to_string(), NoMatch))
        );
        // A tool with no resolvable bin: the distinct ToolNoBin cause (TOOL-17).
        let binless = vec![psib(ItemKind::Tool, "x", None)];
        let c = ctx(store, &none, ItemKind::Skill, "review", &binless);
        assert_eq!(
            expand_paths("{{tools:x}}", &c),
            Err(("{{tools:x}}".to_string(), ToolNoBin))
        );
        // A sibling of that name exists but is not a tool: a miss, not ToolNoBin.
        let not_tool = vec![psib(ItemKind::Skill, "x", None)];
        let c = ctx(store, &none, ItemKind::Skill, "review", &not_tool);
        assert_eq!(
            expand_paths("{{tools:x}}", &c),
            Err(("{{tools:x}}".to_string(), NoMatch))
        );
    }

    #[test]
    fn path_token_resolves_sibling_dir_qualified_and_bare() {
        // spec: TOOL-11
        let store = Path::new("/m/store");
        let none = None;
        let sibs = vec![psib(ItemKind::Tool, "detect", Some("detect"))];
        let c = ctx(store, &none, ItemKind::Skill, "review", &sibs);
        // Kind-qualified, reaching a non-entrypoint file.
        assert_eq!(
            expand_paths("{{path:tool:detect}}/lib/helper.sh", &c).unwrap(),
            "/m/store/tool/detect/lib/helper.sh"
        );
        // Bare name (unambiguous).
        assert_eq!(
            expand_paths("{{path:detect}}", &c).unwrap(),
            "/m/store/tool/detect"
        );
    }

    #[test]
    fn path_token_ambiguity_errors_unless_kind_qualified() {
        // spec: TOOL-11 TOOL-18
        let store = Path::new("/m/store");
        let none = None;
        // A skill and an agent share the bare name `x`.
        let sibs = vec![
            psib(ItemKind::Skill, "x", None),
            psib(ItemKind::Agent, "x", None),
        ];
        let c = ctx(store, &none, ItemKind::Skill, "self", &sibs);
        // Ambiguous across kinds without a qualifier: a distinct cause from a
        // plain miss (TOOL-18). The referent does match -- it is under-qualified.
        assert_eq!(
            expand_paths("{{path:x}}", &c),
            Err((
                "{{path:x}}".to_string(),
                crate::error::BadRefReason::AmbiguousKind
            ))
        );
        // A kind qualifier disambiguates.
        assert_eq!(
            expand_paths("{{path:agent:x}}", &c).unwrap(),
            "/m/store/agent/x"
        );
        // A miss keeps the NoMatch wording, distinct from the ambiguity above.
        assert_eq!(
            expand_paths("{{path:none}}", &c),
            Err((
                "{{path:none}}".to_string(),
                crate::error::BadRefReason::NoMatch
            ))
        );
    }

    #[test]
    fn rewrite_maps_hardcoded_paths_to_tokens() {
        // spec: CLI-138
        let store = Path::new("/m/store");
        let none = None;
        let sibs = vec![
            psib(ItemKind::Tool, "detect", Some("detect")),
            psib(ItemKind::Skill, "release", None),
        ];
        let c = ctx(store, &none, ItemKind::Skill, "review", &sibs);
        let input = "self ~/.claude/skills/review/resources/pr.py \
                     tool ~/.mind/store/tool/detect/detect \
                     other ~/.mind/store/skill/release/x.sh \
                     foreign ~/.claude/skills/unknown/y.sh";
        let (out, n) = rewrite_hardcoded_paths(input, &c);
        assert_eq!(n, 3, "three confident rewrites: {out}");
        assert!(out.contains("self {{self}}/resources/pr.py"), "{out}");
        assert!(out.contains("tool {{tools:detect}}"), "{out}");
        assert!(out.contains("other {{path:skill:release}}/x.sh"), "{out}");
        // A path naming no sibling is left untouched (conservative).
        assert!(
            out.contains("foreign ~/.claude/skills/unknown/y.sh"),
            "{out}"
        );
    }

    #[test]
    fn detect_reports_paths_with_and_without_suggestions() {
        // spec: CLI-136
        let store = Path::new("/m/store");
        let none = None;
        let sibs = vec![psib(ItemKind::Tool, "detect", Some("detect"))];
        let c = ctx(store, &none, ItemKind::Skill, "review", &sibs);
        let found = detect_hardcoded_paths(
            "a ~/.mind/store/tool/detect/detect b ~/.agents/resources/x.sh",
            &c,
        );
        assert_eq!(found.len(), 2);
        assert_eq!(found[0].suggestion.as_deref(), Some("{{tools:detect}}"));
        // ~/.agents/resources/... maps to no kind/name, so it is flagged without a
        // concrete suggestion rather than mis-rewritten.
        assert_eq!(found[1].suggestion, None);
    }

    #[test]
    fn hardcoded_detects_env_and_absolute_home_forms() {
        // spec: CLI-136
        let store = Path::new("/m/store");
        let none = None;
        let sibs = vec![psib(ItemKind::Tool, "detect", Some("detect"))];
        let c = ctx(store, &none, ItemKind::Skill, "review", &sibs);
        // Every home-root spelling reduces to the same tool token.
        for path in [
            "$HOME/.mind/store/tool/detect/detect",
            "${HOME}/.mind/store/tool/detect/detect",
            "/home/jk/.mind/store/tool/detect/detect",
            "/Users/jk/.mind/store/tool/detect/detect",
        ] {
            let found = detect_hardcoded_paths(&format!("run {path} now"), &c);
            assert_eq!(found.len(), 1, "{path}");
            assert_eq!(found[0].matched, path, "matched span is the original form");
            assert_eq!(
                found[0].suggestion.as_deref(),
                Some("{{tools:detect}}"),
                "{path}"
            );
        }
        // A `/home` path that is not an install layout is not flagged.
        assert!(detect_hardcoded_paths("see /home/jk/projects/x", &c).is_empty());
    }

    #[test]
    fn hardcoded_classifies_own_tool_and_other() {
        // spec: CLI-145
        let store = Path::new("/m/store");
        let none = None;
        let sibs = vec![
            psib(ItemKind::Tool, "detect", Some("detect")),
            psib(ItemKind::Skill, "release", None),
        ];
        let c = ctx(store, &none, ItemKind::Skill, "review", &sibs);
        let found = detect_hardcoded_paths(
            "own ~/.claude/skills/review/resources/pr.py \
             tool ~/.mind/store/tool/detect/detect \
             other ~/.mind/store/skill/release/x.sh \
             foreign ~/.claude/skills/unknown/y.sh",
            &c,
        );
        assert_eq!(found.len(), 4);
        assert_eq!(found[0].kind, HardcodedKind::OwnResource);
        assert_eq!(found[1].kind, HardcodedKind::SharedTool);
        assert_eq!(found[2].kind, HardcodedKind::OtherItem);
        // A recognized layout naming no sibling is OtherItem with no suggestion.
        assert_eq!(found[3].kind, HardcodedKind::OtherItem);
        assert_eq!(found[3].suggestion, None);
    }

    #[test]
    fn parse_install_path_strips_md_suffix_for_agent_rule_and_command() {
        // spec: M17 -- the `ItemKind::Command` arm of `parse_install_path`'s
        // `.md`-stripping match had no test. Reverting it (falling into the
        // `_ => first.to_string()` arm) would leave a command's parsed name as
        // `deploy.md`, which never matches a sibling bare name, so the
        // `{{ns:}}` rewrite would silently skip. Cover both the lobe spelling
        // (`~/.claude/<dir>/<name>.md`) and the store spelling
        // (`~/.mind/store/<kind>/<name>`, already suffixless), alongside the
        // agent/rule cases the same match arm shares.
        for (kind, dir) in [
            (ItemKind::Agent, "agents"),
            (ItemKind::Rule, "rules"),
            (ItemKind::Command, "commands"),
        ] {
            let lobe = format!("~/.claude/{dir}/deploy.md");
            assert_eq!(
                parse_install_path(&lobe),
                Some((kind, "deploy".to_string(), String::new())),
                "lobe spelling for {kind:?}"
            );
            let store = format!("~/.mind/store/{}/deploy", kind.as_str());
            assert_eq!(
                parse_install_path(&store),
                Some((kind, "deploy".to_string(), String::new())),
                "store spelling for {kind:?}"
            );
        }
    }

    #[test]
    fn token_for_path_maps_command_lobe_and_store_spellings() {
        // spec: M17 -- exercises `token_for_path`/`parse_install_path` for a
        // real `Command` sibling through both the lobe and the store path
        // spellings mirrored by `rewrite_hardcoded_paths`. If the `Command` arm
        // stopped stripping `.md`, the parsed name `deploy.md` would never
        // match the sibling's bare `deploy` and neither path would rewrite.
        let store = Path::new("/m/store");
        let none = None;
        let sibs = vec![psib(ItemKind::Command, "deploy", None)];
        let c = ctx(store, &none, ItemKind::Skill, "review", &sibs);
        let (out, n) = rewrite_hardcoded_paths(
            "lobe ~/.claude/commands/deploy.md store ~/.mind/store/command/deploy",
            &c,
        );
        assert_eq!(n, 2, "{out}");
        assert!(out.contains("lobe {{path:command:deploy}}"), "{out}");
        assert!(out.contains("store {{path:command:deploy}}"), "{out}");
    }

    #[test]
    fn bare_tool_refs_finds_tool_names_outside_tokens() {
        // spec: CLI-137
        let sibs = vec![
            psib(ItemKind::Tool, "detect", Some("detect")),
            psib(ItemKind::Skill, "review", None),
        ];
        // `detect` in prose is found; the skill `review` (not a tool) is not; a
        // `{{tools:detect}}` token is not double-counted.
        let refs = bare_tool_refs("run detect then review; later {{tools:detect}}", &sibs);
        assert_eq!(refs, vec!["detect".to_string()]);
        // Prefix-independence: no prefix here, yet the bare tool ref is reported.
        assert!(bare_tool_refs("just detect", &sibs).contains(&"detect".to_string()));
    }

    #[test]
    fn path_tokens_ignore_ns_and_handle_edges() {
        // spec: TOOL-14
        let store = Path::new("/m/store");
        let none = None;
        let c = ctx(store, &none, ItemKind::Tool, "t", &[]);
        // An `{{ns:}}` token is left verbatim; a following path token still resolves.
        assert_eq!(
            expand_paths("{{ns:foo}} then {{self}}", &c).unwrap(),
            "{{ns:foo}} then /m/store/tool/t"
        );
        // Inner whitespace is trimmed.
        assert_eq!(expand_paths("{{ self }}", &c).unwrap(), "/m/store/tool/t");
        // An unterminated token is left verbatim.
        assert_eq!(
            expand_paths("see {{self", &c).unwrap(),
            "see {{self".to_string()
        );
        // Content with no token is unchanged.
        assert_eq!(expand_paths("plain prose", &c).unwrap(), "plain prose");
        // A stray `{{` that is not a known token passes through.
        assert_eq!(expand_paths("a {{x}} b", &c).unwrap(), "a {{x}} b");
    }

    // ---- misplaced {{ns:}} detection / un-wrap / templatize hardening (NS-24) -

    #[test]
    fn scan_ns_refs_classifies_context() {
        // spec: NS-24
        let doc = "---\nname: {{ns:dev}}\ndescription: see {{ns:review}}\n---\n\
                   prose {{ns:dev}} here\n`{{ns:test}}` span\n~/{{ns:dev}}\n\
                   ```\n{{ns:do}}\n```\n";
        let got: Vec<(String, NsContext)> = scan_ns_refs(doc)
            .into_iter()
            .map(|r| (r.name, r.context))
            .collect();
        assert_eq!(
            got,
            vec![
                ("dev".into(), NsContext::FrontmatterName),
                ("review".into(), NsContext::Prose), // other frontmatter is prose
                ("dev".into(), NsContext::Prose),
                ("test".into(), NsContext::CodeSpan),
                ("dev".into(), NsContext::Path),
                ("do".into(), NsContext::CodeBlock),
            ]
        );
    }

    /// Collect the classified context of each token, for the structural tests.
    fn contexts(doc: &str) -> Vec<(String, NsContext)> {
        scan_ns_refs(doc)
            .into_iter()
            .map(|r| (r.name, r.context))
            .collect()
    }

    #[test]
    fn token_after_a_multiline_code_span_is_prose() {
        // spec: NS-46
        // The reported repro: a code span opened on one line and closed on the
        // next leaves the closing backtick alone on the continuation line, which
        // line-local backtick parity read as "inside a span" for everything after
        // it. The span is matched document-wide, so the token is prose.
        let doc = "Set up the token with `gh auth\n\
                   token --hostname github.com` and the bundle (via {{ns:cert}}).\n";
        assert_eq!(contexts(doc), vec![("cert".into(), NsContext::Prose)]);
    }

    #[test]
    fn token_inside_a_multiline_code_span_is_a_code_span() {
        // spec: NS-46
        // The true positive the fix must not break: a token genuinely inside a
        // span that wraps across a line break is still misplaced.
        let doc = "run `mind learn\n{{ns:dev}}` now\n";
        assert_eq!(contexts(doc), vec![("dev".into(), NsContext::CodeSpan)]);
    }

    #[test]
    fn code_spans_match_by_run_length_and_stop_at_a_blank_line() {
        // spec: NS-46
        // A double-backtick span may contain a single backtick: only a run of the
        // same length closes it.
        let doc = "``a ` {{ns:dev}}`` and {{ns:do}} after\n";
        assert_eq!(
            contexts(doc),
            vec![
                ("dev".into(), NsContext::CodeSpan),
                ("do".into(), NsContext::Prose),
            ]
        );
        // An unmatched run is literal text, and a blank line ends the paragraph,
        // so a backtick in one paragraph cannot open a span over the next.
        let doc = "a stray ` backtick\n\nsee {{ns:dev}} here\n";
        assert_eq!(contexts(doc), vec![("dev".into(), NsContext::Prose)]);
        // A block boundary ends it too: two list items are two blocks, so the
        // stray backtick in the first cannot pair with the one in the second.
        let doc = "- a stray ` backtick\n- hand off to {{ns:dev}} ` now\n";
        assert_eq!(contexts(doc), vec![("dev".into(), NsContext::Prose)]);
        // A span still crosses the line break inside one list item.
        let doc = "- run `gh auth\n  token` then {{ns:dev}}\n";
        assert_eq!(contexts(doc), vec![("dev".into(), NsContext::Prose)]);
        let doc = "- run `gh {{ns:dev}}\n  token` now\n";
        assert_eq!(contexts(doc), vec![("dev".into(), NsContext::CodeSpan)]);
    }

    #[test]
    fn fences_close_only_on_an_equal_or_longer_run_of_their_own_char() {
        // spec: NS-47
        // A four-backtick fence wrapping a three-backtick example: the inner
        // fence is content, so the outer block ends where it says it does and the
        // prose after it is prose.
        let doc = "````markdown\n{{ns:dev}}\n```sh\necho {{ns:do}}\n```\n````\n\
                   then see {{ns:dev}} in prose\n";
        assert_eq!(
            contexts(doc),
            vec![
                ("dev".into(), NsContext::CodeBlock),
                ("do".into(), NsContext::CodeBlock),
                ("dev".into(), NsContext::Prose),
            ]
        );
        // A tilde fence is a fence too, and a backtick run does not close it.
        let doc = "~~~\n{{ns:dev}}\n```\n{{ns:do}}\n~~~\nprose {{ns:dev}}\n";
        assert_eq!(
            contexts(doc),
            vec![
                ("dev".into(), NsContext::CodeBlock),
                ("do".into(), NsContext::CodeBlock),
                ("dev".into(), NsContext::Prose),
            ]
        );
    }

    #[test]
    fn a_line_initial_backtick_run_with_an_info_backtick_is_not_a_fence() {
        // spec: NS-47
        // ```` ``a ` b`` ```` at the start of a line is a code span, not a fence
        // opener: a backtick fence's info string may not contain a backtick. The
        // old prefix test flipped the fence state here and misclassified the rest
        // of the document as a code block.
        let doc = "``run ` here`` opens nothing\n\nprose {{ns:dev}} here\n";
        assert_eq!(contexts(doc), vec![("dev".into(), NsContext::Prose)]);
    }

    #[test]
    fn templatize_reads_the_same_structure_as_the_token_scan() {
        // spec: NS-46 NS-47
        // Wrapping and un-wrapping must agree about what is code, or `--fix`
        // un-wraps a token that it then declines to re-wrap. A bare sibling name
        // after a multi-line code span is prose and gets wrapped; one inside a
        // nested fence stays bare.
        let s = sibs(&["dev", "do"]);
        let doc = "run `gh auth\nprint` then dev runs\n\n\
                   ````md\n```sh\ndev do\n```\n````\n";
        let (out, n) = templatize(doc, &s);
        assert_eq!(n, 1, "only the prose mention is wrapped: {out}");
        assert!(out.contains("then {{ns:dev}} runs"), "{out}");
        assert!(out.contains("dev do\n"), "fenced content untouched: {out}");
    }

    #[test]
    fn fix_passes_never_introduce_an_unguarded_reference() {
        // spec: NS-48
        // The class-level guard, over a corpus of the reported shapes whose
        // sibling mentions all sit in prose: run the same two passes `review
        // --fix` runs (un-wrap misplaced tokens, then templatize bare prose refs)
        // and require the result to be clean by the independent
        // unguarded-reference check (NS-20/NS-21).
        let s = sibs(&["cos-spec", "cos-cert-setup"]);
        let corpus = [
            // Multi-line code span, then a prose token (the cos-http repro).
            "Get it with `gh auth\ntoken --hostname github.com` (via {{ns:cos-cert-setup}}).\n",
            // Nested fence, then a prose token (the cos-handoff repro).
            "````md\n```sh\necho hi\n```\n````\nSee the {{ns:cos-spec}} skill for EARS.\n",
            // A bare prose mention after a multi-line span: templatize must reach
            // it, or the fixed file keeps an unguarded reference.
            "Get it with `gh auth\ntoken` then read the cos-spec skill.\n",
            // A line-initial backtick run that is not a fence opener.
            "``a ` b`` is a span\n\nhand off to {{ns:cos-spec}} now\n",
        ];
        for doc in corpus {
            let (unwrapped, _) = unwrap_misplaced(doc, false);
            let (fixed, _) = templatize(&unwrapped, &s);
            assert_eq!(
                unguarded_refs(&fixed, &s),
                Vec::<String>::new(),
                "--fix left an unguarded reference in: {fixed}"
            );
        }
    }

    // ---- outside-in coverage of the structural scan (NS-46 / NS-47 / NS-48) --

    /// The two passes `review --fix` runs over a markdown file, in order:
    /// un-wrap misplaced tokens, then wrap bare prose mentions.
    fn fix_passes(content: &str, siblings: &HashSet<String>) -> String {
        let (unwrapped, _) = unwrap_misplaced(content, false);
        templatize(&unwrapped, siblings).0
    }

    /// Every structural shape the classifier has to get right, in one corpus, so
    /// the whole-pipeline properties below are checked against all of them
    /// rather than against one hand-picked happy path.
    const SHAPES: &[&str] = &[
        // A code span that closes on the next line, then a prose mention.
        "Read it with `gh auth\ntoken --hostname github.com` and see dev now.\n",
        // A four-backtick fence quoting a three-backtick one.
        "````md\n```sh\ndev do\n```\n````\nThen see dev in prose.\n",
        // A tilde fence quoting a backtick one.
        "~~~text\n```\ndev\n~~~\nThen see do.\n",
        // A tilde fence carrying an info string.
        "~~~ruby\ndev\n~~~\nThen see dev.\n",
        // A line-initial backtick run that is a span, not a fence opener.
        "``a ` b`` is a span\n\nhand off to dev now\n",
        // A span crossing a line break inside one list item, and a second item.
        "- run `gh auth\n  token` then dev\n- and do\n",
        // A table: each row is its own block.
        "| a | b |\n|---|---|\n| `dev` | do |\n",
        // Frontmatter, a heading, and prose.
        "---\nname: dev\ndescription: see do\n---\n# Title\n\nsee do here\n",
        // Path-adjacent mentions, which are never prose.
        "# Heading\n\nsee ~/dev and dev/x and dev.\n",
        // A fence that never closes.
        "```\nunclosed fence\ndev\n",
        // Existing tokens in all three misplaced contexts.
        "prose {{ns:dev}} and `{{ns:do}}` and ~/{{ns:dev}}\n",
        // Brace pairs inside code spans.
        "`{{`  and `}}` then dev\n",
        // CRLF line endings.
        "a\r\n\r\nsee dev here\r\n",
        // A blockquote holding a fence: `>` keeps the fence from being seen.
        "> ```\n> dev\n> ```\n\nsee do after\n",
        // An indented code block whose content is a lone fence delimiter.
        "Open it with:\n\n    ```\n\nThen see dev.\n",
        // An over-indented lazy continuation of a paragraph.
        "This sentence wraps\n    onto dev here.\n",
        // A fence in an ordered item whose content column is four.
        "1.  step:\n\n    ```sh\n    dev do\n    ```\n\nThen see dev.\n",
        // An escaped backtick before a real code span.
        "Escape it with \\` in prose. Then dev and `do`.\n",
        // A fence left unclosed inside a list item the document then dedents out of.
        "- item:\n\n  ```sh\n  dev\n\nBack at top level, see do.\n",
        // A fence opened on the list-marker line itself.
        "Setup:\n\n- ```sh\n  dev\n  ```\n\n  Then see do.\n",
        // A setext underline, which ends the paragraph above it.
        "Type a lone ` in prose\n---\nThen see dev and run `do`.\n",
        // A thematic break, likewise.
        "Type a lone ` in prose\n***\nThen see dev and run `do`.\n",
        // An HTML block, which runs to the next blank line.
        "Type a lone ` in prose\n<div>\nThen see dev and run `do`.\n",
        // A `{{ns:}}` token written across a line break.
        "see {{ns:\ndev }} for the handoff, then do\n",
        // A blockquoted fence, and a blockquoted paragraph after it.
        "> ```sh\n> mind learn dev\n> ```\n>\n> Then see do.\n",
    ];

    #[test]
    fn fix_is_a_fixed_point_on_every_structural_shape() {
        // spec: NS-46 NS-47
        // What the reporter actually complained about: `--fix` re-dirtying files
        // on every run. Note what this does and does not prove. Because the pass
        // pair is wrap-after-un-wrap, a wrap that disagrees with the scan still
        // converges after one run (the next un-wrap removes exactly what the
        // last wrap added, and the wrap puts it back). What this pins down is
        // that inserting `{{ns:` / `}}` never changes the structure map itself,
        // so no rewrite can turn a line into a different kind of block. The
        // wrap/scan agreement claim is `templatize_only_ever_creates_prose_tokens`.
        let s = sibs(&["dev", "do"]);
        for doc in SHAPES {
            let once = fix_passes(doc, &s);
            let twice = fix_passes(&once, &s);
            assert_eq!(
                once, twice,
                "--fix is not idempotent on:\n{doc}\nfirst:\n{once}\nsecond:\n{twice}"
            );
        }
    }

    #[test]
    fn templatize_only_ever_creates_prose_tokens() {
        // spec: NS-46 NS-47
        // The same agreement from the other side: strip every token from a
        // shape, wrap it, and require the scan to call each token it created
        // prose. A token wrapping puts anywhere else is one `--fix` deletes.
        let s = sibs(&["dev", "do"]);
        for doc in SHAPES {
            let (bare, _) = unwrap_misplaced(doc, true);
            let (wrapped, _) = templatize(&bare, &s);
            for r in scan_ns_refs(&wrapped) {
                assert!(
                    !r.context.is_misplaced(),
                    "templatize wrapped {} into {:?} in:\n{wrapped}",
                    r.name,
                    r.context
                );
            }
        }
    }

    #[test]
    fn wrapping_and_the_token_scan_read_one_map_position_by_position() {
        // spec: NS-46 NS-47 NS-51
        // Agreement asserted directly, position by position, rather than
        // inferred from idempotence: the `--fix` pass pair is wrap-after-unwrap,
        // so a wrap that disagrees with the scan still converges after one run
        // and a fixed-point test cannot see it.
        //
        // For every whole-word sibling mention in every shape, ask the two
        // passes the same question about the same byte offset: did wrapping
        // rewrite this occurrence, and does the scan call a token written at
        // this offset prose? The two must answer alike. The frontmatter
        // `name:` field is the one remaining documented asymmetry (NS-24,
        // NS-56): a token there is a reference the scan calls
        // `FrontmatterName` (not `Prose`), so both sides already agree it is
        // not a wrap target and neither branch below needs to special-case it.
        // Every other frontmatter field agrees with prose now (NS-56).
        let s = sibs(&["dev", "do"]);
        for doc in SHAPES {
            let (bare, _) = unwrap_misplaced(doc, true);
            let (wrapped, _) = templatize(&bare, &s);
            let wrapped_at = wrapped_offsets(&bare, &wrapped);
            for name in ["dev", "do"] {
                for start in occurrences(&bare, name) {
                    let end = start + name.len();
                    let did_wrap = wrapped_at.contains(&start);
                    // What the scan says about a token written right here.
                    let probe = format!("{}{{{{ns:{name}}}}}{}", &bare[..start], &bare[end..]);
                    let scanned = scan_ns_refs(&probe)
                        .into_iter()
                        .find(|r| r.start == start)
                        .map(|r| r.context);
                    let scan_prose = scanned == Some(NsContext::Prose);
                    if did_wrap {
                        assert!(
                            scan_prose,
                            "wrapping created a token at {start} the scan calls \
                             {scanned:?} in:\n{bare}"
                        );
                    } else {
                        assert!(
                            !scan_prose,
                            "the scan calls a token at {start} prose but wrapping \
                             declined to create one in:\n{bare}"
                        );
                    }
                }
            }
        }
    }

    /// The byte offsets of `bare` that `templatize` wrapped, recovered by
    /// walking the original and the rewritten text in step. Doubles as a check
    /// that the only edit wrapping makes is inserting `{{ns:` and `}}` around a
    /// whole word: any other divergence fails here.
    fn wrapped_offsets(bare: &str, wrapped: &str) -> HashSet<usize> {
        let (b, w) = (bare.as_bytes(), wrapped.as_bytes());
        let mut out = HashSet::new();
        let (mut i, mut j) = (0usize, 0usize);
        while i < b.len() {
            if j < w.len() && b[i] == w[j] {
                i += 1;
                j += 1;
                continue;
            }
            assert!(
                wrapped[j..].starts_with("{{ns:"),
                "templatize changed a byte it did not wrap:\n{bare}\n{wrapped}"
            );
            j += "{{ns:".len();
            out.insert(i);
            while let Some(c) = bare[i..].chars().next() {
                if !is_word_char(c) {
                    break;
                }
                assert_eq!(&bare[i..i + c.len_utf8()], &wrapped[j..j + c.len_utf8()]);
                i += c.len_utf8();
                j += c.len_utf8();
            }
            assert!(
                wrapped[j..].starts_with("}}"),
                "an unterminated wrapper:\n{bare}\n{wrapped}"
            );
            j += 2;
        }
        assert_eq!(j, w.len(), "trailing bytes:\n{bare}\n{wrapped}");
        out
    }

    /// Byte offsets of every whole-word occurrence of `name` in `content`.
    fn occurrences(content: &str, name: &str) -> Vec<usize> {
        let mut out = Vec::new();
        let mut from = 0;
        while let Some(rel) = content[from..].find(name) {
            let start = from + rel;
            let end = start + name.len();
            let before = content[..start].chars().next_back();
            let after = content[end..].chars().next();
            if !before.is_some_and(is_word) && !after.is_some_and(is_word) {
                out.push(start);
            }
            from = end;
        }
        out
    }

    #[test]
    fn a_split_token_is_copied_verbatim_rather_than_wrapped_into() {
        // spec: NS-51
        // `expand` reads a `{{ns:}}` token document-wide, so one written across
        // a line break is a live reference at install time. Wrapping line by
        // line used to swallow the opening line looking for `}}`, read the next
        // line as ordinary prose, and wrap the name inside the token, producing
        // `{{ns:\n{{ns:dev}} }}` -- which `install` rejects as a bad reference,
        // so the source stops installing at all. The whole brace span is copied
        // verbatim now, whatever it spans.
        let s = sibs(&["dev", "do"]);
        for doc in [
            "see {{ns:\ndev }} for the handoff\n",
            "see {{ns:  \n  dev\n}} and {{tools:\ndev}} too\n",
        ] {
            let (out, _) = templatize(doc, &s);
            assert!(
                !out.contains("{{ns:{{ns:") && !out.contains("\n{{ns:dev}} }}"),
                "wrapping nested a token inside a split one: {out}"
            );
            assert_eq!(
                expand(&out, &Some("jk".into()), &s, &HashSet::new()),
                expand(doc, &Some("jk".into()), &s, &HashSet::new()),
                "the rewrite changed what install expands: {out}"
            );
        }
        // And the bare mention outside the span is still wrapped.
        let (out, n) = templatize("see {{ns:\ndev }} then do\n", &s);
        assert_eq!(n, 1, "{out}");
        assert_eq!(out, "see {{ns:\ndev }} then {{ns:do}}\n");
    }

    #[test]
    fn fix_clears_prose_mentions_behind_every_desyncing_fence_shape() {
        // spec: NS-48
        // NS-48 over the fence shapes that actually desynchronized the old
        // toggle, which needs an *odd* number of line-initial triple-backtick
        // lines. A balanced nested fence (four such lines) does not desync, so a
        // corpus built only from balanced shapes cannot fail when NS-47 breaks.
        let s = sibs(&["dev"]);
        for doc in [
            // Three: a four-backtick fence quoting one delimiter.
            "````md\n```\nreply here\n````\nThen see the {{ns:dev}} skill.\n",
            // One: a tilde fence quoting a backtick delimiter.
            "~~~md\n```\nreply here\n~~~\nThen see the {{ns:dev}} skill.\n",
            // The same with the mention bare: wrapping has to reach it.
            "~~~md\n```\nreply here\n~~~\nThen see the dev skill.\n",
            // One, unfenced: a triple-backtick *span* at the start of a line.
            // (A line-initial run followed by plain text, e.g. "``` is the
            // delimiter", is a real fence opener with an info string in
            // CommonMark, and is read as one here.)
            "```mind sync``` at line start\n\nThen see the {{ns:dev}} skill.\n",
        ] {
            let fixed = fix_passes(doc, &s);
            assert_eq!(
                unguarded_refs(&fixed, &s),
                Vec::<String>::new(),
                "--fix left an unguarded reference in: {fixed}"
            );
            assert!(
                fixed.contains("{{ns:dev}} skill"),
                "the prose reference must end up tokenized: {fixed}"
            );
        }
    }

    #[test]
    fn fix_clears_prose_mentions_past_an_indented_block_or_an_escaped_backtick() {
        // spec: NS-48 NS-49 NS-50
        // NS-48 over the two shapes that still misclassified after the
        // structural read landed. Both are destructive in the same way as the
        // filed report: a false fence opener (an indented ``` ) and a false span
        // opener (an escaped backtick) each swallow a prose token, and `--fix`
        // deletes its wrapper. Asserted through the whole `--fix` pass pair, in
        // both the already-tokenized and the still-bare direction.
        let s = sibs(&["dev"]);
        for doc in [
            // A lone fence delimiter shown inside an indented code block.
            "Wrap the reply in a fence, opened by:\n\n    ```\n\nThen see the {{ns:dev}} skill.\n",
            "Wrap the reply in a fence, opened by:\n\n    ```\n\nThen see the dev skill.\n",
            // An escaped backtick ahead of a real code span.
            "Escape it with \\` in prose. Then see the {{ns:dev}} skill, then run `mind sync`.\n",
            "Escape it with \\` in prose. Then see the dev skill, then run `mind sync`.\n",
        ] {
            let fixed = fix_passes(doc, &s);
            assert_eq!(
                unguarded_refs(&fixed, &s),
                Vec::<String>::new(),
                "--fix left an unguarded reference in: {fixed}"
            );
            assert!(
                fixed.contains("{{ns:dev}} skill"),
                "the prose reference must end up tokenized: {fixed}"
            );
        }
    }

    #[test]
    fn tilde_fences_take_an_info_string_and_close_only_on_a_bare_run() {
        // spec: NS-47
        // A tilde opener may carry an info string (and, unlike a backtick one,
        // may carry backticks in it); a run that carries trailing text is not a
        // closer, so the block runs on to the real one.
        let doc = "~~~ruby `x`\n{{ns:dev}}\n~~~ still open\n{{ns:do}}\n~~~\nprose {{ns:dev}}\n";
        assert_eq!(
            contexts(doc),
            vec![
                ("dev".into(), NsContext::CodeBlock),
                ("do".into(), NsContext::CodeBlock),
                ("dev".into(), NsContext::Prose),
            ]
        );
        // A longer closing run closes a shorter opener; the reverse does not.
        let doc = "~~~\n{{ns:dev}}\n~~~~~\nprose {{ns:do}}\n";
        assert_eq!(
            contexts(doc),
            vec![
                ("dev".into(), NsContext::CodeBlock),
                ("do".into(), NsContext::Prose),
            ]
        );
    }

    #[test]
    fn an_unclosed_fence_runs_to_the_end_of_the_document() {
        // spec: NS-47
        // CommonMark closes an unterminated fence at the end of the document,
        // and so does this: every later token is in code, and `--fix` un-wraps
        // it. That is correct for a genuinely unclosed fence, and it is why a
        // *false* opener (see the indented-fence tests) is destructive.
        let doc = "```sh\necho {{ns:dev}}\n\nstill fenced {{ns:do}}\n";
        assert_eq!(
            contexts(doc),
            vec![
                ("dev".into(), NsContext::CodeBlock),
                ("do".into(), NsContext::CodeBlock),
            ]
        );
        let (out, n) = unwrap_misplaced(doc, false);
        assert_eq!(n, 2, "both tokens are un-wrapped: {out}");
    }

    #[test]
    fn crlf_documents_classify_and_rewrite_like_lf_ones() {
        // spec: NS-46 NS-47
        // A carriage return rides along on every line; it must not break fence
        // matching (the run is followed by `\r`, not by end-of-line), span
        // matching, or byte-exact round-tripping of the line ending.
        let doc = "````md\r\n```sh\r\n{{ns:dev}}\r\n```\r\n````\r\n\r\n\
                   Read it with `gh auth\r\ntoken` then {{ns:do}} runs.\r\n";
        assert_eq!(
            contexts(doc),
            vec![
                ("dev".into(), NsContext::CodeBlock),
                ("do".into(), NsContext::Prose),
            ]
        );
        // Wrapping preserves CRLF byte for byte and reaches the prose mention.
        let s = sibs(&["dev", "do"]);
        let (out, n) = templatize("a\r\n\r\nRun `gh auth\r\ntoken` then dev.\r\n", &s);
        assert_eq!(n, 1, "{out}");
        assert_eq!(out, "a\r\n\r\nRun `gh auth\r\ntoken` then {{ns:dev}}.\r\n");
    }

    #[test]
    fn a_code_span_may_contain_brace_pairs() {
        // spec: NS-46
        // `{{` / `}}` inside a span are literal text, not token syntax: the span
        // still matches by backtick run, the token inside one is still code, and
        // a token after one is still prose.
        let doc = "see `{{ns:dev}}` and `a }} b` then {{ns:do}} here\n";
        assert_eq!(
            contexts(doc),
            vec![
                ("dev".into(), NsContext::CodeSpan),
                ("do".into(), NsContext::Prose),
            ]
        );
        // Wrapping steps over a lone brace pair in a span without losing the
        // prose mention that follows it.
        let s = sibs(&["dev"]);
        let (out, n) = templatize("`{{` and `}}` then dev\n", &s);
        assert_eq!(n, 1, "{out}");
        assert_eq!(out, "`{{` and `}}` then {{ns:dev}}\n");
    }

    #[test]
    fn a_token_on_a_fence_delimiter_line_is_not_scanned() {
        // spec: NS-47
        // A delimiter line is structure, not content. A token in an info string
        // is invisible to the scan, so `--fix` neither reports nor rewrites it
        // (`expand` still substitutes it at install time).
        let doc = "```{{ns:dev}}\nbody\n```\nprose {{ns:do}}\n";
        assert_eq!(contexts(doc), vec![("do".into(), NsContext::Prose)]);
        let (out, n) = unwrap_misplaced(doc, false);
        assert_eq!(n, 0, "nothing on a delimiter line is rewritten: {out}");
        assert_eq!(out, doc);
        // A run that carries text while a block is open is content, not a
        // closer, so a token on it *is* seen, as code.
        let doc = "```\n``` {{ns:dev}}\n```\nprose {{ns:do}}\n";
        assert_eq!(
            contexts(doc),
            vec![
                ("dev".into(), NsContext::CodeBlock),
                ("do".into(), NsContext::Prose),
            ]
        );
    }

    #[test]
    fn a_table_row_bounds_a_code_span() {
        // spec: NS-46
        // Rows are separate blocks: an unmatched backtick in one cannot pair
        // with one in the next and swallow the token between them.
        let doc = "| col | note |\n|---|---|\n| a ` b | see {{ns:dev}} |\n\
                   | c ` d | and {{ns:do}} |\n";
        assert_eq!(
            contexts(doc),
            vec![
                ("dev".into(), NsContext::Prose),
                ("do".into(), NsContext::Prose),
            ]
        );
        // A real span inside one cell still classifies its token as code.
        let doc = "| a | `{{ns:dev}}` | {{ns:do}} |\n";
        assert_eq!(
            contexts(doc),
            vec![
                ("dev".into(), NsContext::CodeSpan),
                ("do".into(), NsContext::Prose),
            ]
        );
    }

    #[test]
    fn an_indented_fence_line_opens_a_block() {
        // spec: NS-47 NS-49
        // A fence nested in a list item is indented to the item's content
        // column, and is still a fence. The indent allowance is measured from
        // that column (NS-49), not from column zero, which is what keeps this
        // working while a four-column indent at document level does not open a
        // fence (`an_indented_code_block_...` below).
        let doc = "- item:\n\n  ```sh\n  echo {{ns:dev}}\n  ```\n\nprose {{ns:do}}\n";
        assert_eq!(
            contexts(doc),
            vec![
                ("dev".into(), NsContext::CodeBlock),
                ("do".into(), NsContext::Prose),
            ]
        );
    }

    #[test]
    fn a_fence_opens_inside_a_list_item_whose_content_column_is_four() {
        // spec: NS-49
        // The case that rules out "just stop trimming indentation": an ordered
        // item written `1.  text` starts its content at column 4, so its fence
        // is indented four columns and must still open. Only four columns *past*
        // the item's own content column is indented code.
        let doc = "1.  step:\n\n    ```sh\n    echo {{ns:dev}}\n    ```\n\nprose {{ns:do}}\n";
        assert_eq!(
            contexts(doc),
            vec![
                ("dev".into(), NsContext::CodeBlock),
                ("do".into(), NsContext::Prose),
            ]
        );
        // Nested items compose: the inner item's content column is 4, so its
        // fence at column 4 opens too.
        let doc =
            "- outer\n  - inner\n\n    ```sh\n    echo {{ns:dev}}\n    ```\n\nprose {{ns:do}}\n";
        assert_eq!(
            contexts(doc),
            vec![
                ("dev".into(), NsContext::CodeBlock),
                ("do".into(), NsContext::Prose),
            ]
        );
        // And a line four columns past the inner item's content column is
        // indented code inside the item, not a fence.
        let doc = "- outer\n  - inner:\n\n        ```\n\n    still the item, {{ns:dev}}\n";
        assert_eq!(contexts(doc), vec![("dev".into(), NsContext::Prose)]);
    }

    #[test]
    fn an_indented_code_block_containing_a_lone_fence_line_is_not_a_fence() {
        // spec: NS-49
        // An indented code block (four columns, at document level) showing a
        // literal fence delimiter. CommonMark reads the ``` as content of the
        // indented block, and so does this: it opens nothing, so the token in
        // the prose below it is prose and `review --fix` leaves it alone.
        // Previously it opened a fence nothing closed, and every token after it
        // classified CodeBlock -- the same destructive shape as the filed bug.
        let doc = "Wrap the reply in a fence, opened by:\n\n    ```\n\n\
                   Then see the {{ns:dev}} skill.\n";
        assert_eq!(contexts(doc), vec![("dev".into(), NsContext::Prose)]);
        // The indented block is code, so a token *in* it is still misplaced and
        // a bare name in it is never wrapped.
        let doc = "Sample:\n\n    mind learn {{ns:dev}}\n\nprose {{ns:do}} here\n";
        assert_eq!(
            contexts(doc),
            vec![
                ("dev".into(), NsContext::CodeBlock),
                ("do".into(), NsContext::Prose),
            ]
        );
        let s = sibs(&["dev", "do"]);
        let (out, n) = templatize("Sample:\n\n    mind learn dev\n\nprose do here\n", &s);
        assert_eq!(n, 1, "only the prose mention is wrapped: {out}");
        assert!(out.contains("    mind learn dev\n"), "{out}");
    }

    #[test]
    fn an_unclosed_fence_in_a_list_item_ends_with_the_item() {
        // spec: NS-49
        // A fence's content is written from its list item's content column, so
        // a line that dedents past it leaves the item and closes the fence. An
        // unclosed fence inside an item would otherwise swallow the rest of the
        // document and `--fix` would de-tokenize every prose token below it --
        // the same destructive shape as an indented ``` opening a false fence.
        let doc = "- item:\n\n  ```sh\n  echo hi\n\nBack at top level, {{ns:dev}}.\n";
        assert_eq!(contexts(doc), vec![("dev".into(), NsContext::Prose)]);
        // The content of the fence is still code while the item holds it.
        let doc = "- item:\n\n  ```sh\n  echo {{ns:dev}}\n\ntop level {{ns:do}}\n";
        assert_eq!(
            contexts(doc),
            vec![
                ("dev".into(), NsContext::CodeBlock),
                ("do".into(), NsContext::Prose),
            ]
        );
        // A top-level fence has no such boundary: it still runs to the end of
        // the document (NS-47), because no dedent can leave column zero.
        let doc = "```sh\necho hi\n\nstill fenced {{ns:dev}}\n";
        assert_eq!(contexts(doc), vec![("dev".into(), NsContext::CodeBlock)]);
    }

    #[test]
    fn an_indented_line_that_continues_a_paragraph_is_still_prose() {
        // spec: NS-49
        // An indented code block cannot interrupt a paragraph, so a wrapped and
        // over-indented continuation line is prose, not code. Reading it as code
        // would be the same destructive un-wrap in the other direction.
        let doc = "This sentence runs long and wraps\n    onto {{ns:dev}} here.\n";
        assert_eq!(contexts(doc), vec![("dev".into(), NsContext::Prose)]);
        // A blank line closes the paragraph, so the same indent then *is* code.
        let doc = "This sentence ends.\n\n    onto {{ns:dev}} here.\n";
        assert_eq!(contexts(doc), vec![("dev".into(), NsContext::CodeBlock)]);
        // A heading leaves no paragraph open behind it.
        let doc = "# Title\n    {{ns:dev}} here\n";
        assert_eq!(contexts(doc), vec![("dev".into(), NsContext::CodeBlock)]);
    }

    #[test]
    fn a_backslash_escaped_backtick_does_not_open_a_code_span() {
        // spec: NS-50
        // CommonMark treats `\`` as a literal backtick, never a delimiter.
        // Previously it counted as a run, so it paired with the opener of the
        // next real span and everything between them -- including a prose token
        // -- read as code, which `review --fix` then deleted the wrapper of.
        let doc = "Escape it with \\` in prose. Then see {{ns:dev}} and run `mind sync`.\n";
        assert_eq!(contexts(doc), vec![("dev".into(), NsContext::Prose)]);
        // A doubled backslash escapes the backslash, not the backtick, so the
        // run is a real opener again and the token between the two is code.
        let doc = "Literal \\\\` opens. Then {{ns:dev}} and `x`.\n";
        assert_eq!(contexts(doc), vec![("dev".into(), NsContext::CodeSpan)]);
        // Escapes do not apply inside a span: an escaped backtick still closes
        // one that is already open, so the token after it is prose.
        let doc = "run `a \\` then {{ns:dev}} here\n";
        assert_eq!(contexts(doc), vec![("dev".into(), NsContext::Prose)]);
        // Wrapping agrees: the bare mention after an escaped backtick is prose.
        let s = sibs(&["dev"]);
        let (out, n) = templatize("Escape it with \\` then see dev and run `x`.\n", &s);
        assert_eq!(n, 1, "{out}");
        assert_eq!(out, "Escape it with \\` then see {{ns:dev}} and run `x`.\n");
    }

    #[test]
    fn fix_clears_mentions_that_used_to_be_permanently_unclearable() {
        // spec: NS-48 NS-55 NS-56
        // NS-48 used to list a sibling name inside a code span, inside a fence,
        // or in the frontmatter as three of its six known false positives:
        // `unguarded_refs` was context-free, so it reported all three both
        // before and after `--fix`, which had no move left to clear any of
        // them (un-wrapping put the first two there; wrapping never touched any
        // of the three). Structure-aware `unguarded_refs` (NS-55) no longer
        // reports a mention inside a code span or a fence at all -- it was
        // never a real reference there -- and `templatize` wrapping into a
        // non-`name:` frontmatter field (NS-56) clears the frontmatter case
        // instead of leaving it forever reported.
        let s = sibs(&["dev"]);
        for doc in [
            "run `{{ns:dev}}` now\n",
            "```sh\nmind learn {{ns:dev}}\n```\n",
            "---\nname: thing\ndescription: hand off to dev\n---\nbody\n",
        ] {
            let fixed = fix_passes(doc, &s);
            assert_eq!(
                unguarded_refs(&fixed, &s),
                Vec::<String>::new(),
                "no longer reported: {fixed}"
            );
        }
    }

    #[test]
    fn templatize_skips_code_paths_and_frontmatter() {
        // spec: NS-24 INIT-5
        let s = sibs(&["dev", "do"]);
        let doc = "---\nname: dev\n---\nuse dev here\n`dev`\n~/dev\n```\nfor x; do\n```\n";
        let (out, n) = templatize(doc, &s);
        assert_eq!(n, 1, "only the prose mention is wrapped: {out}");
        assert!(out.contains("use {{ns:dev}} here"), "{out}");
        assert!(out.contains("`dev`"), "code span untouched: {out}");
        assert!(out.contains("~/dev"), "path untouched: {out}");
        assert!(out.contains("for x; do"), "code block untouched: {out}");
        assert!(out.contains("name: dev"), "frontmatter untouched: {out}");
    }

    #[test]
    fn templatize_wraps_the_description_field_but_never_name() {
        // spec: NS-56
        // A sibling mention in the `description:` *value* is ordinary prose and
        // gets wrapped; the `name:` field itself stays unwrappable (NS-24) even
        // when its value happens to also be another sibling's name.
        let s = sibs(&["dev", "review"]);
        let doc = "---\nname: review\ndescription: hand off to dev when done\n---\nbody\n";
        let (out, n) = templatize(doc, &s);
        assert_eq!(n, 1, "only the description mention is wrapped: {out}");
        assert!(
            out.contains("description: hand off to {{ns:dev}} when done"),
            "{out}"
        );
        assert!(out.contains("name: review"), "name: stays bare: {out}");

        // Idempotent: a second pass changes nothing further.
        let (again, m) = templatize(&out, &s);
        assert_eq!(again, out);
        assert_eq!(m, 0);
    }

    #[test]
    fn templatize_never_wraps_a_structured_frontmatter_field() {
        // spec: NS-56 NS-24
        // The other half of NS-56, and the half that is destructive to get
        // wrong. `description:` is the only frontmatter field that is free
        // prose. Every other one is machine-read structure parsed out of *this*
        // source file, where no token is ever expanded: `requires:` is a list of
        // item refs (DEP-4/DEP-5) `catalog.rs` reads and `install.rs` validates,
        // and `build:`/`install:`/`uninstall:`/`bin:` are shell commands run
        // verbatim. Wrapping a sibling name into any of them writes a token
        // nothing expands, which breaks the field rather than templating it.
        let s = sibs(&["dev", "review", "detect", "description"]);
        for doc in [
            "---\nname: review\nrequires: agent:dev\n---\nbody\n",
            "---\nname: review\nbuild: make detect\n---\nbody\n",
            "---\nname: review\ninstall: ./setup dev\n---\nbody\n",
            "---\nname: review\nuninstall: ./teardown dev\n---\nbody\n",
            "---\nname: review\nbin: detect\n---\nbody\n",
            "---\nname: review\nmodel: dev\n---\nbody\n",
            // The `description:` KEY is not its value: a source that happens to
            // ship a sibling literally named `description` must not have the
            // key itself wrapped into `{{ns:description}}:`, which would destroy
            // the field outright.
            "---\nname: review\ndescription: a runner\n---\nbody\n",
        ] {
            assert_eq!(
                templatize(doc, &s),
                (doc.to_string(), 0),
                "a structured frontmatter field must never be wrapped: {doc}"
            );
        }
        // And the check agrees with wrapping (NS-48/NS-55): what wrapping
        // declines here is not reported as an unguarded reference either.
        assert_eq!(
            unguarded_refs("---\nname: review\nrequires: agent:dev\n---\nbody\n", &s),
            Vec::<String>::new()
        );
    }

    // ---- outside-in second pass over the structural scan (NS-46..NS-50) ------
    //
    // Written against CommonMark rather than against the implementation: each
    // case below states what a markdown renderer does with the document, and
    // requires the classifier to agree wherever disagreeing would make `--fix`
    // rewrite prose. A `#[ignore]`d test here is a demonstrated defect, not a
    // style preference; its comment says which way the miss goes.

    /// Un-wrapping is what `--fix` does to a token it calls code, so the whole
    /// question for any document is: which tokens survive it?
    fn surviving(doc: &str) -> String {
        unwrap_misplaced(doc, false).0
    }

    #[test]
    fn a_fence_opens_at_the_content_column_of_every_list_marker_shape() {
        // spec: NS-49
        // The indent baseline, probed through documents rather than through a
        // helper: every list-marker shape puts its item's content in a
        // different column, and a fence written from that column is a fence
        // while the prose after the item is prose. The old classifier computed
        // that column by hand; the parser owns it now, so the assertion is
        // about the outcome and not about the arithmetic.
        for (marker, indent) in [
            ("- ", "  "),
            ("* ", "  "),
            ("+ ", "  "),
            ("-   ", "    "),
            ("1. ", "   "),
            ("1) ", "   "),
            ("10. ", "    "),
            ("1.  ", "    "),
            ("123456789. ", "           "),
            ("\t- ", "\t  "),
        ] {
            let doc = format!(
                "{marker}step:\n\n{indent}```sh\n{indent}echo {{{{ns:dev}}}}\n\
                 {indent}```\n\nprose {{{{ns:do}}}}\n"
            );
            assert_eq!(
                contexts(&doc),
                vec![
                    ("dev".into(), NsContext::CodeBlock),
                    ("do".into(), NsContext::Prose)
                ],
                "marker {marker:?}:\n{doc}"
            );
        }
        // Not markers: the text after them is an ordinary paragraph, so a
        // four-column line under one is a lazy continuation, not a fence base.
        for line in ["-item", "1.step", "text"] {
            let doc = format!("{line}\n\n    ```\n\nThen see {{{{ns:dev}}}}.\n");
            assert_eq!(
                contexts(&doc),
                vec![("dev".into(), NsContext::Prose)],
                "{line:?} opens no list, so the indented ``` is code, not a fence"
            );
        }
    }

    #[test]
    fn a_tab_indented_block_is_code_at_document_level() {
        // spec: NS-49
        // The column count is only worth measuring if it reaches the decision:
        // one tab is a full indent, so the block below is code and its token is
        // un-wrapped, while the paragraph after it keeps its own. A tab counted
        // as one column would make the sample prose and let wrapping rewrite
        // the names in it.
        let doc = "Sample:\n\n\tmind learn {{ns:dev}}\n\nprose {{ns:do}}\n";
        assert_eq!(
            contexts(doc),
            vec![
                ("dev".into(), NsContext::CodeBlock),
                ("do".into(), NsContext::Prose)
            ]
        );
        let s = sibs(&["dev", "do"]);
        let (out, n) = templatize("Sample:\n\n\tmind learn dev\n\nprose do\n", &s);
        assert_eq!(n, 1, "only the prose mention is wrapped: {out}");
        assert!(out.contains("\tmind learn dev\n"), "{out}");
    }

    #[test]
    fn a_blank_line_keeps_a_list_item_open_and_a_dedent_closes_it() {
        // spec: NS-49
        // The container stack, probed at its two decision points. An item holds
        // its blank lines, so the block after one is measured from the item's
        // content column; a non-blank line at a lower column leaves the item,
        // and leaves every item nested inside it.
        //
        // Item content at column 2 with a fence written at column 2: inside.
        let doc = "- item:\n\n  ```sh\n  echo {{ns:a}}\n  ```\n\n  Then {{ns:b}}.\n";
        assert_eq!(
            contexts(doc),
            vec![
                ("a".into(), NsContext::CodeBlock),
                ("b".into(), NsContext::Prose)
            ],
            "a blank line inside the item does not end it"
        );
        // Two levels of nesting, then one line back at column 0: both pop, so
        // the four-column line that follows is indented code again.
        let doc = "- outer\n  - inner\n\n    text {{ns:a}}\n\nback at zero\n\n    {{ns:b}} here\n";
        assert_eq!(
            contexts(doc),
            vec![
                ("a".into(), NsContext::Prose),
                ("b".into(), NsContext::CodeBlock)
            ],
            "the inner item's content column is 4, so `    text` is prose; after \
             the dedent the baseline is 0 again, so the same indent is code"
        );
        // Sibling items at the same column do not stack a deeper baseline: the
        // eight-column line under the second item is still indented code.
        let doc = "- one\n- two\n- three\n\n        {{ns:a}} deep\n";
        assert_eq!(contexts(doc), vec![("a".into(), NsContext::CodeBlock)]);
    }

    #[test]
    fn an_escape_never_reaches_across_a_line_break() {
        // spec: NS-50
        // The backslash scan walks backwards over bytes; a line ending in a
        // backslash (a hard line break) must not escape the first backtick of
        // the next line, or a real span opener would be dropped and the token
        // *inside* it would read as prose and survive a `--fix` that should
        // have un-wrapped it. Checked in the direction that matters: the token
        // between the two runs is code.
        let doc = "A trailing backslash \\\nthen `run {{ns:dev}} now` here\n";
        assert_eq!(contexts(doc), vec![("dev".into(), NsContext::CodeSpan)]);
        // The same with CRLF: the byte before the backtick is `\n`, not `\\`.
        let doc = "A trailing backslash \\\r\nthen `run {{ns:dev}} now` here\r\n";
        assert_eq!(contexts(doc), vec![("dev".into(), NsContext::CodeSpan)]);
        // A hard line break written as two trailing spaces changes nothing.
        let doc = "Wrapped line  \nthen `run {{ns:dev}} now` here\n";
        assert_eq!(contexts(doc), vec![("dev".into(), NsContext::CodeSpan)]);
        // A backslash at the very start of the paragraph escapes the backtick
        // right after it, and the scan stops at the paragraph boundary rather
        // than reading backslashes out of the block above.
        let doc =
            "para one ends \\\n\n\\` opens nothing, so {{ns:dev}} is prose and `x` is a span\n";
        assert_eq!(contexts(doc), vec![("dev".into(), NsContext::Prose)]);
    }

    #[test]
    fn an_escaped_backtick_closes_an_open_span_and_is_literal_after_it() {
        // spec: NS-50
        // The asymmetry, driven past the first span: escapes do not apply inside
        // a span, so `\`` closes one; the run after the close is a fresh opener
        // and *is* escape-checked, so an escaped one there opens nothing.
        let doc = "run `a \\` then {{ns:one}} and \\` still prose {{ns:two}} and `x`\n";
        assert_eq!(
            contexts(doc),
            vec![
                ("one".into(), NsContext::Prose),
                ("two".into(), NsContext::Prose)
            ],
            "the span is `a \\`; both later tokens sit in prose"
        );
        // Two backslashes escape the backslash, so the backtick is a live
        // opener again and the token between it and the next run is code.
        let doc = "literal \\\\` opens a span with {{ns:dev}} in it `\n";
        assert_eq!(contexts(doc), vec![("dev".into(), NsContext::CodeSpan)]);
        // Three backslashes: the backtick is escaped again.
        let doc = "literal \\\\\\` opens nothing, {{ns:dev}} is prose, `x` is a span\n";
        assert_eq!(contexts(doc), vec![("dev".into(), NsContext::Prose)]);
    }

    #[test]
    fn a_word_after_a_multibyte_character_is_measured_in_bytes() {
        // spec: NS-46
        // Code-span ranges are byte offsets into the document, while `wrap_line`
        // walks the line by `char`. A single multi-byte character ahead of a
        // sibling name shifts one against the other, so a mention inside a span
        // would be wrapped (corrupting the sample) and one outside skipped.
        let doc = "R\u{e9}sum\u{e9}: run `mind learn {{ns:dev}}` then see {{ns:do}}.\n";
        assert_eq!(
            contexts(doc),
            vec![
                ("dev".into(), NsContext::CodeSpan),
                ("do".into(), NsContext::Prose)
            ]
        );
        let s = sibs(&["dev"]);
        let (out, n) = templatize("R\u{e9}sum\u{e9}: run `dev` then see dev.\n", &s);
        assert_eq!(n, 1, "only the mention outside the span is wrapped: {out}");
        assert_eq!(out, "R\u{e9}sum\u{e9}: run `dev` then see {{ns:dev}}.\n");
        // The same across a line break, where the offset is the line's own base.
        let (out, n) = templatize("caf\u{e9} `gh auth\ntoken` then dev.\n", &s);
        assert_eq!(n, 1, "{out}");
        assert_eq!(out, "caf\u{e9} `gh auth\ntoken` then {{ns:dev}}.\n");
    }

    #[test]
    fn a_spaced_thematic_break_is_a_break_and_not_a_list_marker() {
        // spec: NS-49
        // `* * *` is a thematic break, and opens no container. The hand-rolled
        // classifier read it as a list marker and measured every line under it
        // from column 2, which made an indented code sample prose (wrapping
        // could rewrite a name inside it) and a two-column fence a no-op. Both
        // now read the way a renderer reads them, so both flip to code.
        let doc = "intro\n\n* * *\n\n    mind learn {{ns:dev}}\n";
        assert_eq!(
            contexts(doc),
            vec![("dev".into(), NsContext::CodeBlock)],
            "four columns under a break is an indented code block"
        );
        let s = sibs(&["dev"]);
        let (out, n) = templatize("intro\n\n* * *\n\n    mind learn dev\n", &s);
        assert_eq!(n, 0, "nothing in the code sample is wrapped: {out}");
        // A fence written at column 2 under the break is a top-level fence with
        // no container to end it, so it runs to the end of the document
        // (NS-47), exactly as one written at column 0 does.
        let doc = "intro\n\n* * *\n\n  ```\n\nThen see {{ns:dev}}.\n";
        assert_eq!(contexts(doc), vec![("dev".into(), NsContext::CodeBlock)]);
        // Closed, it bounds nothing after itself.
        let doc = "intro\n\n* * *\n\n  ```\n  echo hi\n  ```\n\nThen see {{ns:dev}}.\n";
        assert_eq!(contexts(doc), vec![("dev".into(), NsContext::Prose)]);
    }

    #[test]
    fn a_fence_opened_on_a_list_marker_line_is_still_a_fence() {
        // spec: NS-47 NS-49
        // A fenced block may open on the same line as the list marker. The
        // classifier only looks for a delimiter at the *start* of a line, so it
        // reads the opener as prose and then reads the real closer as an
        // opener: from there the rest of the item is a code block and `--fix`
        // deletes every token in it.
        let doc = "Setup:\n\n- ```sh\n  mind sync\n  ```\n\n  Then see the {{ns:dev}} skill.\n";
        // NS-48 fails here too: every sibling mention in this document sits in
        // prose, yet `--fix` leaves a bare one behind.
        let s = sibs(&["dev"]);
        assert_eq!(
            unguarded_refs(&fix_passes(doc, &s), &s),
            Vec::<String>::new(),
            "NS-48"
        );
        assert_eq!(
            contexts(doc),
            vec![("dev".into(), NsContext::Prose)],
            "the item's trailing paragraph is prose, not code"
        );
        assert_eq!(surviving(doc), doc, "`--fix` must not touch it");
    }

    #[test]
    fn a_thematic_break_ends_the_paragraph_a_code_span_may_cross() {
        // spec: NS-46
        // A code span may cross a line break inside one paragraph, so the run
        // it is matched over has to stop at every block boundary. A thematic
        // break (`***`) and a setext underline (`---`) both end the paragraph
        // above them, and neither is recognized, so a lone literal backtick in
        // that paragraph pairs with the opener of a real span two blocks later
        // and everything between them -- including a prose token -- reads as a
        // code span that `--fix` un-wraps.
        let doc = "Type a lone ` in prose\n---\nThen see {{ns:dev}} and run `mind sync`.\n";
        // NS-48 fails on the same documents: the mention is prose, and `--fix`
        // hands back the bare name the unguarded-reference check reports.
        let s = sibs(&["dev"]);
        assert_eq!(
            unguarded_refs(&fix_passes(doc, &s), &s),
            Vec::<String>::new(),
            "NS-48"
        );
        assert_eq!(
            contexts(doc),
            vec![("dev".into(), NsContext::Prose)],
            "setext"
        );
        assert_eq!(surviving(doc), doc);
        let doc = "Type a lone ` in prose\n***\nThen see {{ns:dev}} and run `mind sync`.\n";
        assert_eq!(
            contexts(doc),
            vec![("dev".into(), NsContext::Prose)],
            "break"
        );
        assert_eq!(surviving(doc), doc);
        // An HTML block interrupts a paragraph the same way.
        let doc = "Type a lone ` in prose\n<div>\nThen see {{ns:dev}} and run `mind sync`.\n";
        assert_eq!(
            contexts(doc),
            vec![("dev".into(), NsContext::Prose)],
            "html"
        );
        assert_eq!(surviving(doc), doc);
        // NS-48 fails on the same documents: the mention is prose, and `--fix`
        // hands back the bare name the unguarded-reference check reports.
        let s = sibs(&["dev"]);
        assert_eq!(
            unguarded_refs(&fix_passes(doc, &s), &s),
            Vec::<String>::new()
        );
    }

    #[test]
    fn a_setext_heading_leaves_no_paragraph_open_behind_it() {
        // spec: NS-49
        // The lazy-continuation rule asks whether a paragraph is open above an
        // indented line. A setext heading closes the paragraph it is made of,
        // so the indented block after one is code; reading it as a lazy
        // continuation makes it prose, and wrapping then rewrites a bare name
        // inside a code sample.
        let doc = "Title\n---\n    mind learn {{ns:dev}}\n";
        assert_eq!(contexts(doc), vec![("dev".into(), NsContext::CodeBlock)]);
        let s = sibs(&["dev"]);
        let (out, n) = templatize("Title\n---\n    mind learn dev\n", &s);
        assert_eq!(n, 0, "nothing in the code block is wrapped: {out}");
        // A thematic break closes the paragraph above it for the same reason.
        let doc = "Some prose\n***\n    mind learn {{ns:dev}}\n";
        assert_eq!(contexts(doc), vec![("dev".into(), NsContext::CodeBlock)]);
    }

    #[test]
    fn a_token_broken_across_a_line_break_is_left_alone() {
        // spec: NS-24 NS-46 NS-51
        // `expand` finds a `{{ns:}}` token document-wide, so a token split over
        // a line break is a live reference at install time. The line-by-line
        // wrapper swallowed the rest of the opening line looking for `}}`, then
        // treated the next line as ordinary prose and wrapped the name inside
        // the token, producing `{{ns:\n{{ns:dev}}}}`, which `install` rejects as
        // a bad reference (the source stops installing at all). The whole brace
        // span is copied verbatim now (NS-51), so the token is left as written.
        let s = sibs(&["dev"]);
        let doc = "see {{ns:\ndev }} for the handoff\n";
        let (out, n) = templatize(doc, &s);
        assert_eq!(n, 0, "a split token is left alone, not nested: {out}");
        assert_eq!(out, doc);
    }

    #[test]
    fn a_fence_inside_a_blockquote_is_a_fence() {
        // spec: NS-47
        // Quoted code is code for both passes. `> ` before the delimiter used
        // to hide it, so the quoted block classified as prose throughout: that
        // cannot delete a token, but it is the same defect pointed the other
        // way, since wrapping rewrites a bare sibling name inside quoted code
        // and the rewritten sample is wrong for the reader and expands to a
        // prefixed name at install.
        let s = sibs(&["dev"]);
        let (out, n) = templatize("> ```sh\n> mind learn dev\n> ```\n", &s);
        assert_eq!(n, 0, "quoted code is not prose: {out}");
        assert_eq!(out, "> ```sh\n> mind learn dev\n> ```\n");
        let doc = "> ```sh\n> mind learn {{ns:dev}}\n> ```\n\nThen see {{ns:do}}.\n";
        assert_eq!(
            contexts(doc),
            vec![
                ("dev".into(), NsContext::CodeBlock),
                ("do".into(), NsContext::Prose)
            ]
        );
        // An indented code block inside a blockquote is code too, and quoted
        // prose around it is still prose (so its mentions are still wrapped).
        let doc = "> Sample:\n>\n>     mind learn {{ns:dev}}\n>\n> Then see {{ns:do}}.\n";
        assert_eq!(
            contexts(doc),
            vec![
                ("dev".into(), NsContext::CodeBlock),
                ("do".into(), NsContext::Prose)
            ]
        );
        let (out, n) = templatize(
            "> Sample:\n>\n>     mind learn dev\n>\n> Then see do.\n",
            &s,
        );
        assert_eq!(n, 0, "only `do` is a sibling here: {out}");
        let s2 = sibs(&["dev", "do"]);
        let (out, n) = templatize(
            "> Sample:\n>\n>     mind learn dev\n>\n> Then see do.\n",
            &s2,
        );
        assert_eq!(n, 1, "the quoted prose mention is wrapped: {out}");
        assert_eq!(
            out,
            "> Sample:\n>\n>     mind learn dev\n>\n> Then see {{ns:do}}.\n"
        );
    }

    #[test]
    fn unwrap_misplaced_restores_words() {
        // spec: NS-24
        let doc = "prose {{ns:dev}}\n`{{ns:test}}`\n~/{{ns:dev}}\n";
        let (out, n) = unwrap_misplaced(doc, false);
        assert_eq!(
            n, 2,
            "code-span and path tokens un-wrapped, prose kept: {out}"
        );
        assert_eq!(out, "prose {{ns:dev}}\n`test`\n~/dev\n");
        // all_code: every token is misplaced.
        let (all, m) = unwrap_misplaced(doc, true);
        assert_eq!(m, 3);
        assert_eq!(all, "prose dev\n`test`\n~/dev\n");
    }

    // ---- outside-in third pass: constructs the parser swap did not exercise --
    //
    // Every case below is stated as what CommonMark says about the document,
    // derived from the spec and not from the classifier's output, and then
    // asserted in the direction that would corrupt a file: a prose token must
    // survive `--fix`, a token in code must still be un-wrapped, and wrapping
    // must never rewrite a name inside something a reader reads as code.

    /// Does `templatize` rewrite `name` in `doc`? The one question wrapping
    /// answers, asked without the caller having to spell out the whole rewrite.
    fn wraps(doc: &str, name: &str) -> bool {
        let (out, n) = templatize(doc, &sibs(&[name]));
        assert!(n <= 1, "more than one wrap in:\n{doc}\n{out}");
        n == 1
    }

    #[test]
    fn an_html_block_is_prose_and_its_preformatted_content_is_not_protected() {
        // spec: NS-46 NS-47
        // Characterization, not endorsement. A raw HTML block is not a code
        // block in CommonMark and the parser reports it as HTML, so the map
        // leaves it prose: a token in one survives `--fix` (the safe
        // direction), and a bare name in one is wrapped. That is right for a
        // `<div>` wrapper around prose and wrong for `<pre>`, whose content a
        // reader reads as a code sample exactly like a fence's. See the
        // report: the `<pre>` leg is a known gap, pinned here so a change to it
        // is a decision.
        //
        // The decision, taken with the NS-52 link fix and recorded here: leave
        // it. Calling a `<pre>`-led HTML block code would newly *delete* tokens
        // inside it (`--fix` un-wraps what it calls code), which is the
        // destructive direction, and the rule available is only CommonMark's
        // type-1 block condition -- a block that *begins* with `<pre>`. A
        // `<pre>` opened inside a `<div>` block, or inline in a paragraph, would
        // stay prose, so the inconsistency would survive in a shape at least as
        // common as the one fixed. NS-52 is the opposite case: a link is a
        // construct the parser hands over whole, so the rule is complete and
        // costs no deletion the destination did not already invite.
        let doc = "<div align=\"center\">\nSee {{ns:dev}} for details.\n</div>\n";
        assert_eq!(
            contexts(doc),
            vec![("dev".into(), NsContext::Prose)],
            "a token inside an HTML block survives --fix"
        );
        assert_eq!(surviving(doc), doc);
        // Wrapped, because the block is prose to the map.
        assert!(wraps("<div>\nHand off to dev.\n</div>\n", "dev"));
        // The gap: `<pre>` content is preformatted, and wrapping rewrites it.
        assert!(
            wraps("Sample:\n\n<pre>\nmind learn dev\n</pre>\n", "dev"),
            "known gap: a `<pre>` code sample is not protected the way a fence is"
        );
        // Inline HTML is prose either way, and a token beside it is prose.
        let doc = "A <b>bold</b> word, then {{ns:dev}} here.\n";
        assert_eq!(contexts(doc), vec![("dev".into(), NsContext::Prose)]);
        assert_eq!(surviving(doc), doc);
    }

    #[test]
    fn an_html_block_bounds_the_paragraph_a_code_span_may_cross() {
        // spec: NS-46
        // A type-6 HTML block interrupts a paragraph, so an unmatched backtick
        // above one cannot pair with the opener of a real span below it. The
        // token between them is prose and `--fix` must leave it alone.
        let doc = "Type a lone ` in prose\n<div>\n</div>\n\n\
                   Then see {{ns:dev}} and run `mind sync`.\n";
        assert_eq!(contexts(doc), vec![("dev".into(), NsContext::Prose)]);
        assert_eq!(surviving(doc), doc);
    }

    #[test]
    fn a_link_is_prose_and_its_text_is_what_wrapping_is_meant_to_reach() {
        // spec: NS-24 INIT-5
        // A link carries no code, so a token anywhere in one is prose and
        // `--fix` deletes nothing: that is the direction that matters. A URL
        // path segment is held back by the path-adjacency rule rather than by
        // the structure map, and the link text is genuine prose that wrapping
        // is supposed to rewrite.
        let doc = "See [the docs](https://example.com/x) and {{ns:dev}}.\n";
        assert_eq!(contexts(doc), vec![("dev".into(), NsContext::Prose)]);
        assert_eq!(surviving(doc), doc);
        assert!(
            !wraps("See [the docs](https://example.com/dev).\n", "dev"),
            "a name after a `/` is path-adjacent and is left alone"
        );
        let (out, n) = templatize("See [the dev skill](x.md).\n", &sibs(&["dev"]));
        assert_eq!(n, 1);
        assert_eq!(out, "See [the {{ns:dev}} skill](x.md).\n");
    }

    #[test]
    fn wrapping_leaves_link_syntax_alone() {
        // spec: NS-24 NS-52 INIT-5
        // The structure map calls a byte code only when a code block or a code
        // span claims it, so everything a link is made of except its text is
        // prose to wrapping: `[label]: url` at the top of a document, the
        // `[label]` of a reference link, and a relative destination. Wrapping a
        // sibling name in any of them rewrites markdown syntax in the author's
        // working tree -- the reference stops resolving and the file renders
        // with a literal `[{{ns:name}}]` -- which is a file rewrite of the same
        // class as the misclassified fences, reached through the wrapping pass
        // instead of the un-wrapping one.
        let s = sibs(&["dev"]);
        for doc in [
            "[dev]: https://example.com/x\n\nSee [the docs][dev].\n",
            "See [the docs](dev.md).\n",
        ] {
            assert_eq!(
                templatize(doc, &s),
                (doc.to_string(), 0),
                "link syntax must not be rewritten"
            );
        }
    }

    #[test]
    fn every_part_of_a_link_but_its_text_is_syntax() {
        // spec: NS-52
        // The rule the map now states, walked over each part of a link in turn.
        // A destination, a title, a reference label, and a link reference
        // definition are markdown syntax: a sibling name in one is not a prose
        // reference and wrapping must decline it. The visible text of an inline
        // or full-reference link is prose and stays wrappable, because a sibling
        // named there is a real reference to it.
        let s = sibs(&["dev"]);
        for doc in [
            // Inline destination, relative and absolute, with and without a
            // path separator to fall back on.
            "See [the docs](dev.md).\n",
            "See [the docs](docs/dev.md).\n",
            "See [the docs](https://example.com/x#dev).\n",
            // Title, in each of CommonMark's three quotings.
            "See [the docs](x.md \"the dev notes\").\n",
            "See [the docs](x.md 'the dev notes').\n",
            "See [the docs](x.md (the dev notes)).\n",
            // Reference label, and the definition that resolves it.
            "See [the docs][dev].\n\n[dev]: https://example.com/x\n",
            "[dev]: https://example.com/x \"the dev notes\"\n\nSee [the docs][dev].\n",
            // A shortcut and a collapsed link resolve by their own text, so the
            // text is the label and rewriting it breaks the link too.
            "[dev]: https://example.com/x\n\nSee [dev] for details.\n",
            "[dev]: https://example.com/x\n\nSee [dev][] for details.\n",
            // An image is the same construct: its destination is a path.
            "![the diagram](dev.png)\n",
            // An email autolink renders its own destination as its text.
            "Mail <dev@example.com> about it.\n",
            // A URL autolink does too. This spelling puts the name where no
            // path separator touches it, so the path rule (NS-24) cannot be
            // what declines the wrap: only the autolink rule can.
            "See <https://example.com?q=dev> now.\n",
            "Mail <mailto:dev@example.com> about it.\n",
        ] {
            assert_eq!(
                templatize(doc, &s),
                (doc.to_string(), 0),
                "link syntax must not be rewritten:\n{doc}"
            );
        }
        // The other direction, so the rule is not "links are untouchable": the
        // visible text of an inline link and of a full reference link is prose,
        // and a sibling named there is wrapped.
        let (out, n) = templatize("See [the dev skill](x.md).\n", &s);
        assert_eq!(n, 1, "{out}");
        assert_eq!(out, "See [the {{ns:dev}} skill](x.md).\n");
        let (out, n) = templatize("See [the dev skill][docs].\n\n[docs]: x.md\n", &s);
        assert_eq!(n, 1, "{out}");
        assert_eq!(out, "See [the {{ns:dev}} skill][docs].\n\n[docs]: x.md\n");
        // Emphasis inside link text does not make the text syntax again.
        let (out, n) = templatize("See [the **dev** skill](x.md).\n", &s);
        assert_eq!(n, 1, "{out}");
        assert_eq!(out, "See [the **{{ns:dev}}** skill](x.md).\n");
        // And a link is inline structure, so nothing after it changes: the
        // paragraph beside one is still prose.
        let (out, n) = templatize("See [the docs](dev.md), then dev.\n", &s);
        assert_eq!(n, 1, "only the mention outside the link: {out}");
        assert_eq!(out, "See [the docs](dev.md), then {{ns:dev}}.\n");
    }

    #[test]
    fn a_token_in_link_syntax_is_misplaced_and_fix_takes_it_out() {
        // spec: NS-52 NS-24
        // The un-wrapping direction of the same rule. A `{{ns:}}` token in a
        // destination or a label expands at install to the referent's effective
        // name (NS-11), which under a prefix is a destination and a label that
        // no longer resolve -- the same failure a token beside a path separator
        // has, so it is reported as one and `--fix` takes it back out. The two
        // passes agree afterwards, since wrapping declines to put it back.
        let s = sibs(&["dev"]);
        for (doc, fixed) in [
            (
                "See [the docs]({{ns:dev}}.md) now.\n",
                "See [the docs](dev.md) now.\n",
            ),
            (
                "See [the docs](x.md \"{{ns:dev}} notes\") now.\n",
                "See [the docs](x.md \"dev notes\") now.\n",
            ),
            (
                "[{{ns:dev}}]: https://example.com/x\n\nSee [the docs][{{ns:dev}}].\n",
                "[dev]: https://example.com/x\n\nSee [the docs][dev].\n",
            ),
        ] {
            assert!(
                contexts(doc)
                    .iter()
                    .all(|(name, ctx)| name == "dev" && *ctx == NsContext::Path),
                "every token here is misplaced link syntax: {:?}",
                contexts(doc)
            );
            assert_eq!(surviving(doc), fixed, "`--fix` must un-wrap it");
            assert_eq!(fix_passes(doc, &s), fixed, "and must not put it back");
            assert_eq!(fix_passes(fixed, &s), fixed, "idempotent");
        }
    }

    #[test]
    fn a_link_only_mention_is_no_longer_a_false_positive() {
        // spec: NS-48 NS-52 NS-55
        // A name reachable only as a destination or a label: wrapping still has
        // no move on it, since wrapping it would break the link, but
        // structure-aware `unguarded_refs` (NS-55) now reads the same link-syntax
        // cell and no longer reports it either -- it was never a real reference
        // there. NS-48 no longer names this a false positive: there is nothing
        // left to report.
        let s = sibs(&["dev"]);
        for doc in [
            "See [the docs](dev.md).\n",
            "[dev]: https://example.com/x\n\nSee [the docs][dev].\n",
        ] {
            let fixed = fix_passes(doc, &s);
            assert_eq!(fixed, doc, "wrapping must not rewrite link syntax: {fixed}");
            assert_eq!(
                unguarded_refs(&fixed, &s),
                Vec::<String>::new(),
                "link syntax is not a reference and is no longer reported: {fixed}"
            );
        }
    }

    #[test]
    fn a_list_only_interrupts_a_paragraph_when_commonmark_says_it_does() {
        // spec: NS-49
        // A bullet item, and an ordered item numbered 1, may interrupt a
        // paragraph; an ordered item numbered anything else may not, so it is a
        // lazy continuation of the paragraph and opens no container. That
        // decides the indent baseline for the block below, and so decides
        // whether a four-column code sample is a code sample.
        //
        // `2.` does not interrupt: the baseline stays 0, four columns is an
        // indented code block, and the name in it must not be wrapped.
        let doc = "Intro line\n2. Step:\n\n    mind learn {{ns:dev}}\n";
        assert_eq!(contexts(doc), vec![("dev".into(), NsContext::CodeBlock)]);
        assert!(!wraps(
            "Intro line\n2. Step:\n\n    mind learn dev\n",
            "dev"
        ));
        // `1.` does interrupt: content starts at column 3, so a line at column
        // 4 is one past it -- a paragraph inside the item, which is prose.
        let doc = "Intro line\n1. Step:\n\n    mind learn {{ns:dev}}\n";
        assert_eq!(contexts(doc), vec![("dev".into(), NsContext::Prose)]);
        assert!(wraps("Intro line\n1. Step:\n\n    mind learn dev\n", "dev"));
        // A bullet interrupts too, and eight columns is four past its content
        // column, so the sample inside the item is code again.
        let doc = "Intro line\n- Step:\n\n      mind learn {{ns:dev}}\n";
        assert_eq!(contexts(doc), vec![("dev".into(), NsContext::CodeBlock)]);
    }

    #[test]
    fn an_atx_heading_is_prose_and_bounds_an_inline_run() {
        // spec: NS-46
        // A heading's closing hash sequence is not content, and a heading is
        // its own block: an unmatched backtick in one cannot pair with a run in
        // the paragraph after it, so the token between them is prose.
        let doc = "## Hand off to {{ns:dev}} ##\n";
        assert_eq!(contexts(doc), vec![("dev".into(), NsContext::Prose)]);
        let (out, n) = templatize("## Hand off to dev ##\n", &sibs(&["dev"]));
        assert_eq!(n, 1);
        assert_eq!(out, "## Hand off to {{ns:dev}} ##\n");
        let doc = "# A lone ` in a heading\nThen see {{ns:dev}} and run `x`.\n";
        assert_eq!(contexts(doc), vec![("dev".into(), NsContext::Prose)]);
        assert_eq!(surviving(doc), doc);
        // Seven hashes is not a heading, and no space after the hashes is not
        // one either: both are ordinary paragraphs, and still prose.
        for doc in ["####### {{ns:dev}} here\n", "#{{ns:dev}} here\n"] {
            assert_eq!(
                contexts(doc),
                vec![("dev".into(), NsContext::Prose)],
                "{doc}"
            );
        }
    }

    #[test]
    fn a_nested_blockquote_carries_its_code_block_like_any_other_container() {
        // spec: NS-47 NS-49
        // One `>` was already covered; two is where a container stack that
        // only tracks one level shows up. The quoted fence is code and the
        // quoted prose after it is prose, in both directions.
        let doc = "> > ```sh\n> > mind learn {{ns:dev}}\n> > ```\n>\n> Then see {{ns:do}}.\n";
        assert_eq!(
            contexts(doc),
            vec![
                ("dev".into(), NsContext::CodeBlock),
                ("do".into(), NsContext::Prose)
            ]
        );
        let s = sibs(&["dev", "do"]);
        let (out, n) = templatize(
            "> > ```sh\n> > mind learn dev\n> > ```\n>\n> Then see do.\n",
            &s,
        );
        assert_eq!(n, 1, "only the quoted prose mention is wrapped: {out}");
        assert_eq!(
            out,
            "> > ```sh\n> > mind learn dev\n> > ```\n>\n> Then see {{ns:do}}.\n"
        );
        // A list item inside a blockquote composes the same way.
        let doc = "> - item:\n>\n>   ```sh\n>   echo {{ns:dev}}\n>   ```\n>\n>   Then {{ns:do}}.\n";
        assert_eq!(
            contexts(doc),
            vec![
                ("dev".into(), NsContext::CodeBlock),
                ("do".into(), NsContext::Prose)
            ]
        );
    }

    #[test]
    fn a_table_cell_cannot_hold_a_code_block() {
        // spec: NS-46 NS-47
        // A fence delimiter inside a table cell is inline text, not a block
        // opener, so it cannot swallow the rest of the table. A matched run in
        // one cell is still a span.
        let doc = "| a | b |\n|---|---|\n| ``` | see {{ns:dev}} |\n| x | and {{ns:do}} |\n";
        assert_eq!(
            contexts(doc),
            vec![
                ("dev".into(), NsContext::Prose),
                ("do".into(), NsContext::Prose)
            ]
        );
        assert_eq!(surviving(doc), doc);
        // A pipe inside a code span does not split the cell, and the token in
        // that span is still code.
        let doc = "| a | `x | {{ns:dev}}` |\n|---|---|\n| c | {{ns:do}} |\n";
        assert_eq!(
            contexts(doc),
            vec![
                ("dev".into(), NsContext::CodeSpan),
                ("do".into(), NsContext::Prose)
            ]
        );
    }

    #[test]
    fn a_hard_line_break_does_not_stop_the_block_below_it() {
        // spec: NS-47 NS-50
        // A fenced block may interrupt a paragraph, including one whose last
        // line ends in a hard break. Both hard-break spellings are checked,
        // because the backslash one is also what the escape rule looks at.
        for br in ["\\", "  "] {
            let doc = format!(
                "Line one{br}\n```sh\nmind learn {{{{ns:dev}}}}\n```\n\nThen {{{{ns:do}}}}.\n"
            );
            assert_eq!(
                contexts(&doc),
                vec![
                    ("dev".into(), NsContext::CodeBlock),
                    ("do".into(), NsContext::Prose)
                ],
                "break {br:?}"
            );
        }
        // And a hard break inside a paragraph does not end the span a code
        // span opened before it.
        let doc = "run `gh auth\\\ntoken {{ns:dev}}` now\n";
        assert_eq!(contexts(doc), vec![("dev".into(), NsContext::CodeSpan)]);
    }

    #[test]
    fn a_loose_list_item_is_prose_and_a_tight_one_is_too() {
        // spec: NS-49
        // Tightness changes the events a parser emits (a tight item has no
        // Paragraph), which is exactly the sort of difference a map keyed on
        // events can trip over. Neither shape may change what is code.
        let tight = "- see {{ns:dev}}\n- run `{{ns:do}}`\n";
        let loose = "- see {{ns:dev}}\n\n- run `{{ns:do}}`\n";
        for doc in [tight, loose] {
            assert_eq!(
                contexts(doc),
                vec![
                    ("dev".into(), NsContext::Prose),
                    ("do".into(), NsContext::CodeSpan)
                ],
                "{doc}"
            );
        }
        let s = sibs(&["dev", "do"]);
        for doc in ["- see dev\n- run `do`\n", "- see dev\n\n- run `do`\n"] {
            let (out, n) = templatize(doc, &s);
            assert_eq!(n, 1, "only the prose mention is wrapped: {out}");
            assert!(out.contains("`do`"), "the span is untouched: {out}");
        }
    }

    #[test]
    fn an_autolink_is_prose_and_a_url_path_segment_is_not_wrapped() {
        // spec: NS-24
        // An autolink's URL is not code, so the only thing keeping a sibling
        // name in one from being rewritten is the path-adjacency rule (NS-24).
        // It covers every segment of a URL path and the host, because both
        // abut a `/`.
        assert!(!wraps("See <https://example.com/dev> now.\n", "dev"));
        assert!(!wraps("See <https://example.com/dev/x> now.\n", "dev"));
        assert!(!wraps("See <https://dev.example.com/> now.\n", "dev"));
        // A token in an autolink is prose and survives, which is what matters
        // for `--fix`: it deletes nothing here.
        let doc = "See <https://example.com/{{ns:dev}}> now.\n";
        assert_eq!(contexts(doc), vec![("dev".into(), NsContext::Path)]);
        // An entity reference is ordinary text and changes nothing around it.
        let doc = "A &amp; B, then {{ns:dev}} and `{{ns:do}}`.\n";
        assert_eq!(
            contexts(doc),
            vec![
                ("dev".into(), NsContext::Prose),
                ("do".into(), NsContext::CodeSpan)
            ]
        );
    }

    #[test]
    fn a_three_backtick_run_carrying_a_backtick_is_a_span_and_not_a_fence() {
        // spec: NS-47
        // The NS-47 clause about a backtick opener's info string, at the run
        // length where it can actually bite. The existing test for it uses a
        // two-backtick run, which is not a fence opener at any length, so it
        // cannot tell a correct reading from one that ignores the clause. A
        // three-backtick run carrying another backtick is the real case: it is
        // a code span, so it opens nothing and the prose below it is prose.
        let doc = "```a ` b``` opens nothing\n\nprose {{ns:dev}} here\n";
        assert_eq!(contexts(doc), vec![("dev".into(), NsContext::Prose)]);
        assert_eq!(surviving(doc), doc);
        // The counterpart: the same run with a backtick-free info string is a
        // fence opener, it never closes, and the token below it is code.
        let doc = "```a b\nstill fenced {{ns:dev}}\n";
        assert_eq!(contexts(doc), vec![("dev".into(), NsContext::CodeBlock)]);
        // And a tilde opener may carry a backtick in its info string.
        let doc = "~~~a ` b\n{{ns:dev}}\n~~~\nprose {{ns:do}}\n";
        assert_eq!(
            contexts(doc),
            vec![
                ("dev".into(), NsContext::CodeBlock),
                ("do".into(), NsContext::Prose)
            ]
        );
    }

    #[test]
    fn a_thematic_break_of_any_spelling_opens_no_list() {
        // spec: NS-49
        // The deleted marker test asserted that `---`, `***` and a spaced
        // `* * *` are not list markers. A thematic break takes precedence over
        // a list item in CommonMark, so none of these lifts the indent
        // baseline, and the four-column block under each is an indented code
        // block whose mention wrapping must not rewrite.
        for brk in ["---", "***", "___", "* * *", "- - -", "  ***"] {
            let doc = format!("intro\n\n{brk}\n\n    mind learn {{{{ns:dev}}}}\n");
            assert_eq!(
                contexts(&doc),
                vec![("dev".into(), NsContext::CodeBlock)],
                "break {brk:?}"
            );
            assert!(
                !wraps(&format!("intro\n\n{brk}\n\n    mind learn dev\n"), "dev"),
                "break {brk:?}"
            );
        }
    }

    #[test]
    fn no_construct_leaks_its_code_into_the_prose_paragraph_after_it() {
        // spec: NS-46 NS-47 NS-48 NS-49 NS-50
        // The systematic form of every bug filed against this classifier: a
        // construct opens something it should not, or fails to close something
        // it should, and the *next* paragraph reads as code -- so `--fix`
        // deletes the token in it. Rather than hand-pick the constructs that
        // have already bitten, run the whole catalogue of them past one prose
        // paragraph and require, for each, that the token survives, that the
        // bare mention beside it is wrapped, and that NS-48 holds.
        let s = sibs(&["dev", "do"]);
        for prefix in [
            "",
            "# Heading\n\n",
            "## Heading ##\n\n",
            "Setext\n---\n\n",
            "***\n\n",
            "* * *\n\n",
            "```sh\nmind sync\n```\n\n",
            "````md\n```sh\nmind sync\n```\n````\n\n",
            "~~~text\n```\n~~~\n\n",
            "```a ` b``` is a span\n\n",
            "    indented code\n\n",
            "\ttab indented code\n\n",
            "> quoted prose\n\n",
            "> ```sh\n> mind sync\n> ```\n\n",
            "> > ```sh\n> > mind sync\n> > ```\n\n",
            "- item\n- item\n\n",
            "- item:\n\n  ```sh\n  mind sync\n  ```\n\n",
            "1.  item:\n\n    ```sh\n    mind sync\n    ```\n\n",
            "- ```sh\n  mind sync\n  ```\n\n",
            "| a | b |\n|---|---|\n| `x` | y |\n\n",
            "<div>\nraw html\n</div>\n\n",
            "<https://example.com/x>\n\n",
            "[label]: https://example.com/x\n\n",
            "Escape a backtick as \\` in prose.\n\n",
            "A lone ` backtick in prose.\n\n",
            "Trailing backslash \\\n\n",
            "Trailing spaces  \n\n",
            "---\nname: thing\ndescription: x\n---\n",
            "Run `gh auth\ntoken --hostname x` inline.\n\n",
            "``a ` b`` is a span.\n\n",
            "Text with {{tools:x}} and {{self}} tokens.\n\n",
        ] {
            // Direction one: an existing prose token must survive `--fix`.
            let doc = format!("{prefix}Then see the {{{{ns:dev}}}} skill.\n");
            assert_eq!(
                contexts(&doc),
                vec![("dev".into(), NsContext::Prose)],
                "prefix {prefix:?} leaked into the paragraph after it:\n{doc}"
            );
            assert_eq!(surviving(&doc), doc, "prefix {prefix:?}");
            // Direction two: a bare mention there must be wrapped, and the
            // result must satisfy NS-48.
            let bare = format!("{prefix}Then see the do skill.\n");
            let fixed = fix_passes(&bare, &s);
            assert!(
                fixed.contains("{{ns:do}} skill"),
                "prefix {prefix:?} kept wrapping from reaching prose:\n{fixed}"
            );
            assert_eq!(
                unguarded_refs(&fixed, &sibs(&["do"])),
                Vec::<String>::new(),
                "prefix {prefix:?} left an unguarded reference:\n{fixed}"
            );
        }
    }

    // ---- the hand-rolled remnants: frontmatter, and the brace-span copier ----

    #[test]
    fn frontmatter_delimiters_are_read_at_their_documented_edges() {
        // spec: NS-24 NS-47
        // `mark_frontmatter` is the one part of the structural read that is
        // still hand-rolled, so its edges are pinned directly: the block opens
        // only on the first line, either delimiter may carry trailing
        // whitespace, the closer need not end in a newline, and the body after
        // it is ordinary markdown.
        let s = sibs(&["dev", "do"]);
        // No trailing newline anywhere: the closer is still a closer.
        assert!(!wraps("---\nname: dev\n---", "dev"));
        // Trailing whitespace on both delimiters, and a body that is wrapped.
        let (out, n) = templatize("---  \nname: dev\n--- \nsee do\n", &s);
        assert_eq!(n, 1, "only the body mention is wrapped: {out}");
        assert_eq!(out, "---  \nname: dev\n--- \nsee {{ns:do}}\n");
        // A closer with no trailing newline still ends the block, so the body
        // that follows it on the same read is markdown.
        let (out, n) = templatize("---\nname: dev\n---\nsee do", &s);
        assert_eq!(n, 1, "{out}");
        assert_eq!(out, "---\nname: dev\n---\nsee {{ns:do}}");
        // Degenerate inputs: no panic, no rewrite.
        for doc in ["", "---", "---\n", "\n", "   \n"] {
            assert_eq!(templatize(doc, &s), (doc.to_string(), 0), "{doc:?}");
            assert!(scan_ns_refs(doc).is_empty(), "{doc:?}");
        }
    }

    #[test]
    fn an_unterminated_frontmatter_block_swallows_the_whole_document() {
        // spec: NS-24 NS-47 NS-56
        // Characterization of the documented "runs to the end of the document"
        // rule, in the shape where it costs something. A file whose leading
        // `---` never closes is frontmatter throughout, so every line in it that
        // is not `description:` is a structured field: wrapping rewrites nothing
        // there (NS-56 opens up only the `description:` value) and every token in
        // it classifies prose, so it survives `--fix` (NS-24). Nothing is
        // deleted, which is the direction that matters.
        let s = sibs(&["dev"]);
        let doc = "---\nname: thing\ndescription: x\n\nSee dev in the body.\n";
        assert_eq!(templatize(doc, &s), (doc.to_string(), 0));
        let tokenized = "---\nname: thing\n\nSee {{ns:dev}} in the body.\n";
        assert_eq!(contexts(tokenized), vec![("dev".into(), NsContext::Prose)]);
        assert_eq!(surviving(tokenized), tokenized);
        assert_eq!(
            unguarded_refs(&fix_passes(doc, &s), &s),
            Vec::<String>::new(),
            "and the structure-aware check does not report what wrapping declines"
        );
        // The same rule read the other way: a leading `---` that the author
        // meant as a thematic break opens a frontmatter block, and a later one
        // closes it, so the text between them is read as structured frontmatter
        // and is never wrapped, while the ordinary markdown body after the close
        // is.
        let doc = "---\n\nIntro mentioning dev.\n\n---\n\nBody mentioning dev.\n";
        let (out, n) = templatize(doc, &s);
        assert_eq!(n, 1, "only the text past the second delimiter: {out}");
        assert_eq!(
            out,
            "---\n\nIntro mentioning dev.\n\n---\n\nBody mentioning {{ns:dev}}.\n"
        );
    }

    #[test]
    fn a_setext_underline_is_not_mistaken_for_a_frontmatter_delimiter() {
        // spec: NS-47
        // The block opens only on the document's first line, so a `---` used
        // as a setext underline further down is markdown, not a delimiter, and
        // the heading text above it is prose.
        let s = sibs(&["dev", "do"]);
        let (out, n) = templatize("Title\n---\nSee dev here.\n", &s);
        assert_eq!(n, 1, "{out}");
        assert_eq!(out, "Title\n---\nSee {{ns:dev}} here.\n");
        // The heading text itself is prose and is wrapped like any other.
        let (out, n) = templatize("dev\n---\nSee do here.\n", &s);
        assert_eq!(n, 2, "{out}");
        assert_eq!(out, "{{ns:dev}}\n---\nSee {{ns:do}} here.\n");
    }

    #[test]
    fn a_utf8_bom_does_not_expose_the_frontmatter_to_wrapping() {
        // spec: NS-24
        // `frontmatter.rs` strips a leading BOM before its delimiter check
        // (DSC-23), so a BOM-prefixed SKILL.md is a valid item whose `name:`
        // field mind reads. `mark_frontmatter` does not strip it, so the
        // opening `---` fails its `== "---"` test, the whole file is parsed as
        // markdown, and the frontmatter becomes a setext heading whose text is
        // prose -- which wrapping then rewrites, turning the item's declared
        // name into a token. NS-24 names that field as the one place wrapping
        // must never touch.
        let doc = "\u{feff}---\nname: dev\ndescription: x\n---\nBody text.\n";
        assert_eq!(
            templatize(doc, &sibs(&["dev"])),
            (doc.to_string(), 0),
            "the `name:` field must survive a BOM"
        );
    }

    #[test]
    fn a_bom_is_stripped_without_shifting_any_offset() {
        // spec: NS-47 NS-24
        // The other half of the BOM fix: stripping it must not move the map. The
        // BOM is three bytes ahead of the opening delimiter, so a strip that
        // forgot to add them back would shift every body offset by three and the
        // tokens below -- flush against a code span's delimiters on both sides --
        // would classify wrong in one direction or the other.
        let doc = "\u{feff}---\nname: thing\ndescription: caf\u{e9}\n---\n\
                   `x`{{ns:dev}} and `{{ns:do}}`\n";
        assert_eq!(
            contexts(doc),
            vec![
                ("dev".into(), NsContext::Prose),
                ("do".into(), NsContext::CodeSpan)
            ]
        );
        assert_eq!(
            surviving(doc),
            "\u{feff}---\nname: thing\ndescription: caf\u{e9}\n---\n\
             `x`{{ns:dev}} and `do`\n"
        );
        // The body after a BOM-prefixed frontmatter block is ordinary markdown,
        // so a bare mention in it is still wrapped: the strip protects the
        // `name:` field without turning the rest of the file into a dead zone.
        // The `description:` mention is wrapped too (NS-56): only `name:` is
        // exempt.
        let s = sibs(&["dev"]);
        let (out, n) = templatize(
            "\u{feff}---\nname: thing\ndescription: hands off to dev\n---\nSee dev here.\n",
            &s,
        );
        assert_eq!(n, 2, "the description mention and the body mention: {out}");
        assert_eq!(
            out,
            "\u{feff}---\nname: thing\ndescription: hands off to {{ns:dev}}\n---\n\
             See {{ns:dev}} here.\n"
        );
        // A BOM on a file with no frontmatter opens no block: the first line is
        // markdown and its mention is wrapped.
        let (out, n) = templatize("\u{feff}# Title\n\nSee dev here.\n", &s);
        assert_eq!(n, 1, "{out}");
        assert_eq!(out, "\u{feff}# Title\n\nSee {{ns:dev}} here.\n");
    }

    #[test]
    fn the_frontmatter_shift_keeps_every_body_offset_exact() {
        // spec: NS-46 NS-47
        // The body is parsed as its own document and every range is shifted
        // back by the frontmatter's byte length, so a multi-byte character in
        // the frontmatter is where an off-by-one would land. The tokens below
        // sit flush against a code span's delimiters in both directions, so a
        // shift of one byte either way flips one of them.
        let doc = "---\ndescription: caf\u{e9} r\u{e9}sum\u{e9} \u{1d400}\n---\n\
                   `x`{{ns:dev}} and `{{ns:do}}`{{ns:dev}}\n";
        assert_eq!(
            contexts(doc),
            vec![
                ("dev".into(), NsContext::Prose),
                ("do".into(), NsContext::CodeSpan),
                ("dev".into(), NsContext::Prose)
            ]
        );
        assert_eq!(
            surviving(doc),
            "---\ndescription: caf\u{e9} r\u{e9}sum\u{e9} \u{1d400}\n---\n\
                   `x`{{ns:dev}} and `do`{{ns:dev}}\n"
        );
        // The same with a multi-byte character in the body, ahead of the span.
        let doc = "---\nname: thing\n---\nCaf\u{e9} \u{1d400}: `x`{{ns:dev}} and `{{ns:do}}`\n";
        assert_eq!(
            contexts(doc),
            vec![
                ("dev".into(), NsContext::Prose),
                ("do".into(), NsContext::CodeSpan)
            ]
        );
        // CRLF frontmatter shifts by one more byte per line, and the body
        // classification must not move with it.
        let doc = "---\r\ndescription: caf\u{e9}\r\n---\r\n`x`{{ns:dev}} and `{{ns:do}}`\r\n";
        assert_eq!(
            contexts(doc),
            vec![
                ("dev".into(), NsContext::Prose),
                ("do".into(), NsContext::CodeSpan)
            ]
        );
    }

    #[test]
    fn an_unterminated_brace_span_stops_wrapping_at_the_document_level() {
        // spec: NS-51
        // The behavior change NS-51 brought with it: the brace-span copier is
        // document-wide, not line-local, so a stray `{{` copies everything up
        // to the next `}}` anywhere in the file -- and, with no `}}` at all,
        // the whole remainder. That can only *suppress* a wrap, never create
        // or delete one, which is why it is acceptable; pinned so the blast
        // radius is a decision.
        let s = sibs(&["dev", "do"]);
        // No closing `}}` at all: everything after the stray `{{` is verbatim.
        let doc = "Use {{ v } here. Then hand off to dev.\n";
        assert_eq!(templatize(doc, &s), (doc.to_string(), 0));
        // A `}}` further down closes the copied span, and wrapping resumes
        // after it -- so the mention before it is skipped and the one after is
        // not. Line-local consumption used to skip only the first line.
        let doc = "Use {{ v } then dev. Later {{ns:do}} ends it. And dev again.\n";
        let (out, n) = templatize(doc, &s);
        assert_eq!(n, 1, "{out}");
        assert_eq!(
            out,
            "Use {{ v } then dev. Later {{ns:do}} ends it. And {{ns:dev}} again.\n"
        );
        // A word ending exactly at a `{{` is still emitted, and still wrapped.
        assert_eq!(
            templatize("see dev{{", &s),
            ("see {{ns:dev}}{{".to_string(), 1)
        );
        // The pass pair still settles on the same shape, so a stray `{{` does
        // not make `--fix` oscillate.
        let doc = "Use {{ v } then `{{ns:dev}}` here, and do.\n";
        let once = fix_passes(doc, &s);
        assert_eq!(once, fix_passes(&once, &s), "{once}");
    }

    #[test]
    fn a_path_adjacent_mention_is_no_longer_a_false_positive() {
        // spec: NS-48 NS-24 NS-55
        // A path-adjacent mention: wrapping still declines it (NS-24) --
        // wrapping `~/{{ns:dev}}` would rewrite a path -- but structure-aware
        // `unguarded_refs` (NS-55) shares the same path-adjacency test and no
        // longer reports it either, so `--fix` is not asked to clear a mention
        // that was never a real reference to begin with.
        let s = sibs(&["dev"]);
        for doc in [
            "Config lives in ~/dev now.\n",
            "Config lives in etc/dev/x now.\n",
        ] {
            let fixed = fix_passes(doc, &s);
            assert_eq!(fixed, doc, "wrapping must not rewrite a path: {fixed}");
            assert_eq!(
                unguarded_refs(&fixed, &s),
                Vec::<String>::new(),
                "a path-adjacent mention is not a reference and is no longer reported: {fixed}"
            );
        }
        // The proof that it is the path rule and not a structural miss: the
        // same name one space away from the separator is wrapped.
        assert!(wraps("Config lives in ~/ dev now.\n", "dev"));
    }

    #[test]
    fn a_token_on_a_delimiter_line_survives_an_all_code_un_wrap() {
        // spec: NS-47 NS-24
        // Characterization of the `all_code` leg, which `review` uses for a
        // non-markdown file (a shell script, say): the structure map is still a
        // markdown map, so a line in the script that happens to look like a
        // fence delimiter is `Skip`, and a token on it is not reported and so
        // not un-wrapped -- even though `all_code` means "un-wrap everything".
        // Not destructive (a leftover token still expands at install, NS-11),
        // but the cleanup is incomplete, and it is incomplete only for lines
        // that a markdown parser would read as structure.
        let script = "#!/bin/sh\n# hand off to {{ns:dev}}\ncat <<'EOF'\n\
                      ```{{ns:do}}\nEOF\n";
        let (out, n) = unwrap_misplaced(script, true);
        assert_eq!(n, 1, "only the token off the delimiter line: {out}");
        assert_eq!(
            out,
            "#!/bin/sh\n# hand off to dev\ncat <<'EOF'\n```{{ns:do}}\nEOF\n"
        );
        // A script with no delimiter-shaped line is cleaned completely.
        let script = "#!/bin/sh\n# hand off to {{ns:dev}}\nrun {{ns:do}}\n";
        assert_eq!(
            unwrap_misplaced(script, true),
            ("#!/bin/sh\n# hand off to dev\nrun do\n".to_string(), 2)
        );
    }

    // ---- indentation coverage the removed helper tests used to carry --------

    #[test]
    fn the_space_run_after_a_list_marker_sets_the_content_column() {
        // spec: NS-49
        // What the deleted `list_item_content_col` cases asserted about the
        // marker's own spacing, restated as documents. One to four spaces after
        // the marker put the content that many columns out; five or more make
        // the item's first block an indented code block whose content column is
        // one past the marker.
        //
        // Three spaces: content column 4, so a line at column 4 continues the
        // item as a paragraph and its mention is wrapped.
        assert!(wraps("-   Step:\n\n    mind learn dev\n", "dev"));
        // Four spaces: content column 5, so the same line is short of it. The
        // item ends and the line is a top-level indented code block.
        assert!(!wraps("-    Step:\n\n    mind learn dev\n", "dev"));
        // Five spaces: the item's own content is an indented code block.
        assert!(!wraps("-     mind learn dev\n", "dev"));
        let doc = "-     mind learn {{ns:dev}}\n";
        assert_eq!(contexts(doc), vec![("dev".into(), NsContext::CodeBlock)]);
        // Ten digits is not an ordered marker (nine is the cap), so the line is
        // a paragraph and the four-column block under it is code.
        assert!(!wraps("1234567890. Step:\n\n    mind learn dev\n", "dev"));
        // Nine digits is a marker, and its content column is 11, so a fence
        // written there is a fence while the same line at column 4 is not.
        let doc = "123456789. Step:\n\n    mind learn {{ns:dev}}\n";
        assert_eq!(contexts(doc), vec![("dev".into(), NsContext::CodeBlock)]);
    }

    #[test]
    fn a_tab_advances_to_the_next_multiple_of_four_columns() {
        // spec: NS-49
        // What the deleted `indent_cols` cases asserted, restated as documents
        // whose classification turns on the column count. A space then a tab is
        // four columns, not two, so the block is code and its mention is never
        // wrapped.
        assert!(!wraps("Sample:\n\n \tmind learn dev\n", "dev"));
        assert!(!wraps("Sample:\n\n\t\tmind learn dev\n", "dev"));
        assert!(!wraps("Sample:\n\n    \tmind learn dev\n", "dev"));
        let doc = "Sample:\n\n \tmind learn {{ns:dev}}\n";
        assert_eq!(contexts(doc), vec![("dev".into(), NsContext::CodeBlock)]);
        // Inside a list item the same four columns is only two past the item's
        // content column, so it is a paragraph and its mention is wrapped.
        assert!(wraps("- Step:\n\n \tmind learn dev\n", "dev"));
        // A tab right after the marker advances to column 4, so the item's
        // content column is 4 and a line at column 6 is a paragraph inside it.
        // The old hand-rolled reading called that column 2, which made the same
        // line an indented code block: the one case where the parser swap
        // changes the answer in the wrapping direction.
        assert!(wraps("-\tStep:\n\n      mind learn dev\n", "dev"));
        // Four past that content column is code again.
        assert!(!wraps("-\tStep:\n\n        mind learn dev\n", "dev"));
    }

    // ---- outside-in fourth pass: the link rule (NS-52) and the BOM leg ------
    //
    // Written against CommonMark and against what NS-52 claims, not against the
    // shapes the change set names. Each case states what a renderer does with
    // the document and requires the classifier to agree wherever disagreeing
    // would rewrite the author's working tree.

    #[test]
    fn an_image_nested_in_a_links_text_is_syntax_in_both_layers() {
        // spec: NS-52
        // The `links` stack exists for this shape and had no case: an image may
        // be the visible text of the link around it, so two link constructs are
        // open over the same bytes. Both destinations are syntax and the
        // image's alt text is the only prose in the whole span. Neither
        // destination here abuts a `/`, so the path rule cannot be what saves
        // them: only the link map can.
        let s = sibs(&["dev", "do"]);
        let (out, n) = templatize("See [![the dev diagram](do.png)](do.md) now.\n", &s);
        assert_eq!(n, 1, "only the alt text is prose: {out}");
        assert_eq!(out, "See [![the {{ns:dev}} diagram](do.png)](do.md) now.\n");
        // The un-wrapping direction over the same nesting: a token in either
        // destination is misplaced, and one in the alt text is not.
        let doc = "See [![the {{ns:dev}} diagram]({{ns:do}}.png)]({{ns:do}}.md).\n";
        assert_eq!(
            contexts(doc),
            vec![
                ("dev".into(), NsContext::Prose),
                ("do".into(), NsContext::Path),
                ("do".into(), NsContext::Path),
            ]
        );
        assert_eq!(
            surviving(doc),
            "See [![the {{ns:dev}} diagram](do.png)](do.md).\n"
        );
        // And the stack pops: the prose after the nest is prose again.
        let (out, n) = templatize("See [![alt](x.png)](y.md), then dev.\n", &s);
        assert_eq!(n, 1, "{out}");
        assert_eq!(out, "See [![alt](x.png)](y.md), then {{ns:dev}}.\n");
        // Two link constructs deep the other way round: an image description
        // may contain a link, and each layer keeps its own parts straight.
        let doc = "![a dev shot with [the do notes](do.md) inside](dev.png)\n";
        let (out, n) = templatize(doc, &s);
        assert_eq!(n, 2, "both visible texts, neither destination: {out}");
        assert_eq!(
            out,
            "![a {{ns:dev}} shot with [the {{ns:do}} notes](do.md) inside](dev.png)\n"
        );
        // The stack's "no open link is opaque" test is written over the whole
        // stack rather than its top, which turns out to be defensive only: an
        // opaque link is a shortcut, a collapsed reference, or an autolink, and
        // none of them can contain a nested link or image, because a link label
        // may not hold an unescaped bracket and an autolink has no inner
        // structure. The shape that would tell the two readings apart is
        // therefore not CommonMark at all -- `[![alt](x.png)]` is not a link,
        // it is literal text, so both of its mentions are ordinary prose.
        let doc = "[![the dev diagram](x.png)]\n\n[![the dev diagram](x.png)]: y.md\n";
        let (out, n) = templatize(doc, &s);
        assert_eq!(n, 2, "a bracketed label is not a label: {out}");
        assert_eq!(
            out,
            "[![the {{ns:dev}} diagram](x.png)]\n\n\
             [![the {{ns:dev}} diagram](x.png)]: y.md\n"
        );
    }

    #[test]
    fn a_link_in_a_table_cell_or_a_blockquote_is_still_a_link() {
        // spec: NS-52
        // Containers were the shapes the fence rule kept getting wrong, so the
        // link rule is asked the same question. A table cell is not CommonMark
        // and a blockquote re-indents its content, so either could move the
        // ranges the map is filled from.
        let s = sibs(&["dev", "do"]);
        let doc = "| step | link |\n|---|---|\n| one | [the dev notes](do.md) |\n";
        let (out, n) = templatize(doc, &s);
        assert_eq!(n, 1, "only the link text in the cell: {out}");
        assert_eq!(
            out,
            "| step | link |\n|---|---|\n| one | [the {{ns:dev}} notes](do.md) |\n"
        );
        let doc = "> See [the dev notes](do.md) first.\n";
        let (out, n) = templatize(doc, &s);
        assert_eq!(n, 1, "{out}");
        assert_eq!(out, "> See [the {{ns:dev}} notes](do.md) first.\n");
        // Two levels of quoting, where the container prefix is longest.
        let doc = "> > See [the dev notes](do.md) first.\n";
        let (out, n) = templatize(doc, &s);
        assert_eq!(n, 1, "{out}");
        assert_eq!(out, "> > See [the {{ns:dev}} notes](do.md) first.\n");
        // A link inside a list item, at a content column the fence rule cares
        // about.
        let doc = "1.  Step:\n\n    See [the dev notes](do.md).\n";
        let (out, n) = templatize(doc, &s);
        assert_eq!(n, 1, "{out}");
        assert_eq!(out, "1.  Step:\n\n    See [the {{ns:dev}} notes](do.md).\n");
        // The un-wrapping direction inside a container: a token in a quoted
        // destination is still misplaced.
        let doc = "> See [the docs]({{ns:dev}}.md).\n";
        assert_eq!(contexts(doc), vec![("dev".into(), NsContext::Path)]);
        assert_eq!(surviving(doc), "> See [the docs](dev.md).\n");
    }

    #[test]
    fn a_code_span_inside_link_text_is_still_a_code_span() {
        // spec: NS-52 NS-46
        // Link text is re-filled as prose from the text events inside the link,
        // and a code span in that text is not a text event: it must keep the
        // span classification rather than be flattened into the prose refill.
        let s = sibs(&["dev", "do"]);
        let doc = "See [the `dev` command](do.md) now.\n";
        assert_eq!(
            templatize(doc, &s),
            (doc.to_string(), 0),
            "a name in a span inside link text is code, and the destination is syntax"
        );
        let doc = "See [the `{{ns:dev}}` command](x.md) now.\n";
        assert_eq!(contexts(doc), vec![("dev".into(), NsContext::CodeSpan)]);
        assert_eq!(surviving(doc), "See [the `dev` command](x.md) now.\n");
        // The prose either side of the span inside the same link text is still
        // wrappable, so the span is a hole in the refill and not a wall.
        let (out, n) = templatize("See [the dev `x` guide](y.md).\n", &s);
        assert_eq!(n, 1, "{out}");
        assert_eq!(out, "See [the {{ns:dev}} `x` guide](y.md).\n");
    }

    #[test]
    fn an_angle_bracket_destination_is_syntax_like_any_other() {
        // spec: NS-52
        // CommonMark lets a destination be wrapped in `<...>` so it may contain
        // spaces. That is the one destination spelling where a sibling name can
        // sit as a bare word with no path separator anywhere near it, so the
        // path rule cannot stand in for the link rule.
        let s = sibs(&["dev", "do"]);
        for doc in [
            "See [the docs](<my dev notes.md>).\n",
            "See [the docs](<dev>).\n",
            "![the diagram](<a dev diagram.png>)\n",
        ] {
            assert_eq!(
                templatize(doc, &s),
                (doc.to_string(), 0),
                "an angle-bracket destination is syntax: {doc}"
            );
        }
        let doc = "See [the docs](<{{ns:dev}} notes.md>).\n";
        assert_eq!(contexts(doc), vec![("dev".into(), NsContext::Path)]);
        assert_eq!(surviving(doc), "See [the docs](<dev notes.md>).\n");
    }

    #[test]
    fn a_link_reference_definition_span_covers_it_and_stops_at_its_end() {
        // spec: NS-52
        // The definition span is read off the parser's own table rather than
        // from an event, so its reach is the thing to pin: too short and the
        // destination or title is wrapped, too long and the prose on the line
        // after it is frozen. Both directions are asserted here.
        let s = sibs(&["dev", "do"]);
        // Label, destination and title all on one line: none of it is prose.
        let doc = "[dev]: https://example.com/x \"the do notes\"\n\nSee [x][dev].\n";
        assert_eq!(templatize(doc, &s), (doc.to_string(), 0), "one-line def");
        // A title on its own continuation line is part of the definition.
        let doc = "[dev]: https://example.com/x\n  \"the do notes\"\n\nSee [x][dev].\n";
        assert_eq!(templatize(doc, &s), (doc.to_string(), 0), "wrapped title");
        // Label on one line and destination on the next is one definition too.
        let doc = "[dev]:\n  https://example.com/do-notes\n\nSee [x][dev].\n";
        assert_eq!(templatize(doc, &s), (doc.to_string(), 0), "wrapped dest");
        // The other edge: an ordinary paragraph on the line right after a
        // definition, with no blank line between them, is prose and is wrapped.
        // A span that over-reached by one line would freeze it.
        let doc = "[dev]: https://example.com/x\nThen see the do skill.\n";
        let (out, n) = templatize(doc, &s);
        assert_eq!(n, 1, "the line after the def is prose: {out}");
        assert_eq!(
            out,
            "[dev]: https://example.com/x\nThen see the {{ns:do}} skill.\n"
        );
        // Two definitions back to back: neither swallows the other's label.
        let doc = "[dev]: https://example.com/a\n[do]: https://example.com/b\n\n\
                   See [x][dev] and [y][do].\n";
        assert_eq!(templatize(doc, &s), (doc.to_string(), 0), "two defs");
    }

    #[test]
    fn a_definition_inside_a_container_is_still_a_definition() {
        // spec: NS-52
        // Link reference definitions are document-global whatever container
        // they were written in, and a blockquote's `>` prefix and a list item's
        // indent both shift the bytes the parser reports them at.
        let s = sibs(&["dev", "do"]);
        for doc in [
            "> [dev]: https://example.com/x\n\nSee [the docs][dev].\n",
            "- [dev]: https://example.com/x\n\nSee [the docs][dev].\n",
            ">   [dev]: https://example.com/x\n\nSee [the docs][dev].\n",
        ] {
            assert_eq!(
                templatize(doc, &s),
                (doc.to_string(), 0),
                "a definition in a container is syntax: {doc}"
            );
        }
        // And the token direction: `--fix` reaches a token in a quoted
        // definition, so the two passes still agree about it.
        let doc = "> [{{ns:dev}}]: https://example.com/x\n\nSee [the docs][{{ns:dev}}].\n";
        assert!(
            contexts(doc).iter().all(|(_, ctx)| *ctx == NsContext::Path),
            "{:?}",
            contexts(doc)
        );
        assert_eq!(
            surviving(doc),
            "> [dev]: https://example.com/x\n\nSee [the docs][dev].\n"
        );
    }

    #[test]
    fn a_definition_below_an_unterminated_frontmatter_block_is_never_read() {
        // spec: NS-52 NS-47 NS-56
        // The frontmatter pre-pass runs before the parse and hands it only the
        // body, so a document whose leading `---` never closes hands it nothing
        // at all: there are no definitions and no links, and every byte is a
        // frontmatter field. Nothing is wrapped -- a swallowed line is a
        // structured field, not the `description:` value NS-56 opens up -- which
        // is the safe direction twice over: the definition's own label would
        // otherwise be rewritten into `[{{ns:dev}}]:`, silently breaking the
        // link the parser never got to see.
        let s = sibs(&["dev", "do"]);
        let doc = "---\nname: thing\n[dev]: https://example.com/x\n\n\
                   See [the do notes][dev].\n";
        assert_eq!(templatize(doc, &s), (doc.to_string(), 0));
        let doc = "---\nname: thing\n\nSee [the docs]({{ns:dev}}.md).\n";
        assert_eq!(
            contexts(doc),
            vec![("dev".into(), NsContext::Prose)],
            "an unterminated block makes everything a field, so nothing is deleted"
        );
        assert_eq!(surviving(doc), doc);
        // Terminated, the same body is read as markdown again and the link rule
        // applies to it.
        let doc = "---\nname: thing\n---\n\nSee [the do notes]({{ns:dev}}.md).\n";
        assert_eq!(contexts(doc), vec![("dev".into(), NsContext::Path)]);
        assert_eq!(
            templatize(&surviving(doc), &s),
            (
                "---\nname: thing\n---\n\nSee [the {{ns:do}} notes](dev.md).\n".to_string(),
                1
            )
        );
    }

    #[test]
    fn an_unresolved_reference_link_is_not_a_link_and_its_label_is_prose() {
        // spec: NS-52
        // The decision NS-52's last clause records, pinned in both directions.
        // `[text][label]` with no definition is not a link in CommonMark: it
        // renders as the literal characters, so the label is ordinary prose and
        // wrapping rewrites it. That is safe because the construct already did
        // not resolve -- the rewrite cannot break a link that was never one, and
        // the expanded form renders literally exactly as the original did.
        let s = sibs(&["dev", "do"]);
        let (out, n) = templatize("See [the docs][dev] now.\n", &s);
        assert_eq!(n, 1, "an unresolved label is prose: {out}");
        assert_eq!(out, "See [the docs][{{ns:dev}}] now.\n");
        // Add the definition and the same document is a link again, so the
        // label stops being wrappable. The rule is the parse, not the spelling.
        let doc = "See [the docs][dev] now.\n\n[dev]: https://example.com/x\n";
        assert_eq!(templatize(doc, &s), (doc.to_string(), 0));
        // The round trip is stable either way: a token written in an unresolved
        // label classifies prose, so `--fix` does not delete what it just wrote.
        let doc = "See [the docs][{{ns:dev}}] now.\n";
        assert_eq!(contexts(doc), vec![("dev".into(), NsContext::Prose)]);
        assert_eq!(surviving(doc), doc);
        assert_eq!(fix_passes(doc, &s), doc, "idempotent");
        // The asymmetry the decision costs, stated rather than left latent: a
        // *defined* shortcut label is syntax that wrapping declines, but writing
        // a token there makes the label unresolvable, so the same position
        // classifies prose and `--fix` leaves the token in place. Wrapping is a
        // subset of the scan here, which is the safe direction (nothing is
        // deleted); the token is dead either way, since it did not resolve
        // before expansion either.
        let doc = "[dev]: https://example.com/x\n\nSee [dev] for details.\n";
        assert_eq!(templatize(doc, &s), (doc.to_string(), 0), "declined");
        let doc = "[dev]: https://example.com/x\n\nSee [{{ns:dev}}] for details.\n";
        assert_eq!(contexts(doc), vec![("dev".into(), NsContext::Prose)]);
        assert_eq!(surviving(doc), doc, "and never deleted");
    }

    #[test]
    fn a_fence_info_string_mention_is_no_longer_a_false_positive() {
        // spec: NS-48 NS-47 NS-55
        // A sibling name in a fence's info string: the delimiter line is
        // `Skip`, so wrapping declines it (NS-47 calls the info string
        // structure, not content), and structure-aware `unguarded_refs` (NS-55)
        // reads the same `Skip` cell, so it no longer reports it either. NS-48's
        // false-positive list, once six deep, is now empty.
        let s = sibs(&["dev"]);
        for doc in ["```dev\nx\n```\n", "~~~ dev\nx\n~~~\n"] {
            let fixed = fix_passes(doc, &s);
            assert_eq!(fixed, doc, "an info string is never rewritten: {fixed}");
            assert_eq!(
                unguarded_refs(&fixed, &s),
                Vec::<String>::new(),
                "delimiter structure is not a reference and is no longer reported: {fixed}"
            );
        }
    }

    #[test]
    fn a_bom_survives_every_frontmatter_spelling() {
        // spec: NS-47 NS-24
        // The BOM strip mirrors `frontmatter.rs` (DSC-23), which trims the
        // first line before comparing it and accepts CRLF, so the strip has to
        // hold up in the same combinations. Each of these is a file mind
        // discovers and installs normally, so wrapping must see its
        // frontmatter and leave the `name:` field alone.
        let s = sibs(&["dev", "do"]);
        // BOM plus CRLF, which shifts every offset by one more byte per line.
        // `name:` stays bare; `description:` and the body mention are both
        // wrapped (NS-56).
        let doc = "\u{feff}---\r\nname: dev\r\ndescription: see do\r\n---\r\nSee do here.\r\n";
        let (out, n) = templatize(doc, &s);
        assert_eq!(n, 2, "the description mention and the body mention: {out}");
        assert_eq!(
            out,
            "\u{feff}---\r\nname: dev\r\ndescription: see {{ns:do}}\r\n---\r\n\
             See {{ns:do}} here.\r\n"
        );
        // BOM plus whitespace before the delimiter: `frontmatter.rs` trims the
        // line, so this is a real item and its `name:` field must be protected
        // too; `description:` is wrapped like the body.
        let doc = "\u{feff} ---\nname: dev\ndescription: see do\n--- \nSee do here.\n";
        let (out, n) = templatize(doc, &s);
        assert_eq!(n, 2, "the description mention and the body mention: {out}");
        assert_eq!(
            out,
            "\u{feff} ---\nname: dev\ndescription: see {{ns:do}}\n--- \nSee {{ns:do}} here.\n"
        );
        // The BOM is marked with the opening delimiter, so a token in the
        // `name:` field three bytes further on still lands on `FmName`: an
        // offset that forgot the BOM would report it as ordinary frontmatter.
        let doc = "\u{feff}---\nname: {{ns:dev}}\n---\n`x`{{ns:do}}\n";
        assert_eq!(
            contexts(doc),
            vec![
                ("dev".into(), NsContext::FrontmatterName),
                ("do".into(), NsContext::Prose),
            ]
        );
        // A BOM-prefixed block that never closes swallows the document, exactly
        // as an unprefixed one does; the swallowed body line is a structured
        // field, not the `description:` value, so nothing in it is wrapped
        // (NS-56).
        let doc = "\u{feff}---\nname: dev\n\nSee do in the body.\n";
        assert_eq!(templatize(doc, &s), (doc.to_string(), 0));
        // The body offset the read returns has to land exactly on the end of
        // the closing delimiter, BOM included. An indented code block as the
        // body's first block is what proves it: hand the parser even one byte
        // of the delimiter line and a paragraph is open above the block, which
        // makes it a lazy continuation -- prose, and its sample wrapped. The
        // classification tests above cannot see that error, because a body
        // offset that is short by the BOM and parser ranges that are long by
        // the same amount cancel out everywhere the extra bytes do not change
        // the parse.
        let doc = "\u{feff}---\nname: thing\n---\n    mind learn do\n";
        assert_eq!(
            templatize(doc, &s),
            (doc.to_string(), 0),
            "the body's first block is an indented code block, not a continuation"
        );
        assert_eq!(
            contexts("\u{feff}---\nname: thing\n---\n    mind learn {{ns:do}}\n"),
            vec![("do".into(), NsContext::CodeBlock)]
        );
        // The same shift seen from the frontmatter side: a `name:` field long
        // enough to put its token past the delimiter's own byte width is still
        // the `name:` field, and still the one hard finding `review` reports.
        // An offset short by the BOM marks it as the delimiter and drops it.
        let doc = "\u{feff}---\nname: aaaaaaaaaaa-{{ns:dev}}\n---\nbody\n";
        assert_eq!(
            contexts(doc),
            vec![("dev".into(), NsContext::FrontmatterName)]
        );
    }

    #[test]
    fn a_bom_does_not_move_the_bytes_an_all_code_un_wrap_deletes() {
        // spec: NS-47 NS-24
        // The `all_code` leg, which `review` uses for a non-markdown file, edits
        // by byte span: `unwrap_misplaced` copies up to `r.start` and resumes at
        // `r.end`. A BOM three bytes wide at the front is exactly where an
        // off-by-three would land, and the damage would be silent -- three bytes
        // of the wrong text, not a panic. This is a splice-integrity net rather
        // than a test of the strip itself: `all_code` un-wraps every token the
        // scan reports whatever it calls them, so the strip changes no outcome
        // here. What the strip decides is pinned by the test above.
        let script = "\u{feff}#!/bin/sh\n# hand off to {{ns:dev}}\nrun {{ns:do}}\n";
        assert_eq!(
            unwrap_misplaced(script, true),
            (
                "\u{feff}#!/bin/sh\n# hand off to dev\nrun do\n".to_string(),
                2
            )
        );
        // A non-markdown file whose first line happens to be `---` (a YAML
        // document, say) opens the frontmatter read even with a BOM in front,
        // and `all_code` still un-wraps the fields, at the right offsets.
        let yaml = "\u{feff}---\nkey: {{ns:dev}}\nother: {{ns:do}}\n";
        assert_eq!(
            unwrap_misplaced(yaml, true),
            ("\u{feff}---\nkey: dev\nother: do\n".to_string(), 2)
        );
        // And the markdown leg leaves the same fields alone, since a token in a
        // frontmatter field is an ordinary reference.
        assert_eq!(unwrap_misplaced(yaml, false), (yaml.to_string(), 0));
    }

    #[test]
    fn a_bom_on_a_file_with_no_frontmatter_is_behind_its_block_structure() {
        // spec: NS-47
        // A BOM is never part of the body, whether or not frontmatter follows.
        // `mark_frontmatter` returns the BOM width even when it finds no block,
        // so the parser never sees U+FEFF -- which is not whitespace to
        // CommonMark and would otherwise displace the first line's structure.
        let s = sibs(&["dev", "do"]);
        // A fence opened on line one is a fence: its sample is code, so the bare
        // name in it is left alone, and the closer pairs correctly so the prose
        // below it is prose and gets wrapped.
        let doc = "\u{feff}```sh\nmind learn dev\n```\n\nThen see dev.\n";
        let (out, n) = templatize(doc, &s);
        assert_eq!(n, 1, "{out}");
        assert_eq!(
            out, "\u{feff}```sh\nmind learn dev\n```\n\nThen see {{ns:dev}}.\n",
            "the sample is code and the prose below it is wrapped"
        );
        // A tilde fence, which has no inline spelling to soften a miss.
        let doc = "\u{feff}~~~sh\nmind learn dev\n~~~\n";
        assert_eq!(templatize(doc, &s), (doc.to_string(), 0));
        // One line down, the BOM is behind the block structure and the same
        // fence is read correctly, which is what bounds the blast radius: only
        // a construct on the document's very first line is affected.
        let doc = "\u{feff}Intro.\n\n~~~sh\nmind learn dev\n~~~\n";
        assert_eq!(templatize(doc, &s), (doc.to_string(), 0));
        // And with frontmatter in front of it -- the shape every SKILL.md has --
        // the body is parsed on its own and the fence is a fence again.
        let doc = "\u{feff}---\nname: thing\n---\n```sh\nmind learn dev\n```\n\nThen see do.\n";
        let (out, n) = templatize(doc, &s);
        assert_eq!(n, 1, "{out}");
        assert_eq!(
            out,
            "\u{feff}---\nname: thing\n---\n```sh\nmind learn dev\n```\n\n\
             Then see {{ns:do}}.\n"
        );
    }

    #[test]
    fn a_bom_prefixed_file_with_no_frontmatter_still_sees_its_first_line_fence() {
        // spec: NS-47 NS-24 NS-48
        // The correct behavior, asserted so the defect is demonstrated rather
        // than described. A `.md` file inside an item need not carry
        // frontmatter (a REFERENCE.md or a README.md does not), `review --fix`
        // rewrites it all the same, and a BOM is what a Windows editor leaves
        // in front of it. Both directions of the damage are here: wrapping
        // rewrites the code sample, and un-wrapping deletes the token in the
        // prose below, which is the filed bug's exact symptom.
        let s = sibs(&["dev", "do"]);
        let doc = "\u{feff}```sh\nmind learn dev\n```\n\nThen see do.\n";
        let (out, n) = templatize(doc, &s);
        assert_eq!(n, 1, "{out}");
        assert_eq!(
            out, "\u{feff}```sh\nmind learn dev\n```\n\nThen see {{ns:do}}.\n",
            "the sample keeps its bare name and the prose below it is wrapped"
        );
        let doc = "\u{feff}```sh\nmind learn {{ns:dev}}\n```\n\nThen see {{ns:do}}.\n";
        assert_eq!(
            contexts(doc),
            vec![
                ("dev".into(), NsContext::CodeBlock),
                ("do".into(), NsContext::Prose),
            ]
        );
        assert_eq!(
            surviving(doc),
            "\u{feff}```sh\nmind learn dev\n```\n\nThen see {{ns:do}}.\n",
            "`--fix` must not delete the prose token below the fence"
        );
    }

    // ---- inert_tokens (CLI-223): the non-markdown-file token net -------------
    // These lock the tokenizer `inert_tokens` shares with `strip_braced`
    // (`scan_braced`) and cross-check it against what the expanders (`expand`,
    // `expand_paths`) actually treat as a token. The load-bearing direction:
    // `inert_tokens` must never MISS a span that covers a token an install would
    // expand (a false-clean review that lets a live token silently die in a
    // non-markdown file); over-reporting an inert brace construct is the safe
    // way to be wrong.

    #[test]
    fn inert_tokens_reports_each_braced_span_as_written_deduped() {
        // spec: CLI-223
        // Braces included, first-seen order, de-duplicated across token kinds.
        assert_eq!(
            inert_tokens("a {{ns:x}} b {{tools:t}} c {{ns:x}} d"),
            vec!["{{ns:x}}".to_string(), "{{tools:t}}".to_string()],
        );
        // The report is the file's literal text: interior whitespace is kept,
        // not trimmed the way an expander trims before resolving.
        assert_eq!(inert_tokens("{{ self }}"), vec!["{{ self }}".to_string()]);
        // No braces -> nothing.
        assert!(inert_tokens("plain text, no tokens").is_empty());
    }

    #[test]
    fn inert_tokens_agree_with_expanders_on_unterminated_and_empty() {
        // spec: CLI-223
        let store = Path::new("/m/store");
        let none = None;
        let c = ctx(store, &none, ItemKind::Skill, "self", &[]);

        // An unterminated `{{` is not a token to any expander, and inert_tokens
        // must agree: reporting it would be a false positive no install could
        // ever realize. Every parser stops at the missing `}}`.
        assert!(inert_tokens("#!/bin/sh\n{{tools:detect --scan\n").is_empty());
        assert!(referenced_names("{{ns:x").is_empty());
        assert_eq!(expand_paths("{{self", &c).unwrap(), "{{self");
        assert_eq!(
            expand("{{ns:x", &none, &sibs(&["x"]), &sibs(&[])).unwrap(),
            "{{ns:x",
        );

        // `{{}}` is a well-formed brace span, so `scan_braced`/`inert_tokens`
        // report it, but no expander treats an empty token as a reference. This
        // is the accepted over-report direction: named by review, expanded by
        // nobody, so it can never be a silently-dropped live reference.
        assert_eq!(inert_tokens("{{}}"), vec!["{{}}".to_string()]);
        assert_eq!(expand_paths("{{}}", &c).unwrap(), "{{}}");
        assert_eq!(
            expand("{{}}", &none, &sibs(&[]), &sibs(&[])).unwrap(),
            "{{}}",
        );
    }

    #[test]
    fn inert_tokens_never_misses_a_token_the_path_expander_would_expand() {
        // spec: CLI-223
        // `scan_braced` consumes greedily to the first `}}` then resumes past
        // it, whereas `expand_paths` resumes just past a passthrough `{{`, so
        // the two disagree about token BOUNDARIES on a nested construct. What
        // must never happen is `inert_tokens` failing to name a span that
        // CONTAINS a token `expand_paths` expands -- that would be a review
        // reporting "no tokens here" while the install silently drops a live one.
        let store = Path::new("/m/store");
        let none = None;
        let sib = [psib(ItemKind::Tool, "detect", Some("detect"))];
        let c = ctx(store, &none, ItemKind::Skill, "run", &sib);

        let nested = "{{ {{tools:detect}} }}";
        let expanded = expand_paths(nested, &c).unwrap();
        assert_ne!(
            expanded, nested,
            "the inner path token really does expand once the outer passes through"
        );
        let reported = inert_tokens(nested);
        assert!(
            reported.iter().any(|t| t.contains("tools:detect")),
            "inert_tokens must name a span covering the token expand_paths \
             expands, never miss it: {reported:?}"
        );

        // The same holds for an `{{ns:}}` token nested inside a passthrough span:
        // `expand` reaches the inner token and rewrites it, and inert_tokens
        // names a span covering it.
        let nested_ns = "{{ {{ns:detect}} }}";
        let ns_expanded = expand(nested_ns, &none, &sibs(&["detect"]), &sibs(&[])).unwrap();
        assert_ne!(
            ns_expanded, nested_ns,
            "the inner ns token really does expand"
        );
        assert!(
            inert_tokens(nested_ns)
                .iter()
                .any(|t| t.contains("ns:detect")),
            "inert_tokens must also cover a nested ns token"
        );
    }

    #[test]
    fn inert_tokens_dedup_is_linear_and_correct_on_many_distinct_tokens() {
        // spec: CLI-223
        // `inert_tokens` dedups via a `HashSet` seen-set, O(n) over the token
        // count, rather than the O(k^2) `out.iter().any(...)` scan the previous
        // implementation used: `review` is the tool meant to safely inspect a
        // hostile source, and a source can ship a non-markdown file packed with
        // an attacker-controlled number of distinct `{{...}}` tokens, so a
        // quadratic dedup would let review itself become the denial-of-service
        // vector it exists to guard against. A timing assertion would be
        // brittle, so this pins correctness (every distinct token reported
        // exactly once, in first-seen order) on an input large enough that an
        // accidental O(k^2) regression would be conspicuous in a profiler even
        // though the test itself only asserts on output shape.
        let mut content = String::new();
        for i in 0..5000 {
            content.push_str(&format!("{{{{ns:x{i}}}}} "));
        }
        // Re-append the very first and very last tokens again, out of order, so
        // a dedup bug that only catches adjacent duplicates (as an `out.last()`
        // shortcut would) is still caught.
        content.push_str("{{ns:x0}} {{ns:x4999}} ");

        let tokens = inert_tokens(&content);
        assert_eq!(
            tokens.len(),
            5000,
            "5000 distinct tokens plus 2 repeats must dedup to exactly 5000"
        );
        // First-seen order is preserved.
        assert_eq!(tokens[0], "{{ns:x0}}");
        assert_eq!(tokens[1], "{{ns:x1}}");
        assert_eq!(tokens[4999], "{{ns:x4999}}");
        // No duplicates.
        let unique: HashSet<&String> = tokens.iter().collect();
        assert_eq!(
            unique.len(),
            tokens.len(),
            "every reported token is distinct"
        );
    }

    #[test]
    fn strip_braced_leaves_an_unterminated_token_verbatim() {
        // spec: CLI-223
        // Pins the `scan_braced` refactor's behavior for an unterminated `{{`
        // (no closing `}}` anywhere after it): unlike a *closed* span, which is
        // masked to a single space, an unterminated one -- and everything from
        // it to the end of the content -- is left completely untouched, the
        // same "leave the rest verbatim" treatment `expand`/`expand_paths` give
        // an unterminated token. `bare_tool_refs` is `strip_braced`'s only
        // caller, so drive the pin through it: a tool name that appears only
        // inside the unterminated span must still read as bare prose there
        // (unmasked), while one appearing before it is correctly masked.
        let sib = [
            psib(ItemKind::Tool, "before", None),
            psib(ItemKind::Tool, "after", None),
        ];
        // `before` sits inside a well-formed, closed token (masked); `after`
        // sits inside the unterminated tail (left verbatim, so it reads as an
        // ordinary bare prose mention and IS reported).
        let content = "{{tools:before}} unterminated {{tools:after and more text";
        let found = bare_tool_refs(content, &sib);
        assert!(
            !found.contains(&"before".to_string()),
            "a name inside a closed, masked token must not read as bare prose: {found:?}"
        );
        assert!(
            found.contains(&"after".to_string()),
            "a name inside the unterminated tail is left verbatim, so it reads \
             as an ordinary bare mention: {found:?}"
        );
    }

    // ---- sibling_reference_tokens (M2, M3) --------------------------------

    fn sref(token: &str, name: &str, kind: Option<ItemKind>) -> SiblingRef {
        SiblingRef {
            token: token.to_string(),
            name: name.to_string(),
            kind,
        }
    }

    struct SiblingRefCase {
        label: &'static str,
        content: &'static str,
        expected: Vec<SiblingRef>,
    }

    #[test]
    fn sibling_reference_tokens_table() {
        // spec: NS-10, TOOL-15, TOOL-18
        let cases = vec![
            SiblingRefCase {
                label: "a plain {{ns:name}} token, matching expand's literal scan",
                content: "hand off to {{ns:dev}}.",
                expected: vec![sref("{{ns:dev}}", "dev", None)],
            },
            SiblingRefCase {
                label: "whitespace INSIDE the ns: token is trimmed off the name, \
                         matching expand's trim of the name",
                content: "see {{ns: dev }}",
                expected: vec![sref("{{ns: dev }}", "dev", None)],
            },
            SiblingRefCase {
                // M2: `expand` only scans for the literal `{{ns:` substring, so
                // a space between the braces and `ns:` is not a token there and
                // never expands. This scanner must agree, not over-report.
                label: "whitespace BETWEEN the braces and ns: is not a token (M2)",
                content: "Hand off to {{ ns:dev }}.",
                expected: vec![],
            },
            SiblingRefCase {
                label: "a {{tools:name}} token, matching expand_paths",
                content: "run {{tools:build}}",
                expected: vec![sref("{{tools:build}}", "build", Some(ItemKind::Tool))],
            },
            SiblingRefCase {
                // expand_paths trims the WHOLE inner span before testing, so
                // (unlike ns:) whitespace before the tools:/path: prefix is
                // still a live token there, and this scanner must still report
                // it.
                label: "whitespace between the braces and tools: IS a token, \
                         matching expand_paths' trim-then-test",
                content: "run {{ tools:build }}",
                expected: vec![sref("{{ tools:build }}", "build", Some(ItemKind::Tool))],
            },
            SiblingRefCase {
                label: "a kind-qualified {{path:kind:name}} token",
                content: "see {{path:skill:dev}}",
                expected: vec![sref("{{path:skill:dev}}", "dev", Some(ItemKind::Skill))],
            },
            SiblingRefCase {
                label: "an unqualified {{path:name}} token matches any kind",
                content: "see {{path:dev}}",
                expected: vec![sref("{{path:dev}}", "dev", None)],
            },
            SiblingRefCase {
                // The `kind` segment does not parse, so it is not a qualifier:
                // `resolve_token` fails immediately (Bad(NoMatch)), and this
                // scanner reports the whole reference text as the (unmatchable)
                // name -- both agree the token cannot resolve.
                label: "{{path:notakind:name}} treats the whole reference as the name",
                content: "see {{path:notakind:name}}",
                expected: vec![sref("{{path:notakind:name}}", "notakind:name", None)],
            },
            SiblingRefCase {
                label: "{{self}} is excluded, but the scan resumes after it",
                content: "{{self}} then {{ns:dev}}",
                expected: vec![sref("{{ns:dev}}", "dev", None)],
            },
            SiblingRefCase {
                label: "an unrecognized token is passed through, and the scan \
                         resumes just past its opening brace",
                content: "{{foo}}{{ns:dev}}",
                expected: vec![sref("{{ns:dev}}", "dev", None)],
            },
            SiblingRefCase {
                // The passthrough resume rule must back up to just past the
                // FIRST `{{`, not past the whole failed span. Here the failed
                // reading is `{{ns:dev` (closed early by the `}}` inside
                // `dev}}`), and only resuming right after the outermost `{{`
                // finds the real `{{ns:dev}}` nested one brace-pair in.
                // Resuming past the failed span instead (as if the passthrough
                // arm were `rest = &after[end + 2..]`) skips straight past the
                // end of content and misses it entirely -- this case must fail
                // under that mutation (M3).
                label: "nested {{{{ns:dev}} is still found by resuming just \
                         past the opening brace, not past the failed span",
                content: "{{{{ns:dev}}",
                expected: vec![sref("{{ns:dev}}", "dev", None)],
            },
            SiblingRefCase {
                label: "an unterminated token stops the scan entirely",
                content: "{{ns:dev}} {{ns:oops and more text with no closing braces",
                expected: vec![sref("{{ns:dev}}", "dev", None)],
            },
            SiblingRefCase {
                label: "duplicate token text is deduped to the first occurrence",
                content: "{{ns:dev}} again {{ns:dev}}",
                expected: vec![sref("{{ns:dev}}", "dev", None)],
            },
            SiblingRefCase {
                // Dedup keys on TOKEN TEXT, not referent name: these two tokens
                // name the same sibling but are spelled differently, so both
                // are reported.
                label: "same referent name, different token text: both reported",
                content: "{{ns:dev}} {{ns: dev }}",
                expected: vec![
                    sref("{{ns:dev}}", "dev", None),
                    sref("{{ns: dev }}", "dev", None),
                ],
            },
            SiblingRefCase {
                label: "an empty {{ns:}} name is still reported, matching \
                         expand's rejection of an empty referent",
                content: "{{ns:}}",
                expected: vec![sref("{{ns:}}", "", None)],
            },
        ];

        for case in cases {
            assert_eq!(
                sibling_reference_tokens(case.content),
                case.expected,
                "case: {}",
                case.label
            );
        }
    }

    /// Dedicated pin for the M3-required mutation catch: the passthrough arm
    /// must resume scanning just past the FIRST `{{` (`rest = after`), not
    /// past the whole failed reading (`rest = &after[end + 2..]`). The table
    /// case above covers this too; this test isolates it so a regression here
    /// fails loudly and specifically rather than as one row among many.
    #[test]
    fn sibling_reference_tokens_passthrough_resumes_past_the_open_brace_not_the_failed_span() {
        // spec: NS-10
        assert_eq!(
            sibling_reference_tokens("{{{{ns:dev}}"),
            vec![sref("{{ns:dev}}", "dev", None)],
            "resuming past the failed span instead of past the opening brace \
             would miss the nested {{{{ns:dev}} token entirely"
        );
    }

    /// M2: the `ns:` prefix test must agree with [`expand`], the real
    /// install-time expander, not merely with a hand-derived expectation.
    #[test]
    fn sibling_reference_tokens_ns_prefix_test_agrees_with_expand() {
        // spec: NS-10, LNK-18
        let siblings = sibs(&["dev"]);
        let bare = HashSet::new();

        // A well-formed token: both agree it references `dev`.
        let content = "hand off to {{ns:dev}}.";
        assert_eq!(
            sibling_reference_tokens(content),
            vec![sref("{{ns:dev}}", "dev", None)]
        );
        assert_eq!(
            expand(content, &None, &siblings, &bare).unwrap(),
            "hand off to dev."
        );

        // A space between the braces and `ns:`: `expand` leaves it completely
        // untouched (dead text, never a reference), so the scanner must agree
        // and report nothing.
        let spaced = "hand off to {{ ns:dev }}.";
        assert_eq!(sibling_reference_tokens(spaced), vec![]);
        assert_eq!(expand(spaced, &None, &siblings, &bare).unwrap(), spaced);
    }

    /// M2 (converse): the `tools:`/`path:` prefix tests must keep agreeing with
    /// [`expand_paths`], which trims the whole span before testing -- so
    /// whitespace before those prefixes is still a live token, unlike `ns:`.
    #[test]
    fn sibling_reference_tokens_tools_prefix_test_agrees_with_expand_paths() {
        // spec: TOOL-15, LNK-18
        let sib = [psib(ItemKind::Tool, "build", Some("run.sh"))];
        let ctx = PathCtx {
            store_root: Path::new("/store"),
            home: None,
            prefix: &None,
            self_kind: ItemKind::Skill,
            self_name: "self-item",
            siblings: &sib,
        };

        let content = "run {{ tools:build }}";
        assert_eq!(
            sibling_reference_tokens(content),
            vec![sref("{{ tools:build }}", "build", Some(ItemKind::Tool))]
        );
        let expanded = expand_paths(content, &ctx).unwrap();
        assert!(
            expanded.contains("run.sh"),
            "expand_paths must still resolve the spaced tools: token: {expanded}"
        );
    }
}
