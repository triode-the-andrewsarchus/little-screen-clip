use super::*;

impl AudioPreviewState {
    pub(super) fn session_edit_filter(&self) -> Option<String> {
        let regions = self.region_edits();
        if regions.is_empty() && self.track_gain_db == 0.0 {
            return None;
        }
        let mut cuts = vec![0.0, self.duration_sec];
        for region in &regions {
            cuts.push(region.start_sec.clamp(0.0, self.duration_sec));
            cuts.push(region.end_sec.clamp(0.0, self.duration_sec));
        }
        cuts.sort_by(f32::total_cmp);
        cuts.dedup();
        let mut filters = Self::gain_filter(self.track_gain_db, false)
            .into_iter()
            .collect::<Vec<_>>();
        for pair in cuts.windows(2) {
            let (start, end) = (pair[0], pair[1]);
            if end - start < 0.00001 {
                continue;
            }
            let mid = (start + end) * 0.5;
            let region = regions
                .iter()
                .find(|r| mid >= r.start_sec && mid <= r.end_sec)
                .copied();
            if let Some(gain_filter) = region.and_then(Self::region_filter) {
                let enable = format!("enable='between(t,{start:.6},{end:.6})'");
                filters.extend(
                    gain_filter
                        .split(',')
                        .map(|part| format!("{part}:{enable}")),
                );
            }
        }
        (!filters.is_empty()).then(|| filters.join(","))
    }

    pub(super) fn crossfade_mix_expression(region: AudioRegionEdit, duration_sec: f32) -> String {
        let fade = region.crossfade_sec.max(0.0);
        if fade <= 0.0 {
            return format!("between(t,{:.6},{:.6})", region.start_sec, region.end_sec);
        }
        let left = if region.start_sec <= 0.0 {
            "1".to_string()
        } else {
            format!(
                "clip((t-{:.6})/{fade:.6},0,1)",
                (region.start_sec - fade).max(0.0)
            )
        };
        let right = if region.end_sec >= duration_sec {
            "1".to_string()
        } else {
            format!(
                "clip(({:.6}-t)/{fade:.6},0,1)",
                (region.end_sec + fade).min(duration_sec)
            )
        };
        format!("({left})*({right})")
    }

    pub(super) fn session_crossfade_filter_complex(&self) -> Option<(String, String)> {
        let regions = self.region_edits();
        if !regions.iter().any(|region| region.crossfade_sec > 0.0) {
            return None;
        }
        let mut graph = Vec::new();
        let mut input = "0:a".to_string();
        if let Some(gain) = Self::gain_filter(self.track_gain_db, false) {
            graph.push(format!("[0:a]{gain}[track]"));
            input = "track".to_string();
        }
        for (index, region) in regions.into_iter().enumerate() {
            let effect = Self::region_filter(region)?;
            let mix = Self::crossfade_mix_expression(region, self.duration_sec);
            let dry = format!("dry{index}");
            let wet = format!("wet{index}");
            let dry_mix = format!("dry_mix{index}");
            let wet_mix = format!("wet_mix{index}");
            let output = format!("edit{}", index + 1);
            graph.push(format!("[{input}]asplit=2[{dry}][{wet}]"));
            graph.push(format!(
                "[{wet}]{effect},volume='{mix}':eval=frame[{wet_mix}]"
            ));
            graph.push(format!("[{dry}]volume='1-({mix})':eval=frame[{dry_mix}]"));
            graph.push(format!(
                "[{dry_mix}][{wet_mix}]amix=inputs=2:duration=first:dropout_transition=0:normalize=0[{output}]"
            ));
            input = output;
        }
        Some((graph.join(";"), format!("[{input}]")))
    }

