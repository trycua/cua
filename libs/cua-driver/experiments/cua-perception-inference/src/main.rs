use std::{env, path::PathBuf, time::Instant};

use anyhow::{Context, Result};
use image::ImageReader;
use tract_onnx::prelude::*;

fn main() -> Result<()> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let image_path = env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| root.join("fixtures/screenshot.png"));
    let model_path = env::args()
        .nth(2)
        .map(PathBuf::from)
        .unwrap_or_else(|| root.join("fixtures/identity.onnx"));

    let decode_started = Instant::now();
    let image = ImageReader::open(&image_path)
        .with_context(|| format!("open PNG {}", image_path.display()))?
        .with_guessed_format()?
        .decode()
        .with_context(|| format!("decode PNG {}", image_path.display()))?;
    let decode_elapsed = decode_started.elapsed();

    let load_started = Instant::now();
    let model = tract_onnx::onnx()
        .model_for_path(&model_path)
        .with_context(|| format!("load ONNX {}", model_path.display()))?
        .into_optimized()?
        .into_runnable()?;
    let load_elapsed = load_started.elapsed();

    let input = Tensor::from_shape(
        &[1, 3, 2, 2],
        &[0_f32, 1., 2., 3., 4., 5., 6., 7., 8., 9., 10., 11.],
    )?;
    let run_started = Instant::now();
    let outputs = model.run(tvec!(input.into()))?;
    let run_elapsed = run_started.elapsed();
    let output = outputs[0].to_plain_array_view::<f32>()?;

    anyhow::ensure!(
        output.iter().copied().eq((0..12).map(|value| value as f32)),
        "identity fixture produced unexpected output"
    );

    println!(
        "decoded={}x{} decode_us={} model_load_us={} inference_us={} output_shape={:?}",
        image.width(),
        image.height(),
        decode_elapsed.as_micros(),
        load_elapsed.as_micros(),
        run_elapsed.as_micros(),
        output.shape()
    );
    Ok(())
}
