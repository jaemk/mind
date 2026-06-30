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
/// terminals (TUI-42).
const HINTS: &str = " j/k move \u{b7} Enter details \u{b7} Space expand \u{b7} i install \u{b7} d delete \u{b7} s sync \u{b7} u upgrade \u{b7} m meld \u{b7} M unmeld \u{b7} C lobes \u{b7} q quit";

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
        let wl = word.chars().count();
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
    let status_h = if status_text.is_empty() {
        1
    } else {
        wrapped_rows(&status_text, size.width).clamp(1, 3)
    };
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
}

/// A rounded-border block with a title, the common frame for panes and modals.
fn titled_block(title: &str) -> Block<'_> {
    Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
}

/// Center a `w x h` overlay inside `area` (clamping to fit), render a
/// `Clear` to erase what was under it, render a yellow titled border block,
/// and return the inner `Rect` (the usable content area inside the border).
/// Used by all four modal draw functions so the centering and Clear logic
/// live in one place (TUI-42).
fn overlay(frame: &mut Frame, area: Rect, title: &str, w: u16, h: u16) -> Rect {
    let w = w.min(area.width.max(1));
    let h = h.min(area.height.max(1));
    let x = (area.width.saturating_sub(w)) / 2;
    let y = (area.height.saturating_sub(h)) / 2;
    let modal_area = Rect::new(x, y, w, h);
    let block = titled_block(title).style(ratatui::style::Style::default().fg(Color::Yellow));
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
    let style = if app.search_focused {
        Style::default().fg(Color::Yellow)
    } else {
        Style::default()
    };
    let text = Paragraph::new(app.search.as_str())
        .block(titled_block(title))
        .style(style);
    frame.render_widget(text, area);
}

fn draw_tree(frame: &mut Frame, app: &App, area: Rect) {
    let items: Vec<ListItem> = app
        .visible
        .iter()
        .map(|node| flat_node_to_list_item(node))
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

    let list = List::new(items)
        .block(titled_block("Items"))
        .highlight_style(
            Style::default()
                .bg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("\u{276f} "); // heavy right-pointing angle

    frame.render_stateful_widget(list, area, &mut state);
}

fn flat_node_to_list_item(node: &FlatNode) -> ListItem<'_> {
    let indent = "  ".repeat(node.depth);
    // Disclosure triangle for expandable rows; two spaces keep leaves aligned.
    let expand_marker = if node.expandable {
        if node.expanded {
            "\u{25be} " // down-pointing triangle
        } else {
            "\u{25b8} " // right-pointing triangle
        }
    } else {
        "  "
    };

    // A geometric marker per node kind: filled = present/installed, hollow =
    // available; group headers carry no marker (the bold label leads).
    let icon = match &node.node {
        TreeNode::InstalledGroup | TreeNode::AvailableGroup | TreeNode::UnmanagedGroup => "",
        TreeNode::Source(_) => "\u{25c6} ", // filled diamond
        TreeNode::KindBucket { .. } => "\u{25aa} ", // small square
        TreeNode::InstalledItem(_) => "\u{25cf} ", // filled circle
        TreeNode::AvailableItem(_) => "\u{25cb} ", // hollow circle
        TreeNode::UnmanagedItem(_) => "\u{25cb} ", // hollow circle (not mind-managed)
        TreeNode::SuggestedSource(_) => "\u{25c7} ", // hollow diamond
        // spec: TUI-50 - dependency child nodes shown under an expanded item.
        TreeNode::DepChild(dep) if dep.is_cycle => "\u{21ba} ", // cycle arrow
        TreeNode::DepChild(_) => "\u{21b3} ",                   // dep arrow
    };

    let style = match &node.node {
        TreeNode::InstalledGroup | TreeNode::AvailableGroup | TreeNode::UnmanagedGroup => {
            Style::default().add_modifier(Modifier::BOLD)
        }
        TreeNode::InstalledItem(_) => Style::default().fg(Color::Green),
        TreeNode::AvailableItem(_) => Style::default(),
        TreeNode::UnmanagedItem(_) => Style::default().fg(Color::Yellow),
        TreeNode::Source(_) => Style::default().fg(Color::Cyan),
        TreeNode::KindBucket { .. } => Style::default().fg(Color::Blue),
        TreeNode::SuggestedSource(_) => Style::default().fg(Color::Magenta),
        // spec: TUI-50 - dependency children use a dim style to distinguish
        // them from canonical item lines in the same view.
        TreeNode::DepChild(dep) if dep.is_cycle => Style::default()
            .fg(Color::DarkGray)
            .add_modifier(Modifier::DIM),
        TreeNode::DepChild(_) => Style::default().fg(Color::DarkGray),
    };

    let label = format!("{indent}{expand_marker}{icon}{}", node.label);
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
    let color = if is_error { Color::Red } else { Color::Green };
    let widget = Paragraph::new(text.to_string())
        .style(Style::default().fg(color))
        .wrap(Wrap { trim: false });
    frame.render_widget(widget, area);
}

fn draw_hints(frame: &mut Frame, area: Rect) {
    let widget = Paragraph::new(HINTS)
        .style(Style::default().fg(Color::DarkGray))
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
    let w = modal_width(area.width * 2 / 3, 50, area.width);
    let h = (app.lobes.len() as u16 + 8)
        .min(area.height.saturating_sub(4).max(1))
        .max(8.min(area.height.max(1)));
    let inner = overlay(frame, area, "Agent Homes (Lobes)", w, h);

    // Build lobe list items.
    let items: Vec<ListItem> = if app.lobes.is_empty() {
        vec![ListItem::new(Line::from(vec![Span::styled(
            "  (none configured - using default)",
            Style::default().fg(Color::DarkGray),
        )]))]
    } else {
        app.lobes
            .iter()
            .enumerate()
            .map(|(i, lobe)| {
                let style = if i == app.lobes_selected {
                    Style::default()
                        .bg(Color::DarkGray)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(Color::Cyan)
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

    let list = List::new(items).highlight_style(
        Style::default()
            .bg(Color::DarkGray)
            .add_modifier(Modifier::BOLD),
    );
    frame.render_widget(list, splits[0]);

    let hint = Paragraph::new(hint_line)
        .style(Style::default().fg(Color::DarkGray))
        .wrap(Wrap { trim: false });
    frame.render_widget(hint, splits[1]);
}

/// Draw the details-and-actions dialog opened with Enter on a source or item
/// (TUI-26): the node's detail at the top, its valid actions as a selectable
/// list, and a key hint. Centered and clamped to the terminal (TUI-42).
// spec: TUI-26
fn draw_dialog(frame: &mut Frame, dialog: &crate::tui::app::Dialog, area: Rect) {
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

    let detail = Paragraph::new(dialog.detail.join("\n"))
        .style(Style::default().fg(Color::Gray))
        .wrap(Wrap { trim: false });
    frame.render_widget(detail, splits[0]);

    let items: Vec<ListItem> = dialog
        .actions
        .iter()
        .enumerate()
        .map(|(i, a)| {
            let marker = if i == dialog.selected {
                "\u{276f} "
            } else {
                "  "
            };
            let style = if i == dialog.selected {
                Style::default()
                    .bg(Color::DarkGray)
                    .add_modifier(Modifier::BOLD)
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
        .style(Style::default().fg(Color::DarkGray))
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
    // line count, also bounded.
    let content_w = text.lines().map(|l| l.chars().count()).max().unwrap_or(0) as u16;
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
    use super::{confirm_modal_text, modal_width, scroll_offset, wrapped_rows};
    use crate::tui::app::{ActionKind, PendingAction};

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
