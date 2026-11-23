# Engineering analysis workspace

## Review of the current build

Astra followed the workflow from Files to a four-channel Rigol capture, mixed recordings, PWM and I²S equations, condition-based measurements and RF bandwidth. The review used the supplied scope screenshot, captured screens of the existing build, fixture data and the Rust source. In stacked scope view, voltage ticks for neighboring channels run together, and hover values appear across the top of the graph. The derived editor covers the source plots, and the plotted outputs show their names alone. The RF cutoff and the gated-measurement results already link numbers to the data that produced them.

The branch adds four floating interfaces for adding a recording, editing equations, matching recording timing and attaching a CAN database. The plan makes each available in the workspace where its effects can be seen. Review each change with real recordings at normal and narrow window sizes before designing the next interaction.

## 1. Open a set of recordings from Files

- [x] Open a supported multi-selection with Enter or a visible action beside the selection count. Keep the normal single-file action and the explicit file comparison command.
- [x] Use the existing guarded background loader and identify a file by name if its contents fail to load. Return to the original selection when the viewer closes.
- [x] Remove the typed-path recording dialog. Exercise a mixed selection across expanded folders, mouse and keyboard opening, one unsupported recording and the return to Files.

## 2. Make scope and equation state readable at the trace

- [x] Give stacked channels individually labeled plots, separated scales, units and direct visibility buttons. Keep overlay available for comparable units.
- [x] Put a display offset control and reset on each applicable trace. Indicate a nonzero offset with its unit. Use original samples for measurement, filtering, equations, copying and snapping.
- [x] Draw a hover value at each visible trace intersection. Group labels where the panels would overlap and clamp them to the viewport. Confirm separated labels in stacked view and grouped labels in overlay using the four-channel Rigol recording.
- [x] Exercise hover with missing samples and selections at the limits of the visible zoom range.
- [x] Show each math lane's equation beside its result. Support independent editing, add and remove, naming, inline errors, and direct spectrum targeting. Keep source aliases, predicates, decoded inputs, filters, resampling and feedback connected to their existing evaluators. Preserve surviving bindings when an equation changes.
- [x] Check the inline editor with the four-channel Rigol capture, PWM and I²S inputs, a rejected edit and its recovery, condition-based measurements and a short window.
- [ ] Keep filter actions recognizable and near the affected trace. Move the remaining floating analysis controls into their workspaces.

Place each equation next to the trace it produces, with a direct Spectrum action on the same row. The new-equation action should bring the inline editor into view and focus it. Keep an unapplied draft local to the edited row, and validate the complete dependency graph before replacing the last applied result. Walk through adding two equations, fixing a bad edit in one, and switching their spectra while retaining the other lane's decoder, filter and time selection.

## 3. Save and reopen an analysis

- [x] Save a versioned recipe with stable source and lane identities, lossless paths, source revisions, alignment, equations and their sampling settings, filters, decoder wiring, markers, selections and display state.
- [x] Restore the guarded sources and the dependencies needed to reconstruct results. Show missing and changed recordings at their affected lanes, with a way to repair the binding. Retain surviving identities and edits.
- [x] Use the existing safe save and viewer close paths. Verify a round trip with PWM or I²S pins, derived lanes, an active filter and an offset, plus removed sources, a corrupt recipe and a Save As collision.

Put Save analysis by the workspace actions. For a new session, collect its filename and destination in place, then show the saved filename in the header. Reopening from Files should restore the named workspace, land at its last working interval and list any source that needs attention beside its recording row. Use paths relative to the recipe directory where possible and preserve the recording identities already held by the readers.

## 4. Match clocks across recordings

- [x] Let the user pair landmarks from the traces, inspect each pair and use two or more pairs to propose drift correction. Show the reference epoch, offset, rate in ppm and per-pair residuals. Each residual links back to both native landmarks.
- [x] Route timelines, inputs, selections, decoders and caches through a centered native/workspace transform. Preserve native times and expose preview, apply and reset in the alignment panel. Keep single-offset alignment available.
- [x] Verify the existing fixed-delay fixture, a three-pair case with an inconsistent landmark, coincident and reversed landmarks, large timestamps and a changed source during calculation.

