use crate::{
    app::{APP_NAME, Action, AppPaths, Failure},
    logger::Logger,
    system::{self, Context},
    workflow,
};
use crossterm::{
    cursor,
    event::{
        self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind, MouseButton,
        MouseEventKind,
    },
    execute,
    terminal::{self, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    Frame, Terminal,
    backend::CrosstermBackend,
    layout::{Constraint, Layout, Position, Rect},
    style::{Color, Modifier, Style},
    widgets::{Block, Borders, Paragraph, Wrap},
};
use std::{
    io,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Sender},
    },
    thread,
};

pub enum WorkerEvent {
    ServerUpdateCheck(Result<bool, String>),
    LauncherCheck(Result<Option<crate::self_update::Release>, String>),
    Log(String),
    Finished(Result<String, Failure>),
    UpdateCheck(Result<crate::versions::Revision, String>),
    VersionList(
        crate::versions::Target,
        Result<Vec<crate::versions::Revision>, String>,
    ),
}
#[derive(Clone, Debug, PartialEq, Eq)]
enum Screen {
    ServerUpdateNotice,
    CheckingServerUpdate,
    LauncherNotice(String),
    Server,
    Console,
    UpdateNotice(crate::versions::Revision),
    LoadingVersions(crate::versions::Target),
    Versions(crate::versions::Target, Vec<crate::versions::Revision>),
    Welcome,
    Installation,
    Home,
    Manage,
    Info,
    HelpTopic(usize),
    Confirm(Action),
    Busy(Action),
    Result(Action, String),
}
#[derive(Debug, PartialEq, Eq)]
enum Effect {
    CheckServerUpdate,
    StartServer,
    ServerCommand(String),
    StopServer,
    LoadVersions(crate::versions::Target),
    SelectVersion(crate::versions::Target, Option<crate::versions::Revision>),
    None,
    Start(Action),
    Cancel,
    Quit,
}
#[derive(Clone, Copy)]
enum Choice {
    StartExistingServer,
    Server,
    Console,
    StopServer,
    Versions(crate::versions::Target),
    PickVersion(usize),
    Action(Action),
    Manage,
    Info,
    Back,
    Confirm(Action),
    Cancel,
    Quit,
    Installation,
    HelpTopic(usize),
    Details,
}
const DELETE_ACK: &str = "DELETE MY DATA";

