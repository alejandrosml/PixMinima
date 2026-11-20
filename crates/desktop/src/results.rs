use super::*;
use std::cmp::Ordering as Cmp;

#[derive(Clone, Copy, Default, PartialEq)]
pub(super) enum Sort {
    #[default]
    File,
    Size,
    Gain,
}

impl Outcome {
    fn rank(&self) -> u8 {
        match self {
            Self::Running => 0,
            Self::Pending => 1,
            Self::Error(_) => 2,
            Self::Cancelled | Self::Skipped => 3,
            Self::Saved(..) | Self::Restored(_) => 4,
        }
    }

    fn sizes(&self) -> Option<(u64, u64)> {
        match self {
            Self::Saved(r, _) => Some((r.before, r.after)),
            Self::Restored(n) => Some((*n, *n)),
            _ => None,
        }
    }

    pub(super) fn status(&self, l: Lang) -> &'static str {
        match self {
            Self::Pending => l.pick("En attente", "Waiting", "En espera"),
            Self::Running => l.pick("En cours", "Processing", "En curso"),
            Self::Cancelled => l.pick("Annulé", "Cancelled", "Cancelado"),
            Self::Error(_) => l.pick("Échec", "Failed", "Error"),
            Self::Skipped => l.pick("Ignoré", "Skipped", "Omitido"),
            Self::Restored(_) => l.pick(
                "Original restauré",
                "Original restored",
                "Original restaurado",
            ),
            Self::Saved(r, _) if r.before == r.after => {
                l.pick("Taille inchangée", "Size unchanged", "Mismo tamaño")
            }
            Self::Saved(r, _) if r.lossy_used => l.pick(
                "Compressé avec perte",
                "Lossy compression",
                "Compresión con pérdida",
            ),
            Self::Saved(..) => l.pick(
                "Optimisé sans perte",
                "Lossless optimization",
                "Optimización sin pérdida",
            ),
        }
    }
}

impl Results {
    pub(super) fn update(&mut self, path: PathBuf, outcome: Outcome) {
        if let Some(&index) = self.indices.get(&path) {
            self.rows[index].outcome = outcome;
        } else {
            self.indices.insert(path.clone(), self.rows.len());
            let name = simple_name(&path);
            let folded = name.to_lowercase();
            self.rows.push(Row {
                path,
                name,
                folded,
                outcome,
            });
        }
        self.dirty = true;
    }

    fn choose(&mut self, sort: Sort) {
        self.descending = if self.sort == sort {
            !self.descending
        } else {
            sort != Sort::File
        };
        self.sort = sort;
        self.dirty = true;
    }

    fn reorder(&mut self) {
        if !self.dirty {
            return;
        }
        self.order.clear();
        self.order.extend(0..self.rows.len());
        self.order.sort_by(|&left, &right| {
            let a = &self.rows[left];
            let b = &self.rows[right];
            let ready = a.outcome.rank().cmp(&b.outcome.rank());
            let by_name = || a.folded.cmp(&b.folded).then_with(|| a.path.cmp(&b.path));
            let column = match (self.sort, a.outcome.sizes(), b.outcome.sizes()) {
                (Sort::File, ..) => by_name(),
                (Sort::Size, Some((_, x)), Some((_, y))) => x.cmp(&y),
                (Sort::Gain, Some(x), Some(y)) => compare_gain(x, y),
                _ => Cmp::Equal,
            };
            ready
                .then(if self.descending {
                    column.reverse()
                } else {
                    column
                })
                .then_with(by_name)
        });
        self.dirty = false;
    }
}

fn compare_gain(a: (u64, u64), b: (u64, u64)) -> Cmp {
    let (a0, a1) = if a.0 == 0 { (1, 1) } else { a };
    let (b0, b1) = if b.0 == 0 { (1, 1) } else { b };
    (b1 as u128 * a0 as u128).cmp(&(a1 as u128 * b0 as u128))
}

fn status_icon(ui: &mut egui::Ui, outcome: &Outcome, l: Lang, p: Palette) {
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(16.), egui::Sense::hover());
    let status = outcome.status(l);
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Image, true, status));
    response.on_hover_text(status);
    let center = rect.center();
    let painter = ui.painter();
    match outcome {
        Outcome::Saved(r, _) if r.after != r.before => {
            painter.circle_filled(center, 6.7, p.success);
            let line = Stroke::new(1.4, p.ink_accent);
            painter.line_segment(
                [center + Vec2::new(-3., 0.), center + Vec2::new(-0.6, 2.4)],
                line,
            );
            painter.line_segment(
                [center + Vec2::new(-0.6, 2.4), center + Vec2::new(3.4, -2.6)],
                line,
            );
        }
        Outcome::Error(_) => {
            painter.circle_stroke(center, 6.1, Stroke::new(1.4, p.error));
            painter.line_segment(
                [center + Vec2::new(0., -3.), center + Vec2::new(0., 0.8)],
                Stroke::new(1.6, p.error),
            );
            painter.circle_filled(center + Vec2::new(0., 3.2), 0.9, p.error);
        }
        Outcome::Running => {
            egui::Spinner::new()
                .size(13.)
                .color(p.accent)
                .paint_at(ui, rect);
        }
        _ => {
            painter.circle_stroke(center, 5.8, Stroke::new(1.2, p.muted));
            if !matches!(outcome, Outcome::Pending) {
                painter.line_segment(
                    [center - Vec2::new(2.6, 0.), center + Vec2::new(2.6, 0.)],
                    Stroke::new(1.2, p.muted),
                );
            }
        }
    }
}

