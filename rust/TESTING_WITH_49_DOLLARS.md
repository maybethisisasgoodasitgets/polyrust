# Testing with $49 Balance - Configuration Guide

## Overview

You can absolutely test the new strategy with $49. The bot has been configured to work with minimal position sizes for testing.

## Position Sizing for $49 Balance

### Recommended Configuration

```bash
# In your .env file:
MAX_POSITION_USD=2.0      # Max $2 per trade
MIN_POSITION_USD=1.0      # Min $1 per trade
MOCK_TRADING=false        # Set to false for real testing
```

**With these settings:**
- You can take up to **24 trades** before running out ($49 / $2 = 24.5 trades)
- Each trade risks only **4% of your balance** ($2 / $49 = 4%)
- Minimum Polymarket order is $1, so we're using the absolute minimum

### Why $1-2 Position Sizes Work

1. **Meets Polymarket minimum** ($1 per order)
2. **Allows many test trades** (24+ attempts to validate strategy)
3. **Low risk per trade** (2-4% of balance)
4. **Real market conditions** (not paper trading)

## Expected Results with $49

### Conservative Estimate (Assuming Strategy Works)

| Scenario | Win Rate | Avg Gain | Result After 20 Trades |
|----------|----------|----------|------------------------|
| **Best Case** | 70% | +8% | $49 → $55 (+$6) |
| **Base Case** | 60% | +5% | $49 → $51 (+$2) |
| **Worst Case** | 50% | +0% | $49 → $49 (breakeven) |
| **If Strategy Fails** | 40% | -5% | $49 → $47 (-$2) |

