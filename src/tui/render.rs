//! Ratatui draw functions for the TUI.
//!
//! Pure render of `&App`; no domain-state mutation, no lock (the one exception is
//! the `App::scroll` offset cache, updated here because the viewport height is
//! known only at draw time, TUI-16). The TUI is free to use Unicode (box drawing,
//! geometric markers) for presentation; the ASCII-only rule is for written prose,
//! not the interface.
//!
//! Rendering is responsive (TUI-42): long text wraps to the terminal width and
//! overlays are clamped to the terminal size, so nothing is cut off on a narrow
//! terminal.

use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, List, ListItem, ListState, Paragraph, Wrap};

use crate::error::Result;
use crate::tui::app::{App, FlatNode};
use crate::tui::tree::TreeNode;

/// The bottom key-hint line. Uses a middot separator and wraps on narrow
/// terminals (TUI-42). Covers navigation (including search and expand/collapse
/// and paging, TUI-64), the mutating actions, and the `?` full keymap overlay.
// spec: TUI-64
const HINTS: &str = " j/k move \u{b7} h/l collapse/expand \u{b7} PgUp/PgDn page \u{b7} / search \u{b7} Enter details \u{b7} Space expand \u{b7} i install \u{b7} d delete \u{b7} s sync \u{b7} u upgrade \u{b7} m meld \u{b7} M unmeld \u{b7} C lobes \u{b7} ? help \u{b7} q quit";

/// The full keymap listing shown by the `?` help overlay (TUI-64). Grouped by
/// category so a user can find a binding without reading `HINTS` (which is
/// necessarily abbreviated to fit one status-bar row).
// spec: TUI-64
const HELP_TEXT: &str = "Navigation\n\
  j/k, Up/Down    move selection\n\
  h/l, Left/Right collapse / expand\n\
  Space           toggle expand\n\
  PgUp/PgDn, Ctrl-u/Ctrl-d page up/down\n\
  /               jump to search (Esc clears, Enter/Tab submits)\n\
  Enter           open details dialog\n\
\n\
Actions\n\
  i               install selected item\n\
  d               uninstall (forget) selected item\n\
  s               sync all sources\n\
  u               upgrade pending items\n\
  m               meld a source\n\
  M               unmeld selected source\n\
  C               agent homes (lobes)\n\
\n\
General\n\
  y / n           confirm / cancel a pending action\n\
  Esc             cancel / close the active overlay\n\
  ?               toggle this help\n\
  q, Ctrl-C x2    quit";

/// Estimate how many terminal rows `text` occupies when wrapped at `width`
/// columns (greedy word wrap, hard-splitting words longer than the width).
/// Used to size the hint line and the input modals so wrapped content is never
/// clipped (TUI-42). At least one row.
fn wrapped_rows(text: &str, width: u16) -> u16 {
    let w = width.max(1) as usize;
    let mut rows: u16 = 0;
    for para in text.split('\n') {
        rows = rows.saturating_add(line_rows(para, w));
    }
    rows.max(1)
}

/// Rows one `\n`-free segment needs at `w` columns. An empty segment is one row.
/// Word width is measured with `visible_width` (display columns via
/// `unicode-width`, ANSI-blind), not a raw char count, so a CJK/emoji word does
/// not under-count and wrap later than it actually would on screen (TUI-67).
// spec: TUI-67
fn line_rows(line: &str, w: usize) -> u16 {
    let mut rows: u16 = 1;
    let mut col: usize = 0;
    let place = |word_len: usize, rows: &mut u16, col: &mut usize| {
        // Place a word at the current column, hard-splitting if it is wider than
        // the whole line.
        if word_len <= w {
            *col += word_len;
        } else {
            let extra = (word_len - 1) / w;
            *rows = rows.saturating_add(extra as u16);
            *col = word_len - extra * w;
        }
    };
    for word in line.split(' ') {
        let wl = crate::render::visible_width(word);
        if col == 0 {
            place(wl, &mut rows, &mut col);
        } else if col + 1 + wl <= w {
            col += 1 + wl; // a space then the word
        } else {
            rows = rows.saturating_add(1);
            col = 0;
            place(wl, &mut rows, &mut col);
        }
    }
    rows
}

/// Rows the status/error line gets (TUI-70). Previously clamped to a flat
/// maximum of 3 rows regardless of terminal size, so a long error (a chained
/// `MindError` can run several sentences) was always cut off at 3 lines with
/// no way to read the rest -- the status Paragraph wraps but the layout slot
/// never grew past 3. Instead the cap scales with the terminal height, so a
/// taller terminal shows more of a long message; a fixed reserve (search bar,
/// a minimum tree row, and a minimum hint row) is always held back so a very
/// long message cannot push the tree pane to zero height on a short terminal.
// spec: TUI-70
fn status_height(status_text: &str, width: u16, term_height: u16) -> u16 {
    if status_text.is_empty() {
        return 1;
    }
    const RESERVED: u16 = 3 + 1 + 1; // search bar + min tree row + min hint row
    let cap = term_height.saturating_sub(RESERVED).max(3);
    wrapped_rows(status_text, width).clamp(1, cap)
}

/// Clamp a modal width to the terminal: at least `min` (for readability) but
/// never wider than what is available, so a small terminal does not push the
/// overlay off screen (TUI-42).
fn modal_width(desired: u16, min: u16, avail: u16) -> u16 {
    desired.max(min).min(avail.max(1))
}

/// First-visible-row offset that keeps the `selected` row within the middle
/// two-thirds of a `rows`-high viewport (TUI-16). The offset moves only enough to
/// hold a ~1/6 margin above and below the selection, so the highlight does not
/// reach the top or bottom edge while there are more rows to scroll. `prev` is the
/// previous offset, kept stable while the selection stays inside the band. Near
/// the list ends the highlight may sit at the edge (nothing more to scroll); when
/// everything fits, the offset is 0.
fn scroll_offset(prev: usize, selected: usize, len: usize, rows: usize) -> usize {
    if rows == 0 || len <= rows {
        return 0;
    }
    let max_off = len - rows;
    let margin = rows / 6;
    // The selection must stay within [off + margin, off + rows - 1 - margin].
    let lo = (selected + margin + 1).saturating_sub(rows); // off >= this
    let hi = selected.saturating_sub(margin).min(max_off); // off <= this
    let lo = lo.min(hi); // tiny-viewport safety: never invert the bounds
    prev.clamp(lo, hi)
}

