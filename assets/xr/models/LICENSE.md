# The voice path's models

The files listed in `MODELS.txt` are not in the repository:
`scripts/xr/fetch-model.sh` downloads each into this directory and checks
its SHA-256, `scripts/xr/run.sh build` calls it, and the Android build
packages them into the APK's assets. The decision model's spec is small,
ours, and committed.

## Speech to text

`ggml-base.en.bin`, the English-only Whisper base model in whisper.cpp's
ggml format, used by the XR voice path (`crates/fosfora-xr/src/voice.rs`).

- Source: the public Hugging Face repository `ggerganov/whisper.cpp`,
  <https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-base.en.bin>
- Size: 147,964,211 bytes
- SHA-256: `a03779c86df3323075f5e796cb2ce5029f00ec8869eee3fdfb897afe36c6d002`
- License: MIT (OpenAI's Whisper weights, converted by the whisper.cpp
  project, both MIT).

## The on-device decision model (board #3751, V5)

The voice path's on-device provider (`crates/fosfora-xr/src/local.rs`): a
17M decision model of our own, trained from the Apache-2.0
`cross-encoder/ettin-reranker-17m-v1` (revision `9e4aa35`) with the MIT
Bekko System One toolkit (commit `0fccbb8`) on synthetic cases generated
from the voice path's own catalog and naming (run 3,
`docs/xr/MEASURED.md`, "Our own System One 17M, trained"). Not hosted yet:
`MODELS.txt` carries a placeholder URL, and a dev build copies the files
in from the training export.

- `s1-17m-int8.onnx`: the model, ONNX with int8 embeddings (blocks and
  heads FP32). 29,023,607 bytes; SHA-256
  `cd7f55ced9e661a86ef05004c4afbb57770059f320fccc3028029a87a99310d1`.
  License: Apache-2.0 (ours).
- `s1-17m-tokenizer.json`: the model's tokenizer (ModernBERT's byte-level
  BPE), the Ettin repository's tokenizer as the training saved it.
  3,583,325 bytes; SHA-256
  `f75521d7f18a32ec8a9dce9d378a5edcea370be41cf86bcc9fc3d687d6f929da`.
  License: Apache-2.0 (Ettin).
- `s1-17m-tokenizer_config.json`: the tokenizer's special token names, as
  the export wrote them. 124 bytes; SHA-256
  `6459575245dbb8f85afefb89547b4a0c88b45934810483c806f885b1a5ad801d`.
  License: Apache-2.0.
- `s1-17m-spec.json` (committed): the provider spec the training pipeline
  writes with the model (the prompt layout, the four decisions'
  instructions and candidates, the id mapping), copied verbatim.
  9,777 bytes; SHA-256
  `59f493342995717cf613f22a251f826b3067b1765186b680b0f6fce21c4e6c01`.
  License: Apache-2.0 (ours).

The runtime that runs it, ONNX Runtime 1.28.0 (MIT), is not here: the
Android build takes `libonnxruntime.so` from the official
`com.microsoft.onnxruntime:onnxruntime-android` AAR on Maven Central
(`android/app/build.gradle.kts`).
