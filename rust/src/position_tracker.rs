//! Position Tracker with Stop-Loss
//! Tracks open positions and triggers stop-loss sells when price drops below threshold

use rustc_hash::FxHashMap;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::RwLock;

// =============================================================================
// Configuration
// =============================================================================

/// Stop-loss threshold as a percentage (e.g., 0.05 = 5% loss triggers sell)
pub const STOP_LOSS_PCT: f64 = 0.05;

/// How often to check positions for stop-loss (in seconds)
pub const STOP_LOSS_CHECK_INTERVAL_SECS: u64 = 10;

/// Minimum position age before stop-loss can trigger (avoid selling immediately)
pub const MIN_POSITION_AGE_SECS: u64 = 30;

// =============================================================================
// Position Data
// =============================================================================

/// Represents an open position that we're tracking
#[derive(Debug, Clone)]
pub struct Position {
    /// Token ID for this position
    pub token_id: String,
    /// Average entry price (what we paid per share)
    pub entry_price: f64,
    /// Number of shares we hold
    pub shares: f64,
    /// When we opened this position
    pub opened_at: Instant,
    /// Whether this position is from a BUY (true) or we're tracking a SELL position (false)
    pub is_long: bool,
}

impl Position {
    pub fn new(token_id: String, entry_price: f64, shares: f64, is_long: bool) -> Self {
        Self {
            token_id,
            entry_price,
            shares,
            opened_at: Instant::now(),
            is_long,
        }
    }

    /// Calculate current P&L percentage given current price
    pub fn pnl_pct(&self, current_price: f64) -> f64 {
        if self.entry_price == 0.0 {
            return 0.0;
        }
        if self.is_long {
            // Long position: profit when price goes up
            (current_price - self.entry_price) / self.entry_price
        } else {
            // Short position: profit when price goes down
            (self.entry_price - current_price) / self.entry_price
        }
    }

    /// Check if this position should trigger stop-loss
    pub fn should_stop_loss(&self, current_price: f64) -> bool {
        // Don't trigger stop-loss on very new positions
        if self.opened_at.elapsed().as_secs() < MIN_POSITION_AGE_SECS {
            return false;
        }
        
        let pnl = self.pnl_pct(current_price);
        pnl <= -STOP_LOSS_PCT
    }

    /// Get position age in seconds
    pub fn age_secs(&self) -> u64 {
        self.opened_at.elapsed().as_secs()
    }
}

// =============================================================================
// Position Tracker
// =============================================================================

/// Thread-safe position tracker
pub struct PositionTracker {
    /// Map of token_id -> Position
    positions: Arc<RwLock<FxHashMap<String, Position>>>,
}

impl PositionTracker {
    pub fn new() -> Self {
        Self {
            positions: Arc::new(RwLock::new(FxHashMap::default())),
        }
    }

    /// Add or update a position after a successful buy
    pub async fn add_position(&self, token_id: String, entry_price: f64, shares: f64) {
        let mut positions = self.positions.write().await;
        
        if let Some(existing) = positions.get_mut(&token_id) {
            // Average into existing position
            let total_shares = existing.shares + shares;
            let total_cost = (existing.entry_price * existing.shares) + (entry_price * shares);
            existing.entry_price = total_cost / total_shares;
            existing.shares = total_shares;
            println!(
                "📊 Position updated: {} | avg price: {:.4} | total shares: {:.2}",
                token_id, existing.entry_price, existing.shares
            );
        } else {
            // New position
            let position = Position::new(token_id.clone(), entry_price, shares, true);
            println!(
                "📊 Position opened: {} | entry: {:.4} | shares: {:.2}",
                token_id, entry_price, shares
            );
            positions.insert(token_id, position);
        }
    }

    /// Remove a position (after sell or stop-loss)
    pub async fn remove_position(&self, token_id: &str) -> Option<Position> {
        let mut positions = self.positions.write().await;
        positions.remove(token_id)
    }

    /// Reduce position size (partial sell)
    pub async fn reduce_position(&self, token_id: &str, shares_sold: f64) {
        let mut positions = self.positions.write().await;
        if let Some(position) = positions.get_mut(token_id) {
            position.shares -= shares_sold;
            if position.shares <= 0.0 {
                positions.remove(token_id);
                println!("📊 Position closed: {}", token_id);
            } else {
                println!(
                    "📊 Position reduced: {} | remaining shares: {:.2}",
                    token_id, position.shares
                );
            }
        }
    }

    /// Get a snapshot of all positions
    pub async fn get_all_positions(&self) -> Vec<Position> {
        let positions = self.positions.read().await;
        positions.values().cloned().collect()
    }

    /// Get a specific position
    pub async fn get_position(&self, token_id: &str) -> Option<Position> {
        let positions = self.positions.read().await;
        positions.get(token_id).cloned()
    }

    /// Check all positions for stop-loss triggers
    /// Returns list of (token_id, position) that need to be sold
    pub async fn check_stop_losses(&self, price_fetcher: &impl PriceFetcher) -> Vec<(String, Position, f64)> {
        let positions = self.positions.read().await;
        let mut to_sell = Vec::new();

        for (token_id, position) in positions.iter() {
            if let Some(current_price) = price_fetcher.get_current_price(token_id).await {
                if position.should_stop_loss(current_price) {
                    let pnl_pct = position.pnl_pct(current_price) * 100.0;
                    println!(
                        "🛑 STOP-LOSS TRIGGERED: {} | entry: {:.4} | current: {:.4} | P&L: {:.2}%",
                        token_id, position.entry_price, current_price, pnl_pct
                    );
                    to_sell.push((token_id.clone(), position.clone(), current_price));
                }
            }
        }

        to_sell
    }

    /// Get shared reference for cloning
    pub fn get_shared(&self) -> Arc<RwLock<FxHashMap<String, Position>>> {
        Arc::clone(&self.positions)
    }
}

impl Default for PositionTracker {
    fn default() -> Self {
        Self::new()
    }
}

// =============================================================================
// Price Fetcher Trait
// =============================================================================

/// Trait for fetching current prices (implemented by the order book fetcher)
#[async_trait::async_trait]
pub trait PriceFetcher: Send + Sync {
    async fn get_current_price(&self, token_id: &str) -> Option<f64>;
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_position_pnl() {
        let position = Position::new("test".into(), 0.50, 100.0, true);
        
        // Price went up 10%
        assert!((position.pnl_pct(0.55) - 0.10).abs() < 0.001);
        
        // Price went down 5%
        assert!((position.pnl_pct(0.475) - (-0.05)).abs() < 0.001);
    }

    #[test]
    fn test_stop_loss_threshold() {
        // Create position with old timestamp to bypass age check
        let mut position = Position::new("test".into(), 0.50, 100.0, true);
        position.opened_at = Instant::now() - std::time::Duration::from_secs(60);
        
        // 4% loss - should NOT trigger (threshold is 5%)
        assert!(!position.should_stop_loss(0.48));
        
        // 5% loss - should trigger
        assert!(position.should_stop_loss(0.475));
        
        // 6% loss - should trigger
        assert!(position.should_stop_loss(0.47));
    }

    #[test]
    fn test_new_position_no_stop_loss() {
        // New position should not trigger stop-loss even with big loss
        let position = Position::new("test".into(), 0.50, 100.0, true);
        
        // 10% loss but position is too new
        assert!(!position.should_stop_loss(0.45));
    }
}
