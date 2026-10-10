# Violin Thickness Mapper (Rust)

A native Linux desktop rewrite of the Python/Tkinter thickness mapper. It uses `eframe`/`egui`, `image`, `nalgebra`, `rayon`, `csv`, and `rfd`.

## Features

- Separate front and back plates and measurement sets
- Click-to-add thickness values in millimetres
- Image-pixel coordinate labels and adjustable grid opacity
- RBF interpolation generated on a background thread, with parallel pixel evaluation
- Violin-body mask based on the original image's grayscale threshold (`gray < 240`)
- Automatic or manual color scale, heatmap legend, undo/clear, and CSV save/load
- JPEG and PNG input

## Build on Arch Linux / CachyOS

Install the Rust toolchain and native build dependencies:

```sh
sudo pacman -S --needed rust cargo base-devel pkgconf cmake gtk3
```

From this directory:

```sh
cargo run --release
```

For a release binary:

```sh
cargo build --release
./target/release/violin-thickness-mapper
```

Put `violin_back.jpg` and `violin_front.jpg` in the current working directory to load them automatically at startup. Otherwise, use **Load Image…**. Loading a new image resets that side's measurements.

## Cross-platform releases

The GitHub Actions workflow builds native release binaries for Linux x86_64, Windows x86_64, macOS Apple Silicon, and macOS Intel. On a tag push such as `v0.1.0`, it publishes:

- Linux: `ViolinThicknessMapper-linux-x86_64.tar.gz`, containing the executable, the two default images, this README, and a launcher named `ViolinThicknessMapper`.
- Windows: `ViolinThicknessMapper-windows-x86_64.exe`, an Inno Setup installer with Start Menu shortcuts and an optional desktop shortcut.
- macOS: one `.dmg` per architecture containing `Violin Thickness Mapper.app` and its bundled default images. The app bundle identifier is `dev.alden.violinthicknessmapper`.

To publish, create a GitHub repository, initialize and push this project, then push a version tag:

```sh
git init -b main
git add Cargo.toml Cargo.lock README.md src violin_back.jpg violin_front.jpg packaging .github/workflows/release.yml
git commit -m "Initial release"
git remote add origin https://github.com/USERNAME/violin-thickness-mapper.git
git push -u origin main
git tag -a v0.1.0 -m "Violin Thickness Mapper v0.1.0"
git push origin v0.1.0
```

The tag starts all four native builds and the release upload. `workflow_dispatch` can also build and publish temporary workflow artifacts without creating a GitHub Release. Linux desktop libraries are provided by the user's distribution and are not bundled in the archive.

These initial Windows and macOS packages are **unsigned**. Windows may show SmartScreen warnings, and macOS Gatekeeper may warn or block first launch. No signing credentials or secrets are configured. Distribution without those warnings requires a trusted Windows code-signing identity and, for macOS, a Developer ID signature with hardened runtime plus Apple notarization.

## CSV format

Headerless CSV, one measurement per row:

```csv
120,240,3.2
260,310,2.8
```

The loader expects three numeric columns per row. Coordinates must be within the currently loaded image.

## Notes

- The RBF uses a Gaussian kernel with a small diagonal smoothing term for numerical stability. This is a sensible starting point, but it is not numerically identical to SciPy's default `RBFInterpolator` kernel; validate it against known measurements before using it for precision lutherie decisions.
- The body mask intentionally follows the original script's simple grayscale threshold. For photos with dark backgrounds or shadows, a dedicated segmentation mask would be more reliable.
- The grid spacing is 50 image pixels, matching the Python version.
