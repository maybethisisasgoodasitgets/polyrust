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
pub const BINANCE_BTC_WS_URL: &str = "wss://stream.binance.com:9443/ws/btcusdt@trade";

/// Binance WebSocket URL for ETH/USDT trades
pub const BINANCE_ETH_WS_URL: &str = "wss://stream.binance.com:9443/ws/ethusdt@trade";

/// Binance WebSocket URL for BTC/USDT ticker (more frequent updates)
pub const BINANCE_TICKER_WS_URL: &str = "wss://stream.binance.com:9443/ws/btcusdt@ticker";

/// Crypto asset type
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CryptoAsset {
    BTC,
    ETH,
}

// ============================================================================
// Price State
// ============================================================================

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
    /// Timestamp of last price update
    pub last_update: Instant,
    /// Timestamp of interval start
    pub interval_start_time: Instant,
}

impl Default for PriceState {
    fn default() -> Self {
        Self {
            btc_price: 0.0,
            btc_interval_start_price: 0.0,
            eth_price: 0.0,
            eth_interval_start_price: 0.0,
            last_update: Instant::now(),
            interval_start_time: Instant::now(),
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
    
    /// Get price change for a specific asset
    pub fn price_change_pct(&self, asset: CryptoAsset) -> f64 {
        match asset {
            CryptoAsset::BTC => self.btc_change_pct(),
            CryptoAsset::ETH => self.eth_change_pct(),
        }
    }
    
    /// Get current price for a specific asset
    pub fn current_price(&self, asset: CryptoAsset) -> f64 {
        match asset {
            CryptoAsset::BTC => self.btc_price,
            CryptoAsset::ETH => self.eth_price,
        }
    }
    
    /// Returns true if asset price is up since interval start
    pub fn is_up(&self, asset: CryptoAsset) -> bool {
        match asset {
            CryptoAsset::BTC => self.btc_price > self.btc_interval_start_price,
            CryptoAsset::ETH => self.eth_price > self.eth_interval_start_price,
        }
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
        let asset = market.asset;
        
        // Need valid prices for the relevant asset
        let (current_price, interval_start) = match asset {
            CryptoAsset::BTC => (state.btc_price, state.btc_interval_start_price),
            CryptoAsset::ETH => (state.eth_price, state.eth_interval_start_price),
        };
        
        if current_price == 0.0 || interval_start == 0.0 {
            return None;
        }
        
        let change_pct = state.price_change_pct(asset);
        let abs_change = change_pct.abs();
        
        // Market-type-specific minimum price move thresholds
        // Shorter timeframes need smaller moves, longer need bigger
        let min_move = match market.interval_minutes {
            5 => 0.05,      // 5-minute: 0.05% move
            15 => 0.10,     // 15-minute: 0.10% move  
            60 => 0.20,     // 1-hour/daily: 0.20% move
            240 => 0.30,    // 4-hour: 0.30% move
            _ => 0.15,      // Default: 0.15% move
        };
        
        // Need minimum price movement for this market type
        if abs_change < min_move {
            return None;
        }
        
        // Debug: log when we pass the price move threshold
        let asset_name = match asset {
            CryptoAsset::BTC => "BTC",
            CryptoAsset::ETH => "ETH",
        };
        println!("   🔍 {} move {:.3}% (threshold {:.2}%) - checking edge...", asset_name, change_pct, min_move);
        
        // Determine direction and get relevant market prices
        let (bet_up, token_id, market_ask) = if state.is_up(asset) {
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
        let implied_prob = 0.50 + (abs_change * prob_multiplier).min(45.0) / 100.0;  // Cap at 95%
        let market_prob = market_ask;
        let edge_pct = (implied_prob - market_prob) * 100.0;
        
        // Minimum edge also varies by market type
        let min_edge = match market.interval_minutes {
            5 => 1.5,       // 5-minute: lower edge OK (faster resolution)
            15 => 2.0,      // 15-minute: standard
            60 => 2.5,      // 1-hour: need more edge
            240 => 3.0,     // 4-hour: need even more edge
            _ => 2.0,
        };
        
        if edge_pct < min_edge {
            println!("   ⏭️ Edge {:.2}% < min {:.1}% (market @ {:.0}¢)", edge_pct, min_edge, market_ask * 100.0);
            return None;
        }
        
        // Calculate confidence (0-100) - scaled by market type
        let confidence_multiplier = match market.interval_minutes {
            5 => 30.0,      // 5-minute: small moves = high confidence
            15 => 20.0,     // 15-minute: standard
            60 => 15.0,     // 1-hour: need bigger moves
            240 => 10.0,    // 4-hour: need even bigger moves
            _ => 20.0,
        };
        let confidence = ((abs_change * confidence_multiplier).min(100.0)) as u8;
        
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
            crypto_price: current_price,
            asset,
            price_change_pct: change_pct,
            confidence,
            recommended_size_usd: recommended_size,
        })
    }
    
    /// Reset interval for both assets (call when new Polymarket interval starts)
    pub async fn reset_interval(&self) {
        let mut state = self.price_state.write().await;
        state.btc_interval_start_price = state.btc_price;
        state.eth_interval_start_price = state.eth_price;
        state.interval_start_time = Instant::now();
    }
}

// ============================================================================
// Binance Price Feed
// ============================================================================

/// Spawn a task that maintains WebSocket connections to Binance for both BTC and ETH
/// and updates the shared price state
pub fn spawn_binance_feed(price_state: Arc<RwLock<PriceState>>) -> tokio::task::JoinHandle<()> {
    let btc_state = price_state.clone();
    let eth_state = price_state.clone();
    
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
    })
}

