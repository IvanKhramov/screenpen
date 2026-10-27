//! Draws the app icon and writes it in every format the packages need:
//! `cargo run -p screenpen-core --example icon -- assets`
//! gives icon-<size>.png, screenpen.ico (Windows) and screenpen.icns (macOS).

use screenpen_core::icon::draw;

fn png(size: u32) -> Vec<u8> {
    draw(size).expect("icon").encode_png().expect("png")
}

/// An .ico with PNG entries (Windows Vista and later read those at every size).
fn ico(sizes: &[u32]) -> Vec<u8> {
    let images: Vec<Vec<u8>> = sizes.iter().map(|&s| png(s)).collect();
    let mut out = Vec::new();
    out.extend([0u8, 0, 1, 0]);
    out.extend((sizes.len() as u16).to_le_bytes());
    let mut offset = 6 + 16 * sizes.len() as u32;
    for (&size, image) in sizes.iter().zip(&images) {
        let side = if size >= 256 { 0 } else { size as u8 };
        out.extend([side, side, 0, 0]);
        out.extend(1u16.to_le_bytes());
        out.extend(32u16.to_le_bytes());
        out.extend((image.len() as u32).to_le_bytes());
        out.extend(offset.to_le_bytes());
        offset += image.len() as u32;
    }
    for image in images {
        out.extend(image);
    }
    out
}

/// An .icns with PNG entries.
fn icns() -> Vec<u8> {
    let entries: [(&[u8; 4], u32); 10] = [
        (b"icp4", 16),
        (b"icp5", 32),
        (b"ic11", 32),
        (b"icp6", 64),
        (b"ic12", 64),
        (b"ic07", 128),
        (b"ic08", 256),
        (b"ic13", 256),
        (b"ic09", 512),
        (b"ic14", 512),
    ];
    let mut body = Vec::new();
    for (kind, size) in entries.iter().chain([(b"ic10", 1024)].iter()) {
        let image = png(*size);
        body.extend(*kind);
        body.extend((image.len() as u32 + 8).to_be_bytes());
        body.extend(image);
    }
    let mut out = b"icns".to_vec();
    out.extend((body.len() as u32 + 8).to_be_bytes());
    out.extend(body);
    out
}

fn main() {
    let dir = std::path::PathBuf::from(std::env::args().nth(1).unwrap_or_else(|| "assets".into()));
    std::fs::create_dir_all(&dir).expect("output dir");
    for size in [16, 32, 48, 64, 128, 256, 512, 1024] {
        std::fs::write(dir.join(format!("icon-{size}.png")), png(size)).expect("png");
    }
    std::fs::write(
        dir.join("screenpen.ico"),
        ico(&[16, 20, 24, 32, 40, 48, 64, 256]),
    )
    .expect("ico");
    std::fs::write(dir.join("screenpen.icns"), icns()).expect("icns");
    println!("{}", dir.display());
}
