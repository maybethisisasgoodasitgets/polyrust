# Crypto Arbitrage Bot - Branch Changelog

**Branch:** `crypto-arbitrage`  
**Base:** `main`  
**Total Changes:** +2,200 lines across 7 files

---

## Summary

This branch adds a complete **crypto latency arbitrage bot** that trades BTC, ETH, SOL, and XRP "Up or Down" markets on Polymarket by monitoring real-time Binance price feeds.

---

## New Files

### `src/crypto_arb.rs` (+1,400 lines)
Core arbitrage engine with:
- **Binance WebSocket feeds** - Real-time BTC/USDT, ETH/USDT, SOL/USDT, and XRP/USDT price streaming
- **Polymarket market discovery** - Auto-finds active 5m, 15m, 1h, 4h markets
- **Price state tracking** - Monitors price changes within market intervals
- **Momentum detection** - Analyzes price acceleration and consistency
- **Multi-market support** - Tracks BTC, ETH, SOL, and XRP markets simultaneously
- **Signal generation** - Calculates edge, confidence, and position sizing

### `src/bin/crypto_arb_bot.rs` (+587 lines)
Main bot executable with:
- **Trading state management** - Tracks open positions, P&L, trade history
- **Exit strategy** - Take profit (+15%), stop loss (-10%), time-based exits
- **Mock trading mode** - Paper trading for testing without real funds
- **Live trading mode** - Executes real trades via Polymarket CLOB API
- **Market refresh loop** - Finds best markets every 3 seconds
- **Multi-asset positions** - Can hold BTC, ETH, SOL, and XRP positions simultaneously

---

## Modified Files

### `Cargo.toml` (+10 lines)
- Added `crypto_arb_bot` binary target
- Added dependencies: `tokio-tungstenite`, `futures-util`

### `README.md` (+36 lines)
- Added crypto arbitrage bot documentation
- Momentum detection explanation
- Multi-market trading docs
- Environment variables and usage instructions

### `src/lib.rs` (+1 line)
- Exported `crypto_arb` module

### `src/bin/mempool_monitor.rs` (-3 lines)
- Disabled broken imports (commented out)

### `src/bin/test_order_types.rs` (-1 line)
- Fixed import path

---

## Supported Assets

| Asset | WebSocket Feed | Market Slugs |
|-------|----------------|--------------|
| 🟠 **BTC** | `btcusdt@trade` | `btc-updown-5m-`, `btc-updown-15m-`, `btc-updown-4h-` |
| 🔵 **ETH** | `ethusdt@trade` | `eth-updown-5m-`, `eth-updown-15m-`, `eth-updown-4h-` |
| 🟣 **SOL** | `solusdt@trade` | `sol-updown-15m-`, `sol-updown-5m-`, `sol-updown-4h-` |
| ⚪ **XRP** | `xrpusdt@trade` | `xrp-updown-15m-`, `xrp-updown-5m-`, `xrp-updown-4h-` |

---

## Example Output

```
📡 Starting Binance BTC + ETH + SOL + XRP price feeds...
✅ Got initial BTC price: $95464.97
✅ Got initial ETH price: $3316.33
✅ Got initial SOL price: $187.45
✅ Got initial XRP price: $2.4521

📊 MULTI-MARKET MODE (4 assets):
   🟠 BTC: Bitcoin Up or Down - 12:00PM-12:15PM ET - Yes: 50.00¢
   🔵 ETH: Ethereum Up or Down - 12:00PM-12:15PM ET - Yes: 50.00¢
   🟣 SOL: Solana Up or Down - 12:00PM-12:15PM ET - Yes: 50.00¢
   ⚪ XRP: XRP Up or Down - 12:00PM-12:15PM ET - Yes: 50.00¢

🎯 Monitoring for arbitrage opportunities...

📈 BTC $95464⬆️+0.05% | ETH $3316⬆️+0.03% | SOL $187.4⬆️+0.08% | XRP $2.452⬇️-0.02% | T:0 O:0 | MOCK

🎰 SIGNAL: ⬆️ UP | BTC $95500.12 (+0.08%) | Buy @ 50.00¢ | Edge: 1.2% | Conf: 45%
   📝 [MOCK TRADE] 2026-01-17 19:06:00 UTC
      Market: Bitcoin Up or Down - 12:00PM-12:15PM ET (15m)
      Asset: BTC
      Direction: BUY YES (UP)
      Position Size: $5.00
```

---

## Commit History (35 commits)

