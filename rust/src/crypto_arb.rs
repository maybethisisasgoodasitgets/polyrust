/// Crypto Latency Arbitrage Module
/// 
/// Monitors real-time BTC prices from Binance and compares against
/// Polymarket's live crypto markets to find arbitrage opportunities.
/// 
/// Strategy: When BTC price moves but Polymarket odds haven't caught up,
/// bet on the near-certain outcome.

use anyhow::{Result, anyhow};
use futures::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::sync::{mpsc, RwLock};
use tokio_tungstenite::{connect_async, tungstenite::protocol::Message};

// ============================================================================
// Configuration
// ============================================================================

/// Minimum price move (%) to trigger a bet
pub const MIN_PRICE_MOVE_PCT: f64 = 0.10;  // 0.1% move

/// Maximum odds to buy (e.g., 0.95 = 95 cents for $1 payout)
pub const MAX_BUY_PRICE: f64 = 0.92;

/// Minimum edge required (difference between true prob and market odds)
pub const MIN_EDGE_PCT: f64 = 2.0;  // 2% edge minimum

/// How often to check for opportunities (ms)
pub const CHECK_INTERVAL_MS: u64 = 100;

/// Binance WebSocket URL for BTC/USDT trades
pub const BINANCE_WS_URL: &str = "wss://stream.binance.com:9443/ws/btcusdt@trade";

/// Binance WebSocket URL for BTC/USDT ticker (more frequent updates)
pub const BINANCE_TICKER_WS_URL: &str = "wss://stream.binance.com:9443/ws/btcusdt@ticker";

// ============================================================================
// Price State
// ============================================================================

#[derive(Debug, Clone)]
pub struct PriceState {
    /// Current BTC price from Binance
    pub btc_price: f64,
    /// Price at the start of the current Polymarket interval
    pub interval_start_price: f64,
    /// Timestamp of last price update
    pub last_update: Instant,
    /// Timestamp of interval start
    pub interval_start_time: Instant,
}

impl Default for PriceState {
    fn default() -> Self {
        Self {
            btc_price: 0.0,
            interval_start_price: 0.0,
            last_update: Instant::now(),
            interval_start_time: Instant::now(),
        }
    }
}

impl PriceState {
    /// Calculate price change percentage since interval start
    pub fn price_change_pct(&self) -> f64 {
        if self.interval_start_price == 0.0 {
            return 0.0;
        }
        ((self.btc_price - self.interval_start_price) / self.interval_start_price) * 100.0
    }
    
    /// Returns true if price is up since interval start
    pub fn is_up(&self) -> bool {
        self.btc_price > self.interval_start_price
    }
    
    /// Returns true if price is down since interval start
    pub fn is_down(&self) -> bool {
        self.btc_price < self.interval_start_price
    }
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
    /// Current BTC price
    pub btc_price: f64,
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
    /// Current live crypto market info
    market: Option<LiveCryptoMarket>,
    /// Mock mode (don't execute real trades)
    mock_mode: bool,
    /// Maximum position size per trade
    max_position_usd: f64,
    /// Minimum position size per trade
    min_position_usd: f64,
}

impl CryptoArbEngine {
    pub fn new(mock_mode: bool, max_position_usd: f64, min_position_usd: f64) -> Self {
        Self {
            price_state: Arc::new(RwLock::new(PriceState::default())),
            market: None,
            mock_mode,
            max_position_usd,
            min_position_usd,
        }
    }
    
    /// Get shared price state for external access
    pub fn price_state(&self) -> Arc<RwLock<PriceState>> {
        self.price_state.clone()
    }
    
    /// Set the current live crypto market to monitor
    pub fn set_market(&mut self, market: LiveCryptoMarket) {
        self.market = Some(market);
    }
    
    /// Check for arbitrage opportunity
    pub async fn check_opportunity(&self) -> Option<ArbSignal> {
        let market = self.market.as_ref()?;
        let state = self.price_state.read().await;
        
        // Need valid prices
        if state.btc_price == 0.0 || state.interval_start_price == 0.0 {
            return None;
        }
        
        let change_pct = state.price_change_pct();
        let abs_change = change_pct.abs();
        
        // Need minimum price movement
        if abs_change < MIN_PRICE_MOVE_PCT {
            return None;
        }
        
        // Determine direction and get relevant market prices
        let (bet_up, token_id, market_ask) = if state.is_up() {
            (true, market.yes_token_id.clone(), market.yes_ask)
        } else {
            (false, market.no_token_id.clone(), market.no_ask)
        };
        
        // Check if market price is attractive enough
        if market_ask > MAX_BUY_PRICE {
            return None;
        }
        
        // Calculate edge: if price moved X%, true probability is higher than market implies
        // Simple model: larger move = higher confidence
        let implied_prob = 0.50 + (abs_change * 5.0).min(45.0) / 100.0;  // Cap at 95%
        let market_prob = market_ask;
        let edge_pct = (implied_prob - market_prob) * 100.0;
        
        if edge_pct < MIN_EDGE_PCT {
            return None;
        }
        
        // Calculate confidence (0-100)
        let confidence = ((abs_change * 20.0).min(100.0)) as u8;
        
        // Calculate recommended size based on edge (Kelly-lite)
        let kelly_fraction = (edge_pct / 100.0) / (1.0 - market_ask);
        let recommended_size = (self.max_position_usd * kelly_fraction.min(0.25))
            .max(self.min_position_usd)
            .min(self.max_position_usd);
        
        Some(ArbSignal {
            bet_up,
            token_id,
            buy_price: market_ask,
            edge_pct,
            btc_price: state.btc_price,
            price_change_pct: change_pct,
            confidence,
            recommended_size_usd: recommended_size,
        })
    }
    
