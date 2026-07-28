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
/// confined to prose (NS-24): text already inside a `{{ns:}}` token, a fenced
/// code block, an inline code span, the leading frontmatter, or a path-adjacent
/// position is left untouched, so a keyword or path component is never wrapped.
/// Still heuristic in prose (a sibling name can be an ordinary word), so callers
/// (init-source) keep it opt-in and reviewable, and apply it only to markdown.
pub fn templatize(content: &str, siblings: &HashSet<String>) -> (String, usize) {
    let mut out = String::with_capacity(content.len());
    let mut count = 0;
    // One structural read of the document (NS-46/NS-47): the same map the
    // misplaced-token scan uses, so wrapping and un-wrapping cannot disagree
    // about what is code.
    let doc = Structure::new(content);
    let mut offset = 0usize;
    for (idx, raw) in content.split_inclusive('\n').enumerate() {
        let line = raw.strip_suffix('\n').unwrap_or(raw);
        let nl = &raw[line.len()..];
        if doc.kinds[idx] == LineKind::Text {
            let (wrapped, n) = wrap_line(line, siblings, &doc, offset);
            out.push_str(&wrapped);
            out.push_str(nl);
            count += n;
        } else {
            // Frontmatter, a fence delimiter, or fenced content: never wrapped.
            out.push_str(raw);
        }
        offset += raw.len();
    }
    (out, count)
}

/// Wrap bare sibling names in one prose line, skipping existing `{{...}}` tokens,
/// inline code spans, and path-adjacent positions. `base` is the line's byte
/// offset in the document, used to test each word against the document-wide
/// code-span ranges (NS-46).
fn wrap_line(
    line: &str,
    siblings: &HashSet<String>,
    doc: &Structure,
    base: usize,
) -> (String, usize) {
    let chars: Vec<char> = line.chars().collect();
    let mut out = String::with_capacity(line.len());
    let mut count = 0;
    let mut word = String::new();
    // Byte offset in the document of `chars[i]`, and of the current word's start.
    let mut pos = base;
    let mut word_start = base;
    let mut before: Option<char> = None;
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        // Copy an existing `{{...}}` token verbatim (do not re-wrap inside it).
        if c == '{' && chars.get(i + 1) == Some(&'{') {
            count += emit_word(
                &word,
                siblings,
                doc.in_code_span(word_start),
                before,
                None,
                &mut out,
            );
            word.clear();
            let start = i;
            i += 2;
            while i + 1 < chars.len() && !(chars[i] == '}' && chars[i + 1] == '}') {
                i += 1;
            }
            i = if i + 1 < chars.len() {
                i + 2
            } else {
                chars.len()
            };
            for &ch in &chars[start..i] {
                out.push(ch);
                pos += ch.len_utf8();
            }
            before = Some('}');
            continue;
        }
        if is_word_char(c) {
            if word.is_empty() {
                word_start = pos;
            }
            word.push(c);
            i += 1;
            pos += c.len_utf8();
            continue;
        }
        count += emit_word(
            &word,
            siblings,
            doc.in_code_span(word_start),
            before,
            Some(c),
            &mut out,
        );
        word.clear();
        out.push(c);
        before = Some(c);
        i += 1;
        pos += c.len_utf8();
    }
    count += emit_word(
        &word,
        siblings,
        doc.in_code_span(word_start),
        before,
        None,
        &mut out,
    );
    (out, count)
}

/// Emit one word: wrapped as a `{{ns:}}` token when it is a sibling name in a
/// prose position, else verbatim. Returns 1 if wrapped. A word inside a code
/// span or abutting a path separator (`/`/`~`) is never wrapped (NS-24).
fn emit_word(
    word: &str,
    siblings: &HashSet<String>,
    in_span: bool,
    before: Option<char>,
    after: Option<char>,
    out: &mut String,
) -> usize {
    if word.is_empty() {
        return 0;
    }
    let path_adj = matches!(before, Some('/') | Some('~')) || matches!(after, Some('/'));
    if !in_span && !path_adj && siblings.contains(word) {
        out.push_str("{{ns:");
        out.push_str(word);
        out.push_str("}}");
        1
    } else {
        out.push_str(word);
        0
    }
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

/// Extended reserved prefix words (NS-29 / DEC-9): plausible future item-kind
/// or CLI-subsystem names that are banned pre-emptively.
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
/// must not contain `/`, `\`, `:`, NUL, or any ASCII control character (0x00-0x1F
/// or 0x7F).  The Path component check is belt-and-suspenders: it rejects anything
/// the byte-level scan would miss on unusual platforms.
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
    // Belt-and-suspenders: exactly one Normal path component.
    let mut comps = std::path::Path::new(prefix).components();
    matches!(comps.next(), Some(std::path::Component::Normal(_))) && comps.next().is_none()
}

