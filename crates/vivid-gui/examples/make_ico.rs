//! 从 `assets/icon.png` 生成多尺寸 `assets/icon.ico`。
//!
//! 一次性工具，换了 logo 之后重跑：
//!
//! ```text
//! cargo run -p vivid-gui --example make_ico
//! ```
//!
//! **为什么不放 build.rs**：每次编译都重算七档 Lanczos 缩放没有意义，
//! 而且生成物要入库——打包时 Windows 资源段要用它，CI 上不该临时算。
//!
//! 小尺寸（16/24）从 1156px 缩下来必然发糊，这是已知且接受的：
//! 真要清楚得有一版专门为 16px 画的简化图形（去掉马赛克细节和回环箭头），
//! 那是设计的事，不是缩放算法能补的。

use std::io::Write;

/// Windows 会用到的档位。
///
/// 16 是任务栏与资源管理器的小图标（用得最多的一档），256 是大图标预览。
/// 中间几档让系统在不同 DPI 下都有接近的原始尺寸可用，不必二次缩放。
const SIZES: &[u32] = &[16, 24, 32, 48, 64, 128, 256];

fn main() {
    let out = std::path::Path::new("crates/vivid-gui/assets/icon.ico");

    let src = image::load_from_memory(include_bytes!("../assets/icon.png"))
        .expect("读不到 assets/icon.png")
        .to_rgba8();

    // 和 main.rs 的 load_icon 做同样的预处理。
    // 这里刻意复制而不是共用：vivid-gui 是 bin crate，没有 lib target，
    // example 无法 use 它的私有模块。为两个函数去拆一个 lib 不划算。
    let square = pad_to_square(&crop_to_opaque(&src));

    let mut frames: Vec<(u32, Vec<u8>)> = Vec::new();
    for &size in SIZES {
        let resized =
            image::imageops::resize(&square, size, size, image::imageops::FilterType::Lanczos3);

        // ICO 里的每一帧存成 PNG。Windows Vista 起支持，比 BMP 省一半体积，
        // 也不用自己处理 BMP 的 AND 掩码和上下翻转。
        let mut png = Vec::new();
        {
            use image::ImageEncoder;
            image::codecs::png::PngEncoder::new(&mut png)
                .write_image(
                    resized.as_raw(),
                    size,
                    size,
                    image::ExtendedColorType::Rgba8,
                )
                .expect("PNG 编码失败");
        }
        frames.push((size, png));
    }

    std::fs::write(out, build_ico(&frames)).expect("写不出 icon.ico");

    println!("已生成 {}", out.display());
    println!("  档位：{SIZES:?}");
    println!(
        "  大小：{:.1} KB",
        std::fs::metadata(out).unwrap().len() as f64 / 1024.0
    );
}

/// 拼 ICO 容器。
///
/// 格式很简单：6 字节文件头 + 每帧 16 字节目录项 + 各帧数据。
/// 手写而不是用 image 的 IcoEncoder，是因为要精确控制档位与每帧的编码方式。
fn build_ico(frames: &[(u32, Vec<u8>)]) -> Vec<u8> {
    let mut out = Vec::new();

    // ICONDIR
    out.write_all(&0u16.to_le_bytes()).unwrap(); // reserved
    out.write_all(&1u16.to_le_bytes()).unwrap(); // type: 1 = icon
    out.write_all(&(frames.len() as u16).to_le_bytes()).unwrap();

    // 数据区从目录之后开始
    let mut offset = 6 + frames.len() * 16;

    for (size, png) in frames {
        // 宽高字段是单字节，256 存 0——这是格式规定，不是笔误
        let dim = if *size >= 256 { 0u8 } else { *size as u8 };
        out.push(dim); // width
        out.push(dim); // height
        out.push(0); // 调色板颜色数，真彩色为 0
        out.push(0); // reserved
        out.write_all(&1u16.to_le_bytes()).unwrap(); // color planes
        out.write_all(&32u16.to_le_bytes()).unwrap(); // bits per pixel
        out.write_all(&(png.len() as u32).to_le_bytes()).unwrap();
        out.write_all(&(offset as u32).to_le_bytes()).unwrap();
        offset += png.len();
    }

    for (_, png) in frames {
        out.write_all(png).unwrap();
    }
    out
}

/// 裁掉四周完全透明的边。
fn crop_to_opaque(img: &image::RgbaImage) -> image::RgbaImage {
    const ALPHA_FLOOR: u8 = 8;

    let (w, h) = img.dimensions();
    let (mut x0, mut y0, mut x1, mut y1) = (w, h, 0u32, 0u32);
    for y in 0..h {
        for x in 0..w {
            if img.get_pixel(x, y)[3] > ALPHA_FLOOR {
                x0 = x0.min(x);
                y0 = y0.min(y);
                x1 = x1.max(x + 1);
                y1 = y1.max(y + 1);
            }
        }
    }
    if x1 <= x0 || y1 <= y0 {
        return img.clone();
    }
    image::imageops::crop_imm(img, x0, y0, x1 - x0, y1 - y0).to_image()
}

/// 居中补透明边成正方形。非等比缩放会把圆角压变形。
fn pad_to_square(img: &image::RgbaImage) -> image::RgbaImage {
    let (w, h) = img.dimensions();
    if w == h {
        return img.clone();
    }
    let side = w.max(h);
    let mut out = image::RgbaImage::from_pixel(side, side, image::Rgba([0, 0, 0, 0]));
    image::imageops::overlay(
        &mut out,
        img,
        ((side - w) / 2) as i64,
        ((side - h) / 2) as i64,
    );
    out
}
