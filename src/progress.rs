//! Transient progress on stderr: a one-line spinner naming the step in flight
//! (spec/progress.md).
//!
//! A step is scoped narrowly around work that prints nothing itself (a git
//! network call, a parallel source refresh, an item's staging copy, the
//! catalog scan). Anything that does write to the terminal while a step may be
//! drawn (an advisory note, a prompt, a hook) goes through [`suspend`] first, so
//! it never lands on the end of a spinner line. The spinner is cleared when its
//! step ends and leaves nothing behind: stdout and the verb's own output are
//! byte for byte what they are with progress off.

use std::io::{IsTerminal, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

/// How long a step runs before its spinner appears in auto mode, so a fast
/// step never flickers (PRG-3).
const SHOW_DELAY: Duration = Duration::from_millis(200);

/// Redraw interval of the background ticker.
const TICK: Duration = Duration::from_millis(80);

const UNICODE_FRAMES: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
const ASCII_FRAMES: &[&str] = &["|", "/", "-", "\\"];

/// Return to column 0 and erase the line.
const CLEAR: &str = "\r\x1b[2K";

/// The `MIND_PROGRESS` setting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Setting {
    Auto,
    Always,
    Never,
}

/// Parse `MIND_PROGRESS`: `always` or `never` (any case), anything else
/// (unset, empty, unrecognized) is `auto`.
// spec: PRG-2
pub fn parse_setting(raw: Option<&str>) -> Setting {
    match raw.map(|v| v.trim().to_ascii_lowercase()).as_deref() {
        Some("always") => Setting::Always,
        Some("never") => Setting::Never,
        _ => Setting::Auto,
    }
}

/// Whether a `CI` value marks a CI run: set, non-empty, and not `0`/`false`.
fn is_ci(raw: Option<&str>) -> bool {
    raw.map(str::trim)
        .is_some_and(|v| !v.is_empty() && v != "0" && !v.eq_ignore_ascii_case("false"))
}

/// Everything enablement depends on, injected so the gate is unit-testable.
#[derive(Debug, Clone, Copy)]
pub struct Inputs {
    pub setting: Setting,
    pub no_progress: bool,
    pub json: bool,
    pub stderr_tty: bool,
    pub term_dumb: bool,
    pub ci: bool,
}

/// The show delay progress runs with, or `None` when progress is off.
///
/// `--json`, `--no-progress`, and `MIND_PROGRESS=never` turn it off.
/// `MIND_PROGRESS=always` turns it on with no delay. Otherwise it is on when
/// stderr is a terminal, `TERM` is not `dumb`, and `CI` is unset.
// spec: PRG-2 PRG-3
pub fn decide(i: &Inputs) -> Option<Duration> {
    if i.json || i.no_progress {
        return None;
    }
    match i.setting {
        Setting::Never => None,
        Setting::Always => Some(Duration::ZERO),
        Setting::Auto => (i.stderr_tty && !i.term_dumb && !i.ci).then_some(SHOW_DELAY),
    }
}

/// Spinner configuration.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Config {
    pub delay: Duration,
    pub unicode: bool,
    pub color: bool,
    /// A fixed line width; `None` asks the terminal on each draw.
    pub width: Option<usize>,
}

struct Inner {
    /// Live steps, oldest first; the newest one's label is drawn.
    steps: Vec<(u64, String)>,
    next_id: u64,
    /// When the outermost live step began.
    since: Option<Instant>,
    /// Whether a spinner line is on screen now.
    drawn: bool,
    /// [`Progress::suspend`] nesting depth.
    held: usize,
    frame: usize,
    out: Box<dyn Write + Send>,
}

/// A spinner drawing to one writer. The process has one, on stderr
/// ([`init`]); tests build their own over a buffer.
pub(crate) struct Progress {
    cfg: Config,
    hidden: AtomicBool,
    /// [`Inner::drawn`], readable without the lock (the SIGINT handler).
    on_screen: AtomicBool,
    inner: Mutex<Inner>,
}

