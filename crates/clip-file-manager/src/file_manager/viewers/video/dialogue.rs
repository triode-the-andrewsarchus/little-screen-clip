//! Search the selected embedded transcript without blocking playback or the UI.
mod reader;

use super::{Session, clock};
use crate::file_source::{FileSource, ReadCheck};
use clip_search::name_match::{name_rank, navigation_rank};
use egui::{Context, Id, Key, Modifiers, Ui};
use little_clip_core::{
    BackgroundJobs, html_preview,
    jobs::task::Task,
    media_backend::playback::{SubtitleCue, Track},
    palette::AppPalette,
};
use std::{
    sync::{
        Arc, LazyLock,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

static JOBS: LazyLock<BackgroundJobs> = LazyLock::new(|| BackgroundJobs::new(2, 2));
const RESULT_LIMIT: usize = 100;

#[derive(Default)]
pub(super) struct Find {
    open: bool,
    focus: bool,
    query: String,
    track: Option<u32>,
    loading: Option<reader::Loading>,
    transcript: Option<Result<Arc<[Line]>, String>>,
    searching: Option<Task<Matches>>,
    matches: Option<Matches>,
}

#[derive(Clone)]
struct Line {
    start_ms: u64,
    text: String,
    normalized: String,
}

struct Matches {
    rows: Vec<usize>,
    truncated: bool,
}

impl Find {
    pub fn is_open(&self) -> bool {
        self.open
    }

    pub fn open(&mut self, ctx: &Context) {
        self.open = true;
        self.focus = true;
        ctx.request_repaint();
    }

    pub fn close(&mut self, ctx: &Context) {
        self.open = false;
        self.focus = false;
        self.searching = None;
        ctx.memory_mut(|memory| memory.surrender_focus(Id::unique("video_dialogue_query")));
        ctx.request_repaint();
    }

    pub fn show(&mut self, ui: &mut Ui, player: &mut Session, palette: &AppPalette) {
        if !self.open {
            return;
        }
        let available = ui.available_width();
        let width = (available * 0.34).clamp(220.0, 360.0).min(available * 0.5);
        let panel = egui::Panel::right("video_dialogue_sidebar")
            .resizable(false)
            .exact_size(width)
            .show(ui, |ui| {
                let close = panel_header(ui, palette);
                let enter = self.edit_query(ui);
                if let Some(track) = selected_track(player) {
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(format!(
                                "Subtitle track {} · {}",
                                track.index + 1,
                                track.label
                            ))
                            .weak(),
                        )
                        .truncate(),
                    );
                }
                ui.separator();
                (close, self.search_ui(ui, enter))
            });
        let (close, jump) = panel.inner;
        if let Some(time) = jump {
            player.seek(time);
        }
        if close {
            self.close(ui.ctx());
        }
    }

    pub fn tick(
        &mut self,
        ctx: &Context,
        source: &FileSource,
        check: &ReadCheck,
        track: Option<u32>,
    ) {
        if self.track != track {
            self.track = track;
            self.loading = None;
            self.transcript = None;
            self.invalidate_matches();
        }
        if self.open
            && let Some(track) = self.track
            && self.transcript.is_none()
            && self.loading.is_none()
        {
            self.loading = Some(reader::Loading::start(source.clone(), check.clone(), track));
        }
        self.poll_loading();
        if let Some(result) = self.searching.as_ref().and_then(Task::poll) {
            self.searching = None;
            self.matches = result.ok();
        }
        if self.loading.is_some() || self.open && self.searching.is_some() {
            ctx.request_repaint_after(Duration::from_millis(25));
        }
    }

    fn poll_loading(&mut self) {
        let Some(loading) = &self.loading else {
            return;
        };
        if let Some(result) = loading.task.poll() {
            self.loading = None;
            self.transcript = Some(
                result
                    .map_err(|error| error.to_string())
                    .and_then(|result| result),
            );
        } else if let Ok(lines) = loading.progress.try_recv() {
            self.transcript = Some(Ok(lines));
        } else {
            return;
        }
        self.invalidate_matches();
    }

    fn invalidate_matches(&mut self) {
        self.matches = None;
        self.searching = None;
    }

    fn edit_query(&mut self, ui: &mut Ui) -> bool {
        let editor_id = Id::unique("video_dialogue_query");
        if self.focus && !ui.is_sizing_pass() && ui.is_enabled() {
            ui.memory_mut(|memory| memory.request_focus(editor_id));
        }
        let editor = ui.add(
            egui::TextEdit::singleline(&mut self.query)
                .id(editor_id)
                .hint_text("Search dialogue…")
                .char_limit(128)
                .desired_width(f32::INFINITY),
        );
        if self.focus && editor.has_focus() {
            self.focus = false;
        }
        if editor.changed() {
            self.invalidate_matches();
        }
        (editor.has_focus() || editor.lost_focus())
            && ui.input_mut(|input| input.consume_key(Modifiers::NONE, Key::Enter))
    }

    fn search_ui(&mut self, ui: &mut Ui, enter: bool) -> Option<u64> {
        let lines = match &self.transcript {
            _ if self.track.is_none() => {
                ui.label("No embedded subtitle track available.");
                return None;
            }
            None => {
                ui.label("Reading subtitle track…");
                return None;
            }
            Some(Err(error)) => {
                ui.label(error);
                return None;
            }
            Some(Ok(lines)) if lines.is_empty() => {
                ui.label("This track has no searchable dialogue.");
                return None;
            }
            Some(Ok(lines)) => lines,
        };
        if self.loading.is_some() {
            ui.weak("Reading subtitle track…");
        }
        let query = normalize(&self.query);
        if query.is_empty() {
            return None;
        }
        if self.matches.is_none() && self.searching.is_none() {
            let lines = lines.clone();
            self.searching = Some(Task::start(&JOBS, Duration::from_secs(5), move |cancel| {
                search(&lines, &query, cancel)
            }));
            ui.ctx().request_repaint();
        }
        let Some(matches) = &self.matches else {
            ui.label("Searching…");
            return None;
        };
        show_results(ui, lines, matches, self.loading.is_some()).or_else(|| {
            enter
                .then(|| matches.rows.first().map(|&row| lines[row].start_ms))
                .flatten()
        })
    }
}

