#!/usr/bin/env python3
"""Export Pocket TTS Farsi v1 to a single quantized ONNX file and check it.

The upstream pocket-tts-onnx exporter builds models by config *name*
(``TTSModel.load_model(language=...)``). The Farsi model only exists as an
``hf://`` config, so this script monkey-patches ``load_model`` to route an
``hf://`` language string to ``config=`` and to bake the Farsi-verified
generation settings (temperature 0.3, eos_threshold -2.0) into the ONNX
metadata. The Rust engine enforces the same values at runtime.

Usage (from repo root):
    uv run --project tools/pocket-export python export_farsi.py
    uv run --project tools/pocket-export python export_farsi.py --check

Voice: the HF `example_voice.wav` is downloaded and baked in with at most
5 s of prompt (the training window). Without the private eval folder the
chosen `ref.wav` cannot be baked; see README.
"""

from __future__ import annotations

import argparse
import json
import math
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
OUT = HERE / "out" / "pocket-farsi-v1.onnx"
VOICE_DIR = HERE / "voice"
CONFIG = "hf://mehdi-hf/pocket-tts-farsi/farsi.yaml"
TEMPERATURE = 0.3
EOS_THRESHOLD = -2.0

# Check sentences: short, medium, and number/punctuation heavy. These stand
# in for the private sentences.tsv, which is not on this machine; results on
# the real file are UNKNOWN until it is provided.
CHECK_TEXTS = [
    "سلام، حال شما چطور است؟",
    "امروز هوای تهران آفتابی است و دمای هوا بیست و پنج درجه است.",
    "قیمت این کتاب صد و بیست هزار تومان است و در سال هزار و چهارصد و دو چاپ شده است.",
    "او گفت: «فردا ساعت نه و نیم صبح جلسه داریم؛ لطفاً دیر نکنید!»",
]


def ensure_voice() -> Path:
    VOICE_DIR.mkdir(parents=True, exist_ok=True)
    dest = VOICE_DIR / "example_voice.wav"
    if dest.exists() and dest.stat().st_size > 0:
        return dest
    from huggingface_hub import hf_hub_download

    path = hf_hub_download(
        repo_id="mehdi-hf/pocket-tts-farsi", filename="example_voice.wav"
    )
    dest.write_bytes(Path(path).read_bytes())
    return dest


def export(voice: Path) -> Path:
    from pocket_tts import TTSModel

    original = TTSModel.load_model.__func__

    def patched(cls, language=None, config=None, **kwargs):
        if isinstance(language, str) and language.startswith("hf://"):
            kwargs.setdefault("temp", TEMPERATURE)
            kwargs.setdefault("eos_threshold", EOS_THRESHOLD)
            return original(cls, config=language, **kwargs)
        return original(cls, language=language, config=config, **kwargs)

    TTSModel.load_model = classmethod(patched)  # type: ignore[method-assign]

    from pocket_tts_onnx.export.export import export_model

    OUT.parent.mkdir(parents=True, exist_ok=True)
    export_model(
        OUT,
        language=CONFIG,
        voices=[str(voice)],
        voice_seconds=5.0,
        quantize=True,
    )
    return OUT


def check(onnx_path: Path, voice: Path) -> int:
    """Compare the ONNX runtime against the upstream torch model.

    Prints per-sentence token counts and durations for both paths. The flow
    sampler is chaotic, so waveforms are never compared sample-by-sample;
    what matters is matching tokenization, similar duration, and no runaway
    (duration over the `tokens / 3.0 + 2.0` s cap scaled by 0.97).
    """
    failures = 0

    from pocket_tts_onnx import PocketTTS

    onnx_tts = PocketTTS(str(onnx_path))
    voices = onnx_tts.voices()
    print(f"ONNX voices: {voices}, sample_rate={onnx_tts.sample_rate}")
    if not voices:
        print("FAIL: exported file carries no baked voice")
        return 1
    voice_name = voices[0]

    from pocket_tts import TTSModel

    model = TTSModel.load_model(
        config=CONFIG, temp=TEMPERATURE, eos_threshold=EOS_THRESHOLD
    )
    state = model.get_state_for_audio_prompt(str(voice))
    sp = model.flow_lm.conditioner.tokenizer.sp

    for text in CHECK_TEXTS:
        tokens = len(sp.encode(text))
        cap = (tokens / 3.0 + 2.0) * 0.97

        import numpy as np

        ref = np.asarray(
            model.generate_audio(
                state, text, frames_after_eos=0, copy_state=True
            ),
            dtype=np.float32,
        ).reshape(-1)
        ref_sec = len(ref) / model.sample_rate

        samples, rate = onnx_tts.create(
            text, voice=voice_name, temperature=TEMPERATURE
        )
        onnx_sec = len(samples) / rate

        # Tokenization inside the ONNX file must match upstream exactly.
        onnx_tokens = len(onnx_tts.tokenizer.encode(text))
        same_tokens = onnx_tokens == tokens
        runaway = onnx_sec > cap or ref_sec > cap
        # Durations should be in the same ballpark; sampling noise moves the
        # EOS frame, so allow 35% either way.
        similar = abs(onnx_sec - ref_sec) <= max(0.35 * ref_sec, 0.3)
        ok = same_tokens and similar and not runaway
        failures += 0 if ok else 1
        print(
            f"{'OK ' if ok else 'FAIL'} tokens ref={tokens} onnx={onnx_tokens} "
            f"dur ref={ref_sec:.2f}s onnx={onnx_sec:.2f}s cap={cap:.2f}s "
            f"text={text[:40]}"
        )

    meta = json.loads(
        onnx_tts.session.get_modelmeta().custom_metadata_map["pocket_tts_config"]
    )
    print(
        "metadata: "
        f"sample_rate={meta['sample_rate']} temperature={meta['temperature']} "
        f"eos_threshold={meta['eos_threshold']} "
        f"frames_after_eos={meta['frames_after_eos']} quantized={meta['quantized']}"
    )
    if meta["sample_rate"] != 24000:
        print("FAIL: sample_rate is not 24000")
        failures += 1
    print(f"file size: {onnx_path.stat().st_size / 1e6:.1f} MB")
    return 1 if failures else 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--check", action="store_true", help="verify after export (also runs without re-export if the file exists with --check-only)")
    parser.add_argument("--check-only", action="store_true", help="only verify the existing export")
    args = parser.parse_args()

    voice = ensure_voice()
    print(f"voice prompt: {voice}")
    if not args.check_only:
        export(voice)
        print(f"wrote {OUT}")
    if args.check or args.check_only:
        if not OUT.exists():
            print(f"FAIL: {OUT} does not exist; export first", file=sys.stderr)
            return 1
        return check(OUT, voice)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
