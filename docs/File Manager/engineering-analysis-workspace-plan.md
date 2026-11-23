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

- [x] Pin an existing A/B selection to the analysis workspace from the source that produced it. Agree on the same times across the aligned recordings and show each source's actual coverage.
- [x] Show results beside the source that produced them. Select a result to focus its contributing data at the right scale. Preserve independent local cursors, selections and decoder settings.
- [x] Verify a PWM duty change alongside an analog response, including a trimmed recording, a sampled gap, and a changed clock correction.

Share range is beside the local selection. A purple bracket marks the shared interval on recorded and equation traces, and the header names the source that owns its endpoints. Each recording reports its covered time and the measurements for its active channel. The scope reports present samples and links the minimum and maximum to their original sample neighborhoods. Logic reports observed edges or the extrema of complete PWM cycles when a PWM decoder is selected. The PWM buttons bring the matching cycle and its decoder into view. Show shared range returns to the comparison.

The paired example under Oscilloscope captures spans about 19.46 to 89.37 ms. The analog report found 1,338 of 1,398 samples and a 2 V peak-to-peak difference. The logic recording ended at 75.001 ms and contributed ten complete cycles, with duty values of 25% and 75%. Clicking 75% opened the cycle at 50 ms. Returning kept the original A/B cursors. After a 10 ms logic clock correction, coverage extended to 85.001 ms and the range contained twelve complete cycles. The saved recipe kept the native pin and clock correction. Reopening it at 800 × 620 in the light theme restored both local scope cursors, the shared interval, the selected PWM decoder, and the corrected coverage.

The sample scan is capped at 65,536 readings and the edge scan at 8,192 transitions. Bounded scans identify the actual window used for the readings.

## 11. Add and wire logic decoders in place

- [x] Replace the Add decoder popup with visible protocol actions in the recording and equation workspaces.
- [x] Put the signal assignments where a user can see the participating lanes and inspect the effect of a wiring change. Keep the current assignments and invalid combinations legible in a narrow window.
- [x] Walk from an undecoded SPI or I²C capture to usable annotations, then follow a surprising decoded value to its source pulses. Confirm that saved decoder settings still reopen.

