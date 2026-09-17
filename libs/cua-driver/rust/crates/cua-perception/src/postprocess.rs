use std::collections::VecDeque;

use crate::{manifest::DetectorOutputLayout, preprocess::Letterbox};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub x1: f32,
    pub y1: f32,
    pub x2: f32,
    pub y2: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Detection {
    pub bounds: Rect,
    pub score: f32,
    pub class_id: usize,
}

#[derive(Debug, Clone, Copy)]
pub struct OcrPostprocess {
    pub pixel_threshold: f32,
    pub box_threshold: f32,
    pub unclip_ratio: f32,
    pub minimum_area: u32,
    pub max_candidates: usize,
}

pub fn decode_detector(
    data: &[f32],
    shape: &[i64],
    layout: DetectorOutputLayout,
    transform: Letterbox,
    confidence_threshold: f32,
    iou_threshold: f32,
) -> Result<Vec<Detection>, String> {
    let mut candidates = match layout {
        DetectorOutputLayout::YoloV8CxcywhClassScores => {
            decode_yolo_v8(data, shape, transform, confidence_threshold)?
        }
        DetectorOutputLayout::XyxyScoreClass => {
            decode_xyxy(data, shape, transform, confidence_threshold)?
        }
    };
    candidates.sort_by(|left, right| right.score.total_cmp(&left.score));
    let mut kept: Vec<Detection> = Vec::new();
    for candidate in candidates {
        if kept
            .iter()
            .all(|existing| iou(candidate.bounds, existing.bounds) <= iou_threshold)
        {
            kept.push(candidate);
        }
    }
    Ok(kept)
}

fn decode_yolo_v8(
    data: &[f32],
    shape: &[i64],
    transform: Letterbox,
    threshold: f32,
) -> Result<Vec<Detection>, String> {
    let dims = squeeze_batch(shape)?;
    if dims.len() != 2 {
        return Err(format!(
            "YOLO detector output must be rank 2 or 3, got {shape:?}"
        ));
    }
    let (features, boxes, feature_major) = if dims[0] >= 5 && (dims[1] < 5 || dims[0] < dims[1]) {
        (dims[0], dims[1], true)
    } else if dims[1] >= 5 {
        (dims[1], dims[0], false)
    } else {
        return Err(format!(
            "YOLO detector output has no class scores: {shape:?}"
        ));
    };
    if data.len() != features * boxes {
        return Err("detector tensor shape does not match its data length".to_owned());
    }
    let at = |box_index: usize, feature: usize| {
        if feature_major {
            data[feature * boxes + box_index]
        } else {
            data[box_index * features + feature]
        }
    };
    let mut detections = Vec::new();
    for box_index in 0..boxes {
        let (class_id, score) = (4..features)
            .map(|feature| (feature - 4, at(box_index, feature)))
            .max_by(|left, right| left.1.total_cmp(&right.1))
            .unwrap();
        if !score.is_finite() || score < threshold {
            continue;
        }
        let cx = at(box_index, 0);
        let cy = at(box_index, 1);
        let width = at(box_index, 2);
        let height = at(box_index, 3);
        let bounds = restore(
            Rect {
                x1: cx - width / 2.0,
                y1: cy - height / 2.0,
                x2: cx + width / 2.0,
                y2: cy + height / 2.0,
            },
            transform,
        );
        if valid(bounds) {
            detections.push(Detection {
                bounds,
                score,
                class_id,
            });
        }
    }
    Ok(detections)
}

fn decode_xyxy(
    data: &[f32],
    shape: &[i64],
    transform: Letterbox,
    threshold: f32,
) -> Result<Vec<Detection>, String> {
    let dims = squeeze_batch(shape)?;
    if dims.len() != 2 || dims[1] != 6 || data.len() != dims[0] * 6 {
        return Err(format!(
            "xyxy detector output must have shape [N,6], got {shape:?}"
        ));
    }
    let mut detections = Vec::new();
    for row in data.chunks_exact(6) {
        if row[4].is_finite() && row[4] >= threshold {
            let bounds = restore(
                Rect {
                    x1: row[0],
                    y1: row[1],
                    x2: row[2],
                    y2: row[3],
                },
                transform,
            );
            if valid(bounds) && row[5].is_finite() && row[5] >= 0.0 {
                detections.push(Detection {
                    bounds,
                    score: row[4],
                    class_id: row[5] as usize,
                });
            }
        }
    }
    Ok(detections)
}

