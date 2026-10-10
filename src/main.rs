use anyhow::{Context, Result};
use eframe::egui::{
    self, Color32, ColorImage, Pos2, Rect, Sense, Stroke, TextureHandle, TextureOptions, Vec2,
};
use image::DynamicImage;
use image::{Rgba, RgbaImage};
use nalgebra::{DMatrix, DVector};
use rayon::prelude::*;
#[cfg(target_os = "macos")]
use std::path::Path;
use std::{
    fs::File,
    path::PathBuf,
    sync::mpsc::{self, Receiver, TryRecvError},
    thread,
};

const DEFAULT_BACK: &str = "violin_back.jpg";
const DEFAULT_FRONT: &str = "violin_front.jpg";
const GRID_SPACING: usize = 50;

#[derive(Clone, Copy, Debug)]
struct Point {
    x: f64,
    y: f64,
    value: f64,
}

struct Plate {
    path: Option<PathBuf>,
    image: DynamicImage,
    texture: TextureHandle,
    points: Vec<Point>,
    heatmap: Option<TextureHandle>,
    heatmap_rgba: Option<Vec<u8>>,
    heatmap_range: Option<(f64, f64)>,
    dimensions: [usize; 2],
}

struct HeatmapResult {
    side: usize,
    rgba: Vec<u8>,
    min: f64,
    max: f64,
}

struct MapperApp {
    plates: [Option<Plate>; 2],
    active_side: usize,
    grid_opacity: f32,
    min_scale: String,
    max_scale: String,
    use_manual_scale: bool,
    pending_point: Option<(usize, usize)>,
    thickness_input: String,
    status: String,
    heatmap_rx: Option<Receiver<Result<HeatmapResult, String>>>,
    generating: bool,
    heatmap_range: Option<(f64, f64)>,
    view_zoom: f32,
    view_pan: Vec2,
    view_initialized: bool,
}

