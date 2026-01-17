/// Crypto Latency Arbitrage Bot
/// 
/// Monitors BTC price on Binance and bets on Polymarket's live crypto markets
/// when price movements create arbitrage opportunities.
/// 
/// Usage:
///   cargo run --release --bin crypto_arb_bot
/// 
/// Environment variables:
///   PRIVATE_KEY - Your wallet private key
///   FUNDER_ADDRESS - Your wallet address
///   MOCK_TRADING - Set to "true" for paper trading (default: true)
///   MAX_POSITION_USD - Maximum position size per trade (default: 10.0)
///   MIN_POSITION_USD - Minimum position size per trade (default: 1.0)

use anyhow::{Result, anyhow};
use chrono;
use dotenvy::dotenv;
use pm_whale_follower::crypto_arb::{
    CryptoArbEngine, spawn_binance_feed, fetch_live_crypto_markets, 
    update_market_prices, ArbSignal, LiveCryptoMarket, CryptoAsset,
    MIN_PRICE_MOVE_PCT, MAX_BUY_PRICE, MIN_EDGE_PCT,
};
use pm_whale_follower::{OrderArgs, RustClobClient, PreparedCreds};
use std::env;
use std::time::{Duration, Instant};
use tokio::time::interval;

// ============================================================================
// Configuration
// ============================================================================

struct Config {
    private_key: String,
    funder_address: String,
    mock_trading: bool,
    max_position_usd: f64,
    min_position_usd: f64,
}

impl Config {
    fn from_env() -> Result<Self> {
        let private_key = env::var("PRIVATE_KEY")
            .map_err(|_| anyhow!("PRIVATE_KEY env var required"))?;
        
        let funder_address = env::var("FUNDER_ADDRESS")
            .map_err(|_| anyhow!("FUNDER_ADDRESS env var required"))?;
        
        let mock_trading = env::var("MOCK_TRADING")
            .map(|v| v.eq_ignore_ascii_case("true") || v == "1")
            .unwrap_or(true);  // Default to mock mode for safety
        
        let max_position_usd = env::var("MAX_POSITION_USD")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(10.0);
        
        let min_position_usd = env::var("MIN_POSITION_USD")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(1.0);
        
        Ok(Self {
            private_key,
            funder_address,
            mock_trading,
            max_position_usd,
            min_position_usd,
        })
    }
}

// ============================================================================
// Trading State
// ============================================================================

struct TradingState {
    /// Last time we placed a trade (to avoid rapid-fire)
    last_trade_time: Option<Instant>,
    /// Minimum time between trades
    min_trade_interval: Duration,
    /// Total trades executed this session
    trades_executed: u32,
    /// Total profit/loss (estimated)
    estimated_pnl: f64,
    /// Current open positions
    open_positions: Vec<OpenPosition>,
}

struct OpenPosition {
    token_id: String,
    size_usd: f64,
    entry_price: f64,
    direction_up: bool,
    entry_time: Instant,
    entry_btc_price: f64,
    market_description: String,
    interval_minutes: u32,
}

// Exit thresholds
const TAKE_PROFIT_PCT: f64 = 15.0;   // Sell if price up 15% from entry
const STOP_LOSS_PCT: f64 = -10.0;    // Sell if price down 10% from entry
const MAX_HOLD_MULTIPLIER: f64 = 0.8; // Exit at 80% of interval time if no TP/SL hit

impl TradingState {
    fn new() -> Self {
        Self {
            last_trade_time: None,
            min_trade_interval: Duration::from_secs(120),  // Min 2 minutes between trades
            trades_executed: 0,
            estimated_pnl: 0.0,
            open_positions: Vec::new(),
        }
    }
    
    fn can_trade(&self) -> bool {
        match self.last_trade_time {
            Some(t) => t.elapsed() >= self.min_trade_interval,
            None => true,
        }
    }
    
