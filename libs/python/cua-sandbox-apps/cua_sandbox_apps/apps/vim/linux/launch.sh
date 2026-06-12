#!/bin/bash
# Simple launch script - starts gvim in background
export DISPLAY=:0
gvim /tmp/vim_test.txt > /dev/null 2>&1 &
sleep 2