impl MapperApp {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let mut app = Self {
            plates: [None, None],
            active_side: 0,
            grid_opacity: 0.4,
            min_scale: String::new(),
            max_scale: String::new(),
            use_manual_scale: false,
            pending_point: None,
            thickness_input: String::new(),
            status: "Load a violin image, then click the plate to add measurements.".into(),
            heatmap_rx: None,
            generating: false,
            heatmap_range: None,
            view_zoom: 1.0,
            view_pan: Vec2::ZERO,
            view_initialized: false,
        };
        app.try_load_default(&cc.egui_ctx, 0, DEFAULT_BACK);
        app.try_load_default(&cc.egui_ctx, 1, DEFAULT_FRONT);
        app
    }

    fn try_load_default(&mut self, ctx: &egui::Context, side: usize, filename: &str) {
        let path = default_image_path(filename);
        if path.exists() {
            if let Err(e) = self.load_image(ctx, side, path) {
                self.status = format!("Could not load {filename}: {e:#}");
            }
        }
    }

    fn load_image(&mut self, ctx: &egui::Context, side: usize, path: PathBuf) -> Result<()> {
        let image = image::open(&path).with_context(|| format!("opening {}", path.display()))?;
        let rgba = image.to_rgba8();
        let [w, h] = [rgba.width() as usize, rgba.height() as usize];
        let color_image = ColorImage::from_rgba_unmultiplied([w, h], rgba.as_raw());
        let texture = ctx.load_texture(
            format!("plate-{side}-{}", path.display()),
            color_image,
            TextureOptions::LINEAR,
        );
        self.plates[side] = Some(Plate {
            path: Some(path),
            image,
            texture,
            points: Vec::new(),
            heatmap: None,
            heatmap_rgba: None,
            heatmap_range: None,
            dimensions: [w, h],
        });
        self.heatmap_range = None;
        if side == self.active_side {
            self.reset_view();
        }
        Ok(())
    }

    fn active(&self) -> Option<&Plate> {
        self.plates[self.active_side].as_ref()
    }
    fn active_mut(&mut self) -> Option<&mut Plate> {
        self.plates[self.active_side].as_mut()
    }

    fn reset_view(&mut self) {
        self.view_zoom = 1.0;
        self.view_pan = Vec2::ZERO;
        self.view_initialized = false;
    }

    fn clear_points(&mut self) {
        if let Some(p) = self.active_mut() {
            p.points.clear();
            p.heatmap = None;
            p.heatmap_rgba = None;
            p.heatmap_range = None;
        }
        self.heatmap_range = None;
        self.status = "Measurements cleared for this side.".into();
    }

    fn undo(&mut self) {
        if let Some(p) = self.active_mut() {
            if p.points.pop().is_some() {
                p.heatmap = None;
                p.heatmap_rgba = None;
                p.heatmap_range = None;
                self.heatmap_range = None;
                self.status = "Removed the last measurement.".into();
            }
        }
    }

    fn generate_heatmap(&mut self) {
        let Some(p) = self.active() else {
            self.status = "Load an image first.".into();
            return;
        };
        if p.points.len() < 3 {
            self.status = "At least three measurements are needed.".into();
            return;
        }
        let side = self.active_side;
        let points = p.points.clone();
        let image = p.image.clone();
        let dimensions = p.dimensions;
        let manual = if self.use_manual_scale {
            match (
                self.min_scale.trim().parse::<f64>(),
                self.max_scale.trim().parse::<f64>(),
            ) {
                (Ok(a), Ok(b)) if a < b => Some((a, b)),
                _ => {
                    self.status = "Enter valid scale limits, with minimum below maximum.".into();
                    return;
                }
            }
        } else {
            None
        };
        let (tx, rx) = mpsc::channel();
        self.heatmap_rx = Some(rx);
        self.generating = true;
        self.status = "Generating heatmap…".into();
        thread::spawn(move || {
            let result =
                make_heatmap(&image, &points, dimensions, manual).map_err(|e| format!("{e:#}"));
            let _ = tx.send(result.map(|(rgba, min, max)| HeatmapResult {
                side,
                rgba,
                min,
                max,
            }));
        });
    }

    fn poll_heatmap(&mut self, ctx: &egui::Context) {
        let result = self.heatmap_rx.as_ref().map(|rx| rx.try_recv());
        match result {
            Some(Ok(Ok(data))) => {
                let Some(plate) = self.plates[data.side].as_mut() else {
                    return;
                };
                let [w, h] = plate.dimensions;
                let image = ColorImage::from_rgba_unmultiplied([w, h], &data.rgba);
                plate.heatmap = Some(ctx.load_texture(
                    format!("heatmap-{}", data.side),
                    image,
                    TextureOptions::LINEAR,
                ));
                plate.heatmap_rgba = Some(data.rgba);
                plate.heatmap_range = Some((data.min, data.max));
                self.heatmap_range = Some((data.min, data.max));
                self.generating = false;
                self.heatmap_rx = None;
                self.status = format!(
                    "Heatmap generated. Display range: {:.2}–{:.2} mm.",
                    data.min, data.max
                );
            }
            Some(Ok(Err(err))) => {
                self.generating = false;
                self.heatmap_rx = None;
                self.status = format!("Interpolation failed: {err}");
            }
            Some(Err(TryRecvError::Disconnected)) => {
                self.generating = false;
                self.heatmap_rx = None;
                self.status = "Heatmap worker stopped unexpectedly.".into();
            }
            _ => {}
        }
    }

    fn save_points(&mut self) {
        let Some(plate) = self.active() else {
            self.status = "Load an image first.".into();
            return;
        };
        let points = plate.points.clone();
        let suggested = format!(
            "violin_{}_points.csv",
            if self.active_side == 0 {
                "back"
            } else {
                "front"
            }
        );
        let Some(path) = rfd::FileDialog::new()
            .add_filter("CSV", &["csv"])
            .set_file_name(&suggested)
            .save_file()
        else {
            return;
        };
        let result = (|| -> Result<()> {
            let mut writer = csv::WriterBuilder::new()
                .has_headers(false)
                .from_path(&path)?;
            for p in &points {
                writer.write_record(&[p.x.to_string(), p.y.to_string(), p.value.to_string()])?;
            }
            writer.flush()?;
            Ok(())
        })();
        self.status = match result {
            Ok(()) => format!("Saved {} measurements to {}.", points.len(), path.display()),
            Err(e) => format!("CSV save failed: {e:#}"),
        };
    }

    fn load_points(&mut self) {
        let Some(path) = rfd::FileDialog::new()
            .add_filter("CSV", &["csv"])
            .pick_file()
        else {
            return;
        };
        let result = (|| -> Result<Vec<Point>> {
            let file = File::open(&path)?;
            let mut reader = csv::ReaderBuilder::new()
                .has_headers(false)
                .flexible(false)
                .from_reader(file);
            let mut points = Vec::new();
            for (i, row) in reader.records().enumerate() {
                let row = row.with_context(|| format!("reading CSV row {}", i + 1))?;
                if row.len() != 3 {
                    anyhow::bail!("row {} must contain x,y,thickness", i + 1);
                }
                let x: f64 = row[0]
                    .trim()
                    .parse()
                    .with_context(|| format!("invalid x on row {}", i + 1))?;
                let y: f64 = row[1]
                    .trim()
                    .parse()
                    .with_context(|| format!("invalid y on row {}", i + 1))?;
                let value: f64 = row[2]
                    .trim()
                    .parse()
                    .with_context(|| format!("invalid thickness on row {}", i + 1))?;
                if !x.is_finite() || !y.is_finite() || !value.is_finite() {
                    anyhow::bail!("non-finite value on row {}", i + 1);
                }
                points.push(Point { x, y, value });
            }
            Ok(points)
        })();

        match result {
            Ok(points) => {
                if let Some(p) = self.active_mut() {
                    let [w, h] = p.dimensions;

                    if points
                        .iter()
                        .any(|q| q.x < 0.0 || q.y < 0.0 || q.x >= w as f64 || q.y >= h as f64)
                    {
                        self.status =
                            "CSV contains coordinates outside the current image; nothing loaded."
                                .into();
                        return;
                    }

                    let loaded_count = points.len();
                    p.points = points;
                    p.heatmap = None;
                    p.heatmap_rgba = None;
                    p.heatmap_range = None;

                    self.heatmap_range = None;
                    self.status = format!(
                        "Loaded {} measurements from {}.",
                        loaded_count,
                        path.display()
                    );
                } else {
                    self.status = "Load an image before loading measurements.".into();
                }
            }
            Err(e) => self.status = format!("CSV load failed: {e:#}"),
        }
    }

    fn save_png(&mut self) {
        let Some(p) = self.active() else {
            self.status = "Load an image first.".into();
            return;
        };
        let side_name = if self.active_side == 0 {
            "back"
        } else {
            "front"
        };
        let suggested = format!("violin_{side_name}_thickness.png");
        let Some(path) = rfd::FileDialog::new()
            .add_filter("PNG", &["png"])
            .set_file_name(&suggested)
            .save_file()
        else {
            return;
        };
        let mut image = p.image.to_rgba8();
        let [w, h] = p.dimensions;
        let heatmap = p.heatmap_rgba.clone();
        let points = p.points.clone();
        let opacity = self.grid_opacity;
        let range = p.heatmap_range;
        let result = (|| -> Result<()> {
            if let Some(rgba) = heatmap {
                for (dst, src) in image.pixels_mut().zip(rgba.chunks_exact(4)) {
                    let a = src[3] as f32 / 255.0;
                    if a > 0.0 {
                        for c in 0..3 {
                            dst[c] = (src[c] as f32 * a + dst[c] as f32 * (1.0 - a)).round() as u8;
                        }
                    }
                }
            }
            let grid = Rgba([140, 140, 140, (opacity * 255.0) as u8]);
            for x in (0..w).step_by(GRID_SPACING) {
                draw_export_line(&mut image, x as i32, 0, x as i32, h as i32 - 1, grid);
            }
            for y in (0..h).step_by(GRID_SPACING) {
                draw_export_line(&mut image, 0, y as i32, w as i32 - 1, y as i32, grid);
            }
            for p in &points {
                let x = p.x.round() as i32;
                let y = p.y.round() as i32;
                let radius = ((w.min(h) as f32 / 250.0).round() as i32).max(4);
                draw_export_circle(&mut image, x, y, radius, Rgba([255, 255, 255, 255]), true);
                draw_export_circle(&mut image, x, y, radius, Rgba([0, 0, 0, 255]), false);
                draw_bitmap_text(
                    &mut image,
                    x + radius + 3,
                    y - radius - 10,
                    &format!("{:.1}", p.value),
                    ((w.min(h) / 1200).max(2)) as i32,
                    Rgba([0, 0, 0, 255]),
                );
            }
            let legend_width = ((w as f32 * 0.07).round() as usize).clamp(100, 220);
            let gap = (w / 40).max(24);
            let out_w = w
                .checked_add(gap)
                .and_then(|n| n.checked_add(legend_width))
                .context("export dimensions overflow")?;
            let out_width = u32::try_from(out_w).context("export width exceeds PNG limits")?;
            let out_height = u32::try_from(h).context("export height exceeds PNG limits")?;
            let mut output =
                RgbaImage::from_pixel(out_width, out_height, Rgba([248, 248, 248, 255]));
            image::imageops::replace(&mut output, &image, 0, 0);
            let bar_x = w + gap;
            let margin = (h / 12).max(12);
            let bar_h = h.saturating_sub(margin * 2).max(1);
            for y in 0..bar_h {
                let c = turbo_color(1.0 - y as f32 / bar_h as f32);
                for x in bar_x..bar_x + legend_width.min(32) {
                    output.put_pixel(
                        x as u32,
                        (y + margin) as u32,
                        Rgba([c.r(), c.g(), c.b(), 255]),
                    );
                }
            }
            if let Some((lo, hi)) = range {
                let scale = ((h / 140).max(2)) as i32;
                draw_bitmap_text(
                    &mut output,
                    bar_x as i32 + 38,
                    margin as i32,
                    &format!("{hi:.2}"),
                    scale,
                    Rgba([0, 0, 0, 255]),
                );
                draw_bitmap_text(
                    &mut output,
                    bar_x as i32 + 38,
                    (margin + bar_h) as i32 - 8 * scale,
                    &format!("{lo:.2}"),
                    scale,
                    Rgba([0, 0, 0, 255]),
                );
            }
            output
                .save(&path)
                .with_context(|| format!("writing {}", path.display()))?;
            Ok(())
        })();
        self.status = match result {
            Ok(()) => format!("Saved full-resolution PNG to {}.", path.display()),
            Err(e) => format!("PNG export failed: {e:#}"),
        };
    }
}

