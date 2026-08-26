use std::path::Path;

use anyhow::{Context as _, Result, bail};
use image::{ImageFormat, Rgba, RgbaImage, imageops::FilterType};
use serde::Deserialize;

use super::gh_api_json;

const CANVAS_SIZE: u32 = 512;
const BADGE_SIZE: u32 = 112;
const BORDER_WIDTH: u32 = 6;

#[derive(Debug, Deserialize)]
struct OwnerInfo {
    avatar_url: String,
}

/// Generate a GitHub App logo where the Braid logo is the main avatar content
/// and the owner avatar appears as a small circular badge at the bottom-right.
pub fn generate(owner: &str, output: &Path) -> Result<()> {
    let avatar_url = fetch_avatar_url(owner)?;
    let avatar_bytes = download(&avatar_url).context("cannot download owner avatar")?;
    let owner_avatar =
        image::load_from_memory(&avatar_bytes).context("cannot decode owner avatar")?.to_rgba8();

    let braid_logo =
        image::load_from_memory(include_bytes!("../../docs/assets/braid-logo-transparent.png"))
            .context("cannot decode embedded Braid logo")?;

    let mut canvas = RgbaImage::from_pixel(CANVAS_SIZE, CANVAS_SIZE, Rgba([0, 0, 0, 0]));

    // Braid logo is the main content; keep some padding inside the square.
    let braid_size = CANVAS_SIZE - 96;
    let braid = image::imageops::resize(&braid_logo, braid_size, braid_size, FilterType::Lanczos3);
    let braid_offset = (CANVAS_SIZE - braid_size) / 2;
    image::imageops::overlay(&mut canvas, &braid, i64::from(braid_offset), i64::from(braid_offset));

    // Owner avatar badge at bottom-right, with a white border.
    let badge_x = CANVAS_SIZE - BADGE_SIZE - 20;
    let badge_y = CANVAS_SIZE - BADGE_SIZE - 20;
    let mut badge = circular_avatar(&owner_avatar, BADGE_SIZE);
    // White border ring.
    draw_ring(
        &mut badge,
        BADGE_SIZE / 2,
        BADGE_SIZE / 2,
        BADGE_SIZE / 2,
        Rgba([255, 255, 255, 255]),
    );
    image::imageops::overlay(&mut canvas, &badge, i64::from(badge_x), i64::from(badge_y));

    canvas.save_with_format(output, ImageFormat::Png).context("cannot save composite logo")?;
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

fn circular_avatar(image: &RgbaImage, size: u32) -> RgbaImage {
    let resized = image::imageops::resize(image, size, size, FilterType::Lanczos3);
    let mut out = RgbaImage::from_pixel(size, size, Rgba([0, 0, 0, 0]));
    let radius_squared = i64::from(size / 2) * i64::from(size / 2);
    let cx = size / 2;
    let cy = size / 2;
    for y in 0..size {
        for x in 0..size {
            let dx = i64::from(x) - i64::from(cx);
            let dy = i64::from(y) - i64::from(cy);
            if dx * dx + dy * dy <= radius_squared {
                out.put_pixel(x, y, *resized.get_pixel(x, y));
            }
        }
    }
    out
}

fn draw_ring(image: &mut RgbaImage, cx: u32, cy: u32, radius: u32, color: Rgba<u8>) {
    let (width, height) = (image.width(), image.height());
    let outer = i64::from(radius);
    let inner = i64::from(radius.saturating_sub(BORDER_WIDTH));
    let outer_sq = outer * outer;
    let inner_sq = inner * inner;
    for y in 0..height {
        for x in 0..width {
            let dx = i64::from(x) - i64::from(cx);
            let dy = i64::from(y) - i64::from(cy);
            let dist_sq = dx * dx + dy * dy;
            if dist_sq <= outer_sq && dist_sq >= inner_sq {
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
