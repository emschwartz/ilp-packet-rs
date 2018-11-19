use std::cmp::{max, min};
use std::collections::HashMap;

pub struct CongestionController {
    state: CongestionState,
    increase_amount: u64,
    decrease_factor: f64,
    max_packet_amount: Option<u64>,
    amount_in_flight: u64,
    max_in_flight: u64,
    // id: amount
    packets: HashMap<u32, u64>,
}

#[derive(PartialEq)]
enum CongestionState {
    SlowStart,
    AvoidCongestion,
}

impl CongestionController {
    pub fn new(start_amount: u64, increase_amount: u64, decrease_factor: f64) -> Self {
        CongestionController {
            state: CongestionState::SlowStart,
            increase_amount,
            decrease_factor,
            max_packet_amount: None,
            amount_in_flight: 0,
            max_in_flight: start_amount,
            packets: HashMap::new(),
        }
    }

    pub fn default() -> Self {
        // TODO an increase amount of 1000 might be too small if the units are worth very little
        // should it be adjusted based on something like the max packet amount?
        Self::new(1000, 1000, 2.0)
    }

    pub fn set_max_packet_amount(&mut self, max_packet_amount: u64) {
        self.max_packet_amount = Some(max_packet_amount);
    }

    pub fn get_max_amount(&mut self) -> u64 {
        let amount_left_in_window = self.max_in_flight - self.amount_in_flight;
        if let Some(max_packet_amount) = self.max_packet_amount {
            min(amount_left_in_window, max_packet_amount)
        } else {
            amount_left_in_window
        }
    }

    pub fn prepare(&mut self, id: u32, amount: u64) {
        if amount > 0 {
            self.amount_in_flight += amount;
            self.packets.insert(id, amount);
            debug!(
                "Prepare packet of {}, amount in flight is now: {}",
                amount, self.amount_in_flight
            );
        }
    }

    pub fn fulfill(&mut self, id: u32) {
        if let Some(amount) = self.packets.remove(&id) {
            self.amount_in_flight -= amount;

            // Before we know how much we should be sending at a time,
            // double the window size on every successful packet.
            // Once we start getting errors, switch to Additive Increase,
            // Multiplicative Decrease (AIMD) congestion avoidance
            if self.state == CongestionState::SlowStart {
                self.max_in_flight *= 2;
                debug!(
                    "Fulfilled packet of {}, doubling max in flight to: {}",
                    amount, self.max_in_flight
                );
            } else {
                self.max_in_flight += self.increase_amount;
                debug!(
                    "Fulfilled packet of {}, increasing max in flight to: {}",
                    amount, self.max_in_flight
                );
            }
        }
    }

    pub fn reject(&mut self, id: u32, error_code: &str) {
        if let Some(amount) = self.packets.remove(&id) {
            self.amount_in_flight -= amount;

            if error_code == "T04" {
                self.state = CongestionState::AvoidCongestion;
                self.max_in_flight = max(
                    (self.max_in_flight as f64 / self.decrease_factor).floor() as u64,
                    1,
                );
                debug!("Rejected packet with T04 error. Amount in flight was: {}, decreasing max in flight to: {}", self.amount_in_flight + amount, self.max_in_flight);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    mod slow_start {
        use super::*;

        #[test]
        fn doubles_max_amount_on_fulfill() {
            let mut controller = CongestionController::new(1000, 1000, 2.0);

            let amount = controller.get_max_amount();
            controller.prepare(1, amount);
            controller.fulfill(1);
            assert_eq!(controller.get_max_amount(), 2000);

            let amount = controller.get_max_amount();
            controller.prepare(2, amount);
            controller.fulfill(2);
            assert_eq!(controller.get_max_amount(), 4000);

            let amount = controller.get_max_amount();
            controller.prepare(3, amount);
            controller.fulfill(3);
            assert_eq!(controller.get_max_amount(), 8000);
        }
    }

    mod congestion_avoidance {
        use super::*;

        #[test]
        fn additive_increase() {
            let mut controller = CongestionController::new(1000, 1000, 2.0);
            controller.state = CongestionState::AvoidCongestion;
            for i in 1..5 {
                controller.prepare(i as u32, i * 1000);
                controller.fulfill(i as u32);
                assert_eq!(controller.get_max_amount(), 1000 + i * 1000);
            }
        }

        #[test]
        fn multiplicative_decrease() {
            let mut controller = CongestionController::new(1000, 1000, 2.0);
            controller.state = CongestionState::AvoidCongestion;

            let amount = controller.get_max_amount();
            controller.prepare(1, amount);
            controller.reject(1, "T04");
            assert_eq!(controller.get_max_amount(), 500);

            let amount = controller.get_max_amount();
            controller.prepare(2, amount);
            controller.reject(2, "T04");
            assert_eq!(controller.get_max_amount(), 250);
        }

        #[test]
        fn aimd_combined() {
            let mut controller = CongestionController::new(1000, 1000, 2.0);
            controller.state = CongestionState::AvoidCongestion;

            let amount = controller.get_max_amount();
            controller.prepare(1, amount);
            controller.fulfill(1);
            assert_eq!(controller.get_max_amount(), 2000);

            let amount = controller.get_max_amount();
            controller.prepare(2, amount);
            controller.fulfill(2);
            assert_eq!(controller.get_max_amount(), 3000);

            let amount = controller.get_max_amount();
            controller.prepare(3, amount);
            controller.reject(3, "T04");
            assert_eq!(controller.get_max_amount(), 1500);

            let amount = controller.get_max_amount();
            controller.prepare(4, amount);
            controller.fulfill(4);
            assert_eq!(controller.get_max_amount(), 2500);
        }

        #[test]
        fn max_packet_amount() {
            let mut controller = CongestionController::new(1000, 1000, 2.0);
            controller.set_max_packet_amount(100);

            assert_eq!(controller.get_max_amount(), 100);

            let amount = controller.get_max_amount();
            controller.prepare(1, amount);
            controller.fulfill(1);
            assert_eq!(controller.get_max_amount(), 100);

            let amount = controller.get_max_amount();
            controller.prepare(2, amount);
            controller.fulfill(2);
            assert_eq!(controller.get_max_amount(), 100);
        }
    }

    mod tracking_amount_in_flight {
        use super::*;

        #[test]
        fn tracking_amount_in_flight() {
            let mut controller = CongestionController::new(1000, 1000, 2.0);
            controller.set_max_packet_amount(600);
            assert_eq!(controller.get_max_amount(), 600);

            controller.prepare(1, 100);
            assert_eq!(controller.get_max_amount(), 600);

            controller.prepare(2, 600);
            assert_eq!(controller.get_max_amount(), 1000 - 700);
        }
    }
}
