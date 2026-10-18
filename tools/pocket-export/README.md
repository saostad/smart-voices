# Pocket TTS Farsi export (Phase 1 tooling, not shipped in the app)

Exports mehdi-hf/pocket-tts-farsi (v1) to a single quantized ONNX file with
pocket-tts-onnx, baking the HF example voice (5 s max).

Run from repo root:

    uv run --project tools/pocket-export python export_farsi.py --check

Output: tools/pocket-export/out/pocket-farsi-v1.onnx (gitignored, ~230 MB).
