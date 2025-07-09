use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use rand_distr::{Distribution, Normal};

#[derive(Debug, Clone)]
pub struct LoadPattern {
    max_rps: f64,
    target_avg: f64,
    base_level: f64,
    spike_chance: f64,
    max_spike_chance: f64,
    noise_factor: f64,
    last_spike_time: f64,
    current_spike: Option<(f64, f64, u32)>, // (magnitude, start_time, duration)
}

impl LoadPattern {
    pub fn new(min_rps: f64, max_rps: f64, target_avg: f64) -> anyhow::Result<Self> {
        if target_avg < min_rps || target_avg > max_rps {
            anyhow::bail!("Target average must be between min and max RPS");
        }

        let pattern = LoadPattern {
            max_rps,
            target_avg,
            base_level: target_avg * 0.5, // Lower base to allow more dramatic spikes
            spike_chance: 0.05,
            max_spike_chance: 0.02,
            noise_factor: 0.1,
            last_spike_time: -100.0,
            current_spike: None,
        };

        Ok(pattern)
    }

    pub fn calibrate(&mut self, duration_seconds: u32) {
        log::info!("Starting calibration...");

        const MAX_ITERATIONS: usize = 50;
        const TARGET_ACCURACY: f64 = 0.01;
        const TARGET_MAX_HITS: f64 = 0.005;
        const SPIKE_HITS: f64 = 0.2;

        let mut best_params = self.clone();
        let mut best_error = f64::MAX;
        let mut iteration = 0;

        while iteration < MAX_ITERATIONS {
            iteration += 1;

            let timeline = self.generate_timeline(duration_seconds);
            let actual_avg = timeline.iter().sum::<u64>() as f64 / duration_seconds as f64;
            let max_hits = timeline
                .iter()
                .filter(|&&x| x >= (self.max_rps * 0.995) as u64)
                .count() as f64
                / duration_seconds as f64;

            let spike_hits = timeline
                .iter()
                .filter(|&&x| x >= self.target_avg as _)
                .count() as f64
                / duration_seconds as f64;

            let avg_error = ((actual_avg - self.target_avg) / self.target_avg).abs();
            let max_hits_error = (max_hits - TARGET_MAX_HITS).abs();
            let combined_error = avg_error + max_hits_error;

            log::info!("\nIteration {iteration}:");
            log::info!(
                "  Actual avg: {:.1} (target: {:.1})",
                actual_avg,
                self.target_avg
            );
            log::info!(
                "  Max hits: {:.3}% (target: {:.3}%)",
                max_hits * 100.0,
                TARGET_MAX_HITS * 100.0
            );
            log::info!(
                "  Spike hits: {:.3}% (target: {}%)",
                spike_hits * 100.0,
                SPIKE_HITS * 100.0
            );
            log::info!(
                "  Parameters: base={:.1}, spike={:.4}, max_spike={:.4}",
                self.base_level,
                self.spike_chance,
                self.max_spike_chance
            );
            log::info!("  Error: {:.4}", combined_error);

            if combined_error < best_error {
                best_error = combined_error;
                best_params = self.clone();
            }

            if avg_error <= TARGET_ACCURACY && (max_hits_error <= TARGET_MAX_HITS) {
                log::info!("\nTarget accuracy reached!");
                break;
            }

            let avg_ratio = self.target_avg / actual_avg;
            self.base_level *= avg_ratio.powf(0.5);

            if max_hits < TARGET_MAX_HITS {
                self.max_spike_chance *= 1.2;
            } else {
                self.max_spike_chance *= 0.8;
            }
            if spike_hits < SPIKE_HITS {
                self.spike_chance *= 1.2;
            } else {
                self.spike_chance *= 0.8;
            }

            self.spike_chance = self.spike_chance.clamp(0.02, 0.6);
            self.max_spike_chance = self.max_spike_chance.clamp(0.01, 0.2);
            self.base_level = self
                .base_level
                .clamp(self.target_avg * 0.01, self.target_avg * 0.7);
        }

        if iteration >= MAX_ITERATIONS {
            log::info!("\nMax iterations reached. Using best found parameters.");
            *self = best_params;
        }

        log::info!("\nFinal calibration results:");
        log::info!("Base level: {:.1} RPS", self.base_level);
        log::info!("Spike chance: {:.4}", self.spike_chance);
        log::info!("Max spike chance: {:.4}", self.max_spike_chance);
    }