    pub(super) fn editor_handoff_path(&self) -> Result<PathBuf, String> {
        let Some(filter) = self.session_edit_filter() else {
            return Ok(self.path.clone());
        };
        let out = std::env::temp_dir()
            .join("little_clip_audio_sessions")
            .join(format!("audacity-handoff-{}.wav", uuid::Uuid::new_v4()));
        if let Some(parent) = out.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("Could not prepare audio handoff: {e}"))?;
        }
        let mut command = std::process::Command::new("ffmpeg");
        command.args(["-y", "-i"]).arg(&self.path);
        if let Some((graph, output)) = self.session_crossfade_filter_complex() {
            command.args(["-filter_complex", &graph, "-map", &output]);
        } else {
            command.args(["-af", &filter]);
        }
        let ok = command
            .args(["-c:a", "pcm_s16le"])
            .arg(&out)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if ok && out.is_file() {
            Ok(out)
        } else {
            Err("Could not render audio edits for editor handoff".to_string())
        }
    }

    /// Launches an external audio editor (e.g. Audacity) for the current file.
    pub fn launch_external_audio_editor(
        &mut self,
        _ctx: &egui::Context,
        _close_requested: &mut bool,
    ) {
        let editor = if self.audio_editor.is_empty() {
            "audacity"
        } else {
            &self.audio_editor
        };
        let handoff_path = match self.editor_handoff_path() {
            Ok(path) => path,
            Err(error) => {
                self.status_toast = Some((error.into(), _ctx.input(|i| i.time)));
                return;
            }
        };
        let mut cmd = std::process::Command::new(editor);
        cmd.arg(&handoff_path);
        match cmd.spawn() {
            Ok(_) => {
                let filename = self
                    .path
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_else(|| "file".to_string());
                self.status_toast = Some((
                    crate::ui_ext::StatusMessage::new(
                        crate::icons::Icon::ArrowRight,
                        format!("Opened {} in {}", filename, editor),
                    ),
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap()
                        .as_secs_f64(),
                ));
                self.stop();
            }
            Err(e) => {
                // Try xdg-open fallback
                if std::process::Command::new("xdg-open")
                    .arg(&handoff_path)
                    .spawn()
                    .is_ok()
                {
                    self.status_toast = Some((
                        crate::ui_ext::StatusMessage::new(
                            crate::icons::Icon::ArrowRight,
                            "Opened audio in default application via xdg-open".to_string(),
                        ),
                        std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .unwrap()
                            .as_secs_f64(),
                    ));
                    self.stop();
                } else {
                    self.status_toast = Some((
                        crate::ui_ext::StatusMessage::new(
                            crate::icons::Icon::Alert,
                            format!("Failed to launch '{}': {}", editor, e),
                        ),
                        std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .unwrap()
                            .as_secs_f64(),
                    ));
                }
            }
        }
    }

    pub fn copy_selected_segment(&mut self, now: f64, mode: ChannelCopyMode) {
        let (start, end) = self
            .active_selection_range()
            .unwrap_or((0.0, self.duration_sec.max(0.1)));
        let active_region = self
            .active_region_edit()
            .unwrap_or_else(|| self.region_edit(start, end, 0.0, 0.0, None, None));
        let region = self.export_region_edit(
            start,
            end,
            0.0,
            active_region.gain_db,
            active_region.low_pass_hz,
            active_region.high_pass_hz,
        );

        let plan = audio_slice_copy_plan(
            &self.path,
            start,
            end,
            mode,
            Self::channel_edit_filter(mode, region),
        );
        if self.slice_copy_job.is_some() {
            self.status_toast = Some((
                crate::ui_ext::StatusMessage::new(
                    crate::icons::Icon::Copy,
                    "Audio slice copy is already in progress".to_owned(),
                ),
                now,
            ));
            return;
        }
        let cancelled = Arc::new(AtomicBool::new(false));
        let worker_cancelled = Arc::clone(&cancelled);
        let (ticket, receiver) = BackgroundJobs::global().submit(Duration::MAX, move || {
            extract_and_handoff_audio_slice(plan, &worker_cancelled)
                .map(|delivery| audio_slice_copy_status(mode, start, end, delivery))
                .map_err(|error| {
                    format!(
                        "Failed to copy {} ({:.3}s): {error}",
                        audio_slice_copy_description(mode),
                        (end - start).max(0.001)
                    )
                })
        });
        self.slice_copy_job = Some(AudioSliceCopyJob {
            ticket,
            cancelled,
            receiver,
        });
        self.status_toast = Some((
            crate::ui_ext::StatusMessage::new(
                crate::icons::Icon::Copy,
                "Preparing audio slice for clipboard…".to_owned(),
            ),
            now,
        ));
    }

    pub(super) fn poll_slice_copy_job(&mut self, ctx: &egui::Context, now: f64) {
        let Some(job) = &self.slice_copy_job else {
            return;
        };
        ctx.request_repaint_after(Duration::from_millis(80));
        let result = match job.receiver.try_recv() {
            Ok(result) => result,
            Err(TryRecvError::Empty) => return,
            Err(TryRecvError::Disconnected) => Err(JobError::Cancelled),
        };
        self.slice_copy_job = None;
        let message = match result {
            Ok(Ok(message)) => crate::ui_ext::StatusMessage::new(crate::icons::Icon::Copy, message),
            Ok(Err(error)) => crate::ui_ext::StatusMessage::new(crate::icons::Icon::Alert, error),
            Err(JobError::Cancelled) => crate::ui_ext::StatusMessage::new(
                crate::icons::Icon::Alert,
                "Audio slice copy was cancelled",
            ),
            Err(error) => crate::ui_ext::StatusMessage::new(
                crate::icons::Icon::Alert,
                format!("Audio slice copy failed: {error}"),
            ),
        };
        self.status_toast = Some((message, now));
        ctx.request_repaint();
    }

    pub fn copy_all_multi_segments(&mut self, now: f64, mode: ChannelCopyMode) {
        if self.multi_selections.is_empty() {
            self.copy_selected_segment(now, mode);
            return;
        }
        if self.slice_copy_job.is_some() {
            self.status_toast = Some((
                crate::ui_ext::StatusMessage::new(
                    crate::icons::Icon::Copy,
                    "Audio slice copy is already in progress".to_owned(),
                ),
                now,
            ));
            return;
        }
        let plans = self
            .multi_selections
            .iter()
            .enumerate()
            .map(|(i, &(start, end))| {
                let (s, e) = if start < end {
                    (start, end)
                } else {
                    (end, start)
                };
                let region = self.export_region_edit(
                    s,
                    e,
                    self.region_crossfade_sec.get(i).copied().unwrap_or(0.0),
                    self.region_gain_db.get(i).copied().unwrap_or(0.0),
                    self.region_low_pass_hz.get(i).copied().flatten(),
                    self.region_high_pass_hz.get(i).copied().flatten(),
                );
                audio_slice_copy_plan(
                    &self.path,
                    s,
                    e,
                    mode,
                    Self::channel_edit_filter(mode, region),
                )
            })
            .collect::<Vec<_>>();
        let cancelled = Arc::new(AtomicBool::new(false));
        let worker_cancelled = Arc::clone(&cancelled);
        let (ticket, receiver) = BackgroundJobs::global().submit(Duration::MAX, move || {
            run_audio_slice_batch(
                &plans,
                mode,
                &worker_cancelled,
                extract_audio_slice_file,
                |request| match crate::send_ipc_request_with_timeout(
                    request,
                    Some(Duration::from_secs(2)),
                ) {
                    Ok(little_clip_core::ipc::IpcResponse::Success { .. }) => Ok(()),
                    Ok(little_clip_core::ipc::IpcResponse::Error { error }) => Err(error),
                    Ok(response) => Err(format!("unexpected daemon response: {response:?}")),
                    Err(error) => Err(error.to_string()),
                },
                |paths| {
                    little_clip_core::copy_file_uris_to_system_clipboard(paths)
                        .map_err(|error| error.to_string())
                },
            )
        });
        self.slice_copy_job = Some(AudioSliceCopyJob {
            ticket,
            cancelled,
            receiver,
        });
        self.status_toast = Some((
            crate::ui_ext::StatusMessage::new(
                crate::icons::Icon::Copy,
                format!(
                    "Preparing {} audio slices for clipboard…",
                    self.multi_selections.len()
                ),
            ),
            now,
        ));
    }

    pub fn copy_waveform_image(&mut self, palette: &AppPalette, now: f64) {
        let Some(ref source_analysis) = self.analysis else {
            self.status_toast = Some((
                crate::ui_ext::StatusMessage::new(
                    crate::icons::Icon::Alert,
                    "Waveform analysis not yet available".to_string(),
                ),
                now,
            ));
            return;
        };
        let analysis = self.edited_waveform.as_ref().unwrap_or(source_analysis);
        let region_edits = self.region_edits();
        match Self::copy_waveform_image_on(
            analysis,
            source_analysis,
            &region_edits,
            self.view_start_sec,
            self.view_duration_sec,
            self.duration_sec,
            palette,
            |request| {
                let _ = crate::send_ipc_request(request);
            },
            |bytes, mime| {
                let bytes_for_cmd = bytes.to_vec();
                let mime_for_cmd = mime.to_string();
                std::thread::spawn(move || {
                    if let Err(error) = little_clip_core::copy_bytes_to_system_clipboard(
                        &bytes_for_cmd,
                        &mime_for_cmd,
                    ) {
                        tracing::debug!(%error, "failed to copy waveform image to system clipboard");
                    }
                });
            },
        ) {
            Ok(()) => {
                self.status_toast = Some((
                    crate::ui_ext::StatusMessage::new(
                        crate::icons::Icon::Copy,
                        "Waveform image copied to clipboard".to_string(),
                    ),
                    now,
                ));
            }
            Err(err) => {
                tracing::warn!("Failed to render waveform image: {err}");
                self.status_toast = Some((
                    crate::ui_ext::StatusMessage::new(
                        crate::icons::Icon::Alert,
                        format!("Failed to render waveform: {err}"),
                    ),
                    now,
                ));
            }
        }
    }

    pub(super) fn copy_waveform_image_on(
        analysis: &FullAudioAnalysis,
        source_analysis: &FullAudioAnalysis,
        region_edits: &[AudioRegionEdit],
        view_start_sec: f32,
        view_duration_sec: f32,
        duration_sec: f32,
        palette: &AppPalette,
        mut send: impl FnMut(&IpcRequest),
        mut copy: impl FnMut(&[u8], &str),
    ) -> Result<(), String> {
        let (font_reg, _, _, font_mono) = little_clip_render::ScreenRenderer::load_fonts();
        let png_bytes = tools::render_waveform_image(
            analysis,
            source_analysis,
            region_edits,
            view_start_sec,
            view_duration_sec,
            duration_sec,
            palette,
            font_mono.as_ref(),
            font_reg.as_ref(),
        )?;
        send(&IpcRequest::PushImage {
            bytes: png_bytes.clone(),
            mime: "image/png".to_string(),
            hand: little_clip_core::HandSide::Left,
        });
        copy(&png_bytes, "image/png");
        Ok(())
    }

    pub fn copy_spectrum_image(&mut self, palette: &AppPalette, now: f64) {
        let Some(ref source_analysis) = self.analysis else {
            self.status_toast = Some((
                crate::ui_ext::StatusMessage::new(
                    crate::icons::Icon::Alert,
                    "Spectrum analysis not yet available".to_string(),
                ),
                now,
            ));
            return;
        };
        let analysis = self.edited_waveform.as_ref().unwrap_or(source_analysis);
        let (font_reg, _, _, font_mono) = little_clip_render::ScreenRenderer::load_fonts();

        match tools::render_spectrum_image(
            analysis,
            self.view_start_sec,
            self.view_duration_sec,
            self.duration_sec,
            palette,
            font_mono.as_ref(),
            font_reg.as_ref(),
        ) {
            Ok(png_bytes) => {
                let req = IpcRequest::PushImage {
                    bytes: png_bytes.clone(),
                    mime: "image/png".to_string(),
                    hand: little_clip_core::HandSide::Left,
                };
                let _ = crate::send_ipc_request(&req);

                let bytes_for_cmd = png_bytes;
                std::thread::spawn(move || {
                    if let Err(error) = little_clip_core::copy_bytes_to_system_clipboard(
                        &bytes_for_cmd,
                        "image/png",
                    ) {
                        tracing::debug!(%error, "failed to copy spectrum image to system clipboard");
                    }
                });

                self.status_toast = Some((
                    crate::ui_ext::StatusMessage::new(
                        crate::icons::Icon::Copy,
                        "Spectrum image copied to clipboard".to_string(),
                    ),
                    now,
                ));
            }
            Err(err) => {
                tracing::warn!("Failed to render spectrum image: {err}");
                self.status_toast = Some((
                    crate::ui_ext::StatusMessage::new(
                        crate::icons::Icon::Alert,
                        format!("Failed to render spectrum: {err}"),
                    ),
                    now,
                ));
            }
        }
    }

    pub fn reset_save_filenames(&mut self) {
        let base_name = self
            .path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("audio");

        let ext = self
            .path
            .extension()
            .and_then(|s| s.to_str())
            .map(|s| s.to_lowercase())
            .unwrap_or_else(|| "wav".to_string());
        let export_ext = if matches!(ext.as_str(), "wav" | "flac" | "mp3" | "ogg") {
            ext.as_str()
        } else {
            "wav"
        };

        let slices: Vec<(f32, f32)> = match self.selection_mode {
            SelectionMode::Single => {
                if let Some((s, e)) = self.selection {
                    vec![if s < e { (s, e) } else { (e, s) }]
                } else {
                    vec![(0.0, self.duration_sec)]
                }
            }
            SelectionMode::Multi => {
                if !self.multi_selections.is_empty() {
                    self.multi_selections
                        .iter()
                        .map(|&(s, e)| if s < e { (s, e) } else { (e, s) })
                        .collect()
                } else if let Some((s, e)) = self.selection {
                    vec![if s < e { (s, e) } else { (e, s) }]
                } else {
                    vec![(0.0, self.duration_sec)]
                }
            }
        };

        self.save_filenames = slices
            .into_iter()
            .map(|(s, e)| {
                let dur = (e - s).max(0.001);
                format_sample_filename(base_name, s, dur, "", export_ext)
            })
            .collect();
    }

    pub fn open_save_dialog(&mut self) {
        if self.save_destination_dir.is_empty() {
            self.save_destination_dir = self
                .path
                .parent()
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_else(|| ".".to_string());
        }
        self.reset_save_filenames();
        self.save_dialog_open = true;
    }

    pub fn start_browse_directory(&mut self, now: f64) {
        if !crate::ui_ext::is_zenity_available() {
            self.status_toast = Some((
                crate::ui_ext::StatusMessage::new(
                    crate::icons::Icon::Alert,
                    "Zenity not found; enter destination path directly".to_string(),
                ),
                now,
            ));
            return;
        }
        let (tx, rx) = channel();
        self.browse_rx = Some(rx);
        let start_dir = self.save_destination_dir.clone();
        std::thread::spawn(move || {
            let mut cmd = std::process::Command::new("zenity");
            cmd.arg("--file-selection")
                .arg("--directory")
                .arg("--title=Select Destination Folder");
            if !start_dir.is_empty() && std::path::Path::new(&start_dir).is_dir() {
                cmd.arg(format!("--filename={}/", start_dir));
            }
            if let Ok(output) = cmd.output() {
                if output.status.success() {
                    let picked = String::from_utf8_lossy(&output.stdout).trim().to_string();
                    if !picked.is_empty() {
                        let _ = tx.send(Some(picked));
                        return;
                    }
                }
            }
            let _ = tx.send(None);
        });
    }

    pub fn execute_save(&mut self, now: f64) -> Result<(), String> {
        if self.save_job.is_some() {
            return Err("An audio save is already in progress".to_owned());
        }
        let dest_dir = PathBuf::from(self.save_destination_dir.trim());
        let slices: Vec<(f32, f32)> = match self.selection_mode {
            SelectionMode::Single => {
                if let Some((s, e)) = self.selection {
                    vec![if s < e { (s, e) } else { (e, s) }]
                } else {
                    vec![(0.0, self.duration_sec)]
                }
            }
            SelectionMode::Multi => {
                if !self.multi_selections.is_empty() {
                    self.multi_selections
                        .iter()
                        .map(|&(s, e)| if s < e { (s, e) } else { (e, s) })
                        .collect()
                } else if let Some((s, e)) = self.selection {
                    vec![if s < e { (s, e) } else { (e, s) }]
                } else {
                    vec![(0.0, self.duration_sec)]
                }
            }
        };

        let mut regions = Vec::with_capacity(slices.len());
        for (i, &(start, end)) in slices.iter().enumerate() {
            let mut filename = self
                .save_filenames
                .get(i)
                .cloned()
                .filter(|name| !name.trim().is_empty())
                .unwrap_or_else(|| {
                    let base_name = self
                        .path
                        .file_stem()
                        .and_then(|s| s.to_str())
                        .unwrap_or("audio");
                    let dur = (end - start).max(0.001);
                    format_sample_filename(base_name, start, dur, "", "wav")
                });

            if !filename.contains('.') {
                filename.push_str(".wav");
            }

            let gain_db = match self.selection_mode {
                SelectionMode::Single if self.selection.is_some() => self.single_region_gain_db,
                SelectionMode::Multi if !self.multi_selections.is_empty() => {
                    self.region_gain_db.get(i).copied().unwrap_or(0.0)
                }
                _ => 0.0,
            };
            let (low_pass_hz, high_pass_hz) = match self.selection_mode {
                SelectionMode::Single if self.selection.is_some() => {
                    (self.single_low_pass_hz, self.single_high_pass_hz)
                }
                SelectionMode::Multi if !self.multi_selections.is_empty() => (
                    self.region_low_pass_hz.get(i).copied().flatten(),
                    self.region_high_pass_hz.get(i).copied().flatten(),
                ),
                _ => (None, None),
            };
            let region =
                self.export_region_edit(start, end, 0.0, gain_db, low_pass_hz, high_pass_hz);
            regions.push(AudioSaveRegion {
                start,
                end,
                filename,
                filter: Self::region_filter(region),
            });
        }
        let plan = AudioSavePlan {
            file_source: self.file_source.clone(),
            destination: dest_dir,
            source: self.path.clone(),
            regions,
            open_folder: self.open_folder_after_save,
        };
        let destination_name = plan_destination_name(&self.save_destination_dir);
        let (ticket, receiver) =
            BackgroundJobs::global().submit(Duration::from_secs(600), move || run_audio_save(plan));
        self.save_job = Some(AudioSaveJob {
            ticket,
            receiver,
            destination_name,
        });
        self.status_toast = Some((
            crate::ui_ext::StatusMessage::new(
                crate::icons::Icon::Save,
                "Saving audio file(s)…".to_owned(),
            ),
            now,
        ));
        Ok(())
    }

    pub(super) fn poll_save_job(&mut self, ctx: &egui::Context, now: f64) {
        let Some(job) = &self.save_job else {
            return;
        };
        let destination_name = job.destination_name.clone();
        ctx.request_repaint_after(Duration::from_millis(80));
        let result = match job.receiver.try_recv() {
            Ok(result) => result,
            Err(TryRecvError::Empty) => return,
            Err(TryRecvError::Disconnected) => Err(JobError::Cancelled),
        };
        self.save_job = None;
        let message = match result {
            Ok(Ok(outcome)) => {
                self.save_dialog_open = false;
                if outcome.failures.is_empty() {
                    crate::ui_ext::StatusMessage::new(
                        crate::icons::Icon::Save,
                        format!(
                            "Saved {} file(s) to {destination_name}/",
                            outcome.saved_count
                        ),
                    )
                } else {
                    crate::ui_ext::StatusMessage::new(
                        crate::icons::Icon::Alert,
                        format!(
                            "Saved {} file(s) to {destination_name}; failed: {}",
                            outcome.saved_count,
                            outcome.failures.join(", ")
                        ),
                    )
                }
            }
            Ok(Err(error)) => crate::ui_ext::StatusMessage::new(crate::icons::Icon::Alert, error),
            Err(error) => crate::ui_ext::StatusMessage::new(
                crate::icons::Icon::Alert,
                format!("Audio save failed: {error}"),
            ),
        };
        self.status_toast = Some((message, now));
        ctx.request_repaint();
    }

    pub fn quick_save(&mut self, now: f64) {
        if self.save_destination_dir.is_empty() {
            self.save_destination_dir = self
                .path
                .parent()
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_else(|| ".".to_string());
        }
        self.reset_save_filenames();
        let _ = self.execute_save(now);
    }

    pub fn render_save_dialog(&mut self, ctx: &egui::Context, palette: &AppPalette, now: f64) {
        self.poll_save_job(ctx, now);
        if !self.save_dialog_open {
            return;
        }

        if let Some(rx) = &self.browse_rx {
            if let Ok(res) = rx.try_recv() {
                if let Some(dir) = res {
                    self.save_destination_dir = dir;
                }
                self.browse_rx = None;
            }
        }

        let mut close_dialog = false;
        let mut trigger_save = false;

        egui::Window::new("Save Audio Selection to File(s)")
            .collapsible(false)
            .resizable(true)
            .default_size(Vec2::new(580.0, 360.0))
            .anchor(egui::Align2::CENTER_CENTER, Vec2::ZERO)
            .show(ctx, |ui| {
                ui.add_space(crate::ui_ext::SPACE_XS);
                ui.heading(
                    egui::RichText::new("Save Audio Slices to Disk")
                        .strong()
                        .color(hex_to_c32(&palette.harmonies.base)),
                );
                ui.label(
                    egui::RichText::new(
                        "Extract and save selections as individual audio files with sample timestamps & durations.",
                    )
                    .color(hex_to_c32(&palette.colors.text_secondary)),
                );
                ui.add_space(crate::ui_ext::SPACE_SM);
                ui.separator();
                ui.add_space(crate::ui_ext::SPACE_SM);

                // 1. Destination Directory
                ui.horizontal(|ui| {
                    ui.add(crate::icons::Icon::Folder.label(egui::RichText::new("Destination:")
                            .strong()
                            .color(hex_to_c32(&palette.colors.text_primary))));
                    let text_width = (ui.available_width() - 90.0).max(100.0);
                    ui.add_sized(
                        [text_width, crate::ui_ext::CONTROL_HEIGHT],
                        egui::TextEdit::singleline(&mut self.save_destination_dir)
                            .id(egui::Id::new("audio_save_destination")),
                    );
                    if ui.add(layout::AudioWorkspace::secondary_button("Browse...", hex_to_c32(&palette.colors.surface_recessed), palette))
                        .on_hover_text("Choose destination folder via file manager dialog").clicked() {
                        self.start_browse_directory(now);
                    }
                });

                ui.add_space(10.0);

                // 2. Filenames list
                let slice_count = self.save_filenames.len();
                ui.horizontal(|ui| {
                    ui.add(crate::icons::Icon::Text.label(egui::RichText::new(format!("Files to Save ({}):", slice_count))
                            .strong()
                            .color(hex_to_c32(&palette.colors.text_primary))));
                    ui.add_space(crate::ui_ext::SPACE_SM);
                    if ui.add(crate::ui_ext::secondary_button_icon(Some(crate::icons::Icon::Undo), "Reset Names", hex_to_c32(&palette.colors.surface_recessed), palette))
                        .on_hover_text("Reset filenames to default original_name + timestamp + duration").clicked() {
                        self.reset_save_filenames();
                    }
                });
                ui.label(
                    egui::RichText::new("Format: [OriginalName]_[SampleTimestamp]_[Duration]")
                        .size(crate::ui_ext::BODY_FONT_SIZE)
                        .color(hex_to_c32(&palette.colors.text_muted)),
                );

                ui.add_space(crate::ui_ext::ITEM_SPACING_Y);
                egui::ScrollArea::vertical()
                    .max_height(160.0)
                    .show(ui, |ui| {
                        for (idx, filename) in self.save_filenames.iter_mut().enumerate() {
                            ui.horizontal(|ui| {
                                ui.label(egui::RichText::new(format!("#{:02}:", idx + 1)).weak());
                                ui.add_sized(
                                    [
                                        (ui.available_width() - 8.0).max(64.0),
                                        crate::ui_ext::CONTROL_HEIGHT,
                                    ],
                                    egui::TextEdit::singleline(filename)
                                        .id(egui::Id::new(("audio_save_filename", idx))),
                                );
                            });
                        }
                    });

                ui.add_space(10.0);
                ui.checkbox(
                    &mut self.open_folder_after_save,
                    "Open destination folder in file manager after saving",
                );

                ui.add_space(crate::ui_ext::SPACE_MD);
                ui.separator();
                ui.add_space(crate::ui_ext::SPACE_SM);

                ui.horizontal(|ui| {
                    let btn_text = if slice_count == 1 {
                        "Save File [Enter]".to_string()
                    } else {
                        format!("Save All {} Files [Enter]", slice_count)
                    };
                    let save_btn = crate::ui_ext::action_button_icon(Some(crate::icons::Icon::Save), btn_text, hex_to_c32(&palette.diff_add), palette)
                        .rounding(Rounding::same(crate::ui_ext::PANEL_CORNER_RADIUS));

                    let is_enter = ui.input(|i| i.key_pressed(egui::Key::Enter));
                    if ui.add(save_btn).clicked() || is_enter {
                        trigger_save = true;
                    }

                    if ui.add(layout::AudioWorkspace::secondary_button("Cancel [Esc]", hex_to_c32(&palette.colors.surface_recessed), palette)).clicked()
                        || ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                        close_dialog = true;
                    }
                });
            });

        if trigger_save {
            let _ = self.execute_save(now);
        } else if close_dialog {
            self.save_dialog_open = false;
        }
    }
}

