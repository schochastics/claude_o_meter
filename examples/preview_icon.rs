//! Print an ASCII preview of the sparkle icon for visual sanity check.

fn alpha_char(a: u8) -> char {
    match a {
        0..=20 => ' ',
        21..=80 => '.',
        81..=160 => 'o',
        161..=220 => 'O',
        _ => '#',
    }
}

// Mirror the private render so we can preview without depending on tray-icon's
// Icon (which we can't peek into). Keep these constants in sync with icons.rs.
fn render(size: u32) -> Vec<u8> {
    let mut buf = vec![0u8; (size * size * 4) as usize];
    let center = (size as f64 - 1.0) / 2.0;
    let s = size as f64;
    let cardinal_r = s * 0.50;
    let diagonal_r = s * 0.32;

    let astroid = |dx: f64, dy: f64, r: f64| -> f64 {
        let p = 0.6_f64;
        let val = (dx.abs() / r).powf(p) + (dy.abs() / r).powf(p);
        if val <= 0.85 { 1.0 }
        else if val >= 1.05 { 0.0 }
        else {
            let t = (val - 0.85) / 0.20;
            1.0 - t * t * (3.0 - 2.0 * t)
        }
    };

    let cos45 = std::f64::consts::FRAC_1_SQRT_2;
    for y in 0..size {
        for x in 0..size {
            let dx = x as f64 - center;
            let dy = y as f64 - center;
            let u = dx * cos45 + dy * cos45;
            let v = -dx * cos45 + dy * cos45;
            let a = astroid(dx, dy, cardinal_r)
                .max(astroid(u, v, diagonal_r));
            let i = ((y * size + x) * 4) as usize;
            buf[i + 3] = (a * 255.0).round() as u8;
        }
    }
    buf
}

fn main() {
    let size = 22;
    let buf = render(size);
    for y in 0..size {
        for x in 0..size {
            let a = buf[((y * size + x) * 4 + 3) as usize];
            print!("{}", alpha_char(a));
        }
        println!();
    }
}