async fn run_binance_feed(price_state: Arc<RwLock<PriceState>>, asset: CryptoAsset) -> Result<()> {
    let ws_url = match asset {
        CryptoAsset::BTC => BINANCE_BTC_WS_URL,
        CryptoAsset::ETH => BINANCE_ETH_WS_URL,
    };
    let asset_name = match asset {
        CryptoAsset::BTC => "BTC",
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
                                // Initialize interval start price if not set
                                if state.btc_interval_start_price == 0.0 {
                                    state.btc_interval_start_price = price;
                                }
                                state.btc_price = price;
                            }
                            CryptoAsset::ETH => {
                                // Initialize interval start price if not set
                                if state.eth_interval_start_price == 0.0 {
                                    state.eth_interval_start_price = price;
                                }
                                state.eth_price = price;
                            }
                        }
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
            
            // Check for BTC markets: 5m, 15m, 4h up/down, and price target markets
            let is_btc_updown = slug.starts_with("btc-updown-5m-") 
                || slug.starts_with("btc-updown-15m-") 
                || slug.starts_with("btc-updown-4h-")
                || slug.contains("bitcoin-up-or-down");
            let is_btc_price_target = slug.starts_with("bitcoin-above-") 
                || slug.starts_with("bitcoin-below-")
                || slug.contains("bitcoin-hit")
                || slug.contains("btc-hit");
            
            // Check for ETH markets: 5m, 15m, 4h up/down
            let is_eth_updown = slug.starts_with("eth-updown-5m-") 
                || slug.starts_with("eth-updown-15m-") 
                || slug.starts_with("eth-updown-4h-")
                || slug.contains("ethereum-up-or-down");
            let is_eth_price_target = slug.starts_with("ethereum-above-") 
                || slug.starts_with("ethereum-below-")
                || slug.contains("ethereum-hit")
                || slug.contains("eth-hit");
            
            // Determine which asset this market is for
            let asset = if is_btc_updown || is_btc_price_target {
                Some(CryptoAsset::BTC)
            } else if is_eth_updown || is_eth_price_target {
                Some(CryptoAsset::ETH)
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
                
                // Determine market type for logging
                let market_type = if slug.contains("-5m-") {
                    "5m"
                } else if slug.contains("-15m-") {
                    "15m"
                } else if slug.contains("-4h-") {
                    "4h"
                } else if is_btc_price_target || is_eth_price_target {
                    "price-target"
                } else {
                    "daily"
                };
                let asset_name = match asset {
                    CryptoAsset::BTC => "BTC",
                    CryptoAsset::ETH => "ETH",
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
                        
                        // Determine interval based on market type (works for both BTC and ETH)
                        let interval_minutes = if slug.contains("-5m-") {
                            5
                        } else if slug.contains("-15m-") {
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
        let asset_name = match self.asset {
            CryptoAsset::BTC => "BTC",
            CryptoAsset::ETH => "ETH",
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
}
