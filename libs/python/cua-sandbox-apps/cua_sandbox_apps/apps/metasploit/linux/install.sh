#!/bin/bash
set -e

echo "=========================================="
echo "Installing Metasploit Framework"
echo "=========================================="

# Stage 1: Install system dependencies
echo "Stage 1: Installing system dependencies..."
sudo apt-get update -qq
sudo apt-get install -y -qq \
    curl git wget \
    autoconf bison build-essential \
    libssl-dev libreadline-dev zlib1g-dev \
    libpcap-dev libyaml-dev \
    postgresql-client libpq-dev postgresql postgresql-contrib

# Stage 2: Install Ruby 3.1 (if not already installed)
if ! command -v ruby &> /dev/null || [ "$(ruby -v | grep -o '3\.[0-9]\.' | head -1)" != "3.1" ]; then
    echo "Stage 2: Installing Ruby 3.1..."
    
    cd /tmp
    if [ ! -f "ruby-3.1.4.tar.gz" ]; then
        echo "  Downloading Ruby 3.1.4..."
        wget -q https://cache.ruby-lang.org/pub/ruby/3.1/ruby-3.1.4.tar.gz
    fi
    
    if [ ! -d "ruby-3.1.4" ]; then
        echo "  Extracting Ruby..."
        tar xzf ruby-3.1.4.tar.gz
    fi
    
    cd ruby-3.1.4
    if [ ! -f "Makefile" ]; then
        echo "  Configuring Ruby..."
        ./configure --prefix=/opt/ruby31 --disable-install-doc --quiet
    fi
    
    if [ ! -f "/opt/ruby31/bin/ruby" ]; then
        echo "  Compiling Ruby (this may take 5-10 minutes)..."
        make -j$(nproc) -s
        echo "  Installing Ruby..."
        sudo make install -s
    fi
    
    # Create symlinks
    sudo ln -sf /opt/ruby31/bin/ruby /usr/local/bin/ruby 2>/dev/null || true
    sudo ln -sf /opt/ruby31/bin/gem /usr/local/bin/gem 2>/dev/null || true
    sudo ln -sf /opt/ruby31/bin/bundle /usr/local/bin/bundle 2>/dev/null || true
else
    echo "Stage 2: Ruby 3.1 already installed"
fi

# Stage 3: Clone and install Metasploit Framework
echo "Stage 3: Installing Metasploit Framework..."

if [ ! -d "/opt/metasploit-framework/.git" ]; then
    sudo mkdir -p /opt/metasploit-framework
    echo "  Cloning Metasploit Framework repository..."
    sudo git clone --depth 1 https://github.com/rapid7/metasploit-framework.git /tmp/msf-source
    sudo mv /tmp/msf-source/* /opt/metasploit-framework/ 2>/dev/null || true
    sudo mv /tmp/msf-source/.* /opt/metasploit-framework/ 2>/dev/null || true
fi

cd /opt/metasploit-framework

# Install Ruby gems
echo "  Installing Ruby dependencies..."
sudo gem install bundler -q
sudo bundle install --without development test 2>&1 | grep -E "(Bundle complete|error occurred)" | head -2

# Create convenient symlinks  
echo "  Setting up command symlinks..."
sudo ln -sf /opt/metasploit-framework/msfconsole /usr/local/bin/msfconsole 2>/dev/null || true
sudo ln -sf /opt/metasploit-framework/msfvenom /usr/local/bin/msfvenom 2>/dev/null || true

echo "=========================================="
echo "✓ Metasploit installation complete!"
echo "=========================================="

# Verify
echo ""
echo "Verification:"
echo "  Ruby: $(ruby --version)"
echo "  msfconsole: $(which msfconsole)"
echo ""
echo "To launch Metasploit, run: msfconsole"