pub struct Model {
    deletion_ack: String,
    show_details: bool,
    server_update_available: bool,
    server_update_failed: bool,
    server_only: bool,
    server_running: bool,
    server_output: String,
    server_input: String,
    follow_console: bool,
    available_update: Option<crate::versions::Revision>,
    launcher_update: Option<String>,
    screen: Screen,
    selected: usize,
    hover: Option<usize>,
    pressed: Option<usize>,
    buttons: Vec<Rect>,
    body: Rect,
    scroll: usize,
    logs: Vec<String>,
    installed: bool,
    existing: bool,
    recovery: bool,
    backup: bool,
    ready: bool,
    cancel_requested: bool,
    usable: bool,
    history: Vec<(Screen, usize)>,
}
impl Model {
    fn new(paths: &AppPaths, selection: Option<Action>) -> Self {
        let mut model = Self {
            deletion_ack: String::new(),
            show_details: false,
            server_update_available: false,
            server_update_failed: false,
            server_only: paths.data_dir.join("server-only").exists(),
            server_running: false,
            server_output: String::new(),
            server_input: String::new(),
            follow_console: true,
            available_update: None,
            launcher_update: None,
            screen: if paths.onboarding_complete() {
                Screen::Home
            } else {
                Screen::Welcome
            },
            selected: 0,
            hover: None,
            pressed: None,
            buttons: Vec::new(),
            body: Rect::default(),
            scroll: 0,
            logs: Vec::new(),
            installed: paths.installed(),
            existing: paths.existing_setup(),
            recovery: crate::transaction::Transaction::new(&paths.instance_dir)
                .is_ok_and(|t| t.pending()),
            backup: crate::transaction::Transaction::new(&paths.instance_dir)
                .is_ok_and(|t| t.latest_backup().is_ok()),
            ready: paths.ready(),
            cancel_requested: false,
            usable: true,
            history: Vec::new(),
        };
        if let Some(action) = selection {
            // Flags select an action for review; they cannot start it.
            model.screen = Screen::Confirm(action);
        }
        model
    }
    fn choices(&self) -> Vec<(String, Choice)> {
        let choice = |action: Action| (action.title().to_owned(), Choice::Action(action));
        match &self.screen {
            Screen::CheckingServerUpdate => vec![("Back".into(), Choice::Back)],
            Screen::ServerUpdateNotice => vec![
                ("Start existing version".into(), Choice::StartExistingServer),
                (
                    "Review update".into(),
                    Choice::Action(Action::ServerInstall),
                ),
                ("Back".into(), Choice::Back),
            ],
            Screen::LauncherNotice(_) => vec![
                ("Later".into(), Choice::Back),
                (
                    "Review update".into(),
                    Choice::Action(Action::LauncherUpdate),
                ),
            ],
            Screen::Console => vec![
                ("Stop server".into(), Choice::StopServer),
                ("Back".into(), Choice::Back),
            ],
            Screen::Server => vec![
                if self.server_running {
                    ("Open console".into(), Choice::Console)
                } else {
                    choice(Action::ServerStart)
                },
                choice(Action::ServerInstall),
                (
                    "Server version".into(),
                    Choice::Versions(crate::versions::Target::Server),
                ),
                ("Back".into(), Choice::Back),
            ],
            Screen::UpdateNotice(_) => vec![
                ("Later".into(), Choice::Back),
                ("Review update".into(), Choice::Action(Action::Update)),
            ],
            Screen::LoadingVersions(_) => vec![("Back".into(), Choice::Back)],
            Screen::Versions(_, versions) => {
                let mut choices = vec![("Latest stable release".into(), Choice::PickVersion(0))];
                choices.extend(
                    versions
                        .iter()
                        .enumerate()
                        .map(|(i, v)| (v.label.clone(), Choice::PickVersion(i + 1))),
                );
                choices.push(("Back".into(), Choice::Back));
                choices
            }
            Screen::Welcome if !self.existing && self.installed => vec![
                (
                    "Use installed launcher".into(),
                    Choice::Action(Action::SkipSetup),
                ),
                choice(Action::Uninstall),
                ("Help".into(), Choice::Info),
                ("Quit".into(), Choice::Quit),
            ],
            Screen::Welcome if !self.existing => vec![
                choice(Action::Portable),
                choice(Action::Install),
                choice(Action::ServerOnly),
                ("Help".into(), Choice::Info),
                ("Quit".into(), Choice::Quit),
            ],
            Screen::Welcome | Screen::Installation => {
                let mut items = if self.existing {
                    vec![
                        choice(Action::SkipSetup),
                        choice(Action::ReinstallKeep),
                        choice(Action::ReinstallClean),
                    ]
                } else {
                    vec![choice(Action::Portable), choice(Action::Install)]
                };
                if self.screen == Screen::Installation && self.existing {
                    items.remove(0);
                }
                if !self.installed && self.existing {
                    items.retain(|(_, c)| !matches!(c, Choice::Action(Action::ReinstallKeep)));
                    items.insert(0, choice(Action::Install));
                }
                if self.installed {
                    items.push(choice(Action::Uninstall));
                }
                items.push(if self.screen == Screen::Welcome {
                    ("Quit".into(), Choice::Quit)
                } else {
                    ("Back".into(), Choice::Back)
                });
                items
            }
            Screen::Home if self.server_only => vec![
                ("Server".into(), Choice::Server),
                choice(Action::AddGame),
                ("Tools & settings".into(), Choice::Manage),
                ("Help & folders".into(), Choice::Info),
                ("Quit".into(), Choice::Quit),
            ],
            Screen::Home => {
                let mut items = vec![
                    choice(if self.ready {
                        Action::Play
                    } else {
                        Action::Setup
                    }),
                    (
                        if self.ready {
                            "Update"
                        } else {
                            "Help & folders"
                        }
                        .into(),
                        if self.ready {
                            Choice::Action(Action::Update)
                        } else {
                            Choice::Info
                        },
                    ),
                    ("Tools & settings".into(), Choice::Manage),
                    ("Help & folders".into(), Choice::Info),
                    ("Quit".into(), Choice::Quit),
                ];
                if !self.ready {
                    items.remove(3);
                }
                items
            }
            Screen::Manage => {
                let mut items = Vec::new();
                if self.backup {
                    items.push(choice(Action::Restore));
                }
                if self.recovery {
                    items.push(choice(Action::Recover));
                }
                items.push((
                    "Game version".into(),
                    Choice::Versions(crate::versions::Target::Game),
                ));
                items.push((
                    "Server version".into(),
                    Choice::Versions(crate::versions::Target::Server),
                ));
                items.push(choice(Action::OfflineAccounts));
                items.push(choice(Action::OnlineAccounts));
                items.push(choice(Action::LauncherUpdate));
                items.push(("Server".into(), Choice::Server));
                items.push(("Installation".into(), Choice::Installation));
                items.push(("Back".into(), Choice::Back));
                items
            }
            Screen::Confirm(action) => vec![
                ("Cancel".into(), Choice::Back),
                (
                    if *action == Action::ReinstallClean {
                        if self.deletion_ack == DELETE_ACK {
                            "Delete game data".into()
                        } else {
                            "Delete game data (locked)".into()
                        }
                    } else {
                        format!("Yes, {}", action.title().to_lowercase())
                    },
                    Choice::Confirm(*action),
                ),
            ],
            Screen::Busy(_) => vec![
                (
                    if self.cancel_requested {
                        "Cancellation requested".into()
                    } else {
                        "Cancel operation".into()
                    },
                    Choice::Cancel,
                ),
                (
                    if self.show_details {
                        "Hide details"
                    } else {
                        "Show details"
                    }
                    .into(),
                    Choice::Details,
                ),
            ],
            Screen::Info => vec![
                ("Playing & updates".into(), Choice::HelpTopic(0)),
                ("Backups & recovery".into(), Choice::HelpTopic(1)),
                ("Your folders".into(), Choice::HelpTopic(2)),
                ("Mouse & keyboard".into(), Choice::HelpTopic(3)),
                ("Back".into(), Choice::Back),
            ],
            Screen::HelpTopic(_) => vec![("Back to Help".into(), Choice::Back)],
            Screen::Result(_, _) => vec![
                ("Back to menu".into(), Choice::Back),
                ("Quit".into(), Choice::Quit),
            ],
        }
    }
    fn change(&mut self, screen: Screen) {
        if screen == Screen::Console {
            self.follow_console = true;
        }
        self.deletion_ack.clear();
        self.show_details = false;
        self.screen = screen;
        self.selected = 0;
        self.hover = None;
        self.pressed = None;
        self.scroll = 0;
        self.buttons.clear();
    }
    fn open(&mut self, screen: Screen) {
        self.history.push((self.screen.clone(), self.selected));
        self.change(screen);
    }
    fn back(&mut self) {
        let (screen, selected) = self.history.pop().unwrap_or((Screen::Home, 0));
        self.change(screen);
        self.selected = selected.min(self.choices().len() - 1);
    }
    fn activate(&mut self, index: usize) -> Effect {
        let Some((_, choice)) = self.choices().get(index).cloned() else {
            return Effect::None;
        };
        self.selected = index;
        match choice {
            Choice::StartExistingServer => return Effect::StartServer,
            Choice::Server => self.open(Screen::Server),
            Choice::Console => self.open(Screen::Console),
            Choice::StopServer => return Effect::StopServer,
            Choice::Versions(target) => {
                self.open(Screen::LoadingVersions(target));
                return Effect::LoadVersions(target);
            }
            Choice::PickVersion(index) => {
                if let Screen::Versions(target, revisions) = &self.screen {
                    let revision = if index == 0 {
                        None
                    } else {
                        revisions.get(index - 1).cloned()
                    };
                    return Effect::SelectVersion(*target, revision);
                }
            }

            Choice::Action(action) => self.open(Screen::Confirm(action)),
            Choice::Details => self.show_details = !self.show_details,
            Choice::Manage => self.open(Screen::Manage),
            Choice::Info => self.open(Screen::Info),
            Choice::HelpTopic(topic) => self.open(Screen::HelpTopic(topic)),
            Choice::Back => self.back(),
            Choice::Installation => self.open(Screen::Installation),
            Choice::Confirm(Action::ServerStart) => {
                self.open(Screen::CheckingServerUpdate);
                return Effect::CheckServerUpdate;
            }
            Choice::Confirm(action) => {
                if action == Action::ReinstallClean && self.deletion_ack != DELETE_ACK {
                    return Effect::None;
                }
                self.logs.clear();
                self.cancel_requested = false;
                self.change(Screen::Busy(action));
                return Effect::Start(action);
            }
            Choice::Cancel => {
                self.cancel_requested = true;
                return Effect::Cancel;
            }
            Choice::Quit => return Effect::Quit,
        }
        Effect::None
    }
    fn event(&mut self, event: Event) -> Effect {
        if !self.usable {
            if let Event::Key(key) = event
                && key.kind == KeyEventKind::Press
                && matches!(key.code, KeyCode::Esc | KeyCode::Char('q' | 'Q'))
            {
                return if matches!(self.screen, Screen::Busy(_)) {
                    Effect::Cancel
                } else {
                    Effect::Quit
                };
            }
            return Effect::None;
        }
        if self.screen == Screen::Console
            && let Event::Key(key) = &event
            && key.kind == KeyEventKind::Press
        {
            match key.code {
                KeyCode::Char(ch) => {
                    if self.server_input.len() < 4096 {
                        self.server_input.push(ch);
                    }
                    return Effect::None;
                }
                KeyCode::Backspace => {
                    self.server_input.pop();
                    return Effect::None;
                }
                KeyCode::Enter => {
                    return Effect::ServerCommand(std::mem::take(&mut self.server_input));
                }
                _ => {}
            }
        }
        if self.screen == Screen::Confirm(Action::ReinstallClean)
            && let Event::Key(key) = &event
            && key.kind == KeyEventKind::Press
        {
            match key.code {
                KeyCode::Char(ch)
                    if !key
                        .modifiers
                        .intersects(event::KeyModifiers::CONTROL | event::KeyModifiers::ALT) =>
                {
                    if self.deletion_ack.len() < 40 {
                        self.deletion_ack.push(ch);
                    }
                    return Effect::None;
                }
                KeyCode::Backspace => {
                    self.deletion_ack.pop();
                    return Effect::None;
                }
                _ => {}
            }
        }
        match event {
            Event::Key(key) if key.kind == KeyEventKind::Press => match key.code {
                KeyCode::Up | KeyCode::BackTab => {
                    self.hover = None;
                    self.selected = self
                        .selected
                        .checked_sub(1)
                        .unwrap_or(self.choices().len() - 1)
                }
                KeyCode::Down | KeyCode::Tab => {
                    self.hover = None;
                    self.selected = (self.selected + 1) % self.choices().len()
                }
                KeyCode::Enter | KeyCode::Char(' ') => {
                    return self.activate(self.hover.unwrap_or(self.selected));
                }
                KeyCode::PageUp => {
                    self.follow_console = false;
                    self.scroll = self.scroll.saturating_sub(5);
                }
                KeyCode::PageDown => self.scroll = self.scroll.saturating_add(5),
                KeyCode::Home => {
                    self.follow_console = false;
                    self.scroll = 0;
                }
                KeyCode::End => {
                    self.follow_console = true;
                    self.scroll = usize::MAX;
                }
                KeyCode::Esc | KeyCode::Char('q' | 'Q') => {
                    if matches!(self.screen, Screen::Busy(_)) {
                        self.cancel_requested = true;
                        return Effect::Cancel;
                    }
                    if matches!(self.screen, Screen::Home | Screen::Welcome) {
                        return Effect::Quit;
                    }
                    self.back();
                }
                _ => {}
            },
            Event::Mouse(mouse) => {
                let point = Position::new(mouse.column, mouse.row);
                let hit = self.buttons.iter().position(|area| area.contains(point));
                match mouse.kind {
                    MouseEventKind::Moved => self.hover = hit,
                    MouseEventKind::Down(MouseButton::Left) => {
                        self.pressed = hit;
                        self.hover = hit;
                    }
                    MouseEventKind::Up(MouseButton::Left) => {
                        let pressed = self.pressed.take();
                        if let Some(index) = hit
                            && hit == pressed
                        {
                            return self.activate(index);
                        }
                    }
                    MouseEventKind::Down(MouseButton::Right) => {
                        if !matches!(self.screen, Screen::Busy(_)) {
                            self.back();
                        }
                    }
                    MouseEventKind::ScrollUp => {
                        if self.body.contains(point) {
                            self.follow_console = false;
                            self.scroll = self.scroll.saturating_sub(3);
                        } else {
                            self.selected = self.selected.saturating_sub(1);
                        }
                    }
                    MouseEventKind::ScrollDown => {
                        if self.body.contains(point) {
                            self.scroll = self.scroll.saturating_add(3);
                        } else {
                            self.selected = (self.selected + 1).min(self.choices().len() - 1);
                        }
                    }
                    _ => {}
                }
            }
            Event::Resize(_, _) => {
                self.hover = None;
                self.pressed = None;
                self.buttons.clear();
            }
            _ => {}
        }
        Effect::None
    }
    fn apply(&mut self, event: WorkerEvent, paths: &AppPaths) {
        match event {
            WorkerEvent::ServerUpdateCheck(result) => {
                if self.screen != Screen::CheckingServerUpdate {
                    return;
                }
                self.server_update_available = result.as_ref().copied().unwrap_or(false);
                self.server_update_failed = result.is_err();
                self.change(Screen::ServerUpdateNotice);
            }
            WorkerEvent::LauncherCheck(Ok(Some(release))) => {
                self.launcher_update = Some(release.tag_name);
            }
            WorkerEvent::LauncherCheck(_) => {}
            WorkerEvent::UpdateCheck(Ok(revision)) => {
                let newer = crate::versions::Versions::read(paths)
                    .ok()
                    .is_some_and(|v| {
                        v.selected(crate::versions::Target::Game).is_none()
                            && v.installed(crate::versions::Target::Game)
                                .is_some_and(|r| r.sha != revision.sha)
                    });
                if newer {
                    self.available_update = Some(revision);
                }
            }
            WorkerEvent::UpdateCheck(Err(_)) => {} // Network failures never prevent offline use.
            WorkerEvent::VersionList(target, result) => {
                if self.screen != Screen::LoadingVersions(target) {
                    return;
                }
                match result {
                    Ok(revisions) => self.change(Screen::Versions(target, revisions)),
                    Err(message) => self.change(Screen::Result(Action::Setup, message)),
                }
            }
            WorkerEvent::Log(line) => {
                self.logs.push(line);
                if self.logs.len() > 1000 {
                    self.logs.drain(..100);
                }
                self.scroll = usize::MAX;
            }
            WorkerEvent::Finished(result) => {
                let action = match self.screen {
                    Screen::Busy(action) => action,
                    _ => return,
                };
                self.server_only = paths.data_dir.join("server-only").exists();
                self.ready = paths.ready();
                self.existing = paths.existing_setup();
                self.recovery = crate::transaction::Transaction::new(&paths.instance_dir)
                    .is_ok_and(|t| t.pending());
                self.backup = crate::transaction::Transaction::new(&paths.instance_dir)
                    .is_ok_and(|t| t.latest_backup().is_ok());
                self.installed = paths.installed();
                let success = result.is_ok();
                let message = match result {
                    Ok(text) => text,
                    Err(error) => error.to_string(),
                };
                self.history.clear();
                if success && matches!(action, Action::ReinstallClean | Action::Uninstall) {
                    self.change(Screen::Welcome);
                } else {
                    self.history.push((
                        if paths.onboarding_complete() {
                            Screen::Home
                        } else {
                            Screen::Welcome
                        },
                        0,
                    ));
                    self.change(Screen::Result(action, message));
                }
            }
        }
    }
}

