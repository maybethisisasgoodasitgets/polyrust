/// Strategy Filters Module
/// 
/// This module contains profitable trading filters that work together to identify
/// high-probability trading opportunities while avoiding noise and mean reversion.
/// 
/// Each filter is independently testable and can be enabled/disabled.

use chrono::{DateTime, Utc, Timelike};
use std::time::Instant;

// ============================================================================
// Configuration
// ============================================================================

/// Minimum momentum score to consider a signal valid
pub const MIN_MOMENTUM_SCORE: f64 = 0.4;

/// Minimum momentum consistency (% of moves in same direction)
pub const MIN_MOMENTUM_CONSISTENCY: f64 = 0.8;

/// Minimum orderbook depth in USD to avoid thin markets
pub const MIN_ORDERBOOK_DEPTH_USD: f64 = 500.0;

/// Volume surge multiplier (current volume vs average)
pub const VOLUME_SURGE_MULTIPLIER: f64 = 2.0;

/// Trading hours (EST): 9am-4pm
pub const TRADING_START_HOUR_EST: u32 = 9;
pub const TRADING_END_HOUR_EST: u32 = 16;

// ============================================================================
// Filter Results
// ============================================================================

#[derive(Debug, Clone, PartialEq)]
pub enum FilterResult {
    Pass,
    Fail(String),
}

impl FilterResult {
    pub fn passed(&self) -> bool {
        matches!(self, FilterResult::Pass)
    }

    pub fn reason(&self) -> Option<&str> {
        match self {
            FilterResult::Pass => None,
            FilterResult::Fail(r) => Some(r),
        }
    }
}

// ============================================================================
// 1. Smart Momentum Filter
// ============================================================================

#[derive(Debug, Clone)]
pub struct MomentumFilterConfig {
    pub min_score: f64,
    pub min_consistency: f64,
    pub require_acceleration: bool,
}

impl Default for MomentumFilterConfig {
    fn default() -> Self {
        Self {
            min_score: MIN_MOMENTUM_SCORE,
            min_consistency: MIN_MOMENTUM_CONSISTENCY,
            require_acceleration: true,
        }
    }
}

/// Smart Momentum Filter
/// Only passes signals with strong, consistent, accelerating momentum
pub struct SmartMomentumFilter {
    config: MomentumFilterConfig,
}

impl SmartMomentumFilter {
    pub fn new(config: MomentumFilterConfig) -> Self {
        Self { config }
    }

    pub fn check(
        &self,
        momentum_score: f64,
        consistency: f64,
        is_accelerating: bool,
        direction_matches: bool,
    ) -> FilterResult {
        if !direction_matches {
            return FilterResult::Fail("Momentum direction doesn't match price direction".to_string());
        }

        if momentum_score.abs() < self.config.min_score {
            return FilterResult::Fail(format!(
                "Momentum too weak: {:.2} < {:.2}",
                momentum_score.abs(),
                self.config.min_score
            ));
        }

        if consistency < self.config.min_consistency {
            return FilterResult::Fail(format!(
                "Momentum not consistent: {:.2} < {:.2}",
                consistency,
                self.config.min_consistency
            ));
        }

        if self.config.require_acceleration && !is_accelerating {
            return FilterResult::Fail("Momentum is decelerating".to_string());
        }

        FilterResult::Pass
    }
}

// ============================================================================
// 2. Orderbook Depth Filter
// ============================================================================

#[derive(Debug, Clone)]
pub struct OrderbookFilterConfig {
    pub min_depth_usd: f64,
    pub check_both_sides: bool,
}

impl Default for OrderbookFilterConfig {
    fn default() -> Self {
        Self {
            min_depth_usd: MIN_ORDERBOOK_DEPTH_USD,
            check_both_sides: false,
        }
    }
}

/// Orderbook depth data
#[derive(Debug, Clone)]
pub struct OrderbookDepth {
    pub bid_depth_usd: f64,
    pub ask_depth_usd: f64,
    pub spread_pct: f64,
}