The five supported protocols have direct add buttons above the traces. Each assignment shows its current signal, and the matching trace labels become targets during assignment. The same inline picker serves the active channel and the decoder roles. Large channel lists can be searched. SPI modes and I²S formats have direct buttons. Invalid assignments report the error beside the wiring while the user can still see the signals. The workflow keeps the existing name-based assignment and waveform annotations described in the PulseView manual. [PulseView user manual](https://sigrok.org/doc/pulseview/unstable/manual.html#_basic_operation)

In the two-transfer fixture, assigning SDA to the clock triggered the duplicate-channel error beside the controls. Selecting Bench.SDA again restored the decoded addresses 0x68 and 0x50. Clicking 0x68 at 109 µs and using Show pulses zoomed from 25 ms to about 190 µs and highlighted the address on SCL and SDA. Back to decoded window kept the selected event. The same action highlighted the pulses after decoding two digital equation lanes, and the recorded decoder reopened with its assignments and cursor at 800 × 620 in the light theme.

## 12. Make packet filters visible in the capture

- [x] Surface protocol choices and the criteria currently applied to the packet table. Preserve editing of the Wireshark-style filter expression and report whether an edit has been applied.
- [x] Let a result reveal its source packet and related conversation while retaining the current query and selected time range.
- [x] Verify adding a criterion from the packet inspector, tightening it from the overview, and recovering the prior set and selection. Check narrow windows and a bounded scan.

The packet viewer already links its diagram, field tree, and bytes. Use those links as entry points to the query, and put the applied protocol and time restrictions next to the count they affect.

Start with a capture that contains DNS and several TCP conversations. Choose a protocol using a visible button and see its matching rows and count. Type a further expression and keep the result's applied criteria visible until the new query succeeds. Select a field in the inspector to see its bytes. Add that field to the query from the same place. A selected packet retains its source identity even when a new filter excludes it, so the inspector can explain the restriction and let the user return to the result list. Keep the source brush for inspection and export. Use a separate time restriction when explicitly filtering to that interval, and let the user remove it without changing the expression or conversation.

The match set owns the exact query that produced it. A pending or failed query leaves the previous match set and its labels intact. A protocol click uses the currently requested or applied expression and endpoint, leaving unsubmitted text in the editor. The first acceptance pass should cover a draft that differs from the visible table, a successful protocol update, and recovery from a failed expression. Follow with the field and time round trips, including a source packet outside the match set and a capture large enough to hit the bounded scan.

The fixture had three TCP and nine DNS packets. Choosing DNS while an expression was in draft kept all nine DNS matches until Apply. A failed expression retained the previous three matches and Packet 2. A time brush narrowed those three to one, and removing the time filter restored all three. Clicking a captured IPv4 byte exposed its Source field. Using that field in the filter narrowed the same expression to two matches, with Packet 2 still selected when opening its conversation. At 800 × 620 the protocol controls fit on one line above the diagram and list. A one-million-packet check stopped at 64,000 matches and reported 255,998 source packets searched.

## 13. Keep visibility and decoder setup beside their traces

Astra reviewed the paired Rigol comparison and a thresholded I²C scope capture on commit `577a037c`. The comparison exposes reordering and domain changes on each trace, but finding the same channel again in the Traces menu is required for visibility. Hiding the active trace also leaves the shared inspection tools pointing at it. The scope's threshold popup covers the two digitized lanes and part of the decoded acknowledgement.

- [x] Give every comparison trace a Show or Hide button in its own header. A hidden plot leaves its source, channel and position visible. Reordering and domain changes use distinct controls. The active inspection target moves to the next visible trace when hidden, and the shared tools are unavailable when all traces are hidden.
- [x] Use the shared inline protocol buttons on the scope. Place threshold editing by the corresponding digital lane. Show the fixed or adaptive cutoffs, middle-band interpretation and inversion while the user can see the decoded results. Retain the applied rule when a draft has crossed cutoffs and explain the error next to the fields.
- [x] Walk a two-recording comparison from drag and FFT through Hide, Show, save and reopen. Test hiding and restoring the whole stack. In a scope capture, add I²C, adjust and repair a threshold while watching the annotations, and follow a decoded event back to the analog data.
- [x] Check comparison visibility, decoder placement and the inline threshold editor at 800 × 620.

The paired Rigol test hid the entire stack, restored one CH1 plot, switched it to FFT, moved it, saved the analysis and reopened it. The recipe retained the order, the one visible channel and its domain. In the I²C check the scale changed to fixed cutoffs, then low was set to 3.00 V with high still at 2.00 V. The editor reported the problem while the two decoded acknowledgements stayed visible. Raising high to 3.10 V recovered the rule. Double-clicking the address opened its pulses, and Back to decoded window restored the full transfer. At 800 × 620, Hide moved the active trace to the next visible recording, Show restored the original plot, and all five decoder actions fit on one line above the working threshold editor.

## 14. Connect decoded events to the recorded voltages

- [x] Selecting a decoder annotation marks the event's interval on the digital lanes assigned to that decoder and on their originating voltage traces. The event's name and duration sit beside the decoder controls. Double-clicking an annotation zooms to its pulses and offers a return to the previous time range.
- [x] Use a two-signal I²C capture plus an unrelated analog channel. Confirm that the address highlights clock and data, stays out of FFT lanes, and clears when switching to Edges.
- [x] Move the quick scope predicate into the inline equation and decoder workflow. Keep exact range-to-VCD beside the predicate.
- [ ] Compare the standalone logic and scope workflows and share their remaining timing controls where the interactions match.

The decoder already retains its original result window while following a pulse. Use its event extent and wiring to paint the source interval. This allows a user to distinguish a decoded address from the following acknowledgement, inspect the matching edge positions, and get back to the transfer overview. The analog A/B selection still measures its own interval, and switching a trace to FFT keeps its frequency coordinates.

In the three-channel capture, the address 0x42 ran from 33 to 113 µs and shaded that 80 µs interval on both digital lanes and both originating voltage traces. The unrelated 1.65 V channel kept its own raw probe readout without shading. Switching the clock voltage to FFT kept its spectrum clear while the data waveform still marked the address. Double-clicking the address zoomed to its edges, and Back restored the 238 µs view with the address still highlighted and the controls in place. Switching to Edges cleared the highlight. The decoder selection test also covers two events with coincident timestamps.

A valid scope predicate now exposes Add digital lane and Copy VCD beside its equation. Adding the lane applies the equation directly and gives it stable source bindings. An active-channel comparison stays with that channel when the active trace changes. The derived lane joins the existing inline equation and decoder controls. Copy VCD reads the original sample cells in the brush or visible range.

The private walkthrough added a CH2 threshold and then a CH1 expression with nested comparisons, Boolean operators and a reference to CH2. Both equations retained their channel bindings after saving and reopening the recipe. Wiring the promoted CH1 lane to Clock and the promoted CH2 lane to Data decoded address 0x42 and data 0xA5 with acknowledgements. The exported VCD matched all 91 selected CH2 sample cells and six transitions. At 800 × 620 the added equation and its source trace remained visible together, and closing opened the existing discard confirmation.

## 15. Follow derived decoder events to their inputs

Selecting a decoded event shows its equation inputs above the transaction table. The recording and channel buttons open the corresponding waveform and mark the input window on that channel. A delayed term shows an earlier window, and feedback can extend the window to the start of the equation grid. The readout uses the workspace clock and indicates where interpolation or filter history can contribute neighboring samples. Back to decoded event restores the previous time view and brings the selected result into view. These visits are browsing state and leave the recipe clean.

The desktop walkthrough used two captures with a 4 µs clock correction, reordered channels, a direct data threshold, and a clock threshold retimed through a second equation. Address 0x42 and data 0xA5 both decoded with acknowledgements. Selecting the address at 37 µs showed the input on Clock probe CH1 from 32 to 113 µs, alongside the separate Data probe CH2 input. The unused channels kept their original display. Back restored the address, wiring, full 242 µs viewport, and the independent shared selection from 187 to 212 µs. The same round trip passed at 800 × 620. The recipe closed without a discard prompt in both sizes.

## 16. Select the same transaction in the plot and table

Scope, standalone logic, and equation decoders share the annotation strip interaction. Click a drawn event to select its row without moving the time view. Double-click it to inspect the pulses, then use Back to decoded window to recover the original time range and the selected row. The strip uses the geometry it actually draws. Adjacent events remain individually selectable where they fit, and a table selection stays visible even if that event was covered at the current zoom.

The desktop checks selected I²C address 0x42 from the plot in a Rigol capture, a VCD capture, and a recipe with two clock-aligned recordings. Double-clicking expanded the address pulses and Back restored the full time view with the same event selected. The VCD also passed at 800 × 620. The scope checked a gap between annotations, and the recipe retained its independent 187–212 µs shared range and its links to the recorded input channels. The geometry test covers coincident boundaries, clipped intervals, and a selected event fully covered at the original zoom.

## 17. Locate the recorded window behind an I/Q peak

Peak hold keeps the strongest observed value at each FFT bin and the contributing sample window. The spectrum initially marks the strongest observed bin and accepts another mark directly from the plotted line. The waterfall shows the exact sample and time bounds above it, with a gold band at the source and a frequency marker confined to that window. Show peak window zooms to the source. Marking another peak while inspecting follows its source, and Back to waterfall view restores the original viewport. The brush, peak-hold result, and complete or sampled coverage readout stay available through the visit.

The FFT sizes are direct controls beside the spectrum. A compressed plot column selects the bin that supplies its visible peak. The two frequency axes retain enough digits to distinguish adjacent bins around a large RF center. Peak hold still requires one tuning center across the selected region.

At 1280 × 800 and 800 × 620, the desktop walkthrough followed a +4.096 kHz burst to samples 512–1023 and a −8.192 kHz burst to samples 3072–3583. The second bin read −12.0 dBFS. Back restored the wider waterfall with the second window marked and all 4096 brushed samples still in the peak hold. In the narrow check, a retune immediately after the selection left the wide waterfall on a relative axis and the measured spectrum on the original 915 MHz tuning. Unit tests also cover a retune missed by sampled windows, invalid FFT bins, and tied levels.

## 18. Inspect a CAN signal’s source frame and return to the matches

Clicking a plotted value opens its exact recorded frame, selects the matching DBC field and highlights its payload bits. At a shared timestamp, the nearer plotted value determines which frame opens. A frame outside the matches opens temporarily above the inspector with Back to matches. Frames beyond a bounded filter scan say so. The frame table, time brush and plots keep their current settings. In a short window the inspector scrolls into view when its frame is ready, and Back brings the matches into view.

The two-signal walkthrough filtered to the two 0x123 command frames and brushed 0–580 ms. Two 0x124 response frames shared 580 ms. Clicking the upper point selected Frame 5 at 12 A and highlighted the second byte in `CD 30`. Back restored Frame 1, its selected voltage field and the two matches with the brush intact. Clicking 24 V on the command plot selected Frame 3 and highlighted the first byte in `30 53`. At 800 × 620, clicking the command plot brought Frame 3 and its decoded fields into view.

## 19. Compare antenna matches in a fixed frequency band

Shift-drag either frequency plot or keep the visible range with Use visible match band. Both frequency plots shade the band. The endpoints can be edited in MHz, and Fit band brings the selected frequencies back into view. Best observed stays beside the endpoints. It searches the same interval in both recordings and identifies partial or missing sweep coverage. Click either minimum to place the shared marker at that measured row. Clear band returns the search to the visible frequencies.

The desktop walkthrough used two antenna sweeps with offset frequency grids and 50 Ω and 75 Ω references. B’s visible-band minimum was 436.775 MHz. Brushing about 429.907–434.962 MHz found 433.500 MHz for A and 434.925 MHz for B. Selecting B’s result showed the exact B measurement and the interpolated A value. Zooming past the band kept both match results visible. Clicking B again brought its frequency back into view. Brushing the impedance plot changed the search interval while the marker stayed at 434.925 MHz. Entering 420.000–420.010 MHz found A’s 420 MHz row and showed B outside coverage. At 800 × 620, the band, both results, and all three linked plots fit together.

## 20. Inspect the signal behind a shared power reading

The shared measurements name the contributing recording. Average current and charge open its Current waveform, and energy opens its Power waveform. The highlighted interval is the overlap between the pin and the recorded signal. The integrator weights the portions of sample cells at the boundaries using the held sample values. Estimates from stored means and coverage clipped by the end of a recording keep their respective labels. Back to shared readings restores the previous time view and scroll position. The pin and recipe edit state stay intact.

The desktop check reopened a recipe with a 1 kHz standby capture and a 2 kHz load-step capture shifted forward by 62.5 ms. A’s 557.25 ms–2.51175 s interval produced 0.135729 mAh and 0.678646 mWh. B contributed through 2.0625 s and showed 0.278507 mAh and 0.835521 mWh with partial coverage. B’s energy opened the Power lane and marked its interval across the 1.5–2.25 W step. Back returned to both readings, and A’s charge opened its 250 mA Current lane. The 800 × 620 check also returned to Files without a discard prompt. Core tests cover a missing boundary sample and a range contained within one sample cell.

## 21. Compare Joulescope totals over the same time

With multiple power recordings, Common interval measures the part of the pin covered by all their corrected clocks. Current and power use their own common intervals. Each recording restores the totals over its own coverage of the pin. The waveform statistics still describe the pin in both modes. A power total opens the exact interval used for that reading, and Back restores the viewport and scroll position. Switching the comparison mode counts as browsing.

With the 1 kHz and 2 kHz pair, the pin ran from 557.25 ms to 2.51175 s and the common interval ended at 2.0625 s. A measured 0.104531 mAh and 0.522656 mWh on the common interval. B measured 0.278507 mAh and 0.835521 mWh. Each recording restored A’s 0.678646 mWh and B’s partial label. The shorter Power interval opened from A’s reading and Back restored both rows at 1500 × 1000 and 800 × 620. A second check showed no common coverage after shifting B outside the pin. Removing B’s Power signal from a fixture kept the common current comparison and identified the missing power coverage. The core test integrates different Current and Power intervals in the same recording.

## Review after each addition

Walk a representative task in both directions, from the source to its result and from a surprising result back to the source. Check keyboard and pointer operation, narrow windows, source changes and the recovery path. Time large-file work on the existing guarded readers and prioritize whichever render or read cost most limits the real interaction. Use the next review to find related improvements across the engineering viewers and their shared workflows.
