//! The interactive title before world resources exist. Uses only the menu
//! atlas/panorama; transfers the actual screen and queued entry to Game.
use super::event_loop::EventLoopCtl;
use super::key_focus::{self, Act};
use super::*;
use horizon_client::{
    clipboard::{self, AppendSelection, SystemClipboard},
    options_ui::{Event, OptionsUi, Page},
    widgets::{Kind, Target},
};
use winit::{event::KeyEvent, keyboard::ModifiersState};

#[derive(Default)]
pub(super) struct StartupMenu {
    title: Option<Title>,
    options: OptionsUi,
    options_open: bool,
    /// The credits from Credits & Attribution, and their last frame.
    win: Option<(horizon_client::win_screen::WinScreen, Instant)>,
    root: PathBuf,
    cursor: (f32, f32),
    pointer: (f64, f64),
    modifiers: ModifiersState,
    physical_keys: HashSet<KeyCode>,
    dragging: bool,
    clipboard: SystemClipboard,
    selection: AppendSelection,
    /// The narrator's `NARRATOR_TOGGLE` toast, handed to the game with the title.
    narrator_toast: Option<horizon_client::toasts::SystemToast>,
    /// The screens' narration, handed to the game with the title (which
    /// stays the screen already opened).
    narration: horizon_client::screen_narrator::ScreenNarrator,
    /// Frames drawn, for `widgets::end_frame`.
    frames: u64,
    /// The keyboard's focus (`key_focus`: Tab, the arrows, Enter); the
    /// pointer sits on it, as in the game.
    key_focus: Option<Target>,
    /// The focused slider's arrows change its value (`canChangeValue`).
    key_slider_edit: bool,
    /// What the last click landed on, for the next Tab or arrow.
    key_origin: Option<[f32; 4]>,
    /// The focused slider being nudged and where the keyboard put its handle.
    key_nudge: Option<([f32; 4], f32)>,
    /// A click the keyboard sends is being handled.
    key_synthetic: bool,
    /// The screen the keyboard's focus is on (`key_screen`).
    key_screen: u64,
    pub pending: Option<PendingWorldEntry>,
    pub reload_packs: bool,
    pub perf: horizon_client::telemetry::Session,
}

impl StartupMenu {
    pub fn new(options: &Options, pack_error: Option<&str>) -> Result<Self> {
        let pending = if let Some(address) = &options.server {
            Some(PendingWorldEntry::Server {
                address: address.clone(),
                name: options.player_name(),
                cookies: None,
                lan: false,
            })
        } else {
            options
                .world
                .as_ref()
                .map(|folder| PendingWorldEntry::Local {
                    folder: folder.clone(),
                    name: None,
                    seed: Some(options.seed),
                    creative: options.creative,
                    create: None,
                })
        };
        let mut menu = Self {
            pending,
            ..Default::default()
        };
        menu.root = options.resources.clone();
        if let Some(error) = pack_error {
            menu.options.open_at(false, Page::Packs);
            menu.options.resource_error = Some(error.to_owned());
            menu.options_open = true;
        }
        if menu.pending.is_some() {
            menu.perf.world_opening();
        }
        Ok(menu)
    }

    pub(super) fn set_root(&mut self, root: PathBuf, settings: &Settings) {
        horizon_client::ui::load_metadata(&root);
        horizon_client::lang::set(horizon_client::lang::Language::load(
            &[&root],
            &settings.language,
        ));
        self.root = root;
        if self.title.is_none() {
            let mut title = Title::new(
                std::env::current_dir().unwrap_or_default().join("saves"),
                &self.root,
            );
            title.last_server = settings.file.string("lastServer");
            title.skip_multiplayer_warning = settings.file.bool("skipMultiplayerWarning");
            // Gui.addInitialScreens: the accessibility onboarding comes first.
            if settings.file.bool("onboardAccessibility") {
                title.start_onboarding();
            }
            self.title = Some(title);
        }
    }

    pub(super) fn handoff(&mut self, game: &mut Game<'_>) {
        game.title = self
            .title
            .take()
            .map(|title| title.with_user(|| game.options.player_name()));
        game.pending_entry = self.pending.take();
        game.cursor = self.cursor;
        game.modifiers = self.modifiers;
        game.physical_keys = std::mem::take(&mut self.physical_keys);
        game.clipboard = std::mem::take(&mut self.clipboard);
        game.text_selection = std::mem::take(&mut self.selection);
        game.perf = std::mem::take(&mut self.perf);
        game.narrator_toast = self.narrator_toast.take();
        game.narration = std::mem::take(&mut self.narration);
        // Preserve an existing resource-error screen opened by start_game.
        if self.options_open {
            let resource_error = game.ui.options.resource_error.take();
            game.ui.options = std::mem::take(&mut self.options);
            game.ui.options.resource_error =
                game.ui.options.resource_error.take().or(resource_error);
            game.screen = Screen::Options;
            game.options_from_title = true;
            game.dragging = self.dragging;
        }
        if let Some(focus) = self.key_focus.take() {
            game.key_focus_adopt(focus, self.key_slider_edit);
        }
    }

    /// After a frame is drawn: its widgets become the last frame's, and the
    /// open screen's narration reads them (`Gui.update`'s
    /// `handleDelayedNarration`). `gui` is the GUI size.
    pub(super) fn frame_drawn(&mut self, gui: (f32, f32)) {
        self.frames += 1;
        horizon_client::widgets::end_frame(self.frames);
        // The keyboard's focus follows its widget, or starts again on a new
        // screen.
        if let Some(focus) = self.key_focus {
            let targets = key_focus::keyboard_targets(
                key_focus::on_screen(key_focus::drawn_targets(), gui),
                false,
            );
            let screen = self.key_screen();
            self.key_focus = if screen == self.key_screen {
                key_focus::follow(focus, &targets)
            } else {
                // Screen.setInitialFocus: a forward Tab from nothing.
                self.key_screen = screen;
                key_focus::tab_step(&targets, None, true).map(|i| targets[i])
            };
            if let Some(now) = self.key_focus
                && now.rect != focus.rect
            {
                self.cursor = now.center();
                self.key_nudge = None;
            }
        }
        self.narration
            .frame(gui, self.cursor, self.key_focus.map(|t| t.rect));
    }