pub(super) fn handoff_audio_slice(
    path: &Path,
    cancelled: &AtomicBool,
    send_to_daemon: impl FnMut(&IpcRequest) -> Result<(), String>,
    mut copy_to_system: impl FnMut(&Path) -> Result<(), String>,
) -> Result<AudioSliceDelivery, String> {
    let owned_path = path.to_path_buf();
    handoff_audio_files(
        std::slice::from_ref(&owned_path),
        cancelled,
        send_to_daemon,
        |paths| copy_to_system(&paths[0]),
    )
}

pub(super) fn handoff_audio_files(
    paths: &[PathBuf],
    cancelled: &AtomicBool,
    mut send_to_daemon: impl FnMut(&IpcRequest) -> Result<(), String>,
    mut copy_to_system: impl FnMut(&[PathBuf]) -> Result<(), String>,
) -> Result<AudioSliceDelivery, String> {
    if cancelled.load(Ordering::Acquire) {
        return Err("Audio slice copy cancelled".to_owned());
    }
    if paths.is_empty() {
        return Err("No audio slices were available to hand off".to_owned());
    }
    let request = if paths.len() == 1 {
        IpcRequest::PushFile {
            path: paths[0].clone(),
            hand: little_clip_core::HandSide::Left,
        }
    } else {
        IpcRequest::PushFiles {
            paths: paths.to_vec(),
            hand: little_clip_core::HandSide::Left,
        }
    };
    match send_to_daemon(&request) {
        Ok(()) => Ok(AudioSliceDelivery::Daemon),
        Err(ipc_error) => {
            if cancelled.load(Ordering::Acquire) {
                return Err("Audio slice copy cancelled".to_owned());
            }
            copy_to_system(paths).map_err(|clipboard_error| {
                format!("Daemon handoff failed ({ipc_error}); system clipboard failed ({clipboard_error})")
            })?;
            Ok(AudioSliceDelivery::SystemClipboard)
        }
    }
}

