use image::{imageops::FilterType, Rgb, RgbImage};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Letterbox {
    pub original_width: u32,
    pub original_height: u32,
    pub input_width: u32,
    pub input_height: u32,
    pub resized_width: u32,
    pub resized_height: u32,
    pub pad_x: u32,
    pub pad_y: u32,
    pub scale_x: f32,
    pub scale_y: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ImageTensor {
    pub shape: [usize; 4],
    pub data: Vec<f32>,
    pub letterbox: Letterbox,
}

pub fn detector_tensor(image: &RgbImage, width: u32, height: u32) -> ImageTensor {
    let (canvas, letterbox) = letterbox(image, width, height, Rgb([114, 114, 114]));
    ImageTensor {
        shape: [1, 3, height as usize, width as usize],
        data: rgb_chw(&canvas, [0.0; 3], [1.0; 3], 1.0 / 255.0),
        letterbox,
    }
}

pub fn ocr_detector_tensor(image: &RgbImage, resize_long: u32) -> ImageTensor {
    let scale = resize_long as f32 / image.width().max(image.height()) as f32;
    let width = round_to_multiple((image.width() as f32 * scale).round() as u32, 128);
    let height = round_to_multiple((image.height() as f32 * scale).round() as u32, 128);
    let resized = image::imageops::resize(image, width, height, FilterType::Triangle);
    let letterbox = Letterbox {
        original_width: image.width(),
        original_height: image.height(),
        input_width: width,
        input_height: height,
        resized_width: width,
        resized_height: height,
        pad_x: 0,
        pad_y: 0,
        scale_x: width as f32 / image.width() as f32,
        scale_y: height as f32 / image.height() as f32,
    };
    ImageTensor {
        shape: [1, 3, height as usize, width as usize],
        data: bgr_chw(
            &resized,
            [0.485, 0.456, 0.406],
            [0.229, 0.224, 0.225],
            1.0 / 255.0,
        ),
        letterbox,
    }
}

pub fn ocr_recognizer_tensor(image: &RgbImage, width: u32, height: u32) -> ImageTensor {
    let scale = (height as f32 / image.height() as f32).min(width as f32 / image.width() as f32);
    let resized_width = ((image.width() as f32 * scale).round() as u32).clamp(1, width);
    let resized = image::imageops::resize(image, resized_width, height, FilterType::Triangle);
    // PP-OCR normalization maps 127.5 to zero. Mid-gray padding therefore
    // represents neutral input rather than solid black content.
    let mut canvas = RgbImage::from_pixel(width, height, Rgb([128, 128, 128]));
    image::imageops::replace(&mut canvas, &resized, 0, 0);
    let letterbox = Letterbox {
        original_width: image.width(),
        original_height: image.height(),
        input_width: width,
        input_height: height,
        resized_width,
        resized_height: height,
        pad_x: 0,
        pad_y: 0,
        scale_x: scale,
        scale_y: height as f32 / image.height() as f32,
    };
    ImageTensor {
        shape: [1, 3, height as usize, width as usize],
        data: bgr_chw(&canvas, [0.5; 3], [0.5; 3], 1.0 / 255.0),
        letterbox,
    }
}

fn letterbox(
    image: &RgbImage,
    input_width: u32,
    input_height: u32,
    fill: Rgb<u8>,
) -> (RgbImage, Letterbox) {
    let (original_width, original_height) = image.dimensions();
    let scale = (input_width as f32 / original_width as f32)
        .min(input_height as f32 / original_height as f32);
    let resized_width = ((original_width as f32 * scale).round() as u32).clamp(1, input_width);
    let resized_height = ((original_height as f32 * scale).round() as u32).clamp(1, input_height);
    let pad_x = (input_width - resized_width) / 2;
    let pad_y = (input_height - resized_height) / 2;
    let resized =
        image::imageops::resize(image, resized_width, resized_height, FilterType::Triangle);
    let mut canvas = RgbImage::from_pixel(input_width, input_height, fill);
    image::imageops::replace(&mut canvas, &resized, i64::from(pad_x), i64::from(pad_y));
    (
        canvas,
        Letterbox {
            original_width,
            original_height,
            input_width,
            input_height,
            resized_width,
            resized_height,
            pad_x,
            pad_y,
            scale_x: resized_width as f32 / original_width as f32,
            scale_y: resized_height as f32 / original_height as f32,
        },
    )
}

fn round_to_multiple(value: u32, multiple: u32) -> u32 {
    ((value.max(multiple) + multiple / 2) / multiple * multiple).max(multiple)
}

fn rgb_chw(image: &RgbImage, mean: [f32; 3], std: [f32; 3], scale: f32) -> Vec<f32> {
    channels_chw(image, mean, std, scale, [0, 1, 2])
}

fn bgr_chw(image: &RgbImage, mean: [f32; 3], std: [f32; 3], scale: f32) -> Vec<f32> {
    channels_chw(image, mean, std, scale, [2, 1, 0])
}

fn channels_chw(
    image: &RgbImage,
    mean: [f32; 3],
    std: [f32; 3],
    scale: f32,
    order: [usize; 3],
) -> Vec<f32> {
    let plane = image.width() as usize * image.height() as usize;
    let mut data = vec![0.0; plane * 3];
    for (index, pixel) in image.pixels().enumerate() {
        for channel in 0..3 {
            data[channel * plane + index] =
                (f32::from(pixel[order[channel]]) * scale - mean[channel]) / std[channel];
        }
    }
    data
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detector_preprocessing_is_chw_and_letterboxed() {
        let image = RgbImage::from_fn(2, 1, |x, _| {
            if x == 0 {
                Rgb([255, 0, 0])
            } else {
                Rgb([0, 255, 0])
            }
        });
        let tensor = detector_tensor(&image, 4, 4);
        assert_eq!(tensor.shape, [1, 3, 4, 4]);
        assert_eq!(tensor.letterbox.resized_width, 4);
        assert_eq!(tensor.letterbox.resized_height, 2);
        assert_eq!(tensor.letterbox.pad_y, 1);
        assert_eq!(tensor.data.len(), 48);
        assert!((tensor.data[4] - 1.0).abs() < 1e-6);
        assert!((tensor.data[16..32].iter().copied().fold(0.0, f32::max) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn recognizer_padding_is_deterministic() {
        let image = RgbImage::from_pixel(2, 2, Rgb([255, 255, 255]));
        let tensor = ocr_recognizer_tensor(&image, 8, 2);
        assert_eq!(tensor.letterbox.resized_width, 2);
        assert_eq!(tensor.data[0], 1.0);
        assert!((tensor.data[2] - (128.0 / 255.0 - 0.5) / 0.5).abs() < 1e-6);
    }

    #[test]
    fn ocr_detector_uses_bgr_and_resize_long() {
        let image = RgbImage::from_pixel(64, 32, Rgb([255, 0, 0]));
        let tensor = ocr_detector_tensor(&image, 96);
        assert_eq!(tensor.shape, [1, 3, 128, 128]);
        assert!(tensor.data[0] < 0.0, "B channel must receive source blue");
        assert!(
            tensor.data[2 * 128 * 128] > 1.0,
            "R channel must receive source red"
        );
    }
}