/// Draw the full TUI to the given frame.
pub fn draw(app: &App) -> Result<()> {
    // Get the terminal and draw.
    let mut terminal = crate::tui::term::get_terminal();
    terminal
        .draw(|frame| draw_frame(frame, app))
        .map_err(|e| crate::error::MindError::io("<terminal>", e))?;
    Ok(())
}

fn draw_frame(frame: &mut Frame, app: &App) {
    let size = frame.area();

    // The status line and the hint line grow to as many rows as their text needs
    // at this width (bounded), so neither is truncated on a narrow terminal.
    let status_text = status_text(app);
    let status_h = status_height(&status_text, size.width, size.height);
    let hint_h = wrapped_rows(HINTS, size.width).clamp(1, 3);

    // Layout: search bar at top, main tree in middle, status + hints at bottom.
    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),        // search bar
            Constraint::Min(1),           // tree
            Constraint::Length(status_h), // status line(s)
            Constraint::Length(hint_h),   // key hint line(s)
        ])
        .split(size);

    draw_search_bar(frame, app, layout[0]);
    draw_tree(frame, app, layout[1]);
    draw_status(frame, &status_text, app.error.is_some(), layout[2]);
    draw_hints(frame, layout[3]);

    // If a modal is visible, overlay it.
    if app.modal_visible {
        draw_modal(frame, app, size);
    }

    // If spec-input is active, overlay the spec-input box (TUI-30).
    if app.spec_input_active {
        draw_spec_input(frame, app, size);
    }

    // If the lobes modal is open, overlay it (TUI-23).
    if app.lobes_modal_visible {
        draw_lobes_modal(frame, app, size);
    }

    // If the lobe-path input is active, overlay it (TUI-23).
    if app.lobe_input_active {
        draw_lobe_input(frame, app, size);
    }

    // If the details dialog is open, overlay it (TUI-26).
    if let Some(dialog) = &app.dialog {
        draw_dialog(frame, dialog, size);
    }

    // If namespace-input is active, overlay it (TUI-53). Drawn last so it
    // appears on top of any open dialog (activate_dialog closes the dialog
    // before opening input, so in practice they don't both appear at once).
    // spec: TUI-53
    if app.namespace_input_active {
        draw_namespace_input(frame, app, size);
    }

    // The `?` help overlay is drawn last so it sits on top of everything else
    // (TUI-64); mod.rs's handle_key intercepts every key while it is open so
    // no other overlay is reachable at the same time in practice, but drawing
    // it last keeps the invariant even if that ever changes.
    // spec: TUI-64
    if app.help_visible {
        draw_help(frame, size);
    }
}

/// Draw the `?` keymap help overlay (TUI-64): every key binding grouped by
/// category, centered and clamped to the terminal (TUI-42). Any key closes it
/// (handled in mod.rs's handle_key, not here).
// spec: TUI-64
fn draw_help(frame: &mut Frame, area: Rect) {
    let w = modal_width(60, 40, area.width);
    let inner_w = w.saturating_sub(2).max(1);
    let content_h = wrapped_rows(HELP_TEXT, inner_w);
    let h = content_h
        .saturating_add(2)
        .clamp(5.min(area.height.max(1)), area.height.max(1));
    let inner = overlay(frame, area, "Keymap Help (any key closes)", w, h);
    let widget = Paragraph::new(HELP_TEXT).wrap(Wrap { trim: false });
    frame.render_widget(widget, inner);
}

/// A rounded-border block with a title, the common frame for panes and modals.
fn titled_block(title: &str) -> Block<'_> {
    Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
}

/// Center a `w x h` overlay inside `area` (clamping to fit), render a
/// `Clear` to erase what was under it, render a titled border block -- yellow
/// when color is on, unstyled under NO_COLOR (TUI-65) -- and return the inner
/// `Rect` (the usable content area inside the border). Used by every modal
/// draw function so the centering and Clear logic live in one place (TUI-42).
fn overlay(frame: &mut Frame, area: Rect, title: &str, w: u16, h: u16) -> Rect {
    let w = w.min(area.width.max(1));
    let h = h.min(area.height.max(1));
    let x = (area.width.saturating_sub(w)) / 2;
    let y = (area.height.saturating_sub(h)) / 2;
    let modal_area = Rect::new(x, y, w, h);
    let border_style = if crate::render::ctx().color {
        ratatui::style::Style::default().fg(Color::Yellow)
    } else {
        ratatui::style::Style::default()
    };
    let block = titled_block(title).style(border_style);
    let inner = block.inner(modal_area);
    frame.render_widget(ratatui::widgets::Clear, modal_area);
    frame.render_widget(block, modal_area);
    inner
}

fn draw_search_bar(frame: &mut Frame, app: &App, area: Rect) {
    let title = if app.search_focused {
        "Search (ESC to clear)"
    } else {
        "Search (/) to focus"
    };
    // spec: TUI-65 - no color under NO_COLOR; BOLD still marks focus.
    let style = if app.search_focused {
        if crate::render::ctx().color {
            Style::default().fg(Color::Yellow)
        } else {
            Style::default().add_modifier(Modifier::BOLD)
        }
    } else {
        Style::default()
    };
    let text = Paragraph::new(app.search.as_str())
        .block(titled_block(title))
        .style(style);
    frame.render_widget(text, area);
}

