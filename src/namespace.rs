//! Source namespacing: prefixing every item from a source, and rewriting the
//! intra-source references that prefixing would otherwise break.
//!
//! A source's *effective prefix* is the consumer's `--as` alias if set, else the
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
    let mut in_fence = false;
    let mut in_frontmatter = false;
    for (idx, raw) in content.split_inclusive('\n').enumerate() {
        let line = raw.strip_suffix('\n').unwrap_or(raw);
        let nl = &raw[line.len()..];
        let trimmed = line.trim();
        if idx == 0 && trimmed == "---" {
            in_frontmatter = true;
            out.push_str(raw);
            continue;
        }
        if in_frontmatter {
            if trimmed == "---" {
                in_frontmatter = false;
            }
            out.push_str(raw); // never wrap inside frontmatter
            continue;
        }
        if trimmed.starts_with("```") {
            in_fence = !in_fence;
            out.push_str(raw);
            continue;
        }
        if in_fence {
            out.push_str(raw); // never wrap inside a code block
            continue;
        }
        let (wrapped, n) = wrap_line(line, siblings);
        out.push_str(&wrapped);
        out.push_str(nl);
        count += n;
    }
    (out, count)
}

/// Wrap bare sibling names in one prose line, skipping existing `{{...}}` tokens,
/// inline code spans, and path-adjacent positions.
fn wrap_line(line: &str, siblings: &HashSet<String>) -> (String, usize) {
    let chars: Vec<char> = line.chars().collect();
    let mut out = String::with_capacity(line.len());
    let mut count = 0;
    let mut word = String::new();
    let mut in_span = false;
    let mut before: Option<char> = None;
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        // Copy an existing `{{...}}` token verbatim (do not re-wrap inside it).
        if c == '{' && chars.get(i + 1) == Some(&'{') {
            count += emit_word(&word, siblings, in_span, before, None, &mut out);
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
            }
            before = Some('}');
            continue;
        }
        if c == '`' {
            count += emit_word(&word, siblings, in_span, before, Some('`'), &mut out);
            word.clear();
            in_span = !in_span;
            out.push(c);
            before = Some('`');
            i += 1;
            continue;
        }
        if is_word_char(c) {
            word.push(c);
            i += 1;
            continue;
        }
        count += emit_word(&word, siblings, in_span, before, Some(c), &mut out);
        word.clear();
        out.push(c);
        before = Some(c);
        i += 1;
    }
    count += emit_word(&word, siblings, in_span, before, None, &mut out);
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