Open alignment in the workspace above the linked recordings. Keep the reference and target visible there. Take each landmark from its recording’s cursor, then place both native times and the residual on the same row. Selecting an endpoint should bring that landmark into view without discarding the rest of the pair. Use a single pair for an offset and two or more pairs for drift. Edit native times at full precision and use the checkbox to exclude a suspect pair while keeping its residual visible. Preview the correction on the linked traces before applying it, keep the residuals visible, and save applied correction and native landmarks with the session.

## 5. Explore CAN inter-frame intervals

- [x] Open intervals from the selected frame's bus and typed identifier. Show the interval distribution over time and a table of long gaps. Selecting an interval must bring both exact frame endpoints within reach, and returning from a frame must retain interval context.
- [x] Use the bounded indexed scan and integer timestamps. Show limited coverage, timestamp reversals and duplicates explicitly. Count intervals between observed frames without inferring packet loss.
- [x] Verify the 0, 5 and 10 ms observations of Bus 1 `0x241` in `Bench supply telemetry.asc`, standard and extended identifiers, multiple buses, time filtering and endpoints beyond the current page. Move the database attachment into the CAN workspace.

Open Intervals beside the selected frame’s actions. Keep the bus and identifier width visible, and start with the brushed time range when one exists. Plot the observed durations against the later frame’s time, preserving extremes in dense regions. Brushing the plot narrows the same query. Selecting a point or a row highlights its interval and exposes both source frame numbers. Open either frame in the same workspace with its payload and decoded fields, and return to the selected interval with the range and table intact. Put the DBC path field beside the attachment controls.

## 6. Compare two RF sweeps

- [x] Open two RF sweeps together from Files. Show stable A and B identities with a swap control. Select a shared trace and link the frequency markers, readouts and original measurement rows.
- [x] Compare bandwidth, notch depth and group delay with compatible settings. Show the coverage and the source measurements used for each delta. Distinguish exact and interpolated points, and limit comparisons to common coverage.
- [x] Verify a shared S11 point at 100 MHz, an off-grid point at 130 MHz, a trace present in one sweep and a threshold interval that extends beyond capture. Preserve the one-sweep cutoff and gate interactions.

Open the pair from the Files selection action. Put A and B names beside their plot colors and keep the trace and swap controls above the frequency plot. A shared marker should show both values and link to the actual rows used for an exact or interpolated readout. Use the same cutoff and delay settings for deltas, and show the measurement coverage alongside each result.

