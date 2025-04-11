use std::fs::File;
use std::io::{BufReader};
use serde::Deserialize;
use image::{RgbaImage, Rgba};
use rusttype::{point, Font, Scale};
use imageproc::drawing::draw_filled_rect_mut;
use imageproc::rect::Rect;
use palette::{Srgb, Hsl, FromColor, Oklab};
use acap::euclid::Euclidean;
use acap::Proximity;
use clap::Parser;

const BLOCK_SIZE_X: u32 = 400;
const BLOCK_SIZE_Y: u32 = 300;

const COLUMNS: usize = 8;

#[derive(Parser)]
struct Cli {
    /// The path to the file for color entry
    input: std::path::PathBuf,

    // Output file name
    #[arg(short, long, default_value = "palette.png")]
    output: String
}

#[derive(Debug, Deserialize, Clone)]
struct ColorEntry {
    name: String,
    hex: String
}

fn luminance(hsl: Hsl) -> f32 {
    let rgb: Srgb<f32> = Srgb::from_color(hsl);
    0.299 * rgb.red + 0.587 * rgb.green + 0.114 * rgb.blue
}

pub fn pick_label_color(bg: Srgb<f32>) -> (u8,u8,u8) {
    let hsl_bg: Hsl = Hsl::from_color(bg);
    let mut hue = hsl_bg.hue.into_degrees();
    if hue < 0.0 {
        hue += 360.0;
    }
    let mut hsl_label = hsl_bg;
    hsl_label.saturation *= 0.5;
    let mut lightened = hsl_label;
    lightened.lightness = 0.775;
    let mut darkened = hsl_label;
    darkened.lightness = 0.28;
    let l_bg = luminance(hsl_bg);
    let visually_dark_bg = l_bg < 0.62 && hsl_bg.saturation * hsl_bg.lightness > 0.1;

    // Force light label for certain hues when background is dark
    let hue_prefers_light = (36.0..=80.0).contains(&hue)   // yellow, chartreuse
                         || (90.0..=185.0).contains(&hue) // greenish tones
                         || (300.0..=340.0).contains(&hue);  //purples
    let l_light = luminance(lightened);
    let l_dark = luminance(darkened);
    let used = if visually_dark_bg && hue_prefers_light {
        "light"
    } else if (l_light - l_bg).abs() > (l_dark - l_bg).abs() {
        "light"
    } else {
        "dark "
    };

    let chosen = if used == "light" {
        lightened
    } else {
        darkened
    };
    let adjusted: Srgb<u8> = Srgb::from_color(chosen).into_format();
    (adjusted.red, adjusted.green, adjusted.blue)
}

fn hex_to_rgb(hex: &str) -> (u8, u8, u8) {
    let hex = hex.trim_start_matches('#');
    let r = u8::from_str_radix(&hex[0..2], 16).unwrap();
    let g = u8::from_str_radix(&hex[2..4], 16).unwrap();
    let b = u8::from_str_radix(&hex[4..6], 16).unwrap();
    (r, g, b)
}

fn draw_centered_text(
    imgbuf: &mut RgbaImage,
    font: &Font,
    text: &str,
    block_rect: Rect,
    base_color: Rgba<u8>,
    text_color: Rgba<u8>,
    initial_scale: f32,
    vertical_offset: i32,
) {
    let (x0, y0) = (block_rect.left() as u32, block_rect.top() as u32);
    let (width, height) = (block_rect.width(), block_rect.height());

    let mut scale_factor = initial_scale;
    let mut text_scale;
    let mut v_metrics;
    let mut glyphs;
    let mut glyphs_width;
    let mut glyphs_height;

    loop {
        text_scale = Scale::uniform(width as f32 / scale_factor);
        v_metrics = font.v_metrics(text_scale);

        glyphs = font.layout(text, text_scale, point(0.0, v_metrics.ascent)).collect::<Vec<_>>();

        glyphs_height = (v_metrics.ascent - v_metrics.descent).ceil() as u32;

        glyphs_width = {
            let min_x = glyphs.first().and_then(|g| g.pixel_bounding_box()).map(|bb| bb.min.x).unwrap_or(0);
            let max_x = glyphs.last().and_then(|g| g.pixel_bounding_box()).map(|bb| bb.max.x).unwrap_or(0);
            (max_x - min_x).max(1) as u32
        };

        if glyphs_width < width - width / 20 {
            break;
        }

        scale_factor += 0.5;
    }

    let text_x = x0 + ((width - glyphs_width) / 2);
    let text_y = (y0 + ((height - glyphs_height) / 2)) as i32 + vertical_offset;

    let glyphs = font
        .layout(text, text_scale, point(text_x as f32, text_y as f32 + v_metrics.ascent))
        .collect::<Vec<_>>();

    for glyph in glyphs {
        if let Some(bb) = glyph.pixel_bounding_box() {
            glyph.draw(|x, y, v| {
                let alpha = v;
                let px = x + bb.min.x as u32;
                let py = y + bb.min.y as u32;
                let blended = [
                    ((1.0 - alpha) * base_color[0] as f32 + alpha * text_color[0] as f32) as u8,
                    ((1.0 - alpha) * base_color[1] as f32 + alpha * text_color[1] as f32) as u8,
                    ((1.0 - alpha) * base_color[2] as f32 + alpha * text_color[2] as f32) as u8,
                ];
                
                imgbuf.put_pixel(px, py, Rgba([blended[0], blended[1], blended[2], 255]));
            });
        }
    }
}