    fn record_trade(&mut self, signal: &ArbSignal, market_desc: &str, interval_minutes: u32) {
        self.last_trade_time = Some(Instant::now());
        self.trades_executed += 1;
        self.open_positions.push(OpenPosition {
            token_id: signal.token_id.clone(),
            size_usd: signal.recommended_size_usd,
            entry_price: signal.buy_price,
            direction_up: signal.bet_up,
            entry_time: Instant::now(),
            entry_btc_price: signal.crypto_price,
            market_description: market_desc.to_string(),
            interval_minutes,
        });
    }
    
    /// Check positions for exit conditions and return positions to close
    fn check_exits(&mut self, current_crypto_price: f64) -> Vec<(OpenPosition, &'static str, f64)> {
        let mut exits = Vec::new();
        let mut remaining = Vec::new();
        
        for pos in self.open_positions.drain(..) {
            let hold_time = pos.entry_time.elapsed();
            let max_hold_time = Duration::from_secs((pos.interval_minutes as u64) * 60 * 8 / 10); // 80% of interval
            
            // Calculate current P&L based on crypto price movement since entry
            let crypto_change_pct = ((current_crypto_price - pos.entry_btc_price) / pos.entry_btc_price) * 100.0;
            
            // If we bet UP and crypto went up, we're winning (and vice versa)
            // Use a more realistic multiplier based on how binary options work
            // At 50¢, a correct prediction roughly doubles your money
            let effective_pnl_pct = if pos.direction_up {
                crypto_change_pct * 2.0  // More conservative multiplier
            } else {
                -crypto_change_pct * 2.0
            };
            
            // Require minimum hold time of 60 seconds before checking exits
            if hold_time < Duration::from_secs(60) {
                remaining.push(pos);
                continue;
            }
            
            // Check exit conditions
            if effective_pnl_pct >= TAKE_PROFIT_PCT {
                exits.push((pos, "TAKE PROFIT ✅", effective_pnl_pct));
            } else if effective_pnl_pct <= STOP_LOSS_PCT {
                exits.push((pos, "STOP LOSS ❌", effective_pnl_pct));
            } else if hold_time >= max_hold_time {
                exits.push((pos, "TIME EXIT ⏰", effective_pnl_pct));
            } else {
                remaining.push(pos);
            }
        }
        
        self.open_positions = remaining;
        exits
    }
}

// ============================================================================
// Main
// ============================================================================