fn default_image_path(filename: &str) -> PathBuf {
    #[cfg(target_os = "macos")]
    if let Ok(executable) = std::env::current_exe() {
        if let Some(resources) = executable
            .parent()
            .and_then(Path::parent)
            .map(|contents| contents.join("Resources").join(filename))
        {
            if resources.exists() {
                return resources;
            }
        }
    }
    PathBuf::from(filename)
}

fn draw_export_pixel(image: &mut RgbaImage, x: i32, y: i32, color: Rgba<u8>) {
    if x < 0 || y < 0 || x >= image.width() as i32 || y >= image.height() as i32 {
        return;
    }
    let dst = image.get_pixel_mut(x as u32, y as u32);
    let a = color[3] as f32 / 255.0;
    for c in 0..3 {
        dst[c] = (color[c] as f32 * a + dst[c] as f32 * (1.0 - a)).round() as u8;
    }
}

fn draw_export_line(
    image: &mut RgbaImage,
    mut x0: i32,
    mut y0: i32,
    x1: i32,
    y1: i32,
    color: Rgba<u8>,
) {
    let dx = (x1 - x0).abs();
    let sx = if x0 < x1 { 1 } else { -1 };
    let dy = -(y1 - y0).abs();
    let sy = if y0 < y1 { 1 } else { -1 };
    let mut err = dx + dy;
    loop {
        draw_export_pixel(image, x0, y0, color);
        if x0 == x1 && y0 == y1 {
            break;
        }
        let e2 = 2 * err;
        if e2 >= dy {
            err += dy;
            x0 += sx;
        }
        if e2 <= dx {
            err += dx;
            y0 += sy;
        }
    }
}

