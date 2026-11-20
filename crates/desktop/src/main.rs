#![forbid(unsafe_code)]
#![cfg_attr(
    all(target_os = "windows", not(debug_assertions)),
    windows_subsystem = "windows"
)]

mod about;
mod batch;
mod locale;
#[cfg(target_os = "macos")]
mod macmenu;
mod png_hint;
mod style;
#[cfg(target_os = "macos")]
mod update_ui;
#[cfg(target_os = "macos")]
mod updates;
mod views;

mod recovery;
mod results;
use batch::{Event, Task, WriteMode};
use eframe::egui::{self, Align, FontFamily, FontId, Layout, Margin, RichText, Stroke, Vec2};
use locale::{Lang, LanguageChoice};
use pixminima_core::FileReport;
use pixminima_core::{ComputeMode, Effort};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::time::Duration;
use style::{Palette, configure, secondary, strong};

fn simple_name(p: &Path) -> String {
    p.file_name()
        .unwrap_or(p.as_os_str())
        .to_string_lossy()
        .into_owned()
}
fn full(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

#[derive(Default)]
struct Results {
    rows: Vec<Row>,
    indices: std::collections::HashMap<PathBuf, usize>,
    order: Vec<usize>,
    dirty: bool,
    sort: results::Sort,
    descending: bool,
    task: Task,
    mode: WriteMode,
    input: u64,
    output: u64,
    errors: usize,
    skipped: usize,
    completed: usize,
    total: usize,
    folder: Option<PathBuf>,
}
enum Outcome {
    Saved(FileReport, PathBuf),
    Error(String),
    Skipped,
    Restored(u64),
    Pending,
    Running,
    Cancelled,
}
struct Row {
    name: String,
    folded: String,
    path: PathBuf,
    outcome: Outcome,
}

struct Active {
    events: mpsc::Receiver<Event>,
    cancel: Arc<AtomicBool>,
    now: PathBuf,
}
#[derive(Clone, Copy, PartialEq)]
enum Phase {
    Idle,
    Scanning,
    Running,
    Finished,
    Cancelled,
    Empty,
    Failed,
}

struct Desktop {
    lang: Lang,
    language: LanguageChoice,
    appearance: egui::ThemePreference,
    compute: ComputeMode,
    settings: bool,
    #[cfg(target_os = "macos")]
    update_settings: bool,
    #[cfg(target_os = "macos")]
    updater: update_ui::Updater,
    about: bool,
    lossy: bool,
    task: Task,
    native_toolbar: bool,
    png_selection: png_hint::Selection,
    write_mode: WriteMode,
    restore_request: Option<PathBuf>,
    restoring: Option<(PathBuf, mpsc::Receiver<Result<(), String>>)>,
    locale_check: std::time::Instant,
    #[cfg(target_os = "macos")]
    menus: Option<macmenu::Commands>,
    effort: Effort,
    paths: Vec<PathBuf>,
    destination: Option<PathBuf>,
    results: Results,
    phase: Phase,
    active: Option<Active>,
    notice: Option<String>,
    close_after: bool,
    detail: Option<usize>,
    #[cfg(target_os = "macos")]
    finder: fmv_macos_events::Inbox,
}

impl Desktop {
    fn new(
        cc: &eframe::CreationContext<'_>,
        #[cfg(target_os = "macos")] finder: fmv_macos_events::Inbox,
    ) -> Self {
        #[cfg(target_os = "macos")]
        {
            let ctx = cc.egui_ctx.clone();
            finder.set_wake(move || ctx.request_repaint());
        }
        configure(&cc.egui_ctx);
        let language = LanguageChoice::from_saved(
            cc.storage.and_then(|s| s.get_string("language")).as_deref(),
        );
        let lang = language.resolve(sys_locale::get_locale().as_deref());
        let appearance = match cc
            .storage
            .and_then(|s| s.get_string("appearance"))
            .as_deref()
        {
            Some("light") => egui::ThemePreference::Light,
            Some("dark") => egui::ThemePreference::Dark,
            _ => egui::ThemePreference::System,
        };
        cc.egui_ctx.set_theme(appearance);
        let compute = match cc.storage.and_then(|s| s.get_string("compute")).as_deref() {
            Some("cpu") => ComputeMode::Cpu,
            #[cfg(target_os = "macos")]
            Some("metal") => ComputeMode::Metal,
            _ => ComputeMode::Auto,
        };
        let effort = stored_effort(
            cc.storage.and_then(|s| s.get_string("effort")).as_deref(),
            cc.storage.and_then(|s| s.get_string("thorough")).as_deref(),
        );
        #[cfg(target_os = "macos")]
        if let Some(render) = &cc.wgpu_render_state {
            pixminima_core::share_metal_device(&render.device, &render.queue);
        }
        let args = std::env::args_os()
            .skip(1)
            .filter(|a| !a.to_string_lossy().starts_with("-psn_"))
            .map(PathBuf::from)
            .filter(|p| p.is_file() || p.is_dir())
            .collect();
        Self {
            lang,
            language,
            appearance,
            compute,
            settings: false,
            #[cfg(target_os = "macos")]
            update_settings: false,
            #[cfg(target_os = "macos")]
            updater: update_ui::Updater::new(cc.winit_window().is_some(), cc.storage),
            about: false,
            lossy: cc.storage.and_then(|s| s.get_string("lossy")).as_deref() == Some("true"),
            task: Task::Optimize,
            png_selection: Default::default(),
            native_toolbar: cfg!(target_os = "macos") && cc.winit_window().is_some(),
            write_mode: WriteMode::from_saved(
                cc.storage
                    .and_then(|s| s.get_string("write_mode"))
                    .as_deref(),
                cc.storage.and_then(|s| s.get_string("replace")).as_deref(),
            ),
            restore_request: None,
            restoring: None,
            locale_check: std::time::Instant::now(),
            #[cfg(target_os = "macos")]
            menus: if cc.winit_window().is_some() {
                macmenu::Commands::new(&cc.egui_ctx, lang).ok()
            } else {
                None
            },
            effort,
            paths: args,
            destination: None,
            results: Results::default(),
            phase: Phase::Idle,
            active: None,
            notice: None,
            close_after: false,
            detail: None,
            #[cfg(target_os = "macos")]
            finder,
        }
    }

    fn add(&mut self, files: impl IntoIterator<Item = PathBuf>) {
        let mut present: std::collections::HashSet<_> = self.paths.iter().cloned().collect();
        for path in files {
            if present.insert(path.clone()) {
                self.paths.push(path);
            }
        }
        self.notice = if self.active.is_some() {
            Some(
                self.lang
                    .pick(
                        "Sélection ajoutée au prochain lot.",
                        "Selection added to the next batch.",
                        "Selección añadida al próximo lote.",
                    )
                    .into(),
            )
        } else {
            None
        };
    }
    fn files(&mut self) {
        let l = self.lang;
        let (title, formats): (&str, &[&str]) = match self.task {
            Task::Optimize => (
                l.pick("Ajouter des images", "Add images", "Añadir imágenes"),
                pixminima_core::EXTENSIONS,
            ),
            #[cfg(feature = "archive-jxl")]
            Task::Archive => (
                l.pick(
                    "Choisir des JPEG à archiver",
                    "Choose JPEGs to archive",
                    "Elegir archivos JPEG para archivar",
                ),
                &["jpg", "jpeg", "jpe", "jfif"],
            ),
            #[cfg(feature = "archive-jxl")]
            Task::RestoreJpeg => (
                l.pick(
                    "Choisir des archives JPEG XL",
                    "Choose JPEG XL archives",
                    "Elegir archivos JPEG XL",
                ),
                &["jxl"],
            ),
        };
        let dialog = rfd::FileDialog::new()
            .set_title(title)
            .add_filter(l.pick("Images", "Images", "Imágenes"), formats);
        #[cfg(not(target_os = "macos"))]
        let dialog = dialog.add_filter(
            l.pick("Tous les fichiers", "All files", "Todos los archivos"),
            &["*"],
        );
        if let Some(paths) = dialog.pick_files() {
            self.add(paths);
        }
    }
    fn folder(&mut self) {
        if let Some(path) = rfd::FileDialog::new()
            .set_title(
                self.lang
                    .pick("Ajouter un dossier", "Add a folder", "Añadir una carpeta"),
            )
            .pick_folder()
        {
            self.add([path]);
        }
    }
    fn effective_write_mode(&self) -> WriteMode {
        if self.task == Task::Optimize {
            self.write_mode
        } else {
            WriteMode::Copies
        }
    }
    fn installing_update(&self) -> bool {
        #[cfg(target_os = "macos")]
        {
            self.updater.installing()
        }
        #[cfg(not(target_os = "macos"))]
        {
            false
        }
    }

    fn has_pending_work(&self) -> bool {
        self.active.is_some() || self.restoring.is_some() || self.installing_update()
    }

    fn ready_to_start(&self) -> bool {
        !self.paths.is_empty()
            && !self.has_pending_work()
            && self.restore_request.is_none()
            && self.detail.is_none()
            && !self.settings
            && !self.about
    }

    fn start(&mut self) {
        if !self.ready_to_start() {
            return;
        }
        let paths = self.paths.clone();
        let parent = self.destination.clone();
        let task = self.task;
        let options = batch::Options {
            effort: self.effort,
            lossy: self.lossy && task == Task::Optimize,
            compute: self.compute,
            mode: self.effective_write_mode(),
            task,
        };
        let cancel = Arc::<AtomicBool>::default();
        let flag = cancel.clone();
        let (tx, rx) = mpsc::channel();
        match std::thread::Builder::new()
            .name("pixminima-batch".into())
            .spawn(move || {
                batch::process(&paths, parent.as_deref(), options, &flag, &tx);
            }) {
            Ok(_) => {
                self.paths.clear();
                self.detail = None;
                self.phase = Phase::Scanning;
                self.notice = None;
                self.results = Results {
                    task,
                    mode: options.mode,
                    ..Results::default()
                };
                self.active = Some(Active {
                    events: rx,
                    cancel,
                    now: PathBuf::new(),
                });
            }
            Err(e) => {
                self.phase = Phase::Failed;
                self.notice = Some(e.to_string());
            }
        }
    }
    fn cancel(&mut self) {
        if let Some(active) = &self.active {
            active.cancel.store(true, Ordering::Release);
        }
    }
    fn collect(&mut self, ctx: &egui::Context) {
        if self.active.is_none() {
            return;
        }
        for _ in 0..512 {
            let Some(active) = &mut self.active else {
                break;
            };
            match active.events.try_recv() {
                Ok(Event::Ready {
                    sources,
                    destination,
                }) => {
                    self.phase = Phase::Running;
                    self.results.total = sources.len();
                    for path in sources {
                        self.results.update(path, Outcome::Pending);
                    }
                    self.results.folder = destination;
                }
                Ok(Event::Started(path)) => {
                    active.now = path.clone();
                    self.results.update(path, Outcome::Running);
                }
                Ok(Event::File { source, result }) => {
                    if self.phase == Phase::Running {
                        self.results.completed += 1;
                    }
                    let outcome = match result {
                        Ok((report, dest)) => {
                            self.results.input += report.before;
                            self.results.output += report.after;

                            Outcome::Saved(report, dest)
                        }
                        Err(detail) => {
                            self.results.errors += 1;
                            Outcome::Error(detail)
                        }
                    };
                    self.results.update(source, outcome);
                }
                Ok(Event::Skipped(path)) => {
                    self.results.skipped += 1;
                    self.results.update(path, Outcome::Skipped);
                }
                Ok(Event::Failed(detail)) => {
                    self.notice = Some(format!("{}\n{}", self.lang.error(&detail), detail));
                    self.phase = Phase::Failed;
                    self.active = None;
                }
                Ok(Event::Empty) => {
                    self.phase = Phase::Empty;
                    self.active = None;
                }
                Ok(Event::Finished { cancelled }) => {
                    self.phase = if cancelled {
                        for row in &mut self.results.rows {
                            if matches!(row.outcome, Outcome::Pending | Outcome::Running) {
                                row.outcome = Outcome::Cancelled;
                            }
                        }
                        self.results.dirty = true;
                        Phase::Cancelled
                    } else {
                        Phase::Finished
                    };
                    self.active = None;
                }
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.phase = Phase::Failed;
                    self.notice = Some(
                        self.lang
                            .pick(
                                "Le traitement s’est interrompu.",
                                "The processing worker stopped.",
                                "El proceso se ha interrumpido.",
                            )
                            .into(),
                    );
                    self.active = None;
                }
            }
        }
        if self.active.is_some() {
            ctx.request_repaint_after(Duration::from_millis(90));
        }
    }

    fn request_close(&mut self, ctx: &egui::Context) {
        if self.has_pending_work() {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.close_after = true;
            self.cancel();
        } else {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }

    fn input(&mut self, ctx: &egui::Context) {
        let dropped = ctx.input(|i| {
            i.raw
                .dropped_files
                .iter()
                .map(|f| f.path().to_owned())
                .collect::<Vec<_>>()
        });
        if !dropped.is_empty() {
            self.add(dropped);
        }
        let keyboard = |mods, key| ctx.input_mut(|i| i.consume_key(mods, key));
        if ctx.input(|i| i.viewport().close_requested()) && self.has_pending_work() {
            self.request_close(ctx);
        }
        if self.restore_request.is_some() {
            if keyboard(egui::Modifiers::NONE, egui::Key::Escape) {
                self.restore_request = None;
            }
            return;
        }
        if keyboard(egui::Modifiers::COMMAND, egui::Key::Comma) {
            self.settings = true;
            return;
        }
        if self.about {
            if keyboard(egui::Modifiers::NONE, egui::Key::Escape) {
                self.about = false;
            }
            return;
        }
        if self.settings || self.detail.is_some() {
            if keyboard(egui::Modifiers::NONE, egui::Key::Escape) {
                self.settings = false;
                self.detail = None;
            }
            return;
        }
        if self.active.is_some() {
            if keyboard(egui::Modifiers::NONE, egui::Key::Escape) {
                self.cancel();
            }
        } else if keyboard(
            egui::Modifiers::COMMAND | egui::Modifiers::SHIFT,
            egui::Key::O,
        ) {
            self.folder();
        } else if keyboard(egui::Modifiers::COMMAND, egui::Key::O) {
            self.files();
        } else if keyboard(egui::Modifiers::COMMAND, egui::Key::Enter) {
            self.start();
        }
    }

    fn refresh_language(&mut self) {
        let next = self.language.resolve(sys_locale::get_locale().as_deref());
        if self.lang != next {
            self.lang = next;
            #[cfg(target_os = "macos")]
            if let Some(menu) = &mut self.menus
                && let Err(e) = menu.translate(next)
            {
                self.notice = Some(e.to_string());
            }
        }
    }
}

impl Desktop {
    fn detail_window(&mut self, ctx: &egui::Context) {
        let can_restore = !self.has_pending_work();
        let Some(row) = self.detail.and_then(|i| self.results.rows.get(i)) else {
            return;
        };
        let l = self.lang;
        let mut opened = true;
        egui::Window::new(l.pick("Détails du fichier", "File details", "Detalles del archivo"))
            .id(egui::Id::unique("file-detail"))
            .open(&mut opened)
            .collapsible(false)
            .default_width(420.)
            .show(ctx, |ui| {
                let p = Palette::for_ui(ui);
                ui.label(strong(simple_name(&row.path), 18.));
                ui.label(secondary(full(&row.path), p));
                if ui
                    .button(l.pick(
                        "Afficher dans le dossier",
                        "Reveal in folder",
                        "Mostrar en su carpeta",
                    ))
                    .clicked()
                    && let Err(e) = reveal_file(&row.path)
                {
                    self.notice = Some(e.to_string());
                }
                match &row.outcome {
                    Outcome::Saved(report, out) => {
                        ui.separator();
                        ui.label(row.outcome.status(l));
                        ui.label(format!(
                            "{} · {}",
                            report.format,
                            l.sizes(report.before, report.after)
                        ));
                        ui.label(secondary(
                            format!(
                                "{} → {}  ·  {} {}",
                                l.exact_bytes(report.before),
                                l.exact_bytes(report.after),
                                if report.after > report.before {
                                    l.pick("augmentation", "increase", "aumento")
                                } else {
                                    l.pick("réduction", "reduction", "reducción")
                                },
                                l.exact_bytes(report.before.abs_diff(report.after))
                            ),
                            p,
                        ));
                        if out != &row.path {
                            ui.label(secondary(full(out), p));
                            if ui
                                .button(l.pick(
                                    "Afficher la copie",
                                    "Show output copy",
                                    "Mostrar la copia",
                                ))
                                .clicked()
                                && let Err(e) = reveal_file(out)
                            {
                                self.notice = Some(e.to_string());
                            }
                        }
                    }
                    Outcome::Error(detail) => {
                        ui.label(RichText::new(l.error(detail)).color(p.error));
                        ui.collapsing(
                            l.pick("Détail technique", "Technical detail", "Detalle técnico"),
                            |ui| {
                                ui.label(detail);
                            },
                        );
                    }
                    Outcome::Skipped => {
                        ui.label(l.pick(
                            "Aucune image prise en charge.",
                            "No supported image.",
                            "Ninguna imagen compatible.",
                        ));
                    }
                    Outcome::Pending | Outcome::Running | Outcome::Cancelled => {
                        ui.label(row.outcome.status(l));
                    }
                    Outcome::Restored(_) => {
                        ui.label(l.pick(
                            "L’original a été restauré.",
                            "The original was restored.",
                            "El original se ha restaurado.",
                        ));
                    }
                }
                if pixminima_core::has_backup(&row.path)
                    && ui
                        .add_enabled(
                            can_restore,
                            style::button(l.pick(
                                "Restaurer l’original…",
                                "Restore original…",
                                "Restaurar el original…",
                            )),
                        )
                        .clicked()
                {
                    self.restore_request = Some(row.path.clone());
                }
            });
        if !opened {
            self.detail = None;
        }
    }
}

fn reveal_file(path: &Path) -> std::io::Result<()> {
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open")
            .arg("-R")
            .arg(path)
            .spawn()
            .map(|_| ())
    }
    #[cfg(target_os = "windows")]
    {
        let mut selection = std::ffi::OsString::from("/select,");
        selection.push(path.as_os_str());
        std::process::Command::new("explorer")
            .arg(selection)
            .spawn()
            .map(|_| ())
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        show_folder(path.parent().unwrap_or(path))
    }
}

