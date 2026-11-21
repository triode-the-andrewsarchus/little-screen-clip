//! Fuzzy navigation across the Earth map, sky, local features and Solar System.
mod sight;

pub(super) use sight::Sight;

use super::{Document, sky, solar};
use crate::icons::Icon;
use clip_search::name_match::navigation_rank;
use little_clip_core::{BackgroundJobs, JobError, jobs::task::Task};
use serde::{Deserialize, de::DeserializeOwned};
use std::{
    sync::{
        Arc, LazyLock,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(super) enum PlaceKind {
    City,
    Country,
    Region,
}

impl PlaceKind {
    pub fn zoom(self) -> f64 {
        match self {
            Self::City => 104.0,
            Self::Country => 5.5,
            Self::Region => 18.0,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::City => "City",
            Self::Country => "Country",
            Self::Region => "Region",
        }
    }

    fn icon(self) -> Icon {
        match self {
            Self::City => Icon::Pin,
            Self::Country => Icon::Map,
            Self::Region => Icon::Layers,
        }
    }
}

type Direction = [f32; 3];
type Place = (PlaceKind, String, String, String, [f64; 2], f64);
type NamedStar = (String, u32, f32, Direction);
type Constellation = (String, String, Direction, Vec<[Direction; 2]>);

fn read_catalog<T: DeserializeOwned>(compressed: &[u8]) -> Vec<T> {
    let decoder = zstd::stream::read::Decoder::new(compressed).expect("bundled search data");
    serde_json::from_reader(decoder).expect("valid bundled search catalog")
}

static PLACES: LazyLock<Vec<Place>> =
    LazyLock::new(|| read_catalog(include_bytes!("finder/places.json.zst")));
static NAMED_STARS: LazyLock<Vec<NamedStar>> =
    LazyLock::new(|| read_catalog(include_bytes!("finder/stars.json.zst")));
static CONSTELLATIONS: LazyLock<Vec<Constellation>> =
    LazyLock::new(|| read_catalog(include_bytes!("finder/constellations.json.zst")));

#[derive(Clone, Copy)]
pub(super) enum Target {
    Body(solar::Id),
    Place(PlaceKind, [f64; 2]),
    Star(Direction),
    Constellation(usize),
    Feature(usize),
}

#[derive(Clone)]
pub(super) struct Hit {
    pub label: Arc<str>,
    context: Arc<str>,
    icon: Icon,
    pub target: Target,
}

type Rank = (u8, usize);
type Ranked = (Rank, u32, Hit);

fn scored(
    query: &str,
    label: &str,
    aliases: impl IntoIterator<Item = impl AsRef<str>>,
) -> Option<Rank> {
    let primary = navigation_rank(label, query);
    let additional = aliases
        .into_iter()
        .filter_map(|alias| {
            let alias = alias.as_ref();
            (!alias.is_empty())
                .then(|| navigation_rank(alias, query))
                .flatten()
        })
        .min();
    primary.into_iter().chain(additional).min()
}

fn body_kind(kind: solar::Kind) -> (&'static str, Icon) {
    match kind {
        solar::Kind::Star => ("Star", Icon::Star),
        solar::Kind::Planet => ("Planet", Icon::ShapeCircle),
        solar::Kind::DwarfPlanet => ("Dwarf planet", Icon::ShapeDiamond),
        solar::Kind::Moon => ("Moon", Icon::Contrast),
    }
}

fn search(
    query: &str,
    catalog: Option<&sky::catalog::Catalog>,
    features: &[Arc<str>],
    cancel: &AtomicBool,
) -> Vec<Hit> {
    let mut ranked: Vec<Ranked> = Vec::new();
    let mut add = |rank: Rank, priority: u32, name: &str, context: &str, icon, target| {
        ranked.push((
            rank,
            priority,
            Hit {
                label: Arc::from(name),
                context: Arc::from(context),
                icon,
                target,
            },
        ));
    };
    for body in solar::bodies() {
        let aliases = match body.id {
            solar::SUN => "Sol",
            solar::MOON => "Luna",
            -1 => "Hiiaka",
            -4 => "MK 2",
            _ => "",
        };
        if let Some(rank) = scored(query, body.name, [aliases]) {
            let (kind, icon) = body_kind(body.kind());
            let parent = body
                .parent
                .filter(|&id| id != solar::SUN)
                .and_then(solar::body);
            let context = parent.map_or_else(
                || kind.to_owned(),
                |parent| format!("{kind} of {}", parent.name),
            );
            add(rank, 0, body.name, &context, icon, Target::Body(body.id));
        }
    }
    for (kind, name, locality, alias, coordinates, population) in PLACES.iter() {
        if cancel.load(Ordering::Relaxed) {
            return Vec::new();
        }
        let rank = scored(query, name, [alias.as_str()]).or_else(|| {
            // Also support a typed qualifier, for example “Lincoln Nebraska”.
            let (name_part, locality_part) = query.split_once(' ')?;
            let name_rank = navigation_rank(name, name_part)?;
            let locality_rank = navigation_rank(locality, locality_part.trim())?;
            Some((
                name_rank.0.max(locality_rank.0) + 1,
                name_rank.1 + locality_rank.1,
            ))
        });
        if let Some(rank) = rank {
            let context = if locality.is_empty() {
                kind.label().to_owned()
            } else {
                format!("{} · {locality}", kind.label())
            };
            add(
                rank,
                u32::MAX - *population as u32,
                name,
                &context,
                kind.icon(),
                Target::Place(*kind, *coordinates),
            );
        }
    }
    for (index, (name, aliases, _, _)) in CONSTELLATIONS.iter().enumerate() {
        if let Some(rank) = scored(query, name, aliases.split(" / ")) {
            add(
                rank,
                0,
                name,
                "Constellation",
                Icon::NodeTree,
                Target::Constellation(index),
            );
        }
    }
    let mut named_hips = std::collections::HashSet::new();
    for (name, hip, magnitude, direction) in NAMED_STARS.iter() {
        if *hip > 0 {
            named_hips.insert(*hip);
        }
        let alias = format!("HIP {hip}");
        if let Some(rank) = scored(query, name, [alias]) {
            add(
                rank,
                star_priority(*magnitude),
                name,
                "Star",
                Icon::Star,
                Target::Star(*direction),
            );
        }
    }
    if let Some(catalog) = catalog {
        for star in catalog.stars() {
            if cancel.load(Ordering::Relaxed) {
                return Vec::new();
            }
            if star.hip.is_some_and(|hip| named_hips.contains(&hip)) {
                continue;
            }
            if let Some(rank) = navigation_rank(&star.name, query) {
                add(
                    rank,
                    star_priority(star.magnitude),
                    &star.name,
                    "Star",
                    Icon::Star,
                    Target::Star(star.direction),
                );
            }
        }
    }
    for (index, name) in features.iter().enumerate() {
        if let Some(rank) = navigation_rank(name, query) {
            add(
                rank,
                0,
                name,
                "Open file feature",
                Icon::Map,
                Target::Feature(index),
            );
        }
    }
    ranked.sort_by(|a, b| {
        a.0.cmp(&b.0)
            .then_with(|| a.1.cmp(&b.1))
            .then_with(|| a.2.label.cmp(&b.2.label))
            .then_with(|| a.2.context.cmp(&b.2.context))
    });
    ranked.into_iter().map(|(_, _, hit)| hit).collect()
}

fn star_priority(magnitude: f32) -> u32 {
    ((magnitude + 2.0) * 100.0) as u32
}

#[derive(Default)]
pub(super) struct Finder {
    pub open: bool,
    request_focus: bool,
    query: String,
    requested: Option<(String, usize)>,
    features: Option<Arc<[Arc<str>]>>,
    job: Option<Task<Vec<Hit>>>,
    hits: Vec<Hit>,
    cursor: usize,
    select_when_ready: bool,
}

impl Finder {
    pub fn open(&mut self) {
        self.open = true;
        self.request_focus = true;
    }

    pub fn shortcut(&mut self, ctx: &egui::Context) {
        if ctx.input_mut(|input| input.consume_key(egui::Modifiers::CTRL, egui::Key::F)) {
            self.open();
        }
    }

    pub fn ui(
        &mut self,
        ui: &mut egui::Ui,
        document: &Document,
        sky: Option<&Arc<sky::Assets>>,
    ) -> Option<Hit> {
        let field_id = ui.make_persistent_id("geography-find-query");
        self.poll(ui.ctx());
        let focused = ui.memory(|memory| memory.has_focus(field_id));
        if focused && ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape)) {
            self.close();
            return None;
        }
        // The text widget consumes Enter and arrows itself. Handle result
        // navigation while the field still owns focus.
        let navigated = focused.then(|| self.move_cursor(ui));
        ui.horizontal(|ui| {
            ui.strong("Find");
            ui.menu_button("Sources", |ui| {
                ui.hyperlink_to(
                    "Natural Earth",
                    "https://www.naturalearthdata.com/downloads/10m-cultural-vectors/",
                );
                ui.hyperlink_to("HYG v4.1", sky::catalog::HYG_CREDIT);
                ui.hyperlink_to("Stellarium Modern", sky::catalog::STELLARIUM_CREDIT);
                ui.hyperlink_to(
                    "JPL Solar System Dynamics",
                    "https://ssd.jpl.nasa.gov/orbits.html",
                );
            });
            if ui.small_button("Close").on_hover_text("Escape").clicked() {
                self.close();
            }
        });
        let field = ui.add(
            egui::TextEdit::singleline(&mut self.query)
                .id(field_id)
                .hint_text("Search places and sky")
                .desired_width(ui.available_width())
                .char_limit(128),
        );
        if std::mem::take(&mut self.request_focus) {
            field.request_focus();
        }
        if !self.open {
            return None;
        }
        let started = self.update_query(document, sky);
        if matches!(navigated, Some(CursorAction::Select)) {
            self.select_when_ready = self.job.is_some();
        }
        if (matches!(navigated, Some(CursorAction::Select)) || self.select_when_ready)
            && self.job.is_none()
        {
            self.select_when_ready = false;
            return self.hits.get(self.cursor).cloned();
        }
        if self.hits.is_empty() {
            if self.job.is_some() {
                ui.add(egui::Spinner::new());
            } else if !self.query.trim().is_empty() {
                ui.label("No matches");
            }
            return None;
        }
        ui.label(format!("{} results", self.hits.len()));
        let moved = matches!(navigated, Some(CursorAction::Changed));
        let selected = self.results(ui, started || moved);
        if selected.is_some() {
            field.request_focus();
        }
        selected
    }

    fn close(&mut self) {
        self.open = false;
        self.select_when_ready = false;
        if self.job.take().is_some() {
            self.requested = None;
        }
    }

    fn poll(&mut self, ctx: &egui::Context) {
        let Some(job) = &self.job else { return };
        ctx.request_repaint_after(Duration::from_millis(24));
        if let Some(result) = job.poll() {
            self.job = None;
            match result {
                Ok(hits) => self.hits = hits,
                Err(JobError::QueueFull) => self.requested = None,
                Err(_) => self.hits.clear(),
            }
        }
    }

    fn update_query(&mut self, doc: &Document, sky: Option<&Arc<sky::Assets>>) -> bool {
        static WORKER: LazyLock<BackgroundJobs> = LazyLock::new(|| BackgroundJobs::new(1, 2));
        let query = self.query.trim().to_lowercase();
        let pointer = sky.map_or(0, |assets| Arc::as_ptr(assets) as usize);
        if self
            .requested
            .as_ref()
            .is_some_and(|key| key.0 == query && key.1 == pointer)
        {
            return false;
        }
        self.requested = Some((query.clone(), pointer));
        self.job = None;
        self.hits.clear();
        self.cursor = 0;
        self.select_when_ready = false;
        if !query.is_empty() {
            let features = self
                .features
                .get_or_insert_with(|| {
                    doc.features
                        .iter()
                        .map(|f| Arc::<str>::from(f.name.as_str()))
                        .collect()
                })
                .clone();
            let sky = sky.cloned();
            self.job = Some(Task::start(
                &WORKER,
                Duration::from_secs(10),
                move |cancel| {
                    search(
                        &query,
                        sky.as_deref().and_then(sky::Assets::star_catalog),
                        &features,
                        cancel,
                    )
                },
            ));
        }
        true
    }

    fn move_cursor(&mut self, ui: &egui::Ui) -> CursorAction {
        use egui::Key;
        let key = |key| ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, key));
        let last = self.hits.len().saturating_sub(1);
        if key(Key::ArrowDown) {
            self.cursor = self.cursor.saturating_add(1).min(last);
        } else if key(Key::ArrowUp) {
            self.cursor = self.cursor.saturating_sub(1);
        } else if key(Key::Enter) {
            return CursorAction::Select;
        } else {
            return CursorAction::None;
        }
        CursorAction::Changed
    }

    fn results(&mut self, ui: &mut egui::Ui, scroll_to_cursor: bool) -> Option<Hit> {
        let height = 28.0;
        let mut scroll = egui::ScrollArea::vertical().id_salt("geography-find-results");
        if scroll_to_cursor {
            scroll = scroll.vertical_scroll_offset(
                self.cursor.saturating_sub(2) as f32 * (height + ui.spacing().item_spacing.y),
            );
        }
        scroll
            .show_rows(ui, height, self.hits.len(), |ui, indexes| {
                let mut selected = None;
                for index in indexes {
                    let row = &self.hits[index];
                    ui.horizontal(|ui| {
                        row.icon.ui(ui, 15.0, ui.visuals().text_color());
                        let label = format!("{} · {}", row.label, row.context);
                        let item = egui::Button::selectable(index == self.cursor, &label)
                            .wrap_mode(egui::TextWrapMode::Truncate);
                        if ui
                            .add_sized([ui.available_width(), height], item)
                            .on_hover_text(label)
                            .clicked()
                        {
                            self.cursor = index;
                            selected = Some(row.clone());
                        }
                    });
                }
                selected
            })
            .inner
    }
}