fn panel_header(ui: &mut Ui, palette: &AppPalette) -> bool {
    ui.horizontal(|ui| {
        ui.heading("Find dialogue");
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let response = ui
                .add(crate::ui_ext::icon_button(
                    crate::icons::Icon::Close,
                    14.0,
                    ui.visuals().widgets.inactive.bg_fill,
                    None,
                    palette,
                ))
                .on_hover_text("Close dialogue search");
            response.widget_info(|| {
                egui::WidgetInfo::labeled(
                    egui::Role::Button,
                    ui.is_enabled(),
                    "Close dialogue search",
                )
            });
            response.clicked()
        })
        .inner
    })
    .inner
}

pub(super) fn selected_track(player: &Session) -> Option<&Track> {
    let tracks = &player.tracks;
    let index = player.options().subtitle_track.or(tracks.current_subtitle);
    index.map_or_else(
        || tracks.subtitles.first(),
        |index| tracks.subtitles.iter().find(|track| track.index == index),
    )
}

fn show_results(ui: &mut Ui, lines: &[Line], matches: &Matches, loading: bool) -> Option<u64> {
    if matches.rows.is_empty() {
        if !loading {
            ui.label("No matching dialogue.");
        }
        return None;
    }
    if matches.truncated {
        ui.weak(format!("Showing the first {RESULT_LIMIT} matches"));
    }
    egui::ScrollArea::vertical()
        .id_salt("video_dialogue_results")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            let mut jump = None;
            for (position, &row) in matches.rows.iter().enumerate() {
                let line = &lines[row];
                let label = format!("{}   {}", clock(line.start_ms), line.text);
                if ui
                    .add(egui::Button::selectable(position == 0, label).wrap())
                    .clicked()
                {
                    jump = Some(line.start_ms);
                }
            }
            jump
        })
        .inner
}