fn draw_tree(frame: &mut Frame, app: &App, area: Rect) {
    // spec: TUI-65 - read the process-wide output context once per draw so
    // color/Unicode capability (NO_COLOR, non-UTF-8 locale, --ascii) is honored
    // consistently across every row.
    let rc = crate::render::ctx();
    let items: Vec<ListItem> = app
        .visible
        .iter()
        .map(|node| flat_node_to_list_item(node, rc))
        .collect();

    // Keep the highlighted row within the middle two-thirds of the visible area
    // (TUI-16): compute the first-visible-row offset from the cached previous
    // offset and the real viewport height (area minus the two border rows), then
    // store it back so the scroll position is stable as the selection moves within
    // the band.
    let rows = area.height.saturating_sub(2) as usize;
    let offset = scroll_offset(app.scroll.get(), app.selected, app.visible.len(), rows);
    app.scroll.set(offset);

    let mut state = ListState::default();
    state.select(Some(app.selected));
    *state.offset_mut() = offset;

    // spec: TUI-65 - selection uses REVERSED (a video attribute, not a color)
    // under NO_COLOR so the highlight stays visible without emitting color, and
    // an ASCII arrow instead of the Unicode highlight symbol under a non-UTF-8
    // locale.
    let highlight_style = if rc.color {
        Style::default()
            .bg(Color::DarkGray)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().add_modifier(Modifier::REVERSED)
    };
    let highlight_symbol = if rc.unicode { "\u{276f} " } else { "> " };

    let list = List::new(items)
        .block(titled_block("Items"))
        .highlight_style(highlight_style)
        .highlight_symbol(highlight_symbol);

    frame.render_stateful_widget(list, area, &mut state);
}

/// Disclosure triangle for an expandable row (Unicode) or its ASCII fallback
/// (TUI-65, M12): a non-UTF-8 locale must not draw a triangle glyph that would
/// come out as mojibake. Two spaces keep non-expandable leaves aligned.
fn expand_marker(expandable: bool, expanded: bool, unicode: bool) -> &'static str {
    if !expandable {
        return "  ";
    }
    match (expanded, unicode) {
        (true, true) => "\u{25be} ",  // down-pointing triangle
        (false, true) => "\u{25b8} ", // right-pointing triangle
        (true, false) => "v ",
        (false, false) => "> ",
    }
}

/// A marker per node kind: filled = present/installed, hollow = available;
/// group headers carry no marker (the bold label leads). ASCII fallback under
/// a non-UTF-8 locale (TUI-65, M12).
fn node_icon(node: &TreeNode, unicode: bool) -> &'static str {
    if unicode {
        match node {
            TreeNode::InstalledGroup | TreeNode::AvailableGroup | TreeNode::UnmanagedGroup => "",
            TreeNode::Source(_) => "\u{25c6} ", // filled diamond
            TreeNode::KindBucket { .. } => "\u{25aa} ", // small square
            TreeNode::InstalledItem(_) => "\u{25cf} ", // filled circle
            TreeNode::AvailableItem(_) => "\u{25cb} ", // hollow circle
            TreeNode::UnmanagedItem(_) => "\u{25cb} ", // hollow circle (not mind-managed)
            TreeNode::SuggestedSource(_) => "\u{25c7} ", // hollow diamond
            // spec: TUI-50 - dependency child nodes under an expanded item.
            TreeNode::DepChild(dep) if dep.is_cycle => "\u{21ba} ", // cycle arrow
            TreeNode::DepChild(_) => "\u{21b3} ",                   // dep arrow
            // spec: TUI-68 - a call-to-action row carries no marker.
            TreeNode::EmptyState(_) => "",
        }
    } else {
        match node {
            TreeNode::InstalledGroup | TreeNode::AvailableGroup | TreeNode::UnmanagedGroup => "",
            TreeNode::Source(_) => "* ",
            TreeNode::KindBucket { .. } => "- ",
            TreeNode::InstalledItem(_) => "+ ",
            TreeNode::AvailableItem(_) => "o ",
            TreeNode::UnmanagedItem(_) => "o ",
            TreeNode::SuggestedSource(_) => "? ",
            TreeNode::DepChild(dep) if dep.is_cycle => "^ ",
            TreeNode::DepChild(_) => "> ",
            TreeNode::EmptyState(_) => "",
        }
    }
}

/// Row style per node kind. Under NO_COLOR (`color == false`) every `fg` is
/// dropped -- only BOLD/DIM survive, since those are video attributes a
/// monochrome terminal still renders, not color (TUI-65, M12).
fn node_style(node: &TreeNode, color: bool) -> Style {
    match node {
        TreeNode::InstalledGroup | TreeNode::AvailableGroup | TreeNode::UnmanagedGroup => {
            Style::default().add_modifier(Modifier::BOLD)
        }
        TreeNode::InstalledItem(_) => {
            if color {
                Style::default().fg(Color::Green)
            } else {
                Style::default()
            }
        }
        TreeNode::AvailableItem(_) => Style::default(),
        TreeNode::UnmanagedItem(_) => {
            if color {
                Style::default().fg(Color::Yellow)
            } else {
                Style::default()
            }
        }
        TreeNode::Source(_) => {
            if color {
                Style::default().fg(Color::Cyan)
            } else {
                Style::default()
            }
        }
        TreeNode::KindBucket { .. } => {
            if color {
                Style::default().fg(Color::Blue)
            } else {
                Style::default()
            }
        }
        TreeNode::SuggestedSource(_) => {
            if color {
                Style::default().fg(Color::Magenta)
            } else {
                Style::default()
            }
        }
        // spec: TUI-50 - dependency children use a dim style to distinguish
        // them from canonical item lines in the same view; DIM survives
        // NO_COLOR (it is a video attribute, not a color).
        TreeNode::DepChild(dep) if dep.is_cycle => {
            let s = Style::default().add_modifier(Modifier::DIM);
            if color { s.fg(Color::DarkGray) } else { s }
        }
        TreeNode::DepChild(_) => {
            if color {
                Style::default().fg(Color::DarkGray)
            } else {
                Style::default()
            }
        }
        // spec: TUI-68 - a call-to-action row is dim/secondary, like a hint.
        TreeNode::EmptyState(_) => Style::default().add_modifier(Modifier::DIM),
    }
}

/// A selection/highlight style: colored reverse-video-ish bg+bold when color
/// is on, plain REVERSED (a video attribute, not a color) under NO_COLOR
/// (TUI-65). Shared by the lobes modal list and the details dialog action
/// list, which both highlight the selected row the same way as the main tree.
fn selection_style(color: bool) -> Style {
    if color {
        Style::default()
            .bg(Color::DarkGray)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().add_modifier(Modifier::REVERSED)
    }
}

/// A dim/secondary text style (hints, disabled-looking rows): DarkGray when
/// color is on, unstyled under NO_COLOR (TUI-65).
fn dim_style(color: bool) -> Style {
    if color {
        Style::default().fg(Color::DarkGray)
    } else {
        Style::default()
    }
}