/// Orderbook Depth Filter
/// Ensures sufficient liquidity to avoid slippage and mean reversion
pub struct OrderbookDepthFilter {
    config: OrderbookFilterConfig,
}

impl OrderbookDepthFilter {
    pub fn new(config: OrderbookFilterConfig) -> Self {
        Self { config }
    }

    pub fn check(&self, depth: &OrderbookDepth, buying_yes: bool) -> FilterResult {
        let relevant_depth = if buying_yes {
            depth.ask_depth_usd
        } else {
            depth.bid_depth_usd
        };

        if relevant_depth < self.config.min_depth_usd {
            return FilterResult::Fail(format!(
                "Insufficient orderbook depth: ${:.0} < ${:.0}",
                relevant_depth, self.config.min_depth_usd
            ));
        }

        if self.config.check_both_sides {
            let other_side = if buying_yes {
                depth.bid_depth_usd
            } else {
                depth.ask_depth_usd
            };

            if other_side < self.config.min_depth_usd * 0.5 {
                return FilterResult::Fail(format!(
                    "Other side too thin: ${:.0} < ${:.0}",
                    other_side,
                    self.config.min_depth_usd * 0.5
                ));
            }
        }

        FilterResult::Pass
    }
}

// ============================================================================
// 3. Volume Surge Filter
// ============================================================================

#[derive(Debug, Clone)]
pub struct VolumeSurgeFilterConfig {
    pub surge_multiplier: f64,
    pub min_current_volume: f64,
}

impl Default for VolumeSurgeFilterConfig {
    fn default() -> Self {
        Self {
            surge_multiplier: VOLUME_SURGE_MULTIPLIER,
            min_current_volume: 1000.0,
        }
    }
}

/// Volume surge data
#[derive(Debug, Clone)]
pub struct VolumeData {
    pub current_volume: f64,
    pub average_volume: f64,
}

/// Volume Surge Filter
/// Only trades when volume confirms the move is real
pub struct VolumeSurgeFilter {
    config: VolumeSurgeFilterConfig,
}

impl VolumeSurgeFilter {
    pub fn new(config: VolumeSurgeFilterConfig) -> Self {
        Self { config }
    }

    pub fn check(&self, volume: &VolumeData) -> FilterResult {
        if volume.current_volume < self.config.min_current_volume {
            return FilterResult::Fail(format!(
                "Volume too low: {:.0} < {:.0}",
                volume.current_volume, self.config.min_current_volume
            ));
        }

        if volume.average_volume > 0.0 {
            let surge_ratio = volume.current_volume / volume.average_volume;
            if surge_ratio < self.config.surge_multiplier {
                return FilterResult::Fail(format!(
                    "No volume surge: {:.1}x < {:.1}x",
                    surge_ratio, self.config.surge_multiplier
                ));
            }
        }

        FilterResult::Pass
    }
}

// ============================================================================
// 4. Time-of-Day Filter
// ============================================================================

#[derive(Debug, Clone)]
pub struct TimeFilterConfig {
    pub start_hour_est: u32,
    pub end_hour_est: u32,
    pub allow_weekends: bool,
}

impl Default for TimeFilterConfig {
    fn default() -> Self {
        Self {
            start_hour_est: TRADING_START_HOUR_EST,
            end_hour_est: TRADING_END_HOUR_EST,
            allow_weekends: false,
        }
    }
}

/// Time-of-Day Filter
/// Only trades during high-volatility hours
pub struct TimeOfDayFilter {
    config: TimeFilterConfig,
}

impl TimeOfDayFilter {
    pub fn new(config: TimeFilterConfig) -> Self {
        Self { config }
    }

    pub fn check(&self, time: DateTime<Utc>) -> FilterResult {
        let est_offset = chrono::FixedOffset::west_opt(5 * 3600).unwrap();
        let est_time = time.with_timezone(&est_offset);
        let hour = est_time.hour();

        if hour < self.config.start_hour_est || hour >= self.config.end_hour_est {
            return FilterResult::Fail(format!(
                "Outside trading hours: {}:00 EST (allowed: {}:00-{}:00)",
                hour, self.config.start_hour_est, self.config.end_hour_est
            ));
        }

        if !self.config.allow_weekends {
            let weekday = est_time.weekday();
            if weekday == chrono::Weekday::Sat || weekday == chrono::Weekday::Sun {
                return FilterResult::Fail(format!("Weekend trading disabled: {:?}", weekday));
            }
        }

        FilterResult::Pass
    }
}

