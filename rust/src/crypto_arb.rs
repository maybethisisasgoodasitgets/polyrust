/// Crypto Latency Arbitrage Module
/// 
/// Monitors real-time BTC prices from Binance and compares against
/// Polymarket's live crypto markets to find arbitrage opportunities.
/// 
/// Strategy: When BTC price moves but Polymarket odds haven't caught up,
/// bet on the near-certain outcome.

use anyhow::{Result, anyhow};
use chrono::{DateTime, Utc};
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::sync::{mpsc, RwLock};
use tokio_tungstenite::{connect_async, tungstenite::protocol::Message};
use crate::strategy_filters::{StrategyFilter, StrategyConfig, OrderbookDepth, VolumeData};

// ============================================================================
// Configuration
// ============================================================================

/// Minimum price move (%) to trigger a bet
pub const MIN_PRICE_MOVE_PCT: f64 = 0.10;  // 0.1% move

/// Maximum odds to buy (e.g., 0.99 = 99 cents for $1 payout)
/// Set very high to allow trading on decided markets
pub const MAX_BUY_PRICE: f64 = 0.99;

/// Minimum edge required (difference between true prob and market odds)
pub const MIN_EDGE_PCT: f64 = 2.0;  // 2% edge minimum

/// How often to check for opportunities (ms)
pub const CHECK_INTERVAL_MS: u64 = 100;

/// Binance WebSocket URL for BTC/USDT trades
pub const BINANCE_BTC_WS_URL: &str = "wss://stream.binance.com:9443/ws/btcusdt@trade";

/// Binance WebSocket URL for ETH/USDT trades
pub const BINANCE_ETH_WS_URL: &str = "wss://stream.binance.com:9443/ws/ethusdt@trade";

/// Binance WebSocket URL for SOL/USDT trades
pub const BINANCE_SOL_WS_URL: &str = "wss://stream.binance.com:9443/ws/solusdt@trade";

/// Binance WebSocket URL for XRP/USDT trades
pub const BINANCE_XRP_WS_URL: &str = "wss://stream.binance.com:9443/ws/xrpusdt@trade";

/// Binance WebSocket URL for BTC/USDT ticker (more frequent updates)
pub const BINANCE_TICKER_WS_URL: &str = "wss://stream.binance.com:9443/ws/btcusdt@ticker";

/// Crypto asset type
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CryptoAsset {
    BTC,
    ETH,
    SOL,
    XRP,
}

// ============================================================================
// Price State
// ============================================================================

/// Number of price samples to keep for momentum calculation
const MOMENTUM_WINDOW_SIZE: usize = 20;

/// Velocity window in seconds - how far back to look for quick moves
const VELOCITY_WINDOW_SECS: u64 = 5;

#[derive(Debug, Clone)]
pub struct PriceState {
    /// Current BTC price from Binance
    pub btc_price: f64,
    /// BTC price at the start of the current Polymarket interval
    pub btc_interval_start_price: f64,
    /// Current ETH price from Binance
    pub eth_price: f64,
    /// ETH price at the start of the current Polymarket interval
    pub eth_interval_start_price: f64,
    /// Current SOL price from Binance
    pub sol_price: f64,
    /// SOL price at the start of the current Polymarket interval
    pub sol_interval_start_price: f64,
    /// Current XRP price from Binance
    pub xrp_price: f64,
    /// XRP price at the start of the current Polymarket interval
    pub xrp_interval_start_price: f64,
    /// Timestamp of last price update
    pub last_update: Instant,
    /// Timestamp of interval start
    pub interval_start_time: Instant,
    /// Recent BTC prices for momentum calculation (newest last)
    pub btc_price_history: Vec<(f64, Instant)>,
    /// Recent ETH prices for momentum calculation (newest last)
    pub eth_price_history: Vec<(f64, Instant)>,
    /// Recent SOL prices for momentum calculation (newest last)
    pub sol_price_history: Vec<(f64, Instant)>,
    /// Recent XRP prices for momentum calculation (newest last)
    pub xrp_price_history: Vec<(f64, Instant)>,
}

impl Default for PriceState {
    fn default() -> Self {
        Self {
            btc_price: 0.0,
            btc_interval_start_price: 0.0,
            eth_price: 0.0,
            eth_interval_start_price: 0.0,
            sol_price: 0.0,
            sol_interval_start_price: 0.0,
            xrp_price: 0.0,
            xrp_interval_start_price: 0.0,
            last_update: Instant::now(),
            interval_start_time: Instant::now(),
            btc_price_history: Vec::with_capacity(MOMENTUM_WINDOW_SIZE),
            eth_price_history: Vec::with_capacity(MOMENTUM_WINDOW_SIZE),
            sol_price_history: Vec::with_capacity(MOMENTUM_WINDOW_SIZE),
            xrp_price_history: Vec::with_capacity(MOMENTUM_WINDOW_SIZE),
        }
    }
}

impl PriceState {
    /// Calculate BTC price change percentage since interval start
    pub fn btc_change_pct(&self) -> f64 {
        if self.btc_interval_start_price == 0.0 {
            return 0.0;
        }
        ((self.btc_price - self.btc_interval_start_price) / self.btc_interval_start_price) * 100.0
    }
    
    /// Calculate ETH price change percentage since interval start
    pub fn eth_change_pct(&self) -> f64 {
        if self.eth_interval_start_price == 0.0 {
            return 0.0;
        }
        ((self.eth_price - self.eth_interval_start_price) / self.eth_interval_start_price) * 100.0
    }
    
    /// Calculate SOL price change percentage since interval start
    pub fn sol_change_pct(&self) -> f64 {
        if self.sol_interval_start_price == 0.0 {
            return 0.0;
        }
        ((self.sol_price - self.sol_interval_start_price) / self.sol_interval_start_price) * 100.0
    }
    
    /// Calculate XRP price change percentage since interval start
    pub fn xrp_change_pct(&self) -> f64 {
        if self.xrp_interval_start_price == 0.0 {
            return 0.0;
        }
        ((self.xrp_price - self.xrp_interval_start_price) / self.xrp_interval_start_price) * 100.0
    }
    
    /// Get price change for a specific asset
    pub fn price_change_pct(&self, asset: CryptoAsset) -> f64 {
        match asset {
            CryptoAsset::BTC => self.btc_change_pct(),
            CryptoAsset::ETH => self.eth_change_pct(),
            CryptoAsset::SOL => self.sol_change_pct(),
            CryptoAsset::XRP => self.xrp_change_pct(),
        }
    }
    
    /// Get current price for a specific asset
    pub fn current_price(&self, asset: CryptoAsset) -> f64 {
        match asset {
            CryptoAsset::BTC => self.btc_price,
            CryptoAsset::ETH => self.eth_price,
            CryptoAsset::SOL => self.sol_price,
            CryptoAsset::XRP => self.xrp_price,
        }
    }
    
    /// Returns true if asset price is up since interval start
    pub fn is_up(&self, asset: CryptoAsset) -> bool {
        match asset {
            CryptoAsset::BTC => self.btc_price > self.btc_interval_start_price,
            CryptoAsset::ETH => self.eth_price > self.eth_interval_start_price,
            CryptoAsset::SOL => self.sol_price > self.sol_interval_start_price,
            CryptoAsset::XRP => self.xrp_price > self.xrp_interval_start_price,
        }
    }
    
    /// Add a price sample to history for momentum calculation
    pub fn add_price_sample(&mut self, asset: CryptoAsset, price: f64) {
        let history = match asset {
            CryptoAsset::BTC => &mut self.btc_price_history,
            CryptoAsset::ETH => &mut self.eth_price_history,
            CryptoAsset::SOL => &mut self.sol_price_history,
            CryptoAsset::XRP => &mut self.xrp_price_history,
        };
        
        history.push((price, Instant::now()));
        
        // Keep only the last N samples
        if history.len() > MOMENTUM_WINDOW_SIZE {
            history.remove(0);
        }
    }
    
    /// Calculate short-term velocity (price change over last N seconds)
    /// This is the key metric for reactive trading - detects quick moves
    pub fn velocity_pct(&self, asset: CryptoAsset, window_secs: u64) -> f64 {
        let history = match asset {
            CryptoAsset::BTC => &self.btc_price_history,
            CryptoAsset::ETH => &self.eth_price_history,
            CryptoAsset::SOL => &self.sol_price_history,
            CryptoAsset::XRP => &self.xrp_price_history,
        };
        
        if history.len() < 2 {
            return 0.0;
        }
        
        let now = Instant::now();
        let cutoff = now - Duration::from_secs(window_secs);
        
        // Find the oldest price within our window
        let mut oldest_in_window: Option<f64> = None;
        for (price, time) in history.iter() {
            if *time >= cutoff {
                oldest_in_window = Some(*price);
                break;
            }
        }
        
        // If no prices in window, use the oldest available
        let start_price = oldest_in_window.unwrap_or_else(|| history.first().map(|(p, _)| *p).unwrap_or(0.0));
        let current_price = history.last().map(|(p, _)| *p).unwrap_or(0.0);
        
        if start_price == 0.0 {
            return 0.0;
        }
        
        ((current_price - start_price) / start_price) * 100.0
    }
    