/// Validate that `prefix` is safe to use as a namespace prefix (NS-25).
///
/// Rejects any prefix that is also a reserved item-kind word (`skill`, `agent`,
/// `rule`, `tool`): such a prefix would make `prefix:name` indistinguishable
/// from a kind-qualified item ref and break ref parsing. An empty prefix is
/// always accepted (it means "no prefix in effect" and is handled by [`apply`]).
///
/// This is the single chokepoint for the NS-25 constraint: every code path that
/// accepts a user-supplied prefix (meld `--as`, `[source].prefix`, config) must
/// call this before storing the value.
pub fn validate_prefix(prefix: &str) -> crate::error::Result<()> {
    if !prefix.is_empty() && crate::error::ItemKind::parse(prefix).is_some() {
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
pub fn expand(
    content: &str,
    prefix: &Option<String>,
    siblings: &HashSet<String>,
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
        out.push_str(&apply(name, prefix));
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
    /// A path token whose referent does not resolve (miss, ambiguous, or a tool
    /// with no entrypoint).
    Bad,
}

/// Expand the path tokens `{{self}}`, `{{tools:name}}`, and `{{path:ref}}` in
/// `content` to absolute store paths.
///
/// `{{ns:...}}` tokens are left untouched (handled by [`expand`]); any other
/// `{{...}}` span is passed through verbatim. Returns `Err(token)` with the
/// offending token text when a path token's referent does not resolve, so the
/// caller can report it. Whitespace inside a token is trimmed; an unterminated
/// token (no closing `}}`) leaves the remainder verbatim, mirroring [`expand`].
pub fn expand_paths(content: &str, ctx: &PathCtx) -> Result<String, String> {
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
            Token::Bad => {
                // Report the token exactly as written, including the braces.
                return Err(rest[pos..pos + 2 + end + 2].to_string());
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
                None => Token::Bad,
            },
            None => Token::Bad,
        };
    }
    if let Some(reference) = inner.strip_prefix("path:") {
        let reference = reference.trim();
        let (want_kind, name) = match reference.split_once(':') {
            Some((k, n)) => match crate::error::ItemKind::parse(k) {
                Some(kind) => (Some(kind), n.trim()),
                None => return Token::Bad,
            },
            None => (None, reference),
        };
        let mut hits = ctx
            .siblings
            .iter()
            .filter(|s| s.name == name && want_kind.is_none_or(|k| s.kind == k));
        return match (hits.next(), hits.next()) {
            (Some(s), None) => Token::Path(ctx.store_path(s.kind, name)),
            // No match, or ambiguous across kinds without a qualifier.
            _ => Token::Bad,
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
    } else if let Some(r) = path
        .strip_prefix("/home/")
        .or_else(|| path.strip_prefix("/Users/"))
    {
        // Drop the `<user>` segment of an absolute home path.
        r.split_once('/').map(|(_user, rest)| rest)?
    } else {
        return None;
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
    } else if let Some(rest) = path
        .strip_prefix("~/.claude/")
        .or_else(|| path.strip_prefix("~/.agents/"))
    {
        let mut it = rest.splitn(2, '/');
        let kind = crate::error::ItemKind::from_dir(it.next()?)?;
        (kind, it.next()?.to_string())
    } else {
        return None;
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

/// True when byte position `pos` in `line` is inside an inline code span (an odd
/// number of backticks precede it on the line).
fn in_code_span(line: &str, pos: usize) -> bool {
    line[..pos].bytes().filter(|&b| b == b'`').count() % 2 == 1
}

/// True when the token spanning `[start, end)` in `line` abuts a path separator.
fn path_adjacent(line: &str, start: usize, end: usize) -> bool {
    let before = line[..start].chars().next_back();
    let after = line[end..].chars().next();
    matches!(before, Some('/') | Some('~')) || matches!(after, Some('/'))
}

/// Find every `{{ns:name}}` token in `content`, each with its structural context
/// (NS-24) and byte span. Tracks fenced code blocks and the leading frontmatter
/// so a token can be classified as misplaced (in code, a path, or `name:`).
pub fn scan_ns_refs(content: &str) -> Vec<NsRef> {
    const OPEN: &str = "{{ns:";
    let mut out = Vec::new();
    let mut in_fence = false;
    let mut in_frontmatter = false;
    let mut offset = 0usize;
    for (idx, raw) in content.split_inclusive('\n').enumerate() {
        let line = raw.strip_suffix('\n').unwrap_or(raw);
        let trimmed = line.trim();
        if idx == 0 && trimmed == "---" {
            in_frontmatter = true;
            offset += raw.len();
            continue;
        }
        if in_frontmatter {
            if trimmed == "---" {
                in_frontmatter = false;
                offset += raw.len();
                continue;
            }
        } else if trimmed.starts_with("```") {
            in_fence = !in_fence;
            offset += raw.len();
            continue;
        }
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
            } else if in_fence {
                NsContext::CodeBlock
            } else if in_code_span(line, tstart) {
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
        let got = expand("hand off to {{ns:test}} now", &None, &s).unwrap();
        assert_eq!(got, "hand off to test now");
    }

    #[test]
    fn expand_prefixed_yields_prefixed_names() {
        // spec: NS-11
        let s = sibs(&["test"]);
        let got = expand("see {{ns:test}}.", &Some("jk".into()), &s).unwrap();
        assert_eq!(got, "see jk:test.");
    }

    #[test]
    fn expand_rejects_unknown_referent() {
        let s = sibs(&["test"]);
        assert_eq!(expand("{{ns:nope}}", &None, &s), Err("nope".to_string()));
    }

    #[test]
    fn expand_passes_content_without_tokens() {
        let s = sibs(&["test"]);
        assert_eq!(
            expand("no tokens here", &None, &s).unwrap(),
            "no tokens here"
        );
    }

    #[test]
    fn expand_trims_token_and_leaves_unterminated_verbatim() {
        // spec: NS-15
        let s = sibs(&["dev"]);
        // Whitespace inside the token is trimmed before the sibling lookup.
        assert_eq!(
            expand("{{ns:  dev  }}", &Some("jk".into()), &s).unwrap(),
            "jk:dev"
        );
        // An unterminated token (no closing `}}`) is passed through unchanged.
        assert_eq!(expand("see {{ns:dev", &None, &s).unwrap(), "see {{ns:dev");
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
        // spec: TOOL-12
        let store = Path::new("/m/store");
        let none = None;
        // No such sibling.
        let c = ctx(store, &none, ItemKind::Skill, "review", &[]);
        assert_eq!(
            expand_paths("{{tools:nope}}", &c),
            Err("{{tools:nope}}".to_string())
        );
        // A tool with no resolvable bin.
        let binless = vec![psib(ItemKind::Tool, "x", None)];
        let c = ctx(store, &none, ItemKind::Skill, "review", &binless);
        assert_eq!(
            expand_paths("{{tools:x}}", &c),
            Err("{{tools:x}}".to_string())
        );
        // A sibling of that name exists but is not a tool.
        let not_tool = vec![psib(ItemKind::Skill, "x", None)];
        let c = ctx(store, &none, ItemKind::Skill, "review", &not_tool);
        assert_eq!(
            expand_paths("{{tools:x}}", &c),
            Err("{{tools:x}}".to_string())
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
        // spec: TOOL-11
        let store = Path::new("/m/store");
        let none = None;
        // A skill and an agent share the bare name `x`.
        let sibs = vec![
            psib(ItemKind::Skill, "x", None),
            psib(ItemKind::Agent, "x", None),
        ];
        let c = ctx(store, &none, ItemKind::Skill, "self", &sibs);
        assert_eq!(
            expand_paths("{{path:x}}", &c),
            Err("{{path:x}}".to_string())
        );
        // A kind qualifier disambiguates.
        assert_eq!(
            expand_paths("{{path:agent:x}}", &c).unwrap(),
            "/m/store/agent/x"
        );
        // A miss is an error.
        assert_eq!(
            expand_paths("{{path:none}}", &c),
            Err("{{path:none}}".to_string())
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
