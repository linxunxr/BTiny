//! 托盘图标渲染：内嵌 5x7 点阵字体直接写 RGBA 缓冲，零绘图依赖（规格 4.2 实现约束）。
//!
//! 深色底块等效于描边措施，保证深浅色任务栏下均可读（FR-02 配色可见性）。

use tauri::image::Image;

pub enum IconContent {
    /// 显示电量数字（≤20 时渲染为低电量橙红色）
    Percent(u8),
    /// 有已连接设备但电量不可得（灰 --）
    Unreadable,
    /// 无已连接设备（灰 --，底块更暗）
    Empty,
}

/// 5x7 点阵：bit4..bit0 对应 5 列，7 行。
const DIGITS: [[u8; 7]; 10] = [
    [0b01110, 0b10001, 0b10011, 0b10101, 0b11001, 0b10001, 0b01110], // 0
    [0b00100, 0b01100, 0b00100, 0b00100, 0b00100, 0b00100, 0b01110], // 1
    [0b01110, 0b10001, 0b00001, 0b00010, 0b00100, 0b01000, 0b11111], // 2
    [0b11111, 0b00010, 0b00100, 0b00010, 0b00001, 0b10001, 0b01110], // 3
    [0b00010, 0b00110, 0b01010, 0b10010, 0b11111, 0b00010, 0b00010], // 4
    [0b11111, 0b10000, 0b11110, 0b00001, 0b00001, 0b10001, 0b01110], // 5
    [0b00110, 0b01000, 0b10000, 0b11110, 0b10001, 0b10001, 0b01110], // 6
    [0b11111, 0b00001, 0b00010, 0b00100, 0b01000, 0b01000, 0b01000], // 7
    [0b01110, 0b10001, 0b10001, 0b01110, 0b10001, 0b10001, 0b01110], // 8
    [0b01110, 0b10001, 0b10001, 0b01111, 0b00001, 0b00010, 0b01100], // 9
];

const DASH: [u8; 7] = [0, 0, 0, 0b01110, 0, 0, 0];

const LOW_THRESHOLD: u8 = 20;

/// 渲染 size×size 图标。factor 按尺寸自适应（16px→1x，24px+→2x 起），不硬编码像素（FR-02 DPI）。
pub fn render(size: u32, content: &IconContent) -> Image<'static> {
    let s = size.max(16) as i32;
    let factor = (((s - 2) / 11) as u32).max(1);
    let mut buf = vec![0u8; (s * s * 4) as usize];

    // 底块：深色圆角方块。空态/不可读更暗一档
    let (br, bg, bb, ba) = match content {
        IconContent::Empty | IconContent::Unreadable => (0x1E, 0x1E, 0x1E, 200),
        IconContent::Percent(_) => (0x1E, 0x1E, 0x1E, 235),
    };
    fill_round(&mut buf, s, br, bg, bb, ba, 2 * factor as i32);

    // 前景与字模：两位数字（满电 99 封顶，FR-02）或 "--"
    let (fr, fg, fb, cells) = match content {
        IconContent::Percent(v) => {
            let v = (*v).min(99);
            let color = if v <= LOW_THRESHOLD { (0xFF, 0x6A, 0x00) } else { (0xF0, 0xF0, 0xF0) };
            (color.0, color.1, color.2, [DIGITS[(v / 10) as usize], DIGITS[(v % 10) as usize]])
        }
        IconContent::Unreadable | IconContent::Empty => (0x9A, 0x9A, 0x9A, [DASH, DASH]),
    };

    // 字模区 11×7，居中
    let total_w = 11 * factor as i32;
    let ox = (s - total_w) / 2;
    let oy = (s - 7 * factor as i32) / 2;
    for (char_idx, glyph) in cells.iter().enumerate() {
        for row in 0..7i32 {
            for col in 0..5i32 {
                if glyph[row as usize] & (1 << (4 - col)) == 0 {
                    continue;
                }
                for dy in 0..factor as i32 {
                    for dx in 0..factor as i32 {
                        let x = ox + (char_idx as i32 * 6 + col) * factor as i32 + dx;
                        let y = oy + row * factor as i32 + dy;
                        put(&mut buf, s, x, y, fr, fg, fb, 255);
                    }
                }
            }
        }
    }
    Image::new_owned(buf, s as u32, s as u32)
}

fn put(buf: &mut [u8], s: i32, x: i32, y: i32, r: u8, g: u8, b: u8, a: u8) {
    if x < 0 || y < 0 || x >= s || y >= s {
        return;
    }
    let i = ((y * s + x) * 4) as usize;
    buf[i] = r;
    buf[i + 1] = g;
    buf[i + 2] = b;
    buf[i + 3] = a;
}

fn fill_round(buf: &mut [u8], s: i32, r: u8, g: u8, b: u8, a: u8, radius: i32) {
    for y in 0..s {
        for x in 0..s {
            // 四角扣圆：位于角区且到角心距离超过 radius 则跳过
            let cx = if x < radius { radius } else if x >= s - radius { s - radius - 1 } else { x };
            let cy = if y < radius { radius } else if y >= s - radius { s - radius - 1 } else { y };
            let corner = (x != cx) && (y != cy);
            if corner && (x - cx) * (x - cx) + (y - cy) * (y - cy) > radius * radius {
                continue;
            }
            put(buf, s, x, y, r, g, b, a);
        }
    }
}
