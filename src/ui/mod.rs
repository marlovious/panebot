mod primitives;

use ratatui::{
    Frame,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
};

use crate::{
    app::{
        App, ChannelPrompt, ChannelStage, CreateMode, ManagementPrompt, ManagementStage,
        NewTargetPrompt, NewTargetStage, TargetAction, View,
    },
    command::Mode,
    service::Endpoint,
};
use primitives::*;

pub fn render(frame: &mut Frame<'_>, app: &App) {
    if let Some(details) = &app.details {
        render_details(frame, app, details);
        return;
    }
    match &app.view {
        View::Picker => render_picker(frame, app),
        View::Connecting { endpoint, error } => {
            render_connecting(frame, endpoint, error.as_deref())
        }
        View::Connected { endpoint, snapshot } => render_dashboard(frame, app, endpoint, snapshot),
    }
}

fn render_picker(frame: &mut Frame<'_>, app: &App) {
    let rows = frame_rows(frame.size());
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            brand(),
            separator(),
            Span::styled("Select node to connect", Style::default().fg(HINT)),
        ])),
        rows[0],
    );
    divider(frame, rows[1]);
    let entries = app
        .endpoints
        .iter()
        .enumerate()
        .map(|(index, endpoint)| {
            let selected = index == app.picker_selected;
            Line::from(vec![
                Span::styled(
                    if selected { ">> " } else { "   " },
                    Style::default().fg(ORANGE),
                ),
                selected_label(endpoint.label.clone(), selected),
                separator(),
                Span::styled(endpoint.url.clone(), Style::default().fg(DIM)),
            ])
        })
        .collect();
    selectable_list(frame, rows[2], entries, app.picker_selected);
    divider(frame, rows[3]);
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("[j/k] Select", Style::default().fg(DIM)),
            Span::styled(" :: ", Style::default().fg(ORANGE)),
            Span::styled("[Enter] Connect", Style::default().fg(DIM)),
            Span::styled(" :: ", Style::default().fg(ORANGE)),
            Span::styled("[q] Quit", Style::default().fg(DIM)),
        ])),
        rows[4],
    );
}

fn render_connecting(frame: &mut Frame<'_>, endpoint: &Endpoint, error: Option<&str>) {
    let mut spans = vec![
        brand(),
        separator(),
        Span::styled(
            format!("Connecting to {}", endpoint.label),
            Style::default().fg(HINT),
        ),
        separator(),
        Span::styled(endpoint.url.clone(), Style::default().fg(DIM)),
    ];
    if let Some(error) = error {
        spans.push(separator());
        spans.push(Span::styled(error.to_owned(), Style::default().fg(RED)));
        spans.push(separator());
        spans.push(Span::styled("retrying", Style::default().fg(HINT)));
    }
    spans.push(separator());
    spans.push(Span::styled("[q] Back", Style::default().fg(DIM)));
    frame.render_widget(Paragraph::new(Line::from(spans)), frame.size());
}

fn render_dashboard(
    frame: &mut Frame<'_>,
    app: &App,
    endpoint: &Endpoint,
    snapshot: &mpv_targets::Snapshot,
) {
    let mode_footer = app.input_mode != Mode::Normal;
    let rows = frame_rows(frame.size());
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            brand(),
            separator(),
            Span::styled(
                format!("[{}]", snapshot.node_id),
                Style::default().fg(ORANGE),
            ),
            separator(),
            Span::styled(endpoint.url.clone(), Style::default().fg(DIM)),
            separator(),
            Span::styled("Targets:", Style::default().fg(DIM)),
            Span::styled(
                format!(
                    " {}/{}",
                    snapshot.health.targets_online, snapshot.health.targets_configured
                ),
                Style::default().fg(CYAN),
            ),
        ])),
        rows[0],
    );
    divider(frame, rows[1]);

    if let Some(prompt) = &app.new_target_prompt {
        render_new_target_prompt(frame, rows[2], prompt);
    } else if let Some(prompt) = &app.channel_prompt {
        render_channel_prompt(frame, rows[2], prompt);
    } else if let Some(prompt) = &app.management_prompt {
        render_management_prompt(frame, rows[2], prompt);
    } else {
        let name_width = snapshot
            .targets
            .iter()
            .map(|target| target.name.len() + 2)
            .max()
            .unwrap_or(8);
        let target_rows = snapshot
            .targets
            .iter()
            .enumerate()
            .map(|(index, target)| {
                if index == app.target_selected && app.action_picker.is_some() {
                    return Line::default();
                }
                target_row(
                    target,
                    name_width,
                    index == app.target_selected,
                    if index == app.target_selected {
                        app.input_mode
                    } else {
                        Mode::Normal
                    },
                )
            })
            .collect();
        selectable_list_color(
            frame,
            rows[2],
            target_rows,
            app.target_selected,
            if app.input_mode == Mode::Passthrough {
                MPV_ROW_BG
            } else {
                SELECTED
            },
        );
        if let Some(picker) = &app.action_picker {
            let offset = list_offset(
                app.target_selected,
                snapshot.targets.len(),
                rows[2].height as usize,
            );
            let y = app.target_selected.saturating_sub(offset) as u16;
            if y < rows[2].height {
                let area = ratatui::layout::Rect::new(rows[2].x, rows[2].y + y, rows[2].width, 1);
                let prefix = Line::from(vec![
                    Span::styled(":: ", Style::default().fg(ORANGE)),
                    Span::styled(
                        format!(
                            "{:<name_width$}",
                            format!("\"{}\"", picker.target.to_uppercase())
                        ),
                        Style::default().fg(WHITE).add_modifier(Modifier::BOLD),
                    ),
                    separator(),
                ]);
                let action_width = picker
                    .actions
                    .iter()
                    .map(|action| action.label().len())
                    .max()
                    .unwrap_or(0);
                let prefix_width = prefix
                    .width()
                    .min((area.width as usize).saturating_sub(action_width))
                    as u16;
                let style = Style::default().bg(SELECTED);
                frame.render_widget(
                    Paragraph::new(prefix).style(style),
                    ratatui::layout::Rect::new(area.x, area.y, prefix_width, 1),
                );
                let actions_area = ratatui::layout::Rect::new(
                    area.x + prefix_width,
                    area.y,
                    area.width - prefix_width,
                    1,
                );
                let mut line = Line::default();
                let mut selected_start = 0;
                let mut selected_end = 0;
                for (index, action) in picker.actions.iter().enumerate() {
                    if index > 0 {
                        line.spans.push(separator());
                    }
                    let selected = index == picker.selected;
                    if selected {
                        selected_start = line.width();
                        selected_end = selected_start + action.label().len();
                    }
                    let mut label = selected_label(action.label().to_owned(), selected);
                    if selected {
                        label.style = label.style.bg(SELECTED);
                    }
                    line.spans.push(label);
                }

                let offset = selected_end
                    .saturating_sub(actions_area.width as usize)
                    .min(selected_start);
                frame.render_widget(
                    Paragraph::new(line).style(style).scroll((0, offset as u16)),
                    actions_area,
                );
            }
        }
    }

    divider(frame, rows[3]);
    if let Some(status) = &app.status {
        render_status(frame, rows[4], status);
    } else if mode_footer {
        render_mode_footer(frame, rows[4], app.input_mode);
    } else {
        frame.render_widget(Paragraph::new(dashboard_footer(app)), rows[4]);
    }
}