| Commit | Description |
|--------|-------------|
| `9c83594` | Update CHANGELOG with SOL and XRP support |
| `f293c5d` | Add SOL and XRP 15-min market support (4 assets total) |
| `5be55fb` | Add multi-market trading docs to README |
| `00ccbb9` | Add multi-market trading: trade BTC and ETH simultaneously |
| `a6389b2` | Add momentum detection docs to README |
| `efa0271` | Add momentum detection: filter weak signals, boost strong momentum |
| `b2399d9` | Improve market refresh: check more frequently, detect decided markets |
| `588b79c` | Fix market selection: only pick undecided markets (YES 20-80¢) |
| `51ad1a5` | Fix reset_interval call |
| `32e8321` | Fix orderbook not found error: validate orderbook, auto-switch markets |
| `68f103a` | Disable mempool_monitor binary (broken imports) |
| `effb71b` | Fix mempool_monitor.rs imports |
| `bd3d617` | Fix test_order_types.rs import |
| `2f2bad7` | Fix rapid-fire trading: 2min between trades, 60s min hold |
| `a6e4dd4` | Fix P&L calculation and remove debug spam |
| `d1cdf91` | Lower edge thresholds for 50¢ markets |
| `ef278bb` | Add debug logging for edge calculation |
| `2650743` | Fix: btc_price -> crypto_price field rename |
| `2ac677d` | Fix: add missing asset field to fallback LiveCryptoMarket |
| `678f2ab` | Add ETH support: dual price feeds, ETH market detection |
| `994d80e` | Fix: don't reset interval on every market refresh |
| `b091779` | Add exit strategy with take profit, stop loss, time-based exits |
| `ae4c5a5` | Enhance mock trade logging with timestamps |
| `6c51abf` | Add market-type-specific thresholds for 5m, 15m, 4h markets |
| `9839959` | Add support for 5m, 4h, and price target BTC markets |
| `dd4cef5` | Improve market selection: pick best-priced market |
| `3dfc4cf` | Add verbose logging for arbitrage signal checks |
| `9be0163` | Fix: parse clobTokenIds as JSON string |
| `db68721` | Add debug logging for market discovery |
| `38c2607` | Implement pagination for btc-updown-15m markets |
| `7c2c91c` | Use known BTC price target event slug |
| `cfd78ad` | Add timestamp-based slug lookup |
| `9dd598d` | Try CLOB and strapi endpoints |
| `49d8d24` | Update API endpoints with btc-updown slug filter |
| `598359e` | Initial crypto latency arbitrage bot |

---

## Key Features

### 1. Momentum Detection
```
Score: -1.0 (strong down) to +1.0 (strong up)
- Tracks last 10 price samples
- Calculates acceleration (speeding up or slowing down)
- Measures consistency (all moves same direction?)
- Skips signals against momentum
- Boosts edge/confidence for strong momentum
```

### 2. Multi-Market Trading (4 Assets)
```
- Tracks BTC, ETH, SOL, and XRP markets separately
- Independent position management per asset
- Can hold up to 4 positions simultaneously (one per asset)
- Automatic market discovery and switching
- Real-time price feeds from Binance for all 4 assets
```

### 3. Exit Strategy
```
Take Profit: +15% (sell when winning)
Stop Loss:   -10% (cut losses)
Time Exit:   80% of interval (exit before resolution)
Min Hold:    60 seconds (avoid noise)
```

### 4. Market Selection
```
- Only trades undecided markets (YES price 20-80¢)
- Prefers markets closest to 50¢ (maximum edge)
- Refreshes every 3 seconds
- Auto-switches when markets become decided
```

---

## Environment Variables

| Variable | Default | Description |
|----------|---------|-------------|
| `PRIVATE_KEY` | required | Ethereum private key for signing |
| `FUNDER_ADDRESS` | required | Address that funds trades |
| `MOCK_TRADING` | `true` | Paper trading mode |
| `MAX_POSITION_USD` | `10.0` | Maximum position size |
| `MIN_POSITION_USD` | `1.0` | Minimum position size |

---

## Usage

```bash
# Mock trading (recommended for testing)
MOCK_TRADING=true cargo run --release --bin crypto_arb_bot

# Live trading
MOCK_TRADING=false cargo run --release --bin crypto_arb_bot

# Background with logging
nohup cargo run --release --bin crypto_arb_bot > crypto_arb.log 2>&1 &
tail -f crypto_arb.log
```

---

## Architecture

```
┌─────────────────────────────────────────────────────────────┐
│                    crypto_arb_bot.rs                        │
│  ┌─────────────┐  ┌─────────────┐  ┌─────────────────────┐  │
│  │ TradingState│  │   Config    │  │    Main Loop        │  │
│  │ - positions │  │ - mock mode │  │ - check signals     │  │
│  │ - P&L       │  │ - limits    │  │ - execute trades    │  │
│  │ - exits     │  │             │  │ - refresh markets   │  │
│  └─────────────┘  └─────────────┘  └─────────────────────┘  │
└─────────────────────────────────────────────────────────────┘
                              │
                              ▼
┌─────────────────────────────────────────────────────────────┐
│                      crypto_arb.rs                          │
│  ┌─────────────┐  ┌─────────────┐  ┌─────────────────────┐  │
│  │ PriceState  │  │CryptoArbEng │  │  Market Discovery   │  │
│  │ - BTC price │  │ - btc_market│  │ - Gamma API         │  │
│  │ - ETH price │  │ - eth_market│  │ - CLOB orderbook    │  │
│  │ - momentum  │  │ - signals   │  │ - price updates     │  │
│  └─────────────┘  └─────────────┘  └─────────────────────┘  │
│                                                             │
│  ┌─────────────────────────────────────────────────────┐    │
│  │              Binance WebSocket Feeds                │    │
│  │  wss://stream.binance.com/ws/btcusdt@trade          │    │
│  │  wss://stream.binance.com/ws/ethusdt@trade          │    │
│  └─────────────────────────────────────────────────────┘    │
└─────────────────────────────────────────────────────────────┘
```