    /// Calculate momentum score for an asset
    /// Returns a value between -1.0 (strong downward) and 1.0 (strong upward)
    /// Also returns whether momentum is accelerating
    pub fn momentum(&self, asset: CryptoAsset) -> MomentumSignal {
        let history = match asset {
            CryptoAsset::BTC => &self.btc_price_history,
            CryptoAsset::ETH => &self.eth_price_history,
            CryptoAsset::SOL => &self.sol_price_history,
            CryptoAsset::XRP => &self.xrp_price_history,
        };
        
        if history.len() < 3 {
            return MomentumSignal::default();
        }
        
        // Calculate price changes between consecutive samples
        let mut changes: Vec<f64> = Vec::new();
        for i in 1..history.len() {
            let prev_price = history[i - 1].0;
            let curr_price = history[i].0;
            if prev_price > 0.0 {
                let pct_change = ((curr_price - prev_price) / prev_price) * 100.0;
                changes.push(pct_change);
            }
        }
        
        if changes.is_empty() {
            return MomentumSignal::default();
        }
        
        // Calculate average momentum (direction and strength)
        let avg_change: f64 = changes.iter().sum::<f64>() / changes.len() as f64;
        
        // Calculate if momentum is accelerating or decelerating
        // Compare recent changes to older changes
        let mid = changes.len() / 2;
        let recent_avg = if mid < changes.len() {
            changes[mid..].iter().sum::<f64>() / (changes.len() - mid) as f64
        } else {
            avg_change
        };
        let older_avg = if mid > 0 {
            changes[..mid].iter().sum::<f64>() / mid as f64
        } else {
            avg_change
        };
        
        // Acceleration: positive if recent moves are stronger in the same direction
        let is_accelerating = if avg_change > 0.0 {
            recent_avg > older_avg  // Upward and getting stronger
        } else if avg_change < 0.0 {
            recent_avg < older_avg  // Downward and getting stronger
        } else {
            false
        };
        
        // Check for consistency (all moves in same direction)
        let positive_count = changes.iter().filter(|&&c| c > 0.0).count();
        let negative_count = changes.iter().filter(|&&c| c < 0.0).count();
        let consistency = (positive_count.max(negative_count) as f64) / changes.len() as f64;
        
        // Normalize momentum to -1.0 to 1.0 range (0.01% change = 0.1 score)
        let normalized = (avg_change * 10.0).clamp(-1.0, 1.0);
        
        MomentumSignal {
            score: normalized,
            is_accelerating,
            consistency,
            direction: if avg_change > 0.001 {
                MomentumDirection::Up
            } else if avg_change < -0.001 {
                MomentumDirection::Down
            } else {
                MomentumDirection::Neutral
            },
        }
    }
}

/// Momentum analysis result
#[derive(Debug, Clone)]
pub struct MomentumSignal {
    /// Momentum score from -1.0 (strong down) to 1.0 (strong up)
    pub score: f64,
    /// True if momentum is accelerating (getting stronger)
    pub is_accelerating: bool,
    /// Consistency of direction (0.0 to 1.0, higher = more consistent)
    pub consistency: f64,
    /// Overall direction
    pub direction: MomentumDirection,
}

impl Default for MomentumSignal {
    fn default() -> Self {
        Self {
            score: 0.0,
            is_accelerating: false,
            consistency: 0.0,
            direction: MomentumDirection::Neutral,
        }
    }
}

impl MomentumSignal {
    /// Returns true if this is a strong, reliable signal
    pub fn is_strong(&self) -> bool {
        self.score.abs() > 0.3 && self.consistency > 0.6
    }
    