/// Validate that `prefix` is safe to use as a namespace prefix (NS-25, NS-28, NS-29).
///
/// Rejects any prefix that:
/// - is a reserved item-kind word (`skill`, `agent`, `rule`, `tool`; NS-25), or
/// - is in the extended DEC-9 reserved list (NS-29), or
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
    // NS-29: reject extended DEC-9 reserved words.
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
    // An agent/rule file is `<name>.md`; the store copies it as a bare `<name>`,
    // so stripping a `.md` suffix is correct for both layouts and a no-op for the
    // store form.
    let name = match kind {
        crate::error::ItemKind::Agent | crate::error::ItemKind::Rule => {
            first.strip_suffix(".md").unwrap_or(first).to_string()
        }
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

/// Replace every `{{...}}` span with a space, so prose scanning ignores anything
/// already inside a reference token (any token kind, not just `{{ns:}}`).
fn strip_braced(content: &str) -> String {
    let mut out = String::with_capacity(content.len());
    let mut rest = content;
    while let Some(pos) = rest.find("{{") {
        out.push_str(&rest[..pos]);
        let after = &rest[pos + 2..];
        match after.find("}}") {
            Some(end) => {
                out.push(' ');
                rest = &after[end + 2..];
            }
            None => {
                rest = "";
                break;
            }
        }
    }
    out.push_str(rest);
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

/// Find sibling names referenced in bare prose (outside any `{{...}}` token).
///
/// Heuristic and advisory: used to warn when a source is about to be prefixed
/// but references siblings without the token that would keep them resolvable.
/// A sibling name that already appears inside any token kind (`{{ns:}}`,
/// `{{tools:}}`, `{{path:}}`, `{{self}}`) is correctly guarded and is NOT
/// reported; only names in genuinely bare prose are flagged.
pub fn unguarded_refs(content: &str, siblings: &HashSet<String>) -> Vec<String> {
    let stripped = strip_braced(content);
    let mut found: Vec<String> = siblings
        .iter()
        .filter(|name| whole_word_present(&stripped, name))
        .cloned()
        .collect();
    found.sort();
    found
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

/// What one line of a markdown document is, structurally (NS-47).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LineKind {
    /// A `---` delimiter of the leading frontmatter block.
    FrontmatterDelim,
    /// A line inside the leading frontmatter block.
    Frontmatter,
    /// A fence opener or closer line.
    FenceDelim,
    /// A line inside a fenced code block.
    CodeBlock,
    /// Ordinary text: prose, possibly carrying inline code spans.
    Text,
}

/// The structure of a markdown document: what each line is (NS-47) and where its
/// inline code spans are (NS-46).
///
/// Both are read document-wide rather than line by line, because both constructs
/// are document-wide: a code span may cross a line break inside a paragraph, and
/// a fence closes only on a run of its own character that is at least as long as
/// the opener, so an inner example fence does not end the outer block.
struct Structure {
    /// One entry per line of the document, in order.
    kinds: Vec<LineKind>,
    /// Byte ranges of inline code spans, sorted and non-overlapping.
    spans: Vec<(usize, usize)>,
}

impl Structure {
    fn new(content: &str) -> Self {
        let kinds = line_kinds(content);
        let spans = code_spans(content, &kinds);
        Structure { kinds, spans }
    }

    /// True when byte position `pos` falls inside a matched inline code span.
    fn in_code_span(&self, pos: usize) -> bool {
        self.spans.iter().any(|&(s, e)| pos >= s && pos < e)
    }
}

/// The fence delimiter a line leads with, if any: the fence character, the length
/// of its run, and the trimmed text after the run (the info string on an opener,
/// which must be empty on a closer). A run shorter than three is not a fence.
fn fence_delim(line: &str) -> Option<(char, usize, &str)> {
    let body = line.trim_start();
    let ch = body.chars().next()?;
    if ch != '`' && ch != '~' {
        return None;
    }
    // Both fence characters are one byte, so the run length is a byte count too.
    let len = body.chars().take_while(|&c| c == ch).count();
    if len < 3 {
        return None;
    }
    Some((ch, len, body[len..].trim()))
}

/// The visual indentation of `line` in columns, with a tab advancing to the next
/// multiple of four (NS-49).
fn indent_cols(line: &str) -> usize {
    let mut col = 0;
    for c in line.chars() {
        match c {
            ' ' => col += 1,
            '\t' => col += 4 - (col % 4),
            _ => break,
        }
    }
    col
}

/// The content column a list-item marker on `line` opens, when the line starts
/// one (NS-49). Everything inside the item is written from that column, so it is
/// the baseline the fence-indent and indented-code rules measure against: a
/// fence nested in a list item is indented but is still a fence.
fn list_item_content_col(line: &str) -> Option<usize> {
    let body = line.trim_start();
    let first = body.chars().next()?;
    let marker = match first {
        '-' | '*' | '+' => 1,
        '0'..='9' => {
            let digits = body.chars().take_while(char::is_ascii_digit).count();
            // CommonMark caps an ordered-list marker at nine digits.
            if digits > 9 {
                return None;
            }
            let rest = &body[digits..];
            if !(rest.starts_with('.') || rest.starts_with(')')) {
                return None;
            }
            digits + 1
        }
        _ => return None,
    };
    let after = &body[marker..];
    let spaces = after.chars().take_while(|&c| c == ' ' || c == '\t').count();
    // The marker must be followed by whitespace, or end the line (an empty item).
    if spaces == 0 && !after.is_empty() {
        return None;
    }
    // One to four spaces put the content right after them. More of them start an
    // indented code block *inside* the item, whose content column is one past
    // the marker; so does an empty item.
    let width = if (1..=4).contains(&spaces) { spaces } else { 1 };
    Some(indent_cols(line) + marker + width)
}

/// Whether `line` is an ATX heading, which is a leaf block of its own and so
/// leaves no paragraph open behind it (NS-49).
fn is_atx_heading(line: &str) -> bool {
    let body = line.trim_start();
    let hashes = body.chars().take_while(|&c| c == '#').count();
    (1..=6).contains(&hashes)
        && (body[hashes..].is_empty() || body[hashes..].starts_with([' ', '\t']))
}

/// Classify every line of `content` (NS-47, NS-49). The leading `---` frontmatter
/// block is recognized first, as before; outside it a fenced code block opens on
/// a run of at least three backticks or tildes and closes only on a run of the
/// same character that is at least as long, with nothing but whitespace after it.
///
/// Indentation is measured against the content column of the innermost open list
/// item (NS-49), so a fence nested in a list item is recognized while a line
/// indented four or more columns past that baseline is indented code (or a lazy
/// paragraph continuation) and can never be a delimiter.
fn line_kinds(content: &str) -> Vec<LineKind> {
    let mut kinds = Vec::new();
    let mut in_frontmatter = false;
    // The open fence's character and run length, while a block is open.
    let mut fence: Option<(char, usize)> = None;
    // The content column the open fence was written from (NS-49).
    let mut fence_base = 0usize;
    // Content columns of the list items currently open, outermost first (NS-49).
    let mut containers: Vec<usize> = Vec::new();
    // Whether the previous line left a paragraph open, so an indented line
    // continues it lazily rather than starting an indented code block (NS-49).
    let mut paragraph = false;
    for (idx, raw) in content.split_inclusive('\n').enumerate() {
        let line = raw.strip_suffix('\n').unwrap_or(raw);
        let trimmed = line.trim();
        if idx == 0 && trimmed == "---" {
            in_frontmatter = true;
            kinds.push(LineKind::FrontmatterDelim);
            continue;
        }
        if in_frontmatter {
            if trimmed == "---" {
                in_frontmatter = false;
                kinds.push(LineKind::FrontmatterDelim);
            } else {
                kinds.push(LineKind::Frontmatter);
            }
            continue;
        }
        let blank = trimmed.is_empty();
        let ind = indent_cols(line);
        // A fence opened inside a list item ends with the item (NS-49): its
        // content is written from the item's content column, so a non-blank line
        // that dedents past it leaves both. Without this an unclosed fence in a
        // list item would swallow the rest of the document.
        if fence.is_some() && !blank && ind < fence_base {
            fence = None;
        }
        // A non-blank line that dedents past an open list item's content column
        // leaves the item. A blank line does not: an item may contain them, and
        // the fenced or indented block after one is still inside the item.
        if fence.is_none() && !blank {
            while containers.last().is_some_and(|&col| ind < col) {
                containers.pop();
            }
        }
        let base = containers.last().copied().unwrap_or(0);
        // Four or more columns past the containing block's content column is
        // indented code, so the leading run there is literal text, not a fence
        // delimiter (NS-49).
        let delim = if ind >= base + 4 {
            None
        } else {
            fence_delim(line)
        };
        let kind = match (fence, delim) {
            // Inside a block: only a matching, long-enough, bare run closes it.
            // A shorter run, a different character, or a run with an info string
            // is content (a nested example fence).
            (Some((fc, flen)), Some((c, len, rest))) => {
                if c == fc && len >= flen && rest.is_empty() {
                    fence = None;
                    LineKind::FenceDelim
                } else {
                    LineKind::CodeBlock
                }
            }
            (Some(_), None) => LineKind::CodeBlock,
            // Outside a block: a backtick fence's info string may not contain a
            // backtick, so such a line is ordinary text carrying code spans.
            (None, Some((c, len, rest))) => {
                if c == '`' && rest.contains('`') {
                    LineKind::Text
                } else {
                    fence = Some((c, len));
                    fence_base = base;
                    LineKind::FenceDelim
                }
            }
            // An indented line that does not continue a paragraph is an indented
            // code block; one that does is a lazy continuation, so still prose.
            (None, None) => {
                if !blank && ind >= base + 4 && !paragraph {
                    LineKind::CodeBlock
                } else {
                    LineKind::Text
                }
            }
        };
        if kind == LineKind::Text
            && !blank
            && let Some(col) = list_item_content_col(line)
        {
            containers.push(col);
        }
        paragraph = kind == LineKind::Text && !blank && !is_atx_heading(line);
        kinds.push(kind);
    }
    kinds
}

/// Whether `line` starts a new leaf block (a heading, blockquote, list item, or
/// table row) rather than continuing the paragraph above it. Bounds code-span
/// matching (NS-46): two list items are two blocks, so a backtick in one cannot
/// open a span that closes in the next.
fn starts_leaf_block(line: &str) -> bool {
    let body = line.trim_start();
    let mut chars = body.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    match first {
        '#' => {
            let hashes = body.chars().take_while(|&c| c == '#').count();
            hashes <= 6 && body[hashes..].starts_with([' ', '\t'])
        }
        '>' | '|' => true,
        '-' | '*' | '+' => matches!(chars.next(), Some(' ') | Some('\t')),
        '0'..='9' => {
            let digits = body.chars().take_while(char::is_ascii_digit).count();
            let rest = &body[digits..];
            (rest.starts_with('.') || rest.starts_with(')')) && rest[1..].starts_with([' ', '\t'])
        }
        _ => false,
    }
}

/// Byte ranges of the inline code spans in `content` (NS-46).
///
/// Only text lines are considered: frontmatter, fence delimiters, and fenced
/// content are handled by [`line_kinds`]. A span may cross a line break inside a
/// paragraph but never a blank line or a block boundary, so matching runs over
/// each maximal run of consecutive non-blank text lines belonging to one block.
fn code_spans(content: &str, kinds: &[LineKind]) -> Vec<(usize, usize)> {
    let mut spans = Vec::new();
    let mut offset = 0usize;
    let mut para: Option<(usize, usize)> = None;
    for (idx, raw) in content.split_inclusive('\n').enumerate() {
        let line = raw.strip_suffix('\n').unwrap_or(raw);
        if kinds[idx] == LineKind::Text && !line.trim().is_empty() {
            let end = offset + raw.len();
            // A new block ends the paragraph the previous lines formed.
            if starts_leaf_block(line)
                && let Some((start, prev_end)) = para.take()
            {
                match_spans(content, start, prev_end, &mut spans);
            }
            para = Some(match para {
                Some((start, _)) => (start, end),
                None => (offset, end),
            });
        } else if let Some((start, end)) = para.take() {
            match_spans(content, start, end, &mut spans);
        }
        offset += raw.len();
    }
    if let Some((start, end)) = para.take() {
        match_spans(content, start, end, &mut spans);
    }
    spans
}

/// Whether the byte at `pos` is backslash-escaped: preceded, within
/// `[start, pos)`, by an odd number of backslashes (NS-50).
fn is_escaped(content: &str, start: usize, pos: usize) -> bool {
    let bytes = content.as_bytes();
    let mut n = 0usize;
    let mut i = pos;
    while i > start && bytes[i - 1] == b'\\' {
        n += 1;
        i -= 1;
    }
    n % 2 == 1
}

/// Match backtick runs within `content[start..end]`, pushing each resulting code
/// span's byte range onto `out` (NS-46). A run opens a span that only a later run
/// of exactly the same length closes; an unmatched run is literal text, and
/// scanning resumes right after it.
///
/// A backslash-escaped backtick is literal text and does not open a span
/// (NS-50). Backslash escapes do not apply *inside* a code span, so only the
/// opener is checked: an escaped backtick still closes a span already open.
fn match_spans(content: &str, start: usize, end: usize, out: &mut Vec<(usize, usize)>) {
    let bytes = content.as_bytes();
    let mut i = start;
    while i < end {
        if bytes[i] != b'`' {
            i += 1;
            continue;
        }
        if is_escaped(content, start, i) {
            i += 1;
            continue;
        }
        let open = i;
        while i < end && bytes[i] == b'`' {
            i += 1;
        }
        let len = i - open;
        let mut j = i;
        let mut close = None;
        while j < end {
            if bytes[j] != b'`' {
                j += 1;
                continue;
            }
            let run = j;
            while j < end && bytes[j] == b'`' {
                j += 1;
            }
            if j - run == len {
                close = Some(j);
                break;
            }
        }
        if let Some(e) = close {
            out.push((open, e));
            i = e;
        }
    }
}

/// True when the token spanning `[start, end)` in `line` abuts a path separator.
fn path_adjacent(line: &str, start: usize, end: usize) -> bool {
    let before = line[..start].chars().next_back();
    let after = line[end..].chars().next();
    matches!(before, Some('/') | Some('~')) || matches!(after, Some('/'))
}

/// Find every `{{ns:name}}` token in `content`, each with its structural context
/// (NS-24) and byte span. Reads the document's structure once (NS-46/NS-47) so a
/// token can be classified as misplaced (in code, a path, or `name:`).
pub fn scan_ns_refs(content: &str) -> Vec<NsRef> {
    const OPEN: &str = "{{ns:";
    let mut out = Vec::new();
    let doc = Structure::new(content);
    let mut offset = 0usize;
    for (idx, raw) in content.split_inclusive('\n').enumerate() {
        let line = raw.strip_suffix('\n').unwrap_or(raw);
        let kind = doc.kinds[idx];
        // A delimiter line is structure, not content: it carries no token.
        if matches!(kind, LineKind::FrontmatterDelim | LineKind::FenceDelim) {
            offset += raw.len();
            continue;
        }
        let in_frontmatter = kind == LineKind::Frontmatter;
        let fm_name = in_frontmatter && line.trim_start().starts_with("name:");
        let mut from = 0;
        while let Some(rel) = line[from..].find(OPEN) {
            let tstart = from + rel;
            let after = &line[tstart + OPEN.len()..];
            let Some(erel) = after.find("}}") else { break };
            let tend = tstart + OPEN.len() + erel + 2;
            let name = after[..erel].trim().to_string();
            let context = if fm_name {
                NsContext::FrontmatterName
            } else if in_frontmatter {
                NsContext::Prose
            } else if kind == LineKind::CodeBlock {
                NsContext::CodeBlock
            } else if doc.in_code_span(offset + tstart) {
                NsContext::CodeSpan
            } else if path_adjacent(line, tstart, tend) {
                NsContext::Path
            } else {
                NsContext::Prose
            };
            if !name.is_empty() {
                out.push(NsRef {
                    name,
                    context,
                    start: offset + tstart,
                    end: offset + tend,
                });
            }
            from = tend;
        }
        offset += raw.len();
    }
    out
}

/// Un-wrap misplaced `{{ns:name}}` tokens (NS-24) back to the bare `name`. With
/// `all_code` false, only non-prose tokens are un-wrapped (the markdown case);
/// with it true, every token is un-wrapped (a non-markdown file, which is all
/// code, where no `{{ns:}}` belongs). Returns the new content and the count.
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
        // spec: NS-25
        for word in ["skill", "agent", "rule", "tool"] {
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
    fn fix_does_not_clear_an_unguarded_mention_outside_prose() {
        // spec: NS-48
        // The boundary NS-48 is scoped to, stated as a test rather than left as
        // prose: `unguarded_refs` is context-free, so a sibling name inside a
        // code span, inside a fence, or in the frontmatter is reported both
        // before and after `--fix`, and `--fix` cannot clear it (un-wrapping put
        // the first two there, and wrapping never touches any of the three).
        let s = sibs(&["dev"]);
        for doc in [
            "run `{{ns:dev}}` now\n",
            "```sh\nmind learn {{ns:dev}}\n```\n",
            "---\nname: thing\ndescription: hand off to dev\n---\nbody\n",
        ] {
            let fixed = fix_passes(doc, &s);
            assert_eq!(
                unguarded_refs(&fixed, &s),
                vec!["dev".to_string()],
                "still reported, and `--fix` has no move left: {fixed}"
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
    fn list_item_content_col_reads_every_marker_shape() {
        // spec: NS-49
        // The baseline every indent rule measures against, tested directly
        // rather than through a document, so a change of shape shows up here
        // instead of as a distant misclassification.
        let col = list_item_content_col;
        // Bullet markers: content starts after the marker plus its spaces.
        assert_eq!(col("- item"), Some(2));
        assert_eq!(col("* item"), Some(2));
        assert_eq!(col("+ item"), Some(2));
        assert_eq!(col("-   item"), Some(4), "three spaces put content at 4");
        assert_eq!(col("-    item"), Some(5), "four spaces still count");
        assert_eq!(
            col("-     item"),
            Some(2),
            "five or more spaces start indented code inside the item, whose \
             content column is one past the marker"
        );
        // An empty item: the marker ends the line.
        assert_eq!(col("-"), Some(2));
        assert_eq!(col("1."), Some(3));
        // Ordered markers, both terminators, and the nine-digit cap.
        assert_eq!(col("1. step"), Some(3));
        assert_eq!(col("1) step"), Some(3));
        assert_eq!(col("10. step"), Some(4));
        assert_eq!(col("1.  step"), Some(4), "two spaces after `1.`");
        assert_eq!(col("1234567890. step"), None, "ten digits is not a marker");
        assert_eq!(col("123456789. step"), Some(11));
        // Nesting adds the line's own indentation.
        assert_eq!(col("  - inner"), Some(4));
        assert_eq!(col("\t- inner"), Some(6), "a tab advances to column 4");
        // Not markers.
        assert_eq!(col("-item"), None, "no space after the marker");
        assert_eq!(col("1.step"), None);
        assert_eq!(col("---"), None, "a thematic break of dashes");
        assert_eq!(col("***"), None);
        assert_eq!(col("text"), None);
        assert_eq!(col(""), None);
        // Known and accepted: a spaced thematic break parses as a list marker,
        // which lifts the indent baseline to column 2 for the lines under it.
        // It cannot delete a token (the fence-dedent rule and the container pop
        // both fire on the next line at column 0), so it is left alone.
        assert_eq!(col("* * *"), Some(2));
        // Known and understated (NS-49): a tab after the marker counts as one
        // column where CommonMark advances to the next multiple of four. That
        // errs toward "this is a fence", never toward deleting prose.
        assert_eq!(col("-\titem"), Some(2), "CommonMark would say 4");
    }

    #[test]
    fn indent_cols_expands_tabs_to_the_next_multiple_of_four() {
        // spec: NS-49
        assert_eq!(indent_cols("x"), 0);
        assert_eq!(indent_cols("    x"), 4);
        assert_eq!(indent_cols("\tx"), 4);
        assert_eq!(indent_cols(" \tx"), 4, "a tab completes the first stop");
        assert_eq!(indent_cols("    \tx"), 8);
        assert_eq!(indent_cols("\t\tx"), 8);
        assert_eq!(indent_cols("  "), 2, "a whitespace-only line");
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
    fn a_spaced_thematic_break_lifts_the_baseline_but_cannot_delete_a_token() {
        // spec: NS-49
        // `* * *` parses as a list marker (see the marker test above), so the
        // lines under it are measured from column 2 rather than column 0. That
        // is wrong, and this is the bound on how wrong: the fence-dedent rule
        // and the container pop both fire on the next line at column 0, so no
        // token can end up in code that a renderer calls prose.
        let doc = "intro\n\n* * *\n\n  ```\n\nThen see {{ns:dev}}.\n";
        assert_eq!(
            contexts(doc),
            vec![("dev".into(), NsContext::Prose)],
            "a fence opened under the break closes when the document dedents"
        );
        // The other direction of the same error: four columns under the break
        // is an indented code block to a renderer and prose here, so wrapping
        // may rewrite a name inside it. Non-destructive to tokens, and pinned
        // so that a change of shape is a decision rather than a surprise.
        let doc = "intro\n\n* * *\n\n    mind learn {{ns:dev}}\n";
        assert_eq!(contexts(doc), vec![("dev".into(), NsContext::Prose)]);
    }

    #[test]
    #[ignore = "defect: a fence opened on a list-marker line is read as prose, \
                and its closer as an opener, so `--fix` deletes every token in \
                the rest of the item"]
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
    #[ignore = "defect: a thematic break, a setext underline, and an HTML block \
                are not block boundaries to the span matcher, so a code span \
                leaks across one and `--fix` deletes the token it covers"]
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
    #[ignore = "defect: a setext underline is treated as ordinary paragraph \
                text, so the indented code block after one is read as a lazy \
                continuation and wrapping rewrites a name inside the sample"]
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
    #[ignore = "defect: wrapping nests a new token inside a `{{ns:}}` token \
                that spans a line break, producing `{{ns:\\n{{ns:dev}} }}`, \
                which `install` then rejects as a bad reference"]
    fn a_token_broken_across_a_line_break_is_left_alone() {
        // spec: NS-24 NS-46
        // `expand` finds a `{{ns:}}` token document-wide, so a token split over
        // a line break is a live reference at install time. The scan and the
        // wrapper both work line by line and neither sees it. Not seeing it is
        // acceptable; wrapping *into* it is not, and that is what happens: the
        // wrapper swallows the rest of the opening line looking for `}}`, then
        // treats the next line as ordinary prose and wraps the name inside the
        // token, producing `{{ns:\n{{ns:dev}}}}`, which `install` then rejects
        // as a bad reference (the source stops installing at all).
        let s = sibs(&["dev"]);
        let doc = "see {{ns:\ndev }} for the handoff\n";
        let (out, n) = templatize(doc, &s);
        assert_eq!(n, 0, "a split token is left alone, not nested: {out}");
        assert_eq!(out, doc);
    }

    #[test]
    #[ignore = "defect: `> ` hides the delimiter, so a blockquoted fence is \
                prose throughout and wrapping rewrites a bare sibling name \
                inside the quoted code"]
    fn a_fence_inside_a_blockquote_is_a_fence() {
        // spec: NS-47
        // `> ` before the delimiter hides it, so the quoted block classifies as
        // prose throughout. That cannot delete a token, but it is the same
        // defect pointed the other way: wrapping rewrites a bare sibling name
        // inside quoted code, and the rewritten sample is wrong for the reader
        // and expands to a prefixed name at install.
        let s = sibs(&["dev"]);
        let (out, n) = templatize("> ```sh\n> mind learn dev\n> ```\n", &s);
        assert_eq!(n, 0, "quoted code is not prose: {out}");
        let doc = "> ```sh\n> mind learn {{ns:dev}}\n> ```\n\nThen see {{ns:do}}.\n";
        assert_eq!(
            contexts(doc),
            vec![
                ("dev".into(), NsContext::CodeBlock),
                ("do".into(), NsContext::Prose)
            ]
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
}