impl Progress {
    pub(crate) fn new(cfg: Config, out: Box<dyn Write + Send>) -> Self {
        Self {
            cfg,
            hidden: AtomicBool::new(false),
            on_screen: AtomicBool::new(false),
            inner: Mutex::new(Inner {
                steps: Vec::new(),
                next_id: 0,
                since: None,
                drawn: false,
                held: 0,
                frame: 0,
                out,
            }),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Start a step labeled `label`; it ends when the returned guard drops.
    // spec: PRG-7
    pub(crate) fn step(&self, label: &str) -> Step<'_> {
        let label = crate::sanitize::strip_ansi(label);
        let mut g = self.lock();
        let id = g.next_id;
        g.next_id += 1;
        g.steps.push((id, label));
        if g.since.is_none() {
            g.since = Some(Instant::now());
        }
        self.draw_if_ready(&mut g);
        Step {
            owner: Some(self),
            id,
        }
    }

    /// Whether any step is live.
    fn busy(&self) -> bool {
        !self.lock().steps.is_empty()
    }

    fn relabel(&self, id: u64, label: &str) {
        let label = crate::sanitize::strip_ansi(label);
        let mut g = self.lock();
        if let Some(slot) = g.steps.iter_mut().find(|(i, _)| *i == id) {
            slot.1 = label;
        }
        if g.drawn || self.cfg.delay.is_zero() {
            self.draw_if_ready(&mut g);
        }
    }

    fn end(&self, id: u64) {
        let mut g = self.lock();
        g.steps.retain(|(i, _)| *i != id);
        if g.steps.is_empty() {
            g.since = None;
            self.clear(&mut g);
        } else if g.drawn {
            self.draw_if_ready(&mut g);
        }
    }

    /// Run `f` with the spinner off the screen, then let it come back.
    // spec: PRG-5
    pub(crate) fn suspend<R>(&self, f: impl FnOnce() -> R) -> R {
        struct Resume<'a>(&'a Progress);
        impl Drop for Resume<'_> {
            fn drop(&mut self) {
                let mut g = self.0.lock();
                g.held -= 1;
                if self.0.cfg.delay.is_zero() {
                    self.0.draw_if_ready(&mut g);
                }
            }
        }
        {
            let mut g = self.lock();
            g.held += 1;
            self.clear(&mut g);
        }
        let _resume = Resume(self);
        f()
    }

    /// Hide (or show again) the spinner, for as long as something else owns
    /// the terminal.
    // spec: PRG-9
    pub(crate) fn set_hidden(&self, hidden: bool) {
        self.hidden.store(hidden, Ordering::Relaxed);
        if hidden {
            self.clear(&mut self.lock());
        }
    }

    fn is_hidden(&self) -> bool {
        self.hidden.load(Ordering::Relaxed)
    }

    /// Whether a spinner may draw now: not hidden and not suspended. A git
    /// call made when it may not (under [`Progress::suspend`]) can prompt
    /// directly (PRG-6).
    // spec: PRG-6
    fn may_draw(&self) -> bool {
        !self.is_hidden() && self.lock().held == 0
    }

    /// Advance the animation one frame and redraw, if a step is showing.
    pub(crate) fn tick(&self) {
        let mut g = self.lock();
        g.frame = g.frame.wrapping_add(1);
        self.draw_if_ready(&mut g);
    }

    fn draw_if_ready(&self, g: &mut Inner) {
        let Some((_, label)) = g.steps.last() else {
            return;
        };
        let due = g.since.is_some_and(|t| t.elapsed() >= self.cfg.delay);
        if !due || g.held > 0 || self.is_hidden() {
            return;
        }
        // One write, so the SIGINT handler's clear (PRG-10) cannot land
        // between the clear and the line.
        let line = format!("{CLEAR}{}", self.compose(label, g.frame));
        let _ = g.out.write_all(line.as_bytes());
        let _ = g.out.flush();
        g.drawn = true;
        self.on_screen.store(true, Ordering::SeqCst);
    }

    fn clear(&self, g: &mut Inner) {
        if g.drawn {
            let _ = write!(g.out, "{CLEAR}");
            let _ = g.out.flush();
            g.drawn = false;
            self.on_screen.store(false, Ordering::SeqCst);
        }
    }

    /// `<frame> <label>`, the label cut to fit the line. One column is left
    /// free so the line never reaches the last column and wraps.
    // spec: PRG-7 PRG-8
    fn compose(&self, label: &str, frame: usize) -> String {
        let frames = if self.cfg.unicode {
            UNICODE_FRAMES
        } else {
            ASCII_FRAMES
        };
        let glyph = frames[frame % frames.len()];
        let width = self.cfg.width.unwrap_or_else(stderr_width);
        let label = truncate_to_width(label, width.saturating_sub(3));
        if self.cfg.color {
            format!("\x1b[36m{glyph}\x1b[0m {label}")
        } else {
            format!("{glyph} {label}")
        }
    }
}