enum CursorAction {
    None,
    Select,
    Changed,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fuzzy_find_keeps_exact_matches_across_result_types() {
        let cancel = AtomicBool::new(false);
        let features = [Arc::<str>::from("Mercury")];
        let results = search("mercury", None, &features, &cancel);
        assert!(
            results
                .iter()
                .any(|hit| matches!(hit.target, Target::Body(199)))
        );
        assert!(
            results
                .iter()
                .any(|hit| matches!(hit.target, Target::Feature(0)))
        );
        let typo = search("mercuyr", None, &[], &cancel);
        assert!(matches!(typo[0].target, Target::Body(199)));
        let ambiguous = search("cambridge", None, &[], &cancel);
        assert!(
            ambiguous
                .iter()
                .filter(|hit| hit.label.as_ref() == "Cambridge")
                .count()
                >= 2
        );
    }

    #[test]
    fn bundled_search_covers_cities_regions_stars_and_every_constellation() {
        let cancel = AtomicBool::new(false);
        for (query, icon) in [
            ("lincoln nebraska", Icon::Pin),
            ("nebraska", Icon::Layers),
            ("polaris", Icon::Star),
            ("ursa major", Icon::NodeTree),
        ] {
            assert!(
                search(query, None, &[], &cancel)
                    .iter()
                    .any(|hit| hit.icon == icon),
                "{query}"
            );
        }
        assert_eq!(CONSTELLATIONS.len(), 88);
    }