fn line_from_cue(cue: SubtitleCue) -> Option<Line> {
    let text = if cue.markup && cue.text.contains(['<', '&']) {
        html_preview::parse(&format!("<p>{}</p>", cue.text), |_| None)
            .map(|blocks| {
                blocks
                    .into_iter()
                    .filter_map(|block| match block {
                        html_preview::Block::Text(text) | html_preview::Block::Heading(_, text) => {
                            Some(text)
                        }
                        html_preview::Block::Image(_, _) => None,
                    })
                    .collect::<Vec<_>>()
                    .join(" ")
            })
            .unwrap_or(cue.text)
    } else {
        cue.text
    };
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let normalized = normalize(&text);
    (!normalized.is_empty()).then_some(Line {
        start_ms: cue.start_ms,
        text,
        normalized,
    })
}

fn normalize(text: &str) -> String {
    text.split(|character: char| !character.is_alphanumeric())
        .filter(|word| !word.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

fn search(lines: &[Line], query: &str, cancel: &AtomicBool) -> Matches {
    let words = query.split_whitespace().count();
    let mut ranked = lines
        .iter()
        .enumerate()
        .take_while(|_| !cancel.load(Ordering::Relaxed))
        .filter_map(|(index, line)| {
            let rank = score(&line.normalized, query, words)?;
            Some((rank, line.start_ms, index))
        })
        .collect::<Vec<_>>();
    ranked.sort_unstable();
    let truncated = ranked.len() > RESULT_LIMIT;
    Matches {
        rows: ranked
            .into_iter()
            .take(RESULT_LIMIT)
            .map(|(_, _, index)| index)
            .collect(),
        truncated,
    }
}

fn score(line: &str, query: &str, words: usize) -> Option<(u8, usize)> {
    if let Some(rank) = name_rank(line, query) {
        return Some((rank, 0));
    }
    // Compare word-sized windows so a typo in the middle of a longer cue
    // has the same tolerance as a typo in the whole cue.
    if query.chars().count() >= 4 {
        let bounds = std::iter::once(0)
            .chain(line.match_indices(' ').map(|(at, _)| at + 1))
            .chain(std::iter::once(line.len() + 1))
            .collect::<Vec<_>>();
        if let Some(window) = bounds.windows(words + 1).find(|window| {
            navigation_rank(&line[window[0]..window[words] - 1], query)
                .is_some_and(|(rank, _)| rank == 5)
        }) {
            return Some((3, window[0]));
        }
    }
    navigation_rank(line, query)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dialogue_find_matches_a_typo_inside_a_longer_cue() {
        let query = normalize("Gtaes close!");
        assert!(score("run before the gates close tonight", &query, 2).is_some());
        assert!(score("meet beside the river tonight", &query, 2).is_none());
    }

    #[test]
    fn dialogue_find_strips_caption_markup_and_ranks_literal_hits_first() {
        let lines = [
            SubtitleCue {
                start_ms: 900,
                text: "The <i>gtaes</i> close at dusk".into(),
                markup: true,
            },
            SubtitleCue {
                start_ms: 1800,
                text: "<span foreground='white'>Gates close</span> &amp; lock.".into(),
                markup: true,
            },
        ]
        .into_iter()
        .filter_map(line_from_cue)
        .collect::<Vec<_>>();
        assert_eq!(lines[1].text, "Gates close & lock.");
        let matches = search(&lines, "gates close", &AtomicBool::new(false));
        assert_eq!(matches.rows, [1, 0]);
        assert_eq!(lines[matches.rows[0]].start_ms, 1800);
    }
}
