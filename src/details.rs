use std::collections::HashSet;

use crossterm::event::{KeyCode, KeyEvent};
use mpv_targets::PlaylistItem;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Mode {
    Normal,
    Item,
    Search {
        before: Option<u64>,
        query: String,
        match_index: usize,
    },
    Goto {
        before: Option<u64>,
        input: String,
        error: Option<String>,
    },
    Add {
        input: String,
        stage: AddStage,
    },
    Save {
        input: String,
        overwrite: bool,
    },
    Crop,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AddStage {
    Input,
    Choose,
    ConfirmReplace,
}

#[derive(Clone, Debug, PartialEq)]
enum Activity {
    Idle,
    Loading,
    Action,
    Refreshing(Result<String, String>),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Details {
    pub target: String,
    pub items: Vec<PlaylistItem>,
    pub selected: usize,
    pub marks: HashSet<u64>,
    pub mode: Mode,
    pub status: Option<(String, bool)>,
    activity: Activity,
    refresh_in_flight: bool,
    refresh_needed: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Action {
    Play(usize),
    MoveNext(usize),
    Move {
        indices: Vec<usize>,
        destination: usize,
    },
    Remove {
        indices: Vec<usize>,
        kept_playing: bool,
    },
    CropBefore(usize),
    CropAfter(usize),
    Shuffle,
    Load {
        source: String,
        replace: bool,
    },
    Save {
        channel: String,
        overwrite: bool,
    },
    Loop(bool),
    Repeat(bool),
}

#[derive(Clone, Debug, PartialEq)]
pub enum Effect {
    Close,
    Run { target: String, action: Action },
}

impl Details {
    pub fn new(target: String) -> Self {
        Self {
            target,
            items: vec![],
            selected: 0,
            marks: HashSet::new(),
            mode: Mode::Normal,
            status: Some(("Loading playlist...".into(), false)),
            activity: Activity::Loading,
            refresh_in_flight: true,
            refresh_needed: false,
        }
    }

    pub fn refresh(&mut self, items: Vec<PlaylistItem>) {
        let id = self.items.get(self.selected).map(|item| item.id);
        self.items = items;
        self.selected = id
            .and_then(|id| self.items.iter().position(|item| item.id == id))
            .unwrap_or_else(|| self.selected.min(self.items.len().saturating_sub(1)));
        self.marks
            .retain(|id| self.items.iter().any(|item| item.id == *id));
        self.refresh_in_flight = false;
        if self.activity == Activity::Loading {
            self.activity = Activity::Idle;
        }
        if self
            .status
            .as_ref()
            .is_some_and(|(text, _)| text == "Loading playlist...")
        {
            self.status = None;
        }
    }

    pub fn is_busy(&self) -> bool {
        self.activity != Activity::Idle
    }

    pub fn request_refresh(&mut self) -> bool {
        self.refresh_needed = true;
        self.take_refresh()
    }

    fn take_refresh(&mut self) -> bool {
        if !self.refresh_needed || self.refresh_in_flight || self.activity == Activity::Action {
            return false;
        }
        self.refresh_needed = false;
        self.refresh_in_flight = true;
        true
    }

    pub fn playlist_loaded(&mut self, result: Result<Vec<PlaylistItem>, String>) -> bool {
        self.refresh_in_flight = false;
        let error = match result {
            Ok(items) => {
                self.refresh(items);
                None
            }
            Err(error) => Some(error),
        };
        if self.take_refresh() {
            return true;
        }
        if !matches!(self.activity, Activity::Action) {
            let activity = std::mem::replace(&mut self.activity, Activity::Idle);
            if let Activity::Refreshing(result) = activity {
                // Preserve an edit failure even if the recovery read also failed.
                self.show_result(result.and_then(|message| error.map_or(Ok(message), Err)));
            } else if let Some(error) = error {
                self.show_result(Err(error));
            }
        }
        false
    }

    pub fn complete(&mut self, result: Result<String, String>, refresh: bool) -> bool {
        if let Err(error) = &result
            && let Some(channel) = error.strip_prefix("__OVERWRITE__")
        {
            self.activity = Activity::Idle;
            self.mode = Mode::Save {
                input: channel.into(),
                overwrite: true,
            };
        } else if refresh {
            self.activity = Activity::Refreshing(result);
            return self.request_refresh();
        } else {
            self.activity = Activity::Idle;
            self.show_result(result);
        }
        self.take_refresh()
    }

    fn show_result(&mut self, result: Result<String, String>) {
        self.status = Some(match result {
            Ok(message) => (message, false),
            Err(error) => (error, true),
        });
    }

    pub fn paste(&mut self, value: String) {
        match &mut self.mode {
            Mode::Add {
                input,
                stage: AddStage::Input,
            }
            | Mode::Save {
                input,
                overwrite: false,
            } => input.push_str(&value),
            Mode::Search { query, .. } => query.push_str(&value),
            Mode::Goto { input, .. } if value.chars().all(|c| c.is_ascii_digit()) => {
                input.push_str(&value)
            }
            _ => {}
        }
    }

    pub fn handle(
        &mut self,
        key: KeyEvent,
        page: usize,
        current: Option<usize>,
        loop_on: bool,
        repeat_on: bool,
    ) -> Option<Effect> {
        if self.is_busy() {
            return None;
        }
        match self.mode.clone() {
            Mode::Search {
                before,
                mut query,
                mut match_index,
            } => {
                match key.code {
                    KeyCode::Esc => {
                        self.restore(before);
                        self.mode = Mode::Normal;
                        self.status = None;
                    }
                    KeyCode::Enter => {
                        self.mode = Mode::Normal;
                        self.status = None;
                    }
                    KeyCode::Backspace => {
                        query.pop();
                        match_index = 0;
                        self.mode = Mode::Search {
                            before,
                            query,
                            match_index,
                        };
                        self.select_match(0);
                    }
                    KeyCode::Char(']') => {
                        match_index += 1;
                        self.mode = Mode::Search {
                            before,
                            query,
                            match_index,
                        };
                        self.select_match(match_index);
                    }
                    KeyCode::Char('[') => {
                        match_index = match_index.saturating_sub(1);
                        self.mode = Mode::Search {
                            before,
                            query,
                            match_index,
                        };
                        self.select_match(match_index);
                    }
                    KeyCode::Char(c) => {
                        query.push(c);
                        match_index = 0;
                        self.mode = Mode::Search {
                            before,
                            query,
                            match_index,
                        };
                        self.select_match(0);
                    }
                    _ => {}
                }
                return None;
            }
            Mode::Goto {
                before,
                mut input,
                error: _,
            } => {
                match key.code {
                    KeyCode::Esc => {
                        self.restore(before);
                        self.mode = Mode::Normal;
                    }
                    KeyCode::Backspace => {
                        input.pop();
                        self.mode = Mode::Goto {
                            before,
                            input,
                            error: None,
                        };
                    }
                    KeyCode::Char(c) if c.is_ascii_digit() => {
                        input.push(c);
                        self.mode = Mode::Goto {
                            before,
                            input,
                            error: None,
                        };
                    }
                    KeyCode::Enter if input.is_empty() => self.mode = Mode::Normal,
                    KeyCode::Enter => match input.parse::<usize>() {
                        Ok(i) if i < self.items.len() => {
                            self.selected = i;
                            self.mode = Mode::Normal;
                        }
                        _ => {
                            self.mode = Mode::Goto {
                                before,
                                input,
                                error: Some("Index out of range".into()),
                            };
                        }
                    },
                    _ => {}
                }
                return None;
            }
            Mode::Item => return self.item_key(key, current),
            Mode::Crop => return self.crop_key(key, current),
            Mode::Add { .. } => return self.add_key(key),
            Mode::Save { .. } => return self.save_key(key),
            Mode::Normal => {}
        }
        self.status = None;
        match key.code {
            KeyCode::Left | KeyCode::Char('h') => Some(Effect::Close),
            KeyCode::Up | KeyCode::Char('k') => {
                self.selected = self.selected.saturating_sub(1);
                self.status = None;
                None
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.selected = (self.selected + 1).min(self.items.len().saturating_sub(1));
                self.status = None;
                None
            }
            KeyCode::PageUp | KeyCode::Char('K') => {
                self.selected = self.selected.saturating_sub(page.max(1));
                None
            }
            KeyCode::PageDown | KeyCode::Char('J') => {
                self.selected =
                    (self.selected + page.max(1)).min(self.items.len().saturating_sub(1));
                None
            }
            KeyCode::Home => {
                self.selected = 0;
                None
            }
            KeyCode::End => {
                self.selected = self.items.len().saturating_sub(1);
                None
            }
            KeyCode::Esc => {
                self.marks.clear();
                None
            }
            KeyCode::Char(' ') if !self.items.is_empty() => {
                let id = self.items[self.selected].id;
                if !self.marks.remove(&id) {
                    self.marks.insert(id);
                }
                self.selected = (self.selected + 1).min(self.items.len() - 1);
                None
            }
            KeyCode::Enter if !self.items.is_empty() => {
                self.mode = Mode::Item;
                None
            }
            KeyCode::Char('/') => {
                let before = self.selected_id();
                self.mode = Mode::Search {
                    before,
                    query: String::new(),
                    match_index: 0,
                };
                None
            }
            KeyCode::Char('G') => {
                let before = self.selected_id();
                self.mode = Mode::Goto {
                    before,
                    input: String::new(),
                    error: None,
                };
                None
            }
            KeyCode::Char('A') => {
                self.mode = Mode::Add {
                    input: String::new(),
                    stage: AddStage::Input,
                };
                None
            }
            KeyCode::Char('S') => {
                if self.items.is_empty() {
                    self.status = Some(("Queue is empty".into(), true));
                    None
                } else {
                    self.mode = Mode::Save {
                        input: String::new(),
                        overwrite: false,
                    };
                    None
                }
            }
            KeyCode::Char('R') if !self.items.is_empty() => self.remove(current),
            KeyCode::Char('M') => self.move_marked(current),
            KeyCode::Char('C') => {
                self.mode = Mode::Crop;
                None
            }
            KeyCode::Char('Z') => self.run(Action::Shuffle),
            KeyCode::Char('L') => self.run(Action::Loop(!loop_on)),
            KeyCode::Char('P') => self.run(Action::Repeat(!repeat_on)),
            _ => None,
        }
    }

    fn selected_id(&self) -> Option<u64> {
        self.items.get(self.selected).map(|i| i.id)
    }
    fn restore(&mut self, id: Option<u64>) {
        if let Some(i) = id.and_then(|id| self.items.iter().position(|x| x.id == id)) {
            self.selected = i
        }
    }
    pub fn matches(&self) -> Vec<usize> {
        let Mode::Search { query, .. } = &self.mode else {
            return vec![];
        };
        if query.is_empty() {
            return vec![];
        };
        let terms = search_terms(query);
        self.items
            .iter()
            .enumerate()
            .filter(|(_, i)| {
                let s = normalize(
                    &format!("{} {}", i.title.as_deref().unwrap_or(""), i.filename).to_lowercase(),
                );
                terms.iter().all(|t| s.contains(t))
            })
            .map(|(i, _)| i)
            .collect()
    }
    fn select_match(&mut self, n: usize) {
        if matches!(&self.mode,Mode::Search{query,..} if query.is_empty()) {
            self.status = None;
            return;
        }
        let m = self.matches();
        if let Some(i) = m.get(n % m.len().max(1)) {
            self.selected = *i;
            self.status = None;
        } else {
            self.status = Some(("No matches".into(), true))
        }
    }
    fn run(&mut self, action: Action) -> Option<Effect> {
        self.activity = Activity::Action;
        self.marks.clear();
        Some(Effect::Run {
            target: self.target.clone(),
            action,
        })
    }
    fn item_key(&mut self, key: KeyEvent, current: Option<usize>) -> Option<Effect> {
        match key.code {
            KeyCode::Esc => {
                self.mode = Mode::Normal;
                None
            }
            KeyCode::Enter => {
                self.mode = Mode::Normal;
                self.run(Action::Play(self.selected))
            }
            KeyCode::Char('n') => {
                self.mode = Mode::Normal;
                if current == Some(self.selected) {
                    self.marks.clear();
                    self.status = Some(("Currently playing".into(), false));
                    None
                } else if current.is_some_and(|i| i + 1 == self.selected) {
                    self.marks.clear();
                    self.status = Some(("Already next".into(), false));
                    None
                } else {
                    self.run(Action::MoveNext(self.selected))
                }
            }
            _ => None,
        }
    }
    fn crop_key(&mut self, key: KeyEvent, current: Option<usize>) -> Option<Effect> {
        match key.code {
            KeyCode::Esc => {
                self.mode = Mode::Normal;
                None
            }
            KeyCode::Char('b') | KeyCode::Char('a') => {
                self.mode = Mode::Normal;
                let Some(cur) = current else {
                    self.status = Some(("Nothing playing to crop around".into(), true));
                    return None;
                };
                self.selected = cur;
                if key.code == KeyCode::Char('b') {
                    self.run(Action::CropBefore(cur))
                } else {
                    self.run(Action::CropAfter(cur))
                }
            }
            _ => None,
        }
    }
    fn remove(&mut self, current: Option<usize>) -> Option<Effect> {
        let mut v: Vec<usize> = if self.marks.is_empty() {
            vec![self.selected]
        } else {
            self.items
                .iter()
                .enumerate()
                .filter(|(_, i)| self.marks.contains(&i.id))
                .map(|(n, _)| n)
                .collect()
        };
        let had = current.is_some_and(|c| v.contains(&c));
        v.retain(|i| Some(*i) != current);
        if v.is_empty() {
            self.marks.clear();
            self.status = Some(("Cannot remove playing item".into(), true));
            None
        } else {
            if had {
                self.status = Some((format!("Removed {}; kept playing item", v.len()), false));
            }
            self.run(Action::Remove {
                indices: v,
                kept_playing: had,
            })
        }
    }
    fn move_marked(&mut self, current: Option<usize>) -> Option<Effect> {
        if self.marks.is_empty() {
            self.status = Some(("Mark items with Space first".into(), true));
            return None;
        }
        if self.marks.contains(&self.items[self.selected].id) {
            return None;
        }
        let v: Vec<usize> = self
            .items
            .iter()
            .enumerate()
            .filter(|(_, i)| self.marks.contains(&i.id))
            .map(|(n, _)| n)
            .collect();
        if current.is_some_and(|c| v.contains(&c)) {
            self.marks.clear();
            self.status = Some(("Cannot move playing item".into(), true));
            None
        } else {
            self.run(Action::Move {
                indices: v,
                destination: self.selected,
            })
        }
    }
    fn add_key(&mut self, key: KeyEvent) -> Option<Effect> {
        let Mode::Add { input, stage } = &mut self.mode else {
            return None;
        };
        match (*stage, key.code) {
            (_, KeyCode::Esc) => {
                self.mode = Mode::Normal;
                None
            }
            (AddStage::Input, KeyCode::Backspace) => {
                input.pop();
                self.status = None;
                None
            }
            (AddStage::Input, KeyCode::Char(c)) => {
                input.push(c);
                self.status = None;
                None
            }
            (AddStage::Input, KeyCode::Enter) => {
                let s = input.trim().trim_end_matches('\n').to_owned();
                if s.contains(['\n', '\r']) {
                    self.status = Some(("Source must be one line".into(), true))
                } else if s.starts_with('~') {
                    self.status = Some(("Use an absolute target path or URL".into(), true))
                } else if !s.is_empty() {
                    *input = s;
                    *stage = AddStage::Choose
                }
                None
            }
            (AddStage::Choose, KeyCode::Char('a')) => {
                let s = input.clone();
                self.mode = Mode::Normal;
                self.run(Action::Load {
                    source: s,
                    replace: false,
                })
            }
            (AddStage::Choose, KeyCode::Char('r')) => {
                *stage = AddStage::ConfirmReplace;
                None
            }
            (AddStage::ConfirmReplace, KeyCode::Char('y')) => {
                let s = input.clone();
                self.mode = Mode::Normal;
                self.run(Action::Load {
                    source: s,
                    replace: true,
                })
            }
            (AddStage::ConfirmReplace, KeyCode::Char('n') | KeyCode::Enter) => {
                self.mode = Mode::Normal;
                None
            }
            _ => None,
        }
    }
    fn save_key(&mut self, key: KeyEvent) -> Option<Effect> {
        let Mode::Save { input, overwrite } = &mut self.mode else {
            return None;
        };
        if *overwrite {
            return match key.code {
                KeyCode::Char('y') => {
                    let s = input.clone();
                    self.mode = Mode::Normal;
                    self.run(Action::Save {
                        channel: s,
                        overwrite: true,
                    })
                }
                KeyCode::Char('n') | KeyCode::Enter | KeyCode::Esc => {
                    self.mode = Mode::Normal;
                    None
                }
                _ => None,
            };
        }
        match key.code {
            KeyCode::Esc => {
                self.mode = Mode::Normal;
                None
            }
            KeyCode::Backspace => {
                input.pop();
                self.status = None;
                None
            }
            KeyCode::Char(c) => {
                input.push(c);
                self.status = None;
                None
            }
            KeyCode::Enter => {
                let Some(name) = channel_name(input) else {
                    self.status = Some(("Channel must use .m3u or .m3u8".into(), true));
                    return None;
                };
                self.mode = Mode::Normal;
                self.run(Action::Save {
                    channel: name,
                    overwrite: false,
                })
            }
            _ => None,
        }
    }
}

fn channel_name(value: &str) -> Option<String> {
    let v = value.trim();
    if v.is_empty() || v.contains(['/', '\\']) {
        return None;
    }
    let lower = v.to_lowercase();
    if lower.ends_with(".m3u") || lower.ends_with(".m3u8") {
        Some(v.into())
    } else if v.rsplit('/').next().is_some_and(|s| s.contains('.')) {
        None
    } else {
        Some(format!("{v}.m3u"))
    }
}
fn search_terms(q: &str) -> Vec<String> {
    let mut terms = Vec::new();
    let mut current = String::new();
    let mut quoted = false;
    for c in q.to_lowercase().chars() {
        match c {
            '"' => {
                if quoted && !current.trim().is_empty() {
                    terms.push(normalize(&current));
                    current.clear();
                }
                quoted = !quoted;
            }
            c if c.is_whitespace() && !quoted => {
                if !current.trim().is_empty() {
                    terms.push(normalize(&current));
                    current.clear();
                }
            }
            c => current.push(c),
        }
    }
    if !current.trim().is_empty() {
        terms.push(normalize(&current));
    }
    terms.into_iter().filter(|term| !term.is_empty()).collect()
}

fn normalize(value: &str) -> String {
    value
        .chars()
        .map(|c| if c.is_ascii_punctuation() { ' ' } else { c })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

pub fn loop_enabled(value: Option<&str>) -> bool {
    value.is_some_and(|value| {
        matches!(
            value.to_ascii_lowercase().as_str(),
            "yes" | "true" | "on" | "inf" | "force"
        )
    })
}

pub fn display_item(item: &PlaylistItem) -> String {
    if let Some(title) = item.title.as_deref().filter(|title| !title.is_empty()) {
        return title.into();
    }
    if item.filename.starts_with("http") {
        item.filename.clone()
    } else {
        item.filename
            .rsplit('/')
            .next()
            .unwrap_or(&item.filename)
            .into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyModifiers;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }
    fn item(id: u64, title: &str, current: bool) -> PlaylistItem {
        PlaylistItem {
            id,
            filename: format!("/{title}.mp4"),
            title: Some(title.into()),
            current,
        }
    }
    fn details() -> Details {
        let mut d = Details::new("daily".into());
        d.refresh(vec![
            item(10, "Alpha", true),
            item(20, "Beta", false),
            item(30, "Gamma", false),
        ]);
        d
    }

    #[test]
    fn selection_survives_refresh_by_playlist_id() {
        let mut d = details();
        d.selected = 1;
        d.refresh(vec![
            item(30, "Gamma", false),
            item(20, "Beta", false),
            item(10, "Alpha", true),
        ]);
        assert_eq!(d.selected, 1);
    }

    #[test]
    fn refreshes_coalesce_and_do_not_unlock_an_edit() {
        let mut d = details();
        assert!(d.request_refresh());
        assert!(!d.request_refresh());
        d.handle(key(KeyCode::Char('Z')), 5, Some(0), false, false);

        // A read started before the edit may finish while the edit is running.
        assert!(!d.playlist_loaded(Ok(d.items.clone())));
        assert!(d.is_busy());
        assert_eq!(
            d.handle(key(KeyCode::Char('Z')), 5, Some(0), false, false),
            None
        );
        assert!(d.complete(Ok("Queue shuffled".into()), true));
        assert!(!d.request_refresh());
        assert!(!d.request_refresh());
        // Several changes during one read require only one follow-up read.
        assert!(d.playlist_loaded(Ok(d.items.clone())));
        assert!(d.is_busy());
        assert!(!d.playlist_loaded(Ok(d.items.clone())));
        assert!(!d.is_busy());
        assert_eq!(d.status, Some(("Queue shuffled".into(), false)));
    }

    #[test]
    fn edit_completion_waits_for_a_read_started_before_it() {
        let mut d = details();
        assert!(d.request_refresh());
        d.handle(key(KeyCode::Char('Z')), 5, Some(0), false, false);
        assert!(!d.complete(Ok("Queue shuffled".into()), true));
        assert!(d.playlist_loaded(Ok(d.items.clone())));
        assert!(d.is_busy());
        assert!(!d.playlist_loaded(Ok(d.items.clone())));
        assert!(!d.is_busy());

        d.handle(key(KeyCode::Char('Z')), 5, Some(0), false, false);
        assert!(d.complete(Err("edit failed".into()), true));
        assert!(!d.playlist_loaded(Err("refresh failed".into())));
        assert!(!d.is_busy());
        assert_eq!(d.status, Some(("edit failed".into(), true)));
    }

    #[test]
    fn non_queue_completion_does_not_request_a_playlist() {
        let mut d = details();
        for action in [
            Action::Loop(true),
            Action::Repeat(true),
            Action::Save {
                channel: "saved.m3u".into(),
                overwrite: false,
            },
        ] {
            d.run(action);
            assert!(!d.complete(Ok("done".into()), false));
            assert!(!d.is_busy());
        }
        d.run(Action::Save {
            channel: "saved.m3u".into(),
            overwrite: false,
        });
        assert!(!d.complete(Err("__OVERWRITE__saved.m3u".into()), false));
        assert!(matches!(&d.mode, Mode::Save { input, overwrite: true } if input == "saved.m3u"));
    }

    #[test]
    fn enter_is_single_item_play_or_next_and_escape_only_cancels() {
        let mut d = details();
        d.selected = 2;
        assert_eq!(
            d.handle(key(KeyCode::Enter), 5, Some(0), false, false),
            None
        );
        assert_eq!(d.mode, Mode::Item);
        assert_eq!(
            d.handle(key(KeyCode::Char('h')), 5, Some(0), false, false),
            None
        );
        assert_eq!(d.mode, Mode::Item);
        assert_eq!(
            d.handle(key(KeyCode::Char('n')), 5, Some(0), false, false),
            Some(Effect::Run {
                target: "daily".into(),
                action: Action::MoveNext(2)
            })
        );
    }

    #[test]
    fn marks_drive_move_but_never_enter() {
        let mut d = details();
        d.selected = 1;
        d.handle(key(KeyCode::Char(' ')), 5, Some(0), false, false);
        assert!(d.marks.contains(&20));
        d.selected = 2;
        d.handle(key(KeyCode::Enter), 5, Some(0), false, false);
        assert_eq!(
            d.handle(key(KeyCode::Enter), 5, Some(0), false, false),
            Some(Effect::Run {
                target: "daily".into(),
                action: Action::Play(2)
            })
        );
        assert!(d.marks.is_empty());
    }

    #[test]
    fn search_supports_and_terms_and_quoted_phrases() {
        let mut d = details();
        d.items = vec![
            item(1, "Tool Live Set", false),
            item(2, "Tool Studio", false),
        ];
        d.mode = Mode::Search {
            before: Some(1),
            query: "tool \"live set\"".into(),
            match_index: 0,
        };
        assert_eq!(d.matches(), vec![0]);
    }

    #[test]
    fn search_brackets_navigate_and_n_is_query_text() {
        let mut d = details();
        d.items = vec![item(1, "One Night", false), item(2, "Another Night", false)];
        d.mode = Mode::Search {
            before: Some(1),
            query: String::new(),
            match_index: 0,
        };

        d.handle(key(KeyCode::Char('n')), 5, None, false, false);
        assert!(matches!(&d.mode, Mode::Search { query, .. } if query == "n"));
        assert_eq!(d.selected, 0);

        d.handle(key(KeyCode::Char(']')), 5, None, false, false);
        assert_eq!(d.selected, 1);
        d.handle(key(KeyCode::Char('[')), 5, None, false, false);
        assert_eq!(d.selected, 0);
    }

    #[test]
    fn mpv_loop_values_and_path_display_are_interpreted_exactly() {
        assert!(!loop_enabled(Some("off")));
        assert!(!loop_enabled(Some("no")));
        assert!(loop_enabled(Some("inf")));
        let path = PlaylistItem {
            id: 1,
            filename: "/srv/video/movie.mp4".into(),
            title: None,
            current: false,
        };
        assert_eq!(display_item(&path), "movie.mp4");
    }

    #[test]
    fn details_has_no_quit_command_or_command_mode_binding() {
        let mut d = details();
        assert_eq!(
            d.handle(key(KeyCode::Char('q')), 5, Some(0), false, false),
            None
        );
        assert_eq!(d.handle(key(KeyCode::Tab), 5, Some(0), false, false), None);
        assert_eq!(d.mode, Mode::Normal);
        assert_eq!(
            d.handle(key(KeyCode::Left), 5, Some(0), false, false),
            Some(Effect::Close)
        );
    }
}
