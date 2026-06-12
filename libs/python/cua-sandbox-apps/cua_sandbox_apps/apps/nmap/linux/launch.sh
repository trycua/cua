#!/bin/bash
# Nmap launch script for Linux

# Launch Nmap in a visible terminal window with xfce4-terminal
# Using --hold to keep the terminal open after command completes
# DISPLAY=:1 is the X11 display used by the XFCE desktop environment
DISPLAY=:1 xfce4-terminal --hold --execute /usr/bin/nmap --help