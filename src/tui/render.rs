//! Ratatui draw functions for the TUI.
//!
//! Pure render of `&App`; no state mutation, no lock.

use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph};

use crate::error::Result;
use crate::tui::app::{App, FlatNode};
use crate::tui::tree::TreeNode;

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

    // Layout: search bar at top, main tree in middle, status at bottom.
    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // search bar
            Constraint::Min(1),    // tree
            Constraint::Length(1), // status line
            Constraint::Length(1), // key hint line
        ])
        .split(size);

    draw_search_bar(frame, app, layout[0]);
    draw_tree(frame, app, layout[1]);
    draw_status(frame, app, layout[2]);
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
}

fn draw_search_bar(frame: &mut Frame, app: &App, area: Rect) {
    let title = if app.search_focused { "Search (ESC to clear)" } else { "Search (/) to focus" };
    let style = if app.search_focused {
        Style::default().fg(Color::Yellow)
    } else {
        Style::default()
    };
    let text = Paragraph::new(app.search.as_str())
        .block(Block::default().title(title).borders(Borders::ALL))
        .style(style);
    frame.render_widget(text, area);
}

fn draw_tree(frame: &mut Frame, app: &App, area: Rect) {
    let items: Vec<ListItem> = app
        .visible
        .iter()
        .map(|node| flat_node_to_list_item(node))
        .collect();

    let mut state = ListState::default();
    state.select(Some(app.selected));

    let list = List::new(items)
        .block(Block::default().title("Items").borders(Borders::ALL))
        .highlight_style(
            Style::default()
                .bg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("> ");

    frame.render_stateful_widget(list, area, &mut state);
}

fn flat_node_to_list_item(node: &FlatNode) -> ListItem<'_> {
    let indent = "  ".repeat(node.depth);
    let expand_marker = if node.expandable {
        if node.expanded { "[-] " } else { "[+] " }
    } else {
        "    "
    };

    let icon = match &node.node {
        TreeNode::InstalledGroup => "=== ",
        TreeNode::AvailableGroup => "=== ",
        TreeNode::Source(_) => "@ ",
        TreeNode::KindBucket { .. } => "# ",
        TreeNode::InstalledItem(_) => "* ",
        TreeNode::AvailableItem(_) => "  ",
        TreeNode::SuggestedSource(_) => "? ",
    };

    let style = match &node.node {
        TreeNode::InstalledGroup | TreeNode::AvailableGroup => {
            Style::default().add_modifier(Modifier::BOLD)
        }
        TreeNode::InstalledItem(_) => Style::default().fg(Color::Green),
        TreeNode::AvailableItem(_) => Style::default(),
        TreeNode::Source(_) => Style::default().fg(Color::Cyan),
        TreeNode::KindBucket { .. } => Style::default().fg(Color::Blue),
        TreeNode::SuggestedSource(_) => Style::default().fg(Color::Magenta),
    };

    let label = format!("{indent}{expand_marker}{icon}{}", node.label);
    ListItem::new(Line::from(vec![Span::styled(label, style)]))
}

fn draw_status(frame: &mut Frame, app: &App, area: Rect) {
    let text = if let Some(err) = &app.error {
        Span::styled(format!("ERROR: {err}"), Style::default().fg(Color::Red))
    } else if let Some(msg) = &app.status {
        Span::styled(msg.clone(), Style::default().fg(Color::Green))
    } else {
        Span::raw(String::new())
    };
    frame.render_widget(Paragraph::new(Line::from(vec![text])), area);
}

fn draw_hints(frame: &mut Frame, area: Rect) {
    let hints = "j/k:move  Enter:expand  i:install  d:delete  s:sync  e:evolve  m:meld(preview)  Enter on ?:preview  C:lobes  q:quit";
    let text = Span::styled(hints, Style::default().fg(Color::DarkGray));
    frame.render_widget(Paragraph::new(Line::from(vec![text])), area);
}