/// The out-of-date drift marker (TUI-63, mirrors the CLI's CLI-155 `^`/`↑`
/// stale glyph) for an installed row whose `upgrade` would act on it. `None`
/// when the node is not a stale InstalledItem.
fn stale_suffix(node: &TreeNode, unicode: bool) -> Option<&'static str> {
    match node {
        TreeNode::InstalledItem(info) if info.stale => {
            Some(if unicode { " \u{2191}" } else { " ^" })
        }
        _ => None,
    }
}

fn flat_node_to_list_item(node: &FlatNode, rc: crate::render::OutputCtx) -> ListItem<'_> {
    let indent = "  ".repeat(node.depth);
    let expand_marker = expand_marker(node.expandable, node.expanded, rc.unicode);
    let icon = node_icon(&node.node, rc.unicode);
    let style = node_style(&node.node, rc.color);

    let mut label = format!("{indent}{expand_marker}{icon}{}", node.label);
    // spec: TUI-63 - append the out-of-date marker for a stale installed item.
    if let Some(suffix) = stale_suffix(&node.node, rc.unicode) {
        label.push_str(suffix);
    }
    ListItem::new(Line::from(vec![Span::styled(label, style)]))
}

/// The status-line text for the current app state (error takes precedence).
fn status_text(app: &App) -> String {
    if let Some(err) = &app.error {
        format!("ERROR: {err}")
    } else if let Some(msg) = &app.status {
        msg.clone()
    } else {
        String::new()
    }
}

fn draw_status(frame: &mut Frame, text: &str, is_error: bool, area: Rect) {
    // spec: TUI-65 - status/error text drops color under NO_COLOR; an error
    // still stands out via BOLD (a video attribute, not a color).
    let rc = crate::render::ctx();
    let style = if rc.color {
        let color = if is_error { Color::Red } else { Color::Green };
        Style::default().fg(color)
    } else if is_error {
        Style::default().add_modifier(Modifier::BOLD)
    } else {
        Style::default()
    };
    let widget = Paragraph::new(text.to_string())
        .style(style)
        .wrap(Wrap { trim: false });
    frame.render_widget(widget, area);
}

fn draw_hints(frame: &mut Frame, area: Rect) {
    // spec: TUI-65 - no color under NO_COLOR.
    let style = if crate::render::ctx().color {
        Style::default().fg(Color::DarkGray)
    } else {
        Style::default()
    };
    let widget = Paragraph::new(HINTS)
        .style(style)
        .wrap(Wrap { trim: false });
    frame.render_widget(widget, area);
}

/// Draw the spec-input box (TUI-30): a small centered dialog where the user
/// types a repo spec to preview.
// spec: TUI-30
fn draw_spec_input(frame: &mut Frame, app: &App, area: Rect) {
    let hint = "Enter a repo spec then press Enter. Esc to cancel.\n\
                Examples: /path/to/repo  |  file:///path/to/repo  |  owner/repo  |  https://github.com/owner/repo  |  git@github.com:owner/repo";
    let input = format!("\u{276f} {}", app.spec_input_text);
    draw_input_modal(frame, area, "Meld: enter repo spec", hint, &input);
}

/// Draw the lobes management modal (TUI-23): shows the configured agent homes
/// with navigation and `a`/`D` bindings for add/remove (CLI-111..113).
// spec: TUI-23 CLI-111 CLI-112 CLI-113
fn draw_lobes_modal(frame: &mut Frame, app: &App, area: Rect) {
    let rc = crate::render::ctx();
    let w = modal_width(area.width * 2 / 3, 50, area.width);
    let h = (app.lobes.len() as u16 + 8)
        .min(area.height.saturating_sub(4).max(1))
        .max(8.min(area.height.max(1)));
    let inner = overlay(frame, area, "Agent Homes (Lobes)", w, h);

    // Build lobe list items.
    let items: Vec<ListItem> = if app.lobes.is_empty() {
        vec![ListItem::new(Line::from(vec![Span::styled(
            "  (none configured - using default)",
            dim_style(rc.color),
        )]))]
    } else {
        app.lobes
            .iter()
            .enumerate()
            .map(|(i, lobe)| {
                let style = if i == app.lobes_selected {
                    selection_style(rc.color)
                } else if rc.color {
                    Style::default().fg(Color::Cyan)
                } else {
                    Style::default()
                };
                ListItem::new(Line::from(vec![Span::styled(format!("  {lobe}"), style)]))
            })
            .collect()
    };

    let hint_line = "  [a] add lobe    [D] remove selected    [Esc/q] close";

    // Split modal area: list at top, hint at bottom.
    let hint_h = 1u16;
    let list_h = inner.height.saturating_sub(hint_h);
    let splits = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(list_h), Constraint::Length(hint_h)])
        .split(inner);

    let list = List::new(items).highlight_style(selection_style(rc.color));
    frame.render_widget(list, splits[0]);

    let hint = Paragraph::new(hint_line)
        .style(dim_style(rc.color))
        .wrap(Wrap { trim: false });
    frame.render_widget(hint, splits[1]);
}