fn show_folder(path: &Path) -> std::io::Result<()> {
    #[cfg(target_os = "macos")]
    let app = "open";
    #[cfg(target_os = "windows")]
    let app = "explorer";
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let app = "xdg-open";
    std::process::Command::new(app)
        .arg(path)
        .spawn()
        .map(|_| ())
}

impl eframe::App for Desktop {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        #[cfg(target_os = "macos")]
        if self.updater.take_install_request() {
            let idle =
                !self.has_pending_work() && self.restore_request.is_none() && !self.close_after;
            if idle {
                if let Some(storage) = _frame.storage_mut() {
                    eframe::App::save(self, storage);
                    storage.flush();
                }
                self.updater.install(true, ctx);
            }
        }
        #[cfg(target_os = "macos")]
        for event in self.finder.drain() {
            match event {
                fmv_macos_events::Event::Open(paths) => {
                    self.add(paths);
                    ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                }
                fmv_macos_events::Event::Quit => self.request_close(ctx),
            }
        }
        if self.language == LanguageChoice::System
            && self.locale_check.elapsed() >= Duration::from_secs(5)
        {
            self.refresh_language();
            self.locale_check = std::time::Instant::now();
        }
        if self.language == LanguageChoice::System {
            ctx.request_repaint_after(Duration::from_secs(5));
        }
        #[cfg(target_os = "macos")]
        {
            let events: Vec<String> = self
                .menus
                .as_ref()
                .map(|m| m.events.try_iter().collect())
                .unwrap_or_default();
            for event in events {
                match event.as_str() {
                    "files" => self.files(),
                    "folder" => self.folder(),
                    "settings" => self.settings = true,
                    "about" => self.about = true,
                    "updates" => {
                        self.about = false;
                        self.settings = true;
                        self.update_settings = true;
                        self.updater.check(ctx);
                    }
                    "restore" => self.pick_restore(),
                    "start" => self.start(),
                    "cancel" => self.cancel(),
                    "output" => {
                        if let Some(out) = &self.results.folder
                            && let Err(e) = show_folder(out)
                        {
                            self.notice = Some(e.to_string());
                        }
                    }
                    "quit" => self.request_close(ctx),
                    _ => {}
                }
            }
            if let Some(menu) = &self.menus {
                menu.enable(
                    self.ready_to_start(),
                    self.active.is_some(),
                    self.results.folder.is_some(),
                    !self.has_pending_work() && self.restore_request.is_none(),
                    self.action_name(),
                );
            }
        }
        self.collect(ctx);
        self.collect_restore(ctx);
        #[cfg(target_os = "macos")]
        if self.updater.tick(ctx) {
            self.close_after = true;
        }
        if self.close_after && !self.has_pending_work() {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }
    fn ui(&mut self, ui: &mut egui::Ui, _: &mut eframe::Frame) {
        self.content(ui);
    }
    fn save(&mut self, s: &mut dyn eframe::Storage) {
        #[cfg(target_os = "macos")]
        self.updater.save(s);
        s.set_string("lossy", self.lossy.to_string());
        s.set_string("write_mode", self.write_mode.stored().into());
        s.set_string("language", self.language.saved().into());
        s.set_string(
            "appearance",
            match self.appearance {
                egui::ThemePreference::System => "system",
                egui::ThemePreference::Light => "light",
                egui::ThemePreference::Dark => "dark",
            }
            .into(),
        );
        s.set_string(
            "compute",
            match self.compute {
                ComputeMode::Auto => "auto",
                ComputeMode::Cpu => "cpu",
                ComputeMode::Metal => "metal",
            }
            .into(),
        );
        s.set_string(
            "effort",
            match self.effort {
                Effort::Standard => "standard",
                Effort::Thorough => "thorough",
                Effort::Maximum => "maximum",
            }
            .into(),
        );
    }
}