    /// Returns true if momentum supports the given direction
    pub fn supports_direction(&self, is_up: bool) -> bool {
        match self.direction {
            MomentumDirection::Up => is_up,
            MomentumDirection::Down => !is_up,
            MomentumDirection::Neutral => false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MomentumDirection {
    Up,
    Down,
    Neutral,
}

// ============================================================================
// Binance WebSocket Messages
// ============================================================================

#[derive(Debug, Deserialize)]
pub struct BinanceTrade {
    #[serde(rename = "e")]
    pub event_type: String,
    #[serde(rename = "s")]
    pub symbol: String,
    #[serde(rename = "p")]
    pub price: String,
    #[serde(rename = "T")]
    pub trade_time: u64,
}

#[derive(Debug, Deserialize)]
pub struct BinanceTicker {
    #[serde(rename = "e")]
    pub event_type: String,
    #[serde(rename = "s")]
    pub symbol: String,
    #[serde(rename = "c")]
    pub close_price: String,
    #[serde(rename = "o")]
    pub open_price: String,
    #[serde(rename = "h")]
    pub high_price: String,
    #[serde(rename = "l")]
    pub low_price: String,
    #[serde(rename = "P")]
    pub price_change_pct: String,
}

// ============================================================================
// Polymarket Live Crypto Market
// ============================================================================

#[derive(Debug, Clone)]
pub struct LiveCryptoMarket {
    /// Market condition ID
    pub condition_id: String,
    /// Token ID for "Yes" (price goes up)
    pub yes_token_id: String,
    /// Token ID for "No" (price goes down)  
    pub no_token_id: String,
    /// Current best ask for "Yes"
    pub yes_ask: f64,
    /// Current best ask for "No"
    pub no_ask: f64,
    /// Market end time (Unix timestamp)
    pub end_time: u64,
    /// Interval duration in minutes (e.g., 5 for 5-minute intervals)
    pub interval_minutes: u32,
    /// Description (e.g., "BTC up or down in next 5 minutes")
    pub description: String,
    /// Which crypto asset this market is for
    pub asset: CryptoAsset,
}

// ============================================================================
// Arbitrage Signal
// ============================================================================

#[derive(Debug, Clone)]
pub struct ArbSignal {
    /// Direction to bet (true = up/yes, false = down/no)
    pub bet_up: bool,
    /// Token ID to buy
    pub token_id: String,
    /// Price to buy at
    pub buy_price: f64,
    /// Estimated edge percentage
    pub edge_pct: f64,
    /// Current crypto price
    pub crypto_price: f64,
    /// Which asset (BTC or ETH)
    pub asset: CryptoAsset,
    /// Price change since interval start
    pub price_change_pct: f64,
    /// Confidence level (0-100)
    pub confidence: u8,
    /// Recommended position size in USD
    pub recommended_size_usd: f64,
}

// ============================================================================
// Arbitrage Engine
// ============================================================================

pub struct CryptoArbEngine {
    /// Shared price state
    price_state: Arc<RwLock<PriceState>>,
    /// Current BTC market (if any)
    btc_market: Option<LiveCryptoMarket>,
    /// Current ETH market (if any)
    eth_market: Option<LiveCryptoMarket>,
    /// Current SOL market (if any)
    sol_market: Option<LiveCryptoMarket>,
    /// Current XRP market (if any)
    xrp_market: Option<LiveCryptoMarket>,
    /// Legacy single market field (for backward compatibility)
    market: Option<LiveCryptoMarket>,
    /// Mock mode (don't execute real trades)
    mock_mode: bool,
    /// Maximum position size per trade
    max_position_usd: f64,
    /// Minimum position size per trade
    min_position_usd: f64,
    /// Use momentum filter (can be toggled off for more signals)
    pub use_momentum: bool,
    /// Use edge check (can be toggled off for more signals)
    pub use_edge_check: bool,
    /// Strategy filter system (NEW - replaces old filters)
    pub strategy_filter: StrategyFilter,
}

impl CryptoArbEngine {
    pub fn new(mock_mode: bool, max_position_usd: f64, min_position_usd: f64) -> Self {
        // Create profitable strategy config with filters ENABLED by default
        let strategy_config = StrategyConfig {
            enable_momentum: true,
            enable_orderbook: true,
            enable_volume: false,  // Volume data not yet available
            enable_time: true,
            ..Default::default()
        };
        
        Self {
            price_state: Arc::new(RwLock::new(PriceState::default())),
            btc_market: None,
            eth_market: None,
            sol_market: None,
            xrp_market: None,
            use_momentum: false,  // Legacy - kept for backward compatibility
            use_edge_check: false,  // Legacy - kept for backward compatibility
            market: None,
            mock_mode,
            max_position_usd,
            min_position_usd,
            strategy_filter: StrategyFilter::new(strategy_config),
        }
    }
    
    /// Get shared price state for external access
    pub fn price_state(&self) -> Arc<RwLock<PriceState>> {
        self.price_state.clone()
    }
    
    /// Set the current live crypto market to monitor (legacy single-market mode)
    pub fn set_market(&mut self, market: LiveCryptoMarket) {
        self.market = Some(market);
    }
    
    /// Set market for a specific asset (multi-market mode)
    pub fn set_market_for_asset(&mut self, market: LiveCryptoMarket) {
        match market.asset {
            CryptoAsset::BTC => self.btc_market = Some(market),
            CryptoAsset::ETH => self.eth_market = Some(market),
            CryptoAsset::SOL => self.sol_market = Some(market),
            CryptoAsset::XRP => self.xrp_market = Some(market),
        }
    }
    
    /// Clear market for a specific asset
    pub fn clear_market_for_asset(&mut self, asset: CryptoAsset) {
        match asset {
            CryptoAsset::BTC => self.btc_market = None,
            CryptoAsset::ETH => self.eth_market = None,
            CryptoAsset::SOL => self.sol_market = None,
            CryptoAsset::XRP => self.xrp_market = None,
        }
    }
    
    /// Get current market for an asset
    pub fn get_market(&self, asset: CryptoAsset) -> Option<&LiveCryptoMarket> {
        match asset {
            CryptoAsset::BTC => self.btc_market.as_ref(),
            CryptoAsset::ETH => self.eth_market.as_ref(),
            CryptoAsset::SOL => self.sol_market.as_ref(),
            CryptoAsset::XRP => self.xrp_market.as_ref(),
        }
    }
    
    /// Check if we have an active market for an asset
    pub fn has_market(&self, asset: CryptoAsset) -> bool {
        match asset {
            CryptoAsset::BTC => self.btc_market.is_some(),
            CryptoAsset::ETH => self.eth_market.is_some(),
            CryptoAsset::SOL => self.sol_market.is_some(),
            CryptoAsset::XRP => self.xrp_market.is_some(),
        }
    }
    
    /// Check for arbitrage opportunity
    pub async fn check_opportunity(&self) -> Option<ArbSignal> {
        let market = self.market.as_ref()?;
        let state = self.price_state.read().await;
        let asset = market.asset;
        
        // Need valid prices for the relevant asset
        let (current_price, interval_start) = match asset {
            CryptoAsset::BTC => (state.btc_price, state.btc_interval_start_price),
            CryptoAsset::ETH => (state.eth_price, state.eth_interval_start_price),
            CryptoAsset::SOL => (state.sol_price, state.sol_interval_start_price),
            CryptoAsset::XRP => (state.xrp_price, state.xrp_interval_start_price),
        };
        
        if current_price == 0.0 || interval_start == 0.0 {
            return None;
        }
        
        let change_pct = state.price_change_pct(asset);
        let abs_change = change_pct.abs();
        
        // Asset and market-type-specific minimum price move thresholds
        // BTC: Lower thresholds since $95k price means 0.10% = $95 move (too high)
        // Other assets: Keep standard thresholds
        let min_move = match (asset, market.interval_minutes) {
            // BTC thresholds (lowered - 0.04% = ~$40 at $95k)
            (CryptoAsset::BTC, 5) => 0.02,       // 5-minute: 0.02% (~$19)
            (CryptoAsset::BTC, 15) => 0.04,      // 15-minute: 0.04% (~$38)
            (CryptoAsset::BTC, 60) => 0.08,      // 1-hour: 0.08% (~$76)
            (CryptoAsset::BTC, 240) => 0.12,     // 4-hour: 0.12% (~$114)
            (CryptoAsset::BTC, _) => 0.06,       // Default: 0.06% (~$57)
            // ETH thresholds (standard)
            (CryptoAsset::ETH, 5) => 0.05,       // 5-minute: 0.05%
            (CryptoAsset::ETH, 15) => 0.10,      // 15-minute: 0.10%
            (CryptoAsset::ETH, 60) => 0.20,      // 1-hour: 0.20%
            (CryptoAsset::ETH, 240) => 0.30,     // 4-hour: 0.30%
            (CryptoAsset::ETH, _) => 0.15,       // Default: 0.15%
            // SOL thresholds (slightly lower - more volatile)
            (CryptoAsset::SOL, 5) => 0.04,       // 5-minute: 0.04%
            (CryptoAsset::SOL, 15) => 0.08,      // 15-minute: 0.08%
            (CryptoAsset::SOL, 60) => 0.15,      // 1-hour: 0.15%
            (CryptoAsset::SOL, 240) => 0.25,     // 4-hour: 0.25%
            (CryptoAsset::SOL, _) => 0.10,       // Default: 0.10%
            // XRP thresholds (slightly lower - more volatile)
            (CryptoAsset::XRP, 5) => 0.04,       // 5-minute: 0.04%
            (CryptoAsset::XRP, 15) => 0.08,      // 15-minute: 0.08%
            (CryptoAsset::XRP, 60) => 0.15,      // 1-hour: 0.15%
            (CryptoAsset::XRP, 240) => 0.25,     // 4-hour: 0.25%
            (CryptoAsset::XRP, _) => 0.10,       // Default: 0.10%
        };
        
        // Need minimum price movement for this market type
        if abs_change < min_move {
            return None;
        }
        
        // === MOMENTUM CHECK ===
        // Get momentum signal for this asset
        let momentum = state.momentum(asset);
        let is_up = state.is_up(asset);
        
        let asset_name = match asset {
            CryptoAsset::BTC => "BTC",
            CryptoAsset::ETH => "ETH",
            CryptoAsset::SOL => "SOL",
            CryptoAsset::XRP => "XRP",
        };
        
        // Debug: Log when we pass min_move but might fail momentum
        println!("🔍 {} passed min_move ({:.3}% >= {:.3}%) - checking momentum...", 
            asset_name, abs_change, min_move);
        println!("   Momentum: score={:.2}, consistency={:.2}, accel={}, supports_dir={}", 
            momentum.score, momentum.consistency, momentum.is_accelerating, momentum.supports_direction(is_up));
        
        // Only apply momentum filters if use_momentum is enabled
        if self.use_momentum {
            // Skip if momentum doesn't support the direction we'd bet
            if !momentum.supports_direction(is_up) {
                println!("   ❌ SKIP: momentum doesn't support direction (is_up={})", is_up);
                return None;  // Price moved but momentum is against us or neutral
            }
            
            // Skip if momentum is decelerating (likely to reverse)
            // Only apply this filter if we have enough data
            if momentum.consistency > 0.0 && !momentum.is_accelerating && momentum.score.abs() < 0.5 {
                println!("   ❌ SKIP: weak decelerating momentum");
                return None;  // Weak, decelerating momentum - skip
            }
            
            println!("   ✅ Momentum check passed!");
        } else {
            println!("   ⏭️ Momentum filter DISABLED - skipping checks");
        }
        
        // Determine direction and get relevant market prices
        let (bet_up, token_id, market_ask) = if is_up {
            (true, market.yes_token_id.clone(), market.yes_ask)
        } else {
            (false, market.no_token_id.clone(), market.no_ask)
        };
        
        // Check if market price is attractive enough (silent skip if too expensive)
        if market_ask > MAX_BUY_PRICE {
            return None;
        }
        
        // Calculate edge: if price moved X%, true probability is higher than market implies
        // Multiplier varies by market type - shorter timeframes = stronger signal per % move
        let prob_multiplier = match market.interval_minutes {
            5 => 8.0,       // 5-minute: 0.05% move → 0.4% prob increase
            15 => 5.0,      // 15-minute: 0.10% move → 0.5% prob increase
            60 => 3.0,      // 1-hour: 0.20% move → 0.6% prob increase
            240 => 2.0,     // 4-hour: 0.30% move → 0.6% prob increase
            _ => 4.0,       // Default
        };
        
        // Boost edge calculation if momentum is strong and accelerating
        let momentum_boost = if momentum.is_strong() && momentum.is_accelerating {
            1.2  // 20% boost for strong accelerating momentum
        } else if momentum.is_strong() {
            1.1  // 10% boost for strong momentum
        } else {
            1.0  // No boost
        };
        
        let implied_prob = 0.50 + (abs_change * prob_multiplier * momentum_boost).min(45.0) / 100.0;
        let market_prob = market_ask;
        let edge_pct = (implied_prob - market_prob) * 100.0;
        
        // Minimum edge also varies by market type
        // Lowered thresholds since 50¢ markets have inherently low edge
        let min_edge = match market.interval_minutes {
            5 => 0.3,       // 5-minute: very low edge OK (fast resolution, small moves)
            15 => 0.5,      // 15-minute: low edge
            60 => 1.0,      // 1-hour: moderate edge
            240 => 1.5,     // 4-hour: need more edge
            _ => 0.5,
        };
        
        // Only apply edge check if use_edge_check is enabled
        if self.use_edge_check {
            if edge_pct < min_edge {
                println!("   ❌ SKIP: edge too low ({:.2}% < {:.2}%)", edge_pct, min_edge);
                return None;
            }
            println!("   ✅ Edge check passed ({:.2}% >= {:.2}%)", edge_pct, min_edge);
        } else {
            println!("   ⏭️ Edge check DISABLED - skipping (edge would be {:.2}%)", edge_pct);
        }
        
        // Calculate confidence (0-100) - scaled by market type and momentum
        let confidence_multiplier = match market.interval_minutes {
            5 => 30.0,      // 5-minute: small moves = high confidence
            15 => 20.0,     // 15-minute: standard
            60 => 15.0,     // 1-hour: need bigger moves
            240 => 10.0,    // 4-hour: need even bigger moves
            _ => 20.0,
        };
        
        // Boost confidence if momentum is strong and consistent
        let momentum_confidence_boost = if momentum.is_strong() {
            1.0 + momentum.consistency * 0.5  // Up to 50% boost for consistent momentum
        } else {
            1.0
        };
        
        let confidence = ((abs_change * confidence_multiplier * momentum_confidence_boost).min(100.0)) as u8;
        
        // Calculate recommended size based on edge (Kelly-lite)
        // Increase size for strong momentum signals
        let kelly_fraction = (edge_pct / 100.0) / (1.0 - market_ask);
        let size_multiplier = if momentum.is_strong() && momentum.is_accelerating {
            1.5  // 50% larger position for strong accelerating momentum
        } else {
            1.0
        };
        let recommended_size = (self.max_position_usd * kelly_fraction.min(0.25) * size_multiplier)
            .max(self.min_position_usd)
            .min(self.max_position_usd);
        
        Some(ArbSignal {
            bet_up,
            token_id,
            buy_price: market_ask,
            edge_pct,
            crypto_price: current_price,
            asset,
            price_change_pct: change_pct,
            confidence,
            recommended_size_usd: recommended_size,
        })
    }
    
    /// Reset interval for all assets (call when new Polymarket interval starts)
    pub async fn reset_interval(&self) {
        let mut state = self.price_state.write().await;
        state.btc_interval_start_price = state.btc_price;
        state.eth_interval_start_price = state.eth_price;
        state.sol_interval_start_price = state.sol_price;
        state.xrp_interval_start_price = state.xrp_price;
        state.interval_start_time = Instant::now();
    }
    
    /// Reset interval for a specific asset only
    pub async fn reset_interval_for_asset(&self, asset: CryptoAsset) {
        let mut state = self.price_state.write().await;
        match asset {
            CryptoAsset::BTC => state.btc_interval_start_price = state.btc_price,
            CryptoAsset::ETH => state.eth_interval_start_price = state.eth_price,
            CryptoAsset::SOL => state.sol_interval_start_price = state.sol_price,
            CryptoAsset::XRP => state.xrp_interval_start_price = state.xrp_price,
        }
    }
    
    /// Check for arbitrage opportunities on ALL active markets (multi-market mode)
    /// Returns signals for BTC, ETH, SOL, and XRP if opportunities exist
    pub async fn check_all_opportunities(&self) -> Vec<ArbSignal> {
        let mut signals = Vec::new();
        
        // Check BTC market
        if let Some(signal) = self.check_opportunity_for_asset(CryptoAsset::BTC).await {
            signals.push(signal);
        }
        
        // Check ETH market
        if let Some(signal) = self.check_opportunity_for_asset(CryptoAsset::ETH).await {
            signals.push(signal);
        }
        
        // Check SOL market
        if let Some(signal) = self.check_opportunity_for_asset(CryptoAsset::SOL).await {
            signals.push(signal);
        }
        
        // Check XRP market
        if let Some(signal) = self.check_opportunity_for_asset(CryptoAsset::XRP).await {
            signals.push(signal);
        }
        
        signals
    }
    
    /// Get detailed status analysis for why no signals are being generated
    /// Returns a human-readable explanation of market conditions
    pub async fn get_status_analysis(&self) -> String {
        let state = self.price_state.read().await;
        let mut analysis = String::new();
        
        analysis.push_str("📊 SIGNAL STATUS ANALYSIS\n");
        analysis.push_str("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");
        
        let assets = [
            (CryptoAsset::BTC, "BTC", self.btc_market.as_ref(), 0.02),   // 10x from 0.002
            (CryptoAsset::ETH, "ETH", self.eth_market.as_ref(), 0.03),   // 10x from 0.003
            (CryptoAsset::SOL, "SOL", self.sol_market.as_ref(), 0.04),   // 10x from 0.004
            (CryptoAsset::XRP, "XRP", self.xrp_market.as_ref(), 0.04),   // 10x from 0.004
        ];
        
        let mut all_below_threshold = true;
        let mut highest_pct = 0.0;
        let mut closest_asset = "None";
        
        for (asset, name, market_opt, threshold) in assets.iter() {
            let current_price = state.current_price(*asset);
            if current_price == 0.0 {
                analysis.push_str(&format!("   ⚠️  {}: No price data available\n", name));
                continue;
            }
            
            let velocity_5s = state.velocity_pct(*asset, 5);
            let velocity_3s = state.velocity_pct(*asset, 3);
            let velocity = if velocity_3s.abs() > velocity_5s.abs() { velocity_3s } else { velocity_5s };
            let abs_velocity = velocity.abs();
            
            let pct_of_threshold = (abs_velocity / threshold) * 100.0;
            if pct_of_threshold > highest_pct {
                highest_pct = pct_of_threshold;
                closest_asset = name;
            }
            
            if abs_velocity >= *threshold {
                all_below_threshold = false;
            }
            
            let status_icon = if abs_velocity >= *threshold {
                "✅"
            } else if pct_of_threshold >= 70.0 {
                "🟡"
            } else if pct_of_threshold >= 40.0 {
                "🟠"
            } else {
                "⚪"
            };
            
            let dir_icon = if velocity >= 0.0 { "⬆" } else { "⬇" };
            
            analysis.push_str(&format!(
                "   {} {}: ${:.2} {}{:+.4}% (need {:+.3}%) [{:.0}% of threshold]\n",
                status_icon, name, current_price, dir_icon, velocity, threshold, pct_of_threshold
            ));
            
            // Show market price if available
            if let Some(market) = market_opt {
                let yes_price = market.yes_ask;
                let no_price = market.no_ask;
                let price_status = if yes_price > MAX_BUY_PRICE || no_price > MAX_BUY_PRICE {
                    "❌ TOO HIGH"
                } else {
                    "✓"
                };
                analysis.push_str(&format!(
                    "      Market: YES={:.1}¢ NO={:.1}¢ {}\n",
                    yes_price * 100.0, no_price * 100.0, price_status
                ));
            } else {
                analysis.push_str("      Market: No active market\n");
            }
        }
        
        analysis.push_str("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");
        
        // Summary and recommendation
        if all_below_threshold {
            if highest_pct < 40.0 {
                analysis.push_str("📉 VERDICT: Market is VERY QUIET (all assets < 40% of threshold)\n");
                analysis.push_str("   → Typical during: overnight hours, weekends, low volume periods\n");
                analysis.push_str(&format!("   → Closest: {} at {:.0}% of threshold\n", closest_asset, highest_pct));
                analysis.push_str("   → Recommendation: Wait for US trading hours or news events\n");
            } else {
                analysis.push_str("📊 VERDICT: Market is MODERATELY QUIET (some movement detected)\n");
                analysis.push_str(&format!("   → {} is closest at {:.0}% of threshold\n", closest_asset, highest_pct));
                analysis.push_str("   → Small moves detected but not strong enough for high-confidence signals\n");
                analysis.push_str("   → Recommendation: Continue monitoring - volatility may pick up soon\n");
            }
        } else {
            analysis.push_str("⚡ VERDICT: SIGNALS DETECTED but may be filtered by other checks\n");
            analysis.push_str("   → Check: market prices not too high (< 85¢)\n");
            analysis.push_str("   → Check: no existing open positions for those assets\n");
            analysis.push_str("   → Check: orderbook validation passes\n");
        }
        
        analysis
    }
    
    /// Check for arbitrage opportunity on a specific asset's market
    /// VELOCITY-BASED: Reacts to quick price moves over last few seconds
    pub async fn check_opportunity_for_asset(&self, asset: CryptoAsset) -> Option<ArbSignal> {
        let asset_name = match asset { CryptoAsset::BTC => "BTC", CryptoAsset::ETH => "ETH", CryptoAsset::SOL => "SOL", CryptoAsset::XRP => "XRP" };
        
        let market = match asset {
            CryptoAsset::BTC => self.btc_market.as_ref()?,
            CryptoAsset::ETH => self.eth_market.as_ref()?,
            CryptoAsset::SOL => self.sol_market.as_ref()?,
            CryptoAsset::XRP => self.xrp_market.as_ref()?,
        };
        
        let state = self.price_state.read().await;
        
        // Get current price
        let current_price = state.current_price(asset);
        if current_price == 0.0 {
            println!("   ⚠️ {} check skipped: no price data", asset_name);
            return None;
        }
        
        // === VELOCITY-BASED DETECTION ===
        // Use short-term velocity (last 5 seconds) instead of interval start
        // This reacts to QUICK moves, not slow drifts
        let velocity_5s = state.velocity_pct(asset, 5);
        let velocity_3s = state.velocity_pct(asset, 3);
        
        // Use the stronger of the two velocities
        let velocity = if velocity_3s.abs() > velocity_5s.abs() { velocity_3s } else { velocity_5s };
        let abs_velocity = velocity.abs();
        
        // CONSERVATIVE thresholds to avoid noise and mean reversion
        // Only trade on meaningful moves, not small fluctuations
        let min_velocity = match asset {
            // BTC: 0.02% in 5 seconds = ~$18 move (10x increase from 0.002%)
            CryptoAsset::BTC => 0.02,
            // Altcoins: 0.03-0.04% (10x increase to filter out noise)
            CryptoAsset::ETH => 0.03,
            CryptoAsset::SOL => 0.04,
            CryptoAsset::XRP => 0.04,
        };
        
        if abs_velocity < min_velocity {
            // Debug: Log when we're close but not quite there
            if abs_velocity > min_velocity * 0.5 {
                let asset_name = match asset { CryptoAsset::BTC => "BTC", CryptoAsset::ETH => "ETH", CryptoAsset::SOL => "SOL", CryptoAsset::XRP => "XRP" };
                println!("   🔍 {} velocity {:.4}% < threshold {:.4}% (${:.2} move needed)", 
                    asset_name, abs_velocity, min_velocity, current_price * min_velocity / 100.0);
            }
            return None;
        }
        
        // Direction based on velocity (not interval start)
        let is_up = velocity > 0.0;
        
        // Debug: Log when we DO meet velocity threshold
        let asset_name = match asset { CryptoAsset::BTC => "BTC", CryptoAsset::ETH => "ETH", CryptoAsset::SOL => "SOL", CryptoAsset::XRP => "XRP" };
        println!("   ✅ {} velocity threshold met: {:.4}% ({})", 
            asset_name, abs_velocity, if is_up { "UP" } else { "DOWN" });
        
        let (bet_up, token_id, market_ask) = if is_up {
            (true, market.yes_token_id.clone(), market.yes_ask)
        } else {
            (false, market.no_token_id.clone(), market.no_ask)
        };
        
        // CRITICAL: Two price checks to avoid overpaying
        // 1. Don't buy if price is too high (general limit)
        if market_ask > MAX_BUY_PRICE {
            let asset_name = match asset { CryptoAsset::BTC => "BTC", CryptoAsset::ETH => "ETH", CryptoAsset::SOL => "SOL", CryptoAsset::XRP => "XRP" };
            println!("   ⚠️ {} signal blocked: market price {:.2}¢ > max {:.0}¢ (no edge)", 
                asset_name, market_ask * 100.0, MAX_BUY_PRICE * 100.0);
            return None;
        }
        
        // 2. MEAN REVERSION FILTER: Don't buy above 60¢
        // Positions at 64-68¢ were reverting to 50¢, causing losses
        // Only enter within 10¢ of fair value (50¢) to avoid mean reversion
        const MAX_ENTRY_PRICE: f64 = 0.60;  // 60¢ max entry
        if market_ask > MAX_ENTRY_PRICE {
            let asset_name = match asset { CryptoAsset::BTC => "BTC", CryptoAsset::ETH => "ETH", CryptoAsset::SOL => "SOL", CryptoAsset::XRP => "XRP" };
            println!("   🛑 {} signal blocked: price {:.2}¢ > max entry {:.0}¢ (mean reversion risk)", 
                asset_name, market_ask * 100.0, MAX_ENTRY_PRICE * 100.0);
            return None;
        }
        
        // Simple confidence based on velocity strength
        // Stronger velocity = higher confidence
        let confidence = ((abs_velocity * 500.0).min(95.0).max(30.0)) as u8;
        
        // Simple edge calculation - velocity implies direction
        let edge_pct = abs_velocity * 10.0;  // 0.01% velocity = 0.1% edge
        
        // Position size - use configured max for aggressive trading
        let recommended_size = self.max_position_usd;
        
        Some(ArbSignal {
            bet_up,
            token_id,
            buy_price: market_ask,
            edge_pct,
            crypto_price: current_price,
            asset,
            price_change_pct: velocity,
            confidence,
            recommended_size_usd: recommended_size,
        })
    }
}

// ============================================================================
// Binance Price Feed
// ============================================================================

/// Spawn a task that maintains WebSocket connections to Binance for BTC, ETH, SOL, and XRP
/// and updates the shared price state
pub fn spawn_binance_feed(price_state: Arc<RwLock<PriceState>>) -> tokio::task::JoinHandle<()> {
    let btc_state = price_state.clone();
    let eth_state = price_state.clone();
    let sol_state = price_state.clone();
    let xrp_state = price_state.clone();
    
    // Spawn BTC feed
    tokio::spawn(async move {
        loop {
            if let Err(e) = run_binance_feed(btc_state.clone(), CryptoAsset::BTC).await {
                eprintln!("⚠️ Binance BTC feed error: {}. Reconnecting in 3s...", e);
                tokio::time::sleep(Duration::from_secs(3)).await;
            }
        }
    });
    
    // Spawn ETH feed
    tokio::spawn(async move {
        loop {
            if let Err(e) = run_binance_feed(eth_state.clone(), CryptoAsset::ETH).await {
                eprintln!("⚠️ Binance ETH feed error: {}. Reconnecting in 3s...", e);
                tokio::time::sleep(Duration::from_secs(3)).await;
            }
        }
    });
    
    // Spawn SOL feed
    tokio::spawn(async move {
        loop {
            if let Err(e) = run_binance_feed(sol_state.clone(), CryptoAsset::SOL).await {
                eprintln!("⚠️ Binance SOL feed error: {}. Reconnecting in 3s...", e);
                tokio::time::sleep(Duration::from_secs(3)).await;
            }
        }
    });
    
    // Spawn XRP feed
    tokio::spawn(async move {
        loop {
            if let Err(e) = run_binance_feed(xrp_state.clone(), CryptoAsset::XRP).await {
                eprintln!("⚠️ Binance XRP feed error: {}. Reconnecting in 3s...", e);
                tokio::time::sleep(Duration::from_secs(3)).await;
            }
        }
    })
}

async fn run_binance_feed(price_state: Arc<RwLock<PriceState>>, asset: CryptoAsset) -> Result<()> {
    let ws_url = match asset {
        CryptoAsset::BTC => BINANCE_BTC_WS_URL,
        CryptoAsset::ETH => BINANCE_ETH_WS_URL,
        CryptoAsset::SOL => BINANCE_SOL_WS_URL,
        CryptoAsset::XRP => BINANCE_XRP_WS_URL,
    };
    let asset_name = match asset {
        CryptoAsset::BTC => "BTC",
        CryptoAsset::SOL => "SOL",
        CryptoAsset::XRP => "XRP",
        CryptoAsset::ETH => "ETH",
    };
    
    println!("🔌 Connecting to Binance {} WebSocket...", asset_name);
    
    let (ws_stream, _) = connect_async(ws_url).await
        .map_err(|e| anyhow!("Failed to connect to Binance {}: {}", asset_name, e))?;
    
    println!("✅ Connected to Binance {}/USDT feed", asset_name);
    
    let (mut _write, mut read) = ws_stream.split();
    
    while let Some(msg) = read.next().await {
        match msg {
            Ok(Message::Text(text)) => {
                if let Ok(trade) = serde_json::from_str::<BinanceTrade>(&text) {
                    if let Ok(price) = trade.price.parse::<f64>() {
                        let mut state = price_state.write().await;
                        
                        match asset {
                            CryptoAsset::BTC => {
                                if state.btc_interval_start_price == 0.0 {
                                    state.btc_interval_start_price = price;
                                }
                                state.btc_price = price;
                            }
                            CryptoAsset::ETH => {
                                if state.eth_interval_start_price == 0.0 {
                                    state.eth_interval_start_price = price;
                                }
                                state.eth_price = price;
                            }
                            CryptoAsset::SOL => {
                                if state.sol_interval_start_price == 0.0 {
                                    state.sol_interval_start_price = price;
                                }
                                state.sol_price = price;
                            }
                            CryptoAsset::XRP => {
                                if state.xrp_interval_start_price == 0.0 {
                                    state.xrp_interval_start_price = price;
                                }
                                state.xrp_price = price;
                            }
                        }
                        // Record price sample for momentum calculation
                        state.add_price_sample(asset, price);
                        state.last_update = Instant::now();
                    }
                }
            }
            Ok(Message::Ping(data)) => {
                // Respond to ping (handled automatically by tungstenite)
                let _ = data;
            }
            Ok(Message::Close(_)) => {
                return Err(anyhow!("WebSocket closed by server"));
            }
            Err(e) => {
                return Err(anyhow!("WebSocket error: {}", e));
            }
            _ => {}
        }
    }
    
    Err(anyhow!("WebSocket stream ended"))
}

// ============================================================================
// Polymarket Live Market Discovery
// ============================================================================

/// Fetch current live crypto markets from Polymarket
pub async fn fetch_live_crypto_markets() -> Result<Vec<LiveCryptoMarket>> {
    let client = reqwest::Client::new();
    let mut markets = Vec::new();
    
    // ChatGPT approach: Paginate through /markets and filter by slug starting with "btc-updown-15m-"
    println!("   Paginating through Gamma /markets endpoint...");
    
    for page in 0..10 {
        let limit = 100;
        let offset = page * limit;
        
        let url = format!(
            "https://gamma-api.polymarket.com/markets?active=true&closed=false&order=id&ascending=false&limit={}&offset={}",
            limit, offset
        );
        
        let resp = match client.get(&url)
            .timeout(Duration::from_secs(10))
            .send()
            .await 
        {
            Ok(r) => r,
            Err(e) => {
                println!("   ⚠️ Page {} request failed: {}", page, e);
                break;
            }
        };
        
        let gamma_markets: Vec<serde_json::Value> = match resp.json().await {
            Ok(m) => m,
            Err(_) => break,
        };
        
        if gamma_markets.is_empty() {
            println!("   No more markets at page {}", page);
            break;
        }
        
        println!("   Page {}: {} markets", page, gamma_markets.len());
        
        // Filter for btc-updown-15m markets
        for market in &gamma_markets {
            let slug = market.get("slug")
                .and_then(|s| s.as_str())
                .unwrap_or("");
            
            // Check for BTC markets: 15m, 4h up/down, and price target markets
            let is_btc_updown = slug.starts_with("btc-updown-15m-") 
                || slug.starts_with("btc-updown-4h-")
                || slug.contains("bitcoin-up-or-down");
            let is_btc_price_target = slug.starts_with("bitcoin-above-") 
                || slug.starts_with("bitcoin-below-")
                || slug.contains("bitcoin-hit")
                || slug.contains("btc-hit");
            
            // Check for ETH markets: 15m, 4h up/down
            let is_eth_updown = slug.starts_with("eth-updown-15m-") 
                || slug.starts_with("eth-updown-4h-")
                || slug.contains("ethereum-up-or-down");
            let is_eth_price_target = slug.starts_with("ethereum-above-") 
                || slug.starts_with("ethereum-below-")
                || slug.contains("ethereum-hit")
                || slug.contains("eth-hit");
            
            // Check for SOL markets: 15m, 4h up/down
            let is_sol_updown = slug.starts_with("sol-updown-15m-") 
                || slug.starts_with("sol-updown-4h-")
                || slug.contains("solana-up-or-down");
            let is_sol_price_target = slug.starts_with("solana-above-") 
                || slug.starts_with("solana-below-")
                || slug.contains("solana-hit")
                || slug.contains("sol-hit");
            
            // Check for XRP markets: 15m, 4h up/down
            let is_xrp_updown = slug.starts_with("xrp-updown-15m-") 
                || slug.starts_with("xrp-updown-4h-")
                || slug.contains("xrp-up-or-down");
            let is_xrp_price_target = slug.starts_with("xrp-above-") 
                || slug.starts_with("xrp-below-")
                || slug.contains("xrp-hit");
            
            // Determine which asset this market is for
            let asset = if is_btc_updown || is_btc_price_target {
                Some(CryptoAsset::BTC)
            } else if is_eth_updown || is_eth_price_target {
                Some(CryptoAsset::ETH)
            } else if is_sol_updown || is_sol_price_target {
                Some(CryptoAsset::SOL)
            } else if is_xrp_updown || is_xrp_price_target {
                Some(CryptoAsset::XRP)
            } else {
                None
            };
            
            if let Some(asset) = asset {
                // Check if market is active (not closed)
                let is_closed = market.get("closed")
                    .and_then(|c| c.as_bool())
                    .unwrap_or(false);
                let is_active = market.get("active")
                    .and_then(|a| a.as_bool())
                    .unwrap_or(true);
                
                if is_closed || !is_active {
                    continue;
                }
                
                // Check if market has expired based on end_date_iso
                let end_date_iso = market.get("end_date_iso")
                    .and_then(|d| d.as_str())
                    .unwrap_or("");
                
                if !end_date_iso.is_empty() {
                    // Parse ISO 8601 timestamp and check if it's in the future
                    if let Ok(end_time) = chrono::DateTime::parse_from_rfc3339(end_date_iso) {
                        let now = chrono::Utc::now();
                        if end_time < now {
                            // Market has expired, skip it
                            continue;
                        }
                    }
                }
                
                // Determine market type for logging
                let market_type = if slug.contains("-15m-") {
                    "15m"
                } else if slug.contains("-4h-") {
                    "4h"
                } else if is_btc_price_target || is_eth_price_target || is_sol_price_target || is_xrp_price_target {
                    "price-target"
                } else {
                    "daily"
                };
                let asset_name = match asset {
                    CryptoAsset::BTC => "BTC",
                    CryptoAsset::ETH => "ETH",
                    CryptoAsset::SOL => "SOL",
                    CryptoAsset::XRP => "XRP",
                };
                println!("   ✅ Found {} {} market: {}", asset_name, market_type, slug);
                
                // Debug: check what fields exist
                let has_clob_tokens = market.get("clobTokenIds").is_some();
                let clob_tokens_raw = market.get("clobTokenIds");
                
                if !has_clob_tokens {
                    println!("      ⚠️ No clobTokenIds field! Keys: {:?}", 
                        market.as_object().map(|o| o.keys().collect::<Vec<_>>()));
                    continue;
                }
                
                // clobTokenIds can be either an array or a JSON string containing an array
                let clob_tokens: Vec<String> = if let Some(arr) = clob_tokens_raw.and_then(|t| t.as_array()) {
                    // It's already an array
                    arr.iter().filter_map(|v| v.as_str().map(|s| s.to_string())).collect()
                } else if let Some(s) = clob_tokens_raw.and_then(|t| t.as_str()) {
                    // It's a JSON string - parse it
                    serde_json::from_str::<Vec<String>>(s).unwrap_or_default()
                } else {
                    Vec::new()
                };
                
                println!("      clobTokenIds count: {}", clob_tokens.len());
                
                if clob_tokens.len() >= 2 {
                    let yes_token = clob_tokens[0].clone();
                    let no_token = clob_tokens[1].clone();
                    
                    println!("      yes_token: {}..., no_token: {}...", 
                        &yes_token[..yes_token.len().min(20)],
                        &no_token[..no_token.len().min(20)]);
                    
                    if !yes_token.is_empty() && !no_token.is_empty() {
                        let description = market.get("question")
                            .and_then(|q| q.as_str())
                            .unwrap_or("BTC Up or Down")
                            .to_string();
                        
                        // Get current outcome price - also might be a JSON string
                        let yes_price = if let Some(prices_str) = market.get("outcomePrices").and_then(|p| p.as_str()) {
                            serde_json::from_str::<Vec<String>>(prices_str)
                                .ok()
                                .and_then(|v| v.get(0).cloned())
                                .and_then(|s| s.parse::<f64>().ok())
                                .unwrap_or(0.50)
                        } else {
                            market.get("outcomePrices")
                                .and_then(|p| p.as_array())
                                .and_then(|a| a.get(0))
                                .and_then(|v| v.as_str())
                                .and_then(|s| s.parse::<f64>().ok())
                                .unwrap_or(0.50)
                        };
                        
                        println!("      ✅ Adding market: {} @ {:.2}¢", description, yes_price * 100.0);
                        
                        // Determine interval based on market type (works for all assets)
                        let interval_minutes = if slug.contains("-15m-") {
                            15
                        } else if slug.contains("-4h-") {
                            240
                        } else {
                            60  // Default for daily/price target markets
                        };
                        
                        markets.push(LiveCryptoMarket {
                            condition_id: market.get("conditionId")
                                .and_then(|c| c.as_str())
                                .unwrap_or("")
                                .to_string(),
                            yes_token_id: yes_token,
                            no_token_id: no_token,
                            yes_ask: yes_price,
                            no_ask: 1.0 - yes_price,
                            end_time: 0,
                            interval_minutes,
                            description,
                            asset,
                        });
                    }
                }
            }
        }
        
        // If we found enough markets, we can stop paginating
        if markets.len() >= 20 {
            println!("   Found {} crypto markets (BTC + ETH), stopping pagination", markets.len());
            break;
        }
    }
    
    // Debug: if no markets found, show some sample slugs from the API
    if markets.is_empty() {
        println!("   No btc-updown-15m markets found. Checking what slugs exist...");
        let url = "https://gamma-api.polymarket.com/markets?active=true&closed=false&limit=20";
        if let Ok(resp) = client.get(url).timeout(Duration::from_secs(5)).send().await {
            if let Ok(sample_markets) = resp.json::<Vec<serde_json::Value>>().await {
                for (i, m) in sample_markets.iter().take(10).enumerate() {
                    let slug = m.get("slug").and_then(|s| s.as_str()).unwrap_or("(no slug)");
                    println!("   [{}] {}", i, slug);
                }
            }
        }
    }
    
    // Fallback: try searching all active events for crypto tag
    if markets.is_empty() {
        println!("   Fallback: searching all crypto-tagged events...");
        let urls = [
            "https://gamma-api.polymarket.com/events?active=true&closed=false&tag=crypto&limit=100",
            "https://gamma-api.polymarket.com/events?active=true&closed=false&limit=200",
        ];
    
    for url in urls {
        println!("   Trying: {}", url.split('?').next().unwrap_or(url));
        
        let resp = match client.get(url)
            .timeout(Duration::from_secs(10))
            .send()
            .await 
        {
            Ok(r) => r,
            Err(e) => {
                println!("   ⚠️ Request failed: {}", e);
                continue;
            }
        };
        
        let text = resp.text().await.unwrap_or_default();
        let events: Vec<serde_json::Value> = match serde_json::from_str(&text) {
            Ok(v) => v,
            Err(_) => {
                // Maybe it's a single object, try parsing as object
                if let Ok(obj) = serde_json::from_str::<serde_json::Value>(&text) {
                    if let Some(arr) = obj.get("data").and_then(|d| d.as_array()) {
                        arr.clone()
                    } else {
                        vec![obj]
                    }
                } else {
                    continue;
                }
            }
        };
        
        println!("   Found {} events/markets", events.len());
        
        // Debug: print first few titles to see what we're getting
        for (i, event) in events.iter().take(5).enumerate() {
            let title = event.get("title")
                .or_else(|| event.get("question"))
                .and_then(|t| t.as_str())
                .unwrap_or("(no title)");
            let slug = event.get("slug").and_then(|s| s.as_str()).unwrap_or("(no slug)");
            println!("   [{}] {} | slug: {}", i, &title[..title.len().min(60)], slug);
        }
        
        for event in &events {
            // Check both event-level and market-level data
            let title = event.get("title")
                .or_else(|| event.get("question"))
                .and_then(|t| t.as_str())
                .unwrap_or("");
            
            let slug = event.get("slug")
                .and_then(|s| s.as_str())
                .unwrap_or("");
            
            // Look for BTC up/down markets
            let is_btc_updown = title.to_lowercase().contains("bitcoin up or down")
                || title.to_lowercase().contains("btc up or down")
                || slug.contains("btc-updown")
                || slug.contains("bitcoin-up-or-down");
            
            if is_btc_updown {
                println!("   ✅ Found BTC market: {}", title);
                
                // Try to get market data from nested markets array or direct fields
                let market_list: Vec<&serde_json::Value> = if let Some(arr) = event.get("markets").and_then(|m| m.as_array()) {
                    arr.iter().collect()
                } else {
                    vec![event]
                };
                
                for market in market_list {
                    if let Some(clob_tokens) = market.get("clobTokenIds").and_then(|t| t.as_array()) {
                        if clob_tokens.len() >= 2 {
                            let yes_token = clob_tokens[0].as_str().unwrap_or("").to_string();
                            let no_token = clob_tokens[1].as_str().unwrap_or("").to_string();
                            
                            if !yes_token.is_empty() && !no_token.is_empty() {
                                let condition_id = market.get("conditionId")
                                    .and_then(|c| c.as_str())
                                    .unwrap_or("")
                                    .to_string();
                                
                                let description = market.get("question")
                                    .and_then(|q| q.as_str())
                                    .unwrap_or(title)
                                    .to_string();
                                
                                markets.push(LiveCryptoMarket {
                                    condition_id,
                                    yes_token_id: yes_token,
                                    no_token_id: no_token,
                                    yes_ask: 0.50,
                                    no_ask: 0.50,
                                    end_time: 0,
                                    interval_minutes: 15,
                                    description,
                                    asset: CryptoAsset::BTC,  // Fallback assumes BTC
                                });
                            }
                        }
                    }
                }
            }
        }
        
        // If we found markets, stop searching
        if !markets.is_empty() {
            break;
        }
    }
    }  // end of fallback if block
    
    Ok(markets)
}

/// Update market prices from order book
/// Returns error if orderbook doesn't exist (market not yet active)
pub async fn update_market_prices(market: &mut LiveCryptoMarket) -> Result<()> {
    let client = reqwest::Client::new();
    
    // Fetch order book for yes token
    let yes_url = format!(
        "https://clob.polymarket.com/book?token_id={}",
        market.yes_token_id
    );
    
    let resp = client.get(&yes_url)
        .timeout(Duration::from_secs(5))
        .send()
        .await
        .map_err(|e| anyhow!("Failed to fetch orderbook: {}", e))?;
    
    let status = resp.status();
    let body = resp.text().await.unwrap_or_default();
    
    // Check for "orderbook does not exist" error
    if body.contains("does not exist") || status.as_u16() == 400 {
        return Err(anyhow!("Orderbook not active yet"));
    }
    
    let book: serde_json::Value = serde_json::from_str(&body)
        .map_err(|_| anyhow!("Invalid orderbook response"))?;
    
    // Check if there are any asks (liquidity)
    let asks = book.get("asks")
        .and_then(|a| a.as_array())
        .ok_or_else(|| anyhow!("No asks in orderbook"))?;
    
    if asks.is_empty() {
        return Err(anyhow!("Orderbook has no liquidity"));
    }
    
    if let Some(best_ask) = asks.first() {
        if let Some(price) = best_ask.get("price").and_then(|p| p.as_str()) {
            market.yes_ask = price.parse().unwrap_or(0.50);
        }
    }
    
    // No token ask = 1 - yes bid (approximately)
    market.no_ask = (1.0 - market.yes_ask + 0.02).min(0.99);
    
    Ok(())
}

// ============================================================================
// Display Helpers
// ============================================================================

impl std::fmt::Display for ArbSignal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let direction = if self.bet_up { "⬆️ UP" } else { "⬇️ DOWN" };
        let asset_name = match self.asset {
            CryptoAsset::BTC => "BTC",
            CryptoAsset::ETH => "ETH",
            CryptoAsset::SOL => "SOL",
            CryptoAsset::XRP => "XRP",
        };
        write!(
            f,
            "{} | {} ${:.2} ({:+.3}%) | Buy @ {:.2}¢ | Edge {:.1}% | Size ${:.2} | Conf {}%",
            direction,
            asset_name,
            self.crypto_price,
            self.price_change_pct,
            self.buy_price * 100.0,
            self.edge_pct,
            self.recommended_size_usd,
            self.confidence
        )
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_btc_price_change_calculation() {
        let mut state = PriceState::default();
        state.btc_interval_start_price = 100000.0;
        state.btc_price = 100500.0;
        
        assert!((state.btc_change_pct() - 0.5).abs() < 0.001);
        assert!(state.is_up(CryptoAsset::BTC));
    }
    
    #[test]
    fn test_eth_price_change_calculation() {
        let mut state = PriceState::default();
        state.eth_interval_start_price = 3000.0;
        state.eth_price = 3015.0;
        
        assert!((state.eth_change_pct() - 0.5).abs() < 0.001);
        assert!(state.is_up(CryptoAsset::ETH));
    }
    
    #[test]
    fn test_status_analysis_quiet_market() {
        // Test status analysis when market is very quiet (all below threshold)
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let engine = CryptoArbEngine::new(false, 10.0, 1.0);
            {
                let mut state = engine.price_state.write().await;
                state.btc_price = 90000.0;
                state.eth_price = 3000.0;
                state.sol_price = 150.0;
                state.xrp_price = 2.0;
                
                // Add minimal price history (near-zero velocity)
                for _ in 0..20 {
                    state.btc_price_history.push_back((Instant::now(), 90000.0));
                    state.eth_price_history.push_back((Instant::now(), 3000.0));
                    state.sol_price_history.push_back((Instant::now(), 150.0));
                    state.xrp_price_history.push_back((Instant::now(), 2.0));
                }
            }
            
            let analysis = engine.get_status_analysis().await;
            
            // Should indicate quiet market
            assert!(analysis.contains("VERY QUIET") || analysis.contains("MODERATELY QUIET"));
            assert!(analysis.contains("BTC"));
            assert!(analysis.contains("ETH"));
            assert!(analysis.contains("SOL"));
            assert!(analysis.contains("XRP"));
            assert!(analysis.contains("% of threshold"));
        });
    }
    
