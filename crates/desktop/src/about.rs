use super::*;
use std::process::Command;

const SITE: &str = "https://alejandrosml.github.io/PixMinima/";
const SOURCE: &str = env!("CARGO_PKG_REPOSITORY");
const LICENSE: &str = concat!(env!("CARGO_PKG_REPOSITORY"), "/blob/main/LICENSE");
const NOTICES: &str = concat!(
    env!("CARGO_PKG_REPOSITORY"),
    "/blob/main/docs/THIRD-PARTY.txt"
);

pub fn about_name(l: Lang) -> &'static str {
    l.pick(
        "À propos de PixMinima",
        "About PixMinima",
        "Acerca de PixMinima",
    )
}

pub fn product_type(l: Lang) -> &'static str {
    l.pick(
        "Compression d’images",
        "Image compression",
        "Compresión de imágenes",
    )
}

// Official project links only. Pass each target as one argument, never through a shell.
#[cfg(target_os = "macos")]
pub fn open_project_site() -> std::io::Result<()> {
    open_web(SITE)
}

fn open_web(url: &'static str) -> std::io::Result<()> {
    #[cfg(target_os = "macos")]
    let mut command = Command::new("open");
    #[cfg(target_os = "windows")]
    let mut command = {
        let mut command = Command::new("rundll32.exe");
        command.arg("url.dll,FileProtocolHandler");
        command
    };
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let mut command = Command::new("xdg-open");
    command.arg(url).spawn().map(|_| ())
}

fn open_document(name: &'static str, fallback: &'static str) -> std::io::Result<()> {
    let binary = std::env::current_exe()?;
    #[cfg(target_os = "macos")]
    let resources = binary
        .parent()
        .and_then(Path::parent)
        .map(|p| p.join("Resources"));
    #[cfg(not(target_os = "macos"))]
    let resources = binary.parent().map(Path::to_owned);
    if let Some(file) = resources.map(|dir| dir.join(name)).filter(|p| p.is_file()) {
        #[cfg(target_os = "macos")]
        let mut command = {
            let mut command = Command::new("open");
            command.arg("-t");
            command
        };
        #[cfg(target_os = "windows")]
        let mut command = Command::new("notepad.exe");
        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        let mut command = Command::new("xdg-open");
        command.arg(file).spawn().map(|_| ())
    } else {
        open_web(fallback)
    }
}

impl Desktop {
    pub fn about_window(&mut self, ctx: &egui::Context) {
        if !self.about {
            return;
        }
        let l = self.lang;
        let mut open = true;
        egui::Window::new(about_name(l))
            .id(egui::Id::unique("about-pixminima"))
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, Vec2::ZERO)
            .default_width((ctx.content_rect().width() - 54.).clamp(250., 350.))
            .show(ctx, |ui| {
                let p = Palette::for_ui(ui);
                let mut action = Ok(());
                ui.vertical_centered(|ui| {
                    ui.add_space(11.);
                    style::brand(ui, 64.);
                    ui.add_space(4.);
                    ui.label(strong("PixMinima", 26.));
                    ui.label(product_type(l));
                    ui.label(secondary(
                        format!(
                            "{} {}",
                            l.pick("Version", "Version", "Versión"),
                            env!("CARGO_PKG_VERSION")
                        ),
                        p,
                    ));
                    ui.add_space(4.);
                    ui.label("Alejandro San Martin Lamas");
                    if ui
                        .link(l.pick("Site du projet", "Project website", "Sitio del proyecto"))
                        .on_hover_text(SITE)
                        .clicked()
                    {
                        action = open_web(SITE);
                    }
                    if ui
                        .link(l.pick("Code source", "Source code", "Código fuente"))
                        .on_hover_text(SOURCE)
                        .clicked()
                    {
                        action = open_web(SOURCE);
                    }
                });
                ui.add_space(6.);
                ui.separator();
                ui.add_space(2.);
                ui.vertical_centered(|ui| {
                    if ui
                        .link(l.pick(
                            "Licence GNU GPL v3 ou ultérieure",
                            "GNU GPL v3 or later",
                            "Licencia GNU GPL v3 o posterior",
                        ))
                        .clicked()
                    {
                        action = open_document("LICENSE", LICENSE);
                    }
                    if ui
                        .link(l.pick(
                            "Licences des bibliothèques",
                            "Third-party licenses",
                            "Licencias de las bibliotecas",
                        ))
                        .clicked()
                    {
                        action = open_document("THIRD-PARTY.txt", NOTICES);
                    }
                    ui.add_space(6.);
                });
                if let Err(err) = action {
                    self.notice = Some(format!(
                        "{} · {err}",
                        l.pick(
                            "Impossible d’ouvrir le lien ou le document",
                            "Could not open the link or document",
                            "No se pudo abrir el enlace o el documento"
                        )
                    ));
                }
            });
        self.about = open;
    }
}