fn stored_effort(effort: Option<&str>, old_thorough: Option<&str>) -> Effort {
    match effort {
        Some("standard") => Effort::Standard,
        Some("maximum") => Effort::Maximum,
        Some("thorough") => Effort::Thorough,
        _ if old_thorough == Some("false") => Effort::Standard,
        _ if old_thorough == Some("true") => Effort::Thorough,
        _ => Effort::Maximum,
    }
}

#[cfg(target_os = "macos")]
fn configure_shared_device(setup: &mut eframe::egui_wgpu::WgpuSetup) {
    if let eframe::egui_wgpu::WgpuSetup::CreateNew(setup) = setup {
        let original = Arc::clone(&setup.device_descriptor);
        setup.device_descriptor = Arc::new(move |adapter| {
            let mut descriptor = original(adapter);
            if adapter.get_info().backend == eframe::wgpu::Backend::Metal {
                descriptor.required_features |=
                    adapter.features() & eframe::wgpu::Features::MAPPABLE_PRIMARY_BUFFERS;
            }
            descriptor
        });
    }
}

fn main() -> eframe::Result {
    #[cfg(target_os = "macos")]
    let (_registration, finder) = fmv_macos_events::Registration::install();
    let viewport = egui::ViewportBuilder::default()
        .with_title("PixMinima")
        .with_inner_size([742., 540.])
        .with_min_inner_size([375., 420.])
        .with_app_id("io.github.alejandrosml.PixMinima");
    #[cfg(target_os = "macos")]
    let viewport = viewport
        .with_fullsize_content_view(true)
        .with_title_shown(false)
        .with_titlebar_shown(false)
        .with_titlebar_buttons_shown(true);
    let options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };
    #[cfg(target_os = "macos")]
    let options = {
        let mut options = options;
        configure_shared_device(&mut options.wgpu_options.wgpu_setup);
        options
    };
    eframe::run_native(
        "PixMinima",
        options,
        Box::new(move |cc| {
            Ok(Box::new(Desktop::new(
                cc,
                #[cfg(target_os = "macos")]
                finder,
            )))
        }),
    )
}

