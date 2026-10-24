//! Pictures for the prompt writers: reading a picture from a request and laying several pictures on one sheet for the
//! vision model. Port of `_image_from_prompt_payload`, `_combine_flux_ingredient_images`, `_combine_story_reference_batch` and
//! `_combine_subject_location_images` in `builder/media.py`.

use std::path::Path;

use base64::Engine;
use fancy_regex::Regex;
use image::imageops::FilterType;
use image::{DynamicImage, Rgb, RgbImage};

use crate::files::resolve_existing_file;

/// `_image_from_data_url`: a `data:image/png|jpeg|jpg|webp;base64,...` URL.
pub fn image_from_data_url(data_url: &str) -> Result<DynamicImage, String> {
    let text = data_url.trim();
    let pattern = Regex::new(r"(?is)^data:image/(?:png|jpeg|jpg|webp);base64,(.+)$").expect("valid pattern");
    let Some(found) = pattern.captures(text).ok().flatten().and_then(|c| c.get(1)) else {
        return Err("Expected a base64 image data URL.".into());
    };
    let raw = base64::engine::general_purpose::STANDARD.decode(found.as_str().trim()).map_err(|e| format!("The picture data could not be decoded: {e}"))?;
    image::load_from_memory(&raw).map_err(|_| "cannot identify image file".to_string()).map(|i| DynamicImage::ImageRgb8(i.to_rgb8()))
}

/// `_image_from_prompt_payload`: the picture of `data` when it is there, else the file at `path`.
pub fn image_from_payload(path: &str, data: &str, label: &str) -> Result<DynamicImage, String> {
    let (data, path) = (data.trim(), path.trim().trim_matches('"'));
    if !data.is_empty() {
        return image_from_data_url(data);
    }
    if !path.is_empty() {
        let file = resolve_existing_file(path, label)?;
        return open_rgb(&file);
    }
    Err(format!("{label} is required."))
}

/// A picture file as RGB.
pub fn open_rgb(path: &Path) -> Result<DynamicImage, String> {
    image::open(path).map(|i| DynamicImage::ImageRgb8(i.to_rgb8())).map_err(|e| format!("Could not read the picture {}: {e}", path.display()))
}

fn sheet(width: u32, height: u32) -> RgbImage {
    RgbImage::from_pixel(width, height, Rgb([20, 20, 20]))
}

fn shrink(image: &RgbImage, width: u32, height: u32) -> RgbImage {
    image::imageops::resize(image, width.max(1), height.max(1), FilterType::Lanczos3)
}

fn paste(canvas: &mut RgbImage, image: &RgbImage, x: u32, y: u32) {
    image::imageops::replace(canvas, image, i64::from(x), i64::from(y));
}

/// `_combine_subject_location_images`: a subject and a location side by side, each at most 640 high.
pub fn combine_subject_location(subject: &DynamicImage, location: &DynamicImage) -> DynamicImage {
    let max_height = 640u32;
    let resized: Vec<RgbImage> = [subject, location]
        .iter()
        .map(|image| {
            let image = image.to_rgb8();
            let scale = (f64::from(max_height) / f64::from(image.height().max(1))).min(1.0);
            let width = ((f64::from(image.width()) * scale) as u32).max(1);
            let height = ((f64::from(image.height()) * scale) as u32).max(1);
            shrink(&image, width, height)
        })
        .collect();
    let gap = 24;
    let width = resized[0].width() + resized[1].width() + gap;
    let height = resized[0].height().max(resized[1].height());
    let mut canvas = sheet(width, height);
    paste(&mut canvas, &resized[0], 0, (height - resized[0].height()) / 2);
    paste(&mut canvas, &resized[1], resized[0].width() + gap, (height - resized[1].height()) / 2);
    DynamicImage::ImageRgb8(canvas)
}

