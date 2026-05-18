use std::time::Instant;

pub fn elapsed_ms(start: Instant) -> u64 {
    start.elapsed().as_millis().try_into().unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::elapsed_ms;

    #[test]
    fn elapsed_ms_reports_milliseconds() {
        let start = Instant::now() - Duration::from_millis(7);

        assert!(elapsed_ms(start) >= 7);
    }
}