/// Draw the details-and-actions dialog opened with Enter on a source or item
/// (TUI-26): the node's detail at the top, its valid actions as a selectable
/// list, and a key hint. Centered and clamped to the terminal (TUI-42).
// spec: TUI-26
fn draw_dialog(frame: &mut Frame, dialog: &crate::tui::app::Dialog, area: Rect) {
    let rc = crate::render::ctx();
    let w = modal_width(area.width * 2 / 3, 40, area.width);
    let detail_h = dialog.detail.len() as u16;
    let actions_h = (dialog.actions.len() as u16).max(1);
    let content_h = detail_h
        .saturating_add(1)
        .saturating_add(actions_h)
        .saturating_add(1);
    let h = content_h
        .saturating_add(2)
        .clamp(6.min(area.height.max(1)), area.height.max(1));
    let inner = overlay(frame, area, &dialog.title, w, h);

    let splits = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(detail_h),
            Constraint::Length(1), // blank separator
            Constraint::Min(actions_h),
            Constraint::Length(1), // hint
        ])
        .split(inner);

    let detail_style = if rc.color {
        Style::default().fg(Color::Gray)
    } else {
        Style::default()
    };
    let detail = Paragraph::new(dialog.detail.join("\n"))
        .style(detail_style)
        .wrap(Wrap { trim: false });
    frame.render_widget(detail, splits[0]);

    // spec: TUI-65 - the marker glyph respects the unicode capability.
    let marker_glyph = if rc.unicode { "\u{276f} " } else { "> " };
    let items: Vec<ListItem> = dialog
        .actions
        .iter()
        .enumerate()
        .map(|(i, a)| {
            let marker = if i == dialog.selected {
                marker_glyph
            } else {
                "  "
            };
            let style = if i == dialog.selected {
                selection_style(rc.color)
            } else {
                Style::default()
            };
            ListItem::new(Line::from(vec![Span::styled(
                format!("{marker}{}", a.label),
                style,
            )]))
        })
        .collect();
    frame.render_widget(List::new(items), splits[2]);

    let hint = Paragraph::new("  [j/k] move   [Enter/y] run   [Esc/n] close")
        .style(dim_style(rc.color))
        .wrap(Wrap { trim: false });
    frame.render_widget(hint, splits[3]);
}

/// Draw the lobe-path input box (TUI-23): where the user types the path for a
/// new agent home to add via `config lobes add` (CLI-112).
// spec: TUI-23 CLI-112
fn draw_lobe_input(frame: &mut Frame, app: &App, area: Rect) {
    let hint = "Enter the agent home path (e.g. ~/.other-ai) then press Enter. Esc to cancel.";
    let input = format!("\u{276f} {}", app.lobe_input_text);
    draw_input_modal(frame, area, "Add Agent Home (Lobe)", hint, &input);
}

/// Draw the namespace-input box (TUI-53): where the user types a namespace
/// prefix to install items under `<prefix>:<name>` (NS-1). Empty input clears
/// any consumer alias (falls back to [source].prefix or no prefix). Only
/// reachable when the source has no installed items (NS-30).
// spec: TUI-53 NS-30
fn draw_namespace_input(frame: &mut Frame, app: &App, area: Rect) {
    let hint = "Enter a namespace prefix (e.g. jk) to install items as jk:<name>. Leave empty for no prefix. Enter to save, Esc to cancel.";
    let input = format!("\u{276f} {}", app.namespace_input_text);
    draw_input_modal(frame, area, "Set Namespace", hint, &input);
}

/// A centered single-field input dialog: a wrapped hint, a blank line, and the
/// input line. Width is clamped to the terminal and height grows to fit the
/// wrapped hint, so neither overflows on a narrow terminal (TUI-42).
fn draw_input_modal(frame: &mut Frame, area: Rect, title: &str, hint: &str, input: &str) {
    let w = modal_width(area.width / 2, 50, area.width);
    let inner_w = w.saturating_sub(2).max(1); // minus the side borders
    let body = format!("{hint}\n\n{input}");
    // hint rows + 1 blank + input rows + 2 borders.
    let content_h = wrapped_rows(hint, inner_w)
        .saturating_add(1)
        .saturating_add(wrapped_rows(input, inner_w));
    let h = content_h
        .saturating_add(2)
        .clamp(5.min(area.height.max(1)), area.height.max(1));
    // overlay() clears and draws the border; render the body paragraph on top.
    let inner = overlay(frame, area, title, w, h);
    let widget = Paragraph::new(body).wrap(Wrap { trim: false });
    frame.render_widget(widget, inner);
}

/// Build the confirm-modal body text. When the (Learn) action carries a
/// dependency tree (DEP-40), the tree is included between the prompt and the key
/// hint so a regression that drops it from the confirm is observable without a
/// TTY. Otherwise the modal stays as a single prompt line plus the hint.
// spec: DEP-40
fn confirm_modal_text(action: &crate::tui::app::PendingAction) -> String {
    match action.dep_tree.as_deref() {
        Some(tree) => format!(
            "{}\n\n{}\n  [y] confirm   [n/Esc] cancel",
            action.description,
            tree.trim_end_matches('\n')
        ),
        None => format!("{}\n\n  [y] confirm   [n/Esc] cancel", action.description),
    }
}

fn draw_modal(frame: &mut Frame, app: &App, area: Rect) {
    let Some(action) = &app.pending_action else {
        return;
    };

    // When a Learn action carries a dependency tree (DEP-40), show it between the
    // prompt and the key hint so the user sees the closure the confirm will pull
    // in (the selected / dependency / already-installed distinction comes from
    // the rendered tree itself). The tree is multi-line ASCII; size the modal to
    // fit it (bounded by the available width/height, and wrapping a row that is
    // still wider than the terminal rather than truncating it, TUI-42).
    // spec: DEP-40
    let text = confirm_modal_text(action);

    // Center a dialog sized to the content. Width grows to fit the widest line
    // (tree rows can be long), bounded by the terminal; height to the wrapped
    // line count, also bounded. Measured in display columns (TUI-67), not a
    // raw char count, so a CJK/emoji-heavy line still gets a wide-enough modal.
    // spec: TUI-67
    let content_w = text
        .lines()
        .map(crate::render::visible_width)
        .max()
        .unwrap_or(0) as u16;
    let w = (content_w + 4)
        .max(area.width / 2)
        .max(40)
        .min(area.width.max(1));
    let inner_w = w.saturating_sub(2).max(1);
    // +2 for the top/bottom borders.
    let h = wrapped_rows(&text, inner_w)
        .saturating_add(2)
        .clamp(5.min(area.height.max(1)), area.height.max(1));
    // overlay() clears and draws the border; render the body paragraph on top.
    let inner = overlay(frame, area, "Confirm", w, h);
    let widget = Paragraph::new(text).wrap(Wrap { trim: false });
    frame.render_widget(widget, inner);
}

#[cfg(test)]
mod tests {
    use super::{
        HELP_TEXT, HINTS, confirm_modal_text, dim_style, expand_marker, modal_width, node_icon,
        node_style, scroll_offset, selection_style, stale_suffix, status_height, wrapped_rows,
    };
    use crate::tui::app::{ActionKind, PendingAction};
    use crate::tui::tree::{InstalledInfo, SourceInfo, TreeNode};
    use ratatui::style::Modifier;

