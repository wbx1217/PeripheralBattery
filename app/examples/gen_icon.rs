//! 生成 `app/assets/app.ico` —— exe、任务栏、Alt+Tab 共用同一张图标。
//!
//! 图形不另画：直接复用托盘图标的渲染器 [`peripheral_battery::tray::icon::render_rgba`]，
//! 所以托盘、页眉、exe 三处永远是同一个渐变方块 + 白闪电。
//!
//! 用法（改完几何之后重跑一次，产物要提交进仓库）：
//!
//! ```text
//! cargo run --example gen_icon -p peripheral-battery-app
//! ```

use std::fs;
use std::path::PathBuf;

use peripheral_battery::tray::icon::render_rgba;

/// ICO 里要放的尺寸。16/20/24/32 对应 100%–200% 的任务栏与托盘，
/// 48/64 给 Alt+Tab 与资源管理器富视图，128/256 给快捷方式的大图标。
const SIZES: [i32; 9] = [16, 20, 24, 32, 40, 48, 64, 128, 256];

fn main() {
    let images: Vec<(i32, Vec<u8>)> = SIZES
        .iter()
        .map(|&size| (size, dib_bits(size, &render_rgba(size, false))))
        .collect();

    let header_len = 6 + 16 * images.len();
    let mut out = Vec::new();
    out.extend_from_slice(&0u16.to_le_bytes()); // reserved
    out.extend_from_slice(&1u16.to_le_bytes()); // type: icon
    out.extend_from_slice(&(images.len() as u16).to_le_bytes());

    let mut offset = header_len;
    for (size, data) in &images {
        // 256 在目录项里记作 0。
        out.push(if *size >= 256 { 0 } else { *size as u8 });
        out.push(if *size >= 256 { 0 } else { *size as u8 });
        out.push(0); // 调色板颜色数：真彩色为 0
        out.push(0); // reserved
        out.extend_from_slice(&1u16.to_le_bytes()); // planes
        out.extend_from_slice(&32u16.to_le_bytes()); // bpp
        out.extend_from_slice(&(data.len() as u32).to_le_bytes());
        out.extend_from_slice(&(offset as u32).to_le_bytes());
        offset += data.len();
    }

    for (_, data) in &images {
        out.extend_from_slice(data);
    }

    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("assets/app.ico");
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("创建 assets 目录");
    }
    fs::write(&path, &out).expect("写入 app.ico");
    println!("写出 {}（{} 字节）", path.display(), out.len());
}

/// 把自上而下的 RGBA 包成 ICO 需要的 DIB：BITMAPINFOHEADER + 自下而上的
/// BGRA 像素 + 1bpp AND 掩码。
fn dib_bits(size: i32, rgba: &[u8]) -> Vec<u8> {
    let width = usize::try_from(size).expect("尺寸为正");
    let xor_len = width * width * 4;
    let mask_stride = width.div_ceil(32) * 4;
    let mask_len = mask_stride * width;

    let mut out = Vec::with_capacity(40 + xor_len + mask_len);

    // BITMAPINFOHEADER：高度要写两倍（XOR + AND 两张图叠在一起）。
    out.extend_from_slice(&40u32.to_le_bytes());
    out.extend_from_slice(&size.to_le_bytes());
    out.extend_from_slice(&(size * 2).to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&32u16.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes()); // BI_RGB
    out.extend_from_slice(&((xor_len + mask_len) as u32).to_le_bytes());
    out.extend_from_slice(&0i32.to_le_bytes()); // 水平 DPI，0 表示未指定
    out.extend_from_slice(&0i32.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());

    // XOR 位图：自下而上，BGRA。
    for y in (0..width).rev() {
        for x in 0..width {
            let index = (y * width + x) * 4;
            out.push(rgba[index + 2]);
            out.push(rgba[index + 1]);
            out.push(rgba[index]);
            out.push(rgba[index + 3]);
        }
    }

    // AND 掩码：全 0，透明度交给 alpha 通道。
    out.resize(40 + xor_len + mask_len, 0);
    out
}