    fn determine_spike_type(&self, time_seconds: f64, rng: &mut StdRng) -> Option<(f64, u32)> {
        let time_since_last_spike = time_seconds - self.last_spike_time;

        let base_multiplier = if time_since_last_spike > 60.0 {
            2.0
        } else {
            1.0
        };

        if rng.gen::<f64>() < self.max_spike_chance * base_multiplier {
            return Some((1.0, rng.gen_range(3..8))); // Max spike lasts 3-8 seconds
        }

        if rng.gen::<f64>() < self.spike_chance * base_multiplier {
            let spike_type = rng.gen::<f64>();
            let (magnitude, duration) = match spike_type {
                x if x < 0.3 => (rng.gen_range(0.5..0.7), rng.gen_range(2..5)), // Medium spike
                x if x < 0.6 => (rng.gen_range(0.7..0.9), rng.gen_range(3..6)), // Large spike
                _ => (rng.gen_range(0.9..1.0), rng.gen_range(4..8)),            // Very large spike
            };
            return Some((magnitude, duration));
        }

        None
    }

    fn get_tps(&mut self, time_seconds: f64) -> u64 {
        let mut rng = StdRng::seed_from_u64(time_seconds as u64);

        let normal = Normal::new(0.0, 0.1).unwrap();
        let walk = normal.sample(&mut rng);
        let current_base = self.base_level * (1.0 + walk * 0.2);

        let spike_value = if let Some((magnitude, start_time, duration)) = self.current_spike {
            if time_seconds < start_time + duration as f64 {
                let progress = (time_seconds - start_time) / duration as f64;
                let decay = 1.0 - progress.powf(2.0);
                (self.max_rps - current_base) * magnitude * decay
            } else {
                self.current_spike = None;
                0.0
            }
        } else if let Some((magnitude, duration)) =
            self.determine_spike_type(time_seconds, &mut rng)
        {
            self.last_spike_time = time_seconds;
            self.current_spike = Some((magnitude, time_seconds, duration));
            (self.max_rps - current_base) * magnitude
        } else {
            0.0
        };

        let noise = (rng.gen::<f64>() * 2.0 - 1.0) * self.noise_factor;
        let final_rps = current_base + spike_value;
        let noisy_rps = final_rps * (1.0 + noise);

        noisy_rps.round().clamp(0.0, self.max_rps) as u64
    }

    pub fn generate_timeline(&mut self, duration_seconds: u32) -> Vec<u64> {
        (0..duration_seconds)
            .map(|sec| self.get_tps(sec as f64))
            .collect()
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use log::LevelFilter;

    #[test]
    fn test_load_pattern() {
        env_logger::Builder::new()
            .is_test(true)
            .filter_level(LevelFilter::Debug)
            .try_init()
            .unwrap();
        let mut pattern = LoadPattern::new(1.0, 100.0, 50.0).unwrap();
        pattern.calibrate(3600);

        let timeline = pattern.generate_timeline(3600);

        let total: u64 = timeline.iter().sum();
        let avg = total as f64 / 3600.0;
        assert!((avg - 50.0).abs() < 5.0);
    }

    #[test]
    fn print_csv() {
        env_logger::Builder::new()
            .is_test(true)
            .filter_level(LevelFilter::Debug)
            .try_init()
            .unwrap();
        let mut pattern = LoadPattern::new(200.0, 10000.0, 1000.0).unwrap();
        pattern.calibrate(3600);
        let timeline = pattern.generate_timeline(3600);
        println!("iteration,requests_per_second");
        for (i, rps) in timeline.iter().enumerate() {
            println!("{i},{rps}");
        }
    }
}