    /// Reset interval (call when new Polymarket interval starts)
    pub async fn reset_interval(&self) {
        let mut state = self.price_state.write().await;
        state.interval_start_price = state.btc_price;
        state.interval_start_time = Instant::now();
    }
}

// ============================================================================
// Binance Price Feed
// ============================================================================

/// Spawn a task that maintains a WebSocket connection to Binance
/// and updates the shared price state
pub fn spawn_binance_feed(price_state: Arc<RwLock<PriceState>>) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            if let Err(e) = run_binance_feed(price_state.clone()).await {
                eprintln!("⚠️ Binance feed error: {}. Reconnecting in 3s...", e);
                tokio::time::sleep(Duration::from_secs(3)).await;
            }
        }
    })
}

async fn run_binance_feed(price_state: Arc<RwLock<PriceState>>) -> Result<()> {
    println!("🔌 Connecting to Binance WebSocket...");
    
    let (ws_stream, _) = connect_async(BINANCE_WS_URL).await
        .map_err(|e| anyhow!("Failed to connect to Binance: {}", e))?;
    
    println!("✅ Connected to Binance BTC/USDT feed");
    
    let (mut _write, mut read) = ws_stream.split();
    
    while let Some(msg) = read.next().await {
        match msg {
            Ok(Message::Text(text)) => {
                if let Ok(trade) = serde_json::from_str::<BinanceTrade>(&text) {
                    if let Ok(price) = trade.price.parse::<f64>() {
                        let mut state = price_state.write().await;
                        
                        // Initialize interval start price if not set
                        if state.interval_start_price == 0.0 {
                            state.interval_start_price = price;
                            state.interval_start_time = Instant::now();
                        }
                        
                        state.btc_price = price;
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
    // Polymarket's live crypto markets follow a pattern
    // We need to query their API for active "BTC up or down" markets
    
    let client = reqwest::Client::new();
    
    // Query Gamma API for live crypto events
    let url = "https://gamma-api.polymarket.com/events?active=true&tag=crypto&limit=50";
    
    let resp = client.get(url)
        .timeout(Duration::from_secs(10))
        .send()
        .await?;
    
    let events: Vec<serde_json::Value> = resp.json().await?;
    
    let mut markets = Vec::new();
    
    for event in events {
        // Look for live BTC price markets
        let title = event.get("title")
            .and_then(|t| t.as_str())
            .unwrap_or("");
        
        if title.to_lowercase().contains("bitcoin") || title.to_lowercase().contains("btc") {
            if let Some(event_markets) = event.get("markets").and_then(|m| m.as_array()) {
                for market in event_markets {
                    // Extract market details
                    if let (Some(condition_id), Some(yes_token), Some(no_token)) = (
                        market.get("conditionId").and_then(|c| c.as_str()),
                        market.get("clobTokenIds").and_then(|t| t.as_array()).and_then(|a| a.get(0)).and_then(|t| t.as_str()),
                        market.get("clobTokenIds").and_then(|t| t.as_array()).and_then(|a| a.get(1)).and_then(|t| t.as_str()),
                    ) {
                        let description = market.get("question")
                            .and_then(|q| q.as_str())
                            .unwrap_or("")
                            .to_string();
                        
                        // Check if this is a live/active market
                        let active = market.get("active")
                            .and_then(|a| a.as_bool())
                            .unwrap_or(false);
                        
                        if active && (description.to_lowercase().contains("up") || description.to_lowercase().contains("down")) {
                            markets.push(LiveCryptoMarket {
                                condition_id: condition_id.to_string(),
                                yes_token_id: yes_token.to_string(),
                                no_token_id: no_token.to_string(),
                                yes_ask: 0.50,  // Will be updated from order book
                                no_ask: 0.50,
                                end_time: 0,
                                interval_minutes: 5,  // Default assumption
                                description,
                            });
                        }
                    }
                }
            }
        }
    }
    
    Ok(markets)
}

/// Update market prices from order book
pub async fn update_market_prices(market: &mut LiveCryptoMarket) -> Result<()> {
    let client = reqwest::Client::new();
    
    // Fetch order book for yes token
    let yes_url = format!(
        "https://clob.polymarket.com/book?token_id={}",
        market.yes_token_id
    );
    
    if let Ok(resp) = client.get(&yes_url)
        .timeout(Duration::from_secs(5))
        .send()
        .await
    {
        if let Ok(book) = resp.json::<serde_json::Value>().await {
            if let Some(asks) = book.get("asks").and_then(|a| a.as_array()) {
                if let Some(best_ask) = asks.first() {
                    if let Some(price) = best_ask.get("price").and_then(|p| p.as_str()) {
                        market.yes_ask = price.parse().unwrap_or(0.50);
                    }
                }
            }
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
        write!(
            f,
            "{} | BTC ${:.2} ({:+.3}%) | Buy @ {:.2}¢ | Edge {:.1}% | Size ${:.2} | Conf {}%",
            direction,
            self.btc_price,
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
    fn test_price_change_calculation() {
        let mut state = PriceState::default();
        state.interval_start_price = 100000.0;
        state.btc_price = 100500.0;
        
        assert!((state.price_change_pct() - 0.5).abs() < 0.001);
        assert!(state.is_up());
        assert!(!state.is_down());
    }
    
    #[test]
    fn test_price_down() {
        let mut state = PriceState::default();
        state.interval_start_price = 100000.0;
        state.btc_price = 99500.0;
        
        assert!((state.price_change_pct() - (-0.5)).abs() < 0.001);
        assert!(!state.is_up());
        assert!(state.is_down());
    }
}
