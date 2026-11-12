/// Match Python's `round(value, 2)`, including rounding the actual binary float.
/// Multiplying by 100 before rounding would change cases such as 2.675.
pub fn calc_total(subtotal: f64, percent: i32) -> f64 {
    let total = if percent == 0 {
        subtotal
    } else {
        subtotal * (1.0 - f64::from(percent) / 100.0)
    };
    format!("{total:.2}").parse().unwrap_or(total)
}

pub fn line_total(price: f64, quantity: i32) -> f64 {
    price * f64::from(quantity)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rounds_binary_floats_like_python() {
        assert_eq!(calc_total(2.675, 0), 2.67);
        assert_eq!(calc_total(2.685, 0), 2.69);
        assert_eq!(calc_total(1.005, 0), 1.0);
        assert_eq!(calc_total(1.125, 0), 1.12);
        assert_eq!(calc_total(1.375, 0), 1.38);
        assert_eq!(calc_total(100.0, 10), 90.0);
        assert_eq!(line_total(12.5, 2), 25.0);
    }
}
