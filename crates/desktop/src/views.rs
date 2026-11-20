use super::*;
use crate::style::{Palette, brand, button, photographs, primary, secondary, strong};

impl Desktop {
    pub fn content(&mut self, ui: &mut egui::Ui) {
        let p = Palette::for_ui(ui);
        ui.painter().rect_filled(ui.max_rect(), 0., p.paper);
        self.input(ui.ctx());
        self.detail_window(ui.ctx());
        self.settings_window(ui.ctx());
        self.about_window(ui.ctx());
        self.restore_dialog(ui.ctx());
        let hovering = ui.ctx().input(|i| !i.raw.hovered_files.is_empty());
        let selecting = self.active.is_none()
            && (!self.paths.is_empty() || self.phase == Phase::Idle || hovering);
        // Reserve space for the action on compact windows while the selection scrolls.
        let compact = ui.available_width() < 560. || ui.available_height() < 500.;
        if selecting && compact {
            egui::Panel::bottom("compact-action")
                .show_separator_line(false)
                .frame(
                    egui::Frame::new()
                        .fill(p.paper)
                        .inner_margin(Margin::symmetric(14, 8)),
                )
                .show(ui, |ui| self.output_bar(ui));
        }
        egui::ScrollArea::vertical()
            .id_salt("main")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                let margin = if ui.available_width() < 500. { 16 } else { 28 };
                egui::Frame::new()
                    .inner_margin(Margin::symmetric(margin, 18))
                    .show(ui, |ui| {
                        ui.set_min_width(ui.available_width());
                        self.toolbar(ui);
                        #[cfg(target_os = "macos")]
                        if let Some(version) = self.updater.version() {
                            ui.horizontal_wrapped(|ui| {
                                ui.label(strong(
                                    format!(
                                        "{} {}",
                                        self.lang.pick(
                                            "Mise à jour disponible",
                                            "Update available",
                                            "Actualización disponible"
                                        ),
                                        version
                                    ),
                                    14.,
                                ));
                                if ui.link(self.lang.pick("Afficher", "View", "Ver")).clicked() {
                                    self.settings = true;
                                    self.update_settings = true;
                                }
                            });
                        }
                        ui.add_space(3.);
                        if self.active.is_none() {
                            if selecting {
                                self.drop_area(ui, hovering);
                                if !compact {
                                    ui.add_space(6.);
                                    self.output_bar(ui);
                                }
                            } else {
                                self.add_buttons(ui);
                            }
                        }
                        if self.phase != Phase::Idle {
                            ui.add_space(10.);
                            self.results(ui);
                        }
                        if let Some(notice) = &self.notice {
                            let mut dismiss = false;
                            p.card(12).show(ui, |ui| {
                                ui.label(notice);
                                dismiss = ui
                                    .link(self.lang.pick("Fermer", "Dismiss", "Cerrar"))
                                    .clicked();
                            });
                            if dismiss {
                                self.notice = None;
                            }
                        }
                    });
            });
    }

    fn toolbar(&mut self, ui: &mut egui::Ui) {
        let l = self.lang;
        ui.horizontal(|ui| {
            // macOS draws its native window controls inside this toolbar.
            if self.native_toolbar {
                ui.add_space(51.);
            }
            brand(ui, 32.);
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 0.;
                ui.label(strong("PixMinima", 21.));
                ui.label(secondary(about::product_type(l), Palette::for_ui(ui)).size(12.));
            });
            let space = (ui.available_width() - 123.).max(0.);
            let (_, drag) =
                ui.allocate_exact_size(Vec2::new(space, 30.), egui::Sense::click_and_drag());
            if self.native_toolbar && drag.drag_started() {
                ui.ctx().send_viewport_cmd(egui::ViewportCommand::StartDrag);
            }
            if self.native_toolbar && drag.double_clicked() {
                let maximized = ui.ctx().input(|i| i.viewport().maximized.unwrap_or(false));
                ui.ctx()
                    .send_viewport_cmd(egui::ViewportCommand::Maximized(!maximized));
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui
                    .add(button(l.pick("Réglages…", "Settings…", "Ajustes…")).frame(false))
                    .on_hover_text(l.pick(
                        "Réglages de PixMinima",
                        "PixMinima settings",
                        "Ajustes de PixMinima",
                    ))
                    .clicked()
                {
                    self.settings = true;
                }
            });
        });
    }

    fn add_buttons(&mut self, ui: &mut egui::Ui) {
        let l = self.lang;
        ui.horizontal_wrapped(|ui| {
            if ui
                .add(button(l.pick(
                    "Ajouter des fichiers…",
                    "Add files…",
                    "Añadir archivos…",
                )))
                .clicked()
            {
                self.files();
            }
            if ui
                .add(button(l.pick(
                    "Ajouter un dossier…",
                    "Add folder…",
                    "Añadir carpeta…",
                )))
                .clicked()
            {
                self.folder();
            }
        });
    }

    fn drop_area(&mut self, ui: &mut egui::Ui, hovering: bool) {
        let l = self.lang;
        let p = Palette::for_ui(ui);
        egui::Frame::new()
            .fill(p.well)
            .stroke(Stroke::new(
                if hovering { 2. } else { 1. },
                if hovering { p.accent } else { p.border },
            ))
            .corner_radius(15)
            .inner_margin(Margin::same(18))
            .show(ui, |ui| {
                ui.set_min_width(ui.available_width());
                if self.paths.is_empty() {
                    ui.vertical_centered(|ui| {
                        let footer_height = if self.lossy && self.task == Task::Optimize {
                            if ui.available_width() < 540. { 103. } else { 77. }
                        } else { 0. };
                        let vertical_pad = ((ui.ctx().content_rect().height() - 360. - footer_height) / 2.).max(8.);
                        ui.add_space(vertical_pad);
                        photographs(ui, p);
                        ui.add_space(5.);
                        ui.label(strong(
                            l.pick(
                                "Glissez des images ou des dossiers ici",
                                "Drop images or folders here",
                                "Suelta imágenes o carpetas aquí",
                            ),
                            22.,
                        ));
                        ui.label(secondary(
                            match self.effective_write_mode() {
                                WriteMode::Copies => l.pick(
                                    "Les originaux sont conservés à leur emplacement.",
                                    "Your original files stay where they are.",
                                    "Los originales se conservan en su ubicación.",
                                ),
                                WriteMode::Backup => l.pick(
                                    "Une sauvegarde protège chaque original remplacé.",
                                    "A backup protects each original that gets replaced.",
                                    "Se guarda una copia de cada original reemplazado.",
                                ),
                                WriteMode::Overwrite => l.pick(
                                    "Un original est remplacé uniquement si l’image optimisée est plus petite.",
                                    "An original is replaced only when the optimized image is smaller.",
                                    "Un original solo se reemplaza si la imagen optimizada ocupa menos.",
                                ),
                            },
                            p,
                        ));
                        ui.add_space(5.);
                        ui.horizontal(|ui| {
                            let width = ui.available_width();
                            let names = [
                                l.pick("Ajouter des fichiers…", "Add files…", "Añadir archivos…"),
                                l.pick("Ajouter un dossier…", "Add folder…", "Añadir carpeta…"),
                            ];
                            let font = FontId::new(14., FontFamily::Name("UI Semibold".into()));
                            let total = names
                                .iter()
                                .map(|s| {
                                    ui.painter()
                                        .layout_no_wrap(s.to_string(), font.clone(), p.ink)
                                        .size()
                                        .x
                                        + 2. * ui.spacing().button_padding.x
                                        + 2.
                                })
                                .sum::<f32>()
                                + ui.spacing().item_spacing.x;
                            ui.add_space(
                                ((width - total) * 0.5 - ui.spacing().item_spacing.x).max(0.),
                            );
                            if ui.add(button(names[0])).clicked() {
                                self.files();
                            }
                            if ui.add(button(names[1])).clicked() {
                                self.folder();
                            }
                        });
                        ui.add_space(vertical_pad);
                    });
                } else {
                    ui.horizontal(|ui| {
                        ui.label(strong(l.pick("Sélection", "Selection", "Selección"), 20.));
                        ui.label(secondary(l.items(self.paths.len()), p));
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if ui.link(l.pick("Tout retirer", "Clear", "Vaciar")).clicked() {
                                self.paths.clear();
                            }
                        });
                    });
                    let mut remove = None;
                    egui::ScrollArea::vertical()
                        .id_salt("selection")
                        .max_height(124.)
                        .show(ui, |ui| {
                            for (i, path) in self.paths.iter().enumerate() {
                                ui.horizontal(|ui| {
                                    if ui
                                        .small_button("×")
                                        .on_hover_text(format!(
                                            "{} {}",
                                            l.pick("Retirer", "Remove", "Quitar"),
                                            simple_name(path)
                                        ))
                                        .clicked()
                                    {
                                        remove = Some(i);
                                    }
                                    ui.add(egui::Label::new(simple_name(path)).truncate())
                                        .on_hover_text(full(path));
                                });
                            }
                        });
                    if let Some(i) = remove {
                        self.paths.remove(i);
                    }
                    ui.add_space(4.);
                    self.add_buttons(ui);
                }
            });
    }

    fn output_bar(&mut self, ui: &mut egui::Ui) {
        let l = self.lang;
        let p = Palette::for_ui(ui);
        p.card(13).show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            if self.lossy && self.task == Task::Optimize {
                ui.label(strong(l.pick("Compression avec perte autorisée", "Lossy compression allowed", "Compresión con pérdida permitida"), 15.).color(p.error));
                let message = if self.write_mode == WriteMode::Overwrite {
                    l.pick("Les images compatibles peuvent perdre des détails. En cas de gain, l’original est remplacé sans sauvegarde.",
                           "Compatible images may lose detail. A smaller result replaces the original without a backup.",
                           "Las imágenes compatibles pueden perder detalle. Si se reduce el tamaño, se reemplaza el original sin copia de seguridad.")
                } else {
                    l.pick("Les images compatibles peuvent perdre des détails.", "Compatible images may lose detail.", "Las imágenes compatibles pueden perder detalle.")
                };
                ui.label(secondary(message, p));
                ui.separator();
            }
            let width = ui.available_width();
            let action_width = ui.painter().layout_no_wrap(
                self.action_name().to_owned(),
                FontId::new(14., FontFamily::Name("UI Semibold".into())),
                p.ink,
            ).size().x + 32.;
            if width < action_width + 160. {
                self.output_summary(ui, p);
                ui.add_space(2.);
                self.start_button(ui, p, width);
            } else {
                ui.horizontal(|ui| {
                    ui.allocate_ui_with_layout(
                        Vec2::new((width - action_width - 12.).max(100.), 34.),
                        Layout::top_down(Align::Min),
                        |ui| self.output_summary(ui, p),
                    );
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| self.start_button(ui, p, action_width));
                });
            }
        });
    }

    fn output_summary(&self, ui: &mut egui::Ui, p: Palette) {
        let l = self.lang;
        ui.label(strong(self.profile_text(), 14.));
        let location = match self.effective_write_mode() {
            WriteMode::Copies => self
                .destination
                .as_ref()
                .map(|v| simple_name(v))
                .unwrap_or_else(|| {
                    l.pick(
                        "à côté de la sélection",
                        "beside the selection",
                        "junto a la selección",
                    )
                    .into()
                }),
            WriteMode::Backup => l
                .pick(
                    "Sauvegarde à côté de l’original",
                    "Backup beside the original",
                    "Copia junto al original",
                )
                .into(),
            WriteMode::Overwrite => l
                .pick(
                    "Sur place · sans sauvegarde",
                    "In place · no backup",
                    "En el original · sin copia de seguridad",
                )
                .into(),
        };
        ui.add(egui::Label::new(secondary(location, p)).truncate())
            .on_hover_text(self.output_detail());
    }

    fn start_button(&mut self, ui: &mut egui::Ui, p: Palette, width: f32) {
        if ui
            .add_enabled(
                self.ready_to_start(),
                primary(self.action_name(), p).min_size(Vec2::new(width, 34.)),
            )
            .on_hover_text(self.action_name())
            .clicked()
        {
            self.start();
        }
    }

    pub(super) fn action_name(&self) -> &'static str {
        match self.task {
            Task::Optimize if self.lossy && self.write_mode == WriteMode::Overwrite => {
                self.lang.pick(
                    "Compresser avec perte et remplacer",
                    "Compress and replace (lossy)",
                    "Comprimir con pérdida y reemplazar",
                )
            }
            Task::Optimize => match self.write_mode {
                WriteMode::Copies => self.lang.pick(
                    "Optimiser des copies",
                    "Optimize copies",
                    "Optimizar copias",
                ),
                WriteMode::Backup => self.lang.pick(
                    "Optimiser avec sauvegarde",
                    "Optimize with backup",
                    "Optimizar con copia",
                ),
                WriteMode::Overwrite => self.lang.pick(
                    "Optimiser et remplacer",
                    "Optimize and replace",
                    "Optimizar y reemplazar",
                ),
            },
            #[cfg(feature = "archive-jxl")]
            Task::Archive => self.lang.pick("Archiver", "Archive", "Archivar"),
            #[cfg(feature = "archive-jxl")]
            Task::RestoreJpeg => self.lang.pick("Extraire", "Extract", "Extraer"),
        }
    }
    #[cfg(feature = "archive-jxl")]
    fn task_name(&self) -> &'static str {
        let l = self.lang;
        match self.task {
            Task::Optimize => l.pick(
                "Optimiser les images",
                "Optimize images",
                "Optimizar imágenes",
            ),
            Task::Archive => l.pick(
                "Archiver les JPEG en JPEG XL",
                "Archive JPEGs as JPEG XL",
                "Archivar JPEG en JPEG XL",
            ),
            Task::RestoreJpeg => l.pick(
                "Extraire les JPEG d’origine",
                "Extract original JPEGs",
                "Extraer los JPEG originales",
            ),
        }
    }
    fn profile_text(&self) -> &'static str {
        #[cfg(feature = "archive-jxl")]
        if self.task != Task::Optimize {
            return self.task_name();
        }
        if !self.lossy {
            return match self.effort {
                Effort::Standard => self.lang.pick(
                    "Sans perte · rapide",
                    "Lossless · Fast",
                    "Sin pérdida · Rápida",
                ),
                Effort::Thorough => self.lang.pick(
                    "Sans perte · approfondie",
                    "Lossless · Thorough",
                    "Sin pérdida · Exhaustiva",
                ),
                Effort::Maximum => self.lang.pick(
                    "Sans perte · maximum",
                    "Lossless · Maximum",
                    "Sin pérdida · Máxima",
                ),
            };
        }
        match self.effort {
            Effort::Standard => self.lang.pick(
                "Compression rapide",
                "Fast compression",
                "Compresión rápida",
            ),
            Effort::Thorough => self.lang.pick(
                "Compression approfondie",
                "Thorough compression",
                "Compresión exhaustiva",
            ),
            Effort::Maximum => self.lang.pick(
                "Compression maximale",
                "Maximum compression",
                "Compresión máxima",
            ),
        }
    }
    fn output_detail(&self) -> String {
        if self.effective_write_mode() == WriteMode::Overwrite {
            return self
                .lang
                .pick(
                    "L’image optimisée prend la place de l’original. Aucune sauvegarde n’est créée.",
                    "The optimized image replaces the original. No backup is created.",
                    "La imagen optimizada reemplaza a la original. No se crea una copia de seguridad.",
                )
                .into();
        }
        if self.effective_write_mode() == WriteMode::Backup {
            return self
                .lang
                .pick(
                    "L’original est sauvegardé près du fichier avant tout remplacement.",
                    "The original is backed up beside the file before replacement.",
                    "El original se guarda junto al archivo antes de reemplazarlo.",
                )
                .into();
        }

        self.destination
            .as_ref()
            .map(|p| full(p))
            .unwrap_or_else(|| {
                self.lang
                    .pick(
                        "Un nouveau dossier est créé à côté de la première sélection.",
                        "A new folder is created beside your first selection.",
                        "Se crea una carpeta nueva junto a la primera selección.",
                    )
                    .into()
            })
    }

    fn results(&mut self, ui: &mut egui::Ui) {
        let l = self.lang;
        let p = Palette::for_ui(ui);
        let title = match self.phase {
            Phase::Idle => return,
            Phase::Scanning => l.pick(
                "Recherche des images…",
                "Finding images…",
                "Buscando imágenes…",
            ),
            Phase::Running if self.results.task != Task::Optimize => l.pick(
                "Conversion en cours",
                "Converting files",
                "Conversión en curso",
            ),
            Phase::Running => l.pick(
                "Optimisation en cours",
                "Optimizing images",
                "Optimizando imágenes",
            ),
            Phase::Finished if self.results.task != Task::Optimize => l.pick(
                "Conversion terminée",
                "Conversion complete",
                "Conversión completada",
            ),
            Phase::Finished => l.pick(
                "Optimisation terminée",
                "Optimization complete",
                "Optimización completada",
            ),
            Phase::Cancelled => l.pick("Lot annulé", "Batch cancelled", "Lote cancelado"),
            Phase::Empty => l.pick(
                "Aucune image compatible",
                "No supported images",
                "Ninguna imagen compatible",
            ),
            Phase::Failed => l.pick(
                "Traitement interrompu",
                "Processing stopped",
                "Proceso interrumpido",
            ),
        };
        ui.horizontal_wrapped(|ui| {
            ui.label(strong(title, 22.));
            if self.active.is_some() {
                if ui
                    .add(button(l.pick(
                        "Annuler le lot",
                        "Cancel batch",
                        "Cancelar lote",
                    )))
                    .clicked()
                {
                    self.cancel();
                }
            } else if let Some(folder) = &self.results.folder
                && ui
                    .link(l.pick(
                        "Ouvrir le dossier des copies",
                        "Open output folder",
                        "Abrir carpeta de copias",
                    ))
                    .on_hover_text(full(folder))
                    .clicked()
                && let Err(e) = show_folder(folder)
            {
                self.notice = Some(e.to_string());
            }
        });
        if let Some(active) = &self.active {
            if self.phase == Phase::Running {
                ui.add(
                    egui::ProgressBar::new(
                        self.results.completed as f32 / self.results.total.max(1) as f32,
                    )
                    .fill(p.accent)
                    .text(format!(
                        "{} / {}",
                        self.results.completed, self.results.total
                    )),
                );
                if !active.now.as_os_str().is_empty() {
                    ui.add(egui::Label::new(secondary(simple_name(&active.now), p)).truncate())
                        .on_hover_text(full(&active.now));
                }
            }
            if active.cancel.load(Ordering::Acquire) {
                ui.label(secondary(
                    l.pick(
                        "Annulation après les fichiers en cours…",
                        "Cancelling after the current files…",
                        "Cancelando después de los archivos en curso…",
                    ),
                    p,
                ));
            }
        }
        if self.results.rows.is_empty() {
            return;
        }
        let increasing = self.results.output > self.results.input;
        let saved = self.results.input.abs_diff(self.results.output);

        let measure = if increasing {
            l.pick("Augmentation", "Increase", "Aumento")
        } else {
            if self.results.mode == WriteMode::Overwrite {
                l.pick("Espace libéré", "Space saved", "Espacio liberado")
            } else {
                l.pick("Réduction totale", "Total reduction", "Reducción total")
            }
        };

        p.card(14).show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            ui.columns(3, |cols| {
                for (idx, title, value) in [
                    (0, measure, l.bytes(saved)),
                    (
                        1,
                        l.pick("Avant", "Before", "Antes"),
                        l.bytes(self.results.input),
                    ),
                    (
                        2,
                        l.pick("Après", "After", "Después"),
                        l.bytes(self.results.output),
                    ),
                ] {
                    cols[idx].label(secondary(title, p));
                    cols[idx].label(strong(value, if idx == 0 { 22. } else { 19. }).color(
                        if idx == 0 && !increasing {
                            p.success
                        } else {
                            p.ink
                        },
                    ));
                }
            });
        });
        ui.horizontal_wrapped(|ui| {
            let count = self.results.rows.len();
            let noun = if count == 1 {
                l.pick("fichier", "file", "archivo")
            } else {
                l.pick("fichiers", "files", "archivos")
            };
            ui.label(strong(format!("{count} {noun}"), 15.));
            if self.results.errors > 0 {
                ui.label(
                    RichText::new(format!(
                        "{} {}",
                        self.results.errors,
                        l.pick("en erreur", "failed", "con error")
                    ))
                    .color(p.error),
                );
            }
            if self.results.skipped > 0 {
                ui.label(secondary(
                    format!(
                        "{} {}",
                        self.results.skipped,
                        l.pick("ignorés", "skipped", "omitidos")
                    ),
                    p,
                ));
            }
        });
        self.file_results(ui, p);
    }

    fn settings_window(&mut self, ctx: &egui::Context) {
        if !self.settings {
            return;
        }
        let mut open = true;
        let l = self.lang;
        #[cfg(target_os = "macos")]
        let update_page = self.update_settings;
        #[cfg(not(target_os = "macos"))]
        let update_page = false;
        let panel_height = if update_page {
            293.
        } else {
            (ctx.content_rect().height() - 56.).clamp(270., 600.)
        };
        egui::Window::new(l.pick("Réglages", "Settings", "Ajustes"))
            .id(egui::Id::unique(("settings", update_page)))
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, Vec2::ZERO)
            .default_width((ctx.content_rect().width() - 40.).clamp(270., 430.))
            .default_height(panel_height)
            .max_height((ctx.content_rect().height() - 50.).max(200.))
            .vscroll(true)
            .show(ctx, |ui| {
                let p = Palette::for_ui(ui);
                #[cfg(target_os = "macos")]
                {
                    ui.horizontal(|ui| {
                        ui.selectable_value(&mut self.update_settings,false,l.pick("Général","General","General"));
                        ui.selectable_value(&mut self.update_settings,true,l.pick("Mises à jour","Updates","Actualizaciones"));
                        ui.with_layout(Layout::right_to_left(Align::Center),|ui| {
                            if ui.link(about::about_name(l)).clicked() {self.about=true;}
                        });
                    });
                    ui.separator();
                    if self.update_settings {
                        let idle=self.active.is_none() && self.restoring.is_none() && self.restore_request.is_none();
                        self.updater.settings(ui,l,idle);
                        return;
                    }
                }
                ui.horizontal(|ui| {
                    ui.label(strong(l.pick("Interface", "Interface", "Interfaz"), 17.));
                    #[cfg(not(target_os = "macos"))]
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui.link(about::about_name(l)).clicked() {
                            self.about = true;
                        }
                    });
                });
                ui.add_space(3.);
                egui::Grid::new("interface-settings")
                    .num_columns(2)
                    .spacing([16., 8.])
                    .show(ui, |ui| {
                        let label = ui.label(l.pick("Langue", "Language", "Idioma"));
                        egui::ComboBox::from_id_salt("language")
                            .width(188.)
                            .selected_text(self.language.label(l))
                            .show_ui(ui, |ui| {
                                for choice in [
                                    LanguageChoice::System,
                                    LanguageChoice::Explicit(Lang::Fr),
                                    LanguageChoice::Explicit(Lang::En),
                                    LanguageChoice::Explicit(Lang::Es),
                                ] {
                                    ui.selectable_value(
                                        &mut self.language,
                                        choice,
                                        choice.label(l),
                                    );
                                }
                            })
                            .response
                            .labelled_by(label.id);
                        ui.end_row();
                        let label = ui.label(l.pick("Apparence", "Appearance", "Apariencia"));
                        egui::ComboBox::from_id_salt("appearance")
                            .width(188.)
                            .selected_text(appearance_name(self.appearance, l))
                            .show_ui(ui, |ui| {
                                for t in [
                                    egui::ThemePreference::System,
                                    egui::ThemePreference::Light,
                                    egui::ThemePreference::Dark,
                                ] {
                                    ui.selectable_value(
                                        &mut self.appearance,
                                        t,
                                        appearance_name(t, l),
                                    );
                                }
                            })
                            .response
                            .labelled_by(label.id);
                        ui.end_row();
                    });
                ui.add_space(8.);
                ui.separator();
                ui.add_space(3.);
                #[cfg(feature = "archive-jxl")]
                {
                    let label = ui.label(l.pick("Opération", "Operation", "Operación"));
                    egui::ComboBox::from_id_salt("operation")
                        .selected_text(self.task_name())
                        .show_ui(ui, |ui| {
                            ui.selectable_value(
                                &mut self.task,
                                Task::Optimize,
                                l.pick(
                                    "Optimiser les images",
                                    "Optimize images",
                                    "Optimizar imágenes",
                                ),
                            );
                            ui.selectable_value(
                                &mut self.task,
                                Task::Archive,
                                l.pick(
                                    "Archiver les JPEG en JPEG XL",
                                    "Archive JPEGs as JPEG XL",
                                    "Archivar JPEG en JPEG XL",
                                ),
                            );
                            ui.selectable_value(
                                &mut self.task,
                                Task::RestoreJpeg,
                                l.pick(
                                    "Extraire les JPEG d’origine",
                                    "Extract original JPEGs",
                                    "Extraer los JPEG originales",
                                ),
                            );
                        })
                        .response
                        .labelled_by(label.id);

                }
                if self.task == Task::Optimize {
                    ui.label(strong(
                        l.pick("Optimisation", "Optimization", "Optimización"),
                        17.,
                    ));
                    ui.horizontal_wrapped(|ui| {
                        for (effort,label) in [
                            (Effort::Standard,l.pick("Rapide","Fast","Rápida")),
                            (Effort::Thorough,l.pick("Approfondie","Thorough","Exhaustiva")),
                            (Effort::Maximum,l.pick("Maximum","Maximum","Máxima")),
                        ] { ui.selectable_value(&mut self.effort,effort,label); }
                    });
                    let cost=match self.effort {
                        Effort::Standard=>l.pick("Essaie un nombre limité de stratégies pour réduire le temps de traitement.","Tests fewer strategies to shorten processing time.","Prueba menos estrategias para reducir el tiempo de procesamiento."),
                        Effort::Thorough=>l.pick("Essaie davantage de stratégies sans perte.","Tests more lossless compression strategies.","Prueba más estrategias de compresión sin pérdida."),
                        Effort::Maximum=>l.pick("Essaie davantage de réglages pour les PNG et les JPEG. C’est le niveau le plus lent.","Tests more settings for PNG and JPEG images. This is the slowest level.","Prueba más ajustes para imágenes PNG y JPEG. Es el nivel más lento."),
                    };
                    ui.label(secondary(cost,p));
                    ui.checkbox(&mut self.lossy, l.pick("Autoriser la compression avec perte", "Allow lossy compression", "Permitir compresión con pérdida"));
                    ui.label(secondary(l.pick(
                        "Peut modifier les détails des PNG, JPEG, GIF et WebP compatibles. Les autres images restent sans perte.",
                        "May change detail in compatible PNG, JPEG, GIF and WebP images. Other images stay lossless.",
                        "Puede alterar el detalle de los PNG, JPEG, GIF y WebP compatibles. Las demás imágenes se comprimen sin pérdida."
                    ), p));
                    ui.add_space(3.);
                    let incompatible=self.png_selection.incompatible(&self.paths,ctx);
                    ui.add_enabled_ui(!incompatible,|ui| ui.horizontal(|ui| {
                        let label = ui.label(l.pick(
                            "Filtres PNG et APNG",
                            "PNG and APNG filters",
                            "Filtros PNG y APNG",
                        ));
                        let text = |c: ComputeMode| match c {
                            ComputeMode::Auto => l.pick("Automatique", "Automatic", "Automático"),
                            ComputeMode::Cpu => "CPU",
                            ComputeMode::Metal => "Metal",
                        };
                        egui::ComboBox::from_id_salt("compute")
                            .selected_text(text(self.compute))
                            .show_ui(ui, |ui| {
                                for c in [ComputeMode::Auto, ComputeMode::Cpu] {
                                    ui.selectable_value(&mut self.compute, c, text(c));
                                }
                                #[cfg(target_os = "macos")]
                                ui.selectable_value(&mut self.compute, ComputeMode::Metal, "Metal");
                            })
                            .response
                            .labelled_by(label.id);
                    }));
                    let hint = if incompatible {
                        l.pick("Ce lot utilise le processeur. Ce réglage concerne les PNG et APNG compatibles.",
                               "This batch uses the CPU. This setting applies to compatible PNG and APNG images.",
                               "Este lote utiliza el procesador. El ajuste se aplica a imágenes PNG y APNG compatibles.")
                    } else if !cfg!(target_os="macos") || self.compute==ComputeMode::Cpu {
                        l.pick("Le processeur calcule les filtres des PNG et APNG.",
                               "The CPU processes PNG and APNG filters.",
                               "El procesador calcula los filtros de las imágenes PNG y APNG.")
                    } else if self.compute==ComputeMode::Metal {
                        l.pick("Utilise Metal pour les filtres PNG compatibles. Le processeur prend le relais si nécessaire.",
                               "Uses Metal for compatible PNG filters. The CPU takes over when needed.",
                               "Usa Metal para los filtros PNG compatibles. El procesador se encarga cuando es necesario.")
                    } else {
                        l.pick("Compare le processeur et Metal sur les PNG compatibles, puis utilise le plus rapide.",
                               "Compares the CPU and Metal on compatible PNGs, then uses the faster option.",
                               "Compara el procesador con Metal en los PNG compatibles y utiliza la opción más rápida.")
                    };
                    ui.label(secondary(hint,p));
                    ui.add_space(8.);
                    ui.separator();
                    ui.add_space(3.);
                }
                let label=ui.label(strong(l.pick("Enregistrement", "Saving files", "Guardado de archivos"), 17.));
                if self.task == Task::Optimize {
                    egui::ComboBox::from_id_salt("write-mode")
                        .width(260.)
                        .selected_text(self.write_mode.name(l))
                        .show_ui(ui, |ui| {
                            for mode in [WriteMode::Copies, WriteMode::Backup, WriteMode::Overwrite]
                            {
                                ui.selectable_value(&mut self.write_mode, mode, mode.name(l));
                            }
                        })
                        .response
                        .labelled_by(label.id);
                }
                ui.label(secondary(self.output_detail(), p));
                if self.effective_write_mode() == WriteMode::Backup {
                    if ui
                        .add_enabled(
                            !self.has_pending_work(),
                            button(l.pick(
                                "Restaurer un original…",
                                "Restore an original…",
                                "Restaurar un original…",
                            )),
                        )
                        .clicked()
                    {
                        self.pick_restore();
                    }
                } else if self.effective_write_mode() == WriteMode::Copies {
                    ui.horizontal_wrapped(|ui| {
                        if ui
                            .add(button(l.pick(
                                "Choisir un emplacement…",
                                "Choose location…",
                                "Elegir ubicación…",
                            )))
                            .clicked()
                            && let Some(folder) = rfd::FileDialog::new()
                                .set_title(l.pick(
                                    "Dossier des copies",
                                    "Output folder",
                                    "Carpeta de copias",
                                ))
                                .pick_folder()
                            {
                                self.destination = Some(folder);
                            }
                        if self.destination.is_some()
                            && ui
                                .link(l.pick("Réinitialiser", "Reset", "Restablecer"))
                                .clicked()
                        {
                            self.destination = None;
                        }
                    });
                }
            });
        self.settings = open && !self.about;
        if ctx.options(|o| o.theme_preference) != self.appearance {
            ctx.set_theme(self.appearance);
        }
        self.refresh_language();
    }
}

pub fn appearance_name(t: egui::ThemePreference, l: Lang) -> &'static str {
    match t {
        egui::ThemePreference::System => l.pick("Système", "System", "Sistema"),
        egui::ThemePreference::Light => l.pick("Claire", "Light", "Clara"),
        egui::ThemePreference::Dark => l.pick("Sombre", "Dark", "Oscura"),
    }
}

impl WriteMode {
    pub(super) fn name(self, l: Lang) -> &'static str {
        match self {
            Self::Copies => l.pick("Copies séparées", "Separate copies", "Copias separadas"),
            Self::Backup => l.pick(
                "Remplacer avec sauvegarde",
                "Replace with backup",
                "Reemplazar con copia de seguridad",
            ),
            Self::Overwrite => l.pick(
                "Remplacer les originaux",
                "Replace originals",
                "Reemplazar los originales",
            ),
        }
    }
}
