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
use dotenvy::dotenv;
use pm_whale_follower::crypto_arb::{
    CryptoArbEngine, spawn_binance_feed, fetch_live_crypto_markets, 
    update_market_prices, ArbSignal, LiveCryptoMarket,
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
}

impl TradingState {
    fn new() -> Self {
        Self {
            last_trade_time: None,
            min_trade_interval: Duration::from_secs(5),  // Min 5 seconds between trades
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
    
    fn record_trade(&mut self, signal: &ArbSignal) {
        self.last_trade_time = Some(Instant::now());
        self.trades_executed += 1;
        self.open_positions.push(OpenPosition {
            token_id: signal.token_id.clone(),
            size_usd: signal.recommended_size_usd,
            entry_price: signal.buy_price,
            direction_up: signal.bet_up,
            entry_time: Instant::now(),
        });
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
    
    // Start Binance price feed
    println!("📡 Starting Binance BTC price feed...");
    let price_state = engine.price_state();
    let _binance_handle = spawn_binance_feed(price_state.clone());
    
    // Wait for first price
    println!("⏳ Waiting for initial BTC price...");
    loop {
        let state = price_state.read().await;
        if state.btc_price > 0.0 {
            println!("✅ Got initial BTC price: ${:.2}", state.btc_price);
            break;
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
    
    // Use first market if available
    let mut active_market: Option<LiveCryptoMarket> = markets.into_iter().next();
    
    if let Some(ref mut market) = active_market {
        // Update market prices
        if let Err(e) = update_market_prices(market).await {
            println!("⚠️ Failed to get market prices: {}", e);
        } else {
            println!("📊 Market prices - Yes: {:.2}¢, No: {:.2}¢", 
                market.yes_ask * 100.0, market.no_ask * 100.0);
        }
        engine.set_market(market.clone());
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
    let mut market_refresh_interval = interval(Duration::from_secs(60));
    
    loop {
        tokio::select! {
            _ = check_interval.tick() => {
                // Check for arbitrage opportunity
                if let Some(signal) = engine.check_opportunity().await {
                    if state.can_trade() {
                        println!("🎰 SIGNAL: {}", signal);
                        
                        if cfg.mock_trading {
                            println!("   📝 [MOCK] Would execute trade");
                            state.record_trade(&signal);
                        } else if let (Some(ref client), Some(ref creds)) = (&client, &creds) {
                            // Execute real trade
                            match execute_trade(client, creds, &signal).await {
                                Ok(result) => {
                                    println!("   ✅ Trade executed: {}", result);
                                    state.record_trade(&signal);
                                }
                                Err(e) => {
                                    println!("   ❌ Trade failed: {}", e);
                                }
                            }
                        }
                    }
                }
            }
            
            _ = price_log_interval.tick() => {
                // Log current state
                let ps = price_state.read().await;
                let change = ps.price_change_pct();
                let direction = if change >= 0.0 { "⬆️" } else { "⬇️" };
                println!(
                    "📈 BTC ${:.2} | {} {:+.3}% from interval start | Trades: {} | {}",
                    ps.btc_price,
                    direction,
                    change,
                    state.trades_executed,
                    if cfg.mock_trading { "MOCK" } else { "LIVE" }
                );
            }
            
            _ = market_refresh_interval.tick() => {
                // Refresh market prices
                if let Some(ref mut market) = active_market {
                    if let Err(e) = update_market_prices(market).await {
                        eprintln!("⚠️ Failed to refresh market prices: {}", e);
                    }
                    engine.set_market(market.clone());
                }
                
                // Reset interval for new period
                engine.reset_interval().await;
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