fn dashboard_footer(app: &App) -> Line<'static> {
    if app.action_picker.is_some() {
        return Line::from(Span::styled(
            "[h/l] Select :: [Enter] Perform :: [Esc] Back",
            Style::default().fg(DIM),
        ));
    }
    if let Some(p) = &app.management_prompt {
        return Line::from(Span::styled(
            if p.busy {
                "Please wait"
            } else if p.stage == ManagementStage::Confirming {
                "[y] Confirm :: [n/Enter/Esc] Cancel"
            } else if p.stage == ManagementStage::ChoosingMode {
                "[j/k] Select :: [Enter] Continue :: [Esc] Back"
            } else {
                "[Enter] Continue :: [Esc] Back"
            },
            Style::default().fg(DIM),
        ));
    }
    if let Some(p) = &app.channel_prompt {
        if p.stage == ChannelStage::Picking
            && let Some(query) = &p.search.query
        {
            return channel_search_footer(query, &p.channels, p.selected);
        }
        return Line::from(Span::styled(
            if p.busy || p.stage == ChannelStage::Loading {
                "Please wait"
            } else if p.stage == ChannelStage::Restart {
                "[y] Restart Now :: [n/Enter] Set Without Restart :: [Esc] Back"
            } else {
                "[/] Search :: [j/k] Select :: [Enter] Set :: [Esc] Back"
            },
            Style::default().fg(DIM),
        ));
    }
    if let Some(p) = &app.new_target_prompt {
        if p.stage == NewTargetStage::ChannelPicker
            && let Some(query) = &p.search.query
        {
            return channel_search_footer(query, &p.channels, p.selected);
        }
        return Line::from(Span::styled(
            match p.stage {
                NewTargetStage::ChannelPicker => {
                    "[/] Search :: [j/k] Select :: [Enter] Continue :: [Esc] Back"
                }
                NewTargetStage::ChoosingMode => "[j/k] Select :: [Enter] Continue :: [Esc] Back",
                NewTargetStage::LoadingChannels => "Please wait",
                NewTargetStage::Confirming => "[y] Confirm :: [n/Enter/Esc] Cancel",
                _ => "[Enter] Continue :: [Esc] Back",
            },
            Style::default().fg(DIM),
        ));
    }
    Line::from(Span::styled(
        "[Enter] Target Actions :: [Tab] Send Command :: [S] Solo :: [P] Play/Pause All :: [I] Identify :: [N] New Target :: [l/→] Details :: [C] Nodes :: [q] Quit",
        Style::default().fg(DIM),
    ))
}

fn channel_search_footer(query: &str, channels: &[String], selected: usize) -> Line<'static> {
    Line::from(Span::styled(
        crate::search::footer(query, &crate::search::matches(query, channels), selected),
        Style::default().fg(WHITE),
    ))
}

fn render_channel_prompt(
    frame: &mut Frame<'_>,
    area: ratatui::layout::Rect,
    prompt: &ChannelPrompt,
) {
    let areas = ratatui::layout::Layout::vertical([
        ratatui::layout::Constraint::Length(1),
        ratatui::layout::Constraint::Length(u16::from(prompt.stage != ChannelStage::Picking)),
        ratatui::layout::Constraint::Min(1),
    ])
    .split(area);
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("Set Channel", Style::default().fg(HINT)),
            separator(),
            Span::styled(
                format!("\"{}\"", prompt.target.to_uppercase()),
                Style::default().fg(WHITE),
            ),
        ])),
        areas[0],
    );
    let question = match prompt.stage {
        ChannelStage::Loading => "Loading channels…".to_owned(),
        ChannelStage::Picking => String::new(),
        ChannelStage::Restart => "Restart Now? [y/N]".to_owned(),
    };
    frame.render_widget(Paragraph::new(question), areas[1]);
    if prompt.stage == ChannelStage::Picking {
        let matches = crate::search::matches(
            prompt.search.query.as_deref().unwrap_or(""),
            &prompt.channels,
        );
        selectable_list(
            frame,
            areas[2],
            prompt
                .channels
                .iter()
                .enumerate()
                .map(|(index, channel)| {
                    Line::from(vec![
                        Span::styled(
                            if index == prompt.selected {
                                ":: "
                            } else {
                                "   "
                            },
                            Style::default().fg(ORANGE),
                        ),
                        selected_label(
                            format!(
                                "{}. {}",
                                index + 1,
                                mpv_targets::channel_display_name(channel)
                            ),
                            index == prompt.selected || matches.contains(&index),
                        ),
                    ])
                })
                .collect(),
            prompt.selected,
        );
    } else if let Some(error) = &prompt.error {
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled("Error", Style::default().fg(RED)),
                separator(),
                Span::styled(error.clone(), Style::default().fg(RED)),
            ])),
            areas[2],
        );
    }
}

