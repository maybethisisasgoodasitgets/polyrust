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
///   USE_MOMENTUM - Set to "false" to disable momentum filter (default: true)
///   USE_EDGE_CHECK - Set to "false" to disable edge check (default: true)

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
    use_momentum: bool,
    use_edge_check: bool,
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
        
        // Default to $2 for testing - small trades to prove the process works
        let max_position_usd = env::var("MAX_POSITION_USD")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(2.0);
        
        let min_position_usd = env::var("MIN_POSITION_USD")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(1.0);
        
        // USE_MOMENTUM defaults to true; set to "false" or "0" to disable
        let use_momentum = env::var("USE_MOMENTUM")
            .map(|v| !v.eq_ignore_ascii_case("false") && v != "0")
            .unwrap_or(true);
        
        // USE_EDGE_CHECK defaults to true; set to "false" or "0" to disable
        let use_edge_check = env::var("USE_EDGE_CHECK")
            .map(|v| !v.eq_ignore_ascii_case("false") && v != "0")
            .unwrap_or(true);
        
        Ok(Self {
            private_key,
            funder_address,
            mock_trading,
            max_position_usd,
            min_position_usd,
            use_momentum,
            use_edge_check,
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
    entry_crypto_price: f64,
    market_description: String,
    interval_minutes: u32,
    asset: CryptoAsset,
}

// Exit thresholds (HFT mode - quick exits)
const TAKE_PROFIT_PCT: f64 = 8.0;    // Sell if price up 8% from entry (was 15%)
const STOP_LOSS_PCT: f64 = -6.0;     // Sell if price down 6% from entry (was -10%)
const MAX_HOLD_MULTIPLIER: f64 = 0.6; // Exit at 60% of interval time if no TP/SL hit (was 80%)

impl TradingState {
    fn new() -> Self {
        Self {
            last_trade_time: None,
            min_trade_interval: Duration::from_secs(30),  // Min 30 seconds between trades (HFT mode)
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
            entry_crypto_price: signal.crypto_price,
            market_description: market_desc.to_string(),
            interval_minutes,
            asset: signal.asset,
        });
    }
    
    /// Check if we can trade a specific asset (separate cooldowns per asset)
    fn can_trade_asset(&self, asset: CryptoAsset) -> bool {
        // Check if we have a recent trade for this specific asset
        for pos in &self.open_positions {
            if pos.asset == asset {
                return false;  // Already have an open position for this asset
            }
        }
        true
    }
    
    /// Check positions for exit conditions and return positions to close
    /// Takes prices for all 4 assets for multi-asset support
    fn check_exits_multi(&mut self, btc_price: f64, eth_price: f64, sol_price: f64, xrp_price: f64) -> Vec<(OpenPosition, &'static str, f64)> {
        let mut exits = Vec::new();
        let mut remaining = Vec::new();
        
        for pos in self.open_positions.drain(..) {
            let hold_time = pos.entry_time.elapsed();
            let max_hold_time = Duration::from_secs((pos.interval_minutes as u64) * 60 * 8 / 10); // 80% of interval
            
            // Get the correct price for this position's asset
            let current_crypto_price = match pos.asset {
                CryptoAsset::BTC => btc_price,
                CryptoAsset::ETH => eth_price,
                CryptoAsset::SOL => sol_price,
                CryptoAsset::XRP => xrp_price,
            };
            
            // Calculate current P&L based on crypto price movement since entry
            let crypto_change_pct = ((current_crypto_price - pos.entry_crypto_price) / pos.entry_crypto_price) * 100.0;
            
            // If we bet UP and crypto went up, we're winning (and vice versa)
            // Use a more realistic multiplier based on how binary options work
            // At 50¢, a correct prediction roughly doubles your money
            let effective_pnl_pct = if pos.direction_up {
                crypto_change_pct * 2.0  // More conservative multiplier
            } else {
                -crypto_change_pct * 2.0
            };
            
            // Require minimum hold time of 20 seconds before checking exits (HFT mode)
            if hold_time < Duration::from_secs(20) {
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
    
    // Set momentum filter based on config
    engine.use_momentum = cfg.use_momentum;
    if !cfg.use_momentum {
        println!("⚠️  Momentum filter DISABLED (USE_MOMENTUM=false)");
    }
    
    // Set edge check based on config
    engine.use_edge_check = cfg.use_edge_check;
    if !cfg.use_edge_check {
        println!("⚠️  Edge check DISABLED (USE_EDGE_CHECK=false)");
    }
    
    // Start Binance price feeds for BTC, ETH, SOL, and XRP
    println!("📡 Starting Binance BTC + ETH + SOL + XRP price feeds...");
    let price_state = engine.price_state();
    let _binance_handle = spawn_binance_feed(price_state.clone());
    
    // Wait for first prices from all feeds
    println!("⏳ Waiting for initial prices...");
    loop {
        let state = price_state.read().await;
        if state.btc_price > 0.0 && state.eth_price > 0.0 && state.sol_price > 0.0 && state.xrp_price > 0.0 {
            println!("✅ Got initial BTC price: ${:.2}", state.btc_price);
            println!("✅ Got initial ETH price: ${:.2}", state.eth_price);
            println!("✅ Got initial SOL price: ${:.2}", state.sol_price);
            println!("✅ Got initial XRP price: ${:.4}", state.xrp_price);
            break;
        }
        drop(state);
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    
    // Initialize interval start prices to current prices
    {
        let mut state = price_state.write().await;
        state.btc_interval_start_price = state.btc_price;
        state.eth_interval_start_price = state.eth_price;
        state.sol_interval_start_price = state.sol_price;
        state.xrp_interval_start_price = state.xrp_price;
        println!("📍 Interval start prices initialized");
    }
    
    // Find live crypto markets - MULTI-MARKET MODE
    println!("🔍 Searching for live crypto markets on Polymarket...");
    let markets = fetch_live_crypto_markets().await?;
    
    if markets.is_empty() {
        println!("⚠️ No active live crypto markets found!");
        println!("   The bot will continue monitoring for new markets...");
    } else {
        println!("✅ Found {} potential crypto markets", markets.len());
        for (i, m) in markets.iter().enumerate() {
            let asset_str = match m.asset { 
                CryptoAsset::BTC => "BTC", 
                CryptoAsset::ETH => "ETH",
                CryptoAsset::SOL => "SOL",
                CryptoAsset::XRP => "XRP",
            };
            println!("   {}. [{}] {}", i + 1, asset_str, m.description);
        }
    }
    
    // Find best market for EACH asset (BTC, ETH, SOL, XRP separately)
    // PRIORITY: 15-minute markets > 4-hour markets > daily markets
    let mut best_btc_market: Option<LiveCryptoMarket> = None;
    let mut best_btc_score = f64::MAX;  // Lower is better
    let mut best_eth_market: Option<LiveCryptoMarket> = None;
    let mut best_eth_score = f64::MAX;
    let mut best_sol_market: Option<LiveCryptoMarket> = None;
    let mut best_sol_score = f64::MAX;
    let mut best_xrp_market: Option<LiveCryptoMarket> = None;
    let mut best_xrp_score = f64::MAX;
    
    for mut market in markets {
        // Try to update market prices from CLOB orderbook
        // If it fails (fresh markets don't have orderbooks yet), use the fallback prices from Gamma API
        if let Err(_e) = update_market_prices(&mut market).await {
            // Fresh markets use the initial 50¢ prices from Gamma API - that's fine
        }
        
        let yes_price = market.yes_ask;
        let distance_from_50 = (yes_price - 0.50).abs();
        
        // Priority scoring: prefer 4h markets (longer trading window), then 15m, then daily
        // 4h = 0, 15m = 1, daily = 2, then add distance from 50%
        let interval_priority = match market.interval_minutes {
            240 => 0.0,  // 4 hours - BEST (long trading window)
            15 => 1.0,   // 15m - SECOND (very short trading window)
            _ => 2.0,    // daily or other
        };
        let score = interval_priority + distance_from_50;
        
        let asset_str = match market.asset { 
            CryptoAsset::BTC => "BTC", 
            CryptoAsset::ETH => "ETH",
            CryptoAsset::SOL => "SOL",
            CryptoAsset::XRP => "XRP",
        };
        let interval_str = match market.interval_minutes {
            15 => "15m",
            240 => "4h",
            _ => "daily",
        };
        println!("   [{}][{}] {} - Yes: {:.2}¢, No: {:.2}¢ (score: {:.2})", 
            asset_str, interval_str, market.description, market.yes_ask * 100.0, market.no_ask * 100.0, score);
        
        // Consider markets with tradeable price (YES between 3¢ and 97¢)
        if yes_price >= 0.03 && yes_price <= 0.97 {
            match market.asset {
                CryptoAsset::BTC => {
                    if score < best_btc_score {
                        best_btc_score = score;
                        best_btc_market = Some(market);
                    }
                }
                CryptoAsset::ETH => {
                    if score < best_eth_score {
                        best_eth_score = score;
                        best_eth_market = Some(market);
                    }
                }
                CryptoAsset::SOL => {
                    if score < best_sol_score {
                        best_sol_score = score;
                        best_sol_market = Some(market);
                    }
                }
                CryptoAsset::XRP => {
                    if score < best_xrp_score {
                        best_xrp_score = score;
                        best_xrp_market = Some(market);
                    }
                }
            }
        }
    }
    
    // Set up multi-market tracking
    println!();
    println!("📊 MULTI-MARKET MODE (4 assets):");
    if let Some(ref market) = best_btc_market {
        println!("   🟠 BTC: {} - Yes: {:.2}¢", market.description, market.yes_ask * 100.0);
        engine.set_market_for_asset(market.clone());
    } else {
        println!("   🟠 BTC: No active market found");
    }
    if let Some(ref market) = best_eth_market {
        println!("   🔵 ETH: {} - Yes: {:.2}¢", market.description, market.yes_ask * 100.0);
        engine.set_market_for_asset(market.clone());
    } else {
        println!("   🔵 ETH: No active market found");
    }
    if let Some(ref market) = best_sol_market {
        println!("   🟣 SOL: {} - Yes: {:.2}¢", market.description, market.yes_ask * 100.0);
        engine.set_market_for_asset(market.clone());
    } else {
        println!("   🟣 SOL: No active market found");
    }
    if let Some(ref market) = best_xrp_market {
        println!("   ⚪ XRP: {} - Yes: {:.2}¢", market.description, market.yes_ask * 100.0);
        engine.set_market_for_asset(market.clone());
    } else {
        println!("   ⚪ XRP: No active market found");
    }
    
    // Keep legacy single-market for backward compatibility
    let _active_market = best_btc_market.clone().or(best_eth_market.clone()).or(best_sol_market.clone()).or(best_xrp_market.clone());
    
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
    let mut market_price_log_interval = interval(Duration::from_secs(30));  // Log market prices every 30s
    
    loop {
        tokio::select! {
            _ = check_interval.tick() => {
                // MULTI-MARKET: Check for arbitrage opportunities on ALL active markets
                let signals = engine.check_all_opportunities().await;
                
                for signal in signals {
                    // Check if we can trade this specific asset (no open position for it)
                    if !state.can_trade_asset(signal.asset) {
                        let asset_name = match signal.asset {
                            CryptoAsset::BTC => "BTC",
                            CryptoAsset::ETH => "ETH",
                            CryptoAsset::SOL => "SOL",
                            CryptoAsset::XRP => "XRP",
                        };
                        println!("   ⏸️ {} signal skipped: already have open position", asset_name);
                        continue;  // Already have an open position for this asset
                    }
                    
                    println!("🎰 SIGNAL: {}", signal);
                    
                    let asset_name = match signal.asset {
                        CryptoAsset::BTC => "BTC",
                        CryptoAsset::ETH => "ETH",
                        CryptoAsset::SOL => "SOL",
                        CryptoAsset::XRP => "XRP",
                    };
                    
                    // Get market info for this asset
                    let market_info = engine.get_market(signal.asset);
                    let market_desc = market_info.map(|m| m.description.as_str()).unwrap_or("Unknown");
                    let interval_mins = market_info.map(|m| m.interval_minutes).unwrap_or(15);
                    let market_type = match interval_mins {
                        5 => "5m",
                        15 => "15m", 
                        60 => "1h",
                        240 => "4h",
                        _ => "daily"
                    };
                    
                    if cfg.mock_trading {
                        let timestamp = chrono::Utc::now().format("%Y-%m-%d %H:%M:%S UTC");
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
                        state.record_trade(&signal, market_desc, interval_mins);
                    } else if let (Some(client), Some(creds)) = (&client, &creds) {
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
                
                // Check for exit conditions on open positions (multi-asset)
                let (btc_price, eth_price, sol_price, xrp_price) = {
                    let ps = price_state.read().await;
                    (ps.btc_price, ps.eth_price, ps.sol_price, ps.xrp_price)
                };
                
                let exits = state.check_exits_multi(btc_price, eth_price, sol_price, xrp_price);
                for (pos, reason, pnl_pct) in exits {
                    let timestamp = chrono::Utc::now().format("%Y-%m-%d %H:%M:%S UTC");
                    let hold_duration = pos.entry_time.elapsed();
                    let pnl_usd = pos.size_usd * (pnl_pct / 100.0);
                    
                    let asset_name = match pos.asset { 
                        CryptoAsset::BTC => "BTC", 
                        CryptoAsset::ETH => "ETH",
                        CryptoAsset::SOL => "SOL",
                        CryptoAsset::XRP => "XRP",
                    };
                    let current_price = match pos.asset { 
                        CryptoAsset::BTC => btc_price, 
                        CryptoAsset::ETH => eth_price,
                        CryptoAsset::SOL => sol_price,
                        CryptoAsset::XRP => xrp_price,
                    };
                    
                    println!("💰 [EXIT] {} - {}", reason, timestamp);
                    println!("      Market: {}", pos.market_description);
                    println!("      Asset: {}", asset_name);
                    println!("      Direction: {}", if pos.direction_up { "YES (UP)" } else { "NO (DOWN)" });
                    println!("      Entry: ${:.2} @ {:.2}¢ | {} was ${:.2}", 
                        pos.size_usd, pos.entry_price * 100.0, asset_name, pos.entry_crypto_price);
                    println!("      Exit: {} now ${:.2} | Hold time: {:.1}s", 
                        asset_name, current_price, hold_duration.as_secs_f64());
                    println!("      P&L: {:+.1}% (${:+.2})", pnl_pct, pnl_usd);
                    println!("      ---");
                    
                    state.estimated_pnl += pnl_usd;
                }
            }
            
            _ = price_log_interval.tick() => {
                // Log current state with VELOCITY (5-second price change)
                let ps = price_state.read().await;
                let btc_vel = ps.velocity_pct(CryptoAsset::BTC, 5);
                let eth_vel = ps.velocity_pct(CryptoAsset::ETH, 5);
                let sol_vel = ps.velocity_pct(CryptoAsset::SOL, 5);
                let xrp_vel = ps.velocity_pct(CryptoAsset::XRP, 5);
                let btc_dir = if btc_vel >= 0.0 { "⬆" } else { "⬇" };
                let eth_dir = if eth_vel >= 0.0 { "⬆" } else { "⬇" };
                let sol_dir = if sol_vel >= 0.0 { "⬆" } else { "⬇" };
                let xrp_dir = if xrp_vel >= 0.0 { "⬆" } else { "⬇" };
                let open_pos = state.open_positions.len();
                let pnl_str = if state.estimated_pnl != 0.0 {
                    format!(" | P&L: ${:+.2}", state.estimated_pnl)
                } else {
                    String::new()
                };
                // Show velocity (v) instead of interval change
                println!(
                    "📈 BTC ${:.0} v{}{:+.3}% | ETH ${:.0} v{}{:+.3}% | SOL ${:.1} v{}{:+.3}% | XRP ${:.3} v{}{:+.3}% | T:{} O:{}{} | {}",
                    ps.btc_price, btc_dir, btc_vel,
                    ps.eth_price, eth_dir, eth_vel,
                    ps.sol_price, sol_dir, sol_vel,
                    ps.xrp_price, xrp_dir, xrp_vel,
                    state.trades_executed,
                    open_pos,
                    pnl_str,
                    if cfg.mock_trading { "MOCK" } else { "LIVE" }
                );
            }
            
            _ = market_price_log_interval.tick() => {
                // Log current market prices to show user why we're not trading
                println!("📊 CURRENT MARKET PRICES:");
                if let Some(m) = engine.get_market(CryptoAsset::BTC) {
                    println!("   🟠 BTC: YES={:.1}¢ NO={:.1}¢ | {}", 
                        m.yes_ask * 100.0, m.no_ask * 100.0, m.description);
                }
                if let Some(m) = engine.get_market(CryptoAsset::ETH) {
                    println!("   🔵 ETH: YES={:.1}¢ NO={:.1}¢ | {}", 
                        m.yes_ask * 100.0, m.no_ask * 100.0, m.description);
                }
                if let Some(m) = engine.get_market(CryptoAsset::SOL) {
                    println!("   🟣 SOL: YES={:.1}¢ NO={:.1}¢ | {}", 
                        m.yes_ask * 100.0, m.no_ask * 100.0, m.description);
                }
                if let Some(m) = engine.get_market(CryptoAsset::XRP) {
                    println!("   ⚪ XRP: YES={:.1}¢ NO={:.1}¢ | {}", 
                        m.yes_ask * 100.0, m.no_ask * 100.0, m.description);
                }
            }
            
            _ = market_refresh_interval.tick() => {
                // MULTI-MARKET: Refresh markets for all 4 assets
                if let Ok(markets) = fetch_live_crypto_markets().await {
                    let mut best_btc: Option<LiveCryptoMarket> = None;
                    let mut best_btc_dist = f64::MAX;
                    let mut best_eth: Option<LiveCryptoMarket> = None;
                    let mut best_eth_dist = f64::MAX;
                    let mut best_sol: Option<LiveCryptoMarket> = None;
                    let mut best_sol_dist = f64::MAX;
                    let mut best_xrp: Option<LiveCryptoMarket> = None;
                    let mut best_xrp_dist = f64::MAX;
                    
                    for mut m in markets {
                        if update_market_prices(&mut m).await.is_ok() {
                            let yes_price = m.yes_ask;
                            let distance = (yes_price - 0.50).abs();
                            
                            if yes_price >= 0.03 && yes_price <= 0.97 {
                                match m.asset {
                                    CryptoAsset::BTC => {
                                        if distance < best_btc_dist {
                                            best_btc_dist = distance;
                                            best_btc = Some(m);
                                        }
                                    }
                                    CryptoAsset::ETH => {
                                        if distance < best_eth_dist {
                                            best_eth_dist = distance;
                                            best_eth = Some(m);
                                        }
                                    }
                                    CryptoAsset::SOL => {
                                        if distance < best_sol_dist {
                                            best_sol_dist = distance;
                                            best_sol = Some(m);
                                        }
                                    }
                                    CryptoAsset::XRP => {
                                        if distance < best_xrp_dist {
                                            best_xrp_dist = distance;
                                            best_xrp = Some(m);
                                        }
                                    }
                                }
                            }
                        }
                    }
                    
                    // Update BTC market
                    if let Some(m) = best_btc {
                        if !engine.has_market(CryptoAsset::BTC) {
                            println!("📊 [BTC] Found: {} - Yes: {:.2}¢", m.description, m.yes_ask * 100.0);
                            engine.reset_interval_for_asset(CryptoAsset::BTC).await;
                        }
                        engine.set_market_for_asset(m);
                    } else {
                        if engine.has_market(CryptoAsset::BTC) {
                            println!("⚠️ [BTC] No active market available");
                        }
                        engine.clear_market_for_asset(CryptoAsset::BTC);
                    }
                    
                    // Update ETH market
                    if let Some(m) = best_eth {
                        if !engine.has_market(CryptoAsset::ETH) {
                            println!("📊 [ETH] Found: {} - Yes: {:.2}¢", m.description, m.yes_ask * 100.0);
                            engine.reset_interval_for_asset(CryptoAsset::ETH).await;
                        }
                        engine.set_market_for_asset(m);
                    } else {
                        if engine.has_market(CryptoAsset::ETH) {
                            println!("⚠️ [ETH] No active market available");
                        }
                        engine.clear_market_for_asset(CryptoAsset::ETH);
                    }
                    
                    // Update SOL market
                    if let Some(m) = best_sol {
                        if !engine.has_market(CryptoAsset::SOL) {
                            println!("📊 [SOL] Found: {} - Yes: {:.2}¢", m.description, m.yes_ask * 100.0);
                            engine.reset_interval_for_asset(CryptoAsset::SOL).await;
                        }
                        engine.set_market_for_asset(m);
                    } else {
                        if engine.has_market(CryptoAsset::SOL) {
                            println!("⚠️ [SOL] No active market available");
                        }
                        engine.clear_market_for_asset(CryptoAsset::SOL);
                    }
                    
                    // Update XRP market
                    if let Some(m) = best_xrp {
                        if !engine.has_market(CryptoAsset::XRP) {
                            println!("📊 [XRP] Found: {} - Yes: {:.2}¢", m.description, m.yes_ask * 100.0);
                            engine.reset_interval_for_asset(CryptoAsset::XRP).await;
                        }
                        engine.set_market_for_asset(m);
                    } else {
                        if engine.has_market(CryptoAsset::XRP) {
                            println!("⚠️ [XRP] No active market available");
                        }
                        engine.clear_market_for_asset(CryptoAsset::XRP);
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
    // Round price to 2 decimals (required by Polymarket API)
    let price = (signal.buy_price * 100.0).round() / 100.0;
    
    // Calculate shares to buy, round to 2 decimals
    let shares = signal.recommended_size_usd / price;
    let size = (shares * 100.0).floor() / 100.0;
    
    // Ensure minimum size
    let size = if size < 1.0 { 1.0 } else { size };
    
    // Build order
    let order = OrderArgs {
        token_id: signal.token_id.clone(),
        price,
        size,
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