/// A live step. Dropping it ends the step; the spinner clears when the last
/// live step ends.
pub struct Step<'a> {
    owner: Option<&'a Progress>,
    id: u64,
}

impl Step<'_> {
    /// A step that draws nothing (progress off, or another step already live).
    fn none() -> Step<'static> {
        Step { owner: None, id: 0 }
    }

    /// Replace this step's label (a count that moved, the next item).
    pub fn set(&self, label: impl AsRef<str>) {
        if let Some(p) = self.owner {
            p.relabel(self.id, label.as_ref());
        }
    }
}

impl Drop for Step<'_> {
    fn drop(&mut self) {
        if let Some(p) = self.owner {
            p.end(self.id);
        }
    }
}

/// Cut `s` to at most `max` display columns, marking a cut with `...`.
fn truncate_to_width(s: &str, max: usize) -> String {
    use unicode_width::UnicodeWidthChar;
    if crate::render::visible_width(s) <= max {
        return s.to_string();
    }
    let room = max.saturating_sub(3);
    let mut out = String::new();
    let mut used = 0;
    for c in s.chars() {
        let w = c.width().unwrap_or(0);
        if used + w > room {
            break;
        }
        out.push(c);
        used += w;
    }
    out.push_str(&"..."[..max.min(3)]);
    out
}

/// The width the spinner line fits in: `COLUMNS` when set to a number, else
/// the terminal's, else 80.
fn stderr_width() -> usize {
    std::env::var("COLUMNS")
        .ok()
        .and_then(|v| v.trim().parse::<usize>().ok())
        .or_else(|| {
            crossterm::terminal::size()
                .ok()
                .map(|(c, _)| usize::from(c))
        })
        .unwrap_or(80)
        .max(20)
}

static GLOBAL: OnceLock<Progress> = OnceLock::new();

/// Turn progress on for this process when the flags and environment allow it
/// (PRG-2), and start the redraw ticker. Call once, early in `main`.
pub fn init(json: bool, no_progress: bool, ascii: bool) {
    let stderr_tty = std::io::stderr().is_terminal();
    let inputs = Inputs {
        setting: parse_setting(std::env::var("MIND_PROGRESS").ok().as_deref()),
        no_progress,
        json,
        stderr_tty,
        term_dumb: std::env::var("TERM").is_ok_and(|t| t == "dumb"),
        ci: is_ci(std::env::var("CI").ok().as_deref()),
    };
    let Some(delay) = decide(&inputs) else {
        return;
    };
    let cfg = Config {
        delay,
        unicode: !ascii && crate::render::detect_utf8_locale(),
        color: !ascii && stderr_tty && std::env::var_os("NO_COLOR").is_none(),
        // Off a terminal (`always`) the size lookup can fall back to running
        // `tput`; resolve it once rather than on every tick.
        width: (!stderr_tty).then(stderr_width),
    };
    if GLOBAL
        .set(Progress::new(cfg, Box::new(std::io::stderr())))
        .is_ok()
    {
        clear_on_interrupt();
        std::thread::spawn(|| {
            loop {
                std::thread::sleep(TICK);
                if let Some(p) = GLOBAL.get() {
                    p.tick();
                }
            }
        });
    }
}