#[cfg(test)]
mod gui_tests {
    use super::*;
    use egui_kittest::{Harness, kittest::Queryable};

    fn app_for_test(cc: &mut eframe::CreationContext<'_>, lang: Lang) -> Desktop {
        let mut app = Desktop::new(
            cc,
            #[cfg(target_os = "macos")]
            fmv_macos_events::Inbox::default(),
        );
        app.language = LanguageChoice::Explicit(lang);
        app.refresh_language();
        app.paths.clear();
        app
    }

    #[test]
    fn old_profile_settings_migrate_and_explicit_effort_takes_precedence() {
        assert_eq!(stored_effort(None, None), Effort::Maximum);
        assert_eq!(stored_effort(None, Some("true")), Effort::Thorough);
        assert_eq!(stored_effort(None, Some("false")), Effort::Standard);
        assert_eq!(
            stored_effort(Some("maximum"), Some("false")),
            Effort::Maximum
        );
    }
    #[test]
    fn settings_expose_language_and_restore_system() {
        let mut window = Harness::builder()
            .with_size([724., 586.])
            .build_eframe(|cc| app_for_test(cc, Lang::Fr));
        window.get_by_label("Réglages…").click();
        window.run();
        assert!(window.query_by_label("Langue").is_some());
        window.state_mut().language = LanguageChoice::Explicit(Lang::Es);
        window.run_steps(2);
        assert!(window.query_by_label("Idioma").is_some());
        window.state_mut().language = LanguageChoice::System;
        window.run_steps(2);
        assert_eq!(
            window.state().lang,
            LanguageChoice::System.resolve(sys_locale::get_locale().as_deref())
        );
    }

