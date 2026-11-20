use super::*;
use std::sync::mpsc::{Receiver, TryRecvError};
use std::time::{SystemTime, UNIX_EPOCH};

const CHECK_INTERVAL: u64 = 24 * 60 * 60;

trait Client: Send + Sync + 'static {
    type Release: Clone + PartialEq + Send + 'static;
    type Download: Send + 'static;
    fn check(&self) -> Result<Option<Self::Release>, String>;
    fn version(release: &Self::Release) -> &str;
    fn download(&self, release: &Self::Release) -> Result<Self::Download, String>;
    fn install(&self, download: &Self::Download) -> Result<(), String>;
    fn matches(download: &Self::Download, release: &Self::Release) -> bool;
}

struct MacClient;
impl Client for MacClient {
    type Release = updates::Release;
    type Download = updates::Download;
    fn check(&self) -> Result<Option<Self::Release>, String> {
        updates::check()
    }
    fn version(release: &Self::Release) -> &str {
        &release.version
    }
    fn download(&self, release: &Self::Release) -> Result<Self::Download, String> {
        updates::download(release)
    }
    fn install(&self, download: &Self::Download) -> Result<(), String> {
        updates::install(download)
    }
    fn matches(download: &Self::Download, release: &Self::Release) -> bool {
        download.matches(release)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Operation {
    Check,
    Download,
    Install,
}

enum Status {
    Idle,
    Working(Operation),
    Current,
    Available,
    Ready,
    Error(Operation, String),
}

enum Message<R, D> {
    Checked(Result<Option<R>, String>),
    Downloaded(Result<D, String>),
    Installed(D, Result<(), String>),
}

struct Controller<C: Client> {
    client: Arc<C>,
    enabled: bool,
    automatic: bool,
    auto_download: bool,
    last_check: u64,
    recheck_after_failure: bool,
    install_requested: bool,
    status: Status,
    release: Option<C::Release>,
    download: Option<C::Download>,
    events: Option<Receiver<Message<C::Release, C::Download>>>,
}

impl<C: Client> Controller<C> {
    fn new(client: C, enabled: bool, storage: Option<&dyn eframe::Storage>) -> Self {
        let saved = |name| storage.and_then(|s| s.get_string(name));
        Self {
            client: Arc::new(client),
            enabled,
            automatic: saved("update_checks").as_deref() != Some("false"),
            auto_download: saved("update_downloads").as_deref() == Some("true"),
            last_check: saved("update_last_check")
                .and_then(|v| v.parse().ok())
                .unwrap_or(0),
            recheck_after_failure: false,
            install_requested: false,
            status: Status::Idle,
            release: None,
            download: None,
            events: None,
        }
    }

    fn spawn(
        &mut self,
        op: Operation,
        ctx: &egui::Context,
        job: impl FnOnce() -> Message<C::Release, C::Download> + Send + 'static,
    ) {
        let (tx, rx) = mpsc::channel();
        let wake = ctx.clone();
        match std::thread::Builder::new()
            .name("pixminima-updater".into())
            .spawn(move || {
                let _ = tx.send(job());
                wake.request_repaint();
            }) {
            Ok(_) => {
                self.events = Some(rx);
                self.status = Status::Working(op);
            }
            Err(e) => self.status = Status::Error(op, e.to_string()),
        }
    }

    fn check(&mut self, now: u64, ctx: &egui::Context) {
        if !self.enabled || self.events.is_some() {
            return;
        }
        self.last_check = now;
        let client = Arc::clone(&self.client);
        self.spawn(Operation::Check, ctx, move || {
            Message::Checked(client.check())
        });
    }

    fn fetch(&mut self, ctx: &egui::Context) {
        if !self.enabled || self.events.is_some() || self.download.is_some() {
            return;
        }
        let Some(release) = self.release.clone() else {
            return;
        };
        let client = Arc::clone(&self.client);
        self.spawn(Operation::Download, ctx, move || {
            Message::Downloaded(client.download(&release))
        });
    }

    fn install(&mut self, idle: bool, ctx: &egui::Context) {
        if !self.enabled || !idle || self.events.is_some() {
            return;
        }
        let Some(download) = self.download.take() else {
            return;
        };
        let client = Arc::clone(&self.client);
        self.spawn(Operation::Install, ctx, move || {
            let answer = client.install(&download);
            Message::Installed(download, answer)
        });
    }

    // Returns true only when the new process was launched successfully.
    fn tick(&mut self, now: u64, ctx: &egui::Context) -> bool {
        if let Some(events) = &self.events {
            let message = match events.try_recv() {
                Err(TryRecvError::Empty) => return false,
                Err(TryRecvError::Disconnected) => {
                    let op = match self.status {
                        Status::Working(op) => op,
                        _ => Operation::Check,
                    };
                    self.status = Status::Error(op, "Worker stopped unexpectedly".into());
                    self.events = None;
                    return false;
                }
                Ok(event) => event,
            };
            self.events = None;
            self.status = match message {
                Message::Checked(Ok(next)) => {
                    let valid = self
                        .download
                        .as_ref()
                        .zip(next.as_ref())
                        .is_some_and(|(download, release)| C::matches(download, release));
                    if self.recheck_after_failure || self.release != next || !valid {
                        self.download = None;
                    }
                    self.recheck_after_failure = false;
                    self.release = next;
                    if self.download.is_some() {
                        Status::Ready
                    } else if self.release.is_some() {
                        Status::Available
                    } else {
                        Status::Current
                    }
                }
                Message::Checked(Err(e)) => Status::Error(Operation::Check, e),
                Message::Downloaded(Ok(artifact)) => {
                    self.download = Some(artifact);
                    Status::Ready
                }
                Message::Downloaded(Err(e)) => Status::Error(Operation::Download, e),
                Message::Installed(_, Ok(())) => {
                    self.status = Status::Idle;
                    return true;
                }
                Message::Installed(artifact, Err(e)) => {
                    self.recheck_after_failure = true;
                    self.download = Some(artifact);
                    Status::Error(Operation::Install, e)
                }
            };
        }
        if self.enabled && self.automatic {
            if self.last_check > now {
                self.last_check = 0;
            }
            let elapsed = now.saturating_sub(self.last_check);
            if elapsed >= CHECK_INTERVAL {
                self.check(now, ctx);
            } else {
                ctx.request_repaint_after(Duration::from_secs(
                    CHECK_INTERVAL.saturating_sub(elapsed).max(1),
                ));
            }
            if self.auto_download && matches!(self.status, Status::Available) {
                self.fetch(ctx);
            }
        }
        false
    }
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

pub struct Updater(Controller<MacClient>);
impl Updater {
    pub fn new(enabled: bool, storage: Option<&dyn eframe::Storage>) -> Self {
        Self(Controller::new(MacClient, enabled && !cfg!(test), storage))
    }
    pub fn save(&self, storage: &mut dyn eframe::Storage) {
        storage.set_string("update_checks", self.0.automatic.to_string());
        storage.set_string("update_downloads", self.0.auto_download.to_string());
        storage.set_string("update_last_check", self.0.last_check.to_string());
    }
    pub fn check(&mut self, ctx: &egui::Context) {
        self.0.check(now(), ctx);
    }
    pub fn tick(&mut self, ctx: &egui::Context) -> bool {
        self.0.tick(now(), ctx)
    }
    #[cfg(test)]
    pub(super) fn simulate_install_in_progress(&mut self) -> impl FnOnce() + use<> {
        let (tx, rx) = mpsc::channel();
        self.0.status = Status::Working(Operation::Install);
        self.0.events = Some(rx);
        move || drop(tx)
    }

    pub fn take_install_request(&mut self) -> bool {
        std::mem::take(&mut self.0.install_requested)
    }
    pub fn install(&mut self, idle: bool, ctx: &egui::Context) {
        self.0.install(idle, ctx);
    }
    pub fn installing(&self) -> bool {
        matches!(self.0.status, Status::Working(Operation::Install))
    }
    pub fn version(&self) -> Option<&str> {
        self.0.release.as_ref().map(MacClient::version)
    }

    pub fn settings(&mut self, ui: &mut egui::Ui, l: Lang, idle: bool) {
        self.0.draw_settings(ui, l, idle);
    }
}

impl<C: Client> Controller<C> {
    fn draw_settings(&mut self, ui: &mut egui::Ui, l: Lang, idle: bool) {
        let state = self;
        let p = Palette::for_ui(ui);
        ui.label(strong(
            format!(
                "{} {}",
                l.pick(
                    "Version installée",
                    "Installed version",
                    "Versión instalada"
                ),
                env!("CARGO_PKG_VERSION")
            ),
            17.,
        ));
        ui.checkbox(
            &mut state.automatic,
            l.pick(
                "Vérifier automatiquement les mises à jour",
                "Automatically check for updates",
                "Buscar actualizaciones automáticamente",
            ),
        );
        ui.label(secondary(
            l.pick(
                "PixMinima vérifie une fois par jour si une nouvelle version est disponible.",
                "PixMinima checks for a new release once a day.",
                "PixMinima comprueba una vez al día si hay una nueva versión.",
            ),
            p,
        ));
        ui.add_enabled(
            state.automatic,
            egui::Checkbox::new(
                &mut state.auto_download,
                l.pick(
                    "Télécharger automatiquement les mises à jour",
                    "Automatically download updates",
                    "Descargar actualizaciones automáticamente",
                ),
            ),
        );
        ui.label(secondary(
            l.pick(
                "Vous choisissez quand installer et relancer l’application.",
                "You choose when to install and restart the app.",
                "Tú decides cuándo instalar y reiniciar la aplicación.",
            ),
            p,
        ));
        ui.separator();
        if ui
            .add_enabled(
                state.events.is_none(),
                style::button(l.pick("Vérifier maintenant", "Check now", "Buscar ahora")),
            )
            .clicked()
        {
            state.check(now(), ui.ctx());
        }
        match &state.status {
            Status::Idle => {}
            Status::Working(op) => {
                let text = match op {
                    Operation::Check => l.pick(
                        "Recherche d’une mise à jour…",
                        "Checking for updates…",
                        "Buscando actualizaciones…",
                    ),
                    Operation::Download => l.pick(
                        "Téléchargement et vérification…",
                        "Downloading and verifying…",
                        "Descargando y verificando…",
                    ),
                    Operation::Install => l.pick(
                        "Installation et relance…",
                        "Installing and restarting…",
                        "Instalando y reiniciando…",
                    ),
                };
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(text);
                });
            }
            Status::Current => {
                ui.label(l.pick(
                    "PixMinima est à jour.",
                    "PixMinima is up to date.",
                    "PixMinima está actualizado.",
                ));
            }
            Status::Available | Status::Ready => {}
            Status::Error(op, err) => {
                let label = match op {
                    Operation::Check => l.pick(
                        "La vérification a échoué.",
                        "Could not check for updates.",
                        "No se pudieron buscar actualizaciones.",
                    ),
                    Operation::Download => l.pick(
                        "Le téléchargement ou la vérification a échoué.",
                        "The download or verification failed.",
                        "La descarga o la verificación ha fallado.",
                    ),
                    Operation::Install => l.pick(
                        "L’installation a échoué. La version actuelle est conservée.",
                        "Installation failed. Your current version is preserved.",
                        "La instalación ha fallado. Se conserva la versión actual.",
                    ),
                };
                ui.label(RichText::new(label).color(p.error));
                ui.collapsing(
                    l.pick("Détail technique", "Technical detail", "Detalle técnico"),
                    |ui| {
                        ui.label(err);
                    },
                );
                if ui
                    .link(l.pick(
                        "Ouvrir le site du projet",
                        "Open project website",
                        "Abrir el sitio del proyecto",
                    ))
                    .clicked()
                    && let Err(err) = about::open_project_site()
                {
                    state.status = Status::Error(*op, err.to_string());
                }
            }
        }
        if let Some(release) = state.release.as_ref() {
            ui.label(strong(
                format!(
                    "{} {}",
                    l.pick("Nouvelle version", "New version", "Nueva versión"),
                    C::version(release)
                ),
                15.,
            ));
            if state.download.is_some() {
                if ui
                    .add_enabled(
                        idle && state.events.is_none(),
                        style::primary(
                            l.pick(
                                "Mettre à jour et relancer",
                                "Update and restart",
                                "Actualizar y reiniciar",
                            ),
                            p,
                        ),
                    )
                    .clicked()
                {
                    state.install_requested = true;
                    ui.ctx().request_repaint();
                }
                if !idle {
                    ui.label(secondary(
                        l.pick(
                            "L’installation sera disponible à la fin du traitement.",
                            "Installation will be available when processing finishes.",
                            "Podrás instalarla cuando termine el procesamiento.",
                        ),
                        p,
                    ));
                }
            } else if !matches!(state.status, Status::Working(Operation::Install))
                && ui
                    .add_enabled(
                        state.events.is_none(),
                        style::button(l.pick(
                            "Télécharger la mise à jour",
                            "Download update",
                            "Descargar actualización",
                        )),
                    )
                    .clicked()
            {
                state.fetch(ui.ctx());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    #[derive(Default)]
    struct Fake {
        checks: AtomicUsize,
        downloads: AtomicUsize,
        installations: AtomicUsize,
        fail_install: AtomicBool,
        current: AtomicBool,
        revision: AtomicUsize,
    }
    #[derive(Clone, PartialEq)]
    struct Asset {
        version: String,
        revision: usize,
    }
    impl Client for Fake {
        type Release = Asset;
        type Download = Asset;
        fn matches(download: &Asset, release: &Asset) -> bool {
            download == release
        }
        fn check(&self) -> Result<Option<Asset>, String> {
            self.checks.fetch_add(1, Ordering::Relaxed);
            Ok((!self.current.load(Ordering::Relaxed)).then(|| Asset {
                version: "1.2.0".into(),
                revision: self.revision.load(Ordering::Relaxed),
            }))
        }
        fn version(release: &Asset) -> &str {
            &release.version
        }
        fn download(&self, release: &Asset) -> Result<Asset, String> {
            self.downloads.fetch_add(1, Ordering::Relaxed);
            Ok(release.clone())
        }
        fn install(&self, _: &Asset) -> Result<(), String> {
            self.installations.fetch_add(1, Ordering::Relaxed);
            if self.fail_install.load(Ordering::Relaxed) {
                Err("Read-only test location".into())
            } else {
                Ok(())
            }
        }
    }
    fn finish(state: &mut Controller<Fake>, time: u64, ctx: &egui::Context) -> bool {
        let until = std::time::Instant::now() + Duration::from_secs(2);
        while state.events.is_some() {
            assert!(
                std::time::Instant::now() < until,
                "fake worker did not finish"
            );
            if state.tick(time, ctx) {
                return true;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        false
    }
    #[test]
    fn auto_checks_daily_without_downloading_and_manual_check_works_when_disabled() {
        let ctx = egui::Context::default();
        let mut state = Controller::new(Fake::default(), true, None);
        let today = CHECK_INTERVAL * 10;
        assert!(state.automatic);
        assert!(!state.auto_download);
        state.tick(today, &ctx);
        finish(&mut state, today, &ctx);
        assert!(matches!(state.status, Status::Available));
        state.tick(today + CHECK_INTERVAL - 1, &ctx);
        assert_eq!(state.client.checks.load(Ordering::Relaxed), 1);
        state.tick(today + CHECK_INTERVAL, &ctx);
        finish(&mut state, today + CHECK_INTERVAL, &ctx);
        assert_eq!(state.client.checks.load(Ordering::Relaxed), 2);
        assert_eq!(state.client.downloads.load(Ordering::Relaxed), 0);
        state.automatic = false;
        state.tick(today + CHECK_INTERVAL * 2, &ctx);
        assert!(state.events.is_none());
        state.client.current.store(true, Ordering::Relaxed);
        state.check(today + CHECK_INTERVAL * 2, &ctx);
        finish(&mut state, today + CHECK_INTERVAL * 2, &ctx);
        assert!(matches!(state.status, Status::Current));
    }

    #[test]
    fn recheck_discards_a_replaced_release_or_a_previously_failed_installer() {
        let ctx = egui::Context::default();
        let mut state = Controller::new(Fake::default(), true, None);
        state.automatic = false;
        let time = CHECK_INTERVAL * 4;
        state.check(time, &ctx);
        finish(&mut state, time, &ctx);
        state.fetch(&ctx);
        finish(&mut state, time, &ctx);
        state.client.revision.store(1, Ordering::Relaxed);
        state.check(time, &ctx);
        finish(&mut state, time, &ctx);
        assert!(state.download.is_none());
        assert_eq!(Fake::version(state.release.as_ref().unwrap()), "1.2.0");
        state.fetch(&ctx);
        finish(&mut state, time, &ctx);
        state.client.fail_install.store(true, Ordering::Relaxed);
        state.install(true, &ctx);
        finish(&mut state, time, &ctx);
        assert!(state.download.is_some());
        state.check(time, &ctx);
        finish(&mut state, time, &ctx);
        assert!(state.download.is_none());
        state.fetch(&ctx);
        finish(&mut state, time, &ctx);
        assert_eq!(state.client.downloads.load(Ordering::Relaxed), 3);
    }
    #[test]
    fn settings_guide_a_manual_update_and_disable_install_while_processing_in_each_language() {
        use egui_kittest::{
            Harness,
            kittest::{NodeT, Queryable},
        };
        for l in [Lang::Fr, Lang::En, Lang::Es] {
            let initial = Controller::new(Fake::default(), true, None);
            let mut view = Harness::builder()
                .with_size([570., 650.])
                .with_context_setup(style::configure)
                .build_ui_state(
                    |ui, state: &mut (Controller<Fake>, bool)| {
                        state.0.draw_settings(ui, l, state.1);
                    },
                    (initial, false),
                );
            view.get_by_label(l.pick("Vérifier maintenant", "Check now", "Buscar ahora"))
                .click();
            view.run_steps(3);
            let ctx = view.ctx.clone();
            let time = view.state().0.last_check;
            finish(&mut view.state_mut().0, time, &ctx);
            view.run_steps(2);
            view.get_by_label(l.pick(
                "Télécharger la mise à jour",
                "Download update",
                "Descargar actualización",
            ))
            .click();
            view.run_steps(3);
            finish(&mut view.state_mut().0, time, &ctx);
            view.run_steps(3);
            let label = l.pick(
                "Mettre à jour et relancer",
                "Update and restart",
                "Actualizar y reiniciar",
            );
            assert!(view.get_by_label(label).accesskit_node().is_disabled());
            view.state_mut().1 = true;
            view.run_steps(3);
            assert!(!view.get_by_label(label).accesskit_node().is_disabled());
            view.get_by_label(label).click();
            view.run_steps(3);
            assert!(std::mem::take(&mut view.state_mut().0.install_requested));
            view.state_mut().0.install(true, &ctx);
            assert!(finish(&mut view.state_mut().0, time, &ctx));
            assert_eq!(
                view.state().0.client.installations.load(Ordering::Relaxed),
                1
            );
        }
    }

    #[test]
    fn auto_download_needs_opt_in_and_install_requires_idle_and_explicit_action() {
        let ctx = egui::Context::default();
        let mut state = Controller::new(Fake::default(), true, None);
        let now = CHECK_INTERVAL * 3;
        state.auto_download = true;
        state.tick(now, &ctx);
        finish(&mut state, now, &ctx);
        assert!(matches!(state.status, Status::Ready));
        assert_eq!(state.client.installations.load(Ordering::Relaxed), 0);
        state.install(false, &ctx);
        assert!(state.events.is_none());
        state.client.fail_install.store(true, Ordering::Relaxed);
        state.install(true, &ctx);
        assert!(matches!(state.status, Status::Working(Operation::Install)));
        assert!(!finish(&mut state, now, &ctx));
        assert!(matches!(state.status, Status::Error(Operation::Install, _)));
        assert!(
            state.download.is_some(),
            "the verified download can be retried"
        );
        state.client.fail_install.store(false, Ordering::Relaxed);
        state.install(true, &ctx);
        assert!(finish(&mut state, now, &ctx));
        assert_eq!(state.client.downloads.load(Ordering::Relaxed), 1);
        assert_eq!(state.client.installations.load(Ordering::Relaxed), 2);
    }
}
