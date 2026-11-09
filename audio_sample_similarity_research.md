# Audio Sample Similarity Research

This document records research for a future, independent feature: exploring and finding short music samples by how they sound, in the spirit of XLN Audio XO or Algonaut Atlas. It does not add this feature to another plan or define an implementation commitment.

Research notes checked on 2026-10-18. Repository descriptions and reported results are distinguished below from our expectations. No comparative listening test or local classification benchmark has been performed.

## Goal

The target is a browser for short individual samples: kicks, snares, hats, percussion, bass hits, tonal one-shots, and effects. A useful result should help answer both “what kind of sound is this?” and “which other samples sound like it?” Those are related but different tasks. A category label such as `snare` is a broad hint; it cannot express all the differences between two snares. Similarity should be useful within a category as well as across categories.

A later visual map could place acoustically similar samples near one another, with audition and fast navigation. The map is a browsing aid; its 2D distances should not be treated as the authoritative similarity score.

This is not stem separation or song identification. An image dHash is also not directly transferable: hashing a waveform or spectrogram image would make similarity depend on rendering and alignment choices without establishing that the resulting neighbors sound alike. A compact audio descriptor or learned embedding is a better starting point; recording fingerprints can remain a separate duplicate-identification tool.

## Comparison at a glance

| Approach | Actual representation | Category mechanism | Most useful lesson |
| --- | --- | --- | --- |
| [Subsample](https://github.com/simonholliday/subsample) | Handcrafted spectral, timbral, and temporal descriptors | Similarity to reference sounds; no trained classifier | Separate feature groups and reference-based matching |
| [Glean](https://github.com/yonie/glean) | Small handcrafted feature vector; PCA for display | Explicit acoustic rules | Simple baseline and map/audition interaction |
| [CLAP sample browser](https://github.com/lyramakesmusic/sample-browser) | Learned audio/text embeddings | Similarity to embedded text tags | Flexible text queries and learned audio neighbors |

Our expectation is that the richer DSP baseline deserves testing against CLAP, rather than assuming the neural model wins. Neither these projects' documentation nor their different category schemes establishes a head-to-head accuracy ranking on our samples.

## Existing approaches reviewed

### Subsample

[Subsample](https://github.com/simonholliday/subsample) is a Linux/macOS command-line sampler that analyzes individual captured or imported sounds and compares them with named reference samples. It does not use a learned embedding or a trained category classifier. Its own documentation describes its matching as classification-free.

Its current comparison vector has 58 values in five independently normalized groups: general spectral shape, sustained MFCC timbre, MFCC change over time, attack-weighted MFCC timbre, and frequency-band energy and decay. Configurable group weights combine cosine similarity scores. The onset and band-energy groups are particularly relevant to percussion; Subsample's documentation suggests emphasizing them for drum libraries. It also ships General MIDI reference samples, so the best match to a reference such as a kick can be used as a category hint.

This looks like the most useful reference for an interpretable, low-cost acoustic baseline and prototype matching. It has richer temporal and timbral measurements than the simple heuristic in Glean, but it publishes no independent accuracy benchmark for classifying arbitrary sample libraries. Its source is licensed under [AGPL-3.0](https://github.com/simonholliday/subsample/blob/main/LICENSE), which matters if code is reused.

### Glean

[Glean](https://github.com/yonie/glean) is a small browser-based sample browser and drum machine directly inspired by XO. For each WAV it computes a hand-built vector of about 17 features: duration, RMS level, zero-crossing rate, attack/decay, spectral centroid/spread/rolloff/flatness, and eight coarse frequency bands. It standardizes these vectors and uses PCA to lay the library out in two dimensions. PCA supplies the displayed map; it is not a learned audio embedding or a drum classifier.

Glean assigns dot colors through explicit rules over duration, spectral brightness, noisiness, and tonalness. The author reports about 50–60% category accuracy on ambiguous one-shots and advises treating the colors as hints. The simple features and map are easy to understand and the project is MIT licensed, making it a useful reference for interaction and a transparent baseline. Expect its labels to confuse acoustically overlapping sounds such as claps and snares. PCA also optimizes broad variance, so apparent 2D distance is not a reliable substitute for ranking in the original feature space.

### CLAP sample browser

[lyramakesmusic/sample-browser](https://github.com/lyramakesmusic/sample-browser) uses LAION CLAP audio embeddings and cosine similarity to find sounds like a reference clip. It also embeds text tags and ranks those against a sample, allowing text-to-audio queries. This provides a learned representation and flexible descriptive search, unlike the handcrafted feature systems above.

The supplied tag list focuses on broad concepts such as drums, percussion, instrumentation, mood, and production qualities; it is not a tuned kick/snare/hat taxonomy. CLAP is trained to align audio and captions, so it may recognize broad concepts without preserving the subtle timbral differences needed to choose between similar one-shots. The repository reports around 4.2 GB VRAM for its setup. That is the project's stated requirement, not a benchmark independently measured here. Its repository does not contain a license file, so its code should not be assumed reusable without checking with its author. The model's own license is separate from the application's code license.

## Other useful baselines

[A Wav2Vec2 one-shot hip-hop drum classifier](https://huggingface.co/yojul/wav2vec2-base-one-shot-hip-hop-drums-clf) was fine-tuned on 17,000 samples and predicts seven classes: kick, snare, cymbal, open hat, hi-hat, 808, and clap. Its card reports 92.4% evaluation accuracy. That result is promising for category suggestions, but it is tied to the project's dataset and label definitions. The model predicts categories; it is not trained to rank fine-grained sample similarity. Its hidden representations could be tested as a baseline, but that use is not evaluated in its model card.

## Likely direction

A custom hybrid is more promising than adopting any one project unchanged:

1. **Interpretable acoustic features** provide an inexpensive baseline and user-understandable controls. Preserve attack, body, and tail information instead of reducing a whole sample to one average spectrum. Include pitch/harmonicity when meaningful, frequency-band energy and decay, spectral texture, duration, and transient strength. Normalize feature groups separately and make their influence adjustable.
2. **A learned audio embedding** may capture higher-level timbral relationships that fixed descriptors miss. Compare candidate embeddings on actual one-shots; track/music embeddings and text-aligned embeddings are not automatically good at ranking two kicks or snares.
3. **Category predictions** should be a separate, uncertain layer used for coloring or filtering. Do not require a hard class label before computing similarity. Allow “other” or uncertain predictions, and preserve useful neighbors across category boundaries.
4. **Text-to-audio search** can be an optional candidate-finding mode for descriptions such as “short, dark, punchy kick.” It should not silently determine the default audio-to-audio similarity metric.
5. **Late fusion** is safer than concatenating raw feature and model vectors. Normalize and calibrate each representation, compare their neighbor rankings, and expose meaningful controls such as timbre, attack, decay, pitch, and brightness. A learned combination can follow if listening tests support it.
6. **Similarity ranking precedes mapping.** First validate nearest-neighbor results against human judgments. Then build a map from the chosen representation. Use the higher-dimensional metric for “more like this”; treat the 2D projection as navigation. Preserve map stability across incremental additions where practical.

## Evaluation needed

Create a small, hand-judged fixture set from the intended sample libraries. Include near alternatives within the same class (different kicks and snares), cross-class confusions (clap/snare, tom/conga), acoustic and electronic examples, tonal hits, effects, and poor or unusual recordings. For each query, collect preferred alternatives and optionally separate judgments for “same category” and “sounds similar.”

Compare Subsample-style DSP features, Glean-style simple features, CLAP, music/audio encoders, and any one-shot classifier representation using top-k neighbor preference and category precision/recall. Measure inference cost and behavior under gain changes, leading silence, sample-rate differences, and very short transients. Keep training/test samples separated by source pack or recording where possible, so near-duplicate files do not make results look better than they are.

The current evidence supports a custom prototype and a listening-based comparison. It does not establish which representation will win on the user's library, nor does it establish that a commercial product's proprietary feature space can be reproduced from these projects.

## Decisions deliberately left for that future feature

- Choose the encoder and DSP descriptors after comparing neighbors on real sample libraries, rather than committing to three simultaneous pipelines.
- Decide whether a dedicated classifier improves browsing enough to justify its model and taxonomy. Similarity browsing can work without one.
- Measure CPU/GPU inference time, memory, and cache size before choosing persistent vector precision or dimensions.
- Evaluate projection stability and incremental additions separately from audio similarity quality.
- Review code and model licenses independently before reuse. An algorithmic reference does not require adopting a project's application or storage design.

These are research questions for a later feature. This document does not enroll them in Phase 6 or change the semantic file-search implementation scope.
