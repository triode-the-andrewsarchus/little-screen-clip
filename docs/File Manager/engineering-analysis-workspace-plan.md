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
- [ ] Exercise hover with missing samples and selections at the limits of the visible zoom range.
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

- [ ] Place the four filter actions next to the active trace's spectrum. Initialize a new stage from its marked frequency when available and focus its row.
- [ ] Give each stage an inline row with its current frequency, Q and remove action. Pair the row with the same stage's plot handles. Retain the selected trace, time range and other stages while editing.
- [ ] Verify LP, HP, BP and Notch placement and dragging on measured and derived traces, independent stages for two channels, a narrow window, and saving and reopening the existing filter settings.

The current scope and linked-recording views share the stage controls. Filter creation and stage edits each open a menu, even when the visible spectrum already shows the matching handles. Use a compact row of direct actions and show stages in the workspace below the spectrum settings. Indicate the selected stage in the list and on the plot when the user focuses either representation. Keep the time interval and raw or filtered spectrum state visible while manipulating the frequency response.

## Review after each addition

Walk a representative task in both directions, from the source to its result and from a surprising result back to the source. Check keyboard and pointer operation, narrow windows, source changes and the recovery path. Time large-file work on the existing guarded readers and prioritize whichever render or read cost most limits the real interaction. Use the next review to find related improvements across the engineering viewers and their shared workflows.