    fn installed_node(stale: bool) -> TreeNode {
        TreeNode::InstalledItem(InstalledInfo {
            key: "skill:review".to_string(),
            name: "review".to_string(),
            source: "local/agents".to_string(),
            kind: crate::error::ItemKind::Skill,
            commit: "abc12345".to_string(),
            description: None,
            deps: vec![],
            stale,
        })
    }

    // ==========================================================================
    // TUI-65: NO_COLOR / non-UTF-8 locale (M12)
    // ==========================================================================

    #[test]
    fn expand_marker_ascii_fallback_has_no_unicode() {
        // spec: TUI-65 - under a non-UTF-8 locale (unicode=false) the disclosure
        // triangle must not draw a Unicode glyph that would come out as mojibake.
        for expanded in [true, false] {
            let m = expand_marker(true, expanded, false);
            assert!(
                m.is_ascii(),
                "ASCII-mode expand marker must be pure ASCII: {m:?}"
            );
        }
        // Non-expandable stays the same two-space filler in both modes.
        assert_eq!(expand_marker(false, false, true), "  ");
        assert_eq!(expand_marker(false, false, false), "  ");
    }

    #[test]
    fn expand_marker_unicode_mode_uses_triangles() {
        // spec: TUI-65
        assert_eq!(expand_marker(true, true, true), "\u{25be} ");
        assert_eq!(expand_marker(true, false, true), "\u{25b8} ");
    }

    #[test]
    fn node_icon_ascii_fallback_has_no_unicode() {
        // spec: TUI-65 - every node kind's ASCII icon must be pure ASCII.
        let node = installed_node(false);
        let icon = node_icon(&node, false);
        assert!(
            icon.is_ascii(),
            "ASCII-mode icon must be pure ASCII: {icon:?}"
        );
        assert_eq!(icon, "+ ");
    }

    #[test]
    fn node_icon_unicode_mode_uses_geometric_marker() {
        // spec: TUI-65
        let node = installed_node(false);
        assert_eq!(node_icon(&node, true), "\u{25cf} ");
    }

    #[test]
    fn node_style_drops_fg_color_under_no_color() {
        // spec: TUI-65 - NO_COLOR (color=false) must not set any `fg`; ratatui's
        // `Style::default()` has `fg: None`, so this is directly observable.
        let installed = installed_node(false);
        let src = TreeNode::Source(SourceInfo {
            name: "local/agents".to_string(),
            installed: true,
        });
        assert_eq!(
            node_style(&installed, false).fg,
            None,
            "installed item must carry no fg color under NO_COLOR"
        );
        assert_eq!(
            node_style(&src, false).fg,
            None,
            "source node must carry no fg color under NO_COLOR"
        );
        // With color on, the installed row IS colored (green), proving the
        // false-branch above is a real gate and not just an always-None style.
        assert!(
            node_style(&installed, true).fg.is_some(),
            "installed item must carry a fg color when color is on"
        );
    }

    #[test]
    fn status_height_empty_text_is_one_row() {
        // spec: TUI-70
        assert_eq!(status_height("", 80, 40), 1);
    }

    #[test]
    fn status_height_grows_past_three_rows_on_a_tall_terminal() {
        // spec: TUI-70 - a long error must be able to claim more than the old
        // flat 3-row ceiling when the terminal is tall enough to show it; a
        // regression to the old `clamp(1, 3)` would cap this at 3 regardless
        // of the 40-row terminal.
        let long_error = "ERROR: ".to_string() + &"word ".repeat(60); // wraps to many rows at width 20
        let h = status_height(&long_error, 20, 40);
        assert!(
            h > 3,
            "a long error on a tall terminal must get more than 3 rows, got {h}"
        );
    }

    #[test]
    fn status_height_never_starves_the_tree_pane_on_a_short_terminal() {
        // spec: TUI-70 - even an extremely long message must leave the fixed
        // reserve (search bar + minimum tree + minimum hint row) on a short
        // terminal, so the status pane cannot claim the whole screen.
        let long_error = "x ".repeat(500);
        let h = status_height(&long_error, 10, 12);
        assert!(
            h <= 12,
            "status height must never exceed the terminal height: {h}"
        );
        assert!(
            h < 12 - 3,
            "some rows must remain for search/tree/hints on a 12-row terminal: {h}"
        );
    }

    #[test]
    fn selection_style_uses_reversed_not_bg_color_under_no_color() {
        // spec: TUI-65 - the selection highlight must not set a `bg` color under
        // NO_COLOR, and must instead use the REVERSED modifier so the row is
        // still visually distinguishable on a monochrome terminal.
        let mono = selection_style(false);
        assert_eq!(mono.bg, None, "no bg color under NO_COLOR: {mono:?}");
        assert!(
            mono.add_modifier.contains(Modifier::REVERSED),
            "must use REVERSED under NO_COLOR: {mono:?}"
        );
        let colored = selection_style(true);
        assert!(colored.bg.is_some(), "color mode sets a bg: {colored:?}");
    }

    #[test]
    fn dim_style_drops_fg_under_no_color() {
        // spec: TUI-65
        assert_eq!(dim_style(false).fg, None);
        assert!(dim_style(true).fg.is_some());
    }

    #[test]
    fn stale_suffix_ascii_and_unicode_glyphs() {
        // spec: TUI-63 TUI-65 - a stale installed row gets a drift marker whose
        // glyph respects the unicode capability; a non-stale row gets none.
        assert_eq!(stale_suffix(&installed_node(true), true), Some(" \u{2191}"));
        assert_eq!(stale_suffix(&installed_node(true), false), Some(" ^"));
        assert_eq!(stale_suffix(&installed_node(false), true), None);
        assert_eq!(stale_suffix(&installed_node(false), false), None);
    }

    #[test]
    fn hints_line_covers_search_collapse_paging_and_help() {
        // spec: TUI-64 - M11 discoverability: the bottom hint line must mention
        // search, h/l collapse/expand, paging, and the `?` help key, not just the
        // subset that existed before (which omitted all four).
        assert!(
            HINTS.contains("/ search"),
            "HINTS must mention search: {HINTS:?}"
        );
        assert!(
            HINTS.contains("h/l"),
            "HINTS must mention h/l collapse/expand: {HINTS:?}"
        );
        assert!(
            HINTS.to_lowercase().contains("page"),
            "HINTS must mention paging: {HINTS:?}"
        );
        assert!(
            HINTS.contains("? help"),
            "HINTS must mention the ? help key: {HINTS:?}"
        );
    }