fn render_details(frame: &mut Frame<'_>, app: &App, details: &crate::details::Details) {
    let rows = frame_rows(frame.size());
    let Some(target) = app.selected_target() else {
        return;
    };
    let status = mpv_targets::target_status(target);
    let status_color = match status {
        mpv_targets::TargetStatus::Offline => RED,
        mpv_targets::TargetStatus::Playing => GREEN,
        mpv_targets::TargetStatus::Paused => HINT,
        _ => DIM,
    };
    let volume = if target.state.muted == Some(true) {
        "Vol:Mute".into()
    } else {
        target
            .state
            .volume
            .map_or("Vol:  ?".into(), |v| format!("Vol:{v:3.0}"))
    };
    let loop_on = crate::details::loop_enabled(target.state.loop_file.as_deref());
    let repeat_on = crate::details::loop_enabled(target.state.loop_playlist.as_deref());
    let mut head = vec![
        brand(),
        separator(),
        Span::styled(
            format!("\"{}\"", target.name.to_uppercase()),
            Style::default().fg(WHITE).add_modifier(Modifier::BOLD),
        ),
        separator(),
        Span::styled(
            format!("[{:^8}]", status.label()),
            Style::default().fg(status_color),
        ),
        separator(),
        Span::styled(format!("[{volume:<8}]"), Style::default().fg(CYAN)),
        separator(),
        Span::styled(
            format!("[Loop:{}]", if loop_on { "On " } else { "Off" }),
            Style::default().fg(if loop_on { GREEN } else { DIM }),
        ),
        separator(),
        Span::styled(
            format!("[Repeat:{}]", if repeat_on { "On " } else { "Off" }),
            Style::default().fg(if repeat_on { GREEN } else { DIM }),
        ),
    ];
    if let (Some(pos), Some(dur)) = (target.state.position, target.state.duration)
        && dur.is_finite()
        && pos.is_finite()
    {
        head.push(separator());
        head.push(Span::styled(
            format!("[{} / -{}]", clock(pos), clock((dur - pos).max(0.0))),
            Style::default().fg(DIM),
        ));
    }
    head.push(separator());
    head.push(Span::styled(
        target
            .state
            .title
            .clone()
            .unwrap_or_else(|| format!("{} items", details.items.len())),
        Style::default().fg(CYAN),
    ));
    frame.render_widget(Paragraph::new(Line::from(head)), rows[0]);
    divider(frame, rows[1]);
    let height = rows[2].height as usize;
    let offset = details
        .selected
        .saturating_sub(height / 2)
        .min(details.items.len().saturating_sub(height));
    let matches = details.matches();
    let lines: Vec<Line<'_>> = details
        .items
        .iter()
        .enumerate()
        .skip(offset)
        .take(height)
        .map(|(i, item)| {
            let selected = i == details.selected;
            let marked = details.marks.contains(&item.id);
            let marker = if item.current {
                Span::styled("* ", Style::default().fg(GREEN))
            } else if marked {
                Span::styled("• ", Style::default().fg(ORANGE))
            } else {
                Span::raw("  ")
            };
            let color = if matches.contains(&i) || marked {
                WHITE
            } else if item.current {
                CYAN
            } else {
                HINT
            };
            Line::from(vec![
                marker,
                Span::styled(
                    format!("{i:<4}"),
                    Style::default().fg(if item.current || marked { ORANGE } else { DIM }),
                ),
                Span::styled(" :: ", Style::default().fg(DIM)),
                Span::styled(
                    crate::details::display_item(item),
                    Style::default().fg(color),
                ),
            ])
            .style(if selected {
                Style::default().bg(match app.input_mode {
                    Mode::Command => COMMAND_BG,
                    Mode::Passthrough => MPV_ROW_BG,
                    Mode::Normal => SELECTED,
                })
            } else {
                Style::default()
            })
        })
        .collect();
    frame.render_widget(ratatui::widgets::List::new(lines), rows[2]);
    divider(frame, rows[3]);
    if app.input_mode != Mode::Normal {
        if let Some(status) = &app.status {
            render_status(frame, rows[4], status);
        } else {
            render_mode_footer(frame, rows[4], app.input_mode);
        }
    } else {
        frame.render_widget(
            Paragraph::new(details_footer(details, target, &matches)),
            rows[4],
        );
    }
}

fn clock(seconds: f64) -> String {
    let s = seconds.max(0.0) as u64;
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, (s % 3600) / 60, s % 60)
    } else {
        format!("{:02}:{:02}", s / 60, s % 60)
    }
}
fn tail(value: &str, width: usize) -> String {
    let chars = value.chars().collect::<Vec<_>>();
    chars[chars.len().saturating_sub(width)..].iter().collect()
}
fn details_footer(
    d: &crate::details::Details,
    target: &mpv_targets::TargetRecord,
    matches: &[usize],
) -> Line<'static> {
    use crate::details::{AddStage, Mode};
    if matches!(d.mode, Mode::Normal) {
        if let Some((text, error)) = &d.status {
            return Line::from(vec![
                Span::styled(
                    if *error {
                        "Error"
                    } else if d.is_busy() {
                        "Working"
                    } else {
                        "Done"
                    },
                    Style::default().fg(if *error { RED } else { HINT }),
                ),
                separator(),
                Span::styled(
                    text.clone(),
                    Style::default().fg(if *error { RED } else { WHITE }),
                ),
            ]);
        }
        if target.state.last_end_reason == Some(mpv_targets::MediaEndReason::Error) {
            return Line::from(Span::styled(
                format!(
                    "Playback Error :: {}",
                    target
                        .state
                        .last_end_error
                        .as_deref()
                        .unwrap_or("unknown error")
                ),
                Style::default().fg(RED).add_modifier(Modifier::BOLD),
            ));
        }
    }
    match &d.mode {
        Mode::Normal => Line::from(Span::styled(
            "[/] Search :: [Space] Mark :: [Enter] Play/Next :: [R] Remove :: [M] Move :: [C] Crop :: [A] Add :: [S] Save :: [G] Goto :: [L/P] Loop/Repeat :: [Z] Shuffle :: [h] Back",
            Style::default().fg(DIM),
        )),
        Mode::Item => Line::from(vec![
            Span::styled(
                "Play Now [Enter]",
                Style::default().fg(CYAN).add_modifier(Modifier::BOLD),
            ),
            Span::styled(" :: ", Style::default().fg(ORANGE)),
            Span::styled(
                "Move To Next [n]",
                Style::default().fg(CYAN).add_modifier(Modifier::BOLD),
            ),
            Span::styled(" :: Cancel [Esc]", Style::default().fg(DIM)),
        ]),
        Mode::Search { query, .. } => {
            let m = matches;
            Line::from(Span::styled(
                crate::search::footer(query, m, d.selected),
                Style::default().fg(WHITE),
            ))
        }
        Mode::Goto { input, error, .. } => Line::from(Span::styled(
            error
                .clone()
                .unwrap_or_else(|| format!("Go to index: {}  [Enter] Go  [Esc] Cancel", input)),
            Style::default().fg(if error.is_some() { RED } else { HINT }),
        )),
        Mode::Crop => Line::from(Span::styled(
            "Crop Before Current [b] :: Crop After Current [a] :: Cancel [Esc]",
            Style::default().fg(HINT),
        )),
        Mode::Add { input, stage } => Line::from(Span::styled(
            match stage {
                AddStage::Input => format!("Add: {}", tail(input, 100)),
                AddStage::Choose => {
                    "Append to Queue [a] :: Replace Current Queue [r] :: Cancel [Esc]".into()
                }
                AddStage::ConfirmReplace => "Replace current queue? [y/N]".into(),
            },
            Style::default().fg(HINT),
        )),
        Mode::Save { input, overwrite } => Line::from(Span::styled(
            if *overwrite {
                format!("Overwrite \"{input}\"? [y/N]")
            } else {
                format!("Save Queue as Channel: {}", tail(input, 90))
            },
            Style::default().fg(HINT),
        )),
    }
}

