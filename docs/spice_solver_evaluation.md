# Explicit SPICE simulation evaluation

Files passively reads circuit decks and waveform results. Simulation was evaluated separately as a possible explicit action for trusted, self-contained decks.

| Candidate | Fit for Files | Constraints |
| --- | --- | --- |
| Dune 0.8.1 | Rust engine accepts the SPICE3-derived IR from `dune-netlist`. It writes SPICE binary RAW, which the existing waveform preview can read. A custom include loader can deny dependency reads. | Its dialect and model set differ from ngspice and LTspice. The CLI is a separate distribution. A disposable process would still need hard execution and output limits. Review the engine's model licenses before bundling. |
| ngspice 47 | Mature command-line solver with broad model and analysis coverage. Batch mode writes RAW and can suppress local startup files with `-n`. It works as a separately installed reference solver. | A deck can use control commands, include files and load external models. Admission must restrict the accepted grammar and the helper must confine filesystem, network, startup configuration, resource usage and child lifetime. |

I ran the same self-contained RC deck through Dune's published v0.8.1 Linux binary and ngspice 47. Each run used a 10-second wall limit, 5-second CPU limit, 768 MiB address-space limit and 8 MiB output file limit. Both solvers returned 1.0 V for the output operating point. At 4 ms they returned 0.999664213 V and 0.999664224 V, respectively. Dune emitted 816 transient rows and ngspice emitted 819. This case verifies that matched voltage results can coexist with different adaptive sample grids.

## Run Simulation in Files

Open a local SPICE deck, choose **Simulation…**, then choose **Run Simulation** for a deck you trust. Files uses a separately installed ngspice on Linux. The action requires /usr/bin/bwrap and /usr/bin/ngspice with an unprivileged user namespace, as well as the system shell, cat and runtime libraries. The sandbox probe leaves **Run Simulation** disabled if the required isolation is unavailable. Opening, previewing and comparing a deck never starts the solver.

The admitted subset is ASCII with LF or CRLF, a plain title, up to 64 elements and 16 KiB, a final .end, and at most one .op and one .tran. Values are finite numbers with optional SPICE multipliers. The elements are positive-valued R, C and L and independent V and I sources. Sources may be constant, DC or a seven-number PULSE with positive rise, fall and width. Transient analysis accepts .tran step stop, with stop at most 60 seconds and stop/step at most 20,000. Includes, libraries, external models, control blocks, expressions, continuation lines and other directives are rejected before launch. The launcher supplies the exact admitted snapshot as a read-only in-sandbox file.

The disposable worker uses separate mount, user, network, PID, IPC and UTS namespaces. It has no home, host configuration, procfs, or host workspace mounted. System runtime libraries and fixed executables are read-only. Startup environment and local ngspice startup files are excluded. The worker owns a 16 MiB temporary filesystem, 10-second wall deadline, 5-second CPU limit, 768 MiB address-space limit and 8 MiB file and captured-output limits. Cancellation kills and reaps the worker. Results stay in the viewer session and pass through the existing RAW decoder. Missing analyses and transients that stop early produce an error.

[Dune architecture and packages](https://github.com/milkway/dune/tree/v0.8.1) [Dune compatibility differences](https://github.com/milkway/dune/blob/v0.8.1/corpus/KNOWN_DIFFS.md) [ngspice batch mode and RAW output](https://nmg.gitlab.io/ngspice-manual/analysesandoutputcontrol_batchmode.html) [Bubblewrap isolation options](https://github.com/containers/bubblewrap/blob/main/bwrap.xml)
