use crossterm::event::KeyCode;

pub fn matches(query: &str, values: impl IntoIterator<Item = impl AsRef<str>>) -> Vec<usize> {
    if query.is_empty() {
        return Vec::new();
    }
    let terms = search_terms(query);
    values
        .into_iter()
        .enumerate()
        .filter_map(|(index, value)| {
            let value = normalize(&value.as_ref().to_lowercase());
            terms
                .iter()
                .all(|term| value.contains(term))
                .then_some(index)
        })
        .collect()
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ChannelSearch {
    pub query: Option<String>,
    before: usize,
    match_index: usize,
}

impl ChannelSearch {
    // Consume search keys before the picker's number shortcuts or actions.
    pub fn handle(&mut self, key: KeyCode, channels: &[String], selected: &mut usize) -> bool {
        let Some(query) = self.query.as_mut() else {
            if key != KeyCode::Char('/') {
                return false;
            }
            self.before = *selected;
            self.match_index = 0;
            self.query = Some(String::new());
            return true;
        };
        match key {
            KeyCode::Esc => {
                *selected = self.before;
                self.query = None;
                return true;
            }
            KeyCode::Enter => {
                if matches(query, channels).is_empty() {
                    *selected = self.before;
                }
                self.query = None;
                return true;
            }
            KeyCode::Backspace => {
                query.pop();
                self.match_index = 0;
            }
            KeyCode::Char(']') => self.match_index += 1,
            KeyCode::Char('[') => self.match_index = self.match_index.saturating_sub(1),
            KeyCode::Char(c) => {
                query.push(c);
                self.match_index = 0;
            }
            _ => return true,
        }
        let found = matches(query, channels);
        *selected = found
            .get(self.match_index % found.len().max(1))
            .copied()
            .unwrap_or(self.before);
        true
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

pub fn footer(query: &str, matches: &[usize], selected: usize) -> String {
    let tail = |limit| {
        query
            .chars()
            .skip(query.chars().count().saturating_sub(limit))
            .collect::<String>()
    };
    if query.is_empty() {
        "/ :: [Enter/Esc] Cancel".into()
    } else if matches.is_empty() {
        format!("/{} :: No matches :: [Enter/Esc] Cancel", tail(60))
    } else {
        format!(
            "/{} :: Match {}/{} :: [[/]] Navigate [Enter] Select [Esc] Cancel",
            tail(50),
            matches.iter().position(|i| *i == selected).unwrap_or(0) + 1,
            matches.len()
        )
    }
}