    #[test]
    fn help_text_lists_every_normal_mode_key() {
        // spec: TUI-64 - the help overlay must document every key that HINTS
        // only abbreviates: navigation, every mutating action, and confirm/cancel.
        for key in [
            "j/k", "h/l", "Space", "Enter", "i ", "d ", "s ", "u ", "m ", "M ", "C ", "y / n",
            "Esc", "q,",
        ] {
            assert!(
                HELP_TEXT.contains(key),
                "help overlay must mention {key:?}: {HELP_TEXT:?}"
            );
        }
    }

    #[test]
    fn scroll_offset_keeps_selection_in_middle_band() {
        // spec: TUI-16 - the highlight stays within the middle two-thirds: with a
        // 12-row viewport the margin is 12/6 = 2, so the selection is held between
        // row 2 and row 9 of the band.
        let rows = 12;
        let len = 100;
        // Everything fits -> no scrolling.
        assert_eq!(scroll_offset(0, 5, 8, 12), 0, "list shorter than viewport");
        // At the very top the selection sits at the edge (nothing above to show).
        assert_eq!(scroll_offset(0, 0, len, rows), 0);
        // Moving down within the top band does not scroll yet (selection < rows-1-margin = 9).
        assert_eq!(scroll_offset(0, 9, len, rows), 0, "still inside the band");
        // One past the bottom margin: the view scrolls so the selection stays in the band.
        let off = scroll_offset(0, 10, len, rows);
        assert!(
            off >= 1,
            "must scroll once selection passes the bottom margin: {off}"
        );
        assert!(
            (off..off + rows).contains(&10),
            "selection stays visible after scroll"
        );
        let margin = rows / 6;
        assert!(
            10 >= off + margin && 10 <= off + rows - 1 - margin,
            "selection stays within the middle band [{}, {}], off={off}",
            off + margin,
            off + rows - 1 - margin
        );
    }

    #[test]
    fn scroll_offset_clamps_at_list_end() {
        // spec: TUI-16 - near the end there is nothing further to scroll, so the
        // offset clamps to len-rows and the highlight may reach the bottom edge.
        let rows = 10;
        let len = 30;
        let off = scroll_offset(0, len - 1, len, rows);
        assert_eq!(off, len - rows, "offset clamps to the last full page");
        // The last row is selectable and visible.
        assert!((off..off + rows).contains(&(len - 1)));
    }

    #[test]
    fn scroll_offset_zero_rows_returns_zero_without_panic() {
        // spec: TUI-16 - a zero-height viewport (rows == 0) must not panic on the
        // `len - rows` / margin arithmetic. The `len <= rows` short-circuit does
        // NOT cover this when len > 0 (0 < len is false-leaning), so the explicit
        // `rows == 0` guard is exercised here: it returns 0.
        assert_eq!(scroll_offset(0, 0, 0, 0), 0, "empty list, zero rows");
        assert_eq!(scroll_offset(5, 50, 100, 0), 0, "nonempty list, zero rows");
    }

    #[test]
    fn scroll_offset_tiny_viewport_margin_zero_never_inverts_bounds() {
        // spec: TUI-16 - on a viewport smaller than 6 rows the margin (rows/6) is 0,
        // so the band collapses to the full viewport. The offset must still keep the
        // selection visible and clamp into [0, len-rows] without the lo/hi bounds
        // inverting (the `lo.min(hi)` safety). Regression target: a panic or an
        // off-screen selection on a very short terminal.
        let len = 20;
        for rows in 1..=5usize {
            let margin = rows / 6;
            assert_eq!(margin, 0, "margin is 0 below 6 rows");
            for selected in 0..len {
                let off = scroll_offset(0, selected, len, rows);
                assert!(off <= len - rows, "offset within page range, rows={rows}");
                assert!(
                    (off..off + rows).contains(&selected),
                    "selection {selected} must stay visible on a {rows}-row viewport, off={off}"
                );
            }
        }
    }

    #[test]
    fn scroll_offset_scrolls_up_when_selection_rises_above_the_band() {
        // spec: TUI-16 - the symmetric case to the scroll-down test: starting from a
        // scrolled-down offset, moving the selection up past the TOP margin must
        // scroll the view up so the highlight does not sit at the very top edge while
        // there are rows above to show. The existing tests only cover downward
        // motion; this pins the upper-bound branch (lo) of the clamp.
        let rows = 12;
        let len = 100;
        let margin = rows / 6; // 2
        // Start scrolled down with the selection near the bottom of the band.
        let prev = 50;
        // Selection moves up to just above the top margin of that band.
        let selected = prev + margin - 1; // 51, inside [prev+margin=52? no] -> above top margin
        let off = scroll_offset(prev, selected, len, rows);
        assert!(off < prev, "view must scroll up: off={off} prev={prev}");
        assert!(
            selected >= off + margin && selected <= off + rows - 1 - margin,
            "selection stays within the middle band [{}, {}] after scrolling up, off={off}",
            off + margin,
            off + rows - 1 - margin
        );
    }

    #[test]
    fn scroll_offset_is_stable_within_the_band() {
        // spec: TUI-16 - a previous offset is preserved while the selection stays
        // inside the band (the list does not jump on every keystroke).
        let rows = 12;
        let len = 100;
        // Selection at row 40 with a prior offset of 35: 40 is within [37, 44], so
        // the offset is left unchanged.
        assert_eq!(scroll_offset(35, 40, len, rows), 35);
    }