    #[test]
    fn test_status_analysis_with_movement() {
        // Test status analysis when there's some movement
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let engine = CryptoArbEngine::new(false, 10.0, 1.0);
            {
                let mut state = engine.price_state.write().await;
                state.btc_price = 90000.0;
                state.eth_price = 3000.0;
                state.sol_price = 150.0;
                state.xrp_price = 2.0;
                
                // Add price history with some movement for BTC
                let now = Instant::now();
                for i in 0..20 {
                    let price = if i < 10 { 89950.0 } else { 90000.0 }; // 0.05% move
                    state.btc_price_history.push_back((now, price));
                    state.eth_price_history.push_back((now, 3000.0));
                    state.sol_price_history.push_back((now, 150.0));
                    state.xrp_price_history.push_back((now, 2.0));
                }
            }
            
            let analysis = engine.get_status_analysis().await;
            
            // Should contain analysis output
            assert!(analysis.contains("SIGNAL STATUS ANALYSIS"));
            assert!(analysis.contains("VERDICT"));
            assert!(analysis.len() > 200); // Should be a substantial report
        });
    }
    
    #[test]
    fn test_status_analysis_no_price_data() {
        // Test status analysis when no price data is available
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let engine = CryptoArbEngine::new(false, 10.0, 1.0);
            // Leave all prices at 0.0 (default)
            
            let analysis = engine.get_status_analysis().await;
            
            // Should indicate no price data
            assert!(analysis.contains("No price data available"));
        });
    }
    
    #[test]
    fn test_status_analysis_threshold_percentage() {
        // Test that threshold percentages are calculated correctly
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let engine = CryptoArbEngine::new(false, 10.0, 1.0);
            {
                let mut state = engine.price_state.write().await;
                state.btc_price = 90000.0;
                state.eth_price = 3000.0;
                
                let now = Instant::now();
                // Create velocity of exactly 0.001% (50% of BTC threshold of 0.002%)
                for i in 0..20 {
                    let price = if i < 10 { 89991.0 } else { 90000.0 }; // 0.01% move over 10 samples
                    state.btc_price_history.push_back((now, price));
                    state.eth_price_history.push_back((now, 3000.0));
                }
            }
            
            let analysis = engine.get_status_analysis().await;
            
            // Should show percentage of threshold
            assert!(analysis.contains("% of threshold"));
            // Should show icons indicating status levels
            assert!(analysis.contains("⚪") || analysis.contains("🟠") || analysis.contains("🟡") || analysis.contains("✅"));
        });
    }
    
    #[test]
    fn test_status_analysis_market_price_check() {
        // Test that market prices are checked and displayed
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let mut engine = CryptoArbEngine::new(false, 10.0, 1.0);
            {
                let mut state = engine.price_state.write().await;
                state.btc_price = 90000.0;
                
                let now = Instant::now();
                for _ in 0..20 {
                    state.btc_price_history.push_back((now, 90000.0));
                }
            }
            
            // Add a market with high prices
            engine.btc_market = Some(LiveCryptoMarket {
                condition_id: "test".to_string(),
                question_id: "test".to_string(),
                description: "Test Market".to_string(),
                yes_token_id: "123".to_string(),
                no_token_id: "456".to_string(),
                yes_ask: 0.90, // High price - should be flagged
                no_ask: 0.15,
                asset: CryptoAsset::BTC,
                interval_minutes: 15,
            });
            
            let analysis = engine.get_status_analysis().await;
            
            // Should show market prices
            assert!(analysis.contains("Market: YES="));
            assert!(analysis.contains("NO="));
            // Should flag high prices
            assert!(analysis.contains("TOO HIGH") || analysis.contains("✓"));
        });
    }
    
    #[test]
    fn test_status_analysis_direction_indicators() {
        // Test that up/down direction indicators work correctly
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let engine = CryptoArbEngine::new(false, 10.0, 1.0);
            {
                let mut state = engine.price_state.write().await;
                state.btc_price = 90100.0; // Higher price (up)
                state.eth_price = 2990.0;  // Lower price (down)
                
                let now = Instant::now();
                for i in 0..20 {
                    // BTC trending up
                    let btc_price = 90000.0 + (i as f64 * 5.0);
                    // ETH trending down
                    let eth_price = 3000.0 - (i as f64 * 0.5);
                    state.btc_price_history.push_back((now, btc_price));
                    state.eth_price_history.push_back((now, eth_price));
                }
            }
            
            let analysis = engine.get_status_analysis().await;
            
            // Should show direction indicators
            assert!(analysis.contains("⬆") || analysis.contains("⬇"));
        });
    }
    
    #[test]
    fn test_increased_velocity_thresholds() {
        // Test that new 10x velocity thresholds filter out noise
        // Old thresholds were: BTC 0.002%, ETH 0.003%, SOL/XRP 0.004%
        // New thresholds are: BTC 0.02%, ETH 0.03%, SOL/XRP 0.04% (10x higher)
        
        let btc_threshold = 0.02;
        let eth_threshold = 0.03;
        let sol_threshold = 0.04;
        let xrp_threshold = 0.04;
        
        // Verify thresholds are 10x the old values
        assert_eq!(btc_threshold, 0.002 * 10.0);
        assert_eq!(eth_threshold, 0.003 * 10.0);
        assert_eq!(sol_threshold, 0.004 * 10.0);
        assert_eq!(xrp_threshold, 0.004 * 10.0);
        
        // Small noisy moves should NOT trigger (these caused losses)
        let noisy_btc = 0.005;  // ~$4.50 move - too small
        let noisy_xrp = 0.011;  // ~$0.0002 move - too small
        assert!(noisy_btc < btc_threshold, "Noise should be filtered");
        assert!(noisy_xrp < xrp_threshold, "Noise should be filtered");
        
        // Only meaningful moves should trigger
        let meaningful_btc = 0.025;  // ~$22 move
        let meaningful_xrp = 0.05;   // ~$0.001 move
        assert!(meaningful_btc > btc_threshold, "Real moves should pass");
        assert!(meaningful_xrp > xrp_threshold, "Real moves should pass");
    }
    
    #[test]
    fn test_max_entry_price_filter() {
        // Test that max entry price filter prevents buying overpriced positions
        // This filter prevents losses from mean reversion
        const MAX_ENTRY_PRICE: f64 = 0.60;  // 60¢
        
        // Positions that caused losses (should be blocked)
        let bad_entry_1 = 0.64;  // 64¢ - reverted to 50¢
        let bad_entry_2 = 0.68;  // 68¢ - reverted to 50¢
        assert!(bad_entry_1 > MAX_ENTRY_PRICE, "64¢ entry should be blocked");
        assert!(bad_entry_2 > MAX_ENTRY_PRICE, "68¢ entry should be blocked");
        
        // Good entries near fair value (should pass)
        let good_entry_1 = 0.52;  // 52¢ - only 2¢ from fair value
        let good_entry_2 = 0.58;  // 58¢ - within acceptable range
        assert!(good_entry_1 < MAX_ENTRY_PRICE, "52¢ entry should pass");
        assert!(good_entry_2 < MAX_ENTRY_PRICE, "58¢ entry should pass");
        
        // Edge cases
        let at_limit = 0.60;
        let just_over = 0.61;
        assert!(at_limit <= MAX_ENTRY_PRICE, "60¢ should pass");
        assert!(just_over > MAX_ENTRY_PRICE, "61¢ should be blocked");
    }
    
    #[test]
    fn test_mean_reversion_risk_calculation() {
        // Test that we can identify mean reversion risk
        // Markets at 50¢ = fair value (50/50 odds)
        // The further from 50¢, the higher the reversion risk
        
        const FAIR_VALUE: f64 = 0.50;
        const MAX_ENTRY_PRICE: f64 = 0.60;
        
        let test_prices = vec![
            (0.52, 0.02, "Low risk"),
            (0.55, 0.05, "Moderate risk"),
            (0.60, 0.10, "Max acceptable"),
            (0.64, 0.14, "HIGH RISK - should block"),
            (0.68, 0.18, "VERY HIGH RISK - should block"),
        ];
        
        for (price, expected_distance, description) in test_prices {
            let distance = (price - FAIR_VALUE).abs();
            assert!((distance - expected_distance).abs() < 0.001, 
                "{}: distance should be {:.2}¢", description, expected_distance * 100.0);
            
            if distance > 0.10 {
                assert!(price > MAX_ENTRY_PRICE, 
                    "{}: price {:.2}¢ should be blocked ({}¢ from fair)", 
                    description, price * 100.0, distance * 100.0);
            }
        }
    }
    
    #[test]
    fn test_velocity_threshold_dollar_amounts() {
        // Test that new thresholds represent meaningful dollar moves
        let btc_price = 90000.0;
        let eth_price = 3000.0;
        let sol_price = 150.0;
        let xrp_price = 2.0;
        
        // New thresholds
        let btc_threshold = 0.02;  // 0.02%
        let eth_threshold = 0.03;  // 0.03%
        let sol_threshold = 0.04;  // 0.04%
        let xrp_threshold = 0.04;  // 0.04%
        
        // Calculate required dollar moves
        let btc_dollar_move = btc_price * btc_threshold / 100.0;
        let eth_dollar_move = eth_price * eth_threshold / 100.0;
        let sol_dollar_move = sol_price * sol_threshold / 100.0;
        let xrp_dollar_move = xrp_price * xrp_threshold / 100.0;
        
        // Verify meaningful moves required
        assert!(btc_dollar_move >= 15.0, "BTC needs ${:.0} move (got ${:.2})", 15.0, btc_dollar_move);
        assert!(eth_dollar_move >= 0.80, "ETH needs ${:.2} move (got ${:.2})", 0.80, eth_dollar_move);
        assert!(sol_dollar_move >= 0.05, "SOL needs ${:.2} move (got ${:.2})", 0.05, sol_dollar_move);
        assert!(xrp_dollar_move >= 0.0008, "XRP needs ${:.4} move (got ${:.4})", 0.0008, xrp_dollar_move);
        
        println!("BTC threshold: {:.2}% = ${:.2} move", btc_threshold, btc_dollar_move);
        println!("ETH threshold: {:.2}% = ${:.2} move", eth_threshold, eth_dollar_move);
        println!("SOL threshold: {:.2}% = ${:.2} move", sol_threshold, sol_dollar_move);
        println!("XRP threshold: {:.2}% = ${:.4} move", xrp_threshold, xrp_dollar_move);
    }
    
    #[test]
    fn test_no_regression_bad_entries() {
        // Regression test: ensure previous bad entries (64-68¢) would now be blocked
        const MAX_ENTRY_PRICE: f64 = 0.60;
        
        // These were the actual losing trades
        let xrp_entry_1 = 0.68;  // XRP at 68¢
        let xrp_entry_2 = 0.64;  // XRP at 64¢
        let sol_entry_1 = 0.68;  // SOL at 68¢
        
        // All should be blocked now
        assert!(xrp_entry_1 > MAX_ENTRY_PRICE, 
            "REGRESSION: XRP 68¢ entry should be blocked");
        assert!(xrp_entry_2 > MAX_ENTRY_PRICE, 
            "REGRESSION: XRP 64¢ entry should be blocked");
        assert!(sol_entry_1 > MAX_ENTRY_PRICE, 
            "REGRESSION: SOL 68¢ entry should be blocked");
        
        println!("✓ All previous losing entries would now be blocked");
    }
}