// Fixed cell rectangles keep decimals and column headings aligned independently
// of filename length, even when the table virtualizes thousands of entries.
fn cells(ui: &mut egui::Ui, width: f32, height: f32, seed: usize) -> [egui::Ui; 3] {
    let (band, _) = ui.allocate_exact_size(Vec2::new(width, height), egui::Sense::hover());
    let numeric = if width < 420. { 70. } else { 94. };
    let right = band.right() - 3.;
    let edge_size = right - numeric - 10.;
    let bounds = [
        (band.left(), edge_size - numeric - 12.),
        (edge_size - numeric, edge_size),
        (right - numeric, right),
    ];
    std::array::from_fn(|column| {
        let (a, b) = bounds[column];
        let rect =
            egui::Rect::from_min_max(egui::pos2(a, band.top()), egui::pos2(b, band.bottom()));
        let layout = if column == 0 {
            Layout::left_to_right(Align::Center)
        } else {
            Layout::right_to_left(Align::Center)
        };
        ui.new_child(
            egui::UiBuilder::new()
                .id_salt((seed, column))
                .max_rect(rect)
                .layout(layout),
        )
    })
}

impl Desktop {
    pub(super) fn file_results(&mut self, ui: &mut egui::Ui, p: Palette) {
        let l = self.lang;
        p.card(9).show(ui, |ui| {
            let width = ui.available_width();
            ui.set_min_width(width);
            let mut header = cells(ui, width, 25., usize::MAX);
            for (column, (sort, title)) in [
                (Sort::File, l.pick("Fichier", "File", "Archivo")),
                (Sort::Size, l.pick("Taille", "Size", "Tamaño")),
                (Sort::Gain, l.pick("Gain", "Savings", "Ahorro")),
            ]
            .into_iter()
            .enumerate()
            {
                let cell = &mut header[column];
                cell.spacing_mut().button_padding.x = 0.;
                if column == 0 {
                    cell.add_space(20.);
                }
                let marked = if self.results.sort == sort {
                    format!(
                        "{title} {}",
                        if self.results.descending {
                            "↓"
                        } else {
                            "↑"
                        }
                    )
                } else {
                    title.into()
                };
                let control = egui::Button::new(strong(marked, 13.).color(p.muted)).frame(false);
                if cell.add(control).clicked() {
                    self.results.choose(sort);
                }
            }
            ui.separator();
            self.results.reorder();
            egui::ScrollArea::vertical()
                .id_salt("batch-results")
                .max_height(290.)
                .show_rows(ui, 26., self.results.order.len(), |ui, range| {
                    for visible in range {
                        let index = self.results.order[visible];
                        let row = &self.results.rows[index];
                        let status = row.outcome.status(l);
                        let sizes = row.outcome.sizes();
                        let (size, gain, hint) = if let Some((before, after)) = sizes {
                            (
                                l.bytes(after),
                                l.saved_percent(before, after),
                                format!(
                                    "{status} · {} → {}",
                                    l.exact_bytes(before),
                                    l.exact_bytes(after)
                                ),
                            )
                        } else {
                            (
                                "—".into(),
                                "—".into(),
                                match &row.outcome {
                                    Outcome::Error(detail) => l.error(detail).into(),
                                    _ => status.into(),
                                },
                            )
                        };
                        let [mut file, mut size_cell, mut gain_cell] = cells(ui, width, 26., index);
                        status_icon(&mut file, &row.outcome, l, p);
                        if file
                            .add(egui::Button::new(&row.name).frame(false).truncate())
                            .on_hover_text(&hint)
                            .clicked()
                        {
                            self.detail = Some(index);
                        }
                        size_cell
                            .label(RichText::new(size).size(14.))
                            .on_hover_text(&hint);
                        let ink = if sizes.is_some_and(|(b, a)| a < b) {
                            p.success
                        } else {
                            p.muted
                        };
                        gain_cell
                            .label(strong(gain, 14.).color(ink))
                            .on_hover_text(hint);
                    }
                });
        });
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn saved(before: u64, after: u64) -> Outcome {
        Outcome::Saved(
            FileReport {
                before,
                after,
                format: "PNG".into(),
                lossy_used: false,
            },
            PathBuf::new(),
        )
    }
    fn names(results: &mut Results) -> Vec<String> {
        results.reorder();
        results
            .order
            .iter()
            .map(|&i| results.rows[i].name.clone())
            .collect()
    }
    #[test]
    fn sorting_uses_numeric_file_sizes_and_unrounded_gain_while_preserving_rows() {
        let mut results = Results::default();
        results.update("A.png".into(), saved(100_000, 99_000));
        results.update("B.png".into(), saved(999, 100));
        results.update("C.png".into(), saved(1_000, 100));
        results.update("Z.png".into(), Outcome::Pending);
        assert_eq!(names(&mut results), ["Z.png", "A.png", "B.png", "C.png"]);
        results.choose(Sort::Gain);
        assert_eq!(names(&mut results), ["Z.png", "C.png", "B.png", "A.png"]);
        results.choose(Sort::Gain);
        assert_eq!(names(&mut results), ["Z.png", "A.png", "B.png", "C.png"]);
        results.choose(Sort::Size);
        assert_eq!(names(&mut results), ["Z.png", "A.png", "B.png", "C.png"]);
        results.update(
            "Z.png".into(),
            Outcome::Error("file could not be read".into()),
        );
        assert_eq!(names(&mut results), ["Z.png", "A.png", "B.png", "C.png"]);
        results.update("Z.png".into(), saved(8, 8));
        assert_eq!(names(&mut results), ["A.png", "B.png", "C.png", "Z.png"]);
        assert_eq!(results.rows.len(), 4);
    }
}