/// Erase a drawn spinner line on Ctrl-C, then die of the signal as the
/// default disposition would (PRG-10). Not installed when SIGINT is ignored
/// (a background job), so that ignore is kept. A hook's own SIGINT handling
/// (HOOK-171) saves and restores this one around the hook.
// spec: PRG-10
#[cfg(unix)]
fn clear_on_interrupt() {
    extern "C" fn on_sigint(_: libc::c_int) {
        // `OnceLock::get` is an atomic load, and `write`, `signal`, and
        // `raise` are async-signal-safe. The raised SIGINT is blocked until
        // this handler returns, then delivered with the default action.
        if let Some(p) = GLOBAL.get()
            && p.on_screen.load(Ordering::SeqCst)
        {
            // SAFETY: writes a static byte string to the stderr fd.
            unsafe {
                libc::write(libc::STDERR_FILENO, CLEAR.as_ptr().cast(), CLEAR.len());
            }
        }
        // SAFETY: resets SIGINT to its default and re-raises it.
        unsafe {
            libc::signal(libc::SIGINT, libc::SIG_DFL);
            libc::raise(libc::SIGINT);
        }
    }
    // SAFETY: plain sigaction calls on zeroed structs.
    unsafe {
        let mut old: libc::sigaction = std::mem::zeroed();
        if libc::sigaction(libc::SIGINT, std::ptr::null(), &mut old) != 0
            || old.sa_sigaction == libc::SIG_IGN
        {
            return;
        }
        let mut new: libc::sigaction = std::mem::zeroed();
        new.sa_sigaction = on_sigint as extern "C" fn(libc::c_int) as libc::sighandler_t;
        libc::sigemptyset(&mut new.sa_mask);
        new.sa_flags = libc::SA_RESTART;
        libc::sigaction(libc::SIGINT, &new, std::ptr::null_mut());
    }
}

#[cfg(not(unix))]
fn clear_on_interrupt() {}

/// Whether a spinner may draw in this process now: progress is on, not
/// hidden, and not suspended.
pub fn enabled() -> bool {
    GLOBAL.get().is_some_and(Progress::may_draw)
}

/// Start a step labeled `label` (sanitized, PRG-7). A no-op when progress is
/// off.
pub fn step(label: impl AsRef<str>) -> Step<'static> {
    match GLOBAL.get() {
        Some(p) => p.step(label.as_ref()),
        None => Step::none(),
    }
}

/// Start a step only when none is live, so an inner operation (one git call)
/// does not replace the label of the step it runs under (a counted refresh).
/// `label` is only built when the step starts.
pub fn step_if_idle(label: impl FnOnce() -> String) -> Step<'static> {
    match GLOBAL.get() {
        Some(p) if !p.busy() => p.step(&label()),
        _ => Step::none(),
    }
}

/// Run `f`, which writes to the terminal, with the spinner cleared first
/// (PRG-5). Just `f()` when progress is off.
pub fn suspend<R>(f: impl FnOnce() -> R) -> R {
    match GLOBAL.get() {
        Some(p) => p.suspend(f),
        None => f(),
    }
}