    #[test]
    fn lossy_requires_an_explicit_choice_and_about_is_accessible_in_each_language() {
        for l in [Lang::Fr, Lang::En, Lang::Es] {
            let mut window = Harness::builder()
                .with_size([760., 920.])
                .build_eframe(|cc| app_for_test(cc, l));
            assert!(!window.state().lossy);
            let settings = l.pick("Réglages…", "Settings…", "Ajustes…");
            window
                .get_by_role_and_label(egui::Role::Button, settings)
                .click();
            window.run_steps(3);
            let label = l.pick(
                "Autoriser la compression avec perte",
                "Allow lossy compression",
                "Permitir compresión con pérdida",
            );
            window.get_by_label(label).click();
            window.run_steps(2);
            assert!(window.state().lossy);
            // The native File menu cannot start a newly enabled lossy run behind Settings.
            window
                .state_mut()
                .paths
                .push(PathBuf::from("selection.png"));
            assert!(!window.state().ready_to_start());
            window.get_by_label(about::about_name(l)).click();
            window.run_steps(3);
            assert!(window.state().about);
            assert!(!window.state().settings);
            assert!(
                window
                    .query_by_label("Alejandro San Martin Lamas")
                    .is_some()
            );
            assert!(
                window
                    .query_by_label(l.pick(
                        "Licences des bibliothèques",
                        "Third-party licenses",
                        "Licencias de las bibliotecas"
                    ))
                    .is_some()
            );
            window.key_press(egui::Key::Escape);
            window.run_steps(3);
            assert!(!window.state().about);
            assert!(
                window
                    .query_by_label(l.pick(
                        "Compression avec perte autorisée",
                        "Lossy compression allowed",
                        "Compresión con pérdida permitida"
                    ))
                    .is_some()
            );
            assert!(
                window
                    .query_by_role_and_label(egui::Role::Button, window.state().action_name())
                    .is_some()
            );
            window
                .get_by_role_and_label(egui::Role::Button, settings)
                .click();
            window.run_steps(3);
            window.get_by_label(label).click();
            window.run_steps(2);
            assert!(!window.state().lossy);
        }
    }