    /// Which screen is open, as far as the keyboard's focus cares.
    fn key_screen(&self) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        (self.options_open, self.win.is_some()).hash(&mut hasher);
        std::mem::discriminant(&self.options.page()).hash(&mut hasher);
        self.title.as_ref().map(Title::screen_key).hash(&mut hasher);
        hasher.finish()
    }

    /// The keyboard lets go of the focus (the real mouse is used): it stays
    /// where the next Tab or arrow goes on from. Whether it had it.
    fn key_focus_release(&mut self) -> bool {
        let Some(focus) = self.key_focus.take() else {
            return false;
        };
        self.key_origin = Some(focus.rect);
        self.key_nudge = None;
        true
    }

    fn queue_entry(&mut self, action: Action, player_name: String) {
        let entry = match action {
            Action::Play { folder } => PendingWorldEntry::Local {
                folder,
                name: None,
                seed: None,
                creative: false,
                create: None,
            },
            Action::Create {
                folder,
                name,
                seed,
                creative,
                options,
            } => PendingWorldEntry::Local {
                folder,
                name: Some(name),
                seed,
                creative,
                create: Some(Box::new(options)),
            },
            Action::Connect { address, lan } => PendingWorldEntry::Server {
                address,
                name: player_name,
                cookies: None,
                lan,
            },
            _ => return,
        };
        self.pending = Some(entry);
        self.perf.world_opening();
    }

    fn shortcut(&mut self, event: &KeyEvent) -> Option<clipboard::EditShortcut> {
        let edit = if cfg!(target_os = "macos") {
            self.modifiers.super_key()
        } else {
            self.modifiers.control_key()
        };
        clipboard::edit_shortcut(
            clipboard::shortcut_letter(&event.logical_key, event.physical_key),
            edit,
            self.modifiers.shift_key(),
            self.modifiers.alt_key(),
        )
    }

    /// `Screen.keyPressed` on the title screens: the action a key starts (Enter confirms, see `confirm_key` in title.rs).
    fn title_key(&mut self, event: &KeyEvent, viewport: (f32, f32)) -> Action {
        let shortcut = self.shortcut(event);
        let Some(title) = &mut self.title else {
            return Action::None;
        };
        if let Some(shortcut) = shortcut {
            if title.has_caret_field() {
                title.field_shortcut(shortcut, &mut self.clipboard);
            } else if let Some((id, value, max)) = title.focused_field()
                && self
                    .selection
                    .apply(id, value, shortcut, &mut self.clipboard, |v, t| {
                        clipboard::append_limited(v, t, max)
                    })
            {
                title.search_changed();
            }
            return Action::None;
        }
        // `TabNavigationBar.keyPressed` comes before the open tab's own
        // widgets: Ctrl plus a digit or Tab picks a tab instead of reaching
        // them.
        let ctrl = if cfg!(target_os = "macos") {
            self.modifiers.super_key()
        } else {
            self.modifiers.control_key()
        };
        if let PhysicalKey::Code(key) = event.physical_key
            && title
                .tab_bar_key(key, ctrl, self.modifiers.shift_key())
                .is_some()
        {
            return Action::None;
        }
        match event.physical_key {
            PhysicalKey::Code(KeyCode::Escape) => title.escape(),
            PhysicalKey::Code(KeyCode::F5) => title.refresh_servers(),
            PhysicalKey::Code(KeyCode::Tab) => {
                title.tab_dir(self.modifiers.shift_key(), viewport);
            }
            PhysicalKey::Code(KeyCode::Enter | KeyCode::NumpadEnter) => {
                return title.confirm_key_size(viewport).0;
            }
            PhysicalKey::Code(KeyCode::Space)
                if !title.has_caret_field()
                    && (title.list_focused() || title.focused_button.is_some()) =>
            {
                return title.confirm_key_size(viewport).0;
            }
            PhysicalKey::Code(
                key @ (KeyCode::Backspace
                | KeyCode::Delete
                | KeyCode::ArrowLeft
                | KeyCode::ArrowRight
                | KeyCode::Home
                | KeyCode::End),
            ) if title.has_caret_field() => {
                title.field_key(
                    key,
                    self.modifiers.control_key(),
                    self.modifiers.shift_key(),
                );
            }
            PhysicalKey::Code(
                key @ (KeyCode::ArrowLeft
                | KeyCode::ArrowRight
                | KeyCode::ArrowUp
                | KeyCode::ArrowDown),
            ) => {
                title.arrow_key(key, viewport, self.modifiers.shift_key());
            }
            _ => {
                let cleared = !title.has_caret_field()
                    && title
                        .focused_field()
                        .is_some_and(|(id, value, _)| self.selection.take(id, value));
                if cleared {
                    title.search_changed();
                }
                if event.physical_key == PhysicalKey::Code(KeyCode::Backspace) {
                    if !cleared {
                        title.backspace();
                    }
                } else if let Some(text) = &event.text {
                    title.type_text(text);
                }
            }
        }
        Action::None
    }
}

/// A panic's message: its `&str` or `String` payload.
fn panic_text(payload: &(dyn std::any::Any + Send)) -> &str {
    payload
        .downcast_ref::<&str>()
        .copied()
        .or_else(|| payload.downcast_ref::<String>().map(String::as_str))
        .unwrap_or("Box<dyn Any>")
}

/// Each generation has its own channel and progress. Pack changes wait for the
/// old tail before starting another generation, including cache/global writes.
pub(super) fn spawn_loader(
    options: &Options,
    settings: &Settings,
    vanilla_root: &std::path::Path,
    started: Instant,
    progress: &Progress,
) -> Result<(Receiver<LoaderMsg>, JoinHandle<()>)> {
    let (sender, receiver) = std::sync::mpsc::channel();
    let settings = settings.clone();
    let vanilla_root = vanilla_root.to_path_buf();
    let dev_mods = options.dev_mods.clone();
    let world = launch_world(options).map(str::to_owned);
    let progress = progress.clone();
    let handle = std::thread::Builder::new()
        .name("resource-loader".into())
        .spawn(move || {
            let mut pack_error = None;
            // ONYX fork: the pack overlay build's mirror jobs feed the
            // boot fraction's first band (0..0.27); the band's end is
            // marked when both roots are resolved, cache hit or not.
            let pack_progress = progress.clone();
            let sink: horizon_client::packs::ProgressSink =
                std::sync::Arc::new(move |local: f32| {
                    pack_progress.advance_to(0.27 * local.clamp(0.0, 1.0));
                });
            let root = packs::startup_root_with_progress(
                &vanilla_root,
                &settings.resource_packs,
                &[],
                Some(&sink),
            )
            .unwrap_or_else(|error| {
                pack_error = Some(error.to_string());
                vanilla_root.clone()
            });
            let root = mods::content_root(&root, &settings.mods.enabled, &dev_mods);
            progress.advance_to(0.29);
            let result = load_resources(
                root,
                world.as_deref(),
                &vanilla_root,
                &settings,
                started,
                true,
                false,
                &progress,
                &mut |gui, root| {
                    let _ = sender.send(LoaderMsg::Ui(gui, root.to_path_buf()));
                },
            )
            .map(|mut loaded| {
                loaded.pack_error = loaded.pack_error.or(pack_error);
                loaded
            })
            .map_err(|error| error.to_string());
            let _ = sender.send(LoaderMsg::Done(result));
        })?;
    Ok((receiver, handle))
}

