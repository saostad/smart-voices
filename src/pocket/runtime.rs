//! Pocket TTS runtime: the shared ONNX Runtime environment plus tensor and
//! voice-data helpers.

use std::path::PathBuf;
use std::sync::OnceLock;

use crate::engine::EngineError;

/// IEEE-754 half → single conversion for the baked voice conditioning.
fn f16_to_f32(bits: u16) -> f32 {
    let sign = u32::from(bits >> 15) << 31;
    let exponent = i32::from((bits >> 10) & 0x1f);
    let fraction = u32::from(bits & 0x3ff);
    let out = match exponent {
        0 if fraction == 0 => sign,
        0 => {
            // Subnormal half: normalize.
            let mut e = -14i32;
            let mut f = fraction;
            while f & 0x400 == 0 {
                f <<= 1;
                e -= 1;
            }
            f &= 0x3ff;
            sign | (((e + 127) as u32) << 23) | (f << 13)
        }
        31 => sign | 0x7f80_0000 | (fraction << 13),
        e => sign | (((e - 15 + 127) as u32) << 23) | (fraction << 13),
    };
    f32::from_bits(out)
}

pub(crate) fn decode_f16_base64(text: &str) -> Result<Vec<f32>, EngineError> {
    use base64::Engine as _;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(text)
        .map_err(|err| EngineError::PocketData(format!("voice data is not base64: {err}")))?;
    if bytes.len() % 2 != 0 {
        return Err(EngineError::PocketData(
            "voice data has an odd byte count".to_string(),
        ));
    }
    Ok(bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| f16_to_f32(u16::from_le_bytes([pair[0], pair[1]])))
        .collect())
}

/// The one ONNX Runtime environment for the process, loaded from the
/// `libonnxruntime` sherpa-onnx ships next to the executable.
pub(crate) fn ort_environment() -> Result<&'static ort::environment::Environment, EngineError> {
    static ENV: OnceLock<Result<ort::environment::Environment, String>> = OnceLock::new();
    ENV.get_or_init(|| {
        let candidates = ort_dylib_candidates();
        let mut last_error = String::new();
        for candidate in &candidates {
            match ort::init_from(candidate) {
                Ok(builder) => match builder.build() {
                    Ok(env) => return Ok(env),
                    Err(err) => last_error = format!("{}: {err}", candidate.display()),
                },
                Err(err) => last_error = format!("{}: {err}", candidate.display()),
            }
        }
        Err(format!(
            "no usable ONNX Runtime library (tried {}): {last_error}",
            candidates
                .iter()
                .map(|p| p.display().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        ))
    })
    .as_ref()
    .map_err(|message| EngineError::PocketRuntime(message.clone()))
}

fn ort_dylib_candidates() -> Vec<PathBuf> {
    let file = if cfg!(target_os = "windows") {
        "onnxruntime.dll"
    } else if cfg!(target_os = "macos") {
        "libonnxruntime.dylib"
    } else {
        "libonnxruntime.so"
    };
    let mut candidates = Vec::new();
    if let Ok(path) = std::env::var("ORT_DYLIB_PATH")
        && !path.is_empty()
    {
        candidates.push(PathBuf::from(path));
    }
    if let Ok(exe) = std::env::current_exe()
        && let Some(dir) = exe.parent()
    {
        candidates.push(dir.join(file));
        candidates.push(dir.join("lib").join(file));
    }
    candidates.push(PathBuf::from(file));
    candidates
}

pub(crate) fn thread_count() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1)
        .clamp(1, 8)
}

pub(crate) fn tensor_f32(
    shape: Vec<usize>,
    data: Vec<f32>,
) -> Result<ort::value::Tensor<f32>, EngineError> {
    ort::value::Tensor::from_array((shape, data.into_boxed_slice()))
        .map_err(|err| EngineError::PocketData(format!("couldn't build a model input: {err}")))
}

pub(crate) fn tensor_i64(
    shape: Vec<usize>,
    data: Vec<i64>,
) -> Result<ort::value::Tensor<i64>, EngineError> {
    ort::value::Tensor::from_array((shape, data.into_boxed_slice()))
        .map_err(|err| EngineError::PocketData(format!("couldn't build a model input: {err}")))
}

pub(crate) fn extract_f32(
    outputs: &ort::session::SessionOutputs<'_>,
    name: &str,
) -> Result<Vec<f32>, EngineError> {
    let value = outputs
        .get(name)
        .ok_or_else(|| EngineError::PocketData(format!("model gave no '{name}' output")))?;
    let (_, data) = value
        .try_extract_tensor::<f32>()
        .map_err(|err| EngineError::PocketData(format!("model output '{name}': {err}")))?;
    Ok(data.to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn f16_decoding_covers_zero_one_and_subnormals() {
        assert_eq!(f16_to_f32(0x0000), 0.0);
        assert_eq!(f16_to_f32(0x3c00), 1.0);
        assert_eq!(f16_to_f32(0xbc00), -1.0);
        assert!((f16_to_f32(0x0001) - 5.960_464_5e-8).abs() < 1e-12);
        assert!(f16_to_f32(0x7e00).is_nan());
    }
}