fn render_new_target_prompt(
    frame: &mut Frame<'_>,
    area: ratatui::layout::Rect,
    prompt: &NewTargetPrompt,
) {
    let (header_area, line_area, body_area) = if prompt.stage == NewTargetStage::ChoosingMode {
        let areas = ratatui::layout::Layout::vertical([
            ratatui::layout::Constraint::Length(1),
            ratatui::layout::Constraint::Min(1),
        ])
        .split(area);
        (areas[0], ratatui::layout::Rect::default(), areas[1])
    } else {
        let areas = ratatui::layout::Layout::vertical([
            ratatui::layout::Constraint::Length(1),
            ratatui::layout::Constraint::Length(1),
            ratatui::layout::Constraint::Min(1),
        ])
        .split(area);
        (areas[0], areas[1], areas[2])
    };
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("New Target", Style::default().fg(HINT)),
            separator(),
            Span::styled(
                format!("[{}]", prompt.input.to_uppercase()),
                Style::default().fg(WHITE),
            ),
        ])),
        header_area,
    );

    let line = match prompt.stage {
        NewTargetStage::Name => Line::from(vec![
            Span::styled("New Target Name: ", Style::default().fg(HINT)),
            Span::styled(
                prompt.input.clone(),
                Style::default().fg(WHITE).add_modifier(Modifier::BOLD),
            ),
        ]),
        NewTargetStage::ChannelQuestion => question("Set Startup Channel? [y/N]"),
        NewTargetStage::LoadingChannels => question("Set Startup Channel? loading…"),
        NewTargetStage::ChannelPicker => question("Set Startup Channel"),
        NewTargetStage::PlayQuestion => question("Play On Start? [y/N]"),
        NewTargetStage::MuteQuestion => question("Mute On Start? [Y/n]"),
        NewTargetStage::ChoosingMode => Line::default(),
        NewTargetStage::Confirming => question(&format!(
            "Create \"{}\" ? - [y/N]",
            prompt.input.to_uppercase()
        )),
    };
    if prompt.stage != NewTargetStage::ChoosingMode {
        frame.render_widget(Paragraph::new(line), line_area);
    }

    if prompt.stage == NewTargetStage::ChannelPicker {
        let matches = crate::search::matches(
            prompt.search.query.as_deref().unwrap_or(""),
            &prompt.channels,
        );
        selectable_list(
            frame,
            body_area,
            prompt
                .channels
                .iter()
                .enumerate()
                .map(|(index, channel)| {
                    Line::from(vec![
                        Span::styled(
                            if index == prompt.selected {
                                ":: "
                            } else {
                                "   "
                            },
                            Style::default().fg(ORANGE),
                        ),
                        selected_label(
                            format!(
                                "{}. {}",
                                index + 1,
                                mpv_targets::channel_display_name(channel)
                            ),
                            index == prompt.selected || matches.contains(&index),
                        ),
                    ])
                })
                .collect(),
            prompt.selected,
        );
    } else if prompt.stage == NewTargetStage::ChoosingMode {
        selectable_list(
            frame,
            body_area,
            CreateMode::ALL
                .iter()
                .enumerate()
                .map(|(index, mode)| {
                    Line::from(vec![
                        Span::styled(
                            if index == prompt.mode_selected {
                                ":: "
                            } else {
                                "   "
                            },
                            Style::default().fg(ORANGE),
                        ),
                        selected_label(mode.label().to_owned(), index == prompt.mode_selected),
                    ])
                })
                .collect(),
            prompt.mode_selected,
        );
    } else if let Some(error) = &prompt.error {
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled("Error", Style::default().fg(RED)),
                separator(),
                Span::styled(error.clone(), Style::default().fg(RED)),
            ])),
            body_area,
        );
    }
}

fn question(text: &str) -> Line<'static> {
    Line::from(Span::styled(text.to_owned(), Style::default().fg(HINT)))
}

fn render_management_prompt(
    frame: &mut Frame<'_>,
    area: ratatui::layout::Rect,
    prompt: &ManagementPrompt,
) {
    let areas = ratatui::layout::Layout::vertical([
        ratatui::layout::Constraint::Length(1),
        ratatui::layout::Constraint::Length(1),
        ratatui::layout::Constraint::Min(1),
    ])
    .split(area);
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(prompt.action.label(), Style::default().fg(HINT)),
            separator(),
            Span::styled(
                format!("\"{}\"", prompt.target.to_uppercase()),
                Style::default().fg(WHITE),
            ),
        ])),
        areas[0],
    );

    let line = if prompt.busy {
        Line::from(vec![
            Span::styled("Working", Style::default().fg(HINT)),
            separator(),
            Span::styled(prompt.action.label(), Style::default().fg(WHITE)),
        ])
    } else if prompt.stage == ManagementStage::Confirming {
        confirmation_line(prompt)
    } else if prompt.stage == ManagementStage::ChoosingMode {
        Line::from(vec![
            Span::styled(
                format!("\"{}\"", prompt.input.to_uppercase()),
                Style::default().fg(CYAN),
            ),
            Span::styled(" from ", Style::default().fg(HINT)),
            Span::styled(
                format!("\"{}\"", prompt.target.to_uppercase()),
                Style::default().fg(WHITE),
            ),
        ])
    } else {
        Line::from(vec![
            Span::styled("New name: ", Style::default().fg(HINT)),
            Span::styled(
                prompt.input.clone(),
                Style::default().fg(WHITE).add_modifier(Modifier::BOLD),
            ),
        ])
    };
    frame.render_widget(Paragraph::new(line), areas[1]);

    if prompt.stage == ManagementStage::ChoosingMode {
        selectable_list(
            frame,
            areas[2],
            CreateMode::ALL
                .iter()
                .enumerate()
                .map(|(index, mode)| {
                    Line::from(vec![
                        Span::styled(
                            if index == prompt.selected {
                                ":: "
                            } else {
                                "   "
                            },
                            Style::default().fg(ORANGE),
                        ),
                        selected_label(mode.label().to_owned(), index == prompt.selected),
                    ])
                })
                .collect(),
            prompt.selected,
        );
    } else if let Some(error) = &prompt.error {
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled("Error", Style::default().fg(RED)),
                separator(),
                Span::styled(error.clone(), Style::default().fg(RED)),
            ])),
            areas[2],
        );
    }
}