    #[test]
    fn wrapped_rows_counts_word_wrap_and_hard_splits() {
        // spec: TUI-42 - the row estimate that keeps wrapped content from being
        // clipped on a narrow terminal.
        // Short text fits on one row.
        assert_eq!(wrapped_rows("hello world", 40), 1);
        // "hello world" (11 cols) at width 7 wraps to "hello" + "world" = 2 rows.
        assert_eq!(wrapped_rows("hello world", 7), 2);
        // Explicit newlines always break (and an empty segment is its own row).
        assert_eq!(wrapped_rows("a\n\nb", 40), 3);
        // A single word longer than the width hard-splits across rows.
        assert_eq!(wrapped_rows("abcdefghij", 4), 3); // 10 cols / 4 -> 3 rows
        // Degenerate width never panics and is at least one row.
        assert!(wrapped_rows("anything", 0) >= 1);
        assert_eq!(wrapped_rows("", 10), 1);
    }

    #[test]
    fn wrapped_rows_measures_wide_chars_by_display_width_not_char_count() {
        // spec: TUI-67 - four CJK chars occupy 8 display columns (2 each), not 4;
        // at a width of 4 a char-count-based estimate would (wrongly) fit them
        // on one row, while a display-width-aware one correctly wraps to 2.
        let cjk = "\u{4e2d}\u{6587}\u{5b57}\u{7b26}"; // 4 chars, 8 columns
        assert_eq!(
            wrapped_rows(cjk, 4),
            2,
            "4 wide chars (8 columns) at width 4 must wrap to 2 rows"
        );
        // The equivalent 8-column-wide ASCII string wraps the same way, proving
        // the two are measured on a common (display-width) basis.
        assert_eq!(wrapped_rows("abcdefgh", 4), 2);
    }

    #[test]
    fn modal_width_clamps_to_the_terminal() {
        // spec: TUI-42 - a modal is at least `min` wide for readability but never
        // wider than the terminal, so it cannot be pushed off screen.
        // Roomy terminal: the minimum floor applies.
        assert_eq!(modal_width(40, 50, 80), 50);
        // Desired above the floor is kept (still within the terminal).
        assert_eq!(modal_width(60, 50, 80), 60);
        // Narrow terminal: clamp below the floor to what is available.
        assert_eq!(modal_width(40, 50, 45), 45);
        assert_eq!(modal_width(40, 50, 10), 10);
    }

    #[test]
    fn confirm_modal_includes_dependency_tree_for_learn() {
        // spec: DEP-40 - a Learn confirm carrying a dependency tree must render
        // that tree in the modal body (so the user sees the closure before
        // applying). A regression that drops the tree from the confirm fails here
        // without needing a TTY.
        let tree = "review (selected)\n  dev (dependency)\n    test (already installed)";
        let mut action = PendingAction::new(
            ActionKind::Learn {
                item_key: "skill:review".to_string(),
                source: "local/agents".to_string(),
            },
            "Install skill:review from local/agents?".to_string(),
        );
        action.dep_tree = Some(tree.to_string());

        let body = confirm_modal_text(&action);
        // Each tree line must appear verbatim in the modal body.
        for line in tree.lines() {
            assert!(
                body.contains(line),
                "confirm modal must show the dependency tree line {line:?}; body was:\n{body}"
            );
        }
        // The prompt and key hint must still be present.
        assert!(
            body.contains("Install skill:review from local/agents?"),
            "modal must keep the action description"
        );
        assert!(
            body.contains("[y] confirm"),
            "modal must keep the confirm hint"
        );
    }

    #[test]
    fn confirm_modal_places_tree_between_prompt_and_key_hint() {
        // spec: DEP-40 - ORDER is load-bearing: the dependency tree must appear AFTER
        // the prompt line and BEFORE the key-hint/confirm line. The previous test only
        // checks the tree lines are present somewhere; this pins their position, so a
        // regression that reordered (tree before prompt, or hint before tree) fails.
        let tree = "- skill:review [selected]\n  - agent:dev [dep]\n    - skill:build [installed]";
        let mut action = PendingAction::new(
            ActionKind::Learn {
                item_key: "skill:review".to_string(),
                source: "local/agents".to_string(),
            },
            "Install skill:review from local/agents?".to_string(),
        );
        action.dep_tree = Some(tree.to_string());

        let body = confirm_modal_text(&action);
        let lines: Vec<&str> = body.lines().collect();

        // Prompt is the first line.
        assert_eq!(
            lines.first().copied(),
            Some("Install skill:review from local/agents?"),
            "the prompt must be the first line of the modal body: {body:?}"
        );

        let prompt_idx = 0usize;
        let tree_first_idx = lines
            .iter()
            .position(|l| l.contains("skill:review [selected]"))
            .expect("tree root line must be present");
        let tree_last_idx = lines
            .iter()
            .position(|l| l.contains("skill:build [installed]"))
            .expect("tree leaf line must be present");
        let hint_idx = lines
            .iter()
            .position(|l| l.contains("[y] confirm"))
            .expect("key-hint line must be present");

        assert!(
            prompt_idx < tree_first_idx,
            "the tree must come AFTER the prompt line: {body:?}"
        );
        assert!(
            tree_last_idx < hint_idx,
            "the key hint must come AFTER the whole tree: {body:?}"
        );
        // The tree lines are contiguous and in source order.
        assert!(
            tree_first_idx < tree_last_idx,
            "tree lines must keep their source order (root before leaf): {body:?}"
        );

        // A 3-line tree, a 1-line prompt, a 1-line hint, plus the two blank
        // separators -> exactly 6 lines. This pins that NO tree line was dropped or
        // truncated (a truncation regression would change the count).
        assert_eq!(
            lines.len(),
            6,
            "prompt + blank + 3 tree lines + blank + hint = 6 lines; got {}: {body:?}",
            lines.len()
        );
        // And every one of the three tree rows survived verbatim.
        for row in tree.lines() {
            assert!(
                lines.contains(&row),
                "tree row {row:?} must appear verbatim (no truncation): {body:?}"
            );
        }
    }

    #[test]
    fn confirm_modal_omits_tree_when_no_dependencies() {
        // spec: DEP-40 - when no dependency tree is attached (closure adds
        // nothing, or a non-Learn action), the confirm stays a plain prompt: no
        // stray tree, just the description and the key hint.
        let action = PendingAction::new(ActionKind::Sync, "Sync all sources?".to_string());
        let body = confirm_modal_text(&action);
        assert_eq!(
            body, "Sync all sources?\n\n  [y] confirm   [n/Esc] cancel",
            "a treeless confirm must be exactly the prompt plus the key hint"
        );
    }
}
