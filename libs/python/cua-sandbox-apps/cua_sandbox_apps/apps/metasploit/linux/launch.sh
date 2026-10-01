#!/bin/bash

# Launch script for Metasploit Framework
# Starts msfconsole in interactive mode

# Set up Ruby path
export PATH="/opt/ruby31/bin:/opt/metasploit-framework:$PATH"
export LD_LIBRARY_PATH="/opt/ruby31/lib:$LD_LIBRARY_PATH"

# Change to Metasploit directory
cd /opt/metasploit-framework

# Launch msfconsole with full interactive interface
/opt/metasploit-framework/msfconsole