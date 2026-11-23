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

- [ ] Save a versioned recipe with stable source and lane identities, lossless paths, source revisions, alignment, equations and their sampling settings, filters, decoder wiring, markers, selections and display state.
- [ ] Restore the guarded sources and the dependencies needed to reconstruct results. Show missing and changed recordings at their affected lanes, with a way to repair the binding. Retain surviving identities and edits.
- [ ] Use the existing safe save and viewer close paths. Verify a round trip with PWM or I²S pins, derived lanes, an active filter and an offset, plus removed sources, a corrupt recipe and a Save As collision.

Put Save analysis by the workspace actions. For a new session, collect its filename and destination in place, then show the saved filename in the header. Reopening from Files should restore the named workspace, land at its last working interval and list any source that needs attention beside its recording row. Use paths relative to the recipe directory where possible and preserve the recording identities already held by the readers.

## 4. Match clocks across recordings

- [ ] Let the user pair landmarks from the traces, inspect each pair and use two or more pairs to propose drift correction. Show the reference epoch, offset, rate in ppm and per-pair residuals. Each residual links back to both native landmarks.
- [ ] Route timelines, inputs, selections, decoders and caches through a centered native/workspace transform. Preserve native times and expose preview, apply and reset in the alignment panel. Keep single-offset alignment available.
- [ ] Verify the existing fixed-delay fixture, a three-pair case with an inconsistent landmark, coincident and reversed landmarks, large timestamps and a changed source during calculation.

## 5. Explore CAN inter-frame intervals

- [ ] Open intervals from the selected frame's bus and typed identifier. Show the interval distribution over time and a table of long gaps. Selecting an interval must bring both exact frame endpoints within reach, and returning from a frame must retain interval context.
- [ ] Use the bounded indexed scan and integer timestamps. Show limited coverage, timestamp reversals and duplicates explicitly. Count intervals between observed frames without inferring packet loss.
- [ ] Verify the 0, 5 and 10 ms observations of Bus 1 `0x241` in `Bench supply telemetry.asc`, standard and extended identifiers, multiple buses, time filtering and endpoints beyond the current page. Move the database attachment into the CAN workspace.

## 6. Compare two RF sweeps

- [ ] Open two RF sweeps together from Files. Show stable A and B identities with a swap control. Select a shared trace and link the frequency markers, readouts and original measurement rows.
- [ ] Compare bandwidth, notch depth and group delay with compatible settings. Show the coverage and the source measurements used for each delta. Distinguish exact and interpolated points, and limit comparisons to common coverage.
- [ ] Verify a shared S11 point at 100 MHz, an off-grid point at 130 MHz, a trace present in one sweep and a threshold interval that extends beyond capture. Preserve the one-sweep cutoff and gate interactions.

## Review after each addition

Walk a representative task in both directions, from the source to its result and from a surprising result back to the source. Check keyboard and pointer operation, narrow windows, source changes and the recovery path. Time large-file work on the existing guarded readers and prioritize whichever render or read cost most limits the real interaction. Use the next review to find related improvements across the engineering viewers and their shared workflows.
