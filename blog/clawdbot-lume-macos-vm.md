# You Don't Need to Buy a Mac Mini to Run Clawdbot

_Published on February 3, 2026 by the Cua Team_

Honestly, we've been the open-source infrastructure for computer-use agents for about a year now—we started with macOS sandboxing and just hit the Show HN front page last Sunday. We're very bullish on Claude Code and CLI-first approaches at Cua (announcement coming soon, hopefully next week). So I thought I'd hack something together to show how it all connects.

And here's the thing: you probably don't want to rush to your closest Best Buy.

If you've got an Apple Silicon Mac sitting on your desk—MacBook, iMac, Mac Studio, whatever—you can run Clawdbot in an isolated macOS VM using Lume.

## Why This Works

Lume uses Apple's Virtualization Framework to run macOS VMs on Apple Silicon devices (M1+). You get:

- **Full macOS environment inside a VM** (iMessage, native apps, everything)
- **Complete isolation** from your host system
- **Headless operation** so you can keep using your Mac normally
- **Instant reset and portability** by cloning VMs

Your Mac pulls double duty as your daily driver and Clawdbot server.

## Quick Setup

### 1. Install Lume

```bash
/bin/bash -c "$(curl -fsSL https://raw.githubusercontent.com/trycua/cua/main/libs/lume/scripts/install.sh)"
```

If `~/.local/bin` isn't in your PATH, add it:

```bash
echo 'export PATH="$PATH:$HOME/.local/bin"' >> ~/.zshrc && source ~/.zshrc
```

### 2. Create a macOS VM

```bash
lume create clawdbot --os macos --ipsw latest
```

This downloads macOS and creates the VM. A VNC window opens—complete the Setup Assistant to create your user account and enable Remote Login (SSH) in System Settings.

> **Tip:** For automated setup, check out [unattended installation](https://cua.ai/docs/lume/guide/getting-started/quickstart) in the Lume docs.

### 3. Run headlessly

Once setup is complete, you can run without display:

```bash
lume run clawdbot --no-display
```

### 4. SSH into the VM

```bash
# Get the IP address
lume get clawdbot
# Look for the IP (usually 192.168.64.x)

# SSH in with the user you created
ssh youruser@192.168.64.X
```

### 5. Install Clawdbot inside the VM

```bash
npm install -g clawdbot@latest
clawdbot onboard --install-daemon
```

### 6. Configure your channels

Edit `~/.clawdbot/clawdbot.json` to add your channels:

```json
{
  "channels": {
    "whatsapp": {
      "dmPolicy": "allowlist",
      "allowFrom": ["+15551234567"]
    },
    "telegram": {
      "botToken": "YOUR_BOT_TOKEN"
    }
  }
}
```

### 7. Login and start

```bash
clawdbot channels login   # scan WhatsApp QR
```

The gateway runs automatically via the daemon you installed. Done—Clawdbot running in a sandboxed macOS VM on your existing hardware.

## Bonus: iMessage Integration

This is the flex. With BlueBubbles, you can add iMessage to your Clawdbot setup—something literally impossible on Linux or Raspberry Pi.

### 1. Install BlueBubbles in the VM

Download from [bluebubbles.app](https://bluebubbles.app), sign in with your Apple ID, and enable the Web API.

### 2. Add to your Clawdbot config

```json
{
  "channels": {
    "bluebubbles": {
      "serverUrl": "http://localhost:1234",
      "password": "your-api-password"
    }
  }
}
```

### 3. Restart the gateway

Now your AI agent can send and receive iMessages—reactions, read receipts, group chats, the works.

**This is the killer feature that justifies running on macOS instead of a cheap Linux box.**

## FAQ

### Can I use a MacBook Pro instead of a Mac Mini?

Yes. Any Apple Silicon Mac works—MacBook Air, MacBook Pro, iMac, Mac Studio, Mac Mini. Lume runs on all of them. Your laptop can run Clawdbot in a VM while you use it for other things.

### Does it work on M1/M2/M3, or only M4?

All Apple Silicon works. M1, M2, M3, M4—doesn't matter. That dusty MacBook Air from 2020? Yep, that works too.

### Can I run this 24/7 on a laptop?

Yes, but keep it plugged in. The VM runs headlessly in the background. Your Mac won't sleep while it's running (or configure Energy Saver to prevent sleep).

### What about Windows?

Clawdbot runs on Windows/Linux, but you lose the good stuff—iMessage, BlueBubbles, native macOS apps. If you want the full experience, you need macOS. Lume gets you there without buying another machine.

### What about Raspberry Pi?

Same deal—Clawdbot runs on Linux, but no iMessage. Pi is fine for WhatsApp/Telegram only. For the full experience, you need macOS.

## What About Cloud?

Look, cloud has its place. If you need true always-on and don't want to keep your laptop running, dedicated Mac instances make sense.

But here's the thing—Lume works there too. Spin up an EC2 dedicated Mac host and use Lume to run multiple macOS VMs on that single machine instead of paying for one instance per bot.

That said, AWS Mac instances run ~$1/hour minimum—that's $720/month just to keep one on. Meanwhile your Mac at home is already paid for and probably not even breaking a sweat.

## The Sandbox Advantage

Running in a VM means:

- Clawdbot can't touch your host filesystem
- Credentials stay isolated
- Something breaks? Nuke the VM, clone from backup, you're back in business
- Your main macOS stays squeaky clean

## Save a Golden Image

Before you customize, snapshot it:

```bash
lume stop clawdbot
lume clone clawdbot clawdbot-golden
```

Reset anytime:

```bash
lume stop clawdbot && lume delete clawdbot
lume clone clawdbot-golden clawdbot
lume run clawdbot --no-display
```

---

## Links

- [Lume Repo](https://github.com/trycua/cua)
- [Lume Installation](https://cua.ai/docs/lume/guide/getting-started/installation)
- [Lume Quickstart](https://cua.ai/docs/lume/guide/getting-started/quickstart)
- [Clawdbot Documentation](https://docs.clawd.bot)