pub fn decode_ocr_probability_map(
    data: &[f32],
    shape: &[i64],
    transform: Letterbox,
    config: OcrPostprocess,
) -> Result<Vec<Detection>, String> {
    let (height, width) = probability_map_dimensions(shape)?;
    if data.len() != width * height {
        return Err("OCR detector tensor shape does not match its data length".to_owned());
    }
    let mut visited = vec![false; data.len()];
    let mut regions = Vec::new();
    for start in 0..data.len() {
        if visited[start] || !data[start].is_finite() || data[start] < config.pixel_threshold {
            continue;
        }
        visited[start] = true;
        let mut queue = VecDeque::from([start]);
        let mut min_x = width;
        let mut min_y = height;
        let mut max_x = 0;
        let mut max_y = 0;
        let mut area = 0_u32;
        let mut confidence = 0.0_f32;
        while let Some(index) = queue.pop_front() {
            let x = index % width;
            let y = index / width;
            min_x = min_x.min(x);
            min_y = min_y.min(y);
            max_x = max_x.max(x);
            max_y = max_y.max(y);
            area += 1;
            confidence += data[index];
            for neighbor in neighbors(x, y, width, height) {
                if !visited[neighbor]
                    && data[neighbor].is_finite()
                    && data[neighbor] >= config.pixel_threshold
                {
                    visited[neighbor] = true;
                    queue.push_back(neighbor);
                }
            }
        }
        let confidence = confidence / area as f32;
        if area < config.minimum_area || confidence < config.box_threshold {
            continue;
        }
        let box_width = max_x - min_x + 1;
        let box_height = max_y - min_y + 1;
        let perimeter = 2.0 * (box_width + box_height) as f32;
        let expansion =
            ((box_width * box_height) as f32 * config.unclip_ratio / perimeter).ceil() as usize;
        min_x = min_x.saturating_sub(expansion);
        min_y = min_y.saturating_sub(expansion);
        max_x = (max_x + expansion).min(width - 1);
        max_y = (max_y + expansion).min(height - 1);
        let scale_x = transform.input_width as f32 / width as f32;
        let scale_y = transform.input_height as f32 / height as f32;
        let bounds = restore(
            Rect {
                x1: min_x as f32 * scale_x,
                y1: min_y as f32 * scale_y,
                x2: (max_x + 1) as f32 * scale_x,
                y2: (max_y + 1) as f32 * scale_y,
            },
            transform,
        );
        if valid(bounds) {
            regions.push(Detection {
                bounds,
                score: confidence,
                class_id: 0,
            });
        }
    }
    regions.sort_by(|left, right| {
        left.bounds
            .y1
            .total_cmp(&right.bounds.y1)
            .then(left.bounds.x1.total_cmp(&right.bounds.x1))
    });
    regions.truncate(config.max_candidates);
    Ok(regions)
}

pub fn ctc_decode(
    data: &[f32],
    shape: &[i64],
    dictionary: &[String],
    blank_index: usize,
) -> Result<(String, f32), String> {
    let dims = squeeze_batch(shape)?;
    if dims.len() != 2 {
        return Err(format!(
            "OCR recognizer output must have shape [1,T,C], got {shape:?}"
        ));
    }
    let (steps, classes) = (dims[0], dims[1]);
    if data.len() != steps * classes || blank_index >= classes {
        return Err("OCR recognizer tensor metadata is inconsistent".to_owned());
    }
    let mut previous = blank_index;
    let mut text = String::new();
    let mut confidence = 0.0;
    let mut emitted = 0_u32;
    for logits in data.chunks_exact(classes) {
        let (index, score) = logits
            .iter()
            .copied()
            .enumerate()
            .max_by(|left, right| left.1.total_cmp(&right.1))
            .unwrap();
        if index != blank_index && index != previous {
            let dictionary_index = if index > blank_index {
                index - 1
            } else {
                index
            };
            let symbol = dictionary.get(dictionary_index).ok_or_else(|| {
                format!("OCR class {index} is missing from the recognition dictionary")
            })?;
            text.push_str(symbol);
            confidence += score;
            emitted += 1;
        }
        previous = index;
    }
    Ok((
        text,
        if emitted == 0 {
            0.0
        } else {
            confidence / emitted as f32
        },
    ))
}

fn squeeze_batch(shape: &[i64]) -> Result<Vec<usize>, String> {
    let mut dims = shape
        .iter()
        .map(|dimension| {
            usize::try_from(*dimension).map_err(|_| "negative tensor dimension".to_owned())
        })
        .collect::<Result<Vec<_>, _>>()?;
    if dims.first() == Some(&1) {
        dims.remove(0);
    }
    Ok(dims)
}