impl Loading {
    pub(super) fn poll_menu_resources(&mut self) -> Result<()> {
        // Once `Done` arrived the generation is over: the caller may have taken
        // its result, and the finished loader's channel is closed.
        if self.loaded.is_none() && !self.delivered {
            loop {
                match self.receiver.try_recv() {
                    Ok(LoaderMsg::Ui(gui, root)) => self.set_ui(gui, root),
                    Ok(LoaderMsg::Done(result)) => {
                        self.loaded = Some(result);
                        self.delivered = true;
                        break;
                    }
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        // The loader sends `Done` before it returns: it panicked.
                        let reason = match self.handle.take().map(JoinHandle::join) {
                            Some(Err(payload)) => panic_text(payload.as_ref()).to_owned(),
                            Some(Ok(())) => "it returned without a result".into(),
                            None => "no loader thread was running".into(),
                        };
                        return Err(
                            format!("resource loader stopped before finishing: {reason}").into(),
                        );
                    }
                }
            }
        }
        self.restart_menu_resources()
    }
    pub(super) fn apply_menu_settings(&mut self) {
        if let Some(renderer) = &mut self.renderer {
            renderer.gamma = self.settings.gamma;
            renderer.set_present_mode(self.settings.vsync, self.settings.horizon.present_mode);
        }
        horizon_client::hud::GUI_SCALE.store(self.settings.gui_scale, Ordering::Relaxed);
        super::narrator::sync(&self.settings);
        if let Some(gui) = &mut self.gui {
            gui.atlas.font.uniform = self.settings.force_unicode;
            gui.atlas.font.japanese = self.settings.japanese_glyphs;
        }
        if let Some(window) = self.window.clone() {
            let monitor = window
                .current_monitor()
                .or_else(|| window.primary_monitor());
            if let Some(modes) = self
                .video_modes
                .refresh(monitor, horizon_client::window::monitor_video_modes)
            {
                self.settings.video_modes = modes;
            }
            let preferred = self
                .settings
                .file
                .fullscreen_resolution
                .as_deref()
                .and_then(horizon_client::window::VideoMode::read);
            self.fullscreen.apply(
                &window,
                self.settings.fullscreen,
                self.settings.file.bool("exclusiveFullscreen"),
                preferred,
                false,
            );
        }
        self.update_menu_cursor();
    }

    fn update_menu_cursor(&mut self) {
        let batch = HudBatch::new(self.menu_size());
        self.menu.cursor = (
            self.menu.pointer.0 as f32 / batch.scale,
            self.menu.pointer.1 as f32 / batch.scale,
        );
    }

    fn menu_size(&self) -> [u32; 2] {
        self.renderer
            .as_ref()
            .map_or([1280, 720], |r| [r.config.width, r.config.height])
    }

    fn save_menu_address(&mut self) {
        if self.menu.title.as_ref().is_some_and(|title| {
            title.last_server != self.settings.file.string("lastServer")
                || title.skip_multiplayer_warning
                    != self.settings.file.bool("skipMultiplayerWarning")
        }) {
            self.save_menu_settings();
        }
    }

    fn save_menu_settings(&mut self) {
        if let Some(title) = &self.menu.title {
            self.settings.file.set(
                "lastServer",
                horizon_client::options_file::V::Str(title.last_server.clone()),
            );
            self.settings.file.set(
                "skipMultiplayerWarning",
                horizon_client::options_file::V::Bool(title.skip_multiplayer_warning),
            );
        }
        self.settings.save(&Game::options_path());
    }

    fn open_menu_options(&mut self, page: Page) {
        self.menu.options.open_at(false, page);
        self.menu.options.languages = horizon_client::lang::available(&[&self.menu.root]);
        self.menu.options.pack_infos = packs::pack_infos(&self.vanilla_root, &[]);
        self.menu.options.pack_selection = self.settings.resource_packs.clone();
        self.menu.options.pack_screen = None;
        self.menu.options_open = true;
    }

    fn menu_option_events(&mut self, events: Vec<Event>) {
        for event in events {
            match event {
                Event::Changed => {
                    self.save_menu_settings();
                    self.apply_menu_settings();
                }
                Event::SliderDrag => {
                    self.apply_menu_settings();
                }
                Event::Close if self.menu.options.resource_error.is_some() => {}
                Event::Close => {
                    self.menu.options_open = false;
                    self.menu.dragging = false;
                    self.save_menu_settings();
                }
                Event::Language(code) => {
                    horizon_client::lang::set(horizon_client::lang::Language::load(
                        &[&self.menu.root],
                        &code,
                    ));
                    self.save_menu_settings();
                }
                Event::Packs(_) => {
                    self.save_menu_settings();
                    self.menu.reload_packs = true;
                }
                Event::OpenPackFolder => {
                    let dir = packs::packs_dir();
                    let _ = std::fs::create_dir_all(&dir);
                    packs::open_folder(&dir);
                }
                Event::DownloadPack(i) => {
                    let dir = packs::packs_dir();
                    if let Some(screen) = &mut self.menu.options.pack_screen
                        && let Some(browser) = &mut screen.browser
                    {
                        browser.download(i, &dir, horizon_core::VANILLA_VERSION);
                    }
                }
                Event::DataPacks(ids) => {
                    let resources = self.vanilla_root.clone();
                    if let Some(title) = &mut self.menu.title {
                        title.offer_data_packs(&resources, ids);
                    }
                }
                Event::OpenDataPackFolder(dir) => {
                    let _ = std::fs::create_dir_all(&dir);
                    packs::open_folder(&dir);
                }
                Event::OpenModFolder => {
                    let dir = Game::game_dir().join(horizon_client::mod_list::DIR);
                    let _ = std::fs::create_dir_all(&dir);
                    packs::open_folder(&dir);
                }
                Event::ListShaderPacks => {
                    self.settings.shader_packs =
                        horizon_client::shader_pack_list::scan(&Game::shader_packs_dir());
                }
                Event::ShaderPack => self.save_menu_settings(),
                Event::OpenShaderOptions => {
                    self.settings.shader_options = super::shader_packs::read_shader_options(
                        &self.settings.horizon.shader_pack,
                        &self.settings.language,
                        &horizon_shaders::Macros::standard(),
                    );
                }
                Event::ShaderOptions => {
                    if let Some(options) = self.settings.shader_options.take() {
                        super::shader_packs::write_shader_options(&options);
                    }
                }
                Event::NativePackOptions(i) => {
                    if let Some(pack) = self.settings.shader_packs.get(i) {
                        pack.save_values(&Game::shader_packs_dir());
                    }
                }
                Event::OpenShaderPackFolder => {
                    let dir = Game::shader_packs_dir();
                    let _ = std::fs::create_dir_all(&dir);
                    packs::open_folder(&dir);
                }
                Event::Credits => {
                    if let Some(gui) = &self.gui {
                        let name = self.options.as_ref().map_or_else(
                            || horizon_client::offline_name::generate(0),
                            Options::player_name,
                        );
                        let screen = horizon_client::win_screen::WinScreen::new(
                            false,
                            &self.menu.root,
                            &name,
                            &gui.atlas.font,
                        );
                        self.menu.win = Some((screen, Instant::now()));
                    }
                }
                Event::Narrator => super::narrator::update_status(
                    self.settings.narrator(),
                    &mut self.menu.narrator_toast,
                ),
                // The startup menu has no screen stack for ConfirmLinkScreen;
                // the link opens directly, as the menu's other links do.
                Event::AccessibilityHelp => {
                    super::packs::open_uri(super::options::ACCESSIBILITY_HELP_LINK)
                }
                // Mods' settings pages open only in a running game.
                Event::NativeSettings(_) | Event::NativeButton(..) => {}
                // The startup menu runs before the game opens a control socket,
                // and before its sound engine (no clicks, no volume previews,
                // and no music tracks to list or play).
                Event::ButtonClick
                | Event::CopyAgentDetails
                | Event::ToggleRecording
                | Event::ExportRepro
                | Event::ListMusicTracks
                | Event::PlayMusicTrack(..)
                | Event::Volume(..) => {}
                Event::OpenStudioFolder => {
                    let dir = std::path::PathBuf::from("recordings");
                    let _ = std::fs::create_dir_all(&dir);
                    packs::open_folder(&dir);
                }
            }
        }
    }

    /// `Game::narrator_hotkey` on the startup menu's screens.
    fn menu_narrator_hotkey(&mut self, event: &KeyEvent) {
        let letter = clipboard::shortcut_letter(&event.logical_key, event.physical_key);
        if letter != Some('b') {
            return;
        }
        let menu = &mut self.menu;
        let control_with_quirk = if cfg!(target_os = "macos") {
            menu.modifiers.super_key()
        } else {
            menu.modifiers.control_key()
        };
        let edit_box_focused = if menu.options_open {
            menu.options.typing()
        } else if menu.win.is_some() || menu.pending.is_some() {
            false
        } else {
            menu.title
                .as_mut()
                .is_some_and(|t| t.has_caret_field() || t.focused_field().is_some())
        };
        let hotkey = super::narrator::is_hotkey(
            event.state == ElementState::Pressed,
            control_with_quirk,
            letter,
            edit_box_focused,
            horizon_narrator::is_active(),
            self.settings.file.bool("narratorHotkey"),
        );
        if !hotkey {
            return;
        }
        let was_disabled = super::narrator::cycle_status(&mut self.settings);
        super::narrator::update_status(self.settings.narrator(), &mut self.menu.narrator_toast);
        self.save_menu_settings();
        self.menu
            .narration
            .status_changed(was_disabled, horizon_narrator::screen::Trigger::Keyboard);
    }

    fn menu_options_click(&mut self, drag: bool, right: bool) {
        let size = self.menu_size();
        let batch = HudBatch::new(size);
        let Some(gui) = &self.gui else { return };
        self.menu.options.shift = self.menu.modifiers.shift_key();
        let events = self.menu.options.click(
            &mut self.settings,
            &gui.atlas,
            self.menu.cursor.0,
            self.menu.cursor.1,
            (batch.width, batch.height),
            [size[0] as i32, size[1] as i32],
            drag,
            right,
        );
        self.menu_option_events(events);
    }

    /// The Realms screen's join, once its address came back: queued like a click on a server.
    pub(super) fn poll_menu_realms(&mut self, event_loop: &dyn EventLoopCtl) {
        if let Some(action) = self
            .menu
            .title
            .as_mut()
            .and_then(Title::take_connect_request)
        {
            self.menu_action(event_loop, action);
        }
    }

    fn menu_action(&mut self, event_loop: &dyn EventLoopCtl, action: Action) {
        match action {
            Action::Quit => event_loop.exit(),
            Action::Options => self.open_menu_options(Page::Main),
            Action::Accessibility => self.open_menu_options(Page::Accessibility),
            Action::Language => self.open_menu_options(Page::Language),
            Action::Sounds => self.open_menu_options(Page::Sound),
            Action::Credits => self.open_menu_options(Page::Credits),
            Action::OpenUri(uri) => super::packs::open_uri(&uri),
            Action::CopyText(text) => {
                use horizon_client::clipboard::Clipboard;
                self.menu.clipboard.set(&text);
            }
            Action::DataPacks { dir, selected } => {
                let resources = self.vanilla_root.clone();
                self.menu.options.open_data_packs(dir, selected, resources);
                self.menu.options_open = true;
            }
            action => {
                let name = self
                    .options
                    .as_ref()
                    .map_or_else(|| "Player".into(), |o| o.player_name());
                self.menu.queue_entry(action, name);
            }
        }
        self.save_menu_address();
    }

    /// The real mouse before the screen takes it: a move, a press or a
    /// scroll hands the pointer back from the keyboard (a press or a scroll
    /// where the mouse left it), and a left press focuses what it lands on.
    fn menu_key_focus_mouse(&mut self, event: &WindowEvent) {
        match event {
            WindowEvent::CursorMoved { .. } => {
                self.menu.key_focus_release();
            }
            WindowEvent::MouseInput { state, button, .. } => {
                if self.menu.key_focus_release() {
                    self.update_menu_cursor();
                }
                if *state == ElementState::Pressed && *button == MouseButton::Left {
                    let (cx, cy) = self.menu.cursor;
                    self.menu.key_origin = self
                        .menu_key_targets()
                        .into_iter()
                        .find(|t| {
                            let [x, y, w, h] = t.rect;
                            cx >= x && cy >= y && cx < x + w && cy < y + h
                        })
                        .map(|t| t.rect);
                }
            }
            WindowEvent::MouseWheel { .. } if self.menu.key_focus_release() => {
                self.update_menu_cursor();
            }
            _ => {}
        }
    }

    /// What the keyboard can focus on the screen last drawn.
    fn menu_key_targets(&self) -> Vec<Target> {
        let batch = HudBatch::new(self.menu_size());
        key_focus::keyboard_targets(
            key_focus::on_screen(key_focus::drawn_targets(), (batch.width, batch.height)),
            false,
        )
    }

    /// `Screen.keyPressed`'s focus navigation on the title shown while
    /// resources load, as `Game::key_focus_key` does it in the game (the
    /// pointer sits on the focus; a step past a list's visible rows doesn't
    /// scroll it here). Whether the key was used.
    fn menu_key_focus(&mut self, event_loop: &dyn EventLoopCtl, key: KeyCode) -> bool {
        let Some(nav) = key_focus::nav_of(key, self.menu.modifiers.shift_key()) else {
            return false;
        };
        let options = &self.menu.options;
        if self.gui.is_none()
            || self.menu.pending.is_some()
            || self.menu.win.is_some()
            || (self.menu.options_open
                && (options.selected_key.is_some() || options.selected_pad_action.is_some()))
        {
            return false;
        }
        let targets = self.menu_key_targets();
        if targets.is_empty() {
            return false;
        }
        let find = |rect: [f32; 4]| {
            targets
                .iter()
                .find(|t| t.rect.iter().zip(rect).all(|(a, b)| (a - b).abs() < 0.5))
                .copied()
        };
        let mut slider_edit = self.menu.key_slider_edit;
        let mut focus = self.menu.key_focus.and_then(|f| find(f.rect));
        if focus.is_none()
            && let Some(clicked) = self.menu.key_origin.and_then(find)
        {
            focus = Some(clicked);
            slider_edit = true;
        }
        let (typing, rows_take_enter) = if self.menu.options_open {
            (self.menu.options.typing(), false)
        } else if let Some(title) = &mut self.menu.title {
            (
                title.has_caret_field() || title.focused_field().is_some(),
                title.list_focused(),
            )
        } else {
            (false, false)
        };
        let batch = HudBatch::new(self.menu_size());
        let ctx = key_focus::Context {
            slider_edit,
            typing,
            rows_take_enter,
            gui: (batch.width, batch.height),
        };
        let act = key_focus::act(nav, focus, &targets, ctx);
        if act == Act::Pass {
            return false;
        }
        if self.menu.key_focus.is_none() {
            self.menu.key_screen = self.menu.key_screen();
        }
        self.menu.key_origin = None;
        self.menu.key_focus = focus;
        self.menu.key_slider_edit = slider_edit;
        match (act, focus) {
            (Act::Focus(i), _) => {
                self.menu_key_focus_on(event_loop, targets[i]);
                self.menu.key_slider_edit = key_focus::slider_edit_on_focus(nav);
            }
            (Act::Step(button), Some(focus)) => {
                let rects: Vec<[f32; 4]> = targets.iter().map(|t| t.rect).collect();
                if let Some(i) =
                    horizon_client::gamepad::focus_step_rect(focus.rect, button, &rects)
                {
                    self.menu_key_focus_on(event_loop, targets[i]);
                }
            }
            (Act::Press, Some(focus)) => self.menu_key_click(event_loop, focus.center()),
            (Act::ToggleSlider, _) => self.menu.key_slider_edit = !slider_edit,
            (
                Act::Nudge(sign),
                Some(
                    focus @ Target {
                        kind: Kind::Slider { value, track },
                        ..
                    },
                ),
            ) => {
                let from = match self.menu.key_nudge {
                    Some((rect, x)) if rect == focus.rect => x,
                    _ => horizon_client::gamepad::slider_x(track, value),
                };
                let x = (from + sign).clamp(track[0], track[0] + track[1]);
                self.menu.key_nudge = Some((focus.rect, x));
                self.menu_key_click(event_loop, (x, focus.center().1));
            }
            _ => {}
        }
        if let Some(Target {
            kind: Kind::Slider { .. },
            ..
        }) = self.menu.key_focus
        {
            use horizon_client::screen_narrator::InputType;
            self.menu.narration.set_input(if self.menu.key_slider_edit {
                InputType::KeyboardTab
            } else {
                InputType::KeyboardArrow
            });
        }
        true
    }

    /// The keyboard's focus lands on `target`, the pointer with it (see
    /// `Game::key_focus_landed`).
    fn menu_key_focus_on(&mut self, event_loop: &dyn EventLoopCtl, target: Target) {
        let left = self.menu.key_focus.replace(target);
        self.menu.key_nudge = None;
        self.menu.key_slider_edit = false;
        self.menu.cursor = target.center();
        if left.is_some_and(|l| l.rect == target.rect) {
            return;
        }
        if left.is_some_and(|l| l.kind == Kind::Field)
            && target.kind != Kind::Field
            && !self.menu.options_open
            && let Some(title) = &mut self.menu.title
        {
            title.blur_field();
        }
        match target.kind {
            Kind::Field => self.menu_key_click(event_loop, key_focus::field_click(target.rect)),
            Kind::Row if !self.menu.options_open => {
                let batch = HudBatch::new(self.menu_size());
                if let Some(title) = &mut self.menu.title {
                    title.focus_world_row(target.center(), (batch.width, batch.height));
                }
            }
            _ => {}
        }
    }

    /// A left click at `at` (GUI pixels), the pointer staying there.
    fn menu_key_click(&mut self, event_loop: &dyn EventLoopCtl, at: (f32, f32)) {
        self.menu.cursor = at;
        let was = std::mem::replace(&mut self.menu.key_synthetic, true);
        for state in [ElementState::Pressed, ElementState::Released] {
            let click = WindowEvent::MouseInput {
                device_id: DeviceId::dummy(),
                state,
                button: MouseButton::Left,
            };
            if let Err(e) = self.menu_event(event_loop, click) {
                log::warn!("Horizon: keyboard focus click: {e}");
            }
        }
        self.menu.key_synthetic = was;
    }

    pub(super) fn menu_event(
        &mut self,
        event_loop: &dyn EventLoopCtl,
        event: WindowEvent,
    ) -> Result<()> {
        if let WindowEvent::KeyboardInput { event, .. } = &event
            && let PhysicalKey::Code(key) = event.physical_key
        {
            if event.state == ElementState::Pressed {
                self.menu.physical_keys.insert(key);
            } else {
                self.menu.physical_keys.remove(&key);
            }
        }
        if let WindowEvent::KeyboardInput { event, .. } = &event {
            self.menu_narrator_hotkey(event);
        }
        // The real mouse takes the pointer back from the keyboard's focus.
        let synthetic = self.menu.key_synthetic;
        if !synthetic {
            self.menu_key_focus_mouse(&event);
        }
        // Fullscreen (F11) is the window's: not the screen's input; the
        // keyboard's own clicks aren't the mouse.
        let f11 = matches!(&event, WindowEvent::KeyboardInput { event, .. }
            if event.physical_key == PhysicalKey::Code(KeyCode::F11));
        if !f11 && !synthetic {
            super::narrator::narrate_input(&mut self.menu.narration, &event, self.menu.cursor);
        }
        // Screen.keyPressed: Tab, the arrows and Enter move and press the
        // focus, unless the focused widget or the screen keeps them.
        if let WindowEvent::KeyboardInput {
            event: key_event, ..
        } = &event
            && key_event.state == ElementState::Pressed
            && let PhysicalKey::Code(key) = key_event.physical_key
            && self.menu_key_focus(event_loop, key)
        {
            return Ok(());
        }
        // ONYX fork: while the boot screen is what's on the window — the
        // menu bundle has not arrived, loading is still running under
        // it, or the finish sequence is playing — the screens underneath
        // must not take input: their buttons are invisible, so clicks and
        // keys would act on things the player cannot see. Window-level
        // events fall through to the match below, as do the two keys the
        // boot screen itself honours: F11 and the queued entry's Escape.
        if self.boot_covering() {
            match &event {
                WindowEvent::CloseRequested
                | WindowEvent::Resized(_)
                | WindowEvent::RedrawRequested
                | WindowEvent::ModifiersChanged(_)
                | WindowEvent::Focused(_) => {}
                WindowEvent::KeyboardInput { event, .. }
                    if event.state == ElementState::Pressed =>
                {
                    if event.physical_key == PhysicalKey::Code(KeyCode::F11) {
                        self.settings.fullscreen = !self.settings.fullscreen;
                        self.apply_menu_settings();
                        self.save_menu_settings();
                    } else if event.physical_key == PhysicalKey::Code(KeyCode::Escape)
                        && self.menu.pending.is_some()
                    {
                        self.menu.pending = None;
                        self.menu.perf.cancel_world_opening();
                    }
                    return Ok(());
                }
                _ => return Ok(()),
            }
        }
        match event {
            WindowEvent::CloseRequested => {
                if !self.settings.quit_shortcuts
                    && horizon_client::input::is_quit_shortcut_down(
                        self.menu.modifiers,
                        &self.menu.physical_keys,
                    )
                {
                    return Ok(());
                }
                self.save_menu_settings();
                event_loop.exit();
            }
            WindowEvent::Resized(size) => {
                if let Some(r) = &mut self.renderer {
                    r.resize(size.width, size.height);
                }
                self.apply_menu_settings();
            }
            WindowEvent::RedrawRequested => self.draw(),
            WindowEvent::ModifiersChanged(modifiers) => self.menu.modifiers = modifiers.state(),
            // `PackSelectionScreen.onFilesDrop`: a pack dropped on the window
            // joins the Create New World screen's data packs.
            WindowEvent::DroppedFile(path) => {
                if let Some(title) = &mut self.menu.title {
                    title.drop_file(path);
                }
            }
            WindowEvent::Focused(false) => {
                self.menu.modifiers = ModifiersState::empty();
                self.menu.physical_keys.clear();
                self.menu.dragging = false;
                if let Some(title) = &mut self.menu.title {
                    title.release();
                    title.release_field();
                }
                let events = self.menu.options.release();
                self.menu_option_events(events);
            }
            WindowEvent::KeyboardInput { event, .. }
                if event.state == ElementState::Pressed && self.menu.pending.is_some() =>
            {
                if event.physical_key == PhysicalKey::Code(KeyCode::Escape) {
                    self.menu.pending = None;
                    self.menu.perf.cancel_world_opening();
                }
            }
            _ if self.gui.is_none() || self.menu.pending.is_some() => {}
            WindowEvent::KeyboardInput { event, .. } if self.menu.win.is_some() => {
                if let PhysicalKey::Code(code) = event.physical_key {
                    let pressed = event.state == ElementState::Pressed;
                    if code == KeyCode::Escape && pressed {
                        self.menu.win = None;
                    } else if let Some(key) = horizon_client::win_screen::Key::from_code(code)
                        && let Some((win, _)) = &mut self.menu.win
                    {
                        win.key(key, pressed);
                    }
                }
            }
            WindowEvent::MouseInput { .. } | WindowEvent::MouseWheel { .. }
                if self.menu.win.is_some() => {}
            WindowEvent::CursorMoved { position, .. } => {
                self.menu.pointer = (position.x, position.y);
                self.update_menu_cursor();
                if self.menu.options_open && self.menu.dragging {
                    self.menu_options_click(true, false);
                } else if let Some(title) = &mut self.menu.title {
                    let size = self
                        .renderer
                        .as_ref()
                        .map_or([1280, 720], |r| [r.config.width, r.config.height]);
                    let b = HudBatch::new(size);
                    title.drag(self.menu.cursor, (b.width, b.height));
                    title.drag_field(&self.gui.as_ref().unwrap().atlas, self.menu.cursor.0);
                }
            }
            WindowEvent::MouseInput { state, button, .. } if self.menu.options_open => {
                if state == ElementState::Pressed && self.menu.options.selected_key.is_some() {
                    let events = self.menu.options.bind_mouse(&mut self.settings, button);
                    self.menu_option_events(events);
                } else if matches!(button, MouseButton::Left | MouseButton::Right) {
                    if state == ElementState::Pressed {
                        self.menu.dragging = button == MouseButton::Left;
                        self.menu_options_click(false, button == MouseButton::Right);
                    } else if button == MouseButton::Left {
                        self.menu.dragging = false;
                        let events = self.menu.options.release();
                        self.menu_option_events(events);
                    }
                }
            }
            WindowEvent::MouseInput {
                state,
                button: MouseButton::Left,
                ..
            } => {
                let batch = HudBatch::new(self.menu_size());
                if let (Some(title), Some(gui)) = (&mut self.menu.title, &self.gui) {
                    if state == ElementState::Released {
                        title.release();
                        title.release_field();
                    } else {
                        let (action, _) = title.click(
                            self.menu.cursor.0,
                            self.menu.cursor.1,
                            (batch.width, batch.height),
                        );
                        title.press_field(
                            &gui.atlas,
                            self.menu.cursor,
                            (batch.width, batch.height),
                            self.menu.modifiers.shift_key(),
                        );
                        title.press_rules(
                            &gui.atlas,
                            self.menu.cursor,
                            (batch.width, batch.height),
                        );
                        self.menu_action(event_loop, action);
                    }
                }
            }
            // AbstractSelectionList.mouseClicked: a world row takes a click
            // of any button, so the middle one selects it without joining.
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button: MouseButton::Middle,
                ..
            } => {
                let batch = HudBatch::new(self.menu_size());
                if let Some(title) = &mut self.menu.title {
                    title.select_world_at(
                        self.menu.cursor.0,
                        self.menu.cursor.1,
                        (batch.width, batch.height),
                    );
                }
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let raw = match delta {
                    MouseScrollDelta::LineDelta(_, y) => y,
                    MouseScrollDelta::PixelDelta(p) => (p.y / 30.0) as f32,
                };
                let amount = if self.settings.discrete_scroll {
                    horizon_client::mouse::java_signum(f64::from(raw)) as f32
                } else {
                    raw
                } * self.settings.mouse_wheel_sensitivity;
                let size = self.menu_size();
                let batch = HudBatch::new(size);
                if self.menu.options_open {
                    let events = self.menu.options.wheel(
                        &mut self.settings,
                        (batch.width, batch.height),
                        self.menu.cursor,
                        [size[0] as i32, size[1] as i32],
                        amount,
                    );
                    self.menu_option_events(events);
                } else if let Some(title) = &mut self.menu.title {
                    title.scroll_worlds(amount, self.menu.cursor, (batch.width, batch.height));
                }
            }
            WindowEvent::KeyboardInput { event, .. } if event.state == ElementState::Pressed => {
                if event.physical_key == PhysicalKey::Code(KeyCode::F11) {
                    self.settings.fullscreen = !self.settings.fullscreen;
                    self.apply_menu_settings();
                    self.save_menu_settings();
                } else if self.menu.options_open {
                    let shortcut = self.menu.shortcut(&event);
                    let atlas = &self.gui.as_ref().unwrap().atlas;
                    let handled = shortcut.is_some_and(|s| {
                        self.menu
                            .options
                            .shortcut(atlas, s, &mut self.menu.clipboard)
                    });
                    if !handled && let PhysicalKey::Code(key) = event.physical_key {
                        let events = self.menu.options.key(
                            &mut self.settings,
                            atlas,
                            key,
                            event.text.as_deref(),
                        );
                        self.menu_option_events(events);
                    }
                } else {
                    let batch = HudBatch::new(self.menu_size());
                    let action = self.menu.title_key(&event, (batch.width, batch.height));
                    self.menu_action(event_loop, action);
                }
            }
            _ => {}
        }
        Ok(())
    }

    /// Draws the loading window's frame and reports whether the title
    /// screen itself is in the batch.
    ///
    /// ONYX fork: the boot screen covers the whole loading phase, not
    /// just the wait for the menu bundle — while loading runs (bundle or
    /// not) the title is never the top layer, so it can neither flash
    /// early nor be cut back from. Once the finish sequence starts, the
    /// title is drawn *under* the boot layer: the sequence's fade thins
    /// the boot screen's black field and dissolves into the live title,
    /// and once the sequence has played out the title stands alone until
    /// the handoff. A queued world entry keeps its own overlay on top
    /// throughout, and the sequence stays suppressed while one is queued.
    pub(super) fn draw_menu(&mut self, batch: &mut HudBatch) -> bool {
        if self.gui.is_none() {
            // The pack's screens do not exist yet: the boot screen holds
            // the window while they load.
            self.draw_boot_eased(batch, false);
            return false;
        }
        if self.menu.pending.is_some() {
            return self.draw_screens(batch);
        }
        match self.finishing {
            // The sequence has played out: the title is live, alone,
            // until `finish` takes over.
            Some(at) if at.elapsed() >= self.finishing_len => self.draw_screens(batch),
            // The sequence is playing: title first, then the boot layer
            // over it in overlay mode, its black field riding the fade.
            Some(at) => {
                let title_drawn = self.draw_screens(batch);
                let gui = self.gui.as_ref().expect("the bundle was checked above");
                horizon_client::ui_theme::draw_boot(
                    &gui.atlas,
                    batch,
                    1.0,
                    self.started.elapsed(),
                    Some(at.elapsed()),
                    self.finishing_len,
                    true,
                );
                title_drawn
            }
            // Still loading, bundle in: the boot screen covers, drawn
            // into the menu bundle's atlas — the one the loading
            // renderer samples once the bundle exists.
            None => {
                self.draw_boot_eased(batch, true);
                false
            }
        }
    }

    /// The boot screen on its own, its bar easing toward the loader's
    /// fraction instead of jumping checkpoint to checkpoint; it never
    /// passes the real fraction. Drawn with the boot atlas before the
    /// bundle arrives (`gui_atlas` false), with the bundle's atlas after.
    fn draw_boot_eased(&mut self, batch: &mut HudBatch, gui_atlas: bool) {
        let now = Instant::now();
        let dt = self
            .boot_frame_at
            .map_or(0.0, |at| now.duration_since(at).as_secs_f32());
        self.boot_frame_at = Some(now);
        let target = self.progress.fraction();
        let step = 1.0 - (-dt * 3.5).exp();
        self.boot_shown = (self.boot_shown + (target - self.boot_shown) * step).min(target);
        if target - self.boot_shown < 0.001 {
            self.boot_shown = target;
        }
        let atlas = if gui_atlas {
            &self
                .gui
                .as_ref()
                .expect("the bundle was checked above")
                .atlas
        } else {
            &self.boot
        };
        horizon_client::ui_theme::draw_boot(
            atlas,
            batch,
            self.boot_shown,
            self.started.elapsed(),
            None,
            self.finishing_len,
            false,
        );
    }

    /// The menu's screens as the loader's window dressing: the queued
    /// entry's loading overlay, the win screen, the options, or the
    /// title. Returns whether the title screen itself was drawn, so the
    /// caller can tell a real title frame from a boot-covered one.
    fn draw_screens(&mut self, batch: &mut HudBatch) -> bool {
        let mut title_drawn = false;
        let Some(gui) = &self.gui else {
            return false;
        };
        if self.menu.pending.is_some() {
            // ONYX fork: the queued entry waits on the loader's tail,
            // the same wait the game's parked entry shows — the
            // backdrop treatment and the stage fraction, so the twins
            // cannot drift apart (see `draw_onyx_entry_wait`).
            let caption = horizon_client::ui_theme::onyx_caption(self.progress.line());
            horizon_client::ui_theme::draw_onyx_entry_wait(
                &gui.atlas,
                batch,
                horizon_client::ui_theme::OnyxEntryWait {
                    caption: &caption,
                    fraction: Some(self.progress.stage_fraction()),
                    elapsed: self.started.elapsed(),
                },
            );
        } else if let Some((win, frame)) = &mut self.menu.win {
            let now = Instant::now();
            // 20 ticks a second: the startup menu runs no tick rate.
            let delta_ticks = now.duration_since(*frame).as_secs_f32() * 20.0;
            *frame = now;
            horizon_client::widgets::begin_screen(|| win.screen_info());
            win.draw(batch, &gui.atlas, delta_ticks, Default::default());
            if win.finished(batch.height) {
                self.menu.win = None;
            }
        } else if self.menu.options_open {
            self.menu
                .options
                .draw(&self.settings, batch, &gui.atlas, self.menu.cursor);
        } else if let Some(title) = &mut self.menu.title {
            title_drawn = true;
            title.poll_backup();
            if std::mem::take(&mut title.onboarding_finished) {
                // Options.onboardingAccessibilityFinished.
                self.settings.file.set(
                    "onboardAccessibility",
                    horizon_client::options_file::V::Bool(false),
                );
                self.settings.save(&Game::options_path());
            }
            let _ = title.update_toast(self.settings.notification_display_time);
            for found in title.lan_found_narrations((batch.width, batch.height)) {
                horizon_narrator::say_system_queued(&found.plain());
            }
            let items = match &self.loaded {
                Some(Ok(loaded)) => Some(&*loaded.pack.item_renderer),
                _ => None,
            };
            title.draw_with_items(
                &gui.atlas,
                items,
                batch,
                self.menu.cursor,
                self.settings.file.bool("hideSplashTexts"),
                self.settings.file.bool("hideServerAddress"),
            );
        }
        if let Some(focus) = self.menu.key_focus
            && self.menu.pending.is_none()
            && self.menu.win.is_none()
        {
            key_focus::draw_outline(&gui.atlas, batch, focus);
        }
        // Over every screen, as the game's; no sound before the sound engine.
        let title_toast = self.menu.title.as_ref().is_some_and(|t| t.has_toast());
        super::narrator::draw_toast(
            &mut self.menu.narrator_toast,
            u32::from(title_toast),
            f64::from(self.settings.notification_display_time),
            &gui.atlas,
            batch,
            false,
        );
        title_drawn
    }

    pub(super) fn restart_menu_resources(&mut self) -> Result<()> {
        if !self.menu.reload_packs || self.loaded.is_none() {
            return Ok(());
        }
        if let Some(Ok(loaded)) = &mut self.loaded {
            if let Some(loader) = &loaded.rest_loader
                && !loader.ready()?
            {
                return Ok(());
            }
            if let Some(loader) = loaded.rest_loader.take()
                && let Err(error) = loader.finish()
            {
                log::warn!("Horizon: replaced resource generation failed: {error}");
            }
        }
        self.loaded = None;
        self.delivered = false;
        if let Some(handle) = self.handle.take() {
            handle.join().map_err(|payload| {
                format!("resource loader panicked: {}", panic_text(payload.as_ref()))
            })?;
        }
        self.progress = Progress::new();
        // ONYX fork: a fresh generation starts the boot screen over too —
        // no leftover finish sequence or eased bar from the old one.
        self.finishing = None;
        self.finishing_len = horizon_client::ui_theme::BOOT_SEQ_TOTAL;
        self.boot_shown = 0.0;
        self.boot_frame_at = None;
        let options = self.options.as_ref().ok_or("loading already finished")?;
        let (receiver, handle) = spawn_loader(
            options,
            &self.settings,
            &self.vanilla_root,
            self.started,
            &self.progress,
        )?;
        self.receiver = receiver;
        self.handle = Some(handle);
        self.menu.reload_packs = false;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gui(root: &std::path::Path) -> UiResources {
        UiResources {
            atlas: HudAtlas::fonts(root).unwrap(),
            panorama: Arc::new(horizon_client::gfx::panorama::Faces::colour([
                96, 110, 111, 255,
            ])),
        }
    }

    fn loading(receiver: Receiver<LoaderMsg>) -> Loading {
        Loading {
            window: None,
            renderer: None,
            boot: horizon_client::hud::HudAtlas::boot(),
            gui: None,
            settings: Settings::default(),
            fullscreen: horizon_client::window::FullscreenState::new(false),
            video_modes: Default::default(),
            options: None,
            vanilla_root: PathBuf::new(),
            pack_error: None,
            started: Instant::now(),
            panorama_rotation: Default::default(),
            progress: Progress::new(),
            receiver,
            loaded: None,
            delivered: false,
            handle: None,
            shown: false,
            boot_shown: 0.0,
            boot_frame_at: None,
            finishing: None,
            finishing_len: horizon_client::ui_theme::BOOT_SEQ_TOTAL,
            ui_at: None,
            title_drawn: false,
            menu: StartupMenu::default(),
            error: None,
        }
    }

    #[test]
    fn the_world_renderer_waits_for_the_first_title_frame() {
        let (_send, receiver) = std::sync::mpsc::channel();
        let mut loading = loading(receiver);
        // No menu bundle yet: nothing to wait for.
        assert!(loading.ready_for_handoff());
        // The title artwork is up but no frame of it was presented.
        loading.ui_at = Some(Instant::now());
        assert!(!loading.ready_for_handoff());
        loading.title_drawn = true;
        assert!(loading.ready_for_handoff());
        // A window never asked to redraw doesn't hold the start for long.
        loading.title_drawn = false;
        loading.ui_at = Instant::now().checked_sub(Duration::from_secs(2));
        assert!(loading.ready_for_handoff());
    }

    #[test]
    fn boot_progress_fine_fraction_is_monotonic_and_banded() {
        let progress = Progress::new();
        // The pack band: a local build fraction maps into 0..0.27.
        progress.advance_to(0.27 * 0.5);
        assert!((progress.fraction() - 0.135).abs() < 1e-6);
        // Stage checkpoints still drive the line and can only raise the
        // fraction (stage 1 of 9 is behind the fine value here).
        progress.set(1);
        assert_eq!(progress.line(), STAGE_LINES[1]);
        assert!((progress.fraction() - 0.135).abs() < 1e-6);
        // Later bands advance; a stale or repeated report never regresses.
        progress.advance_to(0.62);
        progress.advance_to(0.30);
        progress.advance_to(0.62);
        assert!((progress.fraction() - 0.62).abs() < 1e-6);
        // A stage rollback (the pack-failure path re-runs the core) cannot
        // pull the bar back either.
        progress.set(0);
        assert!((progress.fraction() - 0.62).abs() < 1e-6);
        // Only the finish trigger's 1.0 completes it.
        progress.advance_to(1.0);
        assert_eq!(progress.fraction(), 1.0);
        // Out-of-range reports clamp.
        progress.advance_to(2.0);
        assert_eq!(progress.fraction(), 1.0);
    }

    #[test]
    fn the_stage_fraction_ignores_the_boot_finishs_fine_advance() {
        let progress = Progress::new();
        progress.set(7);
        assert!((progress.stage_fraction() - 7.0 / 9.0).abs() < 1e-6);
        // The boot finish advancing the fine fraction to 1.0 is what
        // pinned the entry wait's bar at full under LOADING RECIPES:
        // `fraction()` follows it, the stage fraction must not.
        progress.advance_to(1.0);
        assert_eq!(progress.fraction(), 1.0);
        assert!((progress.stage_fraction() - 7.0 / 9.0).abs() < 1e-6);
        assert_eq!(progress.line(), STAGE_LINES[7]);
        progress.set(8);
        assert!((progress.stage_fraction() - 8.0 / 9.0).abs() < 1e-6);
    }

    #[test]
    fn a_queued_entry_waits_on_the_backdrop_for_every_entry_kind() {
        // The menu-phase twin of the game's parked-entry wait: for a
        // create, a play and a connect alike, the queued entry's
        // frame is the entry-wait treatment — the backdrop, the mini
        // wordmark, the caption, the composite bar — never the boot
        // scene. The two draw sites share `draw_onyx_entry_wait`, and
        // this pins the menu site to it.
        let root = std::env::temp_dir().join("horizon-startup-menu-entry-wait");
        // The boot wordmark's polished gradient stops (the probe the
        // kit's own scene test uses to prove the lockup is present).
        let dark_stop = |v: &horizon_client::hud::HudVertex| {
            let (r, g, b) = (
                (v.color & 0xFF) as i32,
                ((v.color >> 8) & 0xFF) as i32,
                ((v.color >> 16) & 0xFF) as i32,
            );
            (60..=115).contains(&r)
                && (60..=115).contains(&g)
                && (60..=125).contains(&b)
                && (b - r).abs() <= 12
                && (g - r).abs() <= 3
        };
        let track = horizon_client::hud::rgba(255, 255, 255, 36);
        for action in [
            Action::Play {
                folder: "World".into(),
            },
            Action::Create {
                folder: "World".into(),
                name: "World".into(),
                seed: None,
                creative: false,
                options: Default::default(),
            },
            Action::Connect {
                address: "localhost:25565".into(),
                lan: true,
            },
        ] {
            let (_send, receiver) = std::sync::mpsc::channel();
            let mut loading = loading(receiver);
            loading.set_ui(gui(&root), root.clone());
            loading.menu.queue_entry(action, "Player".into());
            let mut batch = HudBatch::new([1280, 720]);
            assert!(
                !loading.draw_menu(&mut batch),
                "the wait is not a title frame"
            );
            let (w, h) = (batch.width, batch.height);
            // The backdrop's black field is the frame's base.
            let bg = &batch.vertices[..6];
            assert!(bg.iter().all(|v| v.color == 0xFF00_0000));
            assert!(bg.iter().all(|v| v.pos[0] == 0.0 || v.pos[0] == w));
            assert!(bg.iter().all(|v| v.pos[1] == 0.0 || v.pos[1] == h));
            // No polished wordmark in the centre band: no lockup.
            assert!(
                !batch
                    .vertices
                    .iter()
                    .any(|v| v.pos[1] > h * 0.3 && v.pos[1] < h * 0.6 && dark_stop(v)),
                "a queued entry never wears the boot lockup"
            );
            // The bar is the composite bar, not the boot bar: its
            // track spans exactly the centred 200 GUI px, its white
            // strips cap that span, and no solid-white geometry
            // reaches outside it — the boot bar's strips run nearly
            // the frame's full width along the bottom. (The caption
            // and wordmark cannot be probed here: this harness's
            // atlas carries font metrics only, so glyphs emit no
            // vertices; the shared draw's text is pinned by the
            // ui_theme tests.)
            let left = (w - 200.0) / 2.0;
            let track_ys: Vec<f32> = batch
                .vertices
                .iter()
                .filter(|v| v.color == track)
                .map(|v| v.pos[1])
                .collect();
            assert_eq!(track_ys.len(), 6, "the composite bar's track is drawn");
            let track_y = track_ys[0];
            let track_xs: Vec<f32> = batch
                .vertices
                .iter()
                .filter(|v| v.color == track)
                .map(|v| v.pos[0])
                .collect();
            let (tx0, tx1) = track_xs
                .iter()
                .fold((f32::MAX, f32::MIN), |(a, b), x| (a.min(*x), b.max(*x)));
            assert!(
                (tx0 - left).abs() < 1.0 && (tx1 - (left + 200.0)).abs() < 1.0,
                "the track spans the centred bar: {tx0}..{tx1}"
            );
            let white: Vec<&horizon_client::hud::HudVertex> = batch
                .vertices
                .iter()
                .filter(|v| v.color == u32::MAX)
                .collect();
            assert!(
                white
                    .iter()
                    .all(|v| v.pos[0] >= left - 1.0 && v.pos[0] <= left + 201.0),
                "no white geometry outside the composite bar's span"
            );
            assert!(
                white.iter().any(|v| v.pos[1] < track_y)
                    && white.iter().any(|v| v.pos[1] > track_y),
                "the strips cap the bar above and below its track"
            );
        }
    }

    #[test]
    fn boot_covers_the_menu_while_the_core_worker_is_blocked() {
        let root = std::env::temp_dir().join("horizon-startup-menu-no-world-resources");
        let (sender, receiver) = std::sync::mpsc::channel();
        let (release, wait) = std::sync::mpsc::channel();
        let (published, published_rx) = std::sync::mpsc::channel();
        let worker_root = root.clone();
        let worker = std::thread::spawn(move || {
            sender
                .send(LoaderMsg::Ui(gui(&worker_root), worker_root))
                .unwrap();
            published.send(()).unwrap();
            wait.recv().unwrap();
            sender
                .send(LoaderMsg::Done(Err("core failure".into())))
                .unwrap();
        });
        published_rx.recv().unwrap();
        let mut loading = loading(receiver);
        loading.poll_menu_resources().unwrap();
        assert!(loading.gui.is_some());
        assert!(loading.loaded.is_none());
        assert!(!worker.is_finished());
        // The bundle is in but the core is still loading: the boot
        // screen covers the window — the title is neither drawn nor
        // input-live — where the old flow presented a usable menu.
        assert!(loading.boot_covering());
        let mut batch = HudBatch::new([1280, 720]);
        assert!(
            !loading.draw_menu(&mut batch),
            "no title frame while loading runs"
        );
        let (w, h) = (batch.width, batch.height);
        let bg = &batch.vertices[..6];
        assert!(
            bg.iter().all(|v| v.color == 0xFF00_0000),
            "the boot screen's black field is the frame's base"
        );
        assert!(bg.iter().all(|v| v.pos[0] == 0.0 || v.pos[0] == w));
        assert!(bg.iter().all(|v| v.pos[1] == 0.0 || v.pos[1] == h));
        // State-level navigation survives (it is how launch arguments
        // and a pack reload reach the menu), and replacing fallback
        // artwork preserves it.
        let title = loading.menu.title.as_mut().unwrap();
        title.mode = horizon_client::title::Mode::Create {
            name: String::new(),
            seed: "42".into(),
            creative: true,
            focus: 0,
            options: Default::default(),
        };
        title.type_text("My world");
        loading.set_ui(gui(&root), root);
        let action = loading.menu.title.as_ref().unwrap().create_action();
        loading.menu.queue_entry(action, "Player".into());
        assert!(
            matches!(&loading.menu.pending, Some(PendingWorldEntry::Local {
            name: Some(name), seed: Some(42), creative: true, create: Some(_), ..
        }) if name == "My world")
        );
        // A queued entry's own overlay owns the window, not the boot
        // screen and not the title.
        assert!(!loading.boot_covering());
        let mut batch = HudBatch::new([1280, 720]);
        assert!(
            !loading.draw_menu(&mut batch),
            "the overlay is not a title frame"
        );
        assert!(
            !batch.vertices.is_empty(),
            "queued entry draws loading progress"
        );
        release.send(()).unwrap();
        worker.join().unwrap();
        loading.poll_menu_resources().unwrap();
        assert!(matches!(loading.loaded, Some(Err(ref error)) if error == "core failure"));
    }

    #[test]
    fn the_finish_sequence_starts_at_the_last_checkpoints() {
        let (_send, receiver) = std::sync::mpsc::channel();
        let mut loading = loading(receiver);
        // Below the last checkpoints nothing starts, and no sequence is
        // owed while the loader is still running.
        loading.progress.advance_to(0.90);
        loading.maybe_start_boot_finish();
        assert!(loading.finishing.is_none());
        assert!(loading.boot_finish_done());
        // At the 0.95 band the sequence starts as part of loading
        // itself; the bar goes full (the only place 1.0 is set) and
        // the handoff owes the sequence until it plays out.
        loading.progress.advance_to(0.95);
        loading.maybe_start_boot_finish();
        assert!(loading.finishing.is_some());
        assert_eq!(loading.progress.fraction(), 1.0);
        assert!(!loading.boot_finish_done());
        // This load just started, so its length clamps to the floor.
        assert_eq!(
            loading.finishing_len,
            horizon_client::ui_theme::BOOT_SEQ_MIN
        );
        // Once it has played out, nothing more is owed.
        loading.finishing =
            Instant::now().checked_sub(loading.finishing_len + Duration::from_millis(10));
        assert!(loading.boot_finish_done());
    }

    #[test]
    fn a_queued_entry_and_a_loader_error_both_skip_the_sequence() {
        {
            let (_send, receiver) = std::sync::mpsc::channel();
            let mut loading = loading(receiver);
            loading.menu.queue_entry(
                Action::Connect {
                    address: "localhost:25565".into(),
                    lan: true,
                },
                "Guest".into(),
            );
            loading.progress.advance_to(0.97);
            loading.maybe_start_boot_finish();
            assert!(
                loading.finishing.is_none(),
                "suppressed by the queued entry"
            );
            assert!(loading.boot_finish_done(), "nothing owed");
        }
        {
            let (_send, receiver) = std::sync::mpsc::channel();
            let mut loading = loading(receiver);
            loading.loaded = Some(Err("core failure".into()));
            loading.progress.advance_to(0.97);
            loading.maybe_start_boot_finish();
            assert!(loading.finishing.is_none(), "skipped on error");
            assert!(loading.boot_finish_done(), "nothing owed");
        }
    }

    #[test]
    fn the_sequence_dissolves_into_the_title_and_then_leaves_it_alone() {
        let root = std::env::temp_dir().join("horizon-startup-menu-boot-dissolve");
        let (_send, receiver) = std::sync::mpsc::channel();
        let mut loading = loading(receiver);
        loading.set_ui(gui(&root), root.clone());
        // (a) Before the trigger the batch is the boot screen alone:
        // its opaque black field is the frame's base quad — under the
        // old flow the title's quads came first — and `draw_menu`
        // reports no title in the batch (a vertex count can't say this:
        // the shared glyph cache changes text-quad counts between a
        // first and a second draw).
        let mut batch = HudBatch::new([1280, 720]);
        assert!(!loading.draw_menu(&mut batch));
        assert!(!batch.vertices.is_empty());
        let (w, h) = (batch.width, batch.height);
        let bg = &batch.vertices[..6];
        assert!(bg.iter().all(|v| v.color == 0xFF00_0000), "boot fill first");
        assert!(bg.iter().all(|v| v.pos[0] == 0.0 || v.pos[0] == w));
        assert!(bg.iter().all(|v| v.pos[1] == 0.0 || v.pos[1] == h));
        // (b) Mid-fade the title is in the batch, under the boot layer:
        // the boot fill is a full-screen pure-black quad whose alpha is
        // the sequence's own fade (about 32 at 2.75s of 3.6s — the
        // title's own full-screen dim sits at 160, so a low alpha is
        // the boot field's alone), and it does not come first.
        loading.finishing = Instant::now().checked_sub(Duration::from_millis(2750));
        let mut batch = HudBatch::new([1280, 720]);
        assert!(loading.draw_menu(&mut batch), "the title is the underlay");
        let (w, h) = (batch.width, batch.height);
        let fill_at = batch.vertices.iter().position(|v| {
            v.color & 0x00FF_FFFF == 0
                && (1..=64).contains(&(v.color >> 24))
                && (v.pos == [0.0, 0.0]
                    || v.pos == [w, 0.0]
                    || v.pos == [0.0, h]
                    || v.pos == [w, h])
        });
        assert!(
            matches!(fill_at, Some(at) if at > 0),
            "the boot fill rides the fade, over title quads drawn first"
        );
        // (c) After the sequence the boot screen contributes nothing:
        // the frame is exactly what the screens alone draw (the title
        // has its own full-screen dim quad, so "no black quad" can't
        // say this — a batch with zero boot quads in it can).
        loading.finishing = Instant::now().checked_sub(Duration::from_millis(3700));
        let mut batch = HudBatch::new([1280, 720]);
        assert!(loading.draw_menu(&mut batch), "the title is live");
        let mut screens_only = HudBatch::new([1280, 720]);
        assert!(loading.draw_screens(&mut screens_only));
        assert_eq!(
            batch.vertices.len(),
            screens_only.vertices.len(),
            "no boot quads left on the title's frame"
        );
    }

    #[test]
    fn queued_server_keeps_address_player_identity_and_lan_kind() {
        let mut menu = StartupMenu::default();
        menu.queue_entry(
            Action::Connect {
                address: "localhost:25565".into(),
                lan: true,
            },
            "Guest".into(),
        );
        assert!(
            matches!(menu.pending.take(), Some(PendingWorldEntry::Server { address, name, lan: true, .. })
            if address == "localhost:25565" && name == "Guest")
        );
        assert!(menu.pending.take().is_none(), "an entry is consumed once");
    }

    #[test]
    fn a_failed_generation_keeps_its_error_after_the_loader_ends() {
        let (sender, receiver) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            sender
                .send(LoaderMsg::Done(Err("missing facts.json".into())))
                .unwrap();
        });
        let mut loading = loading(receiver);
        loading.handle = Some(worker);
        while loading.loaded.is_none() {
            loading.poll_menu_resources().unwrap();
        }
        // App::about_to_wait takes the result to report it; the loop may poll
        // once more before it exits, with the loader gone.
        assert!(matches!(loading.loaded.take(), Some(Err(e)) if e == "missing facts.json"));
        while !loading.handle.as_ref().is_none_or(JoinHandle::is_finished) {
            std::thread::yield_now();
        }
        loading.poll_menu_resources().unwrap();
        assert!(loading.loaded.is_none());
    }

    #[test]
    fn a_panicking_loader_reports_its_panic() {
        let (sender, receiver) = std::sync::mpsc::channel::<LoaderMsg>();
        let worker = std::thread::spawn(move || {
            let _sender = sender;
            panic!("atlas has no fonts");
        });
        while !worker.is_finished() {
            std::thread::yield_now();
        }
        let mut loading = loading(receiver);
        loading.handle = Some(worker);
        assert_eq!(
            loading.poll_menu_resources().unwrap_err().to_string(),
            "resource loader stopped before finishing: atlas has no fonts"
        );
    }

    #[test]
    fn disconnected_core_worker_is_reported_instead_of_waiting_forever() {
        let (sender, receiver) = std::sync::mpsc::channel();
        drop(sender);
        let mut loading = loading(receiver);
        assert_eq!(
            loading.poll_menu_resources().unwrap_err().to_string(),
            "resource loader stopped before finishing: no loader thread was running"
        );
    }
}