fn confirmation_line(prompt: &ManagementPrompt) -> Line<'static> {
    let text = match prompt.action {
        TargetAction::Rename => format!(
            "Rename \"{}\" to \"{}\"? [y/N]",
            prompt.target.to_uppercase(),
            prompt.input.to_uppercase()
        ),
        TargetAction::Remove => format!(
            "Remove \"{}\" and its target files? [y/N]",
            prompt.target.to_uppercase()
        ),
        TargetAction::Duplicate => format!(
            "Create \"{}\" from \"{}\" ? - [y/N]",
            prompt.input.to_uppercase(),
            prompt.target.to_uppercase()
        ),
        TargetAction::Lifecycle(_) | TargetAction::SetChannel => unreachable!(),
    };
    Line::from(Span::styled(text, Style::default().fg(HINT)))
}

fn render_status(
    frame: &mut Frame<'_>,
    area: ratatui::layout::Rect,
    status: &crate::app::StatusLine,
) {
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                if status.error {
                    "Error"
                } else if status.busy {
                    "Working"
                } else {
                    "Done"
                },
                Style::default().fg(if status.error { RED } else { HINT }),
            ),
            separator(),
            Span::styled(
                status.text.clone(),
                Style::default().fg(if status.error { RED } else { WHITE }),
            ),
        ])),
        area,
    );
}

fn render_mode_footer(frame: &mut Frame<'_>, area: ratatui::layout::Rect, mode: Mode) {
    let (line, style) = match mode {
        Mode::Command => (
            Line::from(vec![
                Span::styled("[CMD]", Style::default().add_modifier(Modifier::BOLD)),
                separator(),
                Span::raw(
                    "[Space] Pause [m] Mute [h/l] ±5s [j/k] ±60s [9/0] Vol [a] Audio [s] Subs [f] Full [\\] Adv [Tab] Close",
                ),
            ]),
            Style::default().bg(COMMAND_BG).fg(COMMAND_FG),
        ),
        Mode::Passthrough => (
            Line::from(vec![
                Span::styled(
                    "[MPV]",
                    Style::default().fg(MPV).add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    " passthrough — all keys forward to mpv  ",
                    Style::default().fg(DIM),
                ),
                Span::styled("[\\]", Style::default().fg(MPV)),
                Span::styled(" exit", Style::default().fg(DIM)),
            ]),
            Style::default().bg(MPV_BG).fg(MPV),
        ),
        Mode::Normal => return,
    };
    frame.render_widget(Paragraph::new(line).style(style), area);
}

fn target_row(
    target: &mpv_targets::TargetRecord,
    name_width: usize,
    selected: bool,
    mode: Mode,
) -> Line<'static> {
    let status = mpv_targets::target_status(target);
    let status_color = match status {
        mpv_targets::TargetStatus::Disabled
        | mpv_targets::TargetStatus::Stopped
        | mpv_targets::TargetStatus::Idle => DIM,
        mpv_targets::TargetStatus::Offline => RED,
        mpv_targets::TargetStatus::Paused => HINT,
        mpv_targets::TargetStatus::Playing => GREEN,
    };
    let (volume, volume_color) = if !target.online {
        ("Offline".to_owned(), RED)
    } else if target.state.muted == Some(true) {
        ("Vol:Mute".to_owned(), DIM)
    } else if let Some(volume) = target.state.volume {
        (format!("Vol:{volume:3.0}"), CYAN)
    } else {
        ("Vol:  ?".to_owned(), DIM)
    };
    let title = target
        .state
        .title
        .as_deref()
        .filter(|title| !title.is_empty())
        .unwrap_or("-")
        .to_owned();

    let mut spans = vec![
        Span::styled(
            if selected { ":: " } else { "   " },
            Style::default().fg(ORANGE),
        ),
        Span::styled(
            format!(
                "{:<name_width$}",
                format!("\"{}\"", target.name.to_uppercase())
            ),
            Style::default().fg(WHITE).add_modifier(if selected {
                Modifier::BOLD
            } else {
                Modifier::empty()
            }),
        ),
        separator(),
        Span::styled(
            format!("[{:^8}]", status.label()),
            Style::default().fg(status_color),
        ),
        separator(),
        Span::styled(format!("[{volume:<8}]"), Style::default().fg(volume_color)),
        separator(),
        Span::styled(title, Style::default().fg(CYAN)),
    ];
    if selected && mode == Mode::Command {
        spans.push(Span::styled(" [CMD]", Style::default().fg(ORANGE)));
    } else if selected && mode == Mode::Passthrough {
        spans.push(Span::styled(" [MPV]", Style::default().fg(MPV)));
    }
    Line::from(spans)
}

#[cfg(test)]
mod tests {
    use super::*;
    use mpv_targets::{Health, HealthState, Snapshot, TargetRecord, TargetState};
    use ratatui::{Terminal, backend::TestBackend};

    fn rendered_lines(app: &App, width: u16, height: u16) -> Vec<String> {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal.draw(|frame| render(frame, app)).unwrap();
        let buffer = terminal.backend().buffer();
        (0..height)
            .map(|y| {
                (0..width)
                    .map(|x| buffer.get(x, y).symbol())
                    .collect::<String>()
            })
            .collect()
    }

    fn one_target_app() -> App {
        let mut app = App::new(vec![Endpoint::new("local", "wss://127.0.0.1:9876")]);
        app.connected(
            app.endpoints[0].clone(),
            Snapshot {
                protocol_version: 1,
                node_id: "fez".into(),
                health: Health {
                    state: HealthState::Ready,
                    targets_configured: 1,
                    targets_expected_online: 1,
                    targets_online: 1,
                },
                targets: vec![TargetRecord {
                    name: "daily".into(),
                    disabled: false,
                    stopped: false,
                    online: true,
                    channel: None,
                    state: TargetState::default(),
                    revision: 1,
                }],
            },
        );
        app
    }

    #[test]
    fn picker_has_the_approved_order_and_complete_frame() {
        let app = App::new(vec![
            Endpoint::new("local", "wss://127.0.0.1:9876"),
            Endpoint::new("fez", "wss://fez.test:9876"),
        ]);
        assert_eq!(
            rendered_lines(&app, 64, 8),
            vec![
                "[PaneBot] :: Select node to connect                             ",
                "----------------------------------------------------------------",
                ">> local :: wss://127.0.0.1:9876                                ",
                "   fez :: wss://fez.test:9876                                   ",
                "                                                                ",
                "                                                                ",
                "----------------------------------------------------------------",
                "[j/k] Select :: [Enter] Connect :: [q] Quit                     ",
            ]
        );
    }