// ============================================================================
// Combined Strategy Filter
// ============================================================================

#[derive(Debug, Clone)]
pub struct StrategyConfig {
    pub momentum: MomentumFilterConfig,
    pub orderbook: OrderbookFilterConfig,
    pub volume: VolumeSurgeFilterConfig,
    pub time: TimeFilterConfig,
    pub enable_momentum: bool,
    pub enable_orderbook: bool,
    pub enable_volume: bool,
    pub enable_time: bool,
}

impl Default for StrategyConfig {
    fn default() -> Self {
        Self {
            momentum: MomentumFilterConfig::default(),
            orderbook: OrderbookFilterConfig::default(),
            volume: VolumeSurgeFilterConfig::default(),
            time: TimeFilterConfig::default(),
            enable_momentum: true,
            enable_orderbook: true,
            enable_volume: false,
            enable_time: true,
        }
    }
}

#[derive(Debug)]
pub struct FilterResults {
    pub momentum: Option<FilterResult>,
    pub orderbook: Option<FilterResult>,
    pub volume: Option<FilterResult>,
    pub time: Option<FilterResult>,
}

impl FilterResults {
    pub fn all_passed(&self) -> bool {
        let checks = [
            &self.momentum,
            &self.orderbook,
            &self.volume,
            &self.time,
        ];

        checks.iter().all(|r| match r {
            Some(result) => result.passed(),
            None => true,
        })
    }

    pub fn failure_reasons(&self) -> Vec<String> {
        let mut reasons = Vec::new();

        if let Some(FilterResult::Fail(r)) = &self.momentum {
            reasons.push(format!("Momentum: {}", r));
        }
        if let Some(FilterResult::Fail(r)) = &self.orderbook {
            reasons.push(format!("Orderbook: {}", r));
        }
        if let Some(FilterResult::Fail(r)) = &self.volume {
            reasons.push(format!("Volume: {}", r));
        }
        if let Some(FilterResult::Fail(r)) = &self.time {
            reasons.push(format!("Time: {}", r));
        }

        reasons
    }

    pub fn format_for_telegram(&self) -> String {
        let mut msg = String::new();
        
        msg.push_str("📊 Filter Results:\n\n");
        
        if let Some(result) = &self.momentum {
            let icon = if result.passed() { "✅" } else { "❌" };
            msg.push_str(&format!("{} Momentum: {}\n", icon, 
                if result.passed() { "PASS" } else { result.reason().unwrap_or("FAIL") }
            ));
        }
        
        if let Some(result) = &self.orderbook {
            let icon = if result.passed() { "✅" } else { "❌" };
            msg.push_str(&format!("{} Orderbook: {}\n", icon,
                if result.passed() { "PASS" } else { result.reason().unwrap_or("FAIL") }
            ));
        }
        
        if let Some(result) = &self.volume {
            let icon = if result.passed() { "✅" } else { "❌" };
            msg.push_str(&format!("{} Volume: {}\n", icon,
                if result.passed() { "PASS" } else { result.reason().unwrap_or("FAIL") }
            ));
        }
        
        if let Some(result) = &self.time {
            let icon = if result.passed() { "✅" } else { "❌" };
            msg.push_str(&format!("{} Time: {}\n", icon,
                if result.passed() { "PASS" } else { result.reason().unwrap_or("FAIL") }
            ));
        }
        
        if self.all_passed() {
            msg.push_str("\n🎯 All filters PASSED - Taking trade\n");
        } else {
            msg.push_str("\n🚫 Trade REJECTED\n");
        }
        
        msg
    }
}

pub struct StrategyFilter {
    pub momentum_filter: SmartMomentumFilter,
    pub orderbook_filter: OrderbookDepthFilter,
    pub volume_filter: VolumeSurgeFilter,
    pub time_filter: TimeOfDayFilter,
    pub config: StrategyConfig,
}