pub fn run_audio_command_cancellable(
    command: &mut std::process::Command,
    cancelled: &AtomicBool,
) -> Result<std::process::ExitStatus, String> {
    let mut child = command
        .spawn()
        .map_err(|error| format!("Could not start audio process: {error}"))?;
    loop {
        if cancelled.load(Ordering::Acquire) {
            let _ = child.kill();
            let _ = child.wait();
            return Err("Audio process cancelled".to_owned());
        }
        match child.try_wait() {
            Ok(Some(status)) => return Ok(status),
            Ok(None) => std::thread::sleep(Duration::from_millis(20)),
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("Could not wait for audio process: {error}"));
            }
        }
    }
}

pub(super) fn extract_audio_slice_file(
    plan: &AudioSliceCopyPlan,
    cancelled: &AtomicBool,
) -> Result<PathBuf, String> {
    if cancelled.load(Ordering::Acquire) {
        return Err("Audio slice copy cancelled".to_owned());
    }
    let staging_dir = plan.output.parent().unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(staging_dir)
        .map_err(|error| format!("Failed to create audio staging directory: {error}"))?;
    let mut command = std::process::Command::new("ffmpeg");
    command
        .arg("-y")
        .arg("-ss")
        .arg(format!("{:.4}", plan.start))
        .arg("-to")
        .arg(format!("{:.4}", plan.end))
        .arg("-i")
        .arg(&plan.source);
    if let Some(filter) = &plan.filter {
        command.arg("-af").arg(filter);
    }
    command.arg("-c:a").arg("pcm_s16le").arg(&plan.output);
    command
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    let status = run_audio_command_cancellable(&mut command, cancelled)
        .map_err(|error| format!("ffmpeg process failed: {error}"))?;
    if !status.success() || !plan.output.exists() {
        return Err(format!(
            "Failed to extract audio segment via ffmpeg ({status})"
        ));
    }
    Ok(plan.output.clone())
}

