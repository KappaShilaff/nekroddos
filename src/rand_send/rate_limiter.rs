use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use rand_distr::{Distribution, Normal};

#[derive(Debug, Clone)]
pub struct LoadPattern {
    min_rps: f64,
    max_rps: f64,
    target_avg: f64,
    base_level: f64,
    spike_chance: f64,
    max_spike_chance: f64,
    noise_factor: f64,
    last_spike_time: f64,
}

impl LoadPattern {
    pub fn new(min_rps: f64, max_rps: f64, target_avg: f64) -> anyhow::Result<Self> {
        if target_avg < min_rps || target_avg > max_rps {
            anyhow::bail!("Target average must be between min and max RPS");
        }

        let pattern = LoadPattern {
            min_rps,
            max_rps,
            target_avg,
            base_level: target_avg * 0.7, // Lower base to allow more spikes
            spike_chance: 0.03,
            max_spike_chance: 0.01,
            noise_factor: 0.1,
            last_spike_time: -100.0, // Initialize to allow immediate spikes
        };

        Ok(pattern)
    }

    pub fn calibrate(&mut self, duration_seconds: u32) {
        log::info!("Starting calibration...");

        const MAX_ITERATIONS: usize = 50; // Maximum iterations to prevent infinite loops
        const TARGET_ACCURACY: f64 = 0.01; // Target within 1% of the desired average
        const TARGET_MAX_HITS: f64 = 0.002; // Target 0.2% of the time at max

        let mut best_params = self.clone();
        let mut best_error = f64::MAX;
        let mut iteration = 0;

        while iteration < MAX_ITERATIONS {
            iteration += 1;

            // Generate timeline and calculate metrics
            let timeline = self.generate_timeline(duration_seconds);
            let actual_avg = timeline.iter().sum::<u64>() as f64 / duration_seconds as f64;
            let max_hits = timeline
                .iter()
                .filter(|&&x| x >= (self.max_rps * 0.995) as u64)
                .count() as f64
                / duration_seconds as f64;

            // Calculate error metrics
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
                "  Parameters: base={:.1}, spike={:.4}, max_spike={:.4}",
                self.base_level,
                self.spike_chance,
                self.max_spike_chance
            );
            log::info!("  Error: {:.4}", combined_error);

            // Save best parameters
            if combined_error < best_error {
                best_error = combined_error;
                best_params = self.clone();
            }

            // Check if we've reached the desired accuracy
            if avg_error <= TARGET_ACCURACY && (max_hits_error <= TARGET_MAX_HITS) {
                log::info!("\nTarget accuracy reached!");
                break;
            }

            // Adjust parameters
            let avg_ratio = self.target_avg / actual_avg;

            // Adjust base level based on average error
            self.base_level *= avg_ratio.powf(0.5);

            // Adjust spike probabilities based on max hits
            if max_hits < TARGET_MAX_HITS {
                self.max_spike_chance *= 1.2;
                self.spike_chance *= 1.1;
            } else {
                self.max_spike_chance *= 0.8;
                self.spike_chance *= 0.9;
            }

            // Keep probabilities in reasonable bounds
            self.spike_chance = self.spike_chance.clamp(0.01, 0.1);
            self.max_spike_chance = self.max_spike_chance.clamp(0.001, 0.03);
            self.base_level = self
                .base_level
                .clamp(self.target_avg * 0.5, self.target_avg * 0.9);
        }

        // Restore best parameters if we didn't converge
        if iteration >= MAX_ITERATIONS {
            log::info!("\nMax iterations reached. Using best found parameters.");
            *self = best_params;
        }

        log::info!("\nFinal calibration results:");
        log::info!("Base level: {:.1} RPS", self.base_level);
        log::info!("Spike chance: {:.4}", self.spike_chance);
        log::info!("Max spike chance: {:.4}", self.max_spike_chance);
    }

    fn determine_spike_type(&self, time_seconds: f64, rng: &mut StdRng) -> Option<f64> {
        let time_since_last_spike = time_seconds - self.last_spike_time;

        // Increased chance for spikes if we haven't had one recently
        let base_multiplier = if time_since_last_spike > 60.0 {
            1.5
        } else {
            1.0
        };

        // Chance for maximum spike
        if rng.gen::<f64>() < self.max_spike_chance * base_multiplier {
            return Some(1.0); // Maximum spike
        }

        // Chance for regular spike with varying magnitudes
        if rng.gen::<f64>() < self.spike_chance * base_multiplier {
            // Generate different spike types
            let spike_type = rng.gen::<f64>();
            return Some(match spike_type {
                x if x < 0.4 => rng.gen_range(0.3..0.5), // Medium spike
                x if x < 0.7 => rng.gen_range(0.5..0.8), // Large spike
                _ => rng.gen_range(0.8..0.95),           // Very large spike
            });
        }

        None
    }

    fn get_tps(&mut self, time_seconds: f64) -> u64 {
        let mut rng = StdRng::seed_from_u64(time_seconds as u64);

        // Base load with a random walk
        let normal = Normal::new(0.0, 0.1).unwrap();
        let walk = normal.sample(&mut rng);
        let current_base = self.base_level * (1.0 + walk * 0.2);

        // Check for spike
        let spike_value = if let Some(magnitude) = self.determine_spike_type(time_seconds, &mut rng)
        {
            self.last_spike_time = time_seconds;
            let spike_height = (self.max_rps - current_base) * magnitude;

            // Chance for aftershock
            if rng.gen::<f64>() < 0.4 {
                spike_height * rng.gen_range(0.4..0.8)
            } else {
                spike_height
            }
        } else {
            0.0
        };

        // Add noise to the combined value
        let noise = (rng.gen::<f64>() * 2.0 - 1.0) * self.noise_factor;
        let final_rps = current_base + spike_value;
        let noisy_rps = final_rps * (1.0 + noise);

        // Ensure we stay within bounds
        noisy_rps.round().clamp(self.min_rps, self.max_rps) as u64
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
}
