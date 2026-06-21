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
    let hints = "j/k:move  Enter:expand  i:install  d:delete  s:sync  u:upgrade  m:meld(preview)  Enter on ?:preview  C:lobes  q:quit";
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
    let widget = Paragraph::new(text).block(
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
    let h = (app.lobes.len() as u16 + 8)
        .min(area.height.saturating_sub(4))
        .max(8);
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
                ListItem::new(Line::from(vec![Span::styled(format!("  {lobe}"), style)]))
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

    let list = List::new(items).highlight_style(
        Style::default()
            .bg(Color::DarkGray)
            .add_modifier(Modifier::BOLD),
    );
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
    let widget = Paragraph::new(text).block(
        Block::default()
            .title("Add Agent Home (Lobe)")
            .borders(Borders::ALL)
            .style(Style::default().fg(Color::Yellow)),
    );
    frame.render_widget(ratatui::widgets::Clear, modal_area);
    frame.render_widget(widget, modal_area);
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
    // fit it (bounded by the available height).
    // spec: DEP-40
    let text = confirm_modal_text(action);

    // Center a dialog sized to the content. Width grows to fit the widest line
    // (tree rows can be long), height to the line count, both bounded by the area.
    let line_count = text.lines().count() as u16;
    let content_w = text.lines().map(|l| l.chars().count()).max().unwrap_or(0) as u16;
    let w = (content_w + 4)
        .max(area.width / 2)
        .max(40)
        .min(area.width.max(1));
    // +2 for the top/bottom borders.
    let h = (line_count + 2).max(5).min(area.height.max(1));
    let x = (area.width.saturating_sub(w)) / 2;
    let y = (area.height.saturating_sub(h)) / 2;
    let modal_area = Rect::new(x, y, w, h);

    let widget = Paragraph::new(text).block(
        Block::default()
            .title("Confirm")
            .borders(Borders::ALL)
            .style(Style::default().fg(Color::Yellow)),
    );
    frame.render_widget(ratatui::widgets::Clear, modal_area);
    frame.render_widget(widget, modal_area);
}

#[cfg(test)]
mod tests {
    use super::confirm_modal_text;
    use crate::tui::app::{ActionKind, PendingAction};

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