impl StrategyFilter {
    pub fn new(config: StrategyConfig) -> Self {
        Self {
            momentum_filter: SmartMomentumFilter::new(config.momentum.clone()),
            orderbook_filter: OrderbookDepthFilter::new(config.orderbook.clone()),
            volume_filter: VolumeSurgeFilter::new(config.volume.clone()),
            time_filter: TimeOfDayFilter::new(config.time.clone()),
            config,
        }
    }

    pub fn check_all(
        &self,
        momentum_score: f64,
        consistency: f64,
        is_accelerating: bool,
        direction_matches: bool,
        orderbook: Option<&OrderbookDepth>,
        volume: Option<&VolumeData>,
        time: DateTime<Utc>,
        buying_yes: bool,
    ) -> FilterResults {
        FilterResults {
            momentum: if self.config.enable_momentum {
                Some(self.momentum_filter.check(
                    momentum_score,
                    consistency,
                    is_accelerating,
                    direction_matches,
                ))
            } else {
                None
            },
            orderbook: if self.config.enable_orderbook {
                orderbook.map(|ob| self.orderbook_filter.check(ob, buying_yes))
            } else {
                None
            },
            volume: if self.config.enable_volume {
                volume.map(|v| self.volume_filter.check(v))
            } else {
                None
            },
            time: if self.config.enable_time {
                Some(self.time_filter.check(time))
            } else {
                None
            },
        }
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_momentum_filter_pass_strong_momentum() {
        let filter = SmartMomentumFilter::new(MomentumFilterConfig::default());
        let result = filter.check(0.5, 0.85, true, true);
        assert!(result.passed(), "Strong momentum should pass");
    }

    #[test]
    fn test_momentum_filter_fail_weak_momentum() {
        let filter = SmartMomentumFilter::new(MomentumFilterConfig::default());
        let result = filter.check(0.2, 0.85, true, true);
        assert!(!result.passed(), "Weak momentum should fail");
        assert!(result.reason().unwrap().contains("too weak"));
    }

    #[test]
    fn test_momentum_filter_fail_inconsistent() {
        let filter = SmartMomentumFilter::new(MomentumFilterConfig::default());
        let result = filter.check(0.5, 0.5, true, true);
        assert!(!result.passed(), "Inconsistent momentum should fail");
        assert!(result.reason().unwrap().contains("not consistent"));
    }

    #[test]
    fn test_momentum_filter_fail_decelerating() {
        let filter = SmartMomentumFilter::new(MomentumFilterConfig::default());
        let result = filter.check(0.5, 0.85, false, true);
        assert!(!result.passed(), "Decelerating momentum should fail");
        assert!(result.reason().unwrap().contains("decelerating"));
    }

    #[test]
    fn test_momentum_filter_fail_wrong_direction() {
        let filter = SmartMomentumFilter::new(MomentumFilterConfig::default());
        let result = filter.check(0.5, 0.85, true, false);
        assert!(!result.passed(), "Wrong direction should fail");
        assert!(result.reason().unwrap().contains("direction"));
    }

    #[test]
    fn test_momentum_filter_acceleration_optional() {
        let config = MomentumFilterConfig {
            require_acceleration: false,
            ..Default::default()
        };
        let filter = SmartMomentumFilter::new(config);
        let result = filter.check(0.5, 0.85, false, true);
        assert!(result.passed(), "Should pass when acceleration not required");
    }

    #[test]
    fn test_orderbook_filter_pass_sufficient_depth() {
        let filter = OrderbookDepthFilter::new(OrderbookFilterConfig::default());
        let depth = OrderbookDepth {
            bid_depth_usd: 1000.0,
            ask_depth_usd: 800.0,
            spread_pct: 0.02,
        };
        let result = filter.check(&depth, true);
        assert!(result.passed(), "Sufficient depth should pass");
    }

    #[test]
    fn test_orderbook_filter_fail_insufficient_depth() {
        let filter = OrderbookDepthFilter::new(OrderbookFilterConfig::default());
        let depth = OrderbookDepth {
            bid_depth_usd: 1000.0,
            ask_depth_usd: 200.0,
            spread_pct: 0.02,
        };
        let result = filter.check(&depth, true);
        assert!(!result.passed(), "Insufficient ask depth should fail");
        assert!(result.reason().unwrap().contains("Insufficient"));
    }

    #[test]
    fn test_orderbook_filter_checks_correct_side() {
        let filter = OrderbookDepthFilter::new(OrderbookFilterConfig::default());
        let depth = OrderbookDepth {
            bid_depth_usd: 1000.0,
            ask_depth_usd: 200.0,
            spread_pct: 0.02,
        };
        
        let result_buy_yes = filter.check(&depth, true);
        assert!(!result_buy_yes.passed(), "Low ask depth should fail when buying YES");
        
        let result_buy_no = filter.check(&depth, false);
        assert!(result_buy_no.passed(), "High bid depth should pass when buying NO");
    }

    #[test]
    fn test_orderbook_filter_both_sides() {
        let config = OrderbookFilterConfig {
            check_both_sides: true,
            ..Default::default()
        };
        let filter = OrderbookDepthFilter::new(config);
        let depth = OrderbookDepth {
            bid_depth_usd: 100.0,
            ask_depth_usd: 800.0,
            spread_pct: 0.02,
        };
        let result = filter.check(&depth, true);
        assert!(!result.passed(), "Should fail when other side too thin");
    }

    #[test]
    fn test_volume_filter_pass_with_surge() {
        let filter = VolumeSurgeFilter::new(VolumeSurgeFilterConfig::default());
        let volume = VolumeData {
            current_volume: 10000.0,
            average_volume: 4000.0,
        };
        let result = filter.check(&volume);
        assert!(result.passed(), "2.5x surge should pass");
    }

    #[test]
    fn test_volume_filter_fail_no_surge() {
        let filter = VolumeSurgeFilter::new(VolumeSurgeFilterConfig::default());
        let volume = VolumeData {
            current_volume: 5000.0,
            average_volume: 4000.0,
        };
        let result = filter.check(&volume);
        assert!(!result.passed(), "1.25x surge should fail");
        assert!(result.reason().unwrap().contains("No volume surge"));
    }

    #[test]
    fn test_volume_filter_fail_low_absolute_volume() {
        let filter = VolumeSurgeFilter::new(VolumeSurgeFilterConfig::default());
        let volume = VolumeData {
            current_volume: 500.0,
            average_volume: 200.0,
        };
        let result = filter.check(&volume);
        assert!(!result.passed(), "Low absolute volume should fail");
        assert!(result.reason().unwrap().contains("Volume too low"));
    }

    #[test]
    fn test_volume_filter_no_average_data() {
        let filter = VolumeSurgeFilter::new(VolumeSurgeFilterConfig::default());
        let volume = VolumeData {
            current_volume: 5000.0,
            average_volume: 0.0,
        };
        let result = filter.check(&volume);
        assert!(result.passed(), "Should pass with high current volume even without average");
    }

    #[test]
    fn test_time_filter_pass_during_trading_hours() {
        let filter = TimeOfDayFilter::new(TimeFilterConfig::default());
        let time = Utc::now()
            .with_hour(14 + 5)
            .unwrap()
            .with_minute(30)
            .unwrap();
        let result = filter.check(time);
        assert!(result.passed(), "14:30 EST (12:30 PM) should pass");
    }

    #[test]
    fn test_time_filter_fail_before_hours() {
        let filter = TimeOfDayFilter::new(TimeFilterConfig::default());
        let time = Utc::now()
            .with_hour(8 + 5)
            .unwrap()
            .with_minute(0)
            .unwrap();
        let result = filter.check(time);
        assert!(!result.passed(), "8:00 EST should fail (before 9am)");
        assert!(result.reason().unwrap().contains("Outside trading hours"));
    }

    #[test]
    fn test_time_filter_fail_after_hours() {
        let filter = TimeOfDayFilter::new(TimeFilterConfig::default());
        let time = Utc::now()
            .with_hour(17 + 5)
            .unwrap()
            .with_minute(0)
            .unwrap();
        let result = filter.check(time);
        assert!(!result.passed(), "17:00 EST (5pm) should fail (after 4pm)");
    }

    #[test]
    fn test_time_filter_edge_cases() {
        let filter = TimeOfDayFilter::new(TimeFilterConfig::default());
        
        let start_time = Utc::now().with_hour(9 + 5).unwrap().with_minute(0).unwrap();
        assert!(filter.check(start_time).passed(), "9:00 EST should pass (start)");
        
        let end_time = Utc::now().with_hour(16 + 5).unwrap().with_minute(0).unwrap();
        assert!(!filter.check(end_time).passed(), "16:00 EST should fail (end boundary)");
    }

    #[test]
    fn test_combined_filter_all_pass() {
        let config = StrategyConfig {
            enable_momentum: true,
            enable_orderbook: true,
            enable_volume: false,
            enable_time: true,
            ..Default::default()
        };
        let filter = StrategyFilter::new(config);

        let orderbook = OrderbookDepth {
            bid_depth_usd: 1000.0,
            ask_depth_usd: 800.0,
            spread_pct: 0.02,
        };

        let time = Utc::now().with_hour(14 + 5).unwrap();

        let results = filter.check_all(
            0.6,
            0.9,
            true,
            true,
            Some(&orderbook),
            None,
            time,
            true,
        );

        assert!(results.all_passed(), "All filters should pass");
        assert_eq!(results.failure_reasons().len(), 0);
    }

    #[test]
    fn test_combined_filter_momentum_fail() {
        let config = StrategyConfig::default();
        let filter = StrategyFilter::new(config);

        let orderbook = OrderbookDepth {
            bid_depth_usd: 1000.0,
            ask_depth_usd: 800.0,
            spread_pct: 0.02,
        };

        let time = Utc::now().with_hour(14 + 5).unwrap();

        let results = filter.check_all(
            0.2,
            0.9,
            true,
            true,
            Some(&orderbook),
            None,
            time,
            true,
        );

        assert!(!results.all_passed(), "Should fail due to weak momentum");
        let reasons = results.failure_reasons();
        assert!(!reasons.is_empty());
        assert!(reasons[0].contains("Momentum"));
    }

    #[test]
    fn test_combined_filter_disabled_filters() {
        let config = StrategyConfig {
            enable_momentum: false,
            enable_orderbook: false,
            enable_volume: false,
            enable_time: false,
            ..Default::default()
        };
        let filter = StrategyFilter::new(config);

        let time = Utc::now();

        let results = filter.check_all(0.0, 0.0, false, false, None, None, time, true);

        assert!(results.all_passed(), "All filters disabled should pass");
    }

    #[test]
    fn test_filter_results_failure_reasons() {
        let results = FilterResults {
            momentum: Some(FilterResult::Fail("Weak momentum".to_string())),
            orderbook: Some(FilterResult::Fail("Thin book".to_string())),
            volume: Some(FilterResult::Pass),
            time: None,
        };

        let reasons = results.failure_reasons();
        assert_eq!(reasons.len(), 2);
        assert!(reasons[0].contains("Momentum"));
        assert!(reasons[1].contains("Orderbook"));
    }

    #[test]
    fn test_orderbook_depth_realistic_values() {
        let filter = OrderbookDepthFilter::new(OrderbookFilterConfig {
            min_depth_usd: 500.0,
            check_both_sides: false,
        });

        let thin_market = OrderbookDepth {
            bid_depth_usd: 250.0,
            ask_depth_usd: 300.0,
            spread_pct: 0.05,
        };
        assert!(!filter.check(&thin_market, true).passed(), "Thin market should fail");

        let liquid_market = OrderbookDepth {
            bid_depth_usd: 2000.0,
            ask_depth_usd: 1800.0,
            spread_pct: 0.01,
        };
        assert!(filter.check(&liquid_market, true).passed(), "Liquid market should pass");
    }
}