#[tokio::main]
async fn main() -> Result<()> {
    dotenv().ok();
    
    let cfg = Config::from_env()?;
    
    println!("╔════════════════════════════════════════════════════════════╗");
    println!("║        🚀 CRYPTO LATENCY ARBITRAGE BOT                     ║");
    println!("╠════════════════════════════════════════════════════════════╣");
    println!("║  Mode: {}                                        ║", 
        if cfg.mock_trading { "MOCK (paper trading)" } else { "LIVE ⚠️ REAL MONEY" });
    println!("║  Max Position: ${:<6.2}                                    ║", cfg.max_position_usd);
    println!("║  Min Position: ${:<6.2}                                    ║", cfg.min_position_usd);
    println!("╠════════════════════════════════════════════════════════════╣");
    println!("║  Strategy Parameters:                                      ║");
    println!("║  • Min Price Move: {:.2}%                                   ║", MIN_PRICE_MOVE_PCT);
    println!("║  • Max Buy Price: {:.0}¢                                    ║", MAX_BUY_PRICE * 100.0);
    println!("║  • Min Edge: {:.1}%                                         ║", MIN_EDGE_PCT);
    println!("╚════════════════════════════════════════════════════════════╝");
    println!();
    
    // Initialize trading client (only needed for live trading)
    let (client, creds) = if !cfg.mock_trading {
        let c = RustClobClient::new(
            "https://clob.polymarket.com",
            137,
            &cfg.private_key,
            &cfg.funder_address,
        )?;
        
        // Get or create API credentials
        let creds_path = ".clob_creds_arb.json";
        let api_creds = if std::path::Path::new(creds_path).exists() {
            let data = std::fs::read_to_string(creds_path)?;
            serde_json::from_str(&data)?
        } else {
            println!("🔑 Creating API credentials...");
            let new_creds = c.derive_api_key(0)?;
            std::fs::write(creds_path, serde_json::to_string_pretty(&new_creds)?)?;
            new_creds
        };
        
        let prepared = PreparedCreds::from_api_creds(&api_creds)?;
        (Some(c), Some(prepared))
    } else {
        (None, None)
    };
    
    // Create arbitrage engine
    let mut engine = CryptoArbEngine::new(
        cfg.mock_trading,
        cfg.max_position_usd,
        cfg.min_position_usd,
    );
    
    // Start Binance price feeds for both BTC and ETH
    println!("📡 Starting Binance BTC + ETH price feeds...");
    let price_state = engine.price_state();
    let _binance_handle = spawn_binance_feed(price_state.clone());
    
    // Wait for first prices from both feeds
    println!("⏳ Waiting for initial prices...");
    loop {
        let state = price_state.read().await;
        if state.btc_price > 0.0 && state.eth_price > 0.0 {
            println!("✅ Got initial BTC price: ${:.2}", state.btc_price);
            println!("✅ Got initial ETH price: ${:.2}", state.eth_price);
            break;
        } else if state.btc_price > 0.0 {
            // BTC ready, still waiting for ETH
        } else if state.eth_price > 0.0 {
            // ETH ready, still waiting for BTC
        }
        drop(state);
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    
    // Find live crypto markets
    println!("🔍 Searching for live crypto markets on Polymarket...");
    let markets = fetch_live_crypto_markets().await?;
    
    if markets.is_empty() {
        println!("⚠️ No active live crypto markets found!");
        println!("   The bot will continue monitoring for new markets...");
    } else {
        println!("✅ Found {} potential crypto markets", markets.len());
        for (i, m) in markets.iter().enumerate() {
            println!("   {}. {}", i + 1, m.description);
        }
    }
    
    // Find the market with the best price - closer to 50¢ is better for arbitrage
    let mut active_market: Option<LiveCryptoMarket> = None;
    let mut best_price_distance = f64::MAX;
    
    for mut market in markets {
        // Update market prices from CLOB
        if let Err(e) = update_market_prices(&mut market).await {
            println!("⚠️ Failed to get prices for {}: {}", market.description, e);
            continue;
        }
        
        // For arbitrage, we want YES price close to 50¢ (undecided market)
        // Skip markets where outcome is already heavily decided (YES > 80¢ or YES < 20¢)
        let yes_price = market.yes_ask;
        let distance_from_50 = (yes_price - 0.50).abs();
        
        println!("   {} - Yes: {:.2}¢, No: {:.2}¢ (distance from 50¢: {:.2})", 
            market.description, market.yes_ask * 100.0, market.no_ask * 100.0, distance_from_50);
        
        // Only consider markets with YES between 20¢ and 80¢ (undecided)
        if yes_price >= 0.20 && yes_price <= 0.80 && distance_from_50 < best_price_distance {
            best_price_distance = distance_from_50;
            active_market = Some(market);
        }
    }
    
    if let Some(ref market) = active_market {
        println!("📊 Selected market: {} - Yes: {:.2}¢, No: {:.2}¢", 
            market.description, market.yes_ask * 100.0, market.no_ask * 100.0);
        engine.set_market(market.clone());
    } else {
        println!("⚠️ No markets with prices ≤92¢ found - all markets already heavily traded");
    }
    
    // Trading state
    let mut state = TradingState::new();
    
    // Main loop
    println!();
    println!("🎯 Monitoring for arbitrage opportunities...");
    println!("   Press Ctrl+C to stop");
    println!();
    
    let mut check_interval = interval(Duration::from_millis(100));
    let mut price_log_interval = interval(Duration::from_secs(10));
    let mut market_refresh_interval = interval(Duration::from_secs(3));  // Check for new markets every 3 seconds
    
    loop {
        tokio::select! {
            _ = check_interval.tick() => {
                // Check for arbitrage opportunity
                if let Some(signal) = engine.check_opportunity().await {
                    if state.can_trade() {
                        println!("🎰 SIGNAL: {}", signal);
                        
                        if cfg.mock_trading {
                            // Log detailed mock trade for analysis
                            let timestamp = chrono::Utc::now().format("%Y-%m-%d %H:%M:%S UTC");
                            let market_desc = active_market.as_ref()
                                .map(|m| m.description.as_str())
                                .unwrap_or("Unknown");
                            let market_type = active_market.as_ref()
                                .map(|m| match m.interval_minutes {
                                    5 => "5m",
                                    15 => "15m", 
                                    60 => "1h",
                                    240 => "4h",
                                    _ => "daily"
                                })
                                .unwrap_or("?");
                            
                            let asset_name = match signal.asset {
                                CryptoAsset::BTC => "BTC",
                                CryptoAsset::ETH => "ETH",
                            };
                            println!("   📝 [MOCK TRADE] {}", timestamp);
                            println!("      Market: {} ({})", market_desc, market_type);
                            println!("      Asset: {}", asset_name);
                            println!("      Direction: {}", if signal.bet_up { "BUY YES (UP)" } else { "BUY NO (DOWN)" });
                            println!("      {} Price: ${:.2} ({:+.3}% move)", asset_name, signal.crypto_price, signal.price_change_pct);
                            println!("      Entry Price: {:.2}¢ | Edge: {:.1}% | Confidence: {}%", 
                                signal.buy_price * 100.0, signal.edge_pct, signal.confidence);
                            println!("      Position Size: ${:.2}", signal.recommended_size_usd);
                            println!("      Exit Strategy: TP +{}% | SL {}% | Time {}% of interval", 
                                TAKE_PROFIT_PCT, STOP_LOSS_PCT, (MAX_HOLD_MULTIPLIER * 100.0) as i32);
                            println!("      ---");
                            let interval_mins = active_market.as_ref().map(|m| m.interval_minutes).unwrap_or(15);
                            state.record_trade(&signal, market_desc, interval_mins);
                        } else if let (Some(client), Some(creds)) = (&client, &creds) {
                            // Execute real trade
                            let market_desc = active_market.as_ref()
                                .map(|m| m.description.as_str())
                                .unwrap_or("Unknown");
                            let interval_mins = active_market.as_ref().map(|m| m.interval_minutes).unwrap_or(15);
                            match execute_trade(client, creds, &signal).await {
                                Ok(result) => {
                                    println!("   ✅ Trade executed: {}", result);
                                    state.record_trade(&signal, market_desc, interval_mins);
                                }
                                Err(e) => {
                                    println!("   ❌ Trade failed: {}", e);
                                }
                            }
                        }
                    }
                }
                
                // Check for exit conditions on open positions
                // Use the asset-specific price for the active market
                let current_crypto = {
                    let ps = price_state.read().await;
                    match active_market.as_ref().map(|m| m.asset) {
                        Some(CryptoAsset::ETH) => ps.eth_price,
                        _ => ps.btc_price,
                    }
                };
                
                let exits = state.check_exits(current_crypto);
                for (pos, reason, pnl_pct) in exits {
                    let timestamp = chrono::Utc::now().format("%Y-%m-%d %H:%M:%S UTC");
                    let hold_duration = pos.entry_time.elapsed();
                    let pnl_usd = pos.size_usd * (pnl_pct / 100.0);
                    
                    println!("💰 [EXIT] {} - {}", reason, timestamp);
                    println!("      Market: {}", pos.market_description);
                    println!("      Direction: {}", if pos.direction_up { "YES (UP)" } else { "NO (DOWN)" });
                    println!("      Entry: ${:.2} @ {:.2}¢ | Crypto was ${:.2}", 
                        pos.size_usd, pos.entry_price * 100.0, pos.entry_btc_price);
                    println!("      Exit: Crypto now ${:.2} | Hold time: {:.1}s", 
                        current_crypto, hold_duration.as_secs_f64());
                    println!("      P&L: {:+.1}% (${:+.2})", pnl_pct, pnl_usd);
                    println!("      ---");
                    
                    state.estimated_pnl += pnl_usd;
                }
            }
            
            _ = price_log_interval.tick() => {
                // Log current state for both BTC and ETH
                let ps = price_state.read().await;
                let btc_change = ps.btc_change_pct();
                let eth_change = ps.eth_change_pct();
                let btc_dir = if btc_change >= 0.0 { "⬆️" } else { "⬇️" };
                let eth_dir = if eth_change >= 0.0 { "⬆️" } else { "⬇️" };
                let open_pos = state.open_positions.len();
                let pnl_str = if state.estimated_pnl != 0.0 {
                    format!(" | P&L: ${:+.2}", state.estimated_pnl)
                } else {
                    String::new()
                };
                println!(
                    "📈 BTC ${:.2} {}{:+.3}% | ETH ${:.2} {}{:+.3}% | Trades: {} | Open: {}{} | {}",
                    ps.btc_price, btc_dir, btc_change,
                    ps.eth_price, eth_dir, eth_change,
                    state.trades_executed,
                    open_pos,
                    pnl_str,
                    if cfg.mock_trading { "MOCK" } else { "LIVE" }
                );
            }
            
            _ = market_refresh_interval.tick() => {
                // Always search for the best available market (markets change quickly)
                let current_distance = active_market.as_ref()
                    .map(|m| (m.yes_ask - 0.50).abs())
                    .unwrap_or(f64::MAX);
                
                // Refresh current market prices
                let mut current_valid = false;
                if let Some(ref mut market) = active_market {
                    if let Err(e) = update_market_prices(market).await {
                        println!("⚠️ Market {} no longer active: {}", market.description, e);
                    } else {
                        // Check if current market is still undecided
                        if market.yes_ask >= 0.20 && market.yes_ask <= 0.80 {
                            current_valid = true;
                            engine.set_market(market.clone());
                        } else {
                            println!("⚠️ Market {} now at {:.0}¢ (too decided)", market.description, market.yes_ask * 100.0);
                        }
                    }
                }
                
                // Search for a better market if we don't have one or current is invalid
                if !current_valid {
                    let mut best_market: Option<LiveCryptoMarket> = None;
                    let mut best_distance = current_distance;
                    
                    if let Ok(markets) = fetch_live_crypto_markets().await {
                        for mut m in markets {
                            if update_market_prices(&mut m).await.is_ok() {
                                let yes_price = m.yes_ask;
                                let distance = (yes_price - 0.50).abs();
                                // Only consider undecided markets (YES between 20-80¢)
                                if yes_price >= 0.20 && yes_price <= 0.80 && distance < best_distance {
                                    best_distance = distance;
                                    best_market = Some(m);
                                }
                            }
                        }
                    }
                    
                    if let Some(m) = best_market {
                        println!("📊 Switched to: {} - Yes: {:.2}¢", m.description, m.yes_ask * 100.0);
                        engine.set_market(m.clone());
                        engine.reset_interval().await;
                        active_market = Some(m);
                    } else if !current_valid {
                        active_market = None;
                    }
                }
            }
        }
    }
}

// ============================================================================
// Trade Execution
// ============================================================================

async fn execute_trade(
    client: &RustClobClient,
    creds: &PreparedCreds,
    signal: &ArbSignal,
) -> Result<String> {
    // Calculate shares to buy
    let shares = signal.recommended_size_usd / signal.buy_price;
    
    // Build order
    let order = OrderArgs {
        token_id: signal.token_id.clone(),
        price: signal.buy_price,
        size: (shares * 100.0).floor() / 100.0,  // Round to 2 decimals
        side: "BUY".to_string(),
        fee_rate_bps: None,
        nonce: Some(0),
        expiration: None,
        taker: None,
        order_type: Some("FOK".to_string()),  // Fill or Kill for speed
    };
    
    // Execute via blocking call (TODO: make async)
    let result = tokio::task::spawn_blocking({
        let mut client = client.clone();
        let creds = creds.clone();
        move || -> Result<String> {
            let signed = client.create_order(order)?;
            let body = signed.post_body(&creds.api_key, "FOK");
            let resp = client.post_order_fast(body, &creds)?;
            let status = resp.status();
            let text = resp.text().unwrap_or_default();
            Ok(format!("Status: {} - {}", status, text))
        }
    }).await??;
    
    Ok(result)
}
