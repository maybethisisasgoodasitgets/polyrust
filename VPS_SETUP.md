# VPS Setup Guide - Crypto Arbitrage Bot

## Prerequisites
- VPS with the repository already cloned to `/root/polyrust`
- Rust installed (if not, see installation steps below)
- Polymarket wallet with USDC on Polygon network
- Alchemy or Chainstack API key

---

## Step 1: Install Rust (if not already installed)

```bash
# SSH into your VPS
ssh root@YOUR_VPS_IP

# Install Rust
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh

# Follow prompts (select option 1 for default)

# Reload shell or run:
source $HOME/.cargo/env

# Verify installation
rustc --version
cargo --version
```

---

## Step 2: Create and Configure .env File

### Navigate to the rust directory
```bash
cd /root/polyrust/rust
```

### Create .env file from example
```bash
cp .env.example .env
```

### Edit the .env file
```bash
nano .env
```

### Fill in the REQUIRED values:

```bash
# ============================================================================
# REQUIRED SETTINGS (Must fill these in)
# ============================================================================

# Your wallet's private key (64-character hex, NO 0x prefix)
# KEEP THIS SECRET!
PRIVATE_KEY=your_64_character_private_key_here

# Your wallet address (40-character hex, with or without 0x prefix)
FUNDER_ADDRESS=0xyour_40_character_wallet_address_here

# Alchemy API key (get from https://www.alchemy.com/)
ALCHEMY_API_KEY=your_alchemy_api_key_here

# ============================================================================
# TRADING SETTINGS
# ============================================================================

# Enable actual trading (true) or monitoring only (false)
ENABLE_TRADING=true

# Mock trading mode - simulates without executing
# Start with true for testing, then set to false for live trading
MOCK_TRADING=true
```

**Save and exit:** Press `Ctrl+X`, then `Y`, then `Enter`

---

## Step 3: Get Your Required Credentials

### A. Get Your Private Key
1. Open MetaMask
2. Click account menu → Account Details → Show Private Key
3. Enter password and copy the key
4. **Remove the `0x` prefix if present**

### B. Get Your Wallet Address
1. Copy your address from MetaMask (starts with 0x)
2. You can keep the `0x` prefix for `FUNDER_ADDRESS`

### C. Get Alchemy API Key
1. Go to https://www.alchemy.com/
2. Sign up for free account
3. Create a new app:
   - Chain: **Polygon PoS**
   - Network: **Polygon Mainnet**
4. Copy the API key

---

## Step 4: Validate Your Configuration

Before running the bot, test your setup:

```bash
cd /root/polyrust/rust
cargo run --release --bin validate_setup
```

This will check if your configuration is correct and show helpful errors if something is wrong.

---

## Step 5: Test Run (RECOMMENDED FIRST)

Run in **mock mode** to see what the bot would do without real trading:

```bash
cd /root/polyrust/rust

# Make sure MOCK_TRADING=true in your .env file
cargo run --release --bin crypto_arb_bot
```

Watch the output to ensure it's working correctly. Press `Ctrl+C` to stop.

---

## Step 6: Run the Bot in Production

### Option A: Run in Foreground (for testing)
```bash
cd /root/polyrust/rust

# Set MOCK_TRADING=false in .env for real trading
cargo run --release --bin crypto_arb_bot
```

### Option B: Run in Background (recommended for production)

```bash
cd /root/polyrust/rust

# Build the release binary first
cargo build --release --bin crypto_arb_bot

# Run in background with nohup
nohup target/release/crypto_arb_bot > crypto_arb.log 2>&1 &

# Save the process ID
echo $! > bot.pid
```

---

## Step 7: Use the Automated Deployment Script

The repository includes a deployment script that automates everything:

```bash
cd /root/polyrust

# Make it executable (first time only)
chmod +x deploy.sh

# Run deployment (includes tests)
./deploy.sh

# Or skip tests for faster deployment
./deploy.sh --skip-tests
```

**What the script does:**
- Stops existing bot process
- Pulls latest code from git
- Builds release binary
- Runs tests (optional)
- Archives old log file
- Starts bot in background

---

## Monitoring and Management

### Check if bot is running
```bash
ps aux | grep crypto_arb_bot | grep -v grep
```

### View live logs
```bash
tail -f /root/polyrust/rust/crypto_arb.log
```

### View recent logs
```bash
tail -100 /root/polyrust/rust/crypto_arb.log
```

### Stop the bot
```bash
pkill -f crypto_arb_bot
```

### Restart the bot
```bash
# Stop it first
pkill -f crypto_arb_bot

# Wait a moment
sleep 2

# Start again
cd /root/polyrust/rust
nohup target/release/crypto_arb_bot > crypto_arb.log 2>&1 &
```

---

## Important Security Notes

⚠️ **CRITICAL:**
- **NEVER** share your private key with anyone
- **NEVER** commit your `.env` file to git
- Start with **small amounts** to test
- Use **MOCK_TRADING=true** first to verify everything works
- Keep your VPS secure with SSH keys and firewall rules

---

## Troubleshooting

### Build fails
```bash
# Check Rust version
rustc --version

# Update Rust
rustup update

# Try clean build
cd /root/polyrust/rust
cargo clean
cargo build --release --bin crypto_arb_bot
```

### Connection errors
- Verify your Alchemy API key is correct
- Check your VPS has internet access: `curl -I https://polygon-rpc.com`
- Ensure Polygon mainnet is selected in Alchemy dashboard

### Bot not trading
- Verify `ENABLE_TRADING=true` in `.env`
- Verify `MOCK_TRADING=false` in `.env`
- Check you have USDC in your wallet
- Check logs for errors: `tail -100 /root/polyrust/rust/crypto_arb.log`

### Out of memory
- Upgrade VPS plan
- Check for multiple bot instances: `ps aux | grep crypto_arb_bot`

---

## Quick Reference Commands

```bash
# SSH into VPS
ssh root@YOUR_VPS_IP

# Navigate to project
cd /root/polyrust/rust

# Edit .env file
nano .env

# Validate config
cargo run --release --bin validate_setup

# Run bot (foreground)
cargo run --release --bin crypto_arb_bot

# Run bot (background)
nohup target/release/crypto_arb_bot > crypto_arb.log 2>&1 &

# Check status
ps aux | grep crypto_arb_bot | grep -v grep

# View logs
tail -f crypto_arb.log

# Stop bot
pkill -f crypto_arb_bot

# Deploy with script
cd /root/polyrust && ./deploy.sh --skip-tests
```

---

## Next Steps After Setup

1. **Start with Mock Trading** - Test without risk
2. **Monitor for 1-2 days** - Watch logs and behavior
3. **Start Small** - Use minimal USDC for first real trades
4. **Scale Gradually** - Increase capital as you gain confidence
5. **Monitor Daily** - Check logs and performance regularly

---

## Support

For issues or questions:
- Telegram: [@terauss](https://t.me/terauss)
- Check `/root/polyrust/rust/docs/` for detailed documentation