fn sort_colors(colors: Vec<ColorEntry>) -> Vec<ColorEntry>{
    let hsl_coords: Vec<Euclidean<[f32; 3]>> = colors.iter()
    .map(|color| {
        let (r, g, b) = hex_to_rgb(&color.hex);
        let oklab = Oklab::from_color(Srgb::new(r as f32, g as f32 , b as f32));
        Euclidean([
            oklab.l,
            oklab.a,
            oklab.b,
        ])
    })
    .collect();

    // Start with the first point
    let mut path = vec![0];
    let mut visited = vec![false; hsl_coords.len()];
    visited[0] = true;
    let mut current = &hsl_coords[0];

    for _ in 1..hsl_coords.len() {
        // Find next closest that hasn't been visited
        let mut nearest: Option<(usize, f32)> = None;
        for (i, point) in hsl_coords.iter().enumerate() {
            if visited[i] {
                continue;
            }
            let dist = current.distance(point);
            if nearest.is_none() || dist < nearest.unwrap().1 {
                nearest = Some((i, dist.into()));
            }
        }

        if let Some((next_index, _)) = nearest {
            visited[next_index] = true;
            path.push(next_index);
            current = &hsl_coords[next_index];
        }
    }

    // You can now reorder your colors list using the sorted path
    let mut sorted_colors: Vec<_> = path.into_iter().map(|i| colors[i].clone()).collect();
    sorted_colors.sort_by_key(|color| {
        let (r, g, b) = hex_to_rgb(&color.hex);
        let hsl = Hsl::from_color(Srgb::new(r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0));
        if hsl.saturation < 0.05 && hsl.lightness > 0.75{
            (1,(hsl.lightness*100.0) as i32) 
        } else {
            (0,0)
        }
    });
    sorted_colors
}

fn main() {
    let args = Cli::parse();

    // Load JSON file
    let entry_file = File::open(args.input).expect("Failed to open file");
    let entry_reader = BufReader::new(entry_file);

    // Make and sort color entry vector
    let colors: Vec<ColorEntry> = serde_json::from_reader(entry_reader).expect("Failed to parse JSON");
    let sorted_colors = sort_colors(colors.clone());
    
    // Calculate layout
    let rows = (colors.len() + COLUMNS - 1) / COLUMNS;
    let img_width = (COLUMNS as u32) * BLOCK_SIZE_X;
    let img_height = (rows as u32) * BLOCK_SIZE_Y;

    // Draw image w/ a trans background
    let mut imgbuf: RgbaImage = image::ImageBuffer::new(img_width, img_height);
    let trans_bg = Rect::at(0,0).of_size(img_width, img_height);
    draw_filled_rect_mut(&mut imgbuf, trans_bg, Rgba([0u8,0u8,0u8,0u8]));

    // Load in font & set scale
    let font_data = include_bytes!("../JetBrainsMono-Regular.ttf");
    let font = Font::try_from_bytes(font_data as &[u8]).expect("Error constructing Font");

    // Draw Labeled Color Palette
    for (i, color) in sorted_colors.iter().enumerate() {
        let (r, g, b) = hex_to_rgb(&color.hex);

        let col = (i % COLUMNS) as u32;
        let row = (i / COLUMNS) as u32;
        
        let x0 = col * BLOCK_SIZE_X;
        let y0 = row * BLOCK_SIZE_Y;

        let rect = Rect::at(x0 as i32, y0 as i32).of_size(BLOCK_SIZE_X, BLOCK_SIZE_Y);
        draw_filled_rect_mut(&mut imgbuf, rect, Rgba([r, g, b, 255u8]));

        let bg_rgb = Srgb::new(r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0);
        let (text_r, text_g, text_b) = pick_label_color(bg_rgb);
        let hsl = Hsl::from_color(bg_rgb);
        let mut shadow_hsl = hsl;
        shadow_hsl.lightness = (hsl.lightness - 0.1).max(0.0);

        let shadow_rgb: Srgb<u8> = Srgb::from_color(shadow_hsl).into_format();
        let shadow = Rgba([shadow_rgb.red, shadow_rgb.green, shadow_rgb.blue, 255u8]);

        let rect = Rect::at(x0 as i32, (y0 + BLOCK_SIZE_Y - BLOCK_SIZE_Y/20) as i32).of_size(BLOCK_SIZE_X, BLOCK_SIZE_Y/20);
        draw_filled_rect_mut(&mut imgbuf, rect, shadow);

        let name_rect = Rect::at(x0 as i32, y0  as i32).of_size(BLOCK_SIZE_X, BLOCK_SIZE_Y);
        let hex_rect = Rect::at(x0 as i32, y0 as i32 + (BLOCK_SIZE_Y as f32/3.25) as i32).of_size(BLOCK_SIZE_X, BLOCK_SIZE_Y);
        let base_color = Rgba([r, g, b, 255]);
        let text_color = Rgba([text_r, text_g, text_b, 255]);

        /*println!("Color: {:^10} | H: {:>10} | S: {:>10} | L:{:>10}",color.name, hsl.hue.into_degrees().abs(), hsl.saturation, hsl.lightness);*/
        
        draw_centered_text(&mut imgbuf, &font, &color.name, name_rect, base_color, text_color, 3.5, -10);
        draw_centered_text(&mut imgbuf, &font, &color.hex, hex_rect, base_color, text_color, 6.5, 0-(BLOCK_SIZE_Y/20) as i32);
    }
    let output_file = std::path::PathBuf::from(&args.output);
    imgbuf.save(output_file).expect("Failed to save image");
    println!("Saved {} color blocks to {}", colors.len(), args.output);
}
