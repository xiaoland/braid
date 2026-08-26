use std::path::Path;

use anyhow::{Context as _, Result, bail};
use image::{ImageFormat, Rgba, RgbaImage, imageops::FilterType};
use serde::Deserialize;

use super::gh_api_json;

const AVATAR_SIZE: u32 = 512;
const BADGE_SIZE: u32 = 112;
const LOGO_SIZE: u32 = 80;
const PADDING: u32 = 24;
const BACKGROUND_BRIGHTNESS_THRESHOLD: u32 = 50;

#[derive(Debug, Deserialize)]
struct OwnerInfo {
    avatar_url: String,
}

pub fn generate(owner: &str, output: &Path) -> Result<()> {
    let avatar_url = fetch_avatar_url(owner)?;
    let avatar_bytes = download(&avatar_url).context("cannot download owner avatar")?;
    let mut avatar =
        image::load_from_memory(&avatar_bytes).context("cannot decode owner avatar")?.to_rgba8();
    avatar = image::imageops::resize(&avatar, AVATAR_SIZE, AVATAR_SIZE, FilterType::Lanczos3);

    let braid_logo =
        image::load_from_memory(include_bytes!("../../docs/assets/braid-logo-128.png"))
            .context("cannot decode embedded Braid logo")?;
    let logo = image::imageops::resize(&braid_logo, LOGO_SIZE, LOGO_SIZE, FilterType::Lanczos3);

    let mut badge = RgbaImage::from_pixel(BADGE_SIZE, BADGE_SIZE, Rgba([255, 255, 255, 0]));
    draw_circle(
        &mut badge,
        BADGE_SIZE / 2,
        BADGE_SIZE / 2,
        BADGE_SIZE / 2,
        Rgba([255, 255, 255, 255]),
    );

    let mut logo_layer = RgbaImage::from_pixel(BADGE_SIZE, BADGE_SIZE, Rgba([0, 0, 0, 0]));
    let offset = (BADGE_SIZE - LOGO_SIZE) / 2;
    for y in 0..LOGO_SIZE {
        for x in 0..LOGO_SIZE {
            let pixel = logo.get_pixel(x, y);
            let brightness = (u32::from(pixel[0]) + u32::from(pixel[1]) + u32::from(pixel[2])) / 3;
            // The Braid logo has a dark background and bright loops. Make the
            // dark background transparent so it floats over the white badge.
            let alpha = if brightness < BACKGROUND_BRIGHTNESS_THRESHOLD { 0 } else { 255 };
            logo_layer.put_pixel(
                offset + x,
                offset + y,
                Rgba([pixel[0], pixel[1], pixel[2], alpha]),
            );
        }
    }

    let x = avatar.width() - BADGE_SIZE - PADDING;
    let y = avatar.height() - BADGE_SIZE - PADDING;
    image::imageops::overlay(&mut avatar, &badge, i64::from(x), i64::from(y));
    image::imageops::overlay(&mut avatar, &logo_layer, i64::from(x), i64::from(y));

    avatar.save_with_format(output, ImageFormat::Png).context("cannot save composite logo")?;
    Ok(())
}

fn fetch_avatar_url(owner: &str) -> Result<String> {
    let user = gh_api_json::<OwnerInfo>(&format!("users/{owner}"), None);
    let org = gh_api_json::<OwnerInfo>(&format!("orgs/{owner}"), None);
    match (user, org) {
        (Ok(info), _) | (_, Ok(info)) => Ok(info.avatar_url),
        (Err(user_err), Err(_)) => bail!("cannot look up owner {owner}: {user_err}"),
    }
}

fn download(url: &str) -> Result<Vec<u8>> {
    let response = reqwest::blocking::get(url).context("cannot fetch avatar URL")?;
    if !response.status().is_success() {
        bail!("avatar download returned HTTP {}", response.status());
    }
    response.bytes().context("cannot read avatar bytes").map(|b| b.to_vec())
}

fn draw_circle(image: &mut RgbaImage, cx: u32, cy: u32, radius: u32, color: Rgba<u8>) {
    let (width, height) = (image.width(), image.height());
    let radius_squared = i64::from(radius) * i64::from(radius);
    for y in 0..height {
        for x in 0..width {
            let dx = i64::from(x) - i64::from(cx);
            let dy = i64::from(y) - i64::from(cy);
            if dx * dx + dy * dy <= radius_squared {
                image.put_pixel(x, y, color);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    #[test]
    #[ignore = "requires network access to GitHub avatar"]
    fn generate_logo_for_xiaoland() {
        let path = PathBuf::from("/tmp/braid-of-xiaoland-logo-test.png");
        let _ = std::fs::remove_file(&path);
        generate("xiaoland", &path).expect("should generate logo");
        assert!(path.is_file());
    }
}