pub(super) fn extract_and_handoff_audio_slice(
    plan: AudioSliceCopyPlan,
    cancelled: &AtomicBool,
) -> Result<AudioSliceDelivery, String> {
    let output = extract_audio_slice_file(&plan, cancelled)?;
    handoff_audio_slice(
        &output,
        cancelled,
        |request| match crate::send_ipc_request_with_timeout(request, Some(Duration::from_secs(2)))
        {
            Ok(little_clip_core::ipc::IpcResponse::Success { .. }) => Ok(()),
            Ok(little_clip_core::ipc::IpcResponse::Error { error }) => Err(error),
            Ok(response) => Err(format!("unexpected daemon response: {response:?}")),
            Err(error) => Err(error.to_string()),
        },
        |path| {
            let path = path.to_path_buf();
            little_clip_core::copy_file_uris_to_system_clipboard(std::slice::from_ref(&path))
                .map_err(|error| error.to_string())
        },
    )
}

pub(super) fn run_audio_slice_batch(
    plans: &[AudioSliceCopyPlan],
    mode: ChannelCopyMode,
    cancelled: &AtomicBool,
    mut extract: impl FnMut(&AudioSliceCopyPlan, &AtomicBool) -> Result<PathBuf, String>,
    send_to_daemon: impl FnMut(&IpcRequest) -> Result<(), String>,
    copy_to_system: impl FnMut(&[PathBuf]) -> Result<(), String>,
) -> Result<String, String> {
    let mut paths = Vec::with_capacity(plans.len());
    let mut completed = Vec::with_capacity(plans.len());
    let mut failures = Vec::new();
    for plan in plans {
        if cancelled.load(Ordering::Acquire) {
            return Err("Audio slice copy cancelled".to_owned());
        }
        let name = plan
            .output
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("audio slice")
            .to_owned();
        match extract(plan, cancelled) {
            Ok(path) => {
                paths.push(path);
                completed.push(name);
            }
            Err(error) => failures.push(format!("{name}: {error}")),
        }
    }
    if paths.is_empty() {
        return Err(format!(
            "All {} audio slices failed: {}",
            plans.len(),
            failures.join("; ")
        ));
    }
    let partial_failures = if failures.is_empty() {
        String::new()
    } else {
        format!("; failed: {}", failures.join("; "))
    };
    let delivery = handoff_audio_files(&paths, cancelled, send_to_daemon, copy_to_system).map_err(
        |error| {
            format!(
                "Failed to hand off {} extracted slice(s) ({}): {error}{partial_failures}",
                paths.len(),
                completed.join(", ")
            )
        },
    )?;
    let delivery = match delivery {
        AudioSliceDelivery::Daemon => "Left Hand",
        AudioSliceDelivery::SystemClipboard => "system clipboard fallback",
    };
    let mode = match mode {
        ChannelCopyMode::Native => "",
        ChannelCopyMode::LeftOnly => " LEFT (mono)",
        ChannelCopyMode::RightOnly => " RIGHT (mono)",
    };
    Ok(format!(
        "Copied {}/{} audio{mode} slices to {delivery}; completed: {}{partial_failures}",
        paths.len(),
        plans.len(),
        completed.join(", ")
    ))
}

