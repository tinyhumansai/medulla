//! Network graph autoscaling and interface selection helpers.

use std::collections::VecDeque;

/// btop-style adaptive scale for one network direction, in bytes per second.
///
/// The five most recent rates are retained independently of the up/down
/// counters so a scale change uses the same rolling window as btop.
#[derive(Debug, Clone)]
pub struct NetScaler {
    max: f64,
    history: VecDeque<f64>,
    above: u8,
    below: u8,
}

impl Default for NetScaler {
    fn default() -> Self {
        Self::new(10.0 * 1024.0)
    }
}

impl NetScaler {
    /// Create a scaler with its maximum clamped to btop's 10 KiB/s floor.
    pub fn new(max: f64) -> Self {
        Self {
            max: max.max(10.0 * 1024.0),
            history: VecDeque::with_capacity(5),
            above: 0,
            below: 0,
        }
    }

    /// Current graph ceiling in bytes per second.
    pub fn max(&self) -> f64 {
        self.max
    }

    /// Incorporate one sample and return the current ceiling.
    pub fn sample(&mut self, rate: f64) -> f64 {
        let rate = rate.max(0.0);
        if self.history.len() == 5 {
            self.history.pop_front();
        }
        self.history.push_back(rate);
        if rate > self.max {
            self.above = self.above.saturating_add(1);
            self.below = self.below.saturating_sub(1);
        } else if self.max > 10.0 * 1024.0 && rate < self.max / 10.0 {
            self.below = self.below.saturating_add(1);
            self.above = self.above.saturating_sub(1);
        }
        if self.above >= 5 {
            self.max = (self.average().floor() * 1.3).floor().max(10.0 * 1024.0);
            self.above = 0;
            self.below = 0;
        } else if self.below >= 5 {
            self.max = (self.average().floor() * 3.0).floor().max(10.0 * 1024.0);
            self.above = 0;
            self.below = 0;
        }
        self.max
    }

    fn average(&self) -> f64 {
        self.history.iter().sum::<f64>() / self.history.len().max(1) as f64
    }
}

/// Return the maximum shared ceiling when sync is enabled.
pub fn synced_max(download: f64, upload: f64, sync: bool) -> (f64, f64) {
    if sync {
        let max = download.max(upload);
        (max, max)
    } else {
        (download, upload)
    }
}

/// Cycle an interface index forward or backward, ignoring down interfaces.
pub fn cycle_interface(
    current: Option<&str>,
    interfaces: &[(&str, bool)],
    forward: bool,
) -> Option<String> {
    let up: Vec<_> = interfaces
        .iter()
        .filter(|(name, is_up)| *is_up && !is_loopback(name))
        .map(|(name, _)| *name)
        .collect();
    if up.is_empty() {
        return None;
    }
    let index = current.and_then(|name| up.iter().position(|candidate| *candidate == name));
    let next = match (index, forward) {
        (Some(index), true) => (index + 1) % up.len(),
        (Some(index), false) => (index + up.len() - 1) % up.len(),
        (None, true) => 0,
        (None, false) => up.len() - 1,
    };
    Some(up[next].to_owned())
}

/// Pick the first up, non-loopback interface with observed traffic.
pub fn default_interface(interfaces: &[medulla_monitor::NetworkSnapshot]) -> Option<&str> {
    interfaces
        .iter()
        .find(|interface| {
            interface.up
                && !is_loopback(&interface.interface)
                && (interface.total_received_bytes > 0
                    || interface.total_transmitted_bytes > 0
                    || interface.received_bytes_per_second > 0.0
                    || interface.transmitted_bytes_per_second > 0.0)
        })
        .map(|interface| interface.interface.as_str())
}

fn is_loopback(name: &str) -> bool {
    name == "lo" || name.starts_with("lo:") || name == "Loopback Pseudo-Interface 1"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scaler_tracks_steady_burst_idle_with_five_sample_windows() {
        let mut scale = NetScaler::new(10.0 * 1024.0);
        let steady = (0..6).map(|_| scale.sample(2_000.0)).collect::<Vec<_>>();
        assert_eq!(steady, vec![10_240.0; 6]);
        let burst = (0..5).map(|_| scale.sample(20_000.0)).collect::<Vec<_>>();
        assert_eq!(burst[..4], [10_240.0; 4]);
        assert_eq!(burst[4], 26_000.0);
        let idle = (0..5).map(|_| scale.sample(0.0)).collect::<Vec<_>>();
        assert_eq!(idle[..4], [26_000.0; 4]);
        assert_eq!(idle[4], 10_240.0);
    }

    #[test]
    fn scaler_counters_balance_instead_of_resetting_on_neutral_samples() {
        let mut scale = NetScaler::default();
        for rate in [20_000.0, 20_000.0, 20_000.0, 20_000.0, 10_240.0, 20_000.0] {
            scale.sample(rate);
        }
        assert_eq!(scale.max(), 23_462.0);
    }

    #[test]
    fn interface_cycling_skips_down_interfaces() {
        let interfaces = [("lo", true), ("eth0", false), ("wlan0", true)];
        assert_eq!(
            cycle_interface(Some("lo"), &interfaces, true).as_deref(),
            Some("wlan0")
        );
        assert_eq!(
            cycle_interface(Some("lo"), &interfaces, false).as_deref(),
            Some("wlan0")
        );
    }

    #[test]
    fn default_interface_picks_first_up_non_loopback_with_traffic() {
        let samples = vec![
            medulla_monitor::NetworkSnapshot {
                interface: "lo".into(),
                up: true,
                connected: true,
                ip_addresses: vec![],
                received_bytes_per_second: 4.0,
                transmitted_bytes_per_second: 0.0,
                total_received_bytes: 4,
                total_transmitted_bytes: 0,
            },
            medulla_monitor::NetworkSnapshot {
                interface: "eth0".into(),
                up: false,
                connected: false,
                ip_addresses: vec![],
                received_bytes_per_second: 0.0,
                transmitted_bytes_per_second: 0.0,
                total_received_bytes: 1_000_000,
                total_transmitted_bytes: 1_000_000,
            },
            medulla_monitor::NetworkSnapshot {
                interface: "wlan0".into(),
                up: true,
                connected: true,
                ip_addresses: vec![],
                received_bytes_per_second: 4.0,
                transmitted_bytes_per_second: 0.0,
                total_received_bytes: 4,
                total_transmitted_bytes: 0,
            },
            medulla_monitor::NetworkSnapshot {
                interface: "enp2s0".into(),
                up: true,
                connected: true,
                ip_addresses: vec![],
                received_bytes_per_second: 8.0,
                transmitted_bytes_per_second: 0.0,
                total_received_bytes: 50,
                total_transmitted_bytes: 0,
            },
        ];
        assert_eq!(default_interface(&samples), Some("wlan0"));
    }

    #[test]
    fn sync_uses_larger_direction_scale() {
        assert_eq!(synced_max(10.0, 20.0, true), (20.0, 20.0));
    }
}