fn draw_export_circle(
    image: &mut RgbaImage,
    cx: i32,
    cy: i32,
    radius: i32,
    color: Rgba<u8>,
    filled: bool,
) {
    for y in -radius..=radius {
        for x in -radius..=radius {
            let d = x * x + y * y;
            if if filled {
                d <= radius * radius
            } else {
                (d - radius * radius).abs() <= radius
            } {
                draw_export_pixel(image, cx + x, cy + y, color);
            }
        }
    }
}

fn draw_bitmap_text(
    image: &mut RgbaImage,
    x: i32,
    y: i32,
    text: &str,
    scale: i32,
    color: Rgba<u8>,
) {
    let glyph = |c: char| -> [u8; 7] {
        match c {
            '0' => [14, 17, 19, 21, 25, 17, 14],
            '1' => [4, 12, 4, 4, 4, 4, 14],
            '2' => [14, 17, 1, 2, 4, 8, 31],
            '3' => [30, 1, 1, 14, 1, 1, 30],
            '4' => [2, 6, 10, 18, 31, 2, 2],
            '5' => [31, 16, 16, 30, 1, 1, 30],
            '6' => [6, 8, 16, 30, 17, 17, 14],
            '7' => [31, 1, 2, 4, 8, 8, 8],
            '8' => [14, 17, 17, 14, 17, 17, 14],
            '9' => [14, 17, 17, 15, 1, 2, 12],
            '.' => [0, 0, 0, 0, 0, 6, 6],
            '-' => [0, 0, 0, 31, 0, 0, 0],
            _ => [0; 7],
        }
    };
    let mut ox = x;
    for c in text.chars() {
        let bits = glyph(c);
        for (row, mask) in bits.iter().enumerate() {
            for col in 0..5 {
                if mask & (1 << (4 - col)) != 0 {
                    for yy in 0..scale {
                        for xx in 0..scale {
                            draw_export_pixel(
                                image,
                                ox + col * scale + xx,
                                y + row as i32 * scale + yy,
                                color,
                            );
                        }
                    }
                }
            }
        }
        ox += 6 * scale;
    }
}