The paired viewer uses the plotted scale for off-grid marker readings. A hollow point and its two measured rows identify an interpolation. With different normalization or reference impedances, each trace has its own vertical scale and the frequency cursor still moves through both plots. Widths that reach a coverage boundary are lower bounds. Group delay uses the same aperture in both sweeps, even when its edges fall between different measured rows. [Touchstone specification](https://ibis.org/touchstone_ver2.1/touchstone_ver2_1.pdf)

Private Files sessions checked 100 and 130 MHz, row jumps, A/B swaps, S21 on one sweep, bandwidth and notch deltas, coverage-limited widths, and a 1 ns delay difference. They also covered an 800 × 620 window, separate normalized and ohmic scales in the light theme, and the original cutoff drag and time gate. A 19 MB synthetic sweep with one million records took 0.4 ms to produce a 256-row preview, 490 ms to index, and 647 ms to scan at a cutoff. The indexed lookup of a marker and its two source rows took 0.30 ms at the 95th percentile in the reader benchmark.

## 7. Keep spectrum filters in context

- [x] Place the four filter actions next to the active trace's spectrum. Initialize a new stage from its marked frequency when available and focus its row.
- [x] Give each stage an inline row with its current frequency, Q and remove action. Pair the row with the same stage's plot handles. Retain the selected trace, time range and other stages while editing.
- [x] Verify LP, HP, BP and Notch placement and dragging on measured and derived traces, independent stages for two channels, a narrow window, and saving and reopening the existing filter settings.

The four actions are visible beside the chart on wide windows and above it on narrow windows. Each row uses the same number as its plot handle. Selecting the row reveals that stage on the current frequency scale, and interacting with a handle selects its row. The selected band-pass or notch also shows its edge handles. Recorded and filtered views keep their editing controls and show the time range used for the filter.

The scope smoke test placed three stages on CH1, moved its band-pass from about 18.53 to 22.62 kHz, and added a separate high-pass on CH2. The saved recipe retained the four stages and both channel assignments. In the equation spectrum, a 5 kHz notch attenuated the carrier. Dragging the notch to about 25 kHz made the same carrier visible again. The 800 × 620 check included the inline frequency and Q fields.

## 8. Make spectral measurement settings visible

- [x] Put the available time ranges and window functions in the workspace and make the active settings readable at a glance.
- [x] Show the interval used for the current FFT alongside the peak readout. Keep the time selection, cursor and filter interval independently inspectable.
- [x] Walk a transient peak from frequency to its measured time window and back on a recorded channel, logic line and equation lane.

The FFT controls sit above the spectrum and the result shows both the input interval and the contributing window for the active bin. The window button opens the waveform at that interval with its endpoints marked. Back to spectrum restores the previous viewport. The waveform selection and filter range keep their original meanings throughout the inspection.

In the scope test, a 1 kHz transient in the 1–2 s window rose above the baseline tone. The FFT searched 0–3 s, and the saved analysis still held the original A and B cursors at about 210 and 382 ms after the round trip. The logic and equation viewers also returned to the same FFT result after inspecting the contributing window. The controls and both interval readouts fit at 800 × 620. The Touchstone impulse toolbar also uses direct window and gate choices.

## 9. Check scope markers against missing data and zoom limits

- [x] Exercise the hover readouts with a gap on one channel and valid samples on its neighbors. Distinguish a missing value from a measured zero.
- [x] Inspect a selection at the widest and closest zoom levels and around a gap. Verify the source sample positions, zero-crossing snap and Shift override.
- [x] Check start times, units and display offsets in stacked view and switch compatible traces between stacked and overlay views. Keep measurements tied to the original values.

Put a missing-value readout on its channel at the vertical probe. Break the overview trace at empty columns. In the cursor row, show the active channel, each endpoint's time and nearest recorded sample, and a button to center on that endpoint at the current zoom. Show the sample count and any known gaps beside the selection measurements.

The sample file in Engineering has 15,000 missing CH1 values, a continuous zero on CH2 and a late-starting CH3. At fit and at the four-sample zoom limit, the probe distinguished all three. A wide selection snapped to CH1 samples 36,193 and 55,057, with both values at zero. Center A preserved that selection and brought the original crossing into view. A Shift drag moved A to sample 36,194 at about 130.5 mV. After adding a 500 mA display offset to CH2, the hover readout still showed 0 A raw. The all-voltage variant grouped the three nearby labels in overlay and separated them again in stacked view. At 800 × 620, scrolling below the overview exposed both cursor controls and the range statistics.

## 10. Measure the same interval across recordings

- [ ] Pin an existing A/B selection to the analysis workspace from the source that produced it. Agree on the same times across the aligned recordings and show each source's actual coverage.
- [ ] Show results beside the source that produced them. Select a result to focus its contributing data at the right scale. Preserve independent local cursors, selections and decoder settings.
- [ ] Verify a PWM duty change alongside an analog response, including a trimmed recording, a sampled gap, and a changed clock correction.

Put the pin action beside the A/B readout in multi-recording analysis. Take the interval from the recording's native times and draw its aligned edges on the linked tracks. A source that starts partway through the interval should show its covered portion and identify which values can be measured there. Let the user return from a focused result to the shared interval in one action. Begin by reusing the scope and power statistics and decoded PWM cycles.

## Review after each addition

Walk a representative task in both directions, from the source to its result and from a surprising result back to the source. Check keyboard and pointer operation, narrow windows, source changes and the recovery path. Time large-file work on the existing guarded readers and prioritize whichever render or read cost most limits the real interaction. Use the next review to find related improvements across the engineering viewers and their shared workflows.