pub(super) fn audio_slice_copy_status(
    mode: ChannelCopyMode,
    start: f32,
    end: f32,
    delivery: AudioSliceDelivery,
) -> String {
    let description = audio_slice_copy_description(mode);
    let delivery = match delivery {
        AudioSliceDelivery::Daemon => "Left Hand",
        AudioSliceDelivery::SystemClipboard => "system clipboard fallback",
    };
    format!(
        "Copied {description} ({:02}:{:04.1} - {:02}:{:04.1}, {:.3}s) to {delivery}",
        (start / 60.0) as u32,
        start % 60.0,
        (end / 60.0) as u32,
        end % 60.0,
        (end - start).max(0.001)
    )
}

pub(super) fn audio_slice_copy_description(mode: ChannelCopyMode) -> &'static str {
    match mode {
        ChannelCopyMode::Native => "slice",
        ChannelCopyMode::LeftOnly => "LEFT (mono) slice",
        ChannelCopyMode::RightOnly => "RIGHT (mono) slice",
    }
}

pub(super) fn audio_slice_copy_plan(
    source: &Path,
    start: f32,
    end: f32,
    mode: ChannelCopyMode,
    filter: Option<String>,
) -> AudioSliceCopyPlan {
    let base_name = source
        .file_stem()
        .and_then(|name| name.to_str())
        .unwrap_or("audio");
    let suffix = match mode {
        ChannelCopyMode::Native => "",
        ChannelCopyMode::LeftOnly => "_left_mono",
        ChannelCopyMode::RightOnly => "_right_mono",
    };
    let filename =
        format_sample_filename(base_name, start, (end - start).max(0.001), suffix, "wav");
    AudioSliceCopyPlan {
        source: source.to_owned(),
        output: std::env::temp_dir()
            .join("little_clip_audio_segments")
            .join(uuid::Uuid::new_v4().to_string())
            .join(filename),
        start,
        end,
        filter,
    }
}

