# Local search model notices

Weights are downloaded only by explicit setup. These notices do not enroll folders or authorize a download. The application license does not replace the model licenses. Exact weight filenames, revisions, sizes and SHA-256 checksums are in `setup.py` and `answer_models.json`.

All currently offered models declare Apache-2.0. The full license is in `LICENSE.models.apache-2.0`. `LICENSE.models.qwen` additionally preserves the identical upstream Qwen/NuExtract license text, including its Alibaba Cloud copyright notice. Model and projector weights are separate from the MIT-licensed llama.cpp runtime and Recall-derived shim, whose notices are `LICENSE.llama.cpp` and `LICENSE.recall`.

## Default embedding role

- EmbeddingGemma 2 by Google; Q8_0 conversion supplied by ggml-org.
- Weight repository: [ggml-org/embeddinggemma-2-GGUF, pinned revision](https://huggingface.co/ggml-org/embeddinggemma-2-GGUF/tree/bfcd298762cc34d0357ece5ebdd31791a3a374d8).
- Upstream declaration: [google/embeddinggemma-2, reviewed revision](https://huggingface.co/google/embeddinggemma-2/tree/f9972ac22e696a105c482202646979e33e4b5a1e), Apache-2.0.
- Neither reviewed repository includes a separate LICENSE or NOTICE file; retain the declared Apache license above. The upstream review revision is provenance for the license declaration, not a substitute for the pinned GGUF weight revision.

## Default answer role

- Qwen3.5-9B by the Qwen team/Alibaba Cloud; Q8_0 and f16 projector conversion supplied by bartowski.
- Weight repository: [bartowski/Qwen_Qwen3.5-9B-GGUF, pinned revision](https://huggingface.co/bartowski/Qwen_Qwen3.5-9B-GGUF/tree/182be2fd6c7bc44887d88a91cb03ff009cc9f549).
- [Upstream LICENSE at the reviewed source revision](https://huggingface.co/Qwen/Qwen3.5-9B/blob/c202236235762e1c871ad0ccb60c8ee5ba337b9a/LICENSE). Its complete text is retained in `LICENSE.models.qwen`.

## Optional development comparisons

These are explicit alternatives, not additional automatically installed or resident models.

- Qwen3.5-4B: [pinned bartowski conversion](https://huggingface.co/bartowski/Qwen_Qwen3.5-4B-GGUF/tree/4168f45a16a1290d65a4ec0fa312ae917a4c15d6); [reviewed upstream LICENSE](https://huggingface.co/Qwen/Qwen3.5-4B/blob/851bf6e806efd8d0a36b00ddf55e13ccb7b8cd0a/LICENSE), identical to `LICENSE.models.qwen`.
- NuExtract3 by NuMind, based on Qwen3.5-4B: [pinned GGUF conversion](https://huggingface.co/numind/NuExtract3-GGUF/tree/28a1aae6288c5d50bdf999ca2e572240966d669b); [upstream LICENSE at the reviewed source revision](https://huggingface.co/numind/NuExtract3/blob/c99dc8f5641b866aa0192b6ea78f84bf9f3535f1/LICENSE), identical to `LICENSE.models.qwen` after newline normalization.

No separate NOTICE file is present in the reviewed upstream model repositories. Review model notices again when changing a model or pinned revision. Explicit setup copies this document and both model license files beside the installed runtime; copied helper distributions must include them too. These are provenance and redistribution notices, not a claim about the licensing of arbitrary user content.