fn probability_map_dimensions(shape: &[i64]) -> Result<(usize, usize), String> {
    let dims = squeeze_batch(shape)?;
    match dims.as_slice() {
        [1, height, width] | [height, width] => Ok((*height, *width)),
        _ => Err(format!(
            "OCR probability map must have shape [1,1,H,W], got {shape:?}"
        )),
    }
}

fn restore(bounds: Rect, transform: Letterbox) -> Rect {
    let x = |value: f32| {
        ((value - transform.pad_x as f32) / transform.scale_x)
            .clamp(0.0, transform.original_width as f32)
    };
    let y = |value: f32| {
        ((value - transform.pad_y as f32) / transform.scale_y)
            .clamp(0.0, transform.original_height as f32)
    };
    Rect {
        x1: x(bounds.x1),
        y1: y(bounds.y1),
        x2: x(bounds.x2),
        y2: y(bounds.y2),
    }
}

fn valid(bounds: Rect) -> bool {
    bounds.x1.is_finite()
        && bounds.y1.is_finite()
        && bounds.x2.is_finite()
        && bounds.y2.is_finite()
        && bounds.x2 > bounds.x1
        && bounds.y2 > bounds.y1
}

fn iou(left: Rect, right: Rect) -> f32 {
    let intersection_width = (left.x2.min(right.x2) - left.x1.max(right.x1)).max(0.0);
    let intersection_height = (left.y2.min(right.y2) - left.y1.max(right.y1)).max(0.0);
    let intersection = intersection_width * intersection_height;
    let left_area = (left.x2 - left.x1) * (left.y2 - left.y1);
    let right_area = (right.x2 - right.x1) * (right.y2 - right.y1);
    intersection / (left_area + right_area - intersection).max(f32::EPSILON)
}

fn neighbors(x: usize, y: usize, width: usize, height: usize) -> impl Iterator<Item = usize> {
    let mut values = [None; 4];
    if x > 0 {
        values[0] = Some(y * width + x - 1);
    }
    if x + 1 < width {
        values[1] = Some(y * width + x + 1);
    }
    if y > 0 {
        values[2] = Some((y - 1) * width + x);
    }
    if y + 1 < height {
        values[3] = Some((y + 1) * width + x);
    }
    values.into_iter().flatten()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn transform() -> Letterbox {
        Letterbox {
            original_width: 100,
            original_height: 50,
            input_width: 200,
            input_height: 200,
            resized_width: 200,
            resized_height: 100,
            pad_x: 0,
            pad_y: 50,
            scale_x: 2.0,
            scale_y: 2.0,
        }
    }

    #[test]
    fn yolo_decode_restores_coordinates_and_applies_nms() {
        let tensor = [
            100.0, 102.0, 25.0, // cx
            100.0, 102.0, 75.0, // cy
            40.0, 40.0, 10.0, // width
            20.0, 20.0, 10.0, // height
            0.9, 0.8, 0.7, // class 0
        ];
        let detections = decode_detector(
            &tensor,
            &[1, 5, 3],
            DetectorOutputLayout::YoloV8CxcywhClassScores,
            transform(),
            0.5,
            0.5,
        )
        .unwrap();
        assert_eq!(detections.len(), 2);
        assert_eq!(
            detections[0].bounds,
            Rect {
                x1: 40.0,
                y1: 20.0,
                x2: 60.0,
                y2: 30.0
            }
        );
        assert_eq!(
            detections[1].bounds,
            Rect {
                x1: 10.0,
                y1: 10.0,
                x2: 15.0,
                y2: 15.0
            }
        );
    }

    #[test]
    fn probability_map_groups_connected_text_pixels() {
        let map = [0.9, 0.8, 0.0, 0.0, 0.9, 0.0, 0.0, 0.7];
        let regions = decode_ocr_probability_map(
            &map,
            &[1, 1, 2, 4],
            transform(),
            OcrPostprocess {
                pixel_threshold: 0.5,
                box_threshold: 0.8,
                unclip_ratio: 1.5,
                minimum_area: 2,
                max_candidates: 100,
            },
        )
        .unwrap();
        assert_eq!(regions.len(), 1);
        assert!((regions[0].score - 0.8666667).abs() < 1e-5);
    }

    #[test]
    fn ctc_decode_collapses_repeats_and_blank() {
        let logits = [
            0.1, 0.9, 0.0, // a
            0.1, 0.8, 0.1, // repeated a
            0.9, 0.1, 0.0, // blank
            0.0, 0.1, 0.9, // b
        ];
        let (text, confidence) =
            ctc_decode(&logits, &[1, 4, 3], &["a".into(), "b".into()], 0).unwrap();
        assert_eq!(text, "ab");
        assert!((confidence - 0.9).abs() < 1e-6);
    }
}