pub fn run(mut paths: AppPaths, selection: Option<Action>) -> io::Result<()> {
    terminal::enable_raw_mode()?;
    let _cleanup = Cleanup;
    execute!(
        io::stdout(),
        EnterAlternateScreen,
        EnableMouseCapture,
        cursor::Hide
    )?;
    let mut terminal = Terminal::new(CrosstermBackend::new(io::stdout()))?;
    let mut model = Model::new(&paths, selection);
    let (sender, receiver) = mpsc::channel();
    let mut cancel = Arc::new(AtomicBool::new(false));
    let mut worker = None;
    let mut server: Option<crate::server_process::ServerProcess> = None;
    {
        let sender = sender.clone();
        thread::spawn(move || {
            let _ = sender.send(WorkerEvent::LauncherCheck(
                crate::self_update::check().map_err(|e| e.to_string()),
            ));
        });
    }
    if model.ready {
        let sender = sender.clone();
        thread::spawn(move || {
            let _ = sender.send(WorkerEvent::UpdateCheck(
                crate::versions::latest().map_err(|e| e.to_string()),
            ));
        });
    }
    loop {
        if let Some(process) = &mut server {
            let status = process.poll().map_err(io::Error::other)?;
            model.server_output = process.output();
            if model.screen == Screen::Console && model.follow_console {
                model.scroll = usize::MAX;
            }
            model.server_running = status.is_none();
            if let Some(status) = status {
                model
                    .server_output
                    .push_str(&format!("\nServer stopped: {status}"));
                server = None;
            }
        }
        while let Ok(event) = receiver.try_recv() {
            let finished = matches!(event, WorkerEvent::Finished(_));
            if matches!(&event, WorkerEvent::Finished(Ok(_)))
                && matches!(
                    model.screen,
                    Screen::Busy(Action::Install | Action::ReinstallKeep)
                )
            {
                paths = paths.permanent();
            }
            model.apply(event, &paths);
            if finished && let Some(handle) = worker.take() {
                let _: thread::Result<()> = thread::JoinHandle::join(handle);
            }
        }
        if matches!(model.screen, Screen::Home | Screen::Welcome) {
            if let Some(tag) = model.launcher_update.take() {
                model.open(Screen::LauncherNotice(tag));
            } else if let Some(revision) = model.available_update.take() {
                model.open(Screen::UpdateNotice(revision));
            }
        }
        terminal.draw(|frame| draw(frame, &mut model, &paths))?;
        if event::poll(std::time::Duration::from_millis(50))? {
            let effect = model.event(event::read()?);
            match effect {
                Effect::CheckServerUpdate => {
                    let paths = paths.clone();
                    let sender = sender.clone();
                    thread::spawn(move || {
                        let result = (|| -> Result<bool, Failure> {
                            let versions = crate::versions::Versions::read(&paths)?;
                            let desired = match versions.selected(crate::versions::Target::Server) {
                                Some(revision) => revision.clone(),
                                None => crate::versions::latest()?,
                            };
                            Ok(versions
                                .installed(crate::versions::Target::Server)
                                .is_none_or(|installed| installed.sha != desired.sha))
                        })()
                        .map_err(|e| e.to_string());
                        let _ = sender.send(WorkerEvent::ServerUpdateCheck(result));
                    });
                }
                Effect::StartServer => {
                    let result = (|| -> Result<crate::server_process::ServerProcess, Failure> {
                        let folder = paths.data_dir.join("server/active");
                        let java: std::path::PathBuf =
                            serde_json::from_slice(&std::fs::read(folder.join("java-path.json"))?)?;
                        crate::server_process::ServerProcess::start(
                            &java,
                            &folder,
                            &paths.log_dir().join("server-console.log"),
                        )
                    })();
                    match result {
                        Ok(process) => {
                            server = Some(process);
                            model.server_running = true;
                            model.open(Screen::Console);
                        }
                        Err(error) => {
                            model.change(Screen::Result(Action::ServerStart, error.to_string()))
                        }
                    }
                }
                Effect::ServerCommand(command) => {
                    if let Some(process) = &mut server
                        && let Err(error) = process.command(&command)
                    {
                        model.server_output.push_str(&format!("\n{error}"));
                    }
                }
                Effect::StopServer => {
                    if let Some(process) = &mut server {
                        let _ = process.stop();
                    }
                }
                Effect::LoadVersions(target) => {
                    let sender = sender.clone();
                    thread::spawn(move || {
                        let result = crate::versions::catalogue().map_err(|e| e.to_string());
                        let _ = sender.send(WorkerEvent::VersionList(target, result));
                    });
                }
                Effect::SelectVersion(target, revision) => {
                    let result = (|| -> Result<(), Failure> {
                        let _lock = system::lock(&paths.data_dir)?;
                        let mut versions = crate::versions::Versions::read(&paths)?;
                        versions.select(target, revision)?;
                        versions.save(&paths)
                    })();
                    model.change(Screen::Result(
                        Action::Setup,
                        match result {
                            Ok(()) => "Version selected. Nothing has been downloaded yet.".into(),
                            Err(error) => error.to_string(),
                        },
                    ));
                }
                Effect::Start(action) => {
                    cancel = Arc::new(AtomicBool::new(false));
                    worker = Some(start_worker(
                        action,
                        paths.clone(),
                        sender.clone(),
                        Arc::clone(&cancel),
                    ));
                }
                Effect::Cancel => cancel.store(true, Ordering::Relaxed),
                Effect::Quit => {
                    if server.is_some() {
                        model.open(Screen::Console);
                        model
                            .server_output
                            .push_str("\nStop the server before closing the launcher.");
                    } else {
                        break;
                    }
                }
                Effect::None => {}
            }
        }
    }
    Ok(())
}
fn start_worker(
    action: Action,
    paths: AppPaths,
    events: Sender<WorkerEvent>,
    cancel: Arc<AtomicBool>,
) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        let result = std::panic::catch_unwind(|| {
            system::no_links(&paths.data_dir)?;
            let logger = Arc::new(Logger::new(&paths.log_dir(), Some(events.clone()))?);
            logger.info(format!("Log: {}", logger.path().display()));
            let ctx = Context {
                logger,
                cancelled: cancel,
            };
            workflow::execute(action, &paths, &ctx)
        })
        .unwrap_or_else(|_| {
            Err(Failure::plain(
                "The worker stopped unexpectedly. Return to the menu and use Recover if needed.",
            ))
        });
        let _ = events.send(WorkerEvent::Finished(result));
    })
}
fn draw(frame: &mut Frame<'_>, model: &mut Model, paths: &AppPaths) {
    let full = frame.area();
    let width = full.width.min(76);
    let area = Rect::new(
        full.x + (full.width - width) / 2,
        full.y,
        width,
        full.height,
    );
    let destructive = model.screen == Screen::Confirm(Action::ReinstallClean);
    let first_setup = model.screen == Screen::Welcome;
    let accent = if destructive {
        Color::LightRed
    } else if first_setup {
        Color::LightCyan
    } else {
        Color::Yellow
    };
    model.usable = area.width >= 40 && area.height >= if destructive { 22 } else { 12 };
    if !model.usable {
        model.buttons.clear();
        frame.render_widget(Paragraph::new("Resize to at least 40 columns by 22 rows. Q exits, or cancels a running operation.").wrap(Wrap { trim: true }), area);
        return;
    }
    let all_choices = model.choices();
    let visible_count = ((area.height.saturating_sub(9)) / 2).max(2) as usize;
    let first_choice = if model.selected >= visible_count {
        model.selected + 1 - visible_count
    } else {
        0
    };
    let choices: Vec<_> = all_choices
        .iter()
        .skip(first_choice)
        .take(visible_count)
        .cloned()
        .collect();
    let menu_screen = matches!(
        model.screen,
        Screen::Server
            | Screen::Welcome
            | Screen::Installation
            | Screen::Home
            | Screen::Manage
            | Screen::Info
            | Screen::Versions(_, _)
    );
    let compact = area.height < 30;
    let button_height = if !compact {
        3
    } else if area.height >= 22 {
        2
    } else {
        1
    };
    let menu_height = (choices.len() as u16 * button_height).min(area.height.saturating_sub(5));
    let chunks = Layout::vertical([
        Constraint::Length(3),
        Constraint::Length(if menu_screen {
            menu_height
        } else {
            if matches!(model.screen, Screen::Console) {
                area.height.saturating_sub(menu_height + 5)
            } else if destructive {
                10
            } else {
                7
            }
            .min(area.height.saturating_sub(menu_height + 5))
        }),
        Constraint::Min(1),
        Constraint::Length(2),
    ])
    .split(area);
    frame.render_widget(
        Paragraph::new(format!(
            "{}  /  {}",
            APP_NAME,
            match model.screen {
                Screen::CheckingServerUpdate => "Checking server updates",
                Screen::ServerUpdateNotice => "Server version check",
                Screen::LauncherNotice(_) => "Launcher update",
                Screen::Server => "Server",
                Screen::Console => "Server console",
                Screen::UpdateNotice(_) => "Update available",
                Screen::LoadingVersions(_) => "Checking versions",
                Screen::Versions(crate::versions::Target::Game, _) => "Game version",
                Screen::Versions(crate::versions::Target::Server, _) => "Server version",
                Screen::Welcome =>
                    if model.existing {
                        "Setup / Welcome back"
                    } else {
                        "SETUP 1/2"
                    },
                Screen::Installation => "Installation",
                Screen::Home =>
                    if model.installed {
                        "MAIN MENU / Installed"
                    } else {
                        "MAIN MENU / Portable"
                    },
                Screen::Manage => "Tools & settings",
                Screen::Info => "Help",
                Screen::HelpTopic(_) => "Help",
                Screen::Confirm(Action::ReinstallClean) => "Delete game data?",
                Screen::Confirm(_) => "Continue?",
                Screen::Busy(_) => "Working",
                Screen::Result(_, _) => "Result",
            }
        ))
        .style(Style::default().fg(accent).add_modifier(Modifier::BOLD))
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_type(if first_setup {
                    ratatui::widgets::BorderType::Double
                } else {
                    ratatui::widgets::BorderType::Plain
                }),
        ),
        chunks[0],
    );
    let menu_area = if menu_screen { chunks[1] } else { chunks[2] };
    let detail_area = if menu_screen { chunks[2] } else { chunks[1] };
    let body = match &model.screen {
        Screen::CheckingServerUpdate => "Checking for updates before starting.".into(),
        Screen::ServerUpdateNotice => if model.server_update_failed { "Could not check for updates. You can still start the installed server.".into() } else if model.server_update_available { "A newer or different server version is available. Update it or start the installed version.".into() } else { "The installed server matches your selected version.".into() },
        Screen::LauncherNotice(version) => format!("Launcher {version} is available.\n{}", crate::self_update::RELEASES_URL),
        Screen::Server => "Run a server without opening the game.".into(),
        Screen::Console => model.server_output.clone(),
        Screen::UpdateNotice(_) => "A newer BeeWorld pack is available. Install it now or keep playing this version.".into(),
        Screen::LoadingVersions(_) => "Checking published versions. Please wait.".into(),
        Screen::Versions(_, versions) => if versions.is_empty() { "No stable releases published yet. Your installed version is kept.".into() } else { "Choose a stable release. Installation requires a separate confirmation.".into() },
        Screen::Welcome | Screen::Installation => if model.existing {
            "Your existing game was found.".to_owned()
        } else {
            "Install adds shortcuts on this PC.\nPortable runs without installing.".to_owned()
        },
        Screen::Home | Screen::Manage => {
            let index = model.hover.unwrap_or(model.selected);
            let hint = match all_choices.get(index).map(|(_, c)| c) {
                Some(Choice::Action(Action::Setup)) => {
                    "Download the game. Internet required."
                }
                Some(Choice::Action(Action::Play)) => {
                    "Open BeeWorld."
                }
                Some(Choice::Action(Action::Update)) => {
                    "Update your game and keep a backup."
                }
                Some(Choice::Action(Action::Restore)) => {
                    "Restore the game and worlds from a backup."
                }
                Some(Choice::Action(Action::Recover)) => {
                    "Fix an interrupted update."
                }
                Some(Choice::Manage) => "Backups and launcher settings.",
                Some(Choice::Info) => "Instructions and game folders.",
                Some(Choice::Installation) => "Install, reinstall or remove the launcher.",
                Some(Choice::Back) => "Return to the previous screen.",
                _ => "Close the launcher.",
            };
            hint.to_owned()
        }
        Screen::Info => "Choose what you need help with.".to_owned(),
        Screen::HelpTopic(topic) => match topic {
            0 => "PLAYING & UPDATES\n\nPlay opens your current pack. It does not update it.\n\nOn your first launch, sign in through Prism. Prism downloads Java and any missing Minecraft files.\n\nChoose Update when you want the newest pack. Close Minecraft and Prism first. Initial downloads need internet.".to_owned(),
            1 => "BACKUPS & RECOVERY\n\nBefore updating, BeeWorld keeps a complete copy of your instance. Allow space for a second copy.\n\nRestore backup returns to that version, including the worlds and settings saved then.\n\nIf an update was interrupted, use Repair interrupted update. Your existing backups are kept.".to_owned(),
            2 => format!("YOUR FOLDERS\n\nGame data and downloaded tools\n{}\n\nMinecraft instance\n{}\n\nLogs\n{}\n\nSaved data may remain here after uninstall. This does not mean the launcher is installed. Do not share account files.", paths.data_dir.display(), paths.instance_dir.display(), paths.log_dir().display()),
            _ => "CONTROLS\n\nMouse\nClick a button to choose it. Scroll longer text with the wheel. Right-click goes back.\n\nKeyboard\nTab or arrows: choose a button\nEnter: open it\nEsc: go back\nPage Up / Page Down: scroll text\n\nActions ask for confirmation. Cancel is selected first.".to_owned(),
        },
        Screen::Confirm(Action::ReinstallClean) => format!(
            "Your BeeWorld worlds, saved sign-ins, settings\nand backups will be deleted.\nThis cannot be undone.\n\nType DELETE MY DATA to unlock deletion:\n> {}_", model.deletion_ack
        ),
        Screen::Confirm(action) => action.description(paths),
        Screen::Busy(_) => if model.show_details { model.logs.join("\n") } else if model.cancel_requested {
            "Stopping safely. Please wait.".into()
        } else { "Working. Please wait.".into() },
        Screen::Result(action, message) => format!("{}\n\n{}", action.title(), message),
    };
    // Wrap explicitly so wheel scrolling uses display rows, including long paths.
    let inner = detail_area.inner(ratatui::layout::Margin::new(1, 1));
    let lines = wrap_lines(&body, inner.width.max(1) as usize);
    let max_scroll = lines.len().saturating_sub(inner.height as usize);
    model.scroll = model.scroll.min(max_scroll);
    let text = lines
        .iter()
        .skip(model.scroll)
        .take(inner.height as usize)
        .cloned()
        .collect::<Vec<_>>()
        .join("\n");
    frame.render_widget(Paragraph::new(text), inner);
    model.body = detail_area;
    let rows =
        Layout::vertical(vec![Constraint::Length(button_height); choices.len()]).split(menu_area);
    model.buttons = vec![Rect::default(); all_choices.len()];
    for (index, rect) in rows.iter().enumerate() {
        model.buttons[first_choice + index] = *rect;
    }
    for (offset, ((label, _), rect)) in choices.iter().zip(rows.iter()).enumerate() {
        let index = first_choice + offset;
        let focused = index == model.hover.unwrap_or(model.selected);
        let hovered = model.hover == Some(index);
        let locked = destructive && index == 1 && model.deletion_ack != DELETE_ACK;
        let style = if locked {
            Style::default().fg(Color::DarkGray)
        } else if model.pressed == Some(index) {
            Style::default().fg(Color::Black).bg(Color::White)
        } else if hovered || focused {
            Style::default().fg(accent).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::White)
        };
        let button =
            Paragraph::new(format!(" {} {label}", if focused { ">" } else { " " })).style(style);

        frame.render_widget(button, *rect);
    }
    frame.render_widget(
        Paragraph::new(if model.screen == Screen::Console {
            format!("Command: {}_", model.server_input)
        } else {
            "Enter to choose  /  Esc to go back".into()
        })
        .style(Style::default().fg(Color::DarkGray))
        .wrap(Wrap { trim: false }),
        chunks[3],
    );
}
fn wrap_lines(text: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let mut result = Vec::new();
    for line in text.lines() {
        if line.is_empty() {
            result.push(String::new());
            continue;
        }
        let mut row = String::new();
        for word in line.split_whitespace() {
            if !row.is_empty() && row.chars().count() + 1 + word.chars().count() > width {
                result.push(std::mem::take(&mut row));
            }
            if !row.is_empty() {
                row.push(' ');
            }
            for ch in word.chars() {
                if row.chars().count() == width {
                    result.push(std::mem::take(&mut row));
                }
                row.push(ch);
            }
        }
        if !row.is_empty() {
            result.push(row);
        }
    }
    result
}
struct Cleanup;
impl Drop for Cleanup {
    fn drop(&mut self) {
        let _ = execute!(
            io::stdout(),
            DisableMouseCapture,
            cursor::Show,
            LeaveAlternateScreen
        );
        let _ = terminal::disable_raw_mode();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyEvent, KeyModifiers, MouseEvent};
    fn paths(root: &std::path::Path) -> AppPaths {
        AppPaths {
            executable: root.join("Bee.exe"),
            data_dir: root.join("data"),
            instance_dir: root.join("data/prism/instances/BeeWorld"),
            prism_root: root.join("data/prism"),
            install_dir: root.join("installed"),
        }
    }
    fn key(code: KeyCode) -> Event {
        Event::Key(KeyEvent::new(code, KeyModifiers::NONE))
    }
    fn mouse(kind: MouseEventKind, column: u16, row: u16) -> Event {
        Event::Mouse(MouseEvent {
            kind,
            column,
            row,
            modifiers: KeyModifiers::NONE,
        })
    }
    fn render(model: &mut Model, paths: &AppPaths) -> String {
        let backend = ratatui::backend::TestBackend::new(80, 30);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|frame| draw(frame, model, paths)).unwrap();
        terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect()
    }
    #[test]
    fn deletion_requires_exact_typing_and_a_separate_confirmation() {
        let temp = tempfile::tempdir().unwrap();
        let paths = paths(temp.path());
        let mut model = Model::new(&paths, Some(Action::ReinstallClean));
        assert_eq!(model.activate(1), Effect::None);
        for ch in "DELETE MY DATx".chars() {
            assert_eq!(model.event(key(KeyCode::Char(ch))), Effect::None);
        }
        assert_eq!(model.activate(1), Effect::None);
        model.event(key(KeyCode::Backspace));
        assert_eq!(model.event(key(KeyCode::Char('A'))), Effect::None);
        assert_eq!(model.screen, Screen::Confirm(Action::ReinstallClean));
        assert_eq!(
            model.event(key(KeyCode::Enter)),
            Effect::Start(Action::ReinstallClean)
        );
        model.change(Screen::Confirm(Action::ReinstallClean));
        assert!(model.deletion_ack.is_empty());
        assert_eq!(model.activate(1), Effect::None);
    }
    #[test]
    fn deletion_mouse_cannot_bypass_locked_confirmation() {
        let temp = tempfile::tempdir().unwrap();
        let paths = paths(temp.path());
        let mut model = Model::new(&paths, Some(Action::ReinstallClean));
        let text = render(&mut model, &paths);
        assert!(text.contains("Delete game data?"));
        assert!(text.contains("This cannot be undone."));
        let button = model.buttons[1];
        model.event(mouse(
            MouseEventKind::Down(MouseButton::Left),
            button.x,
            button.y,
        ));
        assert_eq!(
            model.event(mouse(
                MouseEventKind::Up(MouseButton::Left),
                button.x,
                button.y
            )),
            Effect::None
        );
        model.event(key(KeyCode::Char('q')));
        assert_eq!(model.deletion_ack, "q");
        model.event(key(KeyCode::Esc));
        assert!(model.deletion_ack.is_empty());
    }
    #[test]
    fn setup_and_main_menu_are_visually_distinct_and_warning_fits_small_terminal() {
        let temp = tempfile::tempdir().unwrap();
        let paths = paths(temp.path());
        let mut model = Model::new(&paths, None);
        assert!(render(&mut model, &paths).contains("SETUP 1/2"));
        model.change(Screen::Home);
        assert!(render(&mut model, &paths).contains("MAIN MENU"));
        model.change(Screen::Confirm(Action::ReinstallClean));
        let mut terminal = Terminal::new(ratatui::backend::TestBackend::new(40, 22)).unwrap();
        terminal.draw(|f| draw(f, &mut model, &paths)).unwrap();
        let text: String = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|c| c.symbol())
            .collect();
        assert!(text.contains("> _"));
        assert!(text.contains("Cancel"));
        assert!(model.buttons.iter().all(|r| r.bottom() <= 22));
        assert_ne!(terminal.backend().buffer()[(1, 1)].bg, Color::Red);
    }
    #[test]
    fn everyday_screens_hide_paths_and_busy_logs_are_opt_in() {
        let temp = tempfile::tempdir().unwrap();
        let paths = paths(temp.path());
        let mut model = Model::new(&paths, Some(Action::ReinstallClean));
        let text = render(&mut model, &paths);
        assert_eq!(text.matches("This cannot be undone.").count(), 1);
        assert!(!text.contains("Instance:"));
        assert!(!text.contains("Data:"));
        assert!(!text.contains("Cancel is selected"));
        model.change(Screen::Busy(Action::Setup));
        model.logs.push("technical diagnostic fixture".into());
        assert!(!render(&mut model, &paths).contains("technical diagnostic fixture"));
        assert_eq!(model.activate(1), Effect::None);
        assert!(render(&mut model, &paths).contains("technical diagnostic fixture"));
    }
    #[test]
    fn orphan_executable_and_old_installed_choice_open_portable_setup() {
        let temp = tempfile::tempdir().unwrap();
        let paths = paths(temp.path()).permanent();
        std::fs::create_dir_all(&paths.data_dir).unwrap();
        std::fs::write(paths.install_dir.join("BeeWorldLauncher.exe"), "leftover").unwrap();
        std::fs::write(
            paths.data_dir.join("launcher-choice.json"),
            r#"{"version":1,"mode":"installed"}"#,
        )
        .unwrap();
        let mut model = Model::new(&paths, None);
        assert_eq!(model.screen, Screen::Welcome);
        assert!(!model.installed);
        assert!(!model.existing);
        assert_eq!(model.choices()[0].0, "Use portable mode");
        std::fs::write(
            paths.data_dir.join("launcher-choice.json"),
            r#"{"version":2,"mode":"portable","had_game":false}"#,
        )
        .unwrap();
        model = Model::new(&paths, None);
        assert_eq!(model.screen, Screen::Home);
        assert!(render(&mut model, &paths).contains("Portable"));
    }
    #[test]
    fn reset_and_uninstall_return_to_setup_in_the_same_session() {
        let temp = tempfile::tempdir().unwrap();
        let paths = paths(temp.path());
        for action in [Action::ReinstallClean, Action::Uninstall] {
            let mut model = Model::new(&paths, None);
            model.change(Screen::Busy(action));
            model.apply(WorkerEvent::Finished(Ok("done".into())), &paths);
            assert_eq!(model.screen, Screen::Welcome);
            assert_eq!(Model::new(&paths, None).screen, Screen::Welcome);
        }
    }
    #[test]
    fn missing_game_and_pending_uninstall_cannot_look_ready_or_installed() {
        let temp = tempfile::tempdir().unwrap();
        let paths = paths(temp.path());
        fs_create_ready(&paths);
        std::fs::write(
            paths.data_dir.join("launcher-choice.json"),
            r#"{"version":2,"mode":"portable","had_game":true}"#,
        )
        .unwrap();
        assert!(paths.onboarding_complete());
        std::fs::remove_file(paths.instance_dir.join("mmc-pack.json")).unwrap();
        assert!(!paths.ready());
        assert!(!paths.onboarding_complete());
        std::fs::create_dir_all(&paths.install_dir).unwrap();
        std::fs::write(paths.install_dir.join("BeeWorldLauncher.exe"), "fixture").unwrap();
        std::fs::write(
            paths.install_dir.join("installation.json"),
            serde_json::to_vec(&crate::install::Installation {
                data_dir: paths.data_dir.clone(),
                instance_dir: paths.instance_dir.clone(),
            })
            .unwrap(),
        )
        .unwrap();
        assert!(paths.installed());
        std::fs::write(paths.install_dir.join("uninstall-pending"), "1").unwrap();
        assert!(!paths.installed());
    }
    #[test]
    fn update_notices_do_not_dispatch_installs_without_confirmation() {
        let temp = tempfile::tempdir().unwrap();
        let paths = paths(temp.path());
        let mut model = Model::new(&paths, None);
        model.change(Screen::ServerUpdateNotice);
        assert_eq!(model.activate(1), Effect::None);
        assert_eq!(model.screen, Screen::Confirm(Action::ServerInstall));
        assert_eq!(model.activate(0), Effect::None);
        assert_eq!(model.screen, Screen::ServerUpdateNotice);
        assert_eq!(model.activate(0), Effect::StartServer);
        model.change(Screen::LauncherNotice("v0.3.0".into()));
        assert_eq!(model.activate(1), Effect::None);
        assert_eq!(model.screen, Screen::Confirm(Action::LauncherUpdate));
        assert_eq!(model.activate(1), Effect::Start(Action::LauncherUpdate));
    }
    #[test]
    fn failed_server_update_check_still_allows_explicit_start() {
        let temp = tempfile::tempdir().unwrap();
        let paths = paths(temp.path());
        let mut model = Model::new(&paths, None);
        model.change(Screen::CheckingServerUpdate);
        model.apply(
            WorkerEvent::ServerUpdateCheck(Err("offline".into())),
            &paths,
        );
        assert_eq!(model.screen, Screen::ServerUpdateNotice);
        assert!(render(&mut model, &paths).contains("Could not check for updates"));
        assert_eq!(model.activate(0), Effect::StartServer);
    }
    #[test]
    fn long_version_list_keeps_selected_button_visible() {
        let temp = tempfile::tempdir().unwrap();
        let paths = paths(temp.path());
        let versions = (0..40)
            .map(|i| crate::versions::Revision {
                sha: format!("{i:040x}"),
                label: format!("Version {i}"),
                date: String::new(),
            })
            .collect();
        let mut model = Model::new(&paths, None);
        model.change(Screen::Versions(crate::versions::Target::Server, versions));
        model.selected = 35;
        let text = render(&mut model, &paths);
        assert!(text.contains("Version 34"));
        assert!(model.buttons[35].height > 0);
        assert!(model.buttons[35].bottom() <= 30);
        assert!(matches!(
            model.activate(35),
            Effect::SelectVersion(crate::versions::Target::Server, Some(_))
        ));
    }
    #[test]
    fn server_setup_never_dispatches_client_setup() {
        let temp = tempfile::tempdir().unwrap();
        let paths = paths(temp.path());
        let mut model = Model::new(&paths, None);
        let index = model
            .choices()
            .iter()
            .position(|(_, c)| matches!(c, Choice::Action(Action::ServerOnly)))
            .unwrap();
        assert_eq!(model.activate(index), Effect::None);
        assert_eq!(model.activate(1), Effect::Start(Action::ServerOnly));
        model.server_only = true;
        model.change(Screen::Home);
        assert!(
            !model
                .choices()
                .iter()
                .any(|(_, c)| matches!(c, Choice::Action(Action::Setup | Action::Play)))
        );
    }
    #[test]
    fn fresh_start_and_flags_never_dispatch_or_create_data() {
        let temp = tempfile::tempdir().unwrap();
        let paths = paths(temp.path());
        for selection in [
            None,
            Some(Action::Install),
            Some(Action::Update),
            Some(Action::Uninstall),
        ] {
            let mut model = Model::new(&paths, selection);
            let text = render(&mut model, &paths);
            assert!(text.contains("BeeWorld Launcher"));
            assert!(!matches!(model.screen, Screen::Busy(_)));
            assert!(!paths.data_dir.exists());
        }
    }
    #[test]
    fn keyboard_requires_separate_confirmation_and_defaults_to_cancel() {
        let temp = tempfile::tempdir().unwrap();
        let mut model = Model::new(&paths(temp.path()), None);
        model.screen = Screen::Home;
        assert_eq!(model.event(key(KeyCode::Enter)), Effect::None);
        assert_eq!(model.screen, Screen::Confirm(Action::Setup));
        assert_eq!(model.event(key(KeyCode::Enter)), Effect::None);
        assert_eq!(model.screen, Screen::Home);
        model.event(key(KeyCode::Enter));
        model.event(key(KeyCode::Tab));
        assert_eq!(
            model.event(key(KeyCode::Enter)),
            Effect::Start(Action::Setup)
        );
    }
    #[test]
    fn mouse_hover_does_not_activate_and_click_requires_matching_release() {
        let temp = tempfile::tempdir().unwrap();
        let paths = paths(temp.path());
        let mut model = Model::new(&paths, None);
        model.screen = Screen::Home;
        render(&mut model, &paths);
        let rect = model.buttons[0];
        assert_eq!(
            model.event(mouse(MouseEventKind::Moved, rect.x, rect.y)),
            Effect::None
        );
        assert_eq!(model.hover, Some(0));
        assert_eq!(model.screen, Screen::Home);
        model.event(mouse(
            MouseEventKind::Down(MouseButton::Left),
            rect.x,
            rect.y,
        ));
        assert_eq!(
            model.event(mouse(MouseEventKind::Up(MouseButton::Left), rect.x, rect.y)),
            Effect::None
        );
        assert_eq!(model.screen, Screen::Confirm(Action::Setup));
        render(&mut model, &paths);
        let confirm = model.buttons[1];
        assert_eq!(
            model.event(mouse(
                MouseEventKind::Up(MouseButton::Left),
                confirm.x,
                confirm.y
            )),
            Effect::None
        );
        model.event(mouse(
            MouseEventKind::Down(MouseButton::Left),
            confirm.x,
            confirm.y,
        ));
        assert_eq!(
            model.event(mouse(
                MouseEventKind::Up(MouseButton::Left),
                confirm.x,
                confirm.y
            )),
            Effect::Start(Action::Setup)
        );
    }
    #[test]
    fn ready_start_still_waits_and_cancel_returns_to_menu() {
        let temp = tempfile::tempdir().unwrap();
        let paths = paths(temp.path());
        fs_create_ready(&paths);
        let mut model = Model::new(&paths, None);
        model.screen = Screen::Home;
        assert_eq!(model.screen, Screen::Home);
        assert_eq!(model.event(key(KeyCode::Enter)), Effect::None);
        assert_eq!(model.screen, Screen::Confirm(Action::Play));
        model.event(key(KeyCode::Esc));
        assert_eq!(model.screen, Screen::Home);
    }
    #[test]
    fn settings_cancel_returns_to_settings_with_selection_preserved() {
        let temp = tempfile::tempdir().unwrap();
        let mut model = Model::new(&paths(temp.path()), None);
        model.screen = Screen::Home;
        model.backup = true;
        model.selected = 2;
        model.activate(2);
        assert_eq!(model.screen, Screen::Manage);
        model.selected = 0;
        model.activate(0);
        assert_eq!(model.screen, Screen::Confirm(Action::Restore));
        model.activate(0);
        assert_eq!(model.screen, Screen::Manage);
        assert_eq!(model.selected, 0);
        model.event(key(KeyCode::Esc));
        assert_eq!(model.screen, Screen::Home);
        assert_eq!(model.selected, 2);
    }
    #[test]
    fn first_run_has_one_setup_choice_and_no_unusable_update_choice() {
        let temp = tempfile::tempdir().unwrap();
        let mut model = Model::new(&paths(temp.path()), None);
        model.screen = Screen::Home;
        let labels: Vec<_> = model
            .choices()
            .into_iter()
            .map(|(label, _)| label)
            .collect();
        assert_eq!(
            labels,
            [
                "Download game",
                "Help & folders",
                "Tools & settings",
                "Quit"
            ]
        );
    }
    #[test]
    fn welcome_requires_confirmation_and_remembers_portable_choice() {
        let temp = tempfile::tempdir().unwrap();
        let paths = paths(temp.path());
        let mut model = Model::new(&paths, None);
        assert_eq!(model.screen, Screen::Welcome);
        assert!(!paths.data_dir.exists());
        assert_eq!(model.activate(0), Effect::None);
        assert_eq!(model.screen, Screen::Confirm(Action::Portable));
        model.activate(0);
        assert_eq!(model.screen, Screen::Welcome);
        std::fs::create_dir_all(&paths.data_dir).unwrap();
        std::fs::write(
            paths.data_dir.join("launcher-choice.json"),
            r#"{"mode":"portable","version":2,"had_game":false}"#,
        )
        .unwrap();
        assert_eq!(Model::new(&paths, None).screen, Screen::Home);
    }
    #[test]
    fn existing_setup_offers_reuse_both_reinstall_modes_and_uninstall() {
        let temp = tempfile::tempdir().unwrap();
        let paths = paths(temp.path());
        fs_create_ready(&paths);
        std::fs::create_dir_all(&paths.install_dir).unwrap();
        std::fs::write(
            paths.install_dir.join("installation.json"),
            serde_json::to_vec(&crate::install::Installation {
                data_dir: paths.data_dir.clone(),
                instance_dir: paths.instance_dir.clone(),
            })
            .unwrap(),
        )
        .unwrap();
        std::fs::write(paths.install_dir.join("BeeWorldLauncher.exe"), "fixture").unwrap();
        let model = Model::new(&paths, None);
        assert_eq!(model.screen, Screen::Welcome);
        let labels: Vec<_> = model
            .choices()
            .into_iter()
            .map(|(label, _)| label)
            .collect();
        assert_eq!(
            labels,
            [
                "Use existing setup",
                "Reinstall - keep my data",
                "Start over - delete my data",
                "Uninstall launcher",
                "Quit"
            ]
        );
    }
    #[test]
    fn help_topics_return_to_help_and_unavailable_tools_are_hidden() {
        let temp = tempfile::tempdir().unwrap();
        let paths = paths(temp.path());
        let mut model = Model::new(&paths, None);
        model.open(Screen::Info);
        model.activate(2);
        assert_eq!(model.screen, Screen::HelpTopic(2));
        model.activate(0);
        assert_eq!(model.screen, Screen::Info);
        assert_eq!(model.selected, 2);
        model.change(Screen::Manage);
        assert_eq!(model.choices().len(), 8);
    }
    fn fs_create_ready(paths: &AppPaths) {
        std::fs::create_dir_all(&paths.instance_dir).unwrap();
        std::fs::write(paths.instance_dir.join("mmc-pack.json"), "{}").unwrap();
        std::fs::create_dir_all(&paths.data_dir).unwrap();
        std::fs::write(paths.data_dir.join("setup-complete"), "1").unwrap();
    }
}