    #[test]
    fn ctrl_f_keeps_conflicting_results_and_selects_a_result() {
        use egui_kittest::{Harness, kittest::Queryable};
        let doc = Document::geojson(
            br#"{
            "type": "FeatureCollection",
            "features": [{"type":"Feature","properties":{"name":"Mercury"},
                "geometry":{"type":"Point","coordinates":[0,0]}}]
        }"#,
        )
        .unwrap();
        let mut harness = Harness::builder()
            .with_size(egui::vec2(360.0, 480.0))
            .with_context_setup(egui_extras::install_image_loaders)
            .build_ui_state(
                |ui, (finder, doc, selected)| {
                    finder.shortcut(ui.ctx());
                    if finder.open
                        && let Some(hit) = finder.ui(ui, doc, None)
                    {
                        *selected = Some(hit.target);
                    }
                },
                (Finder::default(), doc, None),
            );

        harness.key_press_modifiers(egui::Modifiers::CTRL, egui::Key::F);
        harness.step();
        assert!(harness.state().0.open);
        harness
            .input_mut()
            .events
            .push(egui::Event::Text("Mercury".into()));
        harness.step();
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        while harness.state().0.job.is_some() {
            assert!(
                std::time::Instant::now() < deadline,
                "Map search did not finish"
            );
            std::thread::sleep(Duration::from_millis(5));
            harness.step();
        }
        harness.get_by_label("Mercury · Open file feature");
        harness.get_by_label("Mercury · Planet");
        harness.key_press(egui::Key::ArrowDown);
        harness.step();
        harness.key_press(egui::Key::Enter);
        harness.step();
        assert!(matches!(harness.state().2, Some(Target::Body(199))));
    }
}