fn rbf_kernel(r: f64) -> f64 {
    // Gaussian radial basis function. Coordinates are normalized to the image diagonal.
    (-4.0 * r * r).exp()
}

fn make_heatmap(
    image: &DynamicImage,
    points: &[Point],
    dims: [usize; 2],
    manual: Option<(f64, f64)>,
) -> Result<(Vec<u8>, f64, f64)> {
    let [w, h] = dims;
    let diagonal = ((w * w + h * h) as f64).sqrt().max(1.0);
    let coords: Vec<[f64; 2]> = points
        .iter()
        .map(|p| [p.x / diagonal, p.y / diagonal])
        .collect();
    let n = points.len();
    let mut matrix = DMatrix::<f64>::zeros(n, n);
    for i in 0..n {
        for j in 0..n {
            let dx = coords[i][0] - coords[j][0];
            let dy = coords[i][1] - coords[j][1];
            matrix[(i, j)] = rbf_kernel((dx * dx + dy * dy).sqrt());
        }
        matrix[(i, i)] += 1e-4; // small smoothing term for numerical stability
    }
    let values = DVector::from_iterator(n, points.iter().map(|p| p.value));
    let weights = matrix
        .lu()
        .solve(&values)
        .context("RBF system is singular; try removing duplicate measurement coordinates")?;
    let base = image.to_rgba8();
    let mut raw_values = vec![0.0f32; w * h];
    raw_values
        .par_iter_mut()
        .enumerate()
        .for_each(|(index, out)| {
            let x = (index % w) as f64 / diagonal;
            let y = (index / w) as f64 / diagonal;
            let mut value = 0.0;
            for i in 0..n {
                let dx = x - coords[i][0];
                let dy = y - coords[i][1];
                value += weights[i] * rbf_kernel((dx * dx + dy * dy).sqrt());
            }
            *out = value as f32;
        });
    let valid: Vec<f64> = raw_values
        .iter()
        .enumerate()
        .filter_map(|(i, &v)| {
            let pixel = base.get_pixel((i % w) as u32, (i / w) as u32);
            let gray = (pixel[0] as f32 + pixel[1] as f32 + pixel[2] as f32) / 3.0;
            (gray < 240.0).then_some(v as f64)
        })
        .collect();
    let (auto_min, auto_max) = if valid.is_empty() {
        (0.0, 1.0)
    } else {
        (
            valid.iter().copied().fold(f64::INFINITY, f64::min),
            valid.iter().copied().fold(f64::NEG_INFINITY, f64::max),
        )
    };
    let (min, max) = manual.unwrap_or((auto_min, auto_max));
    let span = (max - min).max(f64::EPSILON);
    let rgba: Vec<u8> = raw_values
        .iter()
        .enumerate()
        .flat_map(|(i, &v)| {
            let pixel = base.get_pixel((i % w) as u32, (i / w) as u32);
            let gray = (pixel[0] as f32 + pixel[1] as f32 + pixel[2] as f32) / 3.0;
            if gray >= 240.0 {
                return [0, 0, 0, 0];
            }
            let t = (((v as f64 - min) / span).clamp(0.0, 1.0) * 5.0) as usize;
            let f = ((v as f64 - min) / span).clamp(0.0, 1.0) * 5.0 - t as f64;
            let stops = [
                [48.0, 18.0, 59.0],
                [50.0, 100.0, 190.0],
                [30.0, 190.0, 190.0],
                [140.0, 220.0, 70.0],
                [250.0, 170.0, 35.0],
                [180.0, 4.0, 38.0],
            ];
            let a = stops[t.min(5)];
            let b = stops[(t + 1).min(5)];
            let r = (a[0] + (b[0] - a[0]) * f) as u8;
            let g = (a[1] + (b[1] - a[1]) * f) as u8;
            let b = (a[2] + (b[2] - a[2]) * f) as u8;
            [r, g, b, 155]
        })
        .collect();
    Ok((rgba, min, max))
}