    #[test]
    fn mixed_lossy_batch_labels_the_winning_encoder_and_preserves_an_unsupported_original() {
        let work = tempfile::tempdir().unwrap();
        let jpeg = work.path().join("texture.jpg");
        let bitmap = work.path().join("dot.pbm");
        std::fs::write(
            &jpeg,
            include_bytes!("../../formats/tests/fixtures/requantize.jpg"),
        )
        .unwrap();
        std::fs::write(&bitmap, b"P1\n1 1\n0\n").unwrap();
        let mut window = Harness::builder()
            .with_size([760., 820.])
            .build_eframe(|cc| app_for_test(cc, Lang::En));
        window
            .get_by_role_and_label(egui::Role::Button, "Settings…")
            .click();
        window.run_steps(4);
        window.get_by_label("Allow lossy compression").click();
        window.run_steps(4);
        window.key_press(egui::Key::Escape);
        window.run_steps(4);
        window.state_mut().effort = Effort::Thorough;
        window.state_mut().add([jpeg.clone(), bitmap.clone()]);
        window.run_steps(4);
        window
            .get_by_role_and_label(egui::Role::Button, "Compress and replace (lossy)")
            .click();
        window.run_steps(4);
        let until = std::time::Instant::now() + Duration::from_secs(30);
        while window.state().active.is_some() && std::time::Instant::now() < until {
            window.step();
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(matches!(window.state().phase, Phase::Finished));
        assert_eq!(window.state().results.errors, 0);
        let rows = &window.state().results.rows;
        assert!(rows.iter().any(|r| r.name == "texture.jpg"
            && matches!(&r.outcome, Outcome::Saved(report, _) if report.lossy_used)));
        assert!(rows.iter().any(|r| r.name == "dot.pbm"
            && matches!(&r.outcome, Outcome::Saved(report, _) if !report.lossy_used)));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn window_close_and_restore_are_deferred_during_an_update_transaction() {
        let mut window = Harness::builder()
            .with_size([742., 540.])
            .build_eframe(|cc| app_for_test(cc, Lang::En));
        let finish_install = window.state_mut().updater.simulate_install_in_progress();
        window.state_mut().paths.push("test.png".into());
        assert!(!window.state().ready_to_start());
        window.state_mut().pick_restore();
        assert!(window.state().restore_request.is_none());
        window.state_mut().restore_request = Some("test.png".into());
        window
            .input_mut()
            .viewports
            .entry(egui::ViewportId::ROOT)
            .or_default()
            .events
            .push(egui::ViewportEvent::Close);
        window.step();
        assert!(window.state().close_after);
        assert!(window.state().restore_request.is_none());
        assert!(window.state().installing_update());
        let commands = &window.output().viewport_output[&egui::ViewportId::ROOT].commands;
        assert!(commands.contains(&egui::ViewportCommand::CancelClose));
        assert!(!commands.contains(&egui::ViewportCommand::Close));
        finish_install();
        window.step();
        assert!(
            window.output().viewport_output[&egui::ViewportId::ROOT]
                .commands
                .contains(&egui::ViewportCommand::Close)
        );
    }

    #[test]
    fn dropping_folders_and_files_appends_to_the_selection_and_runs_the_recursive_batch() {
        #[derive(Debug)]
        struct DropItem(PathBuf);
        impl egui::DroppedFile for DropItem {
            fn bytes(&self) -> Result<Vec<u8>, String> {
                std::fs::read(&self.0).map_err(|e| e.to_string())
            }
            fn path(&self) -> &Path {
                &self.0
            }
        }
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().join("pictures");
        std::fs::create_dir_all(dir.join("nested")).unwrap();
        let nested = dir.join("nested/a.pbm");
        let distinct = temp.path().join("b.pbm");
        let added = temp.path().join("c.pbm");
        for path in [&nested, &distinct, &added] {
            std::fs::write(path, b"P1\n1 1\n0\n").unwrap();
        }
        let mut window = Harness::builder()
            .with_size([742., 540.])
            .build_eframe(|cc| app_for_test(cc, Lang::En));
        for (batch, expected) in [
            (vec![dir.clone(), distinct.clone()], 2),
            (vec![added, distinct], 3),
        ] {
            window.input_mut().dropped_files = batch
                .into_iter()
                .map(|p| Arc::new(DropItem(p)) as egui::DroppedFileHandle)
                .collect();
            window.run_steps(3);
            assert_eq!(window.state().paths.len(), expected);
        }
        assert!(!window.state().settings);
        window
            .get_by_role_and_label(egui::Role::Button, "Optimize and replace")
            .click();
        window.run_steps(4);
        let until = std::time::Instant::now() + Duration::from_secs(15);
        while window.state().active.is_some() && std::time::Instant::now() < until {
            window.step();
            std::thread::sleep(Duration::from_millis(8));
        }
        assert!(matches!(window.state().phase, Phase::Finished));
        assert_eq!(window.state().results.completed, 3);
        assert_eq!(window.state().results.errors, 0);
        assert!(
            window
                .state()
                .results
                .rows
                .iter()
                .any(|row| row.path == nested.canonicalize().unwrap())
        );
    }

    #[test]
    fn maximum_profile_can_be_selected_and_applied_to_a_batch() {
        let directory = tempfile::tempdir().unwrap();
        let image = directory.path().join("maximum.png");
        let data = include_bytes!("../tests/fixtures/flat.png");
        std::fs::write(&image, data).unwrap();
        let mut window = Harness::builder()
            .with_size([742., 540.])
            .build_eframe(|cc| app_for_test(cc, Lang::En));
        window
            .get_by_role_and_label(egui::Role::Button, "Settings…")
            .click();
        window.run_steps(3);
        window.get_by_label("Maximum").click();
        window.run_steps(2);
        assert_eq!(window.state().effort, Effort::Maximum);
        window.state_mut().settings = false;
        window.state_mut().write_mode = WriteMode::Copies;
        window.state_mut().add([image.clone()]);
        window.run_steps(2);
        window
            .get_by_role_and_label(egui::Role::Button, "Optimize copies")
            .click();
        let end = std::time::Instant::now() + Duration::from_secs(30);
        window.run_steps(2);
        while window.state().active.is_some() && std::time::Instant::now() < end {
            window.step();
            std::thread::sleep(Duration::from_millis(8));
        }
        assert!(matches!(window.state().phase, Phase::Finished));
        assert_eq!(window.state().results.errors, 0);
        assert!(window.state().results.output < window.state().results.input);
        assert_eq!(std::fs::read(image).unwrap(), data);
    }

    #[test]
    fn first_launch_replaces_only_smaller_files_and_keeps_the_mode_for_later_batches() {
        let area = tempfile::tempdir().unwrap();
        let input = area.path().join("first.png");
        let corrupt = area.path().join("damaged.png");
        let bytes = include_bytes!("../tests/fixtures/flat.png");
        std::fs::write(&input, bytes).unwrap();
        std::fs::write(&corrupt, b"broken image").unwrap();
        let mut window = Harness::builder()
            .with_size([742., 540.])
            .build_eframe(|cc| app_for_test(cc, Lang::En));
        assert_eq!(window.state().write_mode, WriteMode::Overwrite);
        for (index, selection) in [vec![input.clone(), corrupt.clone()], vec![input.clone()]]
            .into_iter()
            .enumerate()
        {
            window.state_mut().effort = Effort::Standard;
            window.state_mut().add(selection);
            window.run_steps(3);
            window
                .get_by_role_and_label(egui::Role::Button, "Optimize and replace")
                .click();
            let end = std::time::Instant::now() + Duration::from_secs(20);
            window.run_steps(2);
            while window.state().active.is_some() && std::time::Instant::now() < end {
                window.step();
                std::thread::sleep(Duration::from_millis(8));
            }
            assert!(matches!(window.state().phase, Phase::Finished));
            assert_eq!(window.state().results.errors, usize::from(index == 0));
            assert!(window.state().results.folder.is_none());
            assert_eq!(window.state().write_mode, WriteMode::Overwrite);
            assert_eq!(std::fs::read(&corrupt).unwrap(), b"broken image");
            assert!(!pixminima_core::has_backup(&input));
            if index == 1 {
                assert_eq!(window.state().results.input, window.state().results.output);
                assert!(window.query_all_by_label("0.0%").next().is_some());
            }
        }
        assert!(std::fs::metadata(input).unwrap().len() < bytes.len() as u64);
    }

    #[test]
    fn existing_explicit_write_choices_survive_migration_and_restart() {
        for mode in [WriteMode::Backup, WriteMode::Copies, WriteMode::Overwrite] {
            assert_eq!(
                WriteMode::from_saved(Some(mode.stored()), Some("false")),
                mode
            );
        }
        assert_eq!(WriteMode::from_saved(None, Some("true")), WriteMode::Backup);
        assert_eq!(
            WriteMode::from_saved(None, Some("false")),
            WriteMode::Copies
        );
        assert_eq!(WriteMode::from_saved(None, None), WriteMode::Overwrite);
    }

    /// Rebuild the documentation screenshots from the actual UI, public inputs and a fresh output folder.
    /// PIXMINIMA_QA_IMAGES and PIXMINIMA_QA_CAPTURE must point to dedicated local directories.
    #[test]
    #[ignore = "generates release screenshots using a GPU and a local public image corpus"]
    fn capture_documentation() {
        let input =
            PathBuf::from(std::env::var_os("PIXMINIMA_QA_IMAGES").expect("input directory"));
        let out =
            PathBuf::from(std::env::var_os("PIXMINIMA_QA_CAPTURE").expect("output directory"));
        std::fs::create_dir_all(&out).unwrap();
        for lang in [Lang::Fr, Lang::En, Lang::Es] {
            let builder = Harness::builder()
                .with_size([760., 620.])
                .with_pixels_per_point(2.);
            #[cfg(target_os = "macos")]
            let builder = {
                let mut setup = eframe::egui_wgpu::WgpuSetup::without_display_handle();
                configure_shared_device(&mut setup);
                builder.wgpu_setup(setup)
            };
            #[cfg(not(target_os = "macos"))]
            let builder = builder.wgpu();
            let mut window = builder.build_eframe(|cc| {
                #[cfg(target_os = "macos")]
                if let Some(render) = &cc.wgpu_render_state
                    && render.adapter.get_info().backend == eframe::wgpu::Backend::Metal
                    && render
                        .adapter
                        .features()
                        .contains(eframe::wgpu::Features::MAPPABLE_PRIMARY_BUFFERS)
                {
                    assert!(
                        render
                            .device
                            .features()
                            .contains(eframe::wgpu::Features::MAPPABLE_PRIMARY_BUFFERS)
                    );
                }
                app_for_test(cc, lang)
            });
            for dark in [false, true] {
                let choice = if dark {
                    egui::ThemePreference::Dark
                } else {
                    egui::ThemePreference::Light
                };
                window.state_mut().appearance = choice;
                window.ctx.set_theme(choice);
                window.run_steps(4);
                let theme = if dark { "dark" } else { "light" };
                window
                    .render()
                    .unwrap()
                    .save(out.join(format!("{}-{theme}.png", lang.tag())))
                    .unwrap();
            }
            window.state_mut().appearance = egui::ThemePreference::Light;
            window.ctx.set_theme(egui::ThemePreference::Light);
            window.state_mut().settings = true;
            window.run_steps(4);
            window
                .render()
                .unwrap()
                .save(out.join(format!("{}-settings.png", lang.tag())))
                .unwrap();
            #[cfg(target_os = "macos")]
            {
                window.state_mut().update_settings = true;
                window.run_steps(4);
                window
                    .render()
                    .unwrap()
                    .save(out.join(format!("{}-updates.png", lang.tag())))
                    .unwrap();
                window.state_mut().update_settings = false;
            }
            window.state_mut().settings = false;
            window.state_mut().about = true;
            window.run_steps(4);
            window
                .render()
                .unwrap()
                .save(out.join(format!("{}-about.png", lang.tag())))
                .unwrap();
            window.state_mut().about = false;
            window.state_mut().lossy = true;
            window.run_steps(4);
            window
                .render()
                .unwrap()
                .save(out.join(format!("{}-lossy.png", lang.tag())))
                .unwrap();
            window.state_mut().paths =
                vec![Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/flat.png")];
            window.set_size(Vec2::new(375., 420.));
            window.run_steps(7);
            let button =
                window.get_by_role_and_label(egui::Role::Button, window.state().action_name());
            assert!(
                window.ctx.content_rect().contains_rect(button.rect()),
                "off-screen action in {}",
                lang.tag()
            );
            window
                .render()
                .unwrap()
                .save(out.join(format!("{}-lossy-compact.png", lang.tag())))
                .unwrap();
            window.set_size(Vec2::new(760., 620.));
            let app = window.state_mut();
            app.lossy = std::env::var_os("PIXMINIMA_QA_LOSSY").is_some();
            assert_eq!(app.effort, Effort::Maximum);
            assert_eq!(app.write_mode, WriteMode::Overwrite);
            assert_eq!(app.compute, ComputeMode::Auto);
            // Every default-mode run must have its own disposable input set.
            let temporary = tempfile::tempdir().unwrap();
            let test_inputs = temporary.path().join("Photos");
            std::fs::create_dir(&test_inputs).unwrap();
            let source = input.canonicalize().unwrap();
            for original in pixminima_core::collect_files(std::slice::from_ref(&source)).unwrap() {
                let destination = test_inputs.join(original.strip_prefix(&source).unwrap());
                std::fs::create_dir_all(destination.parent().unwrap()).unwrap();
                std::fs::copy(original, destination).unwrap();
            }
            app.paths = vec![test_inputs];

            app.start();
            let limit = std::time::Instant::now() + Duration::from_secs(600);
            while window.state().active.is_some() && std::time::Instant::now() < limit {
                window.step();
                std::thread::sleep(Duration::from_millis(15));
            }
            for row in &window.state().results.rows {
                if let Outcome::Error(message) = &row.outcome {
                    eprintln!("{}: {message}", simple_name(&row.path));
                }
            }
            let destructive_count = window
                .state()
                .results
                .rows
                .iter()
                .filter(|r| matches!(&r.outcome, Outcome::Saved(report, _) if report.lossy_used))
                .count();
            if std::env::var_os("PIXMINIMA_QA_LOSSY").is_some() {
                for row in &window.state().results.rows {
                    if let Outcome::Saved(report, _) = &row.outcome {
                        eprintln!(
                            "{}: {} {} -> {} (lossy={})",
                            lang.tag(),
                            row.name,
                            report.before,
                            report.after,
                            report.lossy_used
                        );
                    }
                }
                assert!(
                    destructive_count > 0,
                    "test corpus must produce a genuinely lossy result"
                );
            } else {
                assert_eq!(destructive_count, 0);
            }
            eprintln!(
                "{}: {} images, {} bytes before, {} bytes after, {destructive_count} lossy",
                lang.tag(),
                window.state().results.completed,
                window.state().results.input,
                window.state().results.output
            );
            assert!(window.state().phase == Phase::Finished && window.state().results.errors == 0);
            window.run_steps(3);
            window
                .render()
                .unwrap()
                .save(out.join(format!("{}-results.png", lang.tag())))
                .unwrap();
        }
    }
}
