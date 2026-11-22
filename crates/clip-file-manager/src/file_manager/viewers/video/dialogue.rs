//! Search the selected embedded transcript without blocking playback or the UI.
use super::{Session, clock};
use crate::file_source::{FileSource, ReadCheck};
use clip_search::name_match::{name_rank, navigation_rank};
use egui::{Context, Id, Key, Modifiers, Ui};
use little_clip_core::{
    BackgroundJobs, html_preview,
    jobs::task::Task,
    media_backend,
    media_backend::playback::{SubtitleCue, Track},
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
    pub open: bool,
    focus: bool,
    query: String,
    track: Option<u32>,
    loading: Option<Task<Result<Arc<[Line]>, String>>>,
    transcript: Option<Result<Arc<[Line]>, String>>,
    searching: Option<Task<Matches>>,
    matches: Option<Matches>,
}

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
    pub fn open(&mut self) {
        self.open = true;
        self.focus = true;
    }

    pub fn show(
        &mut self,
        ctx: &Context,
        player: &mut Session,
        source: &FileSource,
        check: &ReadCheck,
    ) {
        let track = selected_track(player);
        self.update(ctx, source, check, track.map(|track| track.index));
        let dialogue = egui::Modal::new(Id::unique("video_dialogue_find")).show(ctx, |ui| {
            ui.set_width((ctx.content_rect().width() - 40.0).clamp(180.0, 550.0));
            ui.horizontal(|ui| {
                ui.heading("Find dialogue");
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("Close").clicked() {
                        ui.close();
                    }
                });
            });
            if let Some(track) = track {
                ui.weak(format!(
                    "Subtitle track {} · {}",
                    track.index + 1,
                    track.label
                ));
            }
            self.search_ui(ui)
        });
        if let Some(time) = dialogue.inner {
            player.seek(time);
        }
        if dialogue.inner.is_some() || dialogue.should_close() {
            self.open = false;
            self.loading = None;
            self.searching = None;
        }
    }

    fn update(
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
        if let Some(track) = self.track
            && self.transcript.is_none()
            && self.loading.is_none()
        {
            let source = source.clone();
            let check = check.clone();
            self.loading = Some(Task::start(&JOBS, Duration::from_secs(36), move |cancel| {
                read_transcript(&source, &check, track, cancel)
            }));
        }
        if let Some(result) = self.loading.as_ref().and_then(Task::poll) {
            self.loading = None;
            self.transcript = Some(
                result
                    .map_err(|error| error.to_string())
                    .and_then(|result| result),
            );
        }
        if let Some(result) = self.searching.as_ref().and_then(Task::poll) {
            self.searching = None;
            self.matches = result.ok();
        }
        if self.loading.is_some() || self.searching.is_some() {
            ctx.request_repaint_after(Duration::from_millis(25));
        }
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

    fn search_ui(&mut self, ui: &mut Ui) -> Option<u64> {
        let enter = self.edit_query(ui);
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
        show_results(ui, lines, matches).or_else(|| {
            enter
                .then(|| matches.rows.first().map(|&row| lines[row].start_ms))
                .flatten()
        })
    }
}

pub(super) fn selected_track(player: &Session) -> Option<&Track> {
    let tracks = &player.tracks;
    let index = player.options().subtitle_track.or(tracks.current_subtitle);
    index.map_or_else(
        || tracks.subtitles.first(),
        |index| tracks.subtitles.iter().find(|track| track.index == index),
    )
}

fn show_results(ui: &mut Ui, lines: &[Line], matches: &Matches) -> Option<u64> {
    if matches.rows.is_empty() {
        ui.label("No matching dialogue.");
        return None;
    }
    if matches.truncated {
        ui.weak(format!("Showing the first {RESULT_LIMIT} matches"));
    }
    egui::ScrollArea::vertical()
        .max_height((ui.ctx().content_rect().height() * 0.48).min(330.0))
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

fn read_transcript(
    source: &FileSource,
    check: &ReadCheck,
    track: u32,
    cancel: &AtomicBool,
) -> Result<Arc<[Line]>, String> {
    let (input, _) = check.open(source.path(), Some(source), cancel)?;
    let output = media_backend::subtitle_track_request(track)?
        .arg_owned_content_file(input.try_clone().map_err(|error| error.to_string())?)
        .map_err(|error| error.to_string())?
        .timeout(Duration::from_secs(35))
        .run_cancellable(cancel)
        .map_err(|error| error.to_string())?;
    source.check_file(&input)?;
    check.validate(cancel)?;
    if output.stdout_truncated {
        return Err("The subtitle track exceeds the search limit.".into());
    }
    if !output.status.success() {
        return Err(format!(
            "Cannot read subtitle track. {}",
            output.stderr_lossy().trim()
        ));
    }
    let cues: Vec<SubtitleCue> = serde_json::from_slice(&output.stdout)
        .map_err(|error| format!("Cannot read subtitle track. {error}"))?;
    Ok(cues
        .into_iter()
        .take_while(|_| !cancel.load(Ordering::Relaxed))
        .filter_map(line_from_cue)
        .collect())
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