**Maximum Loss:** $49 (if everything goes wrong)  
**Realistic Risk:** $2-5 (if strategy doesn't work)

## Telegram Notifications (IMPORTANT)

The bot will send you detailed notifications about:

### 1. Startup
```
🟢 Crypto Arb Bot Started

Mode: LIVE ($49 balance)
Position Size: $1-2 per trade
Filters: ENABLED

Monitoring: BTC, ETH, SOL, XRP
```

### 2. Low Balance Warning
```
⚠️ Low Balance Warning

Current: $49.00
Recommended: $100+

Using minimum position sizes for testing.
```

### 3. Signal Detection
```
🎯 Signal Detected

Asset: BTC
Velocity: 0.025%
Direction: UP

Validating filters...
```

### 4. Filter Validation (NEW!)
```
🔍 Filter Validation - BTC

📊 Filter Results:

✅ Momentum: PASS
✅ Orderbook: PASS (depth $850)
✅ Time: PASS (14:30 EST)

🎯 All filters PASSED - Taking trade
```

Or if filters reject:
```
🔍 Filter Validation - ETH

📊 Filter Results:

❌ Momentum: Momentum too weak: 0.25 < 0.40
✅ Orderbook: PASS (depth $920)
✅ Time: PASS (11:15 EST)

🚫 Trade REJECTED
```

### 5. Trade Execution
```
✅ LIVE Trade Executed

Asset: BTC
Direction: BUY YES (UP)
Entry: 52¢
Size: $1.50
Market: Bitcoin Up or Down 14:00-14:15 ET
```

### 6. Position Exit
```
💰 Position Closed

Asset: BTC
Entry: 52¢ → Exit: 58¢
P&L: +11.5% (+$0.17)
Hold Time: 8 minutes
```

## Server Deployment for Testing

### Prerequisites
```bash
# Make sure you have Rust installed on server
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source $HOME/.cargo/env
```

### Step 1: Upload to Server
```bash
# On your local machine:
cd /Users/reza/workspace/Polymarket-Copy-Trading-Bot
scp -r rust/ user@your-server:/home/user/polymarket-bot/
```

### Step 2: Configure on Server
```bash
# SSH into server
ssh user@your-server

cd /home/user/polymarket-bot/rust

# Create .env file with your settings
cat > .env << 'EOF'
PRIVATE_KEY=your_private_key_here
FUNDER_ADDRESS=your_wallet_address_here

# Position sizing for $49 testing
MAX_POSITION_USD=2.0
MIN_POSITION_USD=1.0

# Trading mode
MOCK_TRADING=false

# Telegram notifications (REQUIRED for monitoring)
TELEGRAM_BOT_TOKEN=your_bot_token_here
TELEGRAM_CHAT_ID=your_chat_id_here
EOF

chmod 600 .env  # Protect your private key
```

### Step 3: Build and Run
```bash
# Build release version (optimized)
cargo build --release --bin crypto_arb_bot

# Test the build
./target/release/crypto_arb_bot --help

# Run in background with logs
nohup ./target/release/crypto_arb_bot > crypto_arb.log 2>&1 &

# Get process ID
echo $! > crypto_arb.pid

# Check it's running
ps aux | grep crypto_arb_bot
```

### Step 4: Monitor Logs
```bash
# Follow real-time logs
tail -f crypto_arb.log

# Or filter for specific events
tail -f crypto_arb.log | grep "SIGNAL\|TRADE\|FILTER"
```

### Step 5: Stop Bot (When Needed)
```bash
# Get PID
cat crypto_arb.pid

# Stop gracefully
kill $(cat crypto_arb.pid)

# Or force stop
killall crypto_arb_bot
```

## Alternative: Using screen/tmux (Easier)

### Using screen (Recommended for Server)
```bash
# Start a screen session
screen -S polymarket_bot

# Inside screen, run the bot
cd /home/user/polymarket-bot/rust
cargo run --release --bin crypto_arb_bot

# Detach from screen (bot keeps running)
# Press: Ctrl+A then D

# Reattach later to check logs
screen -r polymarket_bot

# Kill screen session when done
screen -S polymarket_bot -X quit
```

### Using tmux
```bash
# Start tmux session
tmux new -s polymarket_bot

# Run bot
cargo run --release --bin crypto_arb_bot

# Detach: Ctrl+B then D
# Reattach: tmux attach -t polymarket_bot
```

## Monitoring Your $49 Test

### What to Watch For

1. **Filter Effectiveness** (via Telegram)
   - How many signals get detected?
   - How many pass filters?
   - What are common rejection reasons?

2. **Entry Prices**
   - Are we entering at good prices (50-55¢)?
   - Or bad prices (60-68¢) that caused losses before?

3. **Win Rate After 10 Trades**
   - If 6+ wins out of 10 = Strategy working ✅
   - If 4 or fewer wins out of 10 = Strategy not working ❌

4. **P&L Trend**
   - Gradually increasing? ✅ Keep going
   - Flat or decreasing? ❌ Stop and analyze

### When to Stop Testing

**Stop if:**
- Balance drops below $40 (-$9 = 18% loss)
- Win rate below 40% after 15+ trades
- Consistent losses on good setups (filters passing but losing anyway)

**Keep going if:**
- Win rate 55%+ after 10+ trades
- Balance stable or increasing
- Losses are small and expected (not every trade wins)

## Safety Checks Before Going Live

### Pre-Launch Checklist
- [ ] .env file configured correctly (private key, position sizes)
- [ ] TELEGRAM_BOT_TOKEN and TELEGRAM_CHAT_ID set
- [ ] MOCK_TRADING=false for real testing
- [ ] MAX_POSITION_USD=2.0, MIN_POSITION_USD=1.0
- [ ] Phone near you to receive Telegram notifications
- [ ] Server/computer will stay online (screen/tmux session)

### During First Hour
- [ ] Received startup notification on Telegram
- [ ] Bot is detecting price moves
- [ ] Filters are validating signals (seeing filter messages)
- [ ] No crashes or errors in logs

### After First Trade
- [ ] Trade executed successfully
- [ ] Position shows in Polymarket UI
- [ ] Entry price was reasonable (50-60¢)
- [ ] Telegram notification received

## Configuration for Different Test Styles

### Ultra-Conservative ($49, max protection)
```bash
MAX_POSITION_USD=1.0      # Only $1 per trade
MIN_POSITION_USD=1.0
```
- 49 possible trades
- 2% risk per trade
- Will take FOREVER to see results

### Moderate Testing ($49, recommended)
```bash
MAX_POSITION_USD=2.0      # $2 per trade
MIN_POSITION_USD=1.0
```
- 24 possible trades  
- 4% risk per trade
- Good balance of safety and speed

### Aggressive Testing ($49, faster results)
```bash
MAX_POSITION_USD=3.0      # $3 per trade
MIN_POSITION_USD=1.0
```
- 16 possible trades
- 6% risk per trade
- Faster validation but riskier

**Recommendation:** Start with Moderate ($2 per trade). You can adjust after 10 trades if needed.

## What Success Looks Like

### After 20 Trades with $49 Starting Balance:

**Good Result:**
```
Total Trades: 20
Wins: 13 (65%)
Losses: 7 (35%)
P&L: +$3.50 (+7.1%)
Balance: $52.50

Avg Win: +$0.42
Avg Loss: -$0.25
Filters working ✅
```

**Bad Result:**
```
Total Trades: 20
Wins: 7 (35%)
Losses: 13 (65%)
P&L: -$4.50 (-9.2%)
Balance: $44.50

Avg Win: +$0.35
Avg Loss: -$0.40
Filters not working ❌
```

## Next Steps After Testing

### If Strategy Works (60%+ win rate)
1. Add more capital ($100-200)
2. Increase position sizes to $5-10
3. Let it run for longer (week+)
4. Monitor results

### If Strategy Doesn't Work (<50% win rate)
1. Stop trading immediately
2. Review all trade logs
3. Analyze what went wrong:
   - Were entry prices too high?
   - Did momentum filter fail?
   - Wrong market timing?
4. Adjust strategy or try different approach

## Summary

✅ **$49 is enough to test** - Using $1-2 per trade  
✅ **Telegram notifications keep you informed** - Real-time filter decisions  
✅ **Server deployment is straightforward** - Use screen/tmux for persistence  
✅ **20 trades will validate strategy** - 60%+ win rate = working  
✅ **Maximum risk is limited** - Can't lose more than $49  

The new filters should prevent the mean reversion losses you had before. You'll get Telegram notifications for every signal showing exactly why trades were taken or rejected.

**Start with MOCK mode first if you want to be extra safe!**