impl eframe::App for MapperApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll_heatmap(ctx);
        egui::TopBottomPanel::top("toolbar").show(ctx, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.heading("Violin Thickness Mapper");
                ui.separator();
                if ui.selectable_label(self.active_side == 0, "Back").clicked()
                    && self.active_side != 0
                {
                    self.active_side = 0;
                    self.reset_view();
                    self.heatmap_range = self.plates[0].as_ref().and_then(|p| p.heatmap_range);
                }
                if ui
                    .selectable_label(self.active_side == 1, "Front")
                    .clicked()
                    && self.active_side != 1
                {
                    self.active_side = 1;
                    self.reset_view();
                    self.heatmap_range = self.plates[1].as_ref().and_then(|p| p.heatmap_range);
                }
                if ui.button("Load Image…").clicked() {
                    if let Some(path) = rfd::FileDialog::new()
                        .add_filter("Images", &["jpg", "jpeg", "png"])
                        .pick_file()
                    {
                        match self.load_image(ctx, self.active_side, path) {
                            Ok(()) => {
                                self.status =
                                    "Image loaded. Measurements reset for this side.".into()
                            }
                            Err(e) => self.status = format!("Image load failed: {e:#}"),
                        }
                    }
                }
                if ui.button("Undo Point").clicked() {
                    self.undo();
                }
                if ui.button("Clear Points").clicked() {
                    self.clear_points();
                }
                if ui
                    .add_enabled(
                        !self.generating,
                        egui::Button::new(if self.generating {
                            "Generating…"
                        } else {
                            "Generate Heatmap"
                        }),
                    )
                    .clicked()
                {
                    self.generate_heatmap();
                }
                if ui.button("Save CSV…").clicked() {
                    self.save_points();
                }
                if ui.button("Save PNG…").clicked() {
                    self.save_png();
                }
                if ui.button("Load CSV…").clicked() {
                    self.load_points();
                }
            });
        });
        egui::SidePanel::right("settings")
            .default_width(210.0)
            .show(ctx, |ui| {
                ui.heading("Display");
                ui.add(egui::Slider::new(&mut self.grid_opacity, 0.0..=1.0).text("Grid opacity"));
                ui.separator();
                ui.checkbox(&mut self.use_manual_scale, "Manual color scale");
                ui.add_enabled_ui(self.use_manual_scale, |ui| {
                    for (label, value) in [
                        ("Min (mm)", &mut self.min_scale),
                        ("Max (mm)", &mut self.max_scale),
                    ] {
                        ui.horizontal(|ui| {
                            ui.label(label);
                            let fill = ui.visuals().extreme_bg_color;
                            ui.add_sized(
                                [92.0, 24.0],
                                egui::TextEdit::singleline(value)
                                    .background_color(fill)
                                    .frame(true)
                                    .text_color(ui.visuals().text_color())
                                    .margin(egui::vec2(5.0, 3.0)),
                            );
                        });
                    }
                    if ui.button("Regenerate heatmap").clicked() {
                        self.generate_heatmap();
                    }
                });
                if let Some((lo, hi)) = self.heatmap_range {
                    ui.separator();
                    ui.label("Thickness (mm)");
                    let (rect, _) = ui.allocate_exact_size(Vec2::new(28.0, 180.0), Sense::hover());
                    for i in 0..100 {
                        let t = i as f32 / 99.0;
                        let y0 = rect.bottom() - rect.height() * (i + 1) as f32 / 100.0;
                        let y1 = rect.bottom() - rect.height() * i as f32 / 100.0;
                        let color = turbo_color(t);
                        ui.painter().rect_filled(
                            Rect::from_min_max(
                                Pos2::new(rect.left(), y0),
                                Pos2::new(rect.right(), y1),
                            ),
                            0.0,
                            color,
                        );
                    }
                    ui.painter().text(
                        Pos2::new(rect.right() + 7.0, rect.top()),
                        egui::Align2::LEFT_TOP,
                        format!("{hi:.2}"),
                        egui::FontId::proportional(12.0),
                        ui.visuals().text_color(),
                    );
                    ui.painter().text(
                        Pos2::new(rect.right() + 7.0, rect.bottom()),
                        egui::Align2::LEFT_BOTTOM,
                        format!("{lo:.2}"),
                        egui::FontId::proportional(12.0),
                        ui.visuals().text_color(),
                    );
                }
                ui.separator();
                let count = self.active().map_or(0, |p| p.points.len());
                ui.label(format!("Measurements: {count}"));
                if let Some(p) = self.active() {
                    ui.label(format!("Image: {} × {}", p.dimensions[0], p.dimensions[1]));
                }
            });
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label("Click image to measure · drag to pan · wheel to zoom");
                if ui.button("Fit").clicked() {
                    self.reset_view();
                }
            });
            let area = ui.available_rect_before_wrap();
            let (canvas, response) = ui.allocate_exact_size(area.size(), Sense::click_and_drag());
            let Some((w, h, texture_id, heatmap_id, points)) = self.active().map(|p| {
                (
                    p.dimensions[0],
                    p.dimensions[1],
                    p.texture.id(),
                    p.heatmap.as_ref().map(TextureHandle::id),
                    p.points.clone(),
                )
            }) else {
                ui.painter().text(
                    canvas.center(),
                    egui::Align2::CENTER_CENTER,
                    "Load an image to begin.",
                    egui::FontId::default(),
                    ui.visuals().text_color(),
                );
                return;
            };
            let fit = (canvas.width() / w as f32)
                .min(canvas.height() / h as f32)
                .max(0.001);
            if !self.view_initialized {
                self.view_zoom = fit;
                self.view_pan = Vec2::ZERO;
                self.view_initialized = true;
            }
            let size = Vec2::new(w as f32, h as f32) * self.view_zoom;
            let mut rect = Rect::from_center_size(canvas.center() + self.view_pan, size);
            if response.dragged() {
                self.view_pan += response.drag_delta();
            }
            if response.hovered() {
                let (scroll, pointer) =
                    ctx.input(|i| (i.raw_scroll_delta.y, i.pointer.hover_pos()));
                if scroll != 0.0 {
                    if let Some(pointer) = pointer {
                        let old = self.view_zoom;
                        self.view_zoom =
                            (old * (scroll * 0.001).exp()).clamp(fit * 0.25, fit * 12.0);
                        let factor = self.view_zoom / old;
                        let from_center = pointer - canvas.center();
                        self.view_pan = from_center + (self.view_pan - from_center) * factor;
                        let size = Vec2::new(w as f32, h as f32) * self.view_zoom;
                        rect = Rect::from_center_size(canvas.center() + self.view_pan, size);
                    }
                }
            }
            let painter = ui.painter_at(canvas);
            painter.image(
                texture_id,
                rect,
                Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                Color32::WHITE,
            );
            if let Some(tex) = heatmap_id {
                painter.image(
                    tex,
                    rect,
                    Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                    Color32::WHITE,
                );
            }
            let alpha = (self.grid_opacity * 255.0) as u8;
            let gc = Color32::from_rgba_unmultiplied(140, 140, 140, alpha);
            for x in (0..w).step_by(GRID_SPACING) {
                let px = rect.left() + x as f32 * self.view_zoom;
                painter.line_segment(
                    [Pos2::new(px, rect.top()), Pos2::new(px, rect.bottom())],
                    Stroke::new(1.0_f32, gc),
                );
            }
            for y in (0..h).step_by(GRID_SPACING) {
                let py = rect.top() + y as f32 * self.view_zoom;
                painter.line_segment(
                    [Pos2::new(rect.left(), py), Pos2::new(rect.right(), py)],
                    Stroke::new(1.0_f32, gc),
                );
            }
            for p in &points {
                let pos = rect.min + Vec2::new(p.x as f32, p.y as f32) * self.view_zoom;
                painter.circle_filled(pos, 4.0, Color32::WHITE);
                painter.circle_stroke(pos, 4.0, Stroke::new(1.0_f32, Color32::BLACK));
                painter.text(
                    pos + Vec2::new(5.0, -5.0),
                    egui::Align2::LEFT_BOTTOM,
                    format!("{:.1}", p.value),
                    egui::FontId::proportional(13.0),
                    Color32::WHITE,
                );
            }
            if response.clicked() && !response.dragged() && response.drag_delta() == Vec2::ZERO {
                if let Some(pos) = response.interact_pointer_pos() {
                    if rect.contains(pos) {
                        let x = ((pos.x - rect.left()) / self.view_zoom)
                            .floor()
                            .clamp(0.0, (w - 1) as f32) as usize;
                        let y = ((pos.y - rect.top()) / self.view_zoom)
                            .floor()
                            .clamp(0.0, (h - 1) as f32) as usize;
                        self.pending_point = Some((x, y));
                        self.thickness_input.clear();
                    }
                }
            }
        });
        egui::TopBottomPanel::bottom("status").show(ctx, |ui| {
            ui.label(&self.status);
        });
        if let Some((x, y)) = self.pending_point {
            egui::Window::new("Thickness measurement")
                .collapsible(false)
                .resizable(false)
                .show(ctx, |ui| {
                    ui.label(format!("Thickness at ({x}, {y}) pixels, in mm:"));
                    let response = ui.text_edit_singleline(&mut self.thickness_input);
                    response.request_focus();
                    ui.horizontal(|ui| {
                        if ui.button("Add point").clicked()
                            || (response.lost_focus()
                                && ui.input(|i| i.key_pressed(egui::Key::Enter)))
                        {
                            match self.thickness_input.trim().parse::<f64>() {
                                Ok(v) if v.is_finite() => {
                                    if let Some(p) = self.active_mut() {
                                        p.points.push(Point {
                                            x: x as f64,
                                            y: y as f64,
                                            value: v,
                                        });
                                        p.heatmap = None;
                                        p.heatmap_rgba = None;
                                        p.heatmap_range = None;
                                    }
                                    self.heatmap_range = None;
                                    self.status =
                                        format!("Added measurement ({x}, {y}) = {v:.2} mm.");
                                    self.pending_point = None;
                                }
                                _ => self.status = "Enter a valid finite thickness value.".into(),
                            }
                        }
                        if ui.button("Cancel").clicked() {
                            self.pending_point = None;
                        }
                    });
                });
        }
        ctx.request_repaint_after(std::time::Duration::from_millis(40));
    }
}