pub(super) fn run_audio_save(plan: AudioSavePlan) -> Result<AudioSaveOutcome, String> {
    if let Some(source) = &plan.file_source {
        source.check()?;
    }
    std::fs::create_dir_all(&plan.destination)
        .map_err(|error| format!("Cannot create destination directory: {error}"))?;
    let mut saved_paths = Vec::new();
    let mut failures = Vec::new();
    for region in &plan.regions {
        match save_audio_region(&plan, region) {
            Ok(path) => saved_paths.push(path),
            Err(error) => failures.push(format!("{}: {error}", region.filename)),
        }
    }
    if saved_paths.is_empty() {
        return Err(if failures.is_empty() {
            "ffmpeg did not produce any output files".to_owned()
        } else {
            format!("Failed to save audio file(s): {}", failures.join(", "))
        });
    }
    if plan.open_folder {
        let _ = std::process::Command::new("xdg-open")
            .arg(&plan.destination)
            .spawn();
    }
    Ok(AudioSaveOutcome {
        saved_count: saved_paths.len(),
        failures,
    })
}

fn save_audio_region(plan: &AudioSavePlan, region: &AudioSaveRegion) -> Result<PathBuf, String> {
    let out_path = plan.destination.join(&region.filename);
    let staging = if plan.file_source.is_some() {
        Some(
            tempfile::Builder::new()
                .prefix(".audio-export-")
                .suffix(&format!(
                    ".{}",
                    out_path
                        .extension()
                        .and_then(|ext| ext.to_str())
                        .unwrap_or("wav")
                ))
                .tempfile_in(&plan.destination)
                .map_err(|e| e.to_string())?,
        )
    } else {
        None
    };
    let output_path = staging
        .as_ref()
        .map_or(out_path.as_path(), |file| file.path());
    let mut cmd = std::process::Command::new("ffmpeg");
    cmd.arg("-y")
        .arg("-ss")
        .arg(format!("{:.4}", region.start))
        .arg("-to")
        .arg(format!("{:.4}", region.end))
        .arg("-i")
        .arg(&plan.source);
    if let Some(filter) = &region.filter {
        cmd.arg("-af").arg(filter);
    }
    match out_path
        .extension()
        .and_then(|ext| ext.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("wav") => {
            cmd.arg("-c:a").arg("pcm_s16le");
        }
        Some("flac") => {
            cmd.arg("-c:a").arg("flac");
        }
        _ => {}
    }
    let status = cmd
        .arg(output_path)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map_err(|e| e.to_string())?;
    if !status.success() || !output_path.exists() {
        return Err(format!("ffmpeg exit {status}"));
    }
    if let Some(staging) = staging {
        plan.file_source.as_ref().unwrap().check()?;
        staging
            .persist_noclobber(&out_path)
            .map_err(|e| e.to_string())?;
    }
    Ok(out_path)
}

pub(super) fn plan_destination_name(path: &str) -> String {
    Path::new(path.trim())
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .unwrap_or("folder")
        .to_owned()
}