    #[test]
    fn connected_view_uses_the_authoritative_node_identity() {
        let mut app = App::new(vec![Endpoint::new("local", "wss://127.0.0.1:9876")]);
        app.connected(
            app.endpoints[0].clone(),
            Snapshot {
                protocol_version: 1,
                node_id: "fez".into(),
                health: Health {
                    state: HealthState::Ready,
                    targets_configured: 4,
                    targets_expected_online: 4,
                    targets_online: 4,
                },
                targets: vec![],
            },
        );
        assert!(rendered_lines(&app, 100, 5)[0].contains("[fez]"));
    }

    #[test]
    fn dashboard_renders_complete_targets_native_rows_with_idle_shortcuts() {
        let mut app = App::new(vec![Endpoint::new("local", "wss://127.0.0.1:9876")]);
        app.connected(
            app.endpoints[0].clone(),
            Snapshot {
                protocol_version: 1,
                node_id: "fezzik".into(),
                health: Health {
                    state: HealthState::Degraded,
                    targets_configured: 2,
                    targets_expected_online: 2,
                    targets_online: 1,
                },
                targets: vec![
                    TargetRecord {
                        name: "daily".into(),
                        disabled: false,
                        stopped: false,
                        online: true,
                        channel: None,
                        state: TargetState {
                            paused: Some(true),
                            muted: Some(true),
                            title: Some("Boiler Room".into()),
                            ..TargetState::default()
                        },
                        revision: 1,
                    },
                    TargetRecord {
                        name: "podcasts".into(),
                        disabled: false,
                        stopped: false,
                        online: false,
                        channel: None,
                        state: TargetState::default(),
                        revision: 2,
                    },
                ],
            },
        );
        let lines = rendered_lines(&app, 160, 7);
        assert_eq!(
            lines[0].trim_end(),
            "[PaneBot] :: [fezzik] :: wss://127.0.0.1:9876 :: Targets: 1/2"
        );
        assert_eq!(lines[1], "-".repeat(160));
        assert_eq!(
            lines[2].trim_end(),
            ":: \"DAILY\"    :: [ Paused ] :: [Vol:Mute] :: Boiler Room"
        );
        assert_eq!(
            lines[3].trim_end(),
            "   \"PODCASTS\" :: [Offline ] :: [Offline ] :: -"
        );
        assert!(lines[4].trim().is_empty());
        assert_eq!(lines[5], "-".repeat(160));
        assert_eq!(
            lines[6].trim_end(),
            "[Enter] Target Actions :: [Tab] Send Command :: [S] Solo :: [P] Play/Pause All :: [I] Identify :: [N] New Target :: [l/→] Details :: [C] Nodes :: [q] Quit"
        );
    }

