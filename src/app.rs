use crossterm::event::{KeyCode, KeyEvent};
use mpv_targets::Snapshot;

use crate::{
    command::{self, Mode},
    details,
    service::Endpoint,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LifecycleAction {
    Enable,
    EnableAndStart,
    Start,
    Restart,
    Stop,
    Disable,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TargetAction {
    Lifecycle(LifecycleAction),
    Rename,
    Remove,
    SetChannel,
    Duplicate,
}

impl TargetAction {
    pub fn label(self) -> &'static str {
        match self {
            Self::Lifecycle(action) => action.label(),
            Self::Rename => "Rename",
            Self::Remove => "Remove",
            Self::SetChannel => "Set Channel",
            Self::Duplicate => "Duplicate",
        }
    }
}

impl LifecycleAction {
    pub fn label(self) -> &'static str {
        match self {
            Self::Enable => "Enable",
            Self::EnableAndStart => "Enable & Start",
            Self::Start => "Start",
            Self::Restart => "Restart",
            Self::Stop => "Stop",
            Self::Disable => "Disable",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum DashboardAction {
    Identify,
    Lifecycle {
        target: String,
        action: LifecycleAction,
    },
    Solo {
        selected: String,
        others: Vec<String>,
    },
    ToggleAll {
        targets: Vec<String>,
        play: bool,
    },
    Command {
        target: String,
        action: command::Action,
    },
    Rename {
        target: String,
        replacement: String,
    },
    Remove {
        target: String,
    },
    Duplicate {
        source: String,
        target: String,
        mode: CreateMode,
    },
    Create {
        target: String,
        channel: Option<String>,
        play_on_start: bool,
        mute_on_start: bool,
        mode: CreateMode,
    },
    SetChannel {
        target: String,
        channel: String,
        restart: bool,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ActionPicker {
    pub target: String,
    pub actions: Vec<TargetAction>,
    pub selected: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ManagementStage {
    Editing,
    ChoosingMode,
    Confirming,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ManagementPrompt {
    pub action: TargetAction,
    pub target: String,
    pub input: String,
    pub stage: ManagementStage,
    pub selected: usize,
    pub error: Option<String>,
    pub busy: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NewTargetStage {
    Name,
    ChannelQuestion,
    LoadingChannels,
    ChannelPicker,
    PlayQuestion,
    MuteQuestion,
    ChoosingMode,
    Confirming,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NewTargetPrompt {
    pub input: String,
    pub stage: NewTargetStage,
    pub channels: Vec<String>,
    pub search: crate::search::ChannelSearch,
    pub channel: Option<String>,
    pub selected: usize,
    pub play_on_start: bool,
    pub mute_on_start: bool,
    pub mode_selected: usize,
    pub error: Option<String>,
    pub busy: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ChannelStage {
    Loading,
    Picking,
    Restart,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ChannelPrompt {
    pub target: String,
    pub channels: Vec<String>,
    pub search: crate::search::ChannelSearch,
    pub selected: usize,
    pub channel: Option<String>,
    pub restart_available: bool,
    pub stage: ChannelStage,
    pub error: Option<String>,
    pub busy: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CreateMode {
    Create,
    Enable,
    Start,
}

impl CreateMode {
    pub const ALL: [Self; 3] = [Self::Create, Self::Enable, Self::Start];

    pub fn label(self) -> &'static str {
        match self {
            Self::Create => "Create: Disabled",
            Self::Enable => "Create: Enabled/Stopped",
            Self::Start => "Create: Enabled/Start",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StatusLine {
    pub text: String,
    pub error: bool,
    pub busy: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub enum View {
    Picker,
    Connecting {
        endpoint: Endpoint,
        error: Option<String>,
    },
    Connected {
        endpoint: Endpoint,
        snapshot: Snapshot,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub struct App {
    pub endpoints: Vec<Endpoint>,
    pub picker_selected: usize,
    pub target_selected: usize,
    pub view: View,
    pub action_picker: Option<ActionPicker>,
    pub management_prompt: Option<ManagementPrompt>,
    pub new_target_prompt: Option<NewTargetPrompt>,
    pub channel_prompt: Option<ChannelPrompt>,
    pub status: Option<StatusLine>,
    pub input_mode: Mode,
    pub details: Option<details::Details>,
    pub details_page: usize,
    operation_busy: bool,
    pending_command: bool,
    pending_selection: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Effect {
    Connect(Endpoint),
    CancelConnection,
    Quit,
    Run(DashboardAction),
    LoadChannels,
    LoadPlaylist {
        target: String,
    },
    RunDetails {
        target: String,
        action: details::Action,
    },
}

impl App {
    pub fn new(endpoints: Vec<Endpoint>) -> Self {
        Self {
            endpoints,
            picker_selected: 0,
            target_selected: 0,
            view: View::Picker,
            action_picker: None,
            management_prompt: None,
            new_target_prompt: None,
            channel_prompt: None,
            status: None,
            input_mode: Mode::Normal,
            details: None,
            details_page: 10,
            operation_busy: false,
            pending_command: false,
            pending_selection: None,
        }
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> Option<Effect> {
        if matches!(self.view, View::Connected { .. }) {
            if self.input_mode != Mode::Normal {
                return self.handle_command_key(key);
            }
            if key.code == KeyCode::Tab {
                if self.is_busy() || self.details.as_ref().is_some_and(|d| d.is_busy()) {
                    return None;
                }
                let target = self.selected_target()?;
                if target_is_controllable(target) {
                    self.input_mode = Mode::Command;
                    self.status = None;
                } else {
                    self.complete_action(Err(format!(
                        "{} is not available for playback control",
                        target.name.to_uppercase()
                    )));
                }
                return None;
            }
        }
        if self.details.is_some() {
            return self.handle_details_key(key);
        }
        match &self.view {
            View::Picker => match key.code {
                KeyCode::Char('j') | KeyCode::Down => {
                    self.picker_selected =
                        (self.picker_selected + 1).min(self.endpoints.len().saturating_sub(1));
                    None
                }
                KeyCode::Char('k') | KeyCode::Up => {
                    self.picker_selected = self.picker_selected.saturating_sub(1);
                    None
                }
                KeyCode::Enter => {
                    self.endpoints
                        .get(self.picker_selected)
                        .cloned()
                        .map(|endpoint| {
                            self.view = View::Connecting {
                                endpoint: endpoint.clone(),
                                error: None,
                            };
                            Effect::Connect(endpoint)
                        })
                }
                KeyCode::Char('q') => Some(Effect::Quit),
                _ => None,
            },
            View::Connecting { .. } => match key.code {
                KeyCode::Char('q') | KeyCode::Esc => {
                    self.view = View::Picker;
                    Some(Effect::CancelConnection)
                }
                _ => None,
            },
            View::Connected { .. } if self.management_prompt.is_some() => {
                self.handle_management_key(key)
            }
            View::Connected { .. } if self.new_target_prompt.is_some() => {
                self.handle_new_target_key(key)
            }
            View::Connected { .. } if self.channel_prompt.is_some() => self.handle_channel_key(key),
            View::Connected { .. } if self.action_picker.is_some() => {
                self.handle_action_picker_key(key)
            }
            View::Connected { .. } => {
                if !self.operation_busy {
                    self.status = None;
                }
                match key.code {
                    KeyCode::Char('j') | KeyCode::Down => {
                        let count = self.target_count();
                        self.target_selected =
                            (self.target_selected + 1).min(count.saturating_sub(1));
                        None
                    }
                    KeyCode::Char('k') | KeyCode::Up => {
                        self.target_selected = self.target_selected.saturating_sub(1);
                        None
                    }
                    KeyCode::Enter => {
                        if self.is_busy() {
                            return None;
                        }
                        let target = self.selected_target()?;
                        self.action_picker = Some(ActionPicker {
                            target: target.name.clone(),
                            actions: target_actions(target),
                            selected: 0,
                        });
                        self.status = None;
                        None
                    }
                    KeyCode::Char('S') => self.solo(),
                    KeyCode::Char('P') => self.toggle_all(),
                    KeyCode::Char('I') if !self.is_busy() => {
                        self.start_action("Identifying targets…".into(), DashboardAction::Identify)
                    }
                    KeyCode::Char('N') => {
                        if self.is_busy() {
                            return None;
                        }
                        self.new_target_prompt = Some(NewTargetPrompt {
                            input: String::new(),
                            stage: NewTargetStage::Name,
                            channels: Vec::new(),
                            search: Default::default(),
                            channel: None,
                            selected: 0,
                            play_on_start: false,
                            mute_on_start: true,
                            mode_selected: 0,
                            error: None,
                            busy: false,
                        });
                        self.status = None;
                        None
                    }
                    KeyCode::Right | KeyCode::Char('l') => {
                        if self.is_busy() {
                            return None;
                        }
                        let target = self.selected_target()?.name.clone();
                        self.details = Some(details::Details::new(target.clone()));
                        Some(Effect::LoadPlaylist { target })
                    }
                    KeyCode::Char('C') => {
                        self.reset_transient();
                        self.view = View::Picker;
                        Some(Effect::CancelConnection)
                    }
                    KeyCode::Char('q') => Some(Effect::Quit),
                    _ => None,
                }
            }
        }
    }

    pub fn handle_paste(&mut self, value: String) {
        if self.input_mode != Mode::Normal {
            return;
        }
        if let Some(details) = self.details.as_mut() {
            details.paste(value);
        }
    }

    fn handle_details_key(&mut self, key: KeyEvent) -> Option<Effect> {
        let target = self.selected_target()?;
        let current = self
            .details
            .as_ref()?
            .items
            .iter()
            .position(|item| item.current)
            .or(target.state.playlist_position.map(|value| value as usize));
        let loop_on = details::loop_enabled(target.state.loop_file.as_deref());
        let repeat_on = details::loop_enabled(target.state.loop_playlist.as_deref());
        let effect =
            self.details
                .as_mut()?
                .handle(key, self.details_page, current, loop_on, repeat_on)?;
        match effect {
            details::Effect::Close => {
                self.details = None;
                None
            }
            details::Effect::Run { target, action } => Some(Effect::RunDetails { target, action }),
        }
    }

    pub fn playlist_loaded(
        &mut self,
        target: &str,
        result: Result<Vec<mpv_targets::PlaylistItem>, String>,
    ) -> Option<String> {
        let details = self.details.as_mut().filter(|d| d.target == target)?;
        details.playlist_loaded(result).then(|| target.to_owned())
    }

    pub fn details_completed(
        &mut self,
        target: &str,
        refresh: bool,
        result: Result<String, String>,
    ) -> Option<String> {
        let details = self.details.as_mut().filter(|d| d.target == target)?;
        details.complete(result, refresh).then(|| target.to_owned())
    }

    pub fn retrying(&mut self, endpoint: Endpoint, error: String) {
        self.reset_transient();
        self.view = View::Connecting {
            endpoint,
            error: Some(error),
        };
    }

    pub fn connected(&mut self, endpoint: Endpoint, mut snapshot: Snapshot) {
        snapshot.targets.sort_by_key(|target| target.disabled);
        self.target_selected = 0;
        self.reset_transient();
        self.view = View::Connected { endpoint, snapshot };
    }

    pub fn apply_snapshot(&mut self, mut snapshot: Snapshot) -> Option<String> {
        snapshot.targets.sort_by_key(|target| target.disabled);
        let refresh = self.details.as_ref().and_then(|details| {
            let old = self.selected_target()?.state.playlist_count;
            let new = snapshot
                .targets
                .iter()
                .find(|target| target.name == details.target)?
                .state
                .playlist_count;
            let old_position = self.selected_target()?.state.playlist_position;
            let new_position = snapshot
                .targets
                .iter()
                .find(|target| target.name == details.target)?
                .state
                .playlist_position;
            (old != new || old_position != new_position).then(|| details.target.clone())
        });
        let selected_name = self
            .pending_selection
            .clone()
            .or_else(|| self.selected_target().map(|target| target.name.clone()));
        if let View::Connected {
            snapshot: current, ..
        } = &mut self.view
        {
            *current = snapshot;
            let selected = selected_name.and_then(|name| {
                current
                    .targets
                    .iter()
                    .position(|target| target.name == name)
            });
            if selected.is_some() {
                self.pending_selection = None;
            }
            self.target_selected = selected.unwrap_or_else(|| {
                self.target_selected
                    .min(current.targets.len().saturating_sub(1))
            });
            if self.input_mode != Mode::Normal
                && current
                    .targets
                    .get(self.target_selected)
                    .is_none_or(|target| !target_is_controllable(target))
            {
                self.input_mode = Mode::Normal;
                self.complete_action(Err(
                    "Target is no longer available for playback control".into()
                ));
            }
        }
        refresh.filter(|_| {
            self.details
                .as_mut()
                .is_some_and(|details| details.request_refresh())
        })
    }

    pub fn selected_target(&self) -> Option<&mpv_targets::TargetRecord> {
        match &self.view {
            View::Connected { snapshot, .. } => snapshot.targets.get(self.target_selected),
            _ => None,
        }
    }

    fn target_count(&self) -> usize {
        match &self.view {
            View::Connected { snapshot, .. } => snapshot.targets.len(),
            _ => 0,
        }
    }

    pub fn complete_action(&mut self, result: Result<String, String>) {
        if self
            .channel_prompt
            .as_ref()
            .is_some_and(|prompt| prompt.busy)
        {
            self.operation_busy = false;
            match result {
                Ok(text) => {
                    self.channel_prompt = None;
                    self.status = Some(StatusLine {
                        text,
                        error: false,
                        busy: false,
                    });
                }
                Err(error) => {
                    let prompt = self
                        .channel_prompt
                        .as_mut()
                        .expect("checked channel prompt");
                    prompt.busy = false;
                    prompt.error = Some(error);
                    prompt.stage = ChannelStage::Picking;
                    self.status = None;
                }
            }
            return;
        }
        if self
            .new_target_prompt
            .as_ref()
            .is_some_and(|prompt| prompt.busy)
        {
            self.operation_busy = false;
            match result {
                Ok(text) => {
                    let prompt = self
                        .new_target_prompt
                        .take()
                        .expect("checked new target prompt");
                    self.select_when_available(prompt.input);
                    self.status = Some(StatusLine {
                        text,
                        error: false,
                        busy: false,
                    });
                }
                Err(error) => {
                    let prompt = self
                        .new_target_prompt
                        .as_mut()
                        .expect("checked new target prompt");
                    prompt.stage = NewTargetStage::Name;
                    prompt.error = Some(error);
                    prompt.busy = false;
                    self.status = None;
                }
            }
            return;
        }
        if self
            .management_prompt
            .as_ref()
            .is_some_and(|prompt| prompt.busy)
        {
            self.operation_busy = false;
            match result {
                Ok(text) => {
                    let prompt = self
                        .management_prompt
                        .take()
                        .expect("checked management prompt");
                    if matches!(
                        prompt.action,
                        TargetAction::Rename | TargetAction::Duplicate
                    ) {
                        self.select_when_available(prompt.input);
                    }
                    self.status = Some(StatusLine {
                        text,
                        error: false,
                        busy: false,
                    });
                }
                Err(error) => {
                    let prompt = self
                        .management_prompt
                        .as_mut()
                        .expect("checked management prompt");
                    prompt.stage = if prompt.action == TargetAction::Remove {
                        ManagementStage::Confirming
                    } else {
                        ManagementStage::Editing
                    };
                    prompt.error = Some(error);
                    prompt.busy = false;
                    self.status = None;
                }
            }
            return;
        }

        let command = self.pending_command;
        self.operation_busy = false;
        self.pending_command = false;
        if command && result.is_ok() {
            self.status = None;
            return;
        }
        self.status = Some(match result {
            Ok(text) => StatusLine {
                text,
                error: false,
                busy: false,
            },
            Err(text) => StatusLine {
                text,
                error: true,
                busy: false,
            },
        });
    }

    pub fn channels_loaded(&mut self, result: Result<Vec<String>, String>) {
        if let Some(prompt) = self.channel_prompt.as_mut() {
            if prompt.stage != ChannelStage::Loading {
                return;
            }
            match result {
                Ok(channels) if !channels.is_empty() => {
                    prompt.channels = channels;
                    prompt.selected = 0;
                    prompt.error = None;
                    prompt.stage = ChannelStage::Picking;
                }
                Ok(_) => prompt.error = Some("No channels available".into()),
                Err(error) => prompt.error = Some(error),
            }
            return;
        }
        let Some(prompt) = self.new_target_prompt.as_mut() else {
            return;
        };
        if prompt.stage != NewTargetStage::LoadingChannels {
            return;
        }
        match result {
            Ok(channels) if !channels.is_empty() => {
                prompt.channels = channels;
                prompt.selected = 0;
                prompt.error = None;
                prompt.stage = NewTargetStage::ChannelPicker;
            }
            Ok(_) => {
                prompt.error = Some("No startup channels available".into());
                prompt.stage = NewTargetStage::ChannelQuestion;
            }
            Err(error) => {
                prompt.error = Some(error);
                prompt.stage = NewTargetStage::ChannelQuestion;
            }
        }
    }

    fn handle_channel_key(&mut self, key: KeyEvent) -> Option<Effect> {
        let prompt = self.channel_prompt.as_mut()?;
        if prompt.busy {
            return None;
        }
        if prompt.stage == ChannelStage::Picking
            && prompt
                .search
                .handle(key.code, &prompt.channels, &mut prompt.selected)
        {
            return None;
        }
        match prompt.stage {
            ChannelStage::Loading => match key.code {
                KeyCode::Esc => {
                    self.channel_prompt = None;
                    None
                }
                _ => None,
            },
            ChannelStage::Picking => match key.code {
                KeyCode::Char(character) if character.is_ascii_digit() => {
                    let number = character.to_digit(10)?;
                    let index = number.checked_sub(1).map(|value| value as usize)?;
                    let channel = prompt.channels.get(index).cloned()?;
                    prompt.selected = index;
                    prompt.channel = Some(channel);
                    self.finish_channel_choice()
                }
                KeyCode::Char('j') | KeyCode::Down => {
                    prompt.selected =
                        (prompt.selected + 1).min(prompt.channels.len().saturating_sub(1));
                    None
                }
                KeyCode::Char('k') | KeyCode::Up => {
                    prompt.selected = prompt.selected.saturating_sub(1);
                    None
                }
                KeyCode::Enter => {
                    prompt.channel = prompt.channels.get(prompt.selected).cloned();
                    self.finish_channel_choice()
                }
                KeyCode::Esc => {
                    self.channel_prompt = None;
                    None
                }
                _ => None,
            },
            ChannelStage::Restart => match key.code {
                KeyCode::Char('y') => self.run_channel_choice(true),
                KeyCode::Char('n') | KeyCode::Enter => self.run_channel_choice(false),
                KeyCode::Esc => {
                    prompt.channel = None;
                    prompt.stage = ChannelStage::Picking;
                    None
                }
                _ => None,
            },
        }
    }

    fn finish_channel_choice(&mut self) -> Option<Effect> {
        if self.channel_prompt.as_ref()?.restart_available {
            self.channel_prompt.as_mut()?.stage = ChannelStage::Restart;
            None
        } else {
            self.run_channel_choice(false)
        }
    }

    fn run_channel_choice(&mut self, restart: bool) -> Option<Effect> {
        let prompt = self.channel_prompt.as_mut()?;
        let channel = prompt.channel.clone()?;
        prompt.busy = true;
        prompt.error = None;
        self.operation_busy = true;
        Some(Effect::Run(DashboardAction::SetChannel {
            target: prompt.target.clone(),
            channel,
            restart,
        }))
    }

    fn handle_new_target_key(&mut self, key: KeyEvent) -> Option<Effect> {
        let prompt = self.new_target_prompt.as_mut()?;
        if prompt.busy {
            return None;
        }
        if prompt.stage == NewTargetStage::ChannelPicker
            && prompt
                .search
                .handle(key.code, &prompt.channels, &mut prompt.selected)
        {
            return None;
        }
        match prompt.stage {
            NewTargetStage::Name => match key.code {
                KeyCode::Esc => {
                    self.new_target_prompt = None;
                    None
                }
                KeyCode::Backspace => {
                    prompt.input.pop();
                    prompt.error = None;
                    None
                }
                KeyCode::Enter => {
                    match mpv_targets::protocol::validate_target_name(&prompt.input) {
                        Ok(()) => {
                            prompt.stage = NewTargetStage::ChannelQuestion;
                            prompt.error = None;
                        }
                        Err(error) => prompt.error = Some(error.to_string()),
                    }
                    None
                }
                KeyCode::Char(character) if prompt.input.len() < 64 => {
                    prompt.input.push(character);
                    prompt.error = None;
                    None
                }
                _ => None,
            },
            NewTargetStage::ChannelQuestion => match key.code {
                KeyCode::Char('y') => {
                    prompt.stage = NewTargetStage::LoadingChannels;
                    prompt.error = None;
                    Some(Effect::LoadChannels)
                }
                KeyCode::Char('n') | KeyCode::Enter => {
                    prompt.channel = None;
                    prompt.stage = NewTargetStage::PlayQuestion;
                    prompt.error = None;
                    None
                }
                KeyCode::Esc => {
                    prompt.stage = NewTargetStage::Name;
                    None
                }
                _ => None,
            },
            NewTargetStage::LoadingChannels => None,
            NewTargetStage::ChannelPicker => match key.code {
                KeyCode::Char(character) if character.is_ascii_digit() => {
                    let number = character.to_digit(10)?;
                    let index = number.checked_sub(1).map(|value| value as usize)?;
                    let channel = prompt.channels.get(index).cloned()?;
                    prompt.selected = index;
                    prompt.channel = Some(channel);
                    prompt.stage = NewTargetStage::PlayQuestion;
                    None
                }
                KeyCode::Char('j') | KeyCode::Down => {
                    prompt.selected =
                        (prompt.selected + 1).min(prompt.channels.len().saturating_sub(1));
                    None
                }
                KeyCode::Char('k') | KeyCode::Up => {
                    prompt.selected = prompt.selected.saturating_sub(1);
                    None
                }
                KeyCode::Enter => {
                    prompt.channel = prompt.channels.get(prompt.selected).cloned();
                    prompt.stage = NewTargetStage::PlayQuestion;
                    None
                }
                KeyCode::Esc => {
                    prompt.stage = NewTargetStage::ChannelQuestion;
                    None
                }
                _ => None,
            },
            NewTargetStage::PlayQuestion => match key.code {
                KeyCode::Char('y') => {
                    prompt.play_on_start = true;
                    prompt.stage = NewTargetStage::MuteQuestion;
                    None
                }
                KeyCode::Char('n') | KeyCode::Enter => {
                    prompt.play_on_start = false;
                    prompt.stage = NewTargetStage::MuteQuestion;
                    None
                }
                KeyCode::Esc => {
                    prompt.stage = NewTargetStage::ChannelQuestion;
                    None
                }
                _ => None,
            },
            NewTargetStage::MuteQuestion => match key.code {
                KeyCode::Char('y') | KeyCode::Enter => {
                    prompt.mute_on_start = true;
                    prompt.stage = NewTargetStage::ChoosingMode;
                    None
                }
                KeyCode::Char('n') => {
                    prompt.mute_on_start = false;
                    prompt.stage = NewTargetStage::ChoosingMode;
                    None
                }
                KeyCode::Esc => {
                    prompt.stage = NewTargetStage::PlayQuestion;
                    None
                }
                _ => None,
            },
            NewTargetStage::ChoosingMode => match key.code {
                KeyCode::Char('j') | KeyCode::Down => {
                    prompt.mode_selected =
                        (prompt.mode_selected + 1).min(CreateMode::ALL.len() - 1);
                    None
                }
                KeyCode::Char('k') | KeyCode::Up => {
                    prompt.mode_selected = prompt.mode_selected.saturating_sub(1);
                    None
                }
                KeyCode::Enter => {
                    prompt.stage = NewTargetStage::Confirming;
                    None
                }
                KeyCode::Esc => {
                    prompt.stage = NewTargetStage::MuteQuestion;
                    None
                }
                _ => None,
            },
            NewTargetStage::Confirming => match key.code {
                KeyCode::Char('y') => {
                    let action = DashboardAction::Create {
                        target: prompt.input.clone(),
                        channel: prompt.channel.clone(),
                        play_on_start: prompt.play_on_start,
                        mute_on_start: prompt.mute_on_start,
                        mode: CreateMode::ALL[prompt.mode_selected],
                    };
                    prompt.busy = true;
                    self.operation_busy = true;
                    Some(Effect::Run(action))
                }
                KeyCode::Char('n') | KeyCode::Enter | KeyCode::Esc => {
                    self.new_target_prompt = None;
                    None
                }
                _ => None,
            },
        }
    }

    fn handle_action_picker_key(&mut self, key: KeyEvent) -> Option<Effect> {
        let picker = self.action_picker.as_mut()?;
        match key.code {
            KeyCode::Char('l') | KeyCode::Right | KeyCode::Char('j') | KeyCode::Down => {
                picker.selected = (picker.selected + 1).min(picker.actions.len().saturating_sub(1));
                None
            }
            KeyCode::Char('h') | KeyCode::Left | KeyCode::Char('k') | KeyCode::Up => {
                picker.selected = picker.selected.saturating_sub(1);
                None
            }
            KeyCode::Esc => {
                self.action_picker = None;
                None
            }
            KeyCode::Enter => {
                let action = picker.actions.get(picker.selected).copied()?;
                let target = picker.target.clone();
                self.action_picker = None;
                match action {
                    TargetAction::Lifecycle(action) => self.start_action(
                        format!("{} {}…", action.label(), target.to_uppercase()),
                        DashboardAction::Lifecycle { target, action },
                    ),
                    TargetAction::Rename => {
                        self.management_prompt = Some(ManagementPrompt {
                            action,
                            target,
                            input: String::new(),
                            stage: ManagementStage::Editing,
                            selected: 0,
                            error: None,
                            busy: false,
                        });
                        None
                    }
                    TargetAction::Remove => {
                        self.management_prompt = Some(ManagementPrompt {
                            action,
                            target,
                            input: String::new(),
                            stage: ManagementStage::Confirming,
                            selected: 0,
                            error: None,
                            busy: false,
                        });
                        None
                    }
                    TargetAction::SetChannel => {
                        let restart_available = self.selected_target().is_some_and(|record| {
                            record.online && !record.disabled && !record.stopped
                        });
                        self.channel_prompt = Some(ChannelPrompt {
                            target,
                            channels: Vec::new(),
                            search: Default::default(),
                            selected: 0,
                            channel: None,
                            restart_available,
                            stage: ChannelStage::Loading,
                            error: None,
                            busy: false,
                        });
                        Some(Effect::LoadChannels)
                    }
                    TargetAction::Duplicate => {
                        self.management_prompt = Some(ManagementPrompt {
                            action,
                            target,
                            input: String::new(),
                            stage: ManagementStage::Editing,
                            selected: 0,
                            error: None,
                            busy: false,
                        });
                        None
                    }
                }
            }
            _ => None,
        }
    }

    fn handle_management_key(&mut self, key: KeyEvent) -> Option<Effect> {
        let prompt = self.management_prompt.as_mut()?;
        if prompt.busy {
            return None;
        }
        match prompt.stage {
            ManagementStage::Editing => match key.code {
                KeyCode::Esc => {
                    self.return_to_actions();
                    None
                }
                KeyCode::Backspace => {
                    prompt.input.pop();
                    prompt.error = None;
                    None
                }
                KeyCode::Enter => {
                    if let Err(error) = mpv_targets::protocol::validate_target_name(&prompt.input) {
                        prompt.error = Some(error.to_string());
                    } else if prompt.action == TargetAction::Duplicate {
                        prompt.stage = ManagementStage::ChoosingMode;
                        prompt.error = None;
                    } else {
                        prompt.stage = ManagementStage::Confirming;
                        prompt.error = None;
                    }
                    None
                }
                KeyCode::Char(character) if prompt.input.len() < 64 => {
                    prompt.input.push(character);
                    prompt.error = None;
                    None
                }
                _ => None,
            },
            ManagementStage::ChoosingMode => match key.code {
                KeyCode::Char('j') | KeyCode::Down => {
                    prompt.selected = (prompt.selected + 1).min(CreateMode::ALL.len() - 1);
                    None
                }
                KeyCode::Char('k') | KeyCode::Up => {
                    prompt.selected = prompt.selected.saturating_sub(1);
                    None
                }
                KeyCode::Enter => {
                    prompt.stage = ManagementStage::Confirming;
                    None
                }
                KeyCode::Esc => {
                    prompt.stage = ManagementStage::Editing;
                    None
                }
                _ => None,
            },
            ManagementStage::Confirming => match key.code {
                KeyCode::Char('y') => {
                    let action = match prompt.action {
                        TargetAction::Rename => DashboardAction::Rename {
                            target: prompt.target.clone(),
                            replacement: prompt.input.clone(),
                        },
                        TargetAction::Remove => DashboardAction::Remove {
                            target: prompt.target.clone(),
                        },
                        TargetAction::Duplicate => DashboardAction::Duplicate {
                            source: prompt.target.clone(),
                            target: prompt.input.clone(),
                            mode: CreateMode::ALL[prompt.selected],
                        },
                        TargetAction::SetChannel => unreachable!(),
                        TargetAction::Lifecycle(_) => unreachable!(),
                    };
                    prompt.busy = true;
                    self.operation_busy = true;
                    Some(Effect::Run(action))
                }
                KeyCode::Char('n') | KeyCode::Enter | KeyCode::Esc => {
                    self.return_to_actions();
                    None
                }
                _ => None,
            },
        }
    }

    fn solo(&mut self) -> Option<Effect> {
        if self.is_busy() {
            return None;
        }
        let selected = self.selected_target()?.name.clone();
        let others = match &self.view {
            View::Connected { snapshot, .. } => snapshot
                .targets
                .iter()
                .filter(|target| target.name != selected && target.online && !target.disabled)
                .map(|target| target.name.clone())
                .collect(),
            _ => return None,
        };
        self.start_action(
            format!("Solo {}…", selected.to_uppercase()),
            DashboardAction::Solo { selected, others },
        )
    }

    fn toggle_all(&mut self) -> Option<Effect> {
        if self.is_busy() {
            return None;
        }
        let (targets, play) = match &self.view {
            View::Connected { snapshot, .. } => {
                let applicable = snapshot
                    .targets
                    .iter()
                    .filter(|target| target.online && !target.disabled)
                    .collect::<Vec<_>>();
                let play = !applicable.is_empty()
                    && applicable
                        .iter()
                        .all(|target| target.state.paused == Some(true));
                (
                    applicable
                        .into_iter()
                        .map(|target| target.name.clone())
                        .collect::<Vec<_>>(),
                    play,
                )
            }
            _ => return None,
        };
        if targets.is_empty() {
            self.complete_action(Err("No online enabled targets".into()));
            return None;
        }
        self.start_action(
            format!("{} all targets…", if play { "Play" } else { "Pause" }),
            DashboardAction::ToggleAll { targets, play },
        )
    }

    fn start_action(&mut self, text: String, action: DashboardAction) -> Option<Effect> {
        self.operation_busy = true;
        self.pending_command = false;
        self.status = Some(StatusLine {
            text,
            error: false,
            busy: true,
        });
        Some(Effect::Run(action))
    }

    fn handle_command_key(&mut self, key: KeyEvent) -> Option<Effect> {
        let input = command::interpret(self.input_mode, key)?;
        match input {
            command::Input::EnterPassthrough => {
                self.input_mode = Mode::Passthrough;
                self.status = None;
                None
            }
            command::Input::ExitPassthrough => {
                self.input_mode = Mode::Command;
                self.status = None;
                None
            }
            command::Input::ExitCommand => {
                self.input_mode = Mode::Normal;
                self.status = None;
                None
            }
            command::Input::Run(action) => {
                if self.is_busy() {
                    return None;
                }
                let target = self.selected_target()?.name.clone();
                self.operation_busy = true;
                self.pending_command = true;
                self.status = Some(StatusLine {
                    text: format!("Command {}…", target.to_uppercase()),
                    error: false,
                    busy: true,
                });
                Some(Effect::Run(DashboardAction::Command { target, action }))
            }
        }
    }

    fn is_busy(&self) -> bool {
        self.operation_busy
    }

    fn reset_transient(&mut self) {
        self.action_picker = None;
        self.management_prompt = None;
        self.new_target_prompt = None;
        self.channel_prompt = None;
        self.status = None;
        self.input_mode = Mode::Normal;
        self.details = None;
        self.operation_busy = false;
        self.pending_command = false;
        self.pending_selection = None;
    }

    fn return_to_actions(&mut self) {
        let Some(prompt) = self.management_prompt.take() else {
            return;
        };
        let target_name = prompt.target;
        let Some(target) = self.selected_target() else {
            return;
        };
        if target.name == target_name {
            let actions = target_actions(target);
            let selected = actions
                .iter()
                .position(|action| *action == prompt.action)
                .unwrap_or(0);
            self.action_picker = Some(ActionPicker {
                target: target_name,
                actions,
                selected,
            });
        }
    }

    fn select_when_available(&mut self, target_name: String) {
        let selected = match &self.view {
            View::Connected { snapshot, .. } => snapshot
                .targets
                .iter()
                .position(|target| target.name == target_name),
            _ => None,
        };
        if let Some(selected) = selected {
            self.target_selected = selected;
            self.pending_selection = None;
        } else {
            self.pending_selection = Some(target_name);
        }
    }
}

pub fn target_actions(target: &mpv_targets::TargetRecord) -> Vec<TargetAction> {
    let mut actions = if target.disabled {
        vec![
            TargetAction::Lifecycle(LifecycleAction::Enable),
            TargetAction::Lifecycle(LifecycleAction::EnableAndStart),
        ]
    } else if target.stopped {
        vec![
            TargetAction::Lifecycle(LifecycleAction::Start),
            TargetAction::Lifecycle(LifecycleAction::Disable),
        ]
    } else {
        vec![
            TargetAction::Lifecycle(LifecycleAction::Restart),
            TargetAction::Lifecycle(LifecycleAction::Stop),
            TargetAction::Lifecycle(LifecycleAction::Disable),
        ]
    };
    if target.stopped {
        actions.push(TargetAction::Rename);
        actions.push(TargetAction::Remove);
    }
    actions.push(TargetAction::Duplicate);
    actions.insert(actions.len() - 1, TargetAction::SetChannel);
    actions
}

fn target_is_controllable(target: &mpv_targets::TargetRecord) -> bool {
    !target.disabled && !target.stopped && target.online
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyModifiers;
    use mpv_targets::{Health, HealthState, TargetRecord, TargetState};

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn app() -> App {
        App::new(vec![
            Endpoint::new("local", "wss://127.0.0.1:9876"),
            Endpoint::new("fez", "wss://fez.test:9876"),
        ])
    }

    fn snapshot(names: &[&str]) -> Snapshot {
        Snapshot {
            protocol_version: 1,
            node_id: "fezzik".into(),
            health: Health {
                state: HealthState::Ready,
                targets_configured: names.len() as u32,
                targets_expected_online: names.len() as u32,
                targets_online: names.len() as u32,
            },
            targets: names
                .iter()
                .map(|name| TargetRecord {
                    name: (*name).into(),
                    disabled: false,
                    stopped: false,
                    online: true,
                    channel: None,
                    state: TargetState::default(),
                    revision: 0,
                })
                .collect(),
        }
    }

    #[test]
    fn picker_navigation_does_not_wrap() {
        let mut app = app();
        app.handle_key(key(KeyCode::Up));
        assert_eq!(app.picker_selected, 0);
        app.handle_key(key(KeyCode::Char('j')));
        app.handle_key(key(KeyCode::Down));
        assert_eq!(app.picker_selected, 1);
    }

    #[test]
    fn picker_connect_and_cancel_preserve_selection() {
        let mut app = app();
        app.handle_key(key(KeyCode::Down));
        assert_eq!(
            app.handle_key(key(KeyCode::Enter)),
            Some(Effect::Connect(Endpoint::new("fez", "wss://fez.test:9876")))
        );
        assert_eq!(
            app.handle_key(key(KeyCode::Char('q'))),
            Some(Effect::CancelConnection)
        );
        assert_eq!(app.view, View::Picker);
        assert_eq!(app.picker_selected, 1);
    }

    #[test]
    fn target_selection_does_not_wrap_and_follows_identity() {
        let mut app = app();
        app.connected(
            app.endpoints[0].clone(),
            snapshot(&["daily", "dat", "gents"]),
        );
        app.handle_key(key(KeyCode::Up));
        assert_eq!(app.selected_target().unwrap().name, "daily");
        app.handle_key(key(KeyCode::Down));
        assert_eq!(app.selected_target().unwrap().name, "dat");

        app.apply_snapshot(snapshot(&["gents", "daily", "dat"]));
        assert_eq!(app.selected_target().unwrap().name, "dat");
        assert_eq!(app.target_selected, 2);
        app.handle_key(key(KeyCode::Char('j')));
        assert_eq!(app.target_selected, 2);
    }

    #[test]
    fn disabled_targets_sort_last_and_selection_follows_them() {
        let mut app = app();
        let mut state = snapshot(&["daily", "dat", "gents", "other"]);
        state.targets[0].disabled = true;
        state.targets[2].disabled = true;
        state.targets[3].online = false;
        app.connected(app.endpoints[0].clone(), state.clone());
        let names = |app: &App| match &app.view {
            View::Connected { snapshot, .. } => snapshot
                .targets
                .iter()
                .map(|target| target.name.clone())
                .collect::<Vec<_>>(),
            _ => unreachable!(),
        };
        assert_eq!(names(&app), ["dat", "other", "daily", "gents"]);

        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(100, 8)).unwrap();
        terminal
            .draw(|frame| crate::ui::render(frame, &app))
            .unwrap();
        for (row, name) in ["DAT", "OTHER", "DAILY", "GENTS"].iter().enumerate() {
            let line = (0..100)
                .map(|x| terminal.backend().buffer().get(x, row as u16 + 2).symbol())
                .collect::<String>();
            assert!(line.contains(&format!("\"{name}\"")));
        }

        state.targets[1].disabled = true;
        app.apply_snapshot(state.clone());
        assert_eq!(names(&app), ["other", "daily", "dat", "gents"]);
        assert_eq!(app.selected_target().unwrap().name, "dat");
        assert_eq!(app.target_selected, 2);
        state.targets[1].disabled = false;
        app.apply_snapshot(state);
        assert_eq!(app.target_selected, 0);
        assert_eq!(app.selected_target().unwrap().name, "dat");
    }

    #[test]
    fn lifecycle_picker_tracks_service_native_target_state() {
        let mut target = snapshot(&["daily"]).targets.remove(0);
        target.disabled = true;
        assert_eq!(
            target_actions(&target),
            vec![
                TargetAction::Lifecycle(LifecycleAction::Enable),
                TargetAction::Lifecycle(LifecycleAction::EnableAndStart),
                TargetAction::SetChannel,
                TargetAction::Duplicate
            ]
        );
        target.disabled = false;
        target.stopped = true;
        assert_eq!(
            target_actions(&target),
            vec![
                TargetAction::Lifecycle(LifecycleAction::Start),
                TargetAction::Lifecycle(LifecycleAction::Disable),
                TargetAction::Rename,
                TargetAction::Remove,
                TargetAction::SetChannel,
                TargetAction::Duplicate
            ]
        );
        target.stopped = false;
        target.online = false;
        assert_eq!(
            target_actions(&target),
            vec![
                TargetAction::Lifecycle(LifecycleAction::Restart),
                TargetAction::Lifecycle(LifecycleAction::Stop),
                TargetAction::Lifecycle(LifecycleAction::Disable),
                TargetAction::SetChannel,
                TargetAction::Duplicate
            ]
        );
    }

    #[test]
    fn solo_and_toggle_all_are_snapshot_ordered_concrete_actions() {
        let mut app = app();
        let mut state = snapshot(&["daily", "dat", "gents"]);
        state.targets[0].state.paused = Some(true);
        state.targets[1].state.paused = Some(true);
        state.targets[2].state.paused = Some(true);
        app.connected(app.endpoints[0].clone(), state);
        app.target_selected = 1;

        assert_eq!(
            app.handle_key(key(KeyCode::Char('S'))),
            Some(Effect::Run(DashboardAction::Solo {
                selected: "dat".into(),
                others: vec!["daily".into(), "gents".into()],
            }))
        );
        app.complete_action(Ok("done".into()));
        assert_eq!(
            app.handle_key(key(KeyCode::Char('P'))),
            Some(Effect::Run(DashboardAction::ToggleAll {
                targets: vec!["daily".into(), "dat".into(), "gents".into()],
                play: true,
            }))
        );
    }

    #[test]
    fn action_picker_closes_without_moving_dashboard_selection() {
        let mut app = app();
        app.connected(app.endpoints[0].clone(), snapshot(&["daily", "dat"]));
        app.target_selected = 1;
        assert_eq!(app.handle_key(key(KeyCode::Enter)), None);
        assert_eq!(app.action_picker.as_ref().unwrap().target, "dat");
        app.handle_key(key(KeyCode::Left));
        assert_eq!(app.action_picker.as_ref().unwrap().selected, 0);
        app.handle_key(key(KeyCode::Char('l')));
        assert_eq!(app.action_picker.as_ref().unwrap().selected, 1);
        app.handle_key(key(KeyCode::Char('h')));
        assert_eq!(app.action_picker.as_ref().unwrap().selected, 0);
        for _ in 0..10 {
            app.handle_key(key(KeyCode::Right));
        }
        let picker = app.action_picker.as_ref().unwrap();
        assert_eq!(picker.selected, picker.actions.len() - 1);
        app.handle_key(key(KeyCode::Esc));
        assert!(app.action_picker.is_none());
        assert_eq!(app.selected_target().unwrap().name, "dat");
    }

    #[test]
    fn identify_is_a_normal_dashboard_action_and_blocks_duplicates() {
        let mut app = app();
        app.connected(app.endpoints[0].clone(), snapshot(&["daily"]));
        app.handle_key(key(KeyCode::Enter));
        assert_eq!(app.handle_key(key(KeyCode::Char('I'))), None);
        app.handle_key(key(KeyCode::Esc));
        app.handle_key(key(KeyCode::Tab));
        assert_eq!(app.handle_key(key(KeyCode::Char('I'))), None);
        app.handle_key(key(KeyCode::Tab));
        assert_eq!(
            app.handle_key(key(KeyCode::Char('I'))),
            Some(Effect::Run(DashboardAction::Identify))
        );
        assert_eq!(app.handle_key(key(KeyCode::Char('I'))), None);
        app.complete_action(Ok("Identified 1 targets".into()));
        app.handle_key(key(KeyCode::Right));
        assert_eq!(app.handle_key(key(KeyCode::Char('I'))), None);
    }

    #[test]
    fn dashboard_completed_status_clears_on_next_normal_interaction() {
        let mut app = app();
        app.connected(app.endpoints[0].clone(), snapshot(&["daily", "dat"]));
        app.complete_action(Ok("Started DAILY".into()));
        assert!(app.status.is_some());

        app.handle_key(key(KeyCode::Down));

        assert!(app.status.is_none());
        assert_eq!(app.selected_target().unwrap().name, "dat");
    }

    #[test]
    fn actions_clear_stale_status_and_rename_preserves_input_on_error() {
        let mut app = app();
        let mut state = snapshot(&["daily"]);
        state.targets[0].stopped = true;
        state.targets[0].online = false;
        app.connected(app.endpoints[0].clone(), state);
        app.status = Some(StatusLine {
            text: "Started DAILY".into(),
            error: false,
            busy: false,
        });

        app.handle_key(key(KeyCode::Enter));
        assert!(app.status.is_none());
        app.handle_key(key(KeyCode::Down));
        app.handle_key(key(KeyCode::Down));
        app.handle_key(key(KeyCode::Enter));
        for character in "movies".chars() {
            app.handle_key(key(KeyCode::Char(character)));
        }
        app.handle_key(key(KeyCode::Enter));
        assert_eq!(
            app.handle_key(key(KeyCode::Char('y'))),
            Some(Effect::Run(DashboardAction::Rename {
                target: "daily".into(),
                replacement: "movies".into(),
            }))
        );

        app.complete_action(Err("target already exists".into()));
        let prompt = app.management_prompt.as_ref().unwrap();
        assert_eq!(prompt.input, "movies");
        assert_eq!(prompt.stage, ManagementStage::Editing);
        assert_eq!(prompt.error.as_deref(), Some("target already exists"));
    }

    #[test]
    fn set_channel_is_available_for_every_target_and_only_running_targets_ask_restart() {
        let mut disabled_app = app();
        let mut state = snapshot(&["daily"]);
        state.targets[0].disabled = true;
        state.targets[0].online = false;
        disabled_app.connected(disabled_app.endpoints[0].clone(), state);
        disabled_app.handle_key(key(KeyCode::Enter));
        disabled_app.handle_key(key(KeyCode::Down));
        disabled_app.handle_key(key(KeyCode::Down));
        assert_eq!(
            disabled_app.handle_key(key(KeyCode::Enter)),
            Some(Effect::LoadChannels)
        );
        disabled_app.channels_loaded(Ok(vec!["music.m3u".into()]));
        assert_eq!(
            disabled_app.handle_key(key(KeyCode::Char('1'))),
            Some(Effect::Run(DashboardAction::SetChannel {
                target: "daily".into(),
                channel: "music.m3u".into(),
                restart: false,
            }))
        );

        let mut app = app();
        app.connected(app.endpoints[0].clone(), snapshot(&["daily"]));
        app.handle_key(key(KeyCode::Enter));
        for _ in 0..3 {
            app.handle_key(key(KeyCode::Down));
        }
        assert_eq!(
            app.handle_key(key(KeyCode::Enter)),
            Some(Effect::LoadChannels)
        );
        app.channels_loaded(Ok(vec!["music.m3u".into()]));
        assert_eq!(app.handle_key(key(KeyCode::Enter)), None);
        assert_eq!(
            app.channel_prompt.as_ref().unwrap().stage,
            ChannelStage::Restart
        );
        assert_eq!(
            app.handle_key(key(KeyCode::Char('y'))),
            Some(Effect::Run(DashboardAction::SetChannel {
                target: "daily".into(),
                channel: "music.m3u".into(),
                restart: true,
            }))
        );
    }

    #[test]
    fn successful_rename_follows_the_replacement_identity() {
        let mut app = app();
        let mut state = snapshot(&["daily", "other"]);
        state.targets[0].stopped = true;
        state.targets[0].online = false;
        app.connected(app.endpoints[0].clone(), state);
        app.management_prompt = Some(ManagementPrompt {
            action: TargetAction::Rename,
            target: "daily".into(),
            input: "movies".into(),
            stage: ManagementStage::Confirming,
            selected: 0,
            error: None,
            busy: true,
        });
        app.operation_busy = true;

        app.complete_action(Ok("Renamed DAILY to MOVIES".into()));
        app.apply_snapshot(snapshot(&["other", "movies"]));
        assert_eq!(app.selected_target().unwrap().name, "movies");
        assert!(app.management_prompt.is_none());
    }

    #[test]
    fn stopped_target_remove_and_duplicate_are_explicit_confirmed_actions() {
        let mut app = app();
        let mut state = snapshot(&["daily"]);
        state.targets[0].stopped = true;
        state.targets[0].online = false;
        app.connected(app.endpoints[0].clone(), state);

        app.handle_key(key(KeyCode::Enter));
        for _ in 0..3 {
            app.handle_key(key(KeyCode::Down));
        }
        app.handle_key(key(KeyCode::Enter));
        assert_eq!(
            app.handle_key(key(KeyCode::Char('y'))),
            Some(Effect::Run(DashboardAction::Remove {
                target: "daily".into()
            }))
        );
        app.complete_action(Err("remove failed".into()));
        assert_eq!(
            app.management_prompt
                .as_ref()
                .and_then(|prompt| prompt.error.as_deref()),
            Some("remove failed")
        );
        app.return_to_actions();

        app.handle_key(key(KeyCode::Down));
        app.handle_key(key(KeyCode::Down));
        app.handle_key(key(KeyCode::Enter));
        for character in "daily-copy".chars() {
            app.handle_key(key(KeyCode::Char(character)));
        }
        app.handle_key(key(KeyCode::Enter));
        assert_eq!(
            app.management_prompt.as_ref().unwrap().stage,
            ManagementStage::ChoosingMode
        );
        app.handle_key(key(KeyCode::Down));
        app.handle_key(key(KeyCode::Down));
        app.handle_key(key(KeyCode::Enter));
        assert_eq!(
            app.handle_key(key(KeyCode::Char('y'))),
            Some(Effect::Run(DashboardAction::Duplicate {
                source: "daily".into(),
                target: "daily-copy".into(),
                mode: CreateMode::Start,
            }))
        );
    }

    #[test]
    fn command_and_passthrough_preserve_production_transitions() {
        let mut app = app();
        app.connected(app.endpoints[0].clone(), snapshot(&["daily"]));
        assert_eq!(app.handle_key(key(KeyCode::Tab)), None);
        assert_eq!(app.input_mode, Mode::Command);
        assert_eq!(
            app.handle_key(key(KeyCode::Down)),
            Some(Effect::Run(DashboardAction::Command {
                target: "daily".into(),
                action: command::Action::Seek(-60.0),
            }))
        );
        app.complete_action(Ok("done".into()));
        assert!(app.status.is_none());

        app.handle_key(key(KeyCode::Char('\\')));
        assert_eq!(app.input_mode, Mode::Passthrough);
        assert_eq!(
            app.handle_key(key(KeyCode::Char('v'))),
            Some(Effect::Run(DashboardAction::Command {
                target: "daily".into(),
                action: command::Action::Keypress("v".into()),
            }))
        );
        app.complete_action(Ok("done".into()));
        app.handle_key(key(KeyCode::Char('\\')));
        assert_eq!(app.input_mode, Mode::Command);
        app.handle_key(key(KeyCode::Tab));
        assert_eq!(app.input_mode, Mode::Normal);
    }

    #[test]
    fn unavailable_target_does_not_enter_command_mode() {
        let mut app = app();
        let mut state = snapshot(&["daily"]);
        state.targets[0].online = false;
        app.connected(app.endpoints[0].clone(), state);
        app.handle_key(key(KeyCode::Tab));
        assert_eq!(app.input_mode, Mode::Normal);
        assert!(app.status.as_ref().is_some_and(|status| status.error));
    }

    #[test]
    fn new_target_collects_independent_base_startup_state() {
        let mut app = app();
        app.connected(app.endpoints[0].clone(), snapshot(&["daily"]));
        app.handle_key(key(KeyCode::Char('N')));
        for character in "monkey".chars() {
            app.handle_key(key(KeyCode::Char(character)));
        }
        app.handle_key(key(KeyCode::Enter));
        assert_eq!(
            app.handle_key(key(KeyCode::Char('y'))),
            Some(Effect::LoadChannels)
        );
        app.channels_loaded(Ok(vec!["one.m3u".into(), "two.m3u".into()]));
        app.handle_key(key(KeyCode::Down));
        app.handle_key(key(KeyCode::Enter));
        app.handle_key(key(KeyCode::Char('y')));
        app.handle_key(key(KeyCode::Char('n')));
        app.handle_key(key(KeyCode::Down));
        app.handle_key(key(KeyCode::Down));
        app.handle_key(key(KeyCode::Enter));
        assert_eq!(
            app.handle_key(key(KeyCode::Char('y'))),
            Some(Effect::Run(DashboardAction::Create {
                target: "monkey".into(),
                channel: Some("two.m3u".into()),
                play_on_start: true,
                mute_on_start: false,
                mode: CreateMode::Start,
            }))
        );
    }

    #[test]
    fn new_target_channel_picker_accepts_its_display_number() {
        let mut app = app();
        app.connected(app.endpoints[0].clone(), snapshot(&["daily"]));
        app.new_target_prompt = Some(NewTargetPrompt {
            input: "monkey".into(),
            stage: NewTargetStage::ChannelPicker,
            channels: vec!["one.m3u".into(), "two.m3u".into()],
            search: Default::default(),
            channel: None,
            selected: 0,
            play_on_start: false,
            mute_on_start: true,
            mode_selected: 0,
            error: None,
            busy: false,
        });
        assert_eq!(app.handle_key(key(KeyCode::Char('2'))), None);
        let prompt = app.new_target_prompt.as_ref().unwrap();
        assert_eq!(prompt.channel.as_deref(), Some("two.m3u"));
        assert_eq!(prompt.stage, NewTargetStage::PlayQuestion);
    }
}
