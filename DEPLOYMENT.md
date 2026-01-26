# Deployment Guide

## Automated Deployment Script

The `deploy.sh` script automates the entire deployment process for the crypto arbitrage bot.

### What It Does

1. **Stops** existing bot process
2. **Pulls** latest code from git (crypto-arbitrage branch)
3. **Builds** release binary
4. **Tests** the code (optional, can skip with `--skip-tests`)
5. **Archives** old log file
6. **Starts** bot in background with nohup

### Usage

#### On Server

```bash
# First time: Upload and make executable
scp deploy.sh root@155.138.132.61:/root/polyrust/
ssh root@155.138.132.61 'chmod +x /root/polyrust/deploy.sh'

# Run deployment (with tests)
ssh root@155.138.132.61 '/root/polyrust/deploy.sh'

# Run deployment (skip tests for faster deploy)
ssh root@155.138.132.61 '/root/polyrust/deploy.sh --skip-tests'
```

#### From Local Machine (One Command)

```bash
# With tests
ssh root@155.138.132.61 'cd /root/polyrust && ./deploy.sh'

# Skip tests (faster)
ssh root@155.138.132.61 'cd /root/polyrust && ./deploy.sh --skip-tests'
```

### Features

- **Color-coded output** for easy reading
- **Error handling** - stops on any failure
- **Git change detection** - shows what changed
- **Log archiving** - old logs saved with timestamp
- **Process verification** - confirms bot actually started
- **Live output** - shows last 30 lines of bot log after startup

### Log Files

- `crypto_arb.log` - Current bot log
- `crypto_arb.log.YYYYMMDD_HHMMSS.old` - Archived logs
- `build.log` - Build output
- `test.log` - Test output

### Troubleshooting

If deployment fails, check:
1. Git pull errors → Check network/credentials
2. Build errors → Check `build.log` in `/root/polyrust/rust/`
3. Test errors → Check `test.log` 
4. Bot startup errors → Check `crypto_arb.log`

### Manual Commands (if needed)

```bash
# Check if bot is running
ps aux | grep crypto_arb_bot | grep -v grep

# Stop bot manually
pkill -f crypto_arb_bot

# View live logs
tail -f /root/polyrust/rust/crypto_arb.log

# View recent logs
tail -100 /root/polyrust/rust/crypto_arb.log
```

### Quick Deploy from Local

You can also create a local alias to make deployment even easier:

```bash
# Add to your ~/.zshrc or ~/.bashrc
alias deploy-bot='ssh root@155.138.132.61 "cd /root/polyrust && ./deploy.sh --skip-tests"'

# Then just run:
deploy-bot
```