fn turbo_color(t: f32) -> Color32 {
    // Compact Turbo-like legend palette.
    let stops = [
        (0.00, [48, 18, 59]),
        (0.20, [50, 100, 190]),
        (0.40, [30, 190, 190]),
        (0.60, [140, 220, 70]),
        (0.80, [250, 170, 35]),
        (1.00, [180, 4, 38]),
    ];
    let t = t.clamp(0.0, 1.0);
    for pair in stops.windows(2) {
        let (a_t, a) = pair[0];
        let (b_t, b) = pair[1];
        if t <= b_t {
            let f = (t - a_t) / (b_t - a_t);
            return Color32::from_rgb(
                (a[0] as f32 + (b[0] as f32 - a[0] as f32) * f) as u8,
                (a[1] as f32 + (b[1] as f32 - a[1] as f32) * f) as u8,
                (a[2] as f32 + (b[2] as f32 - a[2] as f32) * f) as u8,
            );
        }
    }
    Color32::from_rgb(180, 4, 38)
}

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1200.0, 850.0])
            .with_min_inner_size([800.0, 550.0]),
        ..Default::default()
    };
    eframe::run_native(
        "Violin Thickness Mapper",
        options,
        Box::new(|cc| Ok(Box::new(MapperApp::new(cc)))),
    )
}