/// `_combine_flux_ingredient_images`: the reference pictures in a grid of 384 pixel cells (256 above four pictures).
pub fn combine_flux_ingredients(images: &[DynamicImage]) -> Result<DynamicImage, String> {
    if images.is_empty() {
        return Err("At least one image ingredient is required.".into());
    }
    let cell = if images.len() <= 4 { 384u32 } else { 256 };
    let gap = 24u32;
    let columns = if images.len() == 1 { 1 } else { (images.len() as f64).sqrt().ceil() as u32 };
    let rows = (images.len() as f64 / f64::from(columns)).ceil() as u32;
    let mut canvas = sheet(columns * cell + gap * (columns - 1), rows * cell + gap * (rows - 1));
    for (index, image) in images.iter().enumerate() {
        let image = image.to_rgb8();
        let scale = (f64::from(cell) / f64::from(image.width().max(1))).min(f64::from(cell) / f64::from(image.height().max(1))).min(1.0);
        let resized = shrink(&image, ((f64::from(image.width()) * scale) as u32).max(1), ((f64::from(image.height()) * scale) as u32).max(1));
        let (column, row) = (index as u32 % columns, index as u32 / columns);
        let x = column * (cell + gap) + (cell - resized.width()) / 2;
        let y = row * (cell + gap) + (cell - resized.height()) / 2;
        paste(&mut canvas, &resized, x, y);
    }
    Ok(DynamicImage::ImageRgb8(canvas))
}

/// `_combine_story_reference_batch`: up to four pictures in a grid of 512 pixel tiles (scaled to fit, up or down).
pub fn combine_story_batch(images: &[DynamicImage], cell: u32) -> Result<DynamicImage, String> {
    if images.is_empty() {
        return Err("At least one Story reference image is required.".into());
    }
    let batch = &images[..images.len().min(4)];
    let gap = 16u32;
    let columns = if batch.len() == 1 { 1 } else { 2 };
    let rows = (batch.len() as f64 / f64::from(columns)).ceil() as u32;
    let mut canvas = sheet(columns * cell + gap * (columns - 1), rows * cell + gap * (rows - 1));
    for (index, image) in batch.iter().enumerate() {
        let image = image.to_rgb8();
        let scale = (f64::from(cell) / f64::from(image.width().max(1))).min(f64::from(cell) / f64::from(image.height().max(1)));
        let (width, height) = (((f64::from(image.width()) * scale) as u32).max(1), ((f64::from(image.height()) * scale) as u32).max(1));
        let resized = shrink(&image, width, height);
        let (column, row) = (index as u32 % columns, index as u32 / columns);
        paste(&mut canvas, &resized, column * (cell + gap) + (cell - width) / 2, row * (cell + gap) + (cell - height) / 2);
    }
    Ok(DynamicImage::ImageRgb8(canvas))
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::GenericImageView;

    fn picture(width: u32, height: u32) -> DynamicImage {
        DynamicImage::ImageRgb8(RgbImage::from_pixel(width, height, Rgb([200, 40, 40])))
    }

    #[test]
    fn a_data_url_or_a_file_gives_the_picture() {
        use base64::Engine;
        let mut png = std::io::Cursor::new(Vec::new());
        picture(5, 4).write_to(&mut png, image::ImageFormat::Png).unwrap();
        let url = format!("data:image/png;base64,{}", base64::engine::general_purpose::STANDARD.encode(png.into_inner()));
        assert_eq!(image_from_payload("", &url, "x").unwrap().width(), 5);
        assert_eq!(image_from_data_url("data:text/plain;base64,aGk=").unwrap_err(), "Expected a base64 image data URL.");
        assert_eq!(image_from_payload("", "", "Image ingredient 2").unwrap_err(), "Image ingredient 2 is required.");
        assert!(image_from_payload("Z:/missing/a.png", "", "Image ingredient 1").unwrap_err().starts_with("Image ingredient 1 was not found: "));
    }

    #[test]
    fn sheets_have_the_packs_sizes() {
        assert_eq!(combine_flux_ingredients(&[picture(40, 30)]).unwrap().dimensions(), (384, 384));
        assert_eq!(combine_flux_ingredients(&[picture(40, 30), picture(40, 30), picture(40, 30)]).unwrap().dimensions(), (2 * 384 + 24, 2 * 384 + 24));
        assert_eq!(combine_flux_ingredients(&vec![picture(40, 30); 5]).unwrap().dimensions(), (3 * 256 + 48, 2 * 256 + 24));
        assert!(combine_flux_ingredients(&[]).is_err());
        assert_eq!(combine_story_batch(&[picture(40, 30)], 512).unwrap().dimensions(), (512, 512));
        assert_eq!(combine_story_batch(&vec![picture(40, 30); 6], 512).unwrap().dimensions(), (2 * 512 + 16, 2 * 512 + 16));
        assert_eq!(combine_subject_location(&picture(100, 1000), &picture(300, 300)).dimensions(), (64 + 300 + 24, 640));
    }
}
