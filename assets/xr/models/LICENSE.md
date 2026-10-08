# Speech-to-text model

`ggml-base.en.bin`, the English-only Whisper base model in whisper.cpp's
ggml format, used by the XR voice path (`crates/fosfora-xr/src/voice.rs`).

- Source: the public Hugging Face repository `ggerganov/whisper.cpp`,
  <https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-base.en.bin>
- Size: 147,964,211 bytes
- SHA-256: `a03779c86df3323075f5e796cb2ce5029f00ec8869eee3fdfb897afe36c6d002`
- License: MIT (OpenAI's Whisper weights, converted by the whisper.cpp
  project, both MIT).

The file is not in the repository: `scripts/xr/fetch-model.sh` downloads it
into this directory and checks the SHA-256, `scripts/xr/run.sh build` calls
it, and the Android build packages it into the APK's assets.
