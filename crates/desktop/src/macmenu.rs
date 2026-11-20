use crate::locale::Lang;
use eframe::egui;
use muda::{
    Menu, MenuEvent, MenuItem, PredefinedMenuItem as Native, Submenu,
    accelerator::{Accelerator, Code, Modifiers},
};
use std::sync::mpsc::{self, Receiver};

pub struct Commands {
    pub events: Receiver<String>,
    items: Items,
}

struct Items {
    _menu: Menu,
    start: MenuItem,
    cancel: MenuItem,
    result: MenuItem,
    restore: MenuItem,
}

impl Commands {
    pub fn new(ctx: &egui::Context, language: Lang) -> Result<Self, muda::Error> {
        let (tx, rx) = mpsc::channel();
        let items = make(language)?;
        let wake = ctx.clone();
        MenuEvent::set_event_handler(Some(move |e: MenuEvent| {
            let _ = tx.send(e.id.0);
            wake.request_repaint();
        }));
        Ok(Self { events: rx, items })
    }
    pub fn translate(&mut self, l: Lang) -> Result<(), muda::Error> {
        self.items = make(l)?;
        Ok(())
    }
    pub fn enable(&self, start: bool, cancel: bool, result: bool, restore: bool, title: &str) {
        if self.items.start.text() != title {
            self.items.start.set_text(title);
        }
        if self.items.start.is_enabled() != start {
            self.items.start.set_enabled(start);
        }
        if self.items.cancel.is_enabled() != cancel {
            self.items.cancel.set_enabled(cancel);
        }
        if self.items.restore.is_enabled() != restore {
            self.items.restore.set_enabled(restore);
        }
        if self.items.result.is_enabled() != result {
            self.items.result.set_enabled(result);
        }
    }
}

fn make(l: Lang) -> Result<Items, muda::Error> {
    let item = |id: &str, text: &str, mods: Modifiers, key: Code| {
        MenuItem::with_id(id, text, true, Some(Accelerator::new(mods, key)))
    };
    let cmd = Modifiers::META;
    let app = Submenu::with_items(
        "PixMinima",
        true,
        &[
            &MenuItem::with_id(
                "about",
                l.pick(
                    "À propos de PixMinima",
                    "About PixMinima",
                    "Acerca de PixMinima",
                ),
                true,
                None,
            ),
            &MenuItem::with_id(
                "updates",
                l.pick(
                    "Rechercher des mises à jour…",
                    "Check for Updates…",
                    "Buscar actualizaciones…",
                ),
                true,
                None,
            ),
            &Native::separator(),
            &item(
                "settings",
                l.pick("Réglages…", "Settings…", "Ajustes…"),
                cmd,
                Code::Comma,
            ),
            &Native::separator(),
            &Native::services(Some(l.pick("Services", "Services", "Servicios"))),
            &Native::separator(),
            &Native::hide(Some(l.pick(
                "Masquer PixMinima",
                "Hide PixMinima",
                "Ocultar PixMinima",
            ))),
            &Native::hide_others(Some(l.pick(
                "Masquer les autres",
                "Hide Others",
                "Ocultar el resto",
            ))),
            &Native::show_all(Some(l.pick("Tout afficher", "Show All", "Mostrar todo"))),
            &Native::separator(),
            &item(
                "quit",
                l.pick("Quitter PixMinima", "Quit PixMinima", "Salir de PixMinima"),
                cmd,
                Code::KeyQ,
            ),
        ],
    )?;
    let start = item(
        "start",
        l.pick(
            "Optimiser la sélection",
            "Optimize selection",
            "Optimizar selección",
        ),
        cmd,
        Code::Enter,
    );
    let cancel = MenuItem::with_id(
        "cancel",
        l.pick("Annuler le lot", "Cancel batch", "Cancelar lote"),
        false,
        None,
    );
    let result = MenuItem::with_id(
        "output",
        l.pick(
            "Ouvrir le dossier de sortie",
            "Open output folder",
            "Abrir carpeta de salida",
        ),
        false,
        None,
    );
    let restore = MenuItem::with_id(
        "restore",
        l.pick(
            "Restaurer un original…",
            "Restore an original…",
            "Restaurar un original…",
        ),
        true,
        None,
    );
    let file = Submenu::with_items(
        l.pick("Fichier", "File", "Archivo"),
        true,
        &[
            &item(
                "files",
                l.pick("Ajouter des fichiers…", "Add files…", "Añadir archivos…"),
                cmd,
                Code::KeyO,
            ),
            &item(
                "folder",
                l.pick("Ajouter un dossier…", "Add folder…", "Añadir carpeta…"),
                cmd | Modifiers::SHIFT,
                Code::KeyO,
            ),
            &Native::separator(),
            &start,
            &cancel,
            &result,
            &restore,
            &Native::separator(),
            &Native::close_window(Some(l.pick(
                "Fermer la fenêtre",
                "Close Window",
                "Cerrar ventana",
            ))),
        ],
    )?;
    let edit = Submenu::with_items(
        l.pick("Édition", "Edit", "Edición"),
        true,
        &[
            &Native::cut(Some(l.pick("Couper", "Cut", "Cortar"))),
            &Native::copy(Some(l.pick("Copier", "Copy", "Copiar"))),
            &Native::paste(Some(l.pick("Coller", "Paste", "Pegar"))),
            &Native::select_all(Some(l.pick(
                "Tout sélectionner",
                "Select All",
                "Seleccionar todo",
            ))),
        ],
    )?;
    let window = Submenu::with_items(
        l.pick("Fenêtre", "Window", "Ventana"),
        true,
        &[
            &Native::minimize(Some(l.pick("Réduire", "Minimize", "Minimizar"))),
            &Native::zoom(Some("Zoom")),
            &Native::fullscreen(Some(l.pick(
                "Plein écran",
                "Full Screen",
                "Pantalla completa",
            ))),
            &Native::separator(),
            &Native::bring_all_to_front(Some(l.pick(
                "Tout ramener au premier plan",
                "Bring All to Front",
                "Traer todo al frente",
            ))),
        ],
    )?;
    let menu = Menu::with_items(&[&app, &file, &edit, &window])?;
    menu.init_for_nsapp();
    window.set_as_windows_menu_for_nsapp();
    Ok(Items {
        _menu: menu,
        start,
        cancel,
        result,
        restore,
    })
}
