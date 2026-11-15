use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{List, ListItem, Paragraph},
};

pub const ORANGE: Color = Color::Rgb(224, 128, 48);
pub const CYAN: Color = Color::Rgb(50, 140, 140);
pub const DIM: Color = Color::Rgb(100, 120, 120);
pub const HINT: Color = Color::Rgb(140, 160, 160);
pub const DIVIDER: Color = Color::Rgb(40, 58, 58);
pub const RED: Color = Color::Rgb(200, 60, 60);
pub const GREEN: Color = Color::Rgb(60, 180, 100);
pub const WHITE: Color = Color::Rgb(220, 220, 220);
pub const SELECTED: Color = Color::Rgb(30, 58, 58);
pub const COMMAND_BG: Color = Color::Rgb(80, 40, 10);
pub const COMMAND_FG: Color = Color::Rgb(160, 100, 40);
pub const MPV: Color = Color::Rgb(80, 120, 180);
pub const MPV_BG: Color = Color::Rgb(15, 20, 45);
pub const MPV_ROW_BG: Color = Color::Rgb(20, 30, 55);

pub fn frame_rows(area: Rect) -> [Rect; 5] {
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Min(1),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .split(area);
    [rows[0], rows[1], rows[2], rows[3], rows[4]]
}

pub fn brand() -> Span<'static> {
    Span::styled("[PaneBot]", Style::default().fg(ORANGE))
}

pub fn separator() -> Span<'static> {
    Span::styled(" :: ", Style::default().fg(DIM))
}

pub fn divider(frame: &mut Frame<'_>, area: Rect) {
    frame.render_widget(
        Paragraph::new(Span::styled(
            "-".repeat(area.width as usize),
            Style::default().fg(DIVIDER),
        )),
        area,
    );
}

pub fn list_offset(selected: usize, count: usize, height: usize) -> usize {
    selected
        .saturating_sub(height / 2)
        .min(count.saturating_sub(height))
}

pub fn selectable_list(frame: &mut Frame<'_>, area: Rect, rows: Vec<Line<'_>>, selected: usize) {
    selectable_list_color(frame, area, rows, selected, SELECTED);
}

pub fn selectable_list_color(
    frame: &mut Frame<'_>,
    area: Rect,
    rows: Vec<Line<'_>>,
    selected: usize,
    selected_color: Color,
) {
    let height = area.height as usize;
    let offset = list_offset(selected, rows.len(), height);
    let items = rows
        .into_iter()
        .enumerate()
        .skip(offset)
        .take(height)
        .map(|(index, row)| {
            let item = ListItem::new(row);
            if index == selected {
                item.style(Style::default().bg(selected_color))
            } else {
                item
            }
        })
        .collect::<Vec<_>>();
    frame.render_widget(List::new(items), area);
}

pub fn selected_label(value: String, selected: bool) -> Span<'static> {
    Span::styled(
        value,
        Style::default()
            .fg(if selected { WHITE } else { HINT })
            .add_modifier(if selected {
                Modifier::BOLD
            } else {
                Modifier::empty()
            }),
    )
}