/// Hide the spinner while the TUI owns the terminal (PRG-9).
pub fn set_hidden(hidden: bool) {
    if let Some(p) = GLOBAL.get() {
        p.set_hidden(hidden);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    /// A writer the test can read back.
    #[derive(Clone, Default)]
    struct Buf(Arc<Mutex<Vec<u8>>>);

    impl Write for Buf {
        fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(b);
            Ok(b.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl Buf {
        fn text(&self) -> String {
            String::from_utf8(self.0.lock().unwrap().clone()).unwrap()
        }
    }

    fn progress(delay: Duration, unicode: bool, width: usize) -> (Progress, Buf) {
        let buf = Buf::default();
        let cfg = Config {
            delay,
            unicode,
            color: false,
            width: Some(width),
        };
        (Progress::new(cfg, Box::new(buf.clone())), buf)
    }

    fn inputs() -> Inputs {
        Inputs {
            setting: Setting::Auto,
            no_progress: false,
            json: false,
            stderr_tty: true,
            term_dumb: false,
            ci: false,
        }
    }

    // spec: PRG-2
    #[test]
    fn parse_setting_reads_always_and_never_else_auto() {
        assert_eq!(parse_setting(Some("always")), Setting::Always);
        assert_eq!(parse_setting(Some(" NEVER ")), Setting::Never);
        assert_eq!(parse_setting(Some("")), Setting::Auto);
        assert_eq!(parse_setting(Some("sometimes")), Setting::Auto);
        assert_eq!(parse_setting(None), Setting::Auto);
    }

    // spec: PRG-2
    #[test]
    fn auto_needs_a_terminal_and_no_dumb_term_or_ci() {
        assert_eq!(decide(&inputs()), Some(SHOW_DELAY));
        for off in [
            Inputs {
                stderr_tty: false,
                ..inputs()
            },
            Inputs {
                term_dumb: true,
                ..inputs()
            },
            Inputs {
                ci: true,
                ..inputs()
            },
        ] {
            assert_eq!(decide(&off), None, "{off:?}");
        }
    }

    // spec: PRG-2
    #[test]
    fn json_flag_and_never_turn_it_off_even_over_always() {
        for off in [
            Inputs {
                json: true,
                setting: Setting::Always,
                ..inputs()
            },
            Inputs {
                no_progress: true,
                setting: Setting::Always,
                ..inputs()
            },
            Inputs {
                setting: Setting::Never,
                ..inputs()
            },
        ] {
            assert_eq!(decide(&off), None, "{off:?}");
        }
    }

    // spec: PRG-2 PRG-3
    #[test]
    fn always_draws_with_no_delay_off_a_terminal_and_in_ci() {
        let i = Inputs {
            setting: Setting::Always,
            stderr_tty: false,
            term_dumb: true,
            ci: true,
            ..inputs()
        };
        assert_eq!(decide(&i), Some(Duration::ZERO));
    }

    #[test]
    fn ci_value_reading() {
        assert!(is_ci(Some("true")));
        assert!(is_ci(Some("1")));
        assert!(!is_ci(Some("")));
        assert!(!is_ci(Some("0")));
        assert!(!is_ci(Some("false")));
        assert!(!is_ci(None));
    }

    // spec: PRG-1
    #[test]
    fn a_step_draws_its_label_and_clears_when_it_ends() {
        let (p, buf) = progress(Duration::ZERO, false, 80);
        {
            let _s = p.step("cloning repo");
        }
        assert_eq!(buf.text(), format!("{CLEAR}| cloning repo{CLEAR}"));
    }

    // spec: PRG-3
    #[test]
    fn auto_delay_keeps_a_fast_step_off_the_screen() {
        let (p, buf) = progress(Duration::from_secs(60), false, 80);
        {
            let _s = p.step("fast");
            p.tick();
        }
        assert_eq!(
            buf.text(),
            "",
            "a step that ends before the delay draws nothing"
        );
    }

    // spec: PRG-1
    #[test]
    fn nested_step_shows_innermost_then_restores_outer() {
        let (p, buf) = progress(Duration::ZERO, false, 80);
        let outer = p.step("outer");
        {
            let _inner = p.step("inner");
        }
        drop(outer);
        let t = buf.text();
        let inner = t.find("inner").unwrap();
        let back = t.rfind("outer").unwrap();
        assert!(back > inner, "outer label redrawn after inner ends: {t:?}");
        assert!(t.ends_with(CLEAR), "{t:?}");
    }

    // spec: PRG-1
    #[test]
    fn set_relabels_the_live_step() {
        let (p, buf) = progress(Duration::ZERO, false, 80);
        let s = p.step("syncing sources (0/2)");
        s.set("syncing sources (1/2)");
        drop(s);
        assert!(
            buf.text().contains("| syncing sources (1/2)"),
            "{}",
            buf.text()
        );
    }

    // spec: PRG-5
    #[test]
    fn suspend_clears_before_the_closure_and_redraws_after() {
        let (p, buf) = progress(Duration::ZERO, false, 80);
        let _s = p.step("working");
        let seen = p.suspend(|| buf.text());
        assert!(
            seen.ends_with(CLEAR),
            "the line is cleared before the closure writes: {seen:?}"
        );
        assert!(buf.text().ends_with("| working"), "{:?}", buf.text());
    }

    // spec: PRG-5
    #[test]
    fn nothing_draws_while_suspended() {
        let (p, buf) = progress(Duration::ZERO, false, 80);
        p.suspend(|| {
            let _s = p.step("inside");
            p.tick();
        });
        assert_eq!(buf.text(), "");
    }

    // spec: PRG-6
    #[test]
    fn may_not_draw_while_suspended_or_hidden() {
        let (p, _buf) = progress(Duration::ZERO, false, 80);
        assert!(p.may_draw());
        assert!(!p.suspend(|| p.may_draw()), "suspended: git may prompt");
        assert!(p.may_draw());
        p.set_hidden(true);
        assert!(!p.may_draw());
    }

    // spec: PRG-10
    #[test]
    fn on_screen_follows_the_drawn_line() {
        let (p, _buf) = progress(Duration::ZERO, false, 80);
        let s = p.step("x");
        assert!(p.on_screen.load(Ordering::SeqCst));
        p.suspend(|| assert!(!p.on_screen.load(Ordering::SeqCst)));
        assert!(p.on_screen.load(Ordering::SeqCst), "redrawn after suspend");
        drop(s);
        assert!(!p.on_screen.load(Ordering::SeqCst));
    }

    // spec: PRG-9
    #[test]
    fn hidden_clears_and_blocks_drawing_until_shown() {
        let (p, buf) = progress(Duration::ZERO, false, 80);
        let _s = p.step("working");
        p.set_hidden(true);
        let at_hide = buf.text();
        assert!(at_hide.ends_with(CLEAR), "{at_hide:?}");
        p.tick();
        assert_eq!(buf.text(), at_hide, "no draw while hidden");
        p.set_hidden(false);
        p.tick();
        assert!(buf.text().ends_with("working"), "{:?}", buf.text());
    }

    // spec: PRG-7
    #[test]
    fn label_is_sanitized_and_cut_to_the_width() {
        let (p, buf) = progress(Duration::ZERO, false, 20);
        drop(p.step("a\x1b[31mbcdefghijklmnopqrstuvwxyz\nnext"));
        let t = buf.text();
        let drawn = t.trim_start_matches(CLEAR).trim_end_matches(CLEAR);
        assert!(!drawn.contains('\x1b') && !drawn.contains('\n'), "{t:?}");
        assert_eq!(drawn, "| abcdefghijklmn...");
        assert!(crate::render::visible_width(drawn) < 20);
    }

    // spec: PRG-8
    #[test]
    fn frames_follow_the_unicode_capability() {
        let (p, buf) = progress(Duration::ZERO, true, 80);
        drop(p.step("x"));
        assert!(buf.text().contains("⠋ x"), "{:?}", buf.text());
        let (p, buf) = progress(Duration::ZERO, false, 80);
        let s = p.step("x");
        p.tick();
        drop(s);
        assert!(buf.text().contains("/ x"), "ticks advance the ASCII frame");
    }

    #[test]
    fn truncate_handles_wide_chars_and_tiny_widths() {
        assert_eq!(truncate_to_width("abc", 3), "abc");
        assert_eq!(truncate_to_width("日本語テキスト", 7), "日本...");
        assert_eq!(truncate_to_width("abcdef", 2), "..");
    }
}
