use super::*;
use pixminima_core::{has_backup, restore_original};

impl Desktop {
    pub fn pick_restore(&mut self) {
        if self.has_pending_work() || self.restore_request.is_some() {
            return;
        }
        let l = self.lang;
        if let Some(path) = rfd::FileDialog::new()
            .set_title(l.pick(
                "Choisir un fichier optimisé à restaurer",
                "Choose an optimized file to restore",
                "Elegir un archivo optimizado para restaurar",
            ))
            .pick_file()
        {
            if has_backup(&path) {
                self.restore_request = Some(path);
            } else {
                self.notice = Some(
                    l.pick(
                        "Aucune sauvegarde PixMinima trouvée pour ce fichier.",
                        "No PixMinima backup was found for this file.",
                        "No se encontró una copia de seguridad de PixMinima para este archivo.",
                    )
                    .into(),
                );
            }
        }
    }
    pub fn restore_dialog(&mut self, ctx: &egui::Context) {
        if self.has_pending_work() {
            self.restore_request = None;
            return;
        }
        let Some(path) = self.restore_request.clone() else {
            return;
        };
        let l = self.lang;
        let response = egui::Modal::new(egui::Id::unique("restore-confirmation")).show(ctx, |ui| {
            ui.set_max_width((ctx.content_rect().width() - 48.).clamp(220., 410.));
            ui.heading(l.pick("Restaurer l’original", "Restore original", "Restaurar el original"));
            ui.label(strong(simple_name(&path), 17.));
            ui.label(secondary(full(&path), Palette::for_ui(ui)));
            ui.label(l.pick(
                "Remettre l’original sauvegardé à cet emplacement ? Un fichier modifié depuis l’optimisation sera conservé.",
                "Restore the saved original here? A file edited since optimization will be kept.",
                "¿Restaurar el original guardado aquí? Se conservará cualquier archivo modificado tras la optimización.",
            ));
            ui.add_space(5.);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let p = Palette::for_ui(ui);
                if ui.add(style::primary(l.pick("Restaurer", "Restore", "Restaurar"),p)).clicked() {
                    self.restore_request = None;
                    self.launch_restore(path.clone());
                }
                if ui.add(style::button(l.pick("Annuler", "Cancel", "Cancelar"))).clicked() {
                    self.restore_request = None;
                }
            });
        });
        if response.should_close() {
            self.restore_request = None;
        }
    }
    fn launch_restore(&mut self, path: PathBuf) {
        if self.has_pending_work() {
            return;
        }
        let (tx, rx) = mpsc::channel();
        let target = path.clone();
        match std::thread::Builder::new()
            .name("pixminima-restore".into())
            .spawn(move || {
                let _ = tx.send(restore_original(&target));
            }) {
            Ok(_) => {
                self.restoring = Some((path, rx));
            }
            Err(e) => {
                self.notice = Some(self.lang.error(&e.to_string()).into());
            }
        }
    }
    pub fn collect_restore(&mut self, ctx: &egui::Context) {
        let Some((path, rx)) = &self.restoring else {
            return;
        };
        let path = path.clone();
        let l = self.lang;
        let outcome = match rx.try_recv() {
            Ok(value) => value,
            Err(mpsc::TryRecvError::Empty) => {
                ctx.request_repaint_after(Duration::from_millis(80));
                return;
            }
            Err(mpsc::TryRecvError::Disconnected) => Err("Worker stopped".into()),
        };
        self.restoring = None;
        match outcome {
            Ok(()) => {
                for row in &mut self.results.rows {
                    if row.path != path {
                        continue;
                    }
                    if let Outcome::Saved(report, out) = &row.outcome
                        && out == &path
                    {
                        self.results.output += report.before.saturating_sub(report.after);
                        row.outcome = Outcome::Restored(report.before);
                        self.results.dirty = true;
                    }
                }
                self.notice = Some(format!(
                    "{} · {}",
                    l.pick(
                        "Original restauré",
                        "Original restored",
                        "Original restaurado"
                    ),
                    simple_name(&path)
                ));
            }
            Err(detail) => self.notice = Some(format!("{}\n{}", l.error(&detail), detail)),
        }
    }
}
