# Explicit SPICE simulation evaluation

Files passively reads circuit decks and waveform results. Simulation was evaluated separately as a possible explicit action for trusted, self-contained decks.

| Candidate | Fit for Files | Constraints |
| --- | --- | --- |
| Dune 0.8.1 | Rust engine accepts the SPICE3-derived IR from `dune-netlist`. It writes SPICE binary RAW, which the existing waveform preview can read. A custom include loader can deny dependency reads. | Its dialect and model set differ from ngspice and LTspice. The CLI is a separate distribution. A disposable process would still need hard execution and output limits. Review the engine's model licenses before bundling. |
| ngspice 47 | Mature command-line solver with broad model and analysis coverage. Batch mode writes RAW and can suppress local startup files with `-n`. It works as a separately installed reference solver. | A deck can use control commands, include files and load external models. Admission must restrict the accepted grammar and the helper must confine filesystem, network, startup configuration, resource usage and child lifetime. |

I ran the same self-contained RC deck through Dune's published v0.8.1 Linux binary and ngspice 47. Each run used a 10-second wall limit, 5-second CPU limit, 768 MiB address-space limit and 8 MiB output file limit. Both solvers returned 1.0 V for the output operating point. At 4 ms they returned 0.999664213 V and 0.999664224 V, respectively. Dune emitted 816 transient rows and ngspice emitted 819. This case verifies that matched voltage results can coexist with different adaptive sample grids.

For an application implementation, prefer an explicit action that uses a pinned deck snapshot and a disposable worker. Admit a documented subset of independent sources, passive elements and bounded analyses. Reject unsupported cards and external dependencies before launching the worker. Feed the resulting bytes to the existing per-sweep waveform decoder and viewer. Dune is a candidate for a controlled subset and ngspice remains a useful reference for compatibility. This evaluation adds no solver to Files.

[Dune architecture and packages](https://github.com/milkway/dune/tree/v0.8.1) [Dune compatibility differences](https://github.com/milkway/dune/blob/v0.8.1/corpus/KNOWN_DIFFS.md) [ngspice batch mode and RAW output](https://nmg.gitlab.io/ngspice-manual/analysesandoutputcontrol_batchmode.html)