/// Draw the spec-input box (TUI-30): a small centered dialog where the user
/// types a repo spec to preview.
// spec: TUI-30
fn draw_spec_input(frame: &mut Frame, app: &App, area: Rect) {
    let w = (area.width / 2).max(50);
    let h = 5u16;
    let x = (area.width.saturating_sub(w)) / 2;
    let y = (area.height.saturating_sub(h)) / 2;
    let modal_area = Rect::new(x, y, w, h);

    let hint = "Enter a repo spec (path, host/owner/repo) then press Enter. Esc to cancel.";
    let text = format!("{hint}\n\n> {}", app.spec_input_text);
    let widget = Paragraph::new(text)
        .block(
            Block::default()
                .title("Meld: enter repo spec")
                .borders(Borders::ALL)
                .style(Style::default().fg(Color::Yellow)),
        );
    frame.render_widget(ratatui::widgets::Clear, modal_area);
    frame.render_widget(widget, modal_area);
}

/// Draw the lobes management modal (TUI-23): shows the configured agent homes
/// with navigation and `a`/`D` bindings for add/remove (CLI-111..113).
// spec: TUI-23 CLI-111 CLI-112 CLI-113
fn draw_lobes_modal(frame: &mut Frame, app: &App, area: Rect) {
    let w = (area.width * 2 / 3).max(50);
    let h = (app.lobes.len() as u16 + 8).min(area.height.saturating_sub(4)).max(8);
    let x = (area.width.saturating_sub(w)) / 2;
    let y = (area.height.saturating_sub(h)) / 2;
    let modal_area = Rect::new(x, y, w, h);

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
                ListItem::new(Line::from(vec![Span::styled(
                    format!("  {lobe}"),
                    style,
                )]))
            })
            .collect()
    };

    let hint_line = "  [a] add lobe    [D] remove selected    [Esc/q] close";
    let block = Block::default()
        .title("Agent Homes (Lobes)")
        .borders(Borders::ALL)
        .style(Style::default().fg(Color::Yellow));

    // Split modal area: list at top, hint at bottom.
    let inner = block.inner(modal_area);
    frame.render_widget(ratatui::widgets::Clear, modal_area);
    frame.render_widget(block, modal_area);

    let hint_h = 1u16;
    let list_h = inner.height.saturating_sub(hint_h);
    let splits = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(list_h), Constraint::Length(hint_h)])
        .split(inner);

    let list = List::new(items)
        .highlight_style(Style::default().bg(Color::DarkGray).add_modifier(Modifier::BOLD));
    frame.render_widget(list, splits[0]);

    let hint = Paragraph::new(hint_line).style(Style::default().fg(Color::DarkGray));
    frame.render_widget(hint, splits[1]);
}

/// Draw the lobe-path input box (TUI-23): where the user types the path for a
/// new agent home to add via `config lobes add` (CLI-112).
// spec: TUI-23 CLI-112
fn draw_lobe_input(frame: &mut Frame, app: &App, area: Rect) {
    let w = (area.width / 2).max(55);
    let h = 5u16;
    let x = (area.width.saturating_sub(w)) / 2;
    let y = (area.height.saturating_sub(h)) / 2;
    let modal_area = Rect::new(x, y, w, h);

    let hint = "Enter the agent home path (e.g. ~/.other-ai) then press Enter. Esc to cancel.";
    let text = format!("{hint}\n\n> {}", app.lobe_input_text);
    let widget = Paragraph::new(text)
        .block(
            Block::default()
                .title("Add Agent Home (Lobe)")
                .borders(Borders::ALL)
                .style(Style::default().fg(Color::Yellow)),
        );
    frame.render_widget(ratatui::widgets::Clear, modal_area);
    frame.render_widget(widget, modal_area);
}

fn draw_modal(frame: &mut Frame, app: &App, area: Rect) {
    let Some(action) = &app.pending_action else { return };

    // Center a small dialog.
    let w = (area.width / 2).max(40);
    let h = 5u16;
    let x = (area.width.saturating_sub(w)) / 2;
    let y = (area.height.saturating_sub(h)) / 2;
    let modal_area = Rect::new(x, y, w, h);

    let text = format!("{}\n\n  [y] confirm   [n/Esc] cancel", action.description);
    let widget = Paragraph::new(text)
        .block(
            Block::default()
                .title("Confirm")
                .borders(Borders::ALL)
                .style(Style::default().fg(Color::Yellow)),
        );
    frame.render_widget(ratatui::widgets::Clear, modal_area);
    frame.render_widget(widget, modal_area);
}