    #[test]
    fn lifecycle_actions_are_temporary_and_contextual() {
        let mut app = App::new(vec![Endpoint::new("local", "wss://127.0.0.1:9876")]);
        app.connected(
            app.endpoints[0].clone(),
            Snapshot {
                protocol_version: 1,
                node_id: "fez".into(),
                health: Health {
                    state: HealthState::Idle,
                    targets_configured: 1,
                    targets_expected_online: 0,
                    targets_online: 0,
                },
                targets: vec![TargetRecord {
                    name: "daily".into(),
                    disabled: true,
                    stopped: false,
                    online: false,
                    channel: None,
                    state: TargetState::default(),
                    revision: 1,
                }],
            },
        );
        app.status = Some(crate::app::StatusLine {
            text: "Previous action".into(),
            error: false,
            busy: false,
        });
        app.handle_key(crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Enter,
            crossterm::event::KeyModifiers::NONE,
        ));
        assert_eq!(
            rendered_lines(&app, 72, 8),
            vec![
                "[PaneBot] :: [fez] :: wss://127.0.0.1:9876 :: Targets: 0/1              ",
                "------------------------------------------------------------------------",
                ":: \"DAILY\" :: Enable :: Enable & Start :: Set Channel :: Duplicate      ",
                "                                                                        ",
                "                                                                        ",
                "                                                                        ",
                "------------------------------------------------------------------------",
                "[h/l] Select :: [Enter] Perform :: [Esc] Back                           ",
            ]
        );
    }

    #[test]
    fn horizontal_actions_keep_the_selection_visible_in_a_narrow_terminal() {
        let mut app = one_target_app();
        if let View::Connected { snapshot, .. } = &mut app.view {
            let mut other = snapshot.targets[0].clone();
            other.name = "other".into();
            snapshot.targets.push(other);
        }
        let before = rendered_lines(&app, 80, 8);
        let key =
            |code| crossterm::event::KeyEvent::new(code, crossterm::event::KeyModifiers::NONE);
        app.handle_key(key(crossterm::event::KeyCode::Enter));
        for expected in ["Restart", "Stop", "Disable", "Set Channel", "Duplicate"] {
            let lines = rendered_lines(&app, 20, 8);
            assert!(lines[2].contains(expected), "{:?}", lines[2]);
            assert!(lines[3].contains("\"OTHER\""));
            app.handle_key(key(crossterm::event::KeyCode::Right));
        }
        app.handle_key(key(crossterm::event::KeyCode::Left));
        assert!(rendered_lines(&app, 20, 8)[2].contains("Set Channel"));
        app.handle_key(key(crossterm::event::KeyCode::Esc));
        assert_eq!(rendered_lines(&app, 80, 8), before);
    }

    #[test]
    fn channel_pickers_scroll_with_selection_and_resize() {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        let channels = (0..12)
            .map(|i| format!("channel{i}.m3u"))
            .collect::<Vec<_>>();
        for creating in [false, true] {
            let mut app = one_target_app();
            if creating {
                app.new_target_prompt = Some(NewTargetPrompt {
                    input: "new".into(),
                    stage: NewTargetStage::ChannelPicker,
                    channels: channels.clone(),
                    search: Default::default(),
                    channel: None,
                    selected: 0,
                    play_on_start: false,
                    mute_on_start: true,
                    mode_selected: 0,
                    error: None,
                    busy: false,
                });
            } else {
                app.channel_prompt = Some(ChannelPrompt {
                    target: "daily".into(),
                    channels: channels.clone(),
                    search: Default::default(),
                    selected: 0,
                    channel: None,
                    restart_available: true,
                    stage: ChannelStage::Picking,
                    error: None,
                    busy: false,
                });
            }
            let selected_row = if creating { 6 } else { 5 };
            let narrow_row = if creating { 5 } else { 4 };
            let first_row = if creating { 4 } else { 3 };
            for _ in 0..9 {
                app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
            }
            assert_eq!(
                rendered_lines(&app, 40, 10)[selected_row].trim_end(),
                ":: 10. channel9.m3u"
            );
            assert_eq!(
                rendered_lines(&app, 40, 8)[narrow_row].trim_end(),
                ":: 10. channel9.m3u"
            );
            let mut terminal = Terminal::new(TestBackend::new(40, 10)).unwrap();
            terminal.draw(|frame| render(frame, &app)).unwrap();
            assert_eq!(
                terminal.backend().buffer().get(0, selected_row as u16).bg,
                SELECTED
            );
            for _ in 0..9 {
                app.handle_key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));
            }
            assert_eq!(
                rendered_lines(&app, 40, 10)[first_row].trim_end(),
                ":: 1. channel0.m3u"
            );
            let press = |app: &mut App, code| {
                assert_eq!(
                    app.handle_key(KeyEvent::new(code, KeyModifiers::NONE)),
                    None
                );
            };
            press(&mut app, KeyCode::Tab);
            assert_eq!(app.input_mode, Mode::Command);
            assert!(rendered_lines(&app, 120, 10)[9].contains("[CMD]"));
            assert!(
                app.handle_key(KeyEvent::new(KeyCode::Char('S'), KeyModifiers::NONE))
                    .is_none()
            );
            press(&mut app, KeyCode::Tab);
            assert_eq!(
                rendered_lines(&app, 40, 10)[first_row].trim_end(),
                ":: 1. channel0.m3u"
            );
            press(&mut app, KeyCode::Char('/'));
            for c in "channel1".chars() {
                press(&mut app, KeyCode::Char(c));
            }
            let lines = rendered_lines(&app, 100, 10);
            assert_eq!(
                lines[9].trim_end(),
                "/channel1 :: Match 1/3 :: [[/]] Navigate [Enter] Select [Esc] Cancel"
            );
            press(&mut app, KeyCode::Char(']'));
            assert!(rendered_lines(&app, 40, 8)[narrow_row].contains("11. channel10.m3u"));
            press(&mut app, KeyCode::Char('['));
            press(&mut app, KeyCode::Esc);
            assert_eq!(
                rendered_lines(&app, 40, 10)[first_row].trim_end(),
                ":: 1. channel0.m3u"
            );
            press(&mut app, KeyCode::Char('/'));
            for c in "channel9".chars() {
                press(&mut app, KeyCode::Char(c));
            }
            press(&mut app, KeyCode::Enter); // Select only; never assign while searching.
            assert!(rendered_lines(&app, 40, 8)[narrow_row].contains("10. channel9.m3u"));
            press(&mut app, KeyCode::Char('/'));
            press(&mut app, KeyCode::Char('x'));
            assert!(rendered_lines(&app, 100, 10)[9].contains("No matches"));
            press(&mut app, KeyCode::Backspace);
            press(&mut app, KeyCode::Enter);
            assert!(rendered_lines(&app, 40, 8)[narrow_row].contains("10. channel9.m3u"));
        }
    }

    #[test]
    fn rename_prompt_keeps_confirmation_inside_its_footer() {
        let mut app = one_target_app();
        if let View::Connected { snapshot, .. } = &mut app.view {
            snapshot.targets[0].stopped = true;
            snapshot.targets[0].online = false;
        }
        app.handle_key(crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Enter,
            crossterm::event::KeyModifiers::NONE,
        ));
        app.handle_key(crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Down,
            crossterm::event::KeyModifiers::NONE,
        ));
        app.handle_key(crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Down,
            crossterm::event::KeyModifiers::NONE,
        ));
        app.handle_key(crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Enter,
            crossterm::event::KeyModifiers::NONE,
        ));
        for character in "movies".chars() {
            app.handle_key(crossterm::event::KeyEvent::new(
                crossterm::event::KeyCode::Char(character),
                crossterm::event::KeyModifiers::NONE,
            ));
        }
        app.handle_key(crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Enter,
            crossterm::event::KeyModifiers::NONE,
        ));

        assert_eq!(
            rendered_lines(&app, 72, 8),
            vec![
                "[PaneBot] :: [fez] :: wss://127.0.0.1:9876 :: Targets: 1/1              ",
                "------------------------------------------------------------------------",
                "Rename :: \"DAILY\"                                                       ",
                "Rename \"DAILY\" to \"MOVIES\"? [y/N]                                       ",
                "                                                                        ",
                "                                                                        ",
                "------------------------------------------------------------------------",
                "[y] Confirm :: [n/Enter/Esc] Cancel                                     ",
            ]
        );
    }

    #[test]
    fn duplicate_and_remove_have_distinct_confirmed_flows() {
        let mut app = one_target_app();
        if let View::Connected { snapshot, .. } = &mut app.view {
            snapshot.targets[0].stopped = true;
            snapshot.targets[0].online = false;
        }
        let key =
            |code| crossterm::event::KeyEvent::new(code, crossterm::event::KeyModifiers::NONE);

        app.handle_key(key(crossterm::event::KeyCode::Enter));
        for _ in 0..5 {
            app.handle_key(key(crossterm::event::KeyCode::Down));
        }
        app.handle_key(key(crossterm::event::KeyCode::Enter));
        for character in "daily-copy".chars() {
            app.handle_key(key(crossterm::event::KeyCode::Char(character)));
        }
        app.handle_key(key(crossterm::event::KeyCode::Enter));

        let lines = rendered_lines(&app, 80, 10);
        assert_eq!(lines[2].trim_end(), "Duplicate :: \"DAILY\"");
        assert_eq!(lines[3].trim_end(), "\"DAILY-COPY\" from \"DAILY\"");
        assert_eq!(lines[4].trim_end(), ":: Create: Disabled");
        assert_eq!(lines[5].trim_end(), "   Create: Enabled/Stopped");
        assert_eq!(lines[6].trim_end(), "   Create: Enabled/Start");
        assert_eq!(
            lines[9].trim_end(),
            "[j/k] Select :: [Enter] Continue :: [Esc] Back"
        );

        app.handle_key(key(crossterm::event::KeyCode::Enter));
        let lines = rendered_lines(&app, 80, 10);
        assert_eq!(
            lines[3].trim_end(),
            "Create \"DAILY-COPY\" from \"DAILY\" ? - [y/N]"
        );

        app.handle_key(key(crossterm::event::KeyCode::Esc));
        app.handle_key(key(crossterm::event::KeyCode::Up));
        app.handle_key(key(crossterm::event::KeyCode::Up));
        app.handle_key(key(crossterm::event::KeyCode::Enter));
        let lines = rendered_lines(&app, 80, 10);
        assert_eq!(lines[2].trim_end(), "Remove :: \"DAILY\"");
        assert_eq!(
            lines[3].trim_end(),
            "Remove \"DAILY\" and its target files? [y/N]"
        );
        assert_eq!(lines[9].trim_end(), "[y] Confirm :: [n/Enter/Esc] Cancel");
    }

    #[test]
    fn command_and_passthrough_use_production_mode_treatments() {
        let mut app = one_target_app();
        app.input_mode = Mode::Command;
        let lines = rendered_lines(&app, 110, 8);
        assert!(lines[2].contains("[CMD]"));
        assert!(lines[7].contains("[Space] Pause"));
        assert!(lines[7].contains("[a] Audio [s] Subs"));
        assert!(lines[7].contains("[\\] Adv [Tab] Close"));

        let mut terminal = Terminal::new(TestBackend::new(110, 8)).unwrap();
        terminal.draw(|frame| render(frame, &app)).unwrap();
        assert_eq!(terminal.backend().buffer().get(0, 7).bg, COMMAND_BG);

        app.input_mode = Mode::Passthrough;
        let lines = rendered_lines(&app, 110, 8);
        assert!(lines[2].contains("[MPV]"));
        assert!(lines[7].contains("all keys forward to mpv"));
        assert!(lines[7].contains("[\\] exit"));
        terminal.draw(|frame| render(frame, &app)).unwrap();
        assert_eq!(terminal.backend().buffer().get(0, 2).bg, MPV_ROW_BG);
        assert_eq!(terminal.backend().buffer().get(0, 7).bg, MPV_BG);
    }

    #[test]
    fn details_uses_playlist_frame_markers_and_contextual_footer() {
        let mut app = one_target_app();
        if let View::Connected { snapshot, .. } = &mut app.view {
            snapshot.targets[0].state.paused = Some(true);
            snapshot.targets[0].state.title = Some("Now Playing".into());
            snapshot.targets[0].state.volume = Some(80.0);
            snapshot.targets[0].state.loop_file = Some("inf".into());
            snapshot.targets[0].state.loop_playlist = Some("no".into());
        }
        let mut details = crate::details::Details::new("daily".into());
        details.refresh(vec![
            mpv_targets::PlaylistItem {
                id: 7,
                filename: "/media/alpha.mp4".into(),
                title: Some("Alpha".into()),
                current: true,
            },
            mpv_targets::PlaylistItem {
                id: 8,
                filename: "/media/beta.mp4".into(),
                title: Some("Beta".into()),
                current: false,
            },
        ]);
        details.selected = 1;
        details.marks.insert(8);
        app.details = Some(details);
        let lines = rendered_lines(&app, 120, 8);
        assert!(
            lines[0]
                .contains("\"DAILY\" :: [ Paused ] :: [Vol: 80 ] :: [Loop:On ] :: [Repeat:Off]")
        );
        assert_eq!(lines[2].trim_end(), "* 0    :: Alpha");
        assert_eq!(lines[3].trim_end(), "• 1    :: Beta");
        assert!(lines[7].contains("[Enter] Play/Next"));
        assert!(!lines[7].contains("[CMD]"));
        let original = app.details.clone();
        let press = |app: &mut App, code| {
            app.handle_key(crossterm::event::KeyEvent::new(
                code,
                crossterm::event::KeyModifiers::NONE,
            ))
        };
        press(&mut app, crossterm::event::KeyCode::Tab);
        assert!(rendered_lines(&app, 120, 8)[7].contains("[CMD]"));
        let mut terminal = Terminal::new(TestBackend::new(120, 8)).unwrap();
        terminal.draw(|frame| render(frame, &app)).unwrap();
        assert_eq!(terminal.backend().buffer().get(0, 3).bg, COMMAND_BG);
        assert_eq!(
            press(&mut app, crossterm::event::KeyCode::Char('a')),
            Some(crate::app::Effect::Run(
                crate::app::DashboardAction::Command {
                    target: "daily".into(),
                    action: crate::command::Action::CycleAudio,
                }
            ))
        );
        app.complete_action(Ok("done".into()));
        press(&mut app, crossterm::event::KeyCode::Char('\\'));
        assert!(rendered_lines(&app, 120, 8)[7].contains("[MPV]"));
        press(&mut app, crossterm::event::KeyCode::Char('\\'));
        press(&mut app, crossterm::event::KeyCode::Tab);
        assert_eq!(app.details, original);
        assert_eq!(rendered_lines(&app, 120, 8), lines);
    }

    #[test]
    fn new_target_uses_the_approved_questions_and_create_choices() {
        let mut app = one_target_app();
        app.new_target_prompt = Some(NewTargetPrompt {
            input: "monkey".into(),
            stage: NewTargetStage::PlayQuestion,
            channels: vec![],
            search: Default::default(),
            channel: None,
            selected: 0,
            play_on_start: false,
            mute_on_start: true,
            mode_selected: 0,
            error: None,
            busy: false,
        });
        let lines = rendered_lines(&app, 80, 10);
        assert_eq!(lines[2].trim_end(), "New Target :: [MONKEY]");
        assert_eq!(lines[3].trim_end(), "Play On Start? [y/N]");

        let prompt = app.new_target_prompt.as_mut().unwrap();
        prompt.stage = NewTargetStage::MuteQuestion;
        let lines = rendered_lines(&app, 80, 10);
        assert_eq!(lines[3].trim_end(), "Mute On Start? [Y/n]");

        let prompt = app.new_target_prompt.as_mut().unwrap();
        prompt.stage = NewTargetStage::ChannelPicker;
        prompt.channels = vec![
            "/srv/channels/cameras.m3u8".into(),
            "/srv/channels/movies.m3u8".into(),
        ];
        let lines = rendered_lines(&app, 80, 10);
        assert_eq!(lines[4].trim_end(), ":: 1. cameras.m3u8");
        assert_eq!(lines[5].trim_end(), "   2. movies.m3u8");

        let prompt = app.new_target_prompt.as_mut().unwrap();
        prompt.stage = NewTargetStage::ChoosingMode;
        let lines = rendered_lines(&app, 80, 10);
        assert_eq!(lines[3].trim_end(), ":: Create: Disabled");
        assert_eq!(lines[4].trim_end(), "   Create: Enabled/Stopped");
        assert_eq!(lines[5].trim_end(), "   Create: Enabled/Start");
    }
